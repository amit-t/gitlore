//! Git access layer for gitlore (M3-1, SPEC-001 §4.4 / TDD-000 §2.2).
//!
//! This module defines the read-only [`GitProvider`] trait and its supporting
//! value types. The trait is the single seam between the indexer / TUI / CLI
//! and any concrete Git backend (CLI-shell-out for v0, `git2-rs` reserved for
//! Phase 3 per OQ-T-3). Every method is read-only by contract; no
//! [`GitProvider`] implementation may invoke a Git subcommand that mutates the
//! repository (verified by the `no_git_write_subcommand` integration test).
//!
//! ## Concrete backends
//!
//! * [`cli::GitCliProvider`] — shells out to the system `git` binary. Default
//!   for v0; built without any extra Cargo features.
//! * `git2-rs` backend — reserved (Phase 3); will live in its own module
//!   behind the optional `git2` feature.
//!
//! ## Helpers
//!
//! * [`refs::enumerate_refs`] — union of `refs/heads/`, `refs/remotes/`, and
//!   `refs/tags/` with the spec's documented exclusions (Q8).
//! * [`refs::force_push_retention`] — given a list of `Sha`s, returns the
//!   subset that no longer resolve in the repository (force-push orphans).

pub mod cli;
pub mod refs;

use std::path::PathBuf;

use crate::error::{Error, Result};

/// A Git object SHA (full-length or abbreviated), normalised to lowercase hex.
///
/// `Sha` is a transparent newtype around `String` that enforces hex validity
/// at construction. It rejects empty input, non-hex characters, and strings
/// longer than 64 chars (room for SHA-256 + a small safety margin) so the
/// type can stand in safely for both SHA-1 and the SHA-256 transition.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct Sha(String);

impl Sha {
    /// Validate `text` and return a `Sha`, normalising hex to lowercase.
    ///
    /// Returns [`Error::InvalidRef`] when `text` is empty, contains non-hex
    /// characters, or exceeds 64 chars.
    pub fn new(text: impl Into<String>) -> Result<Self> {
        let s = text.into();
        if s.is_empty() || s.len() > 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::InvalidRef { ref_text: s });
        }
        Ok(Self(s.to_ascii_lowercase()))
    }

    /// Borrow the underlying hex string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the `Sha` and return the underlying `String`.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for Sha {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Which ref namespace to enumerate via [`GitProvider::list_refs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefScope {
    /// Local branches under `refs/heads/`.
    Heads,
    /// Remote-tracking branches under `refs/remotes/`.
    Remotes,
    /// Tags under `refs/tags/`.
    Tags,
}

/// The kind of ref a [`RefEntry`] came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefType {
    /// A local branch (`refs/heads/<name>`).
    Branch,
    /// A remote-tracking branch (`refs/remotes/<remote>/<name>`).
    RemoteBranch,
    /// A tag (`refs/tags/<name>`). Lightweight and annotated tags both surface
    /// here; the SHA points to the commit (annotated tags are dereferenced).
    Tag,
}

/// A single entry from [`GitProvider::list_refs`] or
/// [`refs::enumerate_refs`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEntry {
    /// Full ref name including the leading namespace, e.g.
    /// `refs/heads/main`, `refs/remotes/origin/main`, `refs/tags/v1.2.3`.
    pub name: String,
    /// Commit SHA the ref points to (annotated tags are dereferenced to the
    /// underlying commit).
    pub sha: Sha,
    /// Discriminator matching [`RefScope`].
    pub ref_type: RefType,
}

/// A commit walk range supplied to [`GitProvider::walk_commits`].
///
/// Semantics mirror `git log <from>..<to>`: when `from` is `Some`, commits
/// reachable from `to` but not from `from` are returned. When `from` is
/// `None`, every commit reachable from `to` is returned. `max` caps the
/// returned vector length (mapped onto `git log -n`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkRange {
    /// Exclusive lower bound (`A` in `A..B`). `None` means "from the root".
    pub from: Option<Sha>,
    /// Inclusive upper bound (`B` in `A..B`).
    pub to: Sha,
    /// Optional cap on the number of commits returned.
    pub max: Option<usize>,
}

/// A single file change on a commit, parsed from `--name-status --numstat`.
///
/// `insertions` and `deletions` may be `0` for binary files, which is how
/// `git log --numstat` marks them (the literal `-` is normalised to `0`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChange {
    /// Path of the changed file (post-rename for `R<NN>` entries).
    pub path: String,
    /// One-letter status code: `A` add, `M` modify, `D` delete, `R` rename,
    /// `C` copy, `T` type-change, `U` unmerged, `X` unknown.
    pub status: char,
    /// Lines added (`0` for binary).
    pub insertions: u64,
    /// Lines removed (`0` for binary).
    pub deletions: u64,
}

/// Decoded commit metadata as produced by [`GitProvider::walk_commits`].
///
/// Times are unix-epoch seconds. Body is the commit message body sans
/// subject (matches `git log %b`). `coauthors` are parsed from
/// `Co-authored-by:` trailer lines in the body per the conventional commits
/// trailer convention; only well-formed `Name <email>` pairs are surfaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawCommit {
    /// Commit SHA.
    pub sha: Sha,
    /// Author's display name.
    pub author_name: String,
    /// Author's email.
    pub author_email: String,
    /// Committer's display name.
    pub committer_name: String,
    /// Committer's email.
    pub committer_email: String,
    /// Author timestamp (unix epoch seconds, UTC).
    pub authored_at: i64,
    /// Committer timestamp (unix epoch seconds, UTC).
    pub committed_at: i64,
    /// Single-line commit subject (matches `git log %s`).
    pub subject: String,
    /// Commit body (everything after the blank line that follows the subject;
    /// matches `git log %b`).
    pub body: String,
    /// Parent SHAs in order; empty for root commits, two-plus for merges.
    pub parent_shas: Vec<Sha>,
    /// File-level changes as reported by `--name-status --numstat`. Empty
    /// for merge commits when the chosen backend does not request `-m`.
    pub files_changed: Vec<FileChange>,
    /// Top-level directories touched by `files_changed`, deduplicated and
    /// sorted; convenience for hotspot/risk consumers.
    pub dirs_touched: Vec<String>,
    /// `(name, email)` pairs parsed from `Co-authored-by:` trailers in
    /// `body`. Empty when the commit has no co-author trailer.
    pub coauthors: Vec<(String, String)>,
}

/// Knobs passed to [`GitProvider::show`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShowOpts {
    /// When `true`, request ANSI-coloured output (`--color=always`).
    pub color: bool,
    /// When `true`, append a diffstat summary (`--stat`) to the show output.
    pub stat: bool,
}

/// Output of [`GitProvider::check_mailmap`]: the canonical name/email pair
/// after applying the repository's `.mailmap` rewrites.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailmapResolved {
    /// Canonical author display name.
    pub name: String,
    /// Canonical author email.
    pub email: String,
}

/// Read-only Git access seam used by the indexer, search, story, risk, and
/// hotspot engines.
///
/// All methods return [`Error`] on failure and never mutate the repository.
/// Implementations should map non-zero subprocess exits to [`Error::Git`].
pub trait GitProvider: Send + Sync {
    /// Resolve the repository's common dir (the path that backs
    /// `.git/objects`, shared across worktrees). Equivalent to
    /// `git rev-parse --git-common-dir` normalised to an absolute path.
    fn common_dir(&self) -> Result<PathBuf>;

    /// Resolve a ref string to a commit SHA. The input may be a branch name,
    /// tag, partial SHA, or any expression `git rev-parse` accepts. Returns
    /// [`Error::ShaNotFound`] when the ref does not resolve.
    fn rev_parse(&self, refname: &str) -> Result<Sha>;

    /// List every ref in the given `scope`.
    fn list_refs(&self, scope: RefScope) -> Result<Vec<RefEntry>>;

    /// Walk commits in `range` and return their decoded metadata.
    fn walk_commits(&self, range: WalkRange) -> Result<Vec<RawCommit>>;

    /// Render `git show <sha>` with `opts`. The returned `String` is the raw
    /// stdout (may contain ANSI escapes when `opts.color` is `true`).
    fn show(&self, sha: &Sha, opts: ShowOpts) -> Result<String>;

    /// Apply the repository's `.mailmap` rewrites to the supplied
    /// `name`/`email` pair.
    fn check_mailmap(&self, name: &str, email: &str) -> Result<MailmapResolved>;

    /// Return `true` iff `sha` resolves to an existing object. Implementations
    /// must return `Ok(false)` rather than [`Error::ShaNotFound`] when the
    /// SHA is well-formed but absent, so callers can detect force-push
    /// orphans without pattern-matching error variants.
    fn cat_file_exists(&self, sha: &Sha) -> Result<bool>;
}

/// Extract `(name, email)` pairs from `Co-authored-by:` trailers in a commit
/// message body.
///
/// Recognises the conventional commits trailer format:
///
/// ```text
/// Co-authored-by: Alice Example <alice@example.com>
/// ```
///
/// Matching is case-insensitive on the trailer key, trims surrounding
/// whitespace, and silently skips malformed lines. Duplicate `(name, email)`
/// pairs are de-duplicated, preserving first-seen order.
pub fn parse_coauthor_trailers(body: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("co-authored-by") {
            continue;
        }
        let value = value.trim();
        let open = match value.rfind('<') {
            Some(i) => i,
            None => continue,
        };
        let close = match value.rfind('>') {
            Some(i) => i,
            None => continue,
        };
        if close <= open + 1 {
            continue;
        }
        let name = value[..open].trim().to_string();
        let email = value[open + 1..close].trim().to_string();
        if name.is_empty() || email.is_empty() {
            continue;
        }
        let pair = (name, email);
        if !out.contains(&pair) {
            out.push(pair);
        }
    }
    out
}

/// Compute the deduplicated, sorted list of top-level directories from a
/// list of `FileChange.path` entries. A path with no `/` contributes the
/// sentinel `"."` (repository root).
pub fn top_level_dirs(files: &[FileChange]) -> Vec<String> {
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for f in files {
        match f.path.split_once('/') {
            Some((dir, _)) if !dir.is_empty() => {
                set.insert(dir.to_string());
            }
            _ => {
                set.insert(".".to_string());
            }
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha_accepts_full_sha1() {
        let s = Sha::new("0123456789abcdef0123456789abcdef01234567").unwrap();
        assert_eq!(s.as_str(), "0123456789abcdef0123456789abcdef01234567");
    }

    #[test]
    fn sha_lowercases_hex() {
        let s = Sha::new("DEADBEEF").unwrap();
        assert_eq!(s.as_str(), "deadbeef");
    }

    #[test]
    fn sha_accepts_short_prefix() {
        let s = Sha::new("abc1234").unwrap();
        assert_eq!(s.as_str(), "abc1234");
    }

    #[test]
    fn sha_rejects_empty() {
        let e = Sha::new("").unwrap_err();
        assert_eq!(e.code(), "invalid_ref");
    }

    #[test]
    fn sha_rejects_non_hex() {
        let e = Sha::new("zzz").unwrap_err();
        assert_eq!(e.code(), "invalid_ref");
    }

    #[test]
    fn sha_rejects_overlong() {
        let too_long = "a".repeat(65);
        let e = Sha::new(too_long).unwrap_err();
        assert_eq!(e.code(), "invalid_ref");
    }

    #[test]
    fn sha_display_round_trips() {
        let s = Sha::new("ff01").unwrap();
        assert_eq!(format!("{s}"), "ff01");
    }

    #[test]
    fn coauthor_trailer_parses_single() {
        let body = "fix: thing\n\nCo-authored-by: Alice <alice@example.com>\n";
        assert_eq!(
            parse_coauthor_trailers(body),
            vec![("Alice".to_string(), "alice@example.com".to_string())]
        );
    }

    #[test]
    fn coauthor_trailer_is_case_insensitive_on_key() {
        let body = "Co-Authored-By: Bob <b@b>\nco-authored-by: Carol <c@c>";
        assert_eq!(
            parse_coauthor_trailers(body),
            vec![
                ("Bob".to_string(), "b@b".to_string()),
                ("Carol".to_string(), "c@c".to_string()),
            ]
        );
    }

    #[test]
    fn coauthor_trailer_dedupes() {
        let body =
            "Co-authored-by: Alice <alice@example.com>\nCo-authored-by: Alice <alice@example.com>";
        assert_eq!(
            parse_coauthor_trailers(body),
            vec![("Alice".to_string(), "alice@example.com".to_string())]
        );
    }

    #[test]
    fn coauthor_trailer_skips_malformed() {
        let body = "Co-authored-by: no email here\nCo-authored-by: <only@email>\nSigned-off-by: Alice <alice@example.com>\nCo-authored-by: Real <real@example.com>";
        assert_eq!(
            parse_coauthor_trailers(body),
            vec![("Real".to_string(), "real@example.com".to_string())]
        );
    }

    #[test]
    fn coauthor_trailer_handles_multiple() {
        let body = "Co-authored-by: A <a@x>\nCo-authored-by: B <b@x>\nCo-authored-by: C <c@x>\n";
        assert_eq!(parse_coauthor_trailers(body).len(), 3);
    }

    #[test]
    fn top_level_dirs_dedupes_and_sorts() {
        let files = vec![
            FileChange {
                path: "src/main.rs".into(),
                status: 'M',
                insertions: 1,
                deletions: 0,
            },
            FileChange {
                path: "src/lib.rs".into(),
                status: 'M',
                insertions: 1,
                deletions: 0,
            },
            FileChange {
                path: "docs/intro.md".into(),
                status: 'A',
                insertions: 5,
                deletions: 0,
            },
            FileChange {
                path: "README.md".into(),
                status: 'M',
                insertions: 2,
                deletions: 1,
            },
        ];
        assert_eq!(top_level_dirs(&files), vec![".", "docs", "src"]);
    }

    #[test]
    fn ref_scope_values_are_distinct() {
        assert_ne!(RefScope::Heads, RefScope::Remotes);
        assert_ne!(RefScope::Remotes, RefScope::Tags);
        assert_ne!(RefScope::Heads, RefScope::Tags);
    }
}
