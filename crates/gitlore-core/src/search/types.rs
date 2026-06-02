//! Core search types (TDD-001 §3 / SPEC-001 §4.3.1).
//!
//! All types are `serde`-derived so the CLI envelope renderer in the `gitlore`
//! bin crate can serialize `SearchResults` directly into the ADR-030 JSON
//! envelope without any manual mapping.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

/// A fully-resolved search query ready for the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    /// Free-text query string (raw, as typed by the user).
    pub text: String,
    /// Pre-filter options applied before FTS5 scoring.
    pub filters: Filters,
    /// Maximum hits to return (already clamped to soft_cap).
    pub limit: u32,
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// User-supplied pre-filters that narrow the commit candidate set before
/// FTS5 scoring.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Filters {
    /// Restrict to commits touching this path prefix.
    pub path: Option<String>,
    /// Restrict to commits whose author matches this name or e-mail.
    pub author: Option<String>,
    /// Lower bound on commit timestamp (ref / SHA / ISO-8601 date).
    pub since: Option<String>,
    /// Upper bound on commit timestamp (ref / SHA / ISO-8601 date).
    pub until: Option<String>,
    /// Restrict to commits reachable from this branch.
    pub branch: Option<String>,
}

// ---------------------------------------------------------------------------
// SearchMode
// ---------------------------------------------------------------------------

/// How the search result set was produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// FTS5 MATCH query (default for all text queries including empty-result).
    Lexical,
    /// SHA-prefix bypass: the query looked like a hex SHA so the result was
    /// fetched by a direct `commits.sha LIKE 'prefix%'` lookup.
    ShaLookup,
    /// Hybrid vector + lexical blend (deferred to M11).
    Hybrid,
}

// ---------------------------------------------------------------------------
// SearchResults
// ---------------------------------------------------------------------------

/// The complete response from [`super::orchestrator::SearchOrchestrator::query`].
#[derive(Debug, Clone, Serialize)]
pub struct SearchResults {
    /// Original query text.
    pub query: String,
    /// How results were retrieved.
    pub mode: SearchMode,
    /// Ordered list of hits (highest score first), truncated to the query limit.
    pub results: Vec<SearchHit>,
    /// Total hits available before the limit was applied. For the SHA-lookup
    /// path this is always 1 (resolved) or 0 (not found).
    pub total_available: u64,
}

// ---------------------------------------------------------------------------
// SearchHit
// ---------------------------------------------------------------------------

/// A single ranked commit returned by the search orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    /// Full 40-char commit SHA.
    pub sha: String,
    /// Conventional-commit-stripped subject line.
    pub subject: String,
    /// Resolved canonical author name.
    pub author: String,
    /// Commit timestamp (Unix seconds).
    pub committed_at: i64,
    /// Final blended score: `0.50*lexical_bm25 + 0.30*path_relevance + 0.20*recency`.
    pub score: f32,
    /// Per-factor scores that produced `score`.
    pub factors: Factors,
}

// ---------------------------------------------------------------------------
// Factors
// ---------------------------------------------------------------------------

/// Per-factor breakdown of a hit's final score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Factors {
    /// Normalised BM25 score from FTS5 (0.0 on sha-lookup path).
    pub lexical_bm25: f32,
    /// Path relevance score from [`super::path_relevance::score`].
    pub path_relevance: f32,
    /// Recency decay score from [`super::recency::score`].
    pub recency: f32,
    /// Semantic cosine score (M11, omitted in JSON until then).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub semantic: Option<f32>,
}

// ---------------------------------------------------------------------------
// RawHit
// ---------------------------------------------------------------------------

/// Internal hit shape returned by [`super::lexical::LexicalSearch`] before
/// the orchestrator applies path-relevance and recency.
#[derive(Debug, Clone)]
pub struct RawHit {
    /// Full 40-char commit SHA.
    pub sha: String,
    /// Conventional-commit-stripped subject line (post-indexed, may differ
    /// from the raw git subject).
    pub subject: String,
    /// Author display name.
    pub author_name: String,
    /// Author email (lowercased).
    pub author_email: String,
    /// Identity FK (may be None for commits indexed before M3-4).
    pub author_identity_id: Option<i64>,
    /// Commit timestamp (Unix seconds).
    pub committed_at: i64,
    /// Directories touched by this commit (top-level components of each
    /// changed file path).
    pub dirs_touched: Vec<String>,
    /// Raw BM25 score from FTS5 (already normalised to [0, 1] range by the
    /// lexical layer).
    pub bm25: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_mode_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&SearchMode::Lexical).unwrap(),
            r#""lexical""#
        );
        assert_eq!(
            serde_json::to_string(&SearchMode::ShaLookup).unwrap(),
            r#""sha_lookup""#
        );
        assert_eq!(
            serde_json::to_string(&SearchMode::Hybrid).unwrap(),
            r#""hybrid""#
        );
    }

    #[test]
    fn factors_omits_semantic_when_none() {
        let f = Factors {
            lexical_bm25: 0.5,
            path_relevance: 0.3,
            recency: 0.2,
            semantic: None,
        };
        let json = serde_json::to_value(&f).unwrap();
        assert!(json.get("semantic").is_none());
    }

    #[test]
    fn factors_includes_semantic_when_some() {
        let f = Factors {
            lexical_bm25: 0.5,
            path_relevance: 0.3,
            recency: 0.2,
            semantic: Some(0.9),
        };
        let json = serde_json::to_value(&f).unwrap();
        // f32 → f64 conversion introduces sub-epsilon rounding; compare as f64.
        let v = json["semantic"]
            .as_f64()
            .expect("semantic must be a number");
        assert!(
            (v - 0.9_f64).abs() < 1e-6,
            "semantic expected ≈0.9, got {v}"
        );
    }

    #[test]
    fn filters_default_is_all_none() {
        let f = Filters::default();
        assert!(f.path.is_none());
        assert!(f.author.is_none());
        assert!(f.since.is_none());
        assert!(f.until.is_none());
        assert!(f.branch.is_none());
    }

    #[test]
    fn query_round_trips_json() {
        let q = Query {
            text: "retry logic".into(),
            filters: Filters {
                path: Some("src".into()),
                ..Default::default()
            },
            limit: 50,
        };
        let json = serde_json::to_string(&q).unwrap();
        let q2: Query = serde_json::from_str(&json).unwrap();
        assert_eq!(q2.text, "retry logic");
        assert_eq!(q2.filters.path.as_deref(), Some("src"));
        assert_eq!(q2.limit, 50);
    }
}
