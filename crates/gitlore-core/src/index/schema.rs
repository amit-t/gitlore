//! Rust mirrors of the SPEC-001 §5.1 SQL schema.
//!
//! Every type in this module corresponds 1:1 to a table created by
//! [`super::migrations`]. Column ordering matches the `CREATE TABLE`
//! statements verbatim so a row-binding layer can use the field order as
//! the canonical parameter order without an extra mapping table.
//!
//! ## JSON columns
//!
//! The SQL schema stores several columns as TEXT-encoded JSON (arrays of
//! paths, identity lists, classification signals, etc.). The Rust mirrors
//! carry them as [`String`] so:
//!
//! * round-tripping through `rusqlite::params!` requires no extra encoding
//!   step at the binding site;
//! * eval-harness consumers can persist a row verbatim without re-parsing
//!   payloads they do not inspect;
//! * the canonical decoded shape (`Vec<String>`, `BTreeMap<String, u64>`,
//!   etc.) is reached via the `parse_*` helpers below, which use
//!   `serde_json` so the encode/decode pair is a strict round-trip.
//!
//! Use [`serialize_string_list`] / [`parse_string_list`] for arrays of
//! strings (`parent_shas`, `dirs_touched`, `top_paths`, `authors`,
//! `top_contributors`). Use [`serialize_count_map`] / [`parse_count_map`]
//! for `{path: count}` style objects (`cochange_paths`,
//! `admission_signals`). Use [`serialize_file_changes`] /
//! [`parse_file_changes`] for the structured `files_changed` array.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One row of the `commits` table.
///
/// SPEC-001 §5.1 lists 40+ columns including nine file-classification
/// counters (`*_files_changed`). The struct field order mirrors the
/// `CREATE TABLE` column order in
/// [`super::migrations`]'s `0001_init.sql`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Commit {
    /// Full commit SHA (lowercase hex). Primary key.
    pub sha: String,

    // -- Identity (raw + resolved) -----------------------------------------
    /// Raw author display name from the commit object.
    pub author_name: String,
    /// Raw author email from the commit object.
    pub author_email: String,
    /// Resolved [`Identity::id`] after mailmap normalisation (nullable until
    /// the identity-resolution pass has run).
    pub author_identity_id: Option<i64>,
    /// Raw committer display name from the commit object.
    pub committer_name: String,
    /// Raw committer email from the commit object.
    pub committer_email: String,
    /// Resolved [`Identity::id`] for the committer (nullable).
    pub committer_identity_id: Option<i64>,

    // -- Timestamps --------------------------------------------------------
    /// Author timestamp (unix epoch seconds, UTC).
    pub authored_at: i64,
    /// Committer timestamp (unix epoch seconds, UTC).
    pub committed_at: i64,
    /// Author timezone offset in minutes east of UTC.
    pub authored_tz_offset: i32,
    /// Committer timezone offset in minutes east of UTC.
    pub committed_tz_offset: i32,

    // -- Message -----------------------------------------------------------
    /// Single-line subject (matches `git log %s`).
    pub subject: String,
    /// Body sans subject (matches `git log %b`).
    pub body: String,
    /// Synthesised text combining subject, body, and parsed trailers, used
    /// for FTS5 indexing (`commits_fts.expanded`).
    pub expanded: String,

    // -- Topology ----------------------------------------------------------
    /// JSON array of parent SHAs in order. Encode/decode via
    /// [`parse_string_list`] / [`serialize_string_list`].
    pub parent_shas: String,
    /// Number of parents (0 root, 1 normal, >=2 merge).
    pub parent_count: u32,
    /// `1` iff `parent_count >= 2`.
    pub is_merge: u8,
    /// `1` iff `parent_count == 0`.
    pub is_root: u8,

    // -- File-level changes ------------------------------------------------
    /// JSON array of `{path,status,insertions,deletions}` records. Encode/
    /// decode via [`parse_file_changes`] / [`serialize_file_changes`].
    pub files_changed: String,
    /// Number of distinct files in `files_changed`.
    pub file_count: u32,
    /// Total lines added.
    pub insertions: u64,
    /// Total lines removed.
    pub deletions: u64,
    /// JSON array of top-level directories touched. Encode/decode via
    /// [`parse_string_list`] / [`serialize_string_list`].
    pub dirs_touched: String,
    /// Number of distinct top-level directories.
    pub dir_count: u32,

    // -- Classification counters (9 per SPEC-001 §5.1) ---------------------
    /// Files classified as tests (glob `tests/**`, `**/*_test.*`, etc.).
    pub test_files_changed: u32,
    /// Files classified as config (TOML/YAML/JSON config, dotfiles, etc.).
    pub config_files_changed: u32,
    /// Files classified as infrastructure (Terraform, k8s manifests, etc.).
    pub infra_files_changed: u32,
    /// Files classified as documentation (Markdown, RST under `docs/`).
    pub doc_files_changed: u32,
    /// Files classified as source code (language-specific patterns).
    pub code_files_changed: u32,
    /// Files classified as dependency manifests (`Cargo.lock`,
    /// `package.json`, etc.).
    pub dependency_files_changed: u32,
    /// Files classified as CI configuration (`.github/workflows/**`,
    /// `.circleci/**`).
    pub ci_files_changed: u32,
    /// Files classified as test fixtures (`tests/fixtures/**`, etc.).
    pub fixture_files_changed: u32,
    /// Files classified as schema migrations
    /// (`migrations/**`, `db/migrate/**`).
    pub migration_files_changed: u32,

    // -- Revert tracking ---------------------------------------------------
    /// `1` iff the subject matches `Revert "..."` or the body carries a
    /// `This reverts commit <sha>` trailer.
    pub is_revert: u8,
    /// SHA of a later commit that reverts this one (nullable; back-pointer
    /// populated when a downstream revert is discovered).
    pub reverted_by_sha: Option<String>,

    // -- Risk (cached for the §15 scorer) ----------------------------------
    /// Heuristic risk score in `[0.0, 1.0]` per SPEC-001 §15 (nullable until
    /// the risk pass has run).
    pub risk_score: Option<f64>,
    /// Risk label derived from `risk_score` and config cutoffs (`"low"`,
    /// `"medium"`, `"high"`; nullable until scored).
    pub risk_label: Option<String>,

    // -- Story assignment + admission signals ------------------------------
    /// JSON object of story-clusterer admission signals (which heuristics
    /// fired, with weights). Encode/decode via [`parse_count_map`] /
    /// [`serialize_count_map`].
    pub admission_signals: String,
    /// Foreign key into [`Story::id`] (nullable until clustering has run).
    pub story_id: Option<i64>,

    // -- Bookkeeping -------------------------------------------------------
    /// First-indexed timestamp (unix epoch seconds, UTC).
    pub indexed_at: i64,
    /// Last-touched timestamp (unix epoch seconds, UTC).
    pub updated_at: i64,
}

/// One row of the `identities` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Identity {
    /// Auto-incrementing primary key.
    pub id: i64,
    /// Canonical display name (post-mailmap).
    pub canonical_name: String,
    /// Canonical email (post-mailmap).
    pub canonical_email: String,
    /// First time this identity was observed (unix epoch seconds).
    pub first_seen_at: i64,
    /// Most recent time this identity was observed (unix epoch seconds).
    pub last_seen_at: i64,
    /// Number of commits authored under this identity (cached).
    pub commit_count: u64,
}

/// One row of the `identity_aliases` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityAlias {
    /// Auto-incrementing primary key.
    pub id: i64,
    /// FK into [`Identity::id`].
    pub identity_id: i64,
    /// Raw display name as it appears on disk.
    pub raw_name: String,
    /// Raw email as it appears on disk.
    pub raw_email: String,
}

/// One row of the `commit_coauthors` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitCoauthor {
    /// FK into [`Commit::sha`].
    pub sha: String,
    /// FK into [`Identity::id`].
    pub identity_id: i64,
}

/// One row of the `commit_refs` table — many-to-many between commits and
/// the refs they appear on at index time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommitRef {
    /// FK into [`Commit::sha`].
    pub sha: String,
    /// Fully qualified ref name (e.g. `refs/heads/main`).
    pub ref_name: String,
    /// `"branch"`, `"remote_branch"`, or `"tag"`.
    pub ref_kind: String,
}

/// One row of the `tags` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    /// Fully qualified tag ref (e.g. `refs/tags/v1.2.3`). Primary key.
    pub ref_name: String,
    /// Commit the tag points at (annotated tags dereferenced).
    pub sha: String,
    /// `1` for annotated tags, `0` for lightweight.
    pub annotated: u8,
    /// Tag message body (empty for lightweight tags).
    pub message: String,
    /// Tagger timestamp (unix epoch seconds; `0` when unknown).
    pub tagged_at: i64,
}

/// One row of the `stories` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Story {
    /// Auto-incrementing primary key.
    pub id: i64,
    /// Auto-generated title (longest common subject prefix + top dir).
    pub title: String,
    /// Story window start (unix epoch seconds).
    pub date_start: i64,
    /// Story window end (unix epoch seconds).
    pub date_end: i64,
    /// Number of member commits.
    pub member_count: u32,
    /// JSON array of top paths touched by member commits. Encode/decode via
    /// [`parse_string_list`] / [`serialize_string_list`].
    pub top_paths: String,
    /// JSON array of author canonical emails. Encode/decode via
    /// [`parse_string_list`] / [`serialize_string_list`].
    pub authors: String,
    /// Aggregated risk score in `[0.0, 1.0]`.
    pub risk_score: f64,
    /// JSON object mapping risk-factor name to contribution. Encode/decode
    /// via [`parse_count_map`] / [`serialize_count_map`].
    pub risk_factors: String,
    /// Generation timestamp (unix epoch seconds).
    pub generated_at: i64,
}

/// One row of the `story_members` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoryMember {
    /// FK into [`Story::id`].
    pub story_id: i64,
    /// FK into [`Commit::sha`].
    pub sha: String,
}

/// One row of the `path_stats` table — cached per-path hotspot signals.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PathStat {
    /// Repo-relative path. Primary key.
    pub path: String,
    /// Number of commits touching this path within the configured window.
    pub commit_count: u64,
    /// Number of distinct identities that have touched the path.
    pub unique_authors: u64,
    /// Number of reverts on the path.
    pub revert_count: u64,
    /// Most recent touch timestamp (unix epoch seconds).
    pub last_touched: i64,
    /// JSON object mapping co-changing paths to co-change count. Encode/
    /// decode via [`parse_count_map`] / [`serialize_count_map`].
    pub cochange_paths: String,
    /// JSON array of top contributor canonical emails. Encode/decode via
    /// [`parse_string_list`] / [`serialize_string_list`].
    pub top_contributors: String,
}

/// One row of the `repo_stats` singleton table — repo-wide aggregates
/// recomputed at index time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RepoStat {
    /// Singleton primary key (always `1`).
    pub id: i64,
    /// Total commits indexed.
    pub commit_count: u64,
    /// Total distinct identities.
    pub identity_count: u64,
    /// First-commit timestamp (unix epoch seconds; `0` when empty).
    pub first_commit_at: i64,
    /// Last-commit timestamp (unix epoch seconds; `0` when empty).
    pub last_commit_at: i64,
    /// SHA of the most recently indexed commit (empty string when none).
    pub last_indexed_sha: String,
    /// Generation timestamp (unix epoch seconds).
    pub generated_at: i64,
}

/// One row of the `index_state` key/value table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexState {
    /// Key (e.g. `"schema_version"`, `"last_indexed_sha"`).
    pub key: String,
    /// Value, encoded per key convention (numbers serialised as decimal).
    pub value: String,
}

// ---------------------------------------------------------------------------
// JSON column helpers
// ---------------------------------------------------------------------------

/// A single file change as recorded inside [`Commit::files_changed`].
///
/// Field semantics mirror [`crate::git::FileChange`] but the Rust shape lives
/// here as well so the persistence layer does not depend on the git layer
/// when only reading rows back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChangeRecord {
    /// Path of the changed file (post-rename for `R<NN>` entries).
    pub path: String,
    /// One-letter status code (`A`/`M`/`D`/`R`/`C`/`T`/`U`/`X`).
    pub status: char,
    /// Lines added (`0` for binary).
    pub insertions: u64,
    /// Lines removed (`0` for binary).
    pub deletions: u64,
}

/// Serialise a list of strings to the canonical JSON-column encoding.
///
/// Empty input yields the literal `[]` so the on-disk shape never carries
/// a NULL where the schema declares a JSON array.
pub fn serialize_string_list(items: &[String]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string())
}

/// Inverse of [`serialize_string_list`]. Empty / NULL / malformed input
/// decodes to an empty `Vec` so callers can treat the column as
/// "best-effort optional".
pub fn parse_string_list(raw: &str) -> Vec<String> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

/// Serialise a count map (path → count) to the canonical JSON-column
/// encoding. Uses a `BTreeMap` so the encoded output is key-sorted and
/// diff-friendly.
pub fn serialize_count_map(map: &BTreeMap<String, u64>) -> String {
    serde_json::to_string(map).unwrap_or_else(|_| "{}".to_string())
}

/// Inverse of [`serialize_count_map`]. Empty / NULL / malformed input
/// decodes to an empty map.
pub fn parse_count_map(raw: &str) -> BTreeMap<String, u64> {
    if raw.trim().is_empty() {
        return BTreeMap::new();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

/// Serialise a list of file changes to the canonical JSON-column encoding.
pub fn serialize_file_changes(items: &[FileChangeRecord]) -> String {
    serde_json::to_string(items).unwrap_or_else(|_| "[]".to_string())
}

/// Inverse of [`serialize_file_changes`].
pub fn parse_file_changes(raw: &str) -> Vec<FileChangeRecord> {
    if raw.trim().is_empty() {
        return Vec::new();
    }
    serde_json::from_str(raw).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_struct_has_at_least_forty_fields() {
        // Compile-time sanity check via field-count via serde reflection:
        // serialising and counting top-level keys is the simplest available
        // proxy that survives field reorderings.
        let c = sample_commit();
        let v: serde_json::Value = serde_json::to_value(&c).unwrap();
        let obj = v.as_object().expect("commit serialises as object");
        assert!(
            obj.len() >= 40,
            "Commit must declare 40+ fields (SPEC-001 §5.1), got {}",
            obj.len()
        );
    }

    #[test]
    fn commit_struct_has_nine_classification_counters() {
        let counters = [
            "test_files_changed",
            "config_files_changed",
            "infra_files_changed",
            "doc_files_changed",
            "code_files_changed",
            "dependency_files_changed",
            "ci_files_changed",
            "fixture_files_changed",
            "migration_files_changed",
        ];
        let c = sample_commit();
        let v: serde_json::Value = serde_json::to_value(&c).unwrap();
        let obj = v.as_object().unwrap();
        for k in counters {
            assert!(obj.contains_key(k), "commit missing classification key {k}");
        }
    }

    #[test]
    fn string_list_round_trip() {
        let original = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let encoded = serialize_string_list(&original);
        assert_eq!(parse_string_list(&encoded), original);
    }

    #[test]
    fn string_list_empty_input_decodes_to_empty_vec() {
        assert!(parse_string_list("").is_empty());
        assert!(parse_string_list("   ").is_empty());
        assert!(parse_string_list("not json").is_empty());
    }

    #[test]
    fn string_list_empty_vec_encodes_to_empty_array_literal() {
        assert_eq!(serialize_string_list(&[]), "[]");
    }

    #[test]
    fn count_map_round_trip() {
        let mut original = BTreeMap::new();
        original.insert("src/a.rs".to_string(), 3);
        original.insert("src/b.rs".to_string(), 7);
        let encoded = serialize_count_map(&original);
        assert_eq!(parse_count_map(&encoded), original);
    }

    #[test]
    fn count_map_empty_round_trip() {
        assert_eq!(serialize_count_map(&BTreeMap::new()), "{}");
        assert!(parse_count_map("").is_empty());
        assert!(parse_count_map("{}").is_empty());
    }

    #[test]
    fn file_changes_round_trip() {
        let original = vec![
            FileChangeRecord {
                path: "src/main.rs".into(),
                status: 'M',
                insertions: 4,
                deletions: 1,
            },
            FileChangeRecord {
                path: "tests/integ.rs".into(),
                status: 'A',
                insertions: 99,
                deletions: 0,
            },
        ];
        let encoded = serialize_file_changes(&original);
        assert_eq!(parse_file_changes(&encoded), original);
    }

    #[test]
    fn file_changes_empty_round_trip() {
        assert_eq!(serialize_file_changes(&[]), "[]");
        assert!(parse_file_changes("").is_empty());
        assert!(parse_file_changes("[]").is_empty());
    }

    #[test]
    fn all_row_types_round_trip_serde() {
        // Every public row type must survive a serde_json round trip so the
        // eval harness can persist captured fixtures verbatim.
        macro_rules! check {
            ($val:expr) => {{
                let v = $val;
                let s = serde_json::to_string(&v).unwrap();
                let back = serde_json::from_str(&s).unwrap();
                assert_eq!(v, back);
            }};
        }
        check!(sample_commit());
        check!(Identity {
            id: 1,
            canonical_name: "Alice".into(),
            canonical_email: "alice@example.com".into(),
            first_seen_at: 1,
            last_seen_at: 2,
            commit_count: 7,
        });
        check!(IdentityAlias {
            id: 1,
            identity_id: 1,
            raw_name: "alice".into(),
            raw_email: "a@example.com".into(),
        });
        check!(CommitCoauthor {
            sha: "deadbeef".into(),
            identity_id: 2,
        });
        check!(CommitRef {
            sha: "deadbeef".into(),
            ref_name: "refs/heads/main".into(),
            ref_kind: "branch".into(),
        });
        check!(Tag {
            ref_name: "refs/tags/v1.0".into(),
            sha: "deadbeef".into(),
            annotated: 1,
            message: "Release v1.0".into(),
            tagged_at: 42,
        });
        check!(Story {
            id: 1,
            title: "Refactor auth".into(),
            date_start: 1,
            date_end: 10,
            member_count: 3,
            top_paths: serialize_string_list(&["src/auth".into()]),
            authors: serialize_string_list(&["alice@example.com".into()]),
            risk_score: 0.42,
            risk_factors: serialize_count_map(&BTreeMap::new()),
            generated_at: 99,
        });
        check!(StoryMember {
            story_id: 1,
            sha: "deadbeef".into(),
        });
        check!(PathStat {
            path: "src/lib.rs".into(),
            commit_count: 5,
            unique_authors: 2,
            revert_count: 0,
            last_touched: 100,
            cochange_paths: serialize_count_map(&BTreeMap::new()),
            top_contributors: serialize_string_list(&[]),
        });
        check!(RepoStat {
            id: 1,
            commit_count: 100,
            identity_count: 4,
            first_commit_at: 1,
            last_commit_at: 100,
            last_indexed_sha: "deadbeef".into(),
            generated_at: 100,
        });
        check!(IndexState {
            key: "schema_version".into(),
            value: "1".into(),
        });
    }

    fn sample_commit() -> Commit {
        Commit {
            sha: "0".repeat(40),
            author_name: "Alice".into(),
            author_email: "alice@example.com".into(),
            author_identity_id: Some(1),
            committer_name: "Alice".into(),
            committer_email: "alice@example.com".into(),
            committer_identity_id: Some(1),
            authored_at: 100,
            committed_at: 101,
            authored_tz_offset: 0,
            committed_tz_offset: 0,
            subject: "fix: thing".into(),
            body: String::new(),
            expanded: "fix: thing".into(),
            parent_shas: serialize_string_list(&[]),
            parent_count: 0,
            is_merge: 0,
            is_root: 1,
            files_changed: serialize_file_changes(&[]),
            file_count: 0,
            insertions: 0,
            deletions: 0,
            dirs_touched: serialize_string_list(&[]),
            dir_count: 0,
            test_files_changed: 0,
            config_files_changed: 0,
            infra_files_changed: 0,
            doc_files_changed: 0,
            code_files_changed: 0,
            dependency_files_changed: 0,
            ci_files_changed: 0,
            fixture_files_changed: 0,
            migration_files_changed: 0,
            is_revert: 0,
            reverted_by_sha: None,
            risk_score: None,
            risk_label: None,
            admission_signals: serialize_count_map(&BTreeMap::new()),
            story_id: None,
            indexed_at: 200,
            updated_at: 200,
        }
    }
}
