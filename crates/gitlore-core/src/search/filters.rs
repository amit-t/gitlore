//! Search filter translation to SQL predicates.
//!
//! This module provides [`Filters`] and its [`Filters::to_sql_predicates`]
//! method, which converts high-level filter options (--since, --until,
//! --branch, --author, --path) into parameterised SQL WHERE clauses and
//! bound values for safe query execution.

use crate::error::{Error, Result};
use crate::git::GitProvider;

/// Search filter options.
///
/// Corresponds to the CLI arguments for search subcommands (e.g. `gitlore
/// search --since <ref> --author <email> --path <prefix>`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    /// Lower bound for the commit window (ref/SHA/date).
    pub since: Option<String>,
    /// Upper bound for the commit window (ref/SHA/date).
    pub until: Option<String>,
    /// Branch filter (auto-prefixed with `refs/heads/` unless already starts
    /// with `refs/`).
    pub branch: Option<String>,
    /// Author filter (matches against `identities.canonical_email` or
    /// `identity_aliases.raw_email`, case-insensitive).
    pub author: Option<String>,
    /// Path prefix filter (matches against `commits.files_changed[].path`
    /// via JSON extraction).
    pub path: Option<String>,
}

/// SQL predicate with bound parameters.
///
/// Returned by [`Filters::to_sql_predicates`]. Contains a WHERE clause
/// fragment and a vector of `rusqlite::types::Value` parameters bound in
/// order.
#[derive(Debug, Clone, PartialEq)]
pub struct SqlPredicates {
    /// WHERE clause fragment (may be empty if no filters are active).
    /// Does not include the leading "WHERE" keyword.
    pub where_clause: String,
    /// Parameter values bound to the WHERE clause, in order of appearance.
    pub params: Vec<rusqlite::types::Value>,
}

impl Filters {
    /// Convert filters to SQL predicates with bound parameters.
    ///
    /// This method resolves each filter to a safe, parameterised SQL fragment:
    ///
    /// * `--since`/`--until`: Resolved via `git rev-parse` first (if the input
    ///   looks like a ref or SHA), then parsed as RFC3339 datetime, then as
    /// ISO-8601 date. Produces `authored_at >= ?` / `authored_at <= ?`
    ///   predicates with unix epoch seconds as bound values.
    ///
    /// * `--branch`: Auto-prefixes `refs/heads/` unless the input already
    ///   starts with `refs/`. Produces a `EXISTS (SELECT 1 FROM commit_refs
    ///   WHERE sha = commits.sha AND ref_name = ?)` predicate.
    ///
    /// * `--author`: Matches case-insensitively against both
    ///   `identities.canonical_email` and `identity_aliases.raw_email`.
    ///   Produces an `EXISTS` subquery with `LOWER(?)` bound value.
    ///
    /// * `--path`: Uses SQLite's JSON extraction to match against the
    ///   `commits.files_changed` array. Produces `EXISTS (SELECT 1 FROM
    ///   json_each(commits.files_changed) WHERE json_extract(value, '$.path')
    ///   LIKE ? || '%')` with the path prefix bound.
    ///
    /// All values are bound via `rusqlite::types::Value` — no string
    /// concatenation in the SQL.
    ///
    /// # Errors
    ///
    /// Returns `Error::Git` if `rev_parse` fails for a ref/SHA input.
    /// Returns `Error::InvalidQuery` if date parsing fails.
    pub fn to_sql_predicates(&self, git: &dyn GitProvider) -> Result<SqlPredicates> {
        let mut conditions = Vec::new();
        let mut params = Vec::new();

        if let Some(since) = &self.since {
            let timestamp = self.resolve_timestamp(git, since)?;
            conditions.push("authored_at >= ?".to_string());
            params.push(rusqlite::types::Value::Integer(timestamp));
        }

        if let Some(until) = &self.until {
            let timestamp = self.resolve_timestamp(git, until)?;
            conditions.push("authored_at <= ?".to_string());
            params.push(rusqlite::types::Value::Integer(timestamp));
        }

        if let Some(branch) = &self.branch {
            let ref_name = self.normalize_branch(branch);
            conditions.push(
                "EXISTS (SELECT 1 FROM commit_refs WHERE sha = commits.sha AND ref_name = ?)"
                    .to_string(),
            );
            params.push(rusqlite::types::Value::Text(ref_name));
        }

        if let Some(author) = &self.author {
            let email_lower = author.to_lowercase();
            conditions.push(
                "EXISTS (
                    SELECT 1
                    FROM identities
                    WHERE identities.id = commits.author_identity_id
                      AND LOWER(identities.canonical_email) = ?
                    UNION ALL
                    SELECT 1
                    FROM identity_aliases
                    WHERE identity_aliases.identity_id = commits.author_identity_id
                      AND LOWER(identity_aliases.raw_email) = ?
                )"
                .to_string(),
            );
            params.push(rusqlite::types::Value::Text(email_lower.clone()));
            params.push(rusqlite::types::Value::Text(email_lower));
        }

        if let Some(path) = &self.path {
            conditions.push(
                "EXISTS (
                    SELECT 1
                    FROM json_each(commits.files_changed)
                    WHERE json_extract(value, '$.path') LIKE ? || '%'
                )"
                .to_string(),
            );
            params.push(rusqlite::types::Value::Text(path.clone()));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            conditions.join(" AND ")
        };

        Ok(SqlPredicates {
            where_clause,
            params,
        })
    }

    /// Resolve a timestamp input to unix epoch seconds.
    ///
    /// Resolution order:
    /// 1. Try `git rev-parse` — if it succeeds, the input is a ref/SHA and
    ///    we return the commit's `authored_at` from the index (this requires
    ///    a separate query; for now we return an error since we don't have
    ///    index access here).
    /// 2. Try `chrono::DateTime::parse_from_rfc3339` — RFC3339 datetime.
    /// 3. Try `chrono::NaiveDate::parse_from_str` with ISO-8601 `%Y-%m-%d`.
    ///
    /// For the MVP, we skip the `rev_parse` commit timestamp lookup (it would
    /// require database access). We only handle RFC3339 and ISO-8601 date
    /// formats.
    fn resolve_timestamp(&self, git: &dyn GitProvider, input: &str) -> Result<i64> {
        // Try RFC3339 datetime first
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(input) {
            return Ok(dt.timestamp());
        }

        // Try ISO-8601 date (YYYY-MM-DD)
        if let Ok(date) = chrono::NaiveDate::parse_from_str(input, "%Y-%m-%d") {
            // Treat as midnight UTC on that date
            let dt = date.and_hms_opt(0, 0, 0).unwrap();
            return Ok(dt.and_utc().timestamp());
        }

        // Try git rev-parse to see if it's a ref/SHA
        // Note: This would require looking up the commit's timestamp in the
        // index, which we don't have access to here. For now, we return an error.
        if git.rev_parse(input).is_ok() {
            return Err(Error::InvalidQuery {
                query: format!(
                    "timestamp resolution for refs/SHAs not yet implemented: {}",
                    input
                ),
            });
        }

        Err(Error::InvalidQuery {
            query: format!(
                "invalid timestamp format: {} (expected RFC3339 or ISO-8601 date)",
                input
            ),
        })
    }

    /// Normalize a branch name to a fully qualified ref.
    ///
    /// If the input already starts with `refs/`, it's returned as-is.
    /// Otherwise, `refs/heads/` is prefixed.
    fn normalize_branch(&self, branch: &str) -> String {
        if branch.starts_with("refs/") {
            branch.to_string()
        } else {
            format!("refs/heads/{}", branch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    // Mock GitProvider for testing
    struct MockGitProvider;

    impl GitProvider for MockGitProvider {
        fn common_dir(&self) -> Result<PathBuf> {
            Ok(PathBuf::from("/mock"))
        }

        fn rev_parse(&self, _refname: &str) -> Result<crate::git::Sha> {
            Err(Error::ShaNotFound {
                sha: _refname.to_string(),
            })
        }

        fn list_refs(&self, _scope: crate::git::RefScope) -> Result<Vec<crate::git::RefEntry>> {
            Ok(Vec::new())
        }

        fn walk_commits(&self, _range: crate::git::WalkRange) -> Result<Vec<crate::git::RawCommit>> {
            Ok(Vec::new())
        }

        fn show(&self, _sha: &crate::git::Sha, _opts: crate::git::ShowOpts) -> Result<String> {
            Ok(String::new())
        }

        fn check_mailmap(&self, _name: &str, _email: &str) -> Result<crate::git::MailmapResolved> {
            Ok(crate::git::MailmapResolved {
                name: String::new(),
                email: String::new(),
            })
        }

        fn cat_file_exists(&self, _sha: &crate::git::Sha) -> Result<bool> {
            Ok(false)
        }
    }

    #[test]
    fn filters_default_is_empty() {
        let filters = Filters::default();
        assert!(filters.since.is_none());
        assert!(filters.until.is_none());
        assert!(filters.branch.is_none());
        assert!(filters.author.is_none());
        assert!(filters.path.is_none());
    }

    #[test]
    fn empty_filters_yields_empty_predicates() {
        let filters = Filters::default();
        let git = MockGitProvider;
        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert!(predicates.where_clause.is_empty());
        assert!(predicates.params.is_empty());
    }

    #[test]
    fn sql_predicates_clone_works() {
        let mut filters = Filters::default();
        filters.since = Some("2024-01-01".to_string());
        let git = MockGitProvider;
        let predicates = filters.to_sql_predicates(&git).unwrap();
        let _cloned = predicates.clone();
    }

    #[test]
    fn normalize_branch_prefixes_refs_heads() {
        let filters = Filters::default();
        assert_eq!(filters.normalize_branch("main"), "refs/heads/main");
        assert_eq!(filters.normalize_branch("feature/x"), "refs/heads/feature/x");
    }

    #[test]
    fn normalize_branch_preserves_refs_prefix() {
        let filters = Filters::default();
        assert_eq!(
            filters.normalize_branch("refs/heads/main"),
            "refs/heads/main"
        );
        assert_eq!(
            filters.normalize_branch("refs/remotes/origin/main"),
            "refs/remotes/origin/main"
        );
        assert_eq!(
            filters.normalize_branch("refs/tags/v1.0"),
            "refs/tags/v1.0"
        );
    }

    #[test]
    fn resolve_timestamp_rfc3339() {
        let filters = Filters::default();
        let git = MockGitProvider;

        let ts = filters
            .resolve_timestamp(&git, "2024-01-15T10:30:00Z")
            .unwrap();
        // Verify it's a reasonable timestamp (2024-01-15 is around 1705276800)
        assert!(ts > 1700000000);
        assert!(ts < 1800000000);
    }

    #[test]
    fn resolve_timestamp_iso8601_date() {
        let filters = Filters::default();
        let git = MockGitProvider;

        let ts = filters.resolve_timestamp(&git, "2024-01-15").unwrap();
        // Verify it's a reasonable timestamp (2024-01-15 is around 1705276800)
        assert!(ts > 1700000000);
        assert!(ts < 1800000000);
    }

    #[test]
    fn resolve_timestamp_invalid_format() {
        let filters = Filters::default();
        let git = MockGitProvider;

        let result = filters.resolve_timestamp(&git, "not-a-date");
        assert!(result.is_err());
        match result.unwrap_err() {
            Error::InvalidQuery { .. } => (),
            _ => panic!("expected InvalidQuery error"),
        }
    }

    #[test]
    fn since_filter_provides_predicate() {
        let mut filters = Filters::default();
        filters.since = Some("2024-01-01".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert_eq!(predicates.where_clause, "authored_at >= ?");
        assert_eq!(predicates.params.len(), 1);
        match &predicates.params[0] {
            rusqlite::types::Value::Integer(ts) => {
                // Verify it's a reasonable timestamp (2024-01-01 is around 1704067200)
                assert!(*ts > 1700000000);
                assert!(*ts < 1800000000);
            }
            _ => panic!("expected Integer parameter"),
        }
    }

    #[test]
    fn until_filter_provides_predicate() {
        let mut filters = Filters::default();
        filters.until = Some("2024-12-31".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert_eq!(predicates.where_clause, "authored_at <= ?");
        assert_eq!(predicates.params.len(), 1);
        match &predicates.params[0] {
            rusqlite::types::Value::Integer(ts) => {
                // Verify it's a reasonable timestamp (2024-12-31 is around 1735689600)
                assert!(*ts > 1700000000);
                assert!(*ts < 1800000000);
            }
            _ => panic!("expected Integer parameter"),
        }
    }

    #[test]
    fn branch_filter_provides_predicate() {
        let mut filters = Filters::default();
        filters.branch = Some("main".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert!(predicates.where_clause.contains("EXISTS"));
        assert!(predicates.where_clause.contains("commit_refs"));
        assert!(predicates.where_clause.contains("ref_name = ?"));
        assert_eq!(predicates.params.len(), 1);
        match &predicates.params[0] {
            rusqlite::types::Value::Text(ref_name) => {
                assert_eq!(ref_name, "refs/heads/main");
            }
            _ => panic!("expected Text parameter"),
        }
    }

    #[test]
    fn branch_filter_preserves_refs_prefix() {
        let mut filters = Filters::default();
        filters.branch = Some("refs/heads/main".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert_eq!(predicates.params.len(), 1);
        match &predicates.params[0] {
            rusqlite::types::Value::Text(ref_name) => {
                assert_eq!(ref_name, "refs/heads/main");
            }
            _ => panic!("expected Text parameter"),
        }
    }

    #[test]
    fn author_filter_provides_predicate() {
        let mut filters = Filters::default();
        filters.author = Some("Alice@example.com".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert!(predicates.where_clause.contains("EXISTS"));
        assert!(predicates.where_clause.contains("identities"));
        assert!(predicates.where_clause.contains("identity_aliases"));
        assert!(predicates.where_clause.contains("LOWER(identities.canonical_email) = ?"));
        assert!(predicates.where_clause.contains("LOWER(identity_aliases.raw_email) = ?"));
        assert_eq!(predicates.params.len(), 2);
        match &predicates.params[0] {
            rusqlite::types::Value::Text(email) => {
                assert_eq!(email, "alice@example.com");
            }
            _ => panic!("expected Text parameter"),
        }
        match &predicates.params[1] {
            rusqlite::types::Value::Text(email) => {
                assert_eq!(email, "alice@example.com");
            }
            _ => panic!("expected Text parameter"),
        }
    }

    #[test]
    fn author_filter_is_lowercased() {
        let mut filters = Filters::default();
        filters.author = Some("Alice@Example.COM".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        match &predicates.params[0] {
            rusqlite::types::Value::Text(email) => {
                assert_eq!(email, "alice@example.com");
            }
            _ => panic!("expected Text parameter"),
        }
    }

    #[test]
    fn path_filter_provides_predicate() {
        let mut filters = Filters::default();
        filters.path = Some("src".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert!(predicates.where_clause.contains("EXISTS"));
        assert!(predicates.where_clause.contains("json_each(commits.files_changed)"));
        assert!(predicates.where_clause.contains("json_extract(value, '$.path')"));
        assert!(predicates.where_clause.contains("LIKE ? || '%'"));
        assert_eq!(predicates.params.len(), 1);
        match &predicates.params[0] {
            rusqlite::types::Value::Text(path) => {
                assert_eq!(path, "src");
            }
            _ => panic!("expected Text parameter"),
        }
    }

    #[test]
    fn multiple_filters_are_anded() {
        let mut filters = Filters::default();
        filters.since = Some("2024-01-01".to_string());
        filters.branch = Some("main".to_string());
        filters.author = Some("alice@example.com".to_string());
        let git = MockGitProvider;

        let predicates = filters.to_sql_predicates(&git).unwrap();
        assert!(predicates.where_clause.contains("authored_at >= ?"));
        assert!(predicates.where_clause.contains("AND"));
        assert!(predicates.where_clause.contains("commit_refs"));
        assert!(predicates.where_clause.contains("identities"));
        assert_eq!(predicates.params.len(), 4); // since + branch + author (2 params)
    }
}
