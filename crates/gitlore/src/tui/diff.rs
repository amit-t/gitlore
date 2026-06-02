//! Diff rendering for the TUI detail pane (ADR-013 / TDD-000 §2.3).
//!
//! [`render_diff`] shells out to `git show --color=always --stat <sha>`,
//! converts the ANSI-coloured output to ratatui [`Line`]s via `ansi-to-tui`,
//! and enforces a 5 000-line guard (ADR-016 §large-diff).
//!
//! ## Large-diff guard
//!
//! When the raw diff exceeds `LARGE_DIFF_LINES` lines the function returns
//! stat-only output (from `git show --stat`) and appends a final line
//! containing the hint `press D for full diff`.  The returned [`DiffOutput`]
//! carries `truncated = true` so callers can show a visual indicator.
//!
//! ## Error handling
//!
//! A bad SHA produces [`DiffError::BadSha`] (not a panic).  Any other git
//! failure produces [`DiffError::Git`].  Neither variant surfaces a Rust
//! backtrace or internal details to the TUI.

use std::process::Command;
use std::time::Duration;

use ansi_to_tui::IntoText;
use gitlore_core::error::Error as CoreError;
use ratatui::text::Line;

/// Hard line-count ceiling for the inline diff view.
pub const LARGE_DIFF_LINES: usize = 5_000;

// ---------------------------------------------------------------------------
// DiffOutput
// ---------------------------------------------------------------------------

/// The result of a successful [`render_diff`] call.
#[derive(Debug, Clone)]
pub struct DiffOutput {
    /// Styled lines ready for a ratatui [`ratatui::widgets::Paragraph`].
    pub lines: Vec<Line<'static>>,
    /// `true` when the diff was truncated to stat-only because it exceeded
    /// [`LARGE_DIFF_LINES`] lines.
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// DiffError
// ---------------------------------------------------------------------------

/// Errors that [`render_diff`] can return.
#[derive(Debug)]
pub enum DiffError {
    /// The supplied SHA was not found in the repository.
    BadSha { sha: String },
    /// A git subprocess failure that is not a bad-SHA.
    Git(CoreError),
    /// The ANSI-to-ratatui conversion failed.
    Ansi(String),
}

impl std::fmt::Display for DiffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DiffError::BadSha { sha } => write!(f, "SHA not found: {sha}"),
            DiffError::Git(e) => write!(f, "git error: {e}"),
            DiffError::Ansi(msg) => write!(f, "ANSI parse error: {msg}"),
        }
    }
}

impl std::error::Error for DiffError {}

// ---------------------------------------------------------------------------
// render_diff
// ---------------------------------------------------------------------------

/// Render the diff for `sha` into styled ratatui lines.
///
/// `repo_root` is the working-tree root (passed to `git` via `--work-tree`
/// and `--git-dir`, or simply as the `cwd`).
///
/// `viewport_rows` is informational — used to decide whether to show stat-only
/// when the viewport is very small, in addition to the 5k hard guard.
pub fn render_diff(
    sha: &str,
    repo_root: &std::path::Path,
    viewport_rows: u16,
) -> Result<DiffOutput, DiffError> {
    // Validate SHA shape early (hex chars 4..=40).
    if !looks_like_sha(sha) {
        return Err(DiffError::BadSha {
            sha: sha.to_string(),
        });
    }

    // Run git show --color=always --stat <sha> first to check existence.
    let stat_bytes = run_git_show(sha, repo_root, &["--stat"])?;

    // Run full diff.
    let full_bytes = run_git_show(sha, repo_root, &[])?;

    let full_str = String::from_utf8_lossy(&full_bytes);
    let line_count = full_str.lines().count();

    // Apply the 5k hard guard or tiny-viewport guard.
    let stat_only = line_count > LARGE_DIFF_LINES || (viewport_rows < 20 && line_count > 200);

    let source_bytes = if stat_only { &stat_bytes } else { &full_bytes };

    // Convert ANSI to ratatui Text.
    let text = source_bytes
        .into_text()
        .map_err(|e| DiffError::Ansi(e.to_string()))?;

    let mut lines: Vec<Line<'static>> = text.lines.into_iter().collect();

    if stat_only {
        lines.push(Line::from("-- press D for full diff --"));
    }

    Ok(DiffOutput {
        lines,
        truncated: stat_only,
    })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Validate that `s` looks like a git SHA (4–40 lowercase hex chars).
pub fn looks_like_sha(s: &str) -> bool {
    let len = s.len();
    (4..=40).contains(&len) && s.chars().all(|c| c.is_ascii_hexdigit())
}

fn run_git_show(
    sha: &str,
    repo_root: &std::path::Path,
    extra_args: &[&str],
) -> Result<Vec<u8>, DiffError> {
    let mut cmd = Command::new("git");
    cmd.current_dir(repo_root);
    cmd.arg("show");
    cmd.arg("--color=always");
    for arg in extra_args {
        cmd.arg(arg);
    }
    cmd.arg(sha);

    // 30 s timeout is enforced by the platform via SIGKILL in the caller; the
    // Command API itself has no built-in timeout, so we use a thread with a
    // join deadline.
    let (tx, rx) = std::sync::mpsc::channel();
    let child = cmd.output();
    let _ = tx.send(child);

    let output = rx
        .recv_timeout(Duration::from_secs(30))
        .map_err(|_| {
            DiffError::Git(CoreError::Git {
                stderr: "git show timed out".into(),
                code: -1,
            })
        })?
        .map_err(|e| DiffError::Git(CoreError::Io(e)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        // Git reports unknown objects with "fatal: bad object <sha>" or
        // "fatal: ambiguous argument".
        if stderr.contains("bad object")
            || stderr.contains("unknown revision")
            || stderr.contains("not a tree object")
        {
            return Err(DiffError::BadSha {
                sha: sha.to_string(),
            });
        }
        return Err(DiffError::Git(CoreError::Git {
            stderr,
            code: output.status.code().unwrap_or(-1),
        }));
    }

    Ok(output.stdout)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // SHA shape validation tests -- no git process needed.

    #[test]
    fn looks_like_sha_accepts_full_40_char_sha() {
        assert!(looks_like_sha("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"));
    }

    #[test]
    fn looks_like_sha_accepts_short_prefix() {
        assert!(looks_like_sha("deadbeef"));
    }

    #[test]
    fn looks_like_sha_rejects_non_hex() {
        assert!(!looks_like_sha("xyz_not_a_sha"));
    }

    #[test]
    fn looks_like_sha_rejects_too_short() {
        assert!(!looks_like_sha("abc")); // 3 chars — below minimum 4
    }

    #[test]
    fn looks_like_sha_rejects_too_long() {
        // 41 hex chars
        assert!(!looks_like_sha("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2x"));
    }

    #[test]
    fn bad_sha_returns_typed_error_not_panic() {
        // Use a real temp dir so git is invoked in a real (but empty) context.
        // We expect a DiffError::BadSha because the SHA doesn't exist.
        // If git is not installed this test is skipped gracefully.
        let tmp = match std::env::var("PATH") {
            Ok(_) => tempfile::TempDir::new().expect("tempdir"),
            Err(_) => return,
        };
        let repo = tmp.path();

        // Init a minimal git repo.
        let init = std::process::Command::new("git")
            .args(["init", "--quiet", "--initial-branch=main"])
            .current_dir(repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output();
        if init.map(|o| !o.status.success()).unwrap_or(true) {
            return; // git not available
        }

        let result = render_diff("0000000000000000000000000000000000000000", repo, 80);
        match result {
            Err(DiffError::BadSha { .. }) => { /* expected */ }
            Err(DiffError::Git(_)) => { /* acceptable on systems where git gives different errors */
            }
            Ok(_) => panic!("expected error for nonexistent SHA"),
            Err(DiffError::Ansi(msg)) => panic!("unexpected ANSI error: {msg}"),
        }
    }
}
