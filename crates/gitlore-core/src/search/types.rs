//! Search types (SPEC-001 §4.3.1).
//!
//! Serde-derived types for search queries, filters, results, and scoring factors.
//! These types define the wire format for search operations and the structure
//! of search results returned by the search engine.

use serde::{Deserialize, Serialize};

/// Search query parameters.
///
/// Captures the free-text query string and optional filters for path, time
/// window, and result limiting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Query {
    /// Free-text query string (subject, body, paths, author, sha prefix).
    pub query: String,
    /// Optional cap on the number of returned results.
    pub limit: Option<u32>,
    /// Optional path prefix filter.
    pub path: Option<String>,
    /// Optional lower bound for the commit window (ref/SHA/date).
    pub since: Option<String>,
    /// Optional upper bound for the commit window (ref/SHA/date).
    pub until: Option<String>,
}

/// Search filters.
///
/// Structured filter criteria for search operations. This is a more
/// programmatic alternative to the inline [`Query`] fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filters {
    /// Path prefix filter (e.g., "src/auth").
    pub path: Option<String>,
    /// Author filter (canonical email or name).
    pub author: Option<String>,
    /// Lower bound for the commit window (ref/SHA/date).
    pub since: Option<String>,
    /// Upper bound for the commit window (ref/SHA/date).
    pub until: Option<String>,
    /// Branch filter (ref name).
    pub branch: Option<String>,
}

/// A single search result hit.
///
/// Represents one commit that matched the search query, along with its
/// relevance score and scoring factor breakdown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// Commit SHA (lowercase hex).
    pub sha: String,
    /// Author display name.
    pub author_name: String,
    /// Committer timestamp (unix epoch seconds).
    pub committed_at: i64,
    /// Commit subject (single line).
    pub subject: String,
    /// Overall relevance score in `[0.0, 1.0]`.
    pub score: f64,
    /// Scoring factor breakdown.
    pub factors: Factors,
}

/// Scoring factors for a search hit.
///
/// Breakdown of how the final score was computed. All factors are optional
/// since different search modes use different subsets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Factors {
    /// BM25 lexical score from FTS5 (optional, present in lexical/hybrid modes).
    pub lexical_bm25: Option<f64>,
    /// Path relevance score (optional, present when path filter or ranking is active).
    pub path_relevance: Option<f64>,
    /// Recency decay score (optional, present when time-based ranking is active).
    pub recency: Option<f64>,
    /// Semantic similarity score (optional, present only when embeddings are enabled).
    pub semantic: Option<f64>,
}

/// Search mode.
///
/// Determines the search strategy and which scoring factors are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchMode {
    /// Pure lexical search using FTS5 (BM25 ranking).
    #[serde(rename = "lexical")]
    Lexical,
    /// Direct SHA lookup (exact match on SHA prefix).
    #[serde(rename = "sha_lookup")]
    ShaLookup,
    /// Hybrid search combining lexical, semantic, path, and recency signals.
    #[serde(rename = "hybrid")]
    Hybrid,
}

/// Search results response.
///
/// Aggregates the search hits along with metadata about the search operation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    /// Total number of matching commits (may exceed `hits.len()` if limit was applied).
    pub total_matches: u64,
    /// Search mode used for this query.
    pub mode: SearchMode,
    /// Search hits (sorted by score descending).
    pub hits: Vec<SearchHit>,
}

/// Raw database hit (pre-scoring).
///
/// Represents a commit row from the database before scoring factors are applied.
/// Used internally by the search engine to compute final scores.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RawHit {
    /// Commit SHA (lowercase hex).
    pub sha: String,
    /// Author display name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Committer display name.
    pub committer_name: String,
    /// Committer email.
    pub committer_email: String,
    /// Author timestamp (unix epoch seconds).
    pub authored_at: i64,
    /// Committer timestamp (unix epoch seconds).
    pub committed_at: i64,
    /// Commit subject (single line).
    pub subject: String,
    /// Commit body (sans subject).
    pub body: String,
    /// Synthesised expanded text for FTS5 indexing.
    pub expanded: String,
    /// JSON array of parent SHAs.
    pub parent_shas: String,
    /// Number of parents.
    pub parent_count: u32,
    /// JSON array of file change records.
    pub files_changed: String,
    /// Number of files changed.
    pub file_count: u32,
    /// Total lines added.
    pub insertions: u64,
    /// Total lines removed.
    pub deletions: u64,
    /// JSON array of directories touched.
    pub dirs_touched: String,
    /// Number of directories touched.
    pub dir_count: u32,
    /// FTS5 BM25 score (if from FTS5 search).
    pub bm25: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_serializes_correctly() {
        let q = Query {
            query: "retry logic".to_string(),
            limit: Some(10),
            path: Some("src/auth".to_string()),
            since: Some("v2.8.0".to_string()),
            until: None,
        };
        let json = serde_json::to_string(&q).unwrap();
        assert!(json.contains("retry logic"));
        assert!(json.contains("src/auth"));
    }

    #[test]
    fn query_round_trips() {
        let original = Query {
            query: "test".to_string(),
            limit: Some(5),
            path: None,
            since: None,
            until: None,
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Query = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn filters_round_trips() {
        let original = Filters {
            path: Some("src/".to_string()),
            author: Some("alice@example.com".to_string()),
            since: Some("HEAD~10".to_string()),
            until: None,
            branch: Some("main".to_string()),
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: Filters = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn search_hit_round_trips() {
        let original = SearchHit {
            sha: "abc123".to_string(),
            author_name: "Alice".to_string(),
            committed_at: 1234567890,
            subject: "Fix auth bug".to_string(),
            score: 0.85,
            factors: Factors {
                lexical_bm25: Some(0.7),
                path_relevance: Some(0.9),
                recency: Some(0.8),
                semantic: None,
            },
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: SearchHit = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn factors_with_only_lexical() {
        let factors = Factors {
            lexical_bm25: Some(0.75),
            path_relevance: None,
            recency: None,
            semantic: None,
        };
        let json = serde_json::to_string(&factors).unwrap();
        let back: Factors = serde_json::from_str(&json).unwrap();
        assert_eq!(back.lexical_bm25, Some(0.75));
        assert!(back.path_relevance.is_none());
    }

    #[test]
    fn search_mode_serializes_as_lowercase() {
        assert_eq!(
            serde_json::to_string(&SearchMode::Lexical).unwrap(),
            "\"lexical\""
        );
        assert_eq!(
            serde_json::to_string(&SearchMode::ShaLookup).unwrap(),
            "\"sha_lookup\""
        );
        assert_eq!(
            serde_json::to_string(&SearchMode::Hybrid).unwrap(),
            "\"hybrid\""
        );
    }

    #[test]
    fn search_mode_round_trips() {
        for mode in [SearchMode::Lexical, SearchMode::ShaLookup, SearchMode::Hybrid] {
            let json = serde_json::to_string(&mode).unwrap();
            let back: SearchMode = serde_json::from_str(&json).unwrap();
            assert_eq!(mode, back);
        }
    }

    #[test]
    fn search_results_round_trips() {
        let original = SearchResults {
            total_matches: 42,
            mode: SearchMode::Hybrid,
            hits: vec![SearchHit {
                sha: "def456".to_string(),
                author_name: "Bob".to_string(),
                committed_at: 1234567891,
                subject: "Add feature".to_string(),
                score: 0.92,
                factors: Factors {
                    lexical_bm25: Some(0.6),
                    path_relevance: Some(0.8),
                    recency: Some(0.9),
                    semantic: Some(0.85),
                },
            }],
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: SearchResults = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }

    #[test]
    fn raw_hit_round_trips() {
        let original = RawHit {
            sha: "deadbeef".to_string(),
            author_name: "Charlie".to_string(),
            author_email: "charlie@example.com".to_string(),
            committer_name: "Charlie".to_string(),
            committer_email: "charlie@example.com".to_string(),
            authored_at: 1234567892,
            committed_at: 1234567892,
            subject: "Initial commit".to_string(),
            body: "".to_string(),
            expanded: "Initial commit".to_string(),
            parent_shas: "[]".to_string(),
            parent_count: 0,
            files_changed: "[]".to_string(),
            file_count: 0,
            insertions: 0,
            deletions: 0,
            dirs_touched: "[]".to_string(),
            dir_count: 0,
            bm25: Some(0.5),
        };
        let json = serde_json::to_string(&original).unwrap();
        let back: RawHit = serde_json::from_str(&json).unwrap();
        assert_eq!(original, back);
    }
}
