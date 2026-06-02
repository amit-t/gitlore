//! SHA prefix lookup for commit search.
//!
//! Provides [`try_sha_lookup`] for resolving abbreviated commit SHAs against
//! the indexed commits table. Returns structured outcomes for exact matches,
//! ambiguous prefixes, and not-found cases.

use regex::Regex;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// Minimal commit representation for search results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Full commit SHA (lowercase hex).
    pub sha: String,
    /// Commit subject line.
    pub subject: String,
}

/// Outcome of a SHA prefix lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShaLookupOutcome {
    /// The prefix resolved to exactly one commit.
    Resolved(SearchHit),
    /// The prefix matched multiple commits (ambiguous).
    Ambiguous {
        /// The prefix that was queried.
        prefix: String,
        /// Number of matching commits.
        matches: usize,
    },
    /// No commits matched the prefix.
    NotFound {
        /// The prefix that was queried.
        prefix: String,
    },
}

/// Attempt to resolve a SHA prefix against the commits table.
///
/// # Arguments
///
/// * `conn` - SQLite connection to the index database
/// * `q` - Query string (SHA prefix, case-insensitive)
///
/// # Returns
///
/// * `Some(ShaLookupOutcome)` if the input matches the SHA format
/// * `None` if the input does not match the SHA format
///
/// # Behavior
///
/// 1. Lowercases the input
/// 2. Validates against regex `^[0-9a-f]{4,40}$` (4-40 hex characters)
/// 3. Queries commits table with `SELECT sha, subject FROM commits WHERE sha LIKE 'prefix%' LIMIT 2`
/// 4. Returns:
///    - `Resolved(SearchHit)` if exactly one commit matches
///    - `Ambiguous { prefix, matches }` if multiple commits match
///    - `NotFound { prefix }` if no commits match
pub fn try_sha_lookup(conn: &Connection, q: &str) -> Option<Result<ShaLookupOutcome>> {
    let needle = q.to_ascii_lowercase();
    let sha_regex = Regex::new(r"^[0-9a-f]{4,40}$").unwrap();

    if !sha_regex.is_match(&needle) {
        return None;
    }

    let like_pattern = format!("{needle}%");

    let stmt = conn
        .prepare("SELECT sha, subject FROM commits WHERE sha LIKE ?1 LIMIT 2")
        .map_err(|e| Error::Sqlite(e.to_string()));

    let mut stmt = match stmt {
        Ok(s) => s,
        Err(e) => return Some(Err(e)),
    };

    let rows = stmt
        .query_map([&like_pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(|e| Error::Sqlite(e.to_string()));

    let rows = match rows {
        Ok(r) => r,
        Err(e) => return Some(Err(e)),
    };

    let mut matches: Vec<(String, String)> = Vec::new();
    for r in rows {
        match r {
            Ok(sha_subject) => matches.push(sha_subject),
            Err(e) => return Some(Err(Error::Sqlite(e.to_string()))),
        }
    }

    let outcome = match matches.len() {
        0 => ShaLookupOutcome::NotFound { prefix: needle },
        1 => {
            let (sha, subject) = matches.into_iter().next().unwrap();
            ShaLookupOutcome::Resolved(SearchHit { sha, subject })
        }
        n => ShaLookupOutcome::Ambiguous {
            prefix: needle,
            matches: n,
        },
    };

    Some(Ok(outcome))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::migrations;
    use crate::index::schema::serialize_file_changes;
    use rusqlite::params;

    fn insert_test_commit(conn: &Connection, sha: &str, subject: &str) {
        let files_json = serialize_file_changes(&[]);
        conn.execute(
            "INSERT INTO commits ( \
                 sha, author_name, author_email, author_identity_id, \
                 committer_name, committer_email, committer_identity_id, \
                 authored_at, committed_at, authored_tz_offset, committed_tz_offset, \
                 subject, body, expanded, parent_shas, parent_count, is_merge, is_root, \
                 files_changed, file_count, insertions, deletions, dirs_touched, dir_count, \
                 test_files_changed, config_files_changed, infra_files_changed, doc_files_changed, \
                 code_files_changed, dependency_files_changed, ci_files_changed, fixture_files_changed, \
                 migration_files_changed, is_revert, reverted_by_sha, risk_score, risk_label, \
                 admission_signals, story_id, indexed_at, updated_at \
             ) VALUES ( \
                 ?1, 'x', 'x@x', NULL, 'x', 'x@x', NULL, 0, 0, 0, 0, ?2, 'b', 's', '[]', 0, 0, 0, \
                 ?3, 0, 0, 0, '[]', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, NULL, NULL, NULL, '{}', NULL, 0, 0 \
             )",
            params![sha, subject, files_json],
        )
        .unwrap();
    }

    fn setup_test_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        migrations::migrate(&mut conn).unwrap();
        conn
    }

    #[test]
    fn try_sha_lookup_returns_none_for_invalid_format() {
        let conn = setup_test_db();
        
        // Too short
        assert!(try_sha_lookup(&conn, "abc").is_none());
        
        // Too long
        assert!(try_sha_lookup(&conn, &"a".repeat(41)).is_none());
        
        // Invalid characters
        assert!(try_sha_lookup(&conn, "ghijkl").is_none());
        
        // Mixed case (should still match after lowercasing)
        assert!(try_sha_lookup(&conn, "ABCD1234").is_some());
    }

    #[test]
    fn try_sha_lookup_resolves_exact_match() {
        let conn = setup_test_db();
        insert_test_commit(&conn, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111", "Test commit");

        let result = try_sha_lookup(&conn, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
        assert!(result.is_some());

        let outcome = result.unwrap().unwrap();
        match outcome {
            ShaLookupOutcome::Resolved(hit) => {
                assert_eq!(hit.sha, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
                assert_eq!(hit.subject, "Test commit");
            }
            _ => panic!("Expected Resolved, got {:?}", outcome),
        }
    }

    #[test]
    fn try_sha_lookup_resolves_unique_prefix() {
        let conn = setup_test_db();
        insert_test_commit(&conn, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111", "Test commit");

        let result = try_sha_lookup(&conn, "aaaa1111");
        assert!(result.is_some());

        let outcome = result.unwrap().unwrap();
        match outcome {
            ShaLookupOutcome::Resolved(hit) => {
                assert_eq!(hit.sha, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
                assert_eq!(hit.subject, "Test commit");
            }
            _ => panic!("Expected Resolved, got {:?}", outcome),
        }
    }

    #[test]
    fn try_sha_lookup_returns_ambiguous_for_multiple_matches() {
        let conn = setup_test_db();
        insert_test_commit(&conn, "abcd1234abcd1234abcd1234abcd1234abcd1234", "First commit");
        insert_test_commit(&conn, "abcd5678abcd5678abcd5678abcd5678abcd5678", "Second commit");

        let result = try_sha_lookup(&conn, "abcd");
        assert!(result.is_some());

        let outcome = result.unwrap().unwrap();
        match outcome {
            ShaLookupOutcome::Ambiguous { prefix, matches } => {
                assert_eq!(prefix, "abcd");
                assert_eq!(matches, 2);
            }
            _ => panic!("Expected Ambiguous, got {:?}", outcome),
        }
    }

    #[test]
    fn try_sha_lookup_returns_not_found_for_no_matches() {
        let conn = setup_test_db();
        // No commits inserted

        let result = try_sha_lookup(&conn, "ffff9999");
        assert!(result.is_some());

        let outcome = result.unwrap().unwrap();
        match outcome {
            ShaLookupOutcome::NotFound { prefix } => {
                assert_eq!(prefix, "ffff9999");
            }
            _ => panic!("Expected NotFound, got {:?}", outcome),
        }
    }

    #[test]
    fn try_sha_lookup_lowercases_input() {
        let conn = setup_test_db();
        insert_test_commit(&conn, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111", "Test commit");

        // Uppercase input should still work
        let result = try_sha_lookup(&conn, "AAAA1111AAAA1111");
        assert!(result.is_some());

        let outcome = result.unwrap().unwrap();
        match outcome {
            ShaLookupOutcome::Resolved(hit) => {
                assert_eq!(hit.sha, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
            }
            _ => panic!("Expected Resolved, got {:?}", outcome),
        }
    }

    #[test]
    fn try_sha_lookup_accepts_minimum_length() {
        let conn = setup_test_db();
        insert_test_commit(&conn, "abcd1234abcd1234abcd1234abcd1234abcd1234", "Test commit");

        // 4 characters is the minimum
        let result = try_sha_lookup(&conn, "abcd");
        assert!(result.is_some());
    }

    #[test]
    fn try_sha_lookup_accepts_maximum_length() {
        let conn = setup_test_db();
        insert_test_commit(&conn, "abcd1234abcd1234abcd1234abcd1234abcd1234", "Test commit");

        // 40 characters is the maximum (full SHA)
        let result = try_sha_lookup(&conn, "abcd1234abcd1234abcd1234abcd1234abcd1234");
        assert!(result.is_some());
    }
}
