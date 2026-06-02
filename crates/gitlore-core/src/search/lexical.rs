//! Lexical search using SQLite FTS5.
//!
//! This module provides FTS5-based full-text search over commit messages,
//! expanded text, and file paths. It implements query escaping, BM25 scoring
//! with configurable weights, and efficient cached query preparation.

use crate::config::{Bm25Weights, SearchConfig};
use crate::error::{Error, Result};
use rusqlite::Connection;
use std::cell::RefCell;
use std::collections::HashMap;

/// A filter clause for narrowing search results.
///
/// The shape of this struct determines the cache key for prepared MATCH queries.
/// Different filter shapes result in different SQL query templates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct FilterClause {
    /// Filter by author identity ID (if any).
    pub author_identity_id: Option<i64>,
    /// Filter by story ID (if any).
    pub story_id: Option<i64>,
    /// Filter by risk label (if any).
    pub risk_label: Option<String>,
    /// Minimum commit timestamp (unix epoch seconds, if any).
    pub min_authored_at: Option<i64>,
    /// Maximum commit timestamp (unix epoch seconds, if any).
    pub max_authored_at: Option<i64>,
}

/// A raw search hit from FTS5, before any additional processing.
///
/// Contains the BM25 score from FTS5 and the commit SHA.
#[derive(Debug, Clone, PartialEq)]
pub struct RawHit {
    /// The commit SHA (primary key).
    pub sha: String,
    /// The BM25 score from FTS5 (higher is better).
    pub bm25_score: f64,
}

/// Trait for lexical search implementations.
///
/// Implementations provide full-text search over indexed commit data.
pub trait LexicalSearch {
    /// Execute a lexical search query.
    ///
    /// # Arguments
    ///
    /// * `query` - The raw search query string
    /// * `filter` - Optional filter clause to narrow results
    /// * `limit` - Maximum number of results to return
    ///
    /// # Returns
    ///
    /// A vector of raw hits ordered by relevance (highest BM25 score first).
    fn search(&self, query: &str, filter: Option<&FilterClause>, limit: u32)
        -> Result<Vec<RawHit>>;
}

/// FTS5-based lexical search implementation.
///
/// Uses SQLite's FTS5 virtual table for full-text search with BM25 ranking.
/// Supports query escaping, cached query preparation keyed by filter shape,
/// and configurable BM25 weights per field.
pub struct Fts5LexicalSearch<'a> {
    /// SQLite database connection.
    conn: &'a Connection,
    /// BM25 weights for different FTS5 fields.
    bm25_weights: Bm25Weights,
    /// Cache of SQL query strings keyed by filter clause shape.
    /// Uses RefCell for interior mutability to allow caching through &self.
    query_cache: RefCell<HashMap<FilterClause, String>>,
}

impl<'a> Fts5LexicalSearch<'a> {
    /// Create a new FTS5 lexical search instance.
    ///
    /// # Arguments
    ///
    /// * `conn` - SQLite database connection
    /// * `config` - Search configuration containing BM25 weights
    pub fn new(conn: &'a Connection, config: &SearchConfig) -> Self {
        Self {
            conn,
            bm25_weights: config.bm25.clone(),
            query_cache: RefCell::new(HashMap::new()),
        }
    }

    /// Escape a query string for FTS5.
    ///
    /// Performs the following transformations:
    /// 1. Converts to lowercase
    /// 2. Strips NUL bytes
    /// 3. Detects outer phrase quotes and doubles internal quotes
    ///
    /// # Arguments
    ///
    /// * `query` - The raw query string
    ///
    /// # Returns
    ///
    /// The escaped query string, or `Error::InvalidQuery` if the result
    /// is empty or whitespace-only.
    fn escape_query(query: &str) -> Result<String> {
        // Step 1: Lowercase
        let mut escaped = query.to_lowercase();

        // Step 2: Strip NUL bytes
        escaped.retain(|c| c != '\0');

        // Step 3: Detect outer phrase quotes and double internal quotes
        let trimmed = escaped.trim();
        if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2 {
            // This is a phrase query - remove outer quotes and double internal quotes
            let inner = &trimmed[1..trimmed.len() - 1];
            let doubled = inner.replace('"', "\"\"");
            escaped = format!("\"{doubled}\"");
        } else {
            // Not a phrase query - just double any quotes
            escaped = escaped.replace('"', "\"\"");
        }

        // Validate: empty or whitespace-only after escape is invalid
        if escaped.trim().is_empty() {
            return Err(Error::InvalidQuery {
                query: query.to_string(),
            });
        }

        Ok(escaped)
    }

    /// Get a cached MATCH query SQL string keyed by filter clause shape.
    ///
    /// Returns the SQL query string for the given filter. The query string
    /// is cached for reuse with the same filter shape.
    fn get_cached_query(&self, filter: &FilterClause) -> String {
        let mut cache = self.query_cache.borrow_mut();
        if !cache.contains_key(filter) {
            let sql = self.build_match_query(filter);
            cache.insert(filter.clone(), sql);
        }

        cache.get(filter).unwrap().clone()
    }

    /// Build the MATCH query SQL for a given filter clause.
    ///
    /// Constructs a parameterized SQL query that:
    /// 1. Searches commits_fts with the MATCH operator
    /// 2. Applies BM25 weights from configuration
    /// 3. Joins to the commits table
    /// 4. Applies optional filters
    /// 5. Orders by BM25 score descending
    /// 6. Limits results
    fn build_match_query(&self, filter: &FilterClause) -> String {
        let mut where_clauses = vec!["1=1".to_string()];
        let mut params = vec![];

        // Build BM25 weighted match expression
        let bm25_expr = format!(
            "(subject * {}) + (body * {}) + (expanded * {}) + (paths * {})",
            self.bm25_weights.subject,
            self.bm25_weights.body,
            self.bm25_weights.expanded,
            self.bm25_weights.paths
        );

        // Add filter clauses
        if let Some(author_id) = filter.author_identity_id {
            where_clauses.push("c.author_identity_id = ?".to_string());
            params.push(author_id.to_string());
        }

        if let Some(story_id) = filter.story_id {
            where_clauses.push("c.story_id = ?".to_string());
            params.push(story_id.to_string());
        }

        if let Some(ref risk_label) = filter.risk_label {
            where_clauses.push("c.risk_label = ?".to_string());
            params.push(format!("'{risk_label}'"));
        }

        if let Some(min_time) = filter.min_authored_at {
            where_clauses.push("c.authored_at >= ?".to_string());
            params.push(min_time.to_string());
        }

        if let Some(max_time) = filter.max_authored_at {
            where_clauses.push("c.authored_at <= ?".to_string());
            params.push(max_time.to_string());
        }

        let where_clause = where_clauses.join(" AND ");

        format!(
            r#"
            SELECT c.sha, bm25(commits_fts) * ({bm25_expr}) as score
            FROM commits_fts
            JOIN commits c ON commits_fts.sha = c.sha
            WHERE commits_fts MATCH ? AND {where_clause}
            ORDER BY score DESC
            LIMIT ?
            "#
        )
    }
}

impl<'a> LexicalSearch for Fts5LexicalSearch<'a> {
    fn search(
        &self,
        query: &str,
        filter: Option<&FilterClause>,
        limit: u32,
    ) -> Result<Vec<RawHit>> {
        // Escape the query
        let escaped = Self::escape_query(query)?;

        // Use default filter if none provided
        let default_filter = FilterClause::default();
        let filter = filter.unwrap_or(&default_filter);

        // Get cached query SQL
        let sql = self.get_cached_query(filter);

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let rows = stmt
            .query_map(
                [
                    &escaped as &dyn rusqlite::ToSql,
                    &limit as &dyn rusqlite::ToSql,
                ],
                |row| {
                    Ok(RawHit {
                        sha: row.get(0)?,
                        bm25_score: row.get(1)?,
                    })
                },
            )
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let mut hits = Vec::new();
        for row in rows {
            let hit = row.map_err(|e| Error::Sqlite(e.to_string()))?;
            hits.push(hit);
        }

        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_clause_default() {
        let filter = FilterClause::default();
        assert!(filter.author_identity_id.is_none());
        assert!(filter.story_id.is_none());
        assert!(filter.risk_label.is_none());
        assert!(filter.min_authored_at.is_none());
        assert!(filter.max_authored_at.is_none());
    }

    #[test]
    fn test_filter_clause_equality() {
        let f1 = FilterClause {
            author_identity_id: Some(1),
            story_id: Some(2),
            risk_label: Some("high".to_string()),
            min_authored_at: Some(100),
            max_authored_at: Some(200),
        };

        let f2 = FilterClause {
            author_identity_id: Some(1),
            story_id: Some(2),
            risk_label: Some("high".to_string()),
            min_authored_at: Some(100),
            max_authored_at: Some(200),
        };

        assert_eq!(f1, f2);
    }

    #[test]
    fn test_filter_clause_hash() {
        use std::collections::HashSet;

        let f1 = FilterClause {
            author_identity_id: Some(1),
            ..Default::default()
        };

        let f2 = FilterClause {
            author_identity_id: Some(1),
            ..Default::default()
        };

        let f3 = FilterClause {
            author_identity_id: Some(2),
            ..Default::default()
        };

        let mut set = HashSet::new();
        set.insert(f1);
        set.insert(f2);
        set.insert(f3);

        assert_eq!(set.len(), 2); // f1 and f2 are equal, so only 2 unique entries
    }

    #[test]
    fn test_escape_query_lowercase() {
        let result = Fts5LexicalSearch::escape_query("HELLO WORLD").unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn test_escape_query_strip_nul() {
        let result = Fts5LexicalSearch::escape_query("hello\0world").unwrap();
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn test_escape_query_phrase_detection() {
        let result = Fts5LexicalSearch::escape_query("\"hello world\"").unwrap();
        assert_eq!(result, "\"hello world\"");
    }

    #[test]
    fn test_escape_query_phrase_with_internal_quotes() {
        let result = Fts5LexicalSearch::escape_query("\"hello \"world\"\"").unwrap();
        assert_eq!(result, "\"hello \"\"world\"\"\""); // Doubled internal quotes
    }

    #[test]
    fn test_escape_query_double_quotes_non_phrase() {
        let result = Fts5LexicalSearch::escape_query("hello\"world").unwrap();
        assert_eq!(result, "hello\"\"world");
    }

    #[test]
    fn test_escape_query_empty_string() {
        let result = Fts5LexicalSearch::escape_query("");
        assert!(matches!(result, Err(Error::InvalidQuery { .. })));
    }

    #[test]
    fn test_escape_query_whitespace_only() {
        let result = Fts5LexicalSearch::escape_query("   \t\n  ");
        assert!(matches!(result, Err(Error::InvalidQuery { .. })));
    }

    #[test]
    fn test_escape_query_whitespace_after_escape() {
        let result = Fts5LexicalSearch::escape_query("  \0  "); // After stripping NUL, only whitespace
        assert!(matches!(result, Err(Error::InvalidQuery { .. })));
    }

    #[test]
    fn test_build_match_query_no_filter() {
        let search = Fts5LexicalSearch {
            conn: &Connection::open_in_memory().unwrap(),
            bm25_weights: Bm25Weights::default(),
            query_cache: RefCell::new(HashMap::new()),
        };

        let sql = search.build_match_query(&FilterClause::default());
        assert!(sql.contains("WHERE commits_fts MATCH ? AND 1=1"));
        assert!(sql.contains("ORDER BY score DESC"));
        assert!(sql.contains("LIMIT ?"));
    }

    #[test]
    fn test_build_match_query_with_author_filter() {
        let search = Fts5LexicalSearch {
            conn: &Connection::open_in_memory().unwrap(),
            bm25_weights: Bm25Weights::default(),
            query_cache: RefCell::new(HashMap::new()),
        };

        let filter = FilterClause {
            author_identity_id: Some(42),
            ..Default::default()
        };

        let sql = search.build_match_query(&filter);
        assert!(sql.contains("c.author_identity_id = ?"));
    }

    #[test]
    fn test_build_match_query_with_story_filter() {
        let search = Fts5LexicalSearch {
            conn: &Connection::open_in_memory().unwrap(),
            bm25_weights: Bm25Weights::default(),
            query_cache: RefCell::new(HashMap::new()),
        };

        let filter = FilterClause {
            story_id: Some(7),
            ..Default::default()
        };

        let sql = search.build_match_query(&filter);
        assert!(sql.contains("c.story_id = ?"));
    }

    #[test]
    fn test_build_match_query_with_risk_label_filter() {
        let search = Fts5LexicalSearch {
            conn: &Connection::open_in_memory().unwrap(),
            bm25_weights: Bm25Weights::default(),
            query_cache: RefCell::new(HashMap::new()),
        };

        let filter = FilterClause {
            risk_label: Some("high".to_string()),
            ..Default::default()
        };

        let sql = search.build_match_query(&filter);
        assert!(sql.contains("c.risk_label = ?"));
    }

    #[test]
    fn test_build_match_query_with_time_filters() {
        let search = Fts5LexicalSearch {
            conn: &Connection::open_in_memory().unwrap(),
            bm25_weights: Bm25Weights::default(),
            query_cache: RefCell::new(HashMap::new()),
        };

        let filter = FilterClause {
            min_authored_at: Some(1000),
            max_authored_at: Some(2000),
            ..Default::default()
        };

        let sql = search.build_match_query(&filter);
        assert!(sql.contains("c.authored_at >= ?"));
        assert!(sql.contains("c.authored_at <= ?"));
    }

    #[test]
    fn test_build_match_query_bm25_weights() {
        let search = Fts5LexicalSearch {
            conn: &Connection::open_in_memory().unwrap(),
            bm25_weights: Bm25Weights {
                subject: 5.0,
                body: 2.0,
                expanded: 3.0,
                paths: 1.0,
            },
            query_cache: RefCell::new(HashMap::new()),
        };

        let sql = search.build_match_query(&FilterClause::default());
        assert!(sql.contains("(subject * 5)"));
        assert!(sql.contains("(body * 2)"));
        assert!(sql.contains("(expanded * 3)"));
        assert!(sql.contains("(paths * 1)"));
    }
}
