//! Search orchestrator for ranking and blending multiple signals.
//!
//! This module provides the [`SearchOrchestrator`] which coordinates
//! between FTS5 lexical search, path relevance scoring, and recency decay
//! to produce a ranked result set.

use rusqlite::{params, Connection};

use crate::config::SearchConfig;
use crate::error::{Error, Result};
use crate::index::indexer::FTS5_POPULATED_KEY;
use crate::search::path_relevance;
use crate::search::recency;

/// Search query parameters.
#[derive(Debug, Clone, Default)]
pub struct QueryParams {
    /// The search query string.
    pub query: String,
    /// Optional path filter.
    pub path_filter: Option<String>,
    /// Result limit (defaults to SearchConfig::default_limit).
    pub limit: Option<u32>,
}

/// A single search result with blended score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Commit SHA.
    pub sha: String,
    /// Commit subject.
    pub subject: String,
    /// Commit body.
    pub body: String,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Committed timestamp (Unix seconds).
    pub committed_at: u64,
    /// Touched directories (JSON array).
    pub dirs_touched: String,
    /// Touched files (JSON array).
    pub files_changed: String,
    /// Raw BM25 score from FTS5.
    pub lexical_bm25: f32,
    /// Path relevance score.
    pub path_relevance: f32,
    /// Recency score.
    pub recency: f32,
    /// Final blended score.
    pub score: f32,
    /// Search mode used ("lexical" or "hybrid").
    pub mode: String,
}

/// Search orchestrator that coordinates multiple ranking signals.
pub struct SearchOrchestrator<'a> {
    /// SQLite database connection.
    conn: &'a Connection,
    /// Search configuration.
    config: &'a SearchConfig,
}

impl<'a> SearchOrchestrator<'a> {
    /// Create a new search orchestrator.
    pub fn new(conn: &'a Connection, config: &'a SearchConfig) -> Self {
        Self { conn, config }
    }

    /// Execute a search query with blended ranking.
    ///
    /// # Arguments
    ///
    /// * `params` - Search query parameters
    ///
    /// # Returns
    ///
    /// A vector of ranked search results.
    ///
    /// # Errors
    ///
    /// Returns `Error::IndexNotReady` if the FTS5 backfill has not completed.
    pub fn query(&self, params: &QueryParams) -> Result<Vec<SearchResult>> {
        // Check if FTS5 is populated
        self.check_fts5_populated()?;

        // Try SHA lookup bypass first
        if let Some(result) = self.try_sha_lookup(&params.query) {
            return Ok(vec![result]);
        }

        // Build filter clause
        let filter_clause = self.build_filter_clause(params);

        // Execute lexical search
        let mut results = self.lexical_search(params, &filter_clause)?;

        // Compute path relevance and recency scores
        self.compute_secondary_scores(&mut results, params);

        // Blend scores
        self.blend_scores(&mut results);

        // Sort by score descending and truncate
        let limit = params.limit.unwrap_or(self.config.default_limit) as usize;
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        // Set mode to "lexical" even on empty FTS5 result
        for result in &mut results {
            result.mode = "lexical".to_string();
        }

        Ok(results)
    }

    /// Check if FTS5 backfill has completed.
    ///
    /// Returns `Error::IndexNotReady` if `index_state.fts5_populated` is 'false'.
    fn check_fts5_populated(&self) -> Result<()> {
        let populated: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM index_state WHERE key = ?1",
                params![FTS5_POPULATED_KEY],
                |row| row.get::<_, String>(0),
            )
            .ok();

        if populated.as_deref() != Some("true") {
            return Err(Error::IndexNotReady);
        }

        Ok(())
    }

    /// Try direct SHA lookup bypass.
    ///
    /// If the query is a valid 40-character hex SHA, attempt to fetch
    /// the commit directly without FTS5 search.
    fn try_sha_lookup(&self, query: &str) -> Option<SearchResult> {
        // Check if query looks like a SHA (40 hex chars)
        if query.len() == 40 && query.chars().all(|c| c.is_ascii_hexdigit()) {
            // Try to fetch the commit directly
            let result = self
                .conn
                .query_row(
                    "SELECT sha, subject, body, author_name, author_email, committed_at, dirs_touched, files_changed
                     FROM commits WHERE sha = ?1",
                    params![query],
                    |row| {
                        Ok(SearchResult {
                            sha: row.get(0)?,
                            subject: row.get(1)?,
                            body: row.get(2)?,
                            author_name: row.get(3)?,
                            author_email: row.get(4)?,
                            committed_at: row.get(5)?,
                            dirs_touched: row.get(6)?,
                            files_changed: row.get(7)?,
                            lexical_bm25: 1.0, // Perfect match for SHA lookup
                            path_relevance: 0.0,
                            recency: 0.0,
                            score: 1.0,
                            mode: "lexical".to_string(),
                        })
                    },
                )
                .ok();

            return result;
        }

        None
    }

    /// Build SQL filter clause from query parameters.
    fn build_filter_clause(&self, params: &QueryParams) -> String {
        let mut clauses = Vec::new();

        // Path filter (no bot-identity filter per grill #18)
        if let Some(path) = &params.path_filter {
            clauses.push(format!("files_changed LIKE '%{}%'", path));
        }

        if clauses.is_empty() {
            String::from("1=1")
        } else {
            clauses.join(" AND ")
        }
    }

    /// Execute FTS5 lexical search.
    fn lexical_search(&self, params: &QueryParams, filter_clause: &str) -> Result<Vec<SearchResult>> {
        let sql = format!(
            "SELECT
                c.sha, c.subject, c.body, c.author_name, c.author_email,
                c.committed_at, c.dirs_touched, c.files_changed,
                fts.rank
            FROM commits c
            JOIN commits_fts fts ON c.sha = fts.sha
            WHERE commits_fts MATCH ?1 AND {}
            ORDER BY rank
            LIMIT ?2",
            filter_clause
        );

        let limit = params.limit.unwrap_or(self.config.default_limit) as i64;

        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let results = stmt
            .query_map(params![params.query, limit], |row| {
                Ok(SearchResult {
                    sha: row.get(0)?,
                    subject: row.get(1)?,
                    body: row.get(2)?,
                    author_name: row.get(3)?,
                    author_email: row.get(4)?,
                    committed_at: row.get(5)?,
                    dirs_touched: row.get(6)?,
                    files_changed: row.get(7)?,
                    lexical_bm25: row.get::<_, f32>(8)?,
                    path_relevance: 0.0,
                    recency: 0.0,
                    score: 0.0,
                    mode: String::new(),
                })
            })
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let mut output = Vec::new();
        for result in results {
            output.push(result.map_err(|e| Error::Sqlite(e.to_string()))?);
        }

        Ok(output)
    }

    /// Compute path relevance and recency scores for results.
    fn compute_secondary_scores(&self, results: &mut [SearchResult], params: &QueryParams) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Parse dirs_touched JSON for each result
        for result in results {
            // Parse dirs_touched JSON array
            let dirs: Vec<String> = serde_json::from_str(&result.dirs_touched).unwrap_or_default();
            let dir_refs: Vec<&str> = dirs.iter().map(|s| s.as_str()).collect();

            // Compute path relevance
            result.path_relevance = path_relevance::score(
                &dir_refs,
                params.path_filter.as_deref(),
            );

            // Compute recency
            result.recency = recency::score(
                result.committed_at,
                now,
                self.config.recency_half_life_days,
            );
        }
    }

    /// Blend scores using the formula: 0.50*lexical_bm25 + 0.30*path_relevance + 0.20*recency
    fn blend_scores(&self, results: &mut [SearchResult]) {
        for result in results {
            result.score =
                0.50 * result.lexical_bm25 + 0.30 * result.path_relevance + 0.20 * result.recency;
        }
    }
}

#[cfg(test)]
mod tests {
    // Tests would require a test database setup
    // For now, we'll add basic unit tests for the helper functions
}
