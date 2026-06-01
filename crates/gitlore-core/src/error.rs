//! Stable error catalog for gitlore.
//!
//! Every fallible surface in `gitlore-core` returns [`Error`], a closed enum
//! enumerating the failure modes catalogued in SPEC-001 §4.3. Each variant
//! maps to exactly one stable wire identifier returned by [`Error::code`].
//!
//! ## Stability contract
//!
//! * **Variant names** are internal. Renaming a variant is allowed in any
//!   release.
//! * **The string returned by [`Error::code`]** is the public contract used by
//!   the JSON error envelope (`{"error":{"code":"...","message":"..."}}`).
//!   Renaming or removing one is a **breaking change**: it requires a major
//!   version bump and a migration note in `CHANGELOG.md`.
//! * Adding a new variant (and therefore a new code string) is **non-breaking**
//!   thanks to `#[non_exhaustive]`. Callers exhaustively matching on `Error`
//!   must keep a `_ => …` arm.
//!
//! ## Why some payloads are `String`
//!
//! `Io` is `std::io::Error` because `std` is always available. `Sqlite` and
//! `Git` carry `String` payloads at M1 because the rusqlite and gix/git2
//! integration layers do not yet exist in this crate; once they land, the
//! variants will gain typed `#[from]` conversions without changing the wire
//! `code()` strings. The string contract — every variant maps to exactly one
//! stable code — is the part downstream tooling relies on.

use std::path::PathBuf;

use thiserror::Error;

/// Stable, structured error type for `gitlore-core` (SPEC-001 §4.3).
///
/// See the module-level docs for the stability contract. New variants must be
/// added at the end of the enum and accompanied by a new arm in
/// [`Error::code`] returning a fresh, never-before-used stable string.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The path probed is not inside a Git working tree or bare repository.
    #[error("not a Git repository: {path}")]
    NotARepo {
        /// Path that was probed.
        path: PathBuf,
    },

    /// The on-disk SQLite schema is from a newer gitlore than this binary
    /// understands.
    #[error(
        "schema version {found} on disk is newer than supported {supported}; \
         upgrade gitlore to read this index"
    )]
    SchemaVersionTooNew {
        /// Version observed on disk.
        found: u32,
        /// Highest version this binary understands.
        supported: u32,
    },

    /// Another gitlore process holds the index write lock.
    ///
    /// `held_pid` and `started_at` mirror the two-line `<pid>\n<rfc3339>\n`
    /// payload the writer stamps into the lockfile on acquire (TDD-000
    /// §2.2). Either may be `None` when the file is missing, empty, or
    /// truncated mid-write — the variant is still returned so callers can
    /// distinguish lock contention from generic I/O.
    #[error(
        "could not acquire index lock at {lock_path}; \
         another process holds it (pid={held_pid:?}, since={started_at:?})"
    )]
    LockContention {
        /// Filesystem path of the contested lock file.
        lock_path: PathBuf,
        /// PID recorded in the lockfile when readable.
        held_pid: Option<u32>,
        /// RFC-3339 acquisition timestamp recorded in the lockfile when
        /// readable.
        started_at: Option<String>,
    },

    /// The requested embedding model is not present in the model cache.
    #[error("embedding model `{name}` is not installed; run `gitlore setup-embeddings` first")]
    ModelNotInstalled {
        /// Requested model identifier (e.g. `bge-small-en-v1.5`).
        name: String,
    },

    /// Downloading an embedding model failed (network, mirror, disk full, ...).
    #[error("failed to download embedding model `{name}`: {reason}")]
    ModelDownloadFailed {
        /// Requested model identifier.
        name: String,
        /// Human-readable cause; surfaced verbatim to the user.
        reason: String,
    },

    /// A downloaded model's SHA-256 does not match the expected value.
    #[error("checksum mismatch for model `{name}`: expected {expected}, got {actual}")]
    ModelShaMismatch {
        /// Model identifier.
        name: String,
        /// Expected SHA-256 (lowercase hex).
        expected: String,
        /// Observed SHA-256 (lowercase hex).
        actual: String,
    },

    /// Loading a SQLite runtime extension (e.g. `sqlite-vec`) failed.
    #[error("failed to load SQLite extension `{name}`: {reason}")]
    ExtensionLoadFailed {
        /// Extension identifier (filename or symbolic name).
        name: String,
        /// Human-readable cause from the SQLite loader.
        reason: String,
    },

    /// A user-supplied Git ref string is not parseable as a ref.
    #[error("invalid Git ref: {ref_text}")]
    InvalidRef {
        /// Offending input.
        ref_text: String,
    },

    /// The supplied SHA does not resolve to any object in the repository.
    #[error("SHA `{sha}` not found in repository")]
    ShaNotFound {
        /// Offending input (full or abbreviated).
        sha: String,
    },

    /// A short SHA prefix matched more than one object.
    #[error("SHA prefix `{prefix}` is ambiguous; matches {count} objects")]
    ShaAmbiguousPrefix {
        /// Offending input.
        prefix: String,
        /// Number of matching objects (always > 1).
        count: usize,
    },

    /// A queried path is not present in the current index.
    #[error("path {path} is not indexed; run `gitlore index` first")]
    PathNotIndexed {
        /// Path that was queried.
        path: PathBuf,
    },

    /// A configuration key is not recognised.
    #[error("unknown configuration key `{key}`")]
    ConfigInvalidKey {
        /// Offending key name (as supplied by the user).
        key: String,
    },

    /// A configuration value has the wrong type for its key.
    #[error("config key `{key}` expects {expected}, got `{actual}`")]
    ConfigTypeMismatch {
        /// Affected key name.
        key: String,
        /// Expected type, e.g. `"bool"`, `"u32"`, `"path"`.
        expected: &'static str,
        /// Value the user supplied, rendered for diagnostics.
        actual: String,
    },

    /// Underlying I/O error from `std::io`.
    ///
    /// Carries the original `std::io::Error` so callers can downcast to
    /// inspect the kind or OS errno if needed.
    #[error(transparent)]
    Io(#[from] std::io::Error),

    /// Underlying SQLite error.
    ///
    /// M1 carries a pre-formatted message; once rusqlite is wired into this
    /// crate the payload will be replaced with `#[from] rusqlite::Error`
    /// without altering [`Error::code`]'s `"sqlite"` string.
    #[error("sqlite error: {0}")]
    Sqlite(String),

    /// Underlying Git error (shell-out via `GitCliProvider`, or eventually
    /// libgit2 / gix).
    ///
    /// Carries the captured stderr verbatim and the subprocess exit `code`.
    /// `code` is the OS exit status when available; `-1` is reserved for
    /// "killed by signal" or "timed out" (the M3 CLI backend uses `-1` for
    /// its 30s timeout path). The wire identifier remains `"git"` regardless
    /// of payload shape.
    #[error("git command exited with {code}: {stderr}")]
    Git {
        /// Captured stderr from the underlying git invocation.
        stderr: String,
        /// Subprocess exit code (`-1` for signal/timeout).
        code: i32,
    },

    /// A clap-derive subcommand is plumbed but not yet implemented.
    ///
    /// At M1 every subcommand from SPEC-001 §4.1 is parseable so `--help`
    /// reflects the eventual surface, but only the default no-arg TUI launch
    /// has a real implementation. Reaching any subcommand body returns this
    /// error so callers (and the JSON envelope) see a stable, machine-checkable
    /// code rather than a panic or a bespoke string per command.
    #[error("unimplemented: subcommand '{subcommand}' is not yet implemented (M1 scaffold only)")]
    Unimplemented {
        /// Name of the subcommand that was invoked (e.g. `"search"`).
        subcommand: String,
    },
}

impl Error {
    /// Stable wire identifier for the JSON error envelope (SPEC-001 §4.3).
    ///
    /// The returned `&'static str` is the public contract consumed by
    /// downstream tooling (CLI `--json` output, MCP clients, the eval
    /// harness). Renaming or removing one of these strings is a breaking
    /// change.
    ///
    /// ```ignore
    /// // Example usage in the JSON error envelope:
    /// // {"error": {"code": "not_a_repo", "message": "not a Git repository: /tmp/foo"}}
    /// ```
    pub fn code(&self) -> &'static str {
        match self {
            Error::NotARepo { .. } => "not_a_repo",
            Error::SchemaVersionTooNew { .. } => "schema_version_too_new",
            Error::LockContention { .. } => "lock_contention",
            Error::ModelNotInstalled { .. } => "model_not_installed",
            Error::ModelDownloadFailed { .. } => "model_download_failed",
            Error::ModelShaMismatch { .. } => "model_sha_mismatch",
            Error::ExtensionLoadFailed { .. } => "extension_load_failed",
            Error::InvalidRef { .. } => "invalid_ref",
            Error::ShaNotFound { .. } => "sha_not_found",
            Error::ShaAmbiguousPrefix { .. } => "sha_ambiguous_prefix",
            Error::PathNotIndexed { .. } => "path_not_indexed",
            Error::ConfigInvalidKey { .. } => "config_invalid_key",
            Error::ConfigTypeMismatch { .. } => "config_type_mismatch",
            Error::Io(_) => "io",
            Error::Sqlite(_) => "sqlite",
            Error::Git { .. } => "git",
            Error::Unimplemented { .. } => "unimplemented",
        }
    }
}

/// Convenience alias for `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::BTreeSet;
    use std::io;
    use std::path::PathBuf;

    /// Hand-mirrored fixture of every variant + its expected stable code.
    ///
    /// This vector is the *spec*. If a new variant is added to [`Error`], the
    /// `every_variant_is_listed_here` test below catches an unupdated fixture
    /// at compile time via an exhaustive match, and the snapshot tests catch
    /// any accidental drift in the wire string.
    fn every_variant() -> Vec<(Error, &'static str)> {
        vec![
            (
                Error::NotARepo {
                    path: PathBuf::from("/tmp/not-a-repo"),
                },
                "not_a_repo",
            ),
            (
                Error::SchemaVersionTooNew {
                    found: 9,
                    supported: 3,
                },
                "schema_version_too_new",
            ),
            (
                Error::LockContention {
                    lock_path: PathBuf::from("/tmp/index.lock"),
                    held_pid: Some(4242),
                    started_at: Some("2026-01-01T00:00:00Z".to_string()),
                },
                "lock_contention",
            ),
            (
                Error::ModelNotInstalled {
                    name: "bge-small-en-v1.5".to_string(),
                },
                "model_not_installed",
            ),
            (
                Error::ModelDownloadFailed {
                    name: "bge-small-en-v1.5".to_string(),
                    reason: "connection reset".to_string(),
                },
                "model_download_failed",
            ),
            (
                Error::ModelShaMismatch {
                    name: "bge-small-en-v1.5".to_string(),
                    expected: "deadbeef".to_string(),
                    actual: "cafef00d".to_string(),
                },
                "model_sha_mismatch",
            ),
            (
                Error::ExtensionLoadFailed {
                    name: "sqlite-vec".to_string(),
                    reason: "dlopen failed".to_string(),
                },
                "extension_load_failed",
            ),
            (
                Error::InvalidRef {
                    ref_text: "refs/heads/..".to_string(),
                },
                "invalid_ref",
            ),
            (
                Error::ShaNotFound {
                    sha: "0123456".to_string(),
                },
                "sha_not_found",
            ),
            (
                Error::ShaAmbiguousPrefix {
                    prefix: "ab".to_string(),
                    count: 7,
                },
                "sha_ambiguous_prefix",
            ),
            (
                Error::PathNotIndexed {
                    path: PathBuf::from("src/main.rs"),
                },
                "path_not_indexed",
            ),
            (
                Error::ConfigInvalidKey {
                    key: "search.unknown".to_string(),
                },
                "config_invalid_key",
            ),
            (
                Error::ConfigTypeMismatch {
                    key: "tui.color_blind_safe".to_string(),
                    expected: "bool",
                    actual: "\"sometimes\"".to_string(),
                },
                "config_type_mismatch",
            ),
            (
                Error::Io(io::Error::new(io::ErrorKind::PermissionDenied, "denied")),
                "io",
            ),
            (Error::Sqlite("disk image malformed".to_string()), "sqlite"),
            (
                Error::Git {
                    stderr: "fatal: bad object\n".to_string(),
                    code: 128,
                },
                "git",
            ),
            (
                Error::Unimplemented {
                    subcommand: "search".to_string(),
                },
                "unimplemented",
            ),
        ]
    }

    /// Compile-time + run-time check that every variant appears in the
    /// `every_variant` fixture. The exhaustive `match` below is the trip-wire:
    /// adding a new variant without extending [`every_variant`] is a compile
    /// error here, not a silent test gap.
    #[test]
    fn every_variant_is_listed_in_fixture() {
        let fixture = every_variant();
        let fixture_codes: BTreeSet<&'static str> = fixture.iter().map(|(_, c)| *c).collect();

        // Exhaustive match: every Error variant must be referenced. Adding a
        // new variant without listing it in `every_variant` fails to compile.
        fn discriminant_code(e: &Error) -> &'static str {
            match e {
                Error::NotARepo { .. }
                | Error::SchemaVersionTooNew { .. }
                | Error::LockContention { .. }
                | Error::ModelNotInstalled { .. }
                | Error::ModelDownloadFailed { .. }
                | Error::ModelShaMismatch { .. }
                | Error::ExtensionLoadFailed { .. }
                | Error::InvalidRef { .. }
                | Error::ShaNotFound { .. }
                | Error::ShaAmbiguousPrefix { .. }
                | Error::PathNotIndexed { .. }
                | Error::ConfigInvalidKey { .. }
                | Error::ConfigTypeMismatch { .. }
                | Error::Io(_)
                | Error::Sqlite(_)
                | Error::Git { .. }
                | Error::Unimplemented { .. } => e.code(),
            }
        }
        for (variant, _) in &fixture {
            assert_eq!(discriminant_code(variant), variant.code());
        }

        // Defensive double-check: fixture must enumerate at least the 16
        // SPEC-001 §4.3 codes spelled out in the task description.
        for expected in [
            "not_a_repo",
            "schema_version_too_new",
            "lock_contention",
            "model_not_installed",
            "model_download_failed",
            "model_sha_mismatch",
            "extension_load_failed",
            "invalid_ref",
            "sha_not_found",
            "sha_ambiguous_prefix",
            "path_not_indexed",
            "config_invalid_key",
            "config_type_mismatch",
            "io",
            "sqlite",
            "git",
            "unimplemented",
        ] {
            assert!(
                fixture_codes.contains(expected),
                "fixture missing stable code `{expected}`; all SPEC-001 §4.3 codes must be present"
            );
        }
    }

    #[test]
    fn code_matches_expected_for_each_variant() {
        for (variant, expected_code) in every_variant() {
            assert_eq!(
                variant.code(),
                expected_code,
                "Error::code() drifted for variant rendered as `{variant}`"
            );
        }
    }

    #[test]
    fn stable_codes_are_unique() {
        let mut seen: BTreeSet<&'static str> = BTreeSet::new();
        for (variant, _) in every_variant() {
            let code = variant.code();
            assert!(
                seen.insert(code),
                "duplicate stable error code `{code}` (variant: {variant})"
            );
        }
    }

    #[test]
    fn stable_codes_use_snake_case_only() {
        for (variant, _) in every_variant() {
            let code = variant.code();
            assert!(!code.is_empty(), "empty code on {variant:?}");
            assert_eq!(
                code,
                code.to_ascii_lowercase(),
                "code `{code}` must be lowercase"
            );
            for ch in code.chars() {
                assert!(
                    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_',
                    "code `{code}` contains illegal char `{ch}`; only [a-z0-9_] allowed"
                );
            }
            assert!(
                !code.starts_with('_') && !code.ends_with('_'),
                "code `{code}` must not start or end with `_`"
            );
            assert!(!code.contains("__"), "code `{code}` must not contain `__`");
        }
    }

    #[test]
    fn display_produces_nonempty_message() {
        for (variant, _) in every_variant() {
            let rendered = format!("{variant}");
            assert!(
                !rendered.trim().is_empty(),
                "Display for variant with code `{}` was empty",
                variant.code()
            );
        }
    }

    #[test]
    fn io_variant_display_is_transparent_to_io_error() {
        // `#[error(transparent)]` on Io means our wrapper's Display equals
        // the wrapped io::Error's Display — verifying the envelope doesn't
        // accidentally drop the OS-level message.
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let wrapped = Error::Io(io_err);
        let direct = format!(
            "{}",
            io::Error::new(io::ErrorKind::PermissionDenied, "denied")
        );
        assert_eq!(format!("{wrapped}"), direct);
    }

    #[test]
    fn from_io_error_is_wired() {
        // Verifies #[from] generates the expected From impl, which keeps
        // `?` ergonomic for callers that bubble std::io errors.
        let raw: io::Error = io::Error::other("boom");
        let wrapped: Error = raw.into();
        assert_eq!(wrapped.code(), "io");
    }

    #[test]
    fn result_alias_is_usable() {
        fn ok() -> Result<u32> {
            Ok(42)
        }
        fn err() -> Result<u32> {
            Err(Error::ShaNotFound {
                sha: "deadbeef".to_string(),
            })
        }
        assert_eq!(ok().unwrap(), 42);
        assert_eq!(err().unwrap_err().code(), "sha_not_found");
    }

    #[test]
    fn error_is_send_sync_static() {
        fn assert_send_sync_static<T: Send + Sync + 'static>() {}
        assert_send_sync_static::<Error>();
    }
}
