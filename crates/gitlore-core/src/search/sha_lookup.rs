//! SHA-prefix bypass for the search orchestrator (TDD-001 §2.1 / AC-SEARCH-3).
//!
//! When a query looks like a hex SHA (4-40 lowercase hex chars after
//! lowercasing the input — grill #11 B), we skip FTS5 entirely and do a
//! direct prefix lookup against `commits.sha`.

use rusqlite::Connection;

use crate::error::{Error, Result};
use crate::search::types::{Factors, SearchHit, SearchMode};

/// Pattern that matches a SHA-shaped query (4-40 hex chars, after lowercasing).
const SHA_REGEX: &str = r"^[0-9a-f]{4,40}$";

/// Outcome of a SHA-prefix lookup.
#[derive(Debug)]
pub enum ShaLookupOutcome {
    /// The prefix resolved to exactly one commit.
    Resolved(SearchHit),
    /// The prefix matched more than one commit.
    Ambiguous {
        /// The hex prefix that was looked up.
        prefix: String,
        /// All SHAs that matched the prefix (at least 2 entries).
        matches: Vec<String>,
    },
    /// The prefix matched no commits.
    NotFound {
        /// The hex prefix that was looked up.
        prefix: String,
    },
}

/// Try to interpret `q` as a SHA prefix and do a direct lookup.
///
/// Returns:
/// * `Ok(None)` — `q` does not look like a hex SHA; caller should proceed
///   with the FTS5 path.
/// * `Ok(Some(outcome))` — `q` is SHA-shaped; outcome describes the result.
/// * `Err(_)` — database error.
pub fn try_sha_lookup(conn: &Connection, q: &str) -> Result<Option<ShaLookupOutcome>> {
    // Lowercase first (grill #11 B) so mixed-case pasted SHAs resolve.
    let lower = q.to_lowercase();

    // Check against the SHA pattern.
    let re = regex::Regex::new(SHA_REGEX).expect("static regex is valid");
    if !re.is_match(&lower) {
        return Ok(None);
    }

    let prefix = lower.clone();
    let like_pattern = format!("{prefix}%");

    // Fetch up to 2 rows to distinguish Resolved vs Ambiguous.
    let mut stmt = conn
        .prepare_cached(
            "SELECT sha, subject, author_name, author_email, committed_at \
             FROM commits \
             WHERE sha LIKE ?1 \
             LIMIT 2",
        )
        .map_err(|e| Error::Sqlite(e.to_string()))?;

    struct Row {
        sha: String,
        subject: String,
        author_name: String,
        committed_at: i64,
    }

    let rows: Vec<Row> = stmt
        .query_map([&like_pattern], |r| {
            Ok(Row {
                sha: r.get(0)?,
                subject: r.get(1)?,
                author_name: r.get(2)?,
                committed_at: r.get(4)?,
            })
        })
        .map_err(|e| Error::Sqlite(e.to_string()))?
        .collect::<std::result::Result<_, _>>()
        .map_err(|e: rusqlite::Error| Error::Sqlite(e.to_string()))?;

    match rows.len() {
        0 => Ok(Some(ShaLookupOutcome::NotFound { prefix })),
        1 => {
            let r = rows.into_iter().next().unwrap();
            let hit = SearchHit {
                sha: r.sha,
                subject: r.subject,
                author: r.author_name,
                committed_at: r.committed_at,
                // SHA lookup has perfect relevance; factors are nominal.
                score: 1.0,
                factors: Factors {
                    lexical_bm25: 0.0,
                    path_relevance: 0.0,
                    recency: 0.0,
                    semantic: None,
                },
            };
            Ok(Some(ShaLookupOutcome::Resolved(hit)))
        }
        _ => {
            let matches: Vec<String> = rows.into_iter().map(|r| r.sha).collect();
            Ok(Some(ShaLookupOutcome::Ambiguous { prefix, matches }))
        }
    }
}

/// Map `ShaLookupOutcome` to the `SearchMode` that the orchestrator records.
pub fn outcome_mode(_outcome: &ShaLookupOutcome) -> SearchMode {
    SearchMode::ShaLookup
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE commits (
                sha TEXT PRIMARY KEY,
                subject TEXT NOT NULL,
                author_name TEXT NOT NULL,
                author_email TEXT NOT NULL,
                committed_at INTEGER NOT NULL,
                dirs_touched TEXT
            );",
        )
        .unwrap();
        conn
    }

    fn insert(conn: &Connection, sha: &str, subject: &str) {
        conn.execute(
            "INSERT INTO commits (sha, subject, author_name, author_email, committed_at) \
             VALUES (?1, ?2, 'test', 'test@example.com', 1000)",
            [sha, subject],
        )
        .unwrap();
    }

    #[test]
    fn non_sha_query_returns_none() {
        let conn = setup_db();
        assert!(try_sha_lookup(&conn, "hello world").unwrap().is_none());
        assert!(try_sha_lookup(&conn, "fix: things").unwrap().is_none());
        assert!(try_sha_lookup(&conn, "abc").unwrap().is_none()); // 3 chars, too short
    }

    #[test]
    fn mixed_case_sha_normalizes() {
        let conn = setup_db();
        let sha = "deadbeefcafe0000000000000000000000000000";
        insert(&conn, sha, "test commit");
        // Mixed-case prefix should still resolve.
        let outcome = try_sha_lookup(&conn, "DeadBeef").unwrap();
        assert!(matches!(outcome, Some(ShaLookupOutcome::Resolved(_))));
    }

    #[test]
    fn resolved_on_unique_prefix() {
        let conn = setup_db();
        let sha = "aaaa1111000000000000000000000000000000ab";
        insert(&conn, sha, "fix: retry on timeout");
        let outcome = try_sha_lookup(&conn, "aaaa1111").unwrap().unwrap();
        match outcome {
            ShaLookupOutcome::Resolved(hit) => {
                assert_eq!(hit.sha, sha);
                assert_eq!(hit.subject, "fix: retry on timeout");
            }
            other => panic!("expected Resolved, got {other:?}"),
        }
    }

    #[test]
    fn ambiguous_on_shared_prefix() {
        let conn = setup_db();
        insert(
            &conn,
            "aaaa0000000000000000000000000000000000a1",
            "commit a",
        );
        insert(
            &conn,
            "aaaa0000000000000000000000000000000000a2",
            "commit b",
        );
        let outcome = try_sha_lookup(&conn, "aaaa0000").unwrap().unwrap();
        assert!(matches!(outcome, ShaLookupOutcome::Ambiguous { .. }));
    }

    #[test]
    fn not_found_on_no_match() {
        let conn = setup_db();
        let outcome = try_sha_lookup(&conn, "deadbeef").unwrap().unwrap();
        assert!(matches!(outcome, ShaLookupOutcome::NotFound { .. }));
    }

    #[test]
    fn exact_40_char_sha_is_accepted() {
        let conn = setup_db();
        let sha = "abcdef0123456789abcdef0123456789abcdef01";
        insert(&conn, sha, "full sha");
        let outcome = try_sha_lookup(&conn, sha).unwrap().unwrap();
        assert!(matches!(outcome, ShaLookupOutcome::Resolved(_)));
    }
}
