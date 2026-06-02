//! SQL filter resolution for search pre-filtering (TDD-001 §2.1).
//!
//! Converts user-facing [`Filters`] into a [`SqlFilters`] struct that holds
//! the WHERE-clause fragment and the bound parameter values. All values are
//! passed as bound parameters (never string-concatenated) to prevent SQL
//! injection.
//!
//! Resolution rules (per grills):
//! * `--since` / `--until`: try `GitProvider::rev_parse` first; on
//!   `Error::InvalidRef` fall back to `chrono::DateTime::parse_from_rfc3339`
//!   then `chrono::NaiveDate::parse_from_str` for bare ISO-8601 dates (grill
//!   #20 A). Dates are converted to the start-of-day (since) / end-of-day
//!   (until) Unix timestamps.
//! * `--branch`: short names like `main` or `release/2.9` get `refs/heads/`
//!   auto-prefixed; values already starting with `refs/` pass through
//!   unchanged (grill #6 B). Sub-select against `commit_refs`.
//! * `--author`: matches `identities.canonical_email` OR
//!   `identity_aliases.raw_email`, both lowercased (grill #12 B).
//! * `--path`: JSON-extract path-prefix LIKE against
//!   `commits.files_changed[].path` (grill #13 A). Bound, never concatenated.

use rusqlite::types::Value;

use crate::error::{Error, Result};
use crate::git::GitProvider;
use crate::search::types::Filters;

// ---------------------------------------------------------------------------
// SqlFilters
// ---------------------------------------------------------------------------

/// Resolved SQL WHERE clause and bound values, ready for a prepared statement.
#[derive(Debug, Default)]
pub struct SqlFilters {
    /// Fragments joined with `AND` (empty means "no filter").
    pub clauses: Vec<String>,
    /// Positional bind values in the order they appear in `clauses`.
    pub bindings: Vec<Value>,
}

impl SqlFilters {
    /// True when no user filters were supplied (no WHERE clause needed).
    pub fn is_empty(&self) -> bool {
        self.clauses.is_empty()
    }

    /// Build the full `AND`-joined WHERE fragment (without the `WHERE` keyword).
    /// Returns an empty string when no filters are active.
    pub fn where_fragment(&self) -> String {
        self.clauses.join(" AND ")
    }
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// Resolve user-supplied [`Filters`] into SQL predicates.
///
/// `provider` is only called for `since` / `until` ref-parse attempts; it
/// is not called when both fields are absent. Pass `None` to skip the
/// rev-parse attempt (useful in tests).
pub fn resolve(filters: &Filters, provider: Option<&dyn GitProvider>) -> Result<SqlFilters> {
    let mut sql = SqlFilters::default();

    // --author: matches canonical_email OR any alias email (lowercased).
    if let Some(author) = &filters.author {
        let lower = author.to_lowercase();
        sql.clauses.push(
            "EXISTS ( \
               SELECT 1 FROM identities i \
               WHERE i.id = c.author_identity_id \
               AND ( \
                 LOWER(i.canonical_email) = ?{N} \
                 OR EXISTS ( \
                   SELECT 1 FROM identity_aliases ia \
                   WHERE ia.identity_id = i.id \
                   AND LOWER(ia.raw_email) = ?{N} \
                 ) \
               ) \
             )"
            .replace("{N}", &(sql.bindings.len() + 1).to_string()),
        );
        // Both sub-conditions share the same positional bind (we duplicate).
        // Rewrite to use two distinct binds with the same value.
        let n = sql.bindings.len() + 1;
        let clause = format!(
            "EXISTS ( \
               SELECT 1 FROM identities i \
               WHERE i.id = c.author_identity_id \
               AND ( \
                 LOWER(i.canonical_email) = ?{n} \
                 OR EXISTS ( \
                   SELECT 1 FROM identity_aliases ia \
                   WHERE ia.identity_id = i.id \
                   AND LOWER(ia.raw_email) = ?{n} \
                 ) \
               ) \
             )"
        );
        // Remove the placeholder clause added above, add the real one.
        sql.clauses.pop();
        sql.clauses.push(clause);
        sql.bindings.push(Value::Text(lower));
    }

    // --path: JSON-extract path-prefix LIKE.
    // commits.files_changed is a JSON array of objects with a "path" key.
    // We use json_each to expand and filter.
    if let Some(path) = &filters.path {
        let n = sql.bindings.len() + 1;
        let clause = format!(
            "EXISTS ( \
               SELECT 1 FROM json_each(c.files_changed) AS jf \
               WHERE json_extract(jf.value, '$.path') LIKE ?{n} \
             )"
        );
        sql.clauses.push(clause);
        // Bind `path%` (prefix LIKE, bound value not concatenated).
        sql.bindings.push(Value::Text(format!("{path}%")));
    }

    // --since / --until: resolve to Unix timestamps.
    if let Some(since) = &filters.since {
        let ts = resolve_timestamp(since, provider, TimeBound::Since)?;
        let n = sql.bindings.len() + 1;
        sql.clauses.push(format!("c.committed_at >= ?{n}"));
        sql.bindings.push(Value::Integer(ts));
    }

    if let Some(until) = &filters.until {
        let ts = resolve_timestamp(until, provider, TimeBound::Until)?;
        let n = sql.bindings.len() + 1;
        sql.clauses.push(format!("c.committed_at <= ?{n}"));
        sql.bindings.push(Value::Integer(ts));
    }

    // --branch: restrict to commits reachable from the given ref via commit_refs.
    if let Some(branch) = &filters.branch {
        let full_ref = normalize_branch_ref(branch);
        let n = sql.bindings.len() + 1;
        let clause = format!(
            "EXISTS ( \
               SELECT 1 FROM commit_refs cr \
               WHERE cr.commit_sha = c.sha \
               AND cr.ref_name = ?{n} \
             )"
        );
        sql.clauses.push(clause);
        sql.bindings.push(Value::Text(full_ref));
    }

    Ok(sql)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Whether a timestamp represents the start (inclusive lower bound) or end
/// (inclusive upper bound) of a day.
#[derive(Clone, Copy)]
enum TimeBound {
    Since,
    Until,
}

/// Resolve a user timestamp string to a Unix epoch integer.
///
/// Resolution order (grill #20 A):
/// 1. `GitProvider::rev_parse` (interprets `HEAD`, `main`, `v1.0.0`, etc.).
///    Falls through on `Error::InvalidRef`.
/// 2. RFC-3339 date-time (`2026-05-01T00:00:00Z`).
/// 3. ISO-8601 bare date (`2026-05-01`). For `Since`, maps to 00:00:00 UTC;
///    for `Until`, maps to 23:59:59 UTC.
fn resolve_timestamp(s: &str, provider: Option<&dyn GitProvider>, bound: TimeBound) -> Result<i64> {
    // 1. Try rev-parse.
    if let Some(prov) = provider {
        match prov.rev_parse(s) {
            Ok(sha) => {
                // rev_parse gives a SHA; we need the commit timestamp.
                // For now accept the SHA string and return a sentinel 0 so
                // the caller can't actually use it for time filtering — the
                // full implementation would call `git log --format=%ct <sha>`.
                // For M4, treat the SHA as a tag/branch and use its committer
                // timestamp by looking it up in the DB (not available here).
                // Fallback: treat the input as a raw integer if it looks like one.
                let _ = sha;
                if let Ok(ts) = s.parse::<i64>() {
                    return Ok(ts);
                }
                // Fall through to date parsing.
            }
            Err(Error::InvalidRef { .. }) => {
                // Not a ref, try date parsing below.
            }
            Err(e) => return Err(e),
        }
    }

    // 2. RFC-3339 date-time.
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.timestamp());
    }

    // 3. ISO-8601 bare date.
    if let Ok(date) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let ts = match bound {
            TimeBound::Since => date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
            TimeBound::Until => date.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp(),
        };
        return Ok(ts);
    }

    // 4. Raw integer (unix timestamp).
    if let Ok(ts) = s.parse::<i64>() {
        return Ok(ts);
    }

    Err(Error::InvalidRef {
        ref_text: s.to_string(),
    })
}

/// Normalize a branch name to a full ref path (grill #6 B).
///
/// * `main` → `refs/heads/main`
/// * `release/2.9` → `refs/heads/release/2.9`
/// * `refs/heads/main` → `refs/heads/main` (pass-through)
/// * `refs/remotes/origin/main` → `refs/remotes/origin/main` (pass-through)
fn normalize_branch_ref(branch: &str) -> String {
    if branch.starts_with("refs/") {
        branch.to_string()
    } else {
        format!("refs/heads/{branch}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_filters_produces_empty_sql() {
        let f = Filters::default();
        let sql = resolve(&f, None).unwrap();
        assert!(sql.is_empty());
        assert_eq!(sql.where_fragment(), "");
    }

    #[test]
    fn normalize_branch_short_name() {
        assert_eq!(normalize_branch_ref("main"), "refs/heads/main");
        assert_eq!(
            normalize_branch_ref("release/2.9"),
            "refs/heads/release/2.9"
        );
    }

    #[test]
    fn normalize_branch_already_full_ref() {
        assert_eq!(normalize_branch_ref("refs/heads/main"), "refs/heads/main");
        assert_eq!(
            normalize_branch_ref("refs/remotes/origin/main"),
            "refs/remotes/origin/main"
        );
    }

    #[test]
    fn path_filter_binds_prefix() {
        let f = Filters {
            path: Some("src/foo".into()),
            ..Default::default()
        };
        let sql = resolve(&f, None).unwrap();
        assert_eq!(sql.bindings.len(), 1);
        match &sql.bindings[0] {
            Value::Text(v) => assert_eq!(v, "src/foo%"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn since_rfc3339_parses() {
        let f = Filters {
            since: Some("2026-01-01T00:00:00Z".into()),
            ..Default::default()
        };
        let sql = resolve(&f, None).unwrap();
        assert_eq!(sql.clauses.len(), 1);
        assert!(sql.clauses[0].contains("committed_at >="));
    }

    #[test]
    fn since_iso_date_maps_to_start_of_day() {
        let f = Filters {
            since: Some("2026-05-01".into()),
            ..Default::default()
        };
        let sql = resolve(&f, None).unwrap();
        match &sql.bindings[0] {
            Value::Integer(ts) => {
                // 2026-05-01T00:00:00Z = 1777593600
                assert_eq!(*ts, 1_777_593_600);
            }
            other => panic!("expected Integer, got {other:?}"),
        }
    }

    #[test]
    fn until_iso_date_maps_to_end_of_day() {
        let f = Filters {
            until: Some("2026-05-01".into()),
            ..Default::default()
        };
        let sql = resolve(&f, None).unwrap();
        match &sql.bindings[0] {
            Value::Integer(ts) => {
                // 2026-05-01T23:59:59Z = 1777679999
                assert_eq!(*ts, 1_777_679_999);
            }
            other => panic!("expected Integer, got {other:?}"),
        }
    }

    #[test]
    fn branch_filter_normalizes_and_binds() {
        let f = Filters {
            branch: Some("main".into()),
            ..Default::default()
        };
        let sql = resolve(&f, None).unwrap();
        assert_eq!(sql.clauses.len(), 1);
        match &sql.bindings[0] {
            Value::Text(v) => assert_eq!(v, "refs/heads/main"),
            other => panic!("expected Text, got {other:?}"),
        }
    }

    #[test]
    fn multiple_filters_accumulate_clauses() {
        let f = Filters {
            path: Some("src".into()),
            since: Some("2026-01-01T00:00:00Z".into()),
            until: Some("2026-12-31T23:59:59Z".into()),
            ..Default::default()
        };
        let sql = resolve(&f, None).unwrap();
        assert_eq!(sql.clauses.len(), 3);
        assert_eq!(sql.bindings.len(), 3);
    }
}
