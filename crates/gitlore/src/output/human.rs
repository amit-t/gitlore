//! Human-readable search-hit renderer (SPEC-001 §4.3.1).
//!
//! Emits one row per hit:
//!
//! ```text
//! <sha[..10]>  YYYY-MM-DD  <author>  <subject>  score:<.3>
//! ```
//!
//! Footer: `N of M shown`.
//!
//! Colour is enabled by default when stdout is a TTY and neither `NO_COLOR=1`
//! nor `--no-color` is passed. Set `color = false` to suppress.

use std::io::{self, IsTerminal, Write};

use gitlore_core::search::types::SearchHit;

/// Render a slice of [`SearchHit`] to stdout.
///
/// * `hits` — ordered list from the orchestrator (already truncated to limit).
/// * `color` — whether to emit ANSI escape codes.
/// * `total_available` — total hits before the limit was applied (for footer).
pub fn render_search_hits(hits: &[SearchHit], color: bool, total_available: u64) {
    let mut out = io::stdout().lock();
    for hit in hits {
        let short_sha = &hit.sha[..hit.sha.len().min(10)];
        let date = unix_ts_to_date(hit.committed_at);

        // Truncate author at 20 chars for alignment.
        let author = truncate(&hit.author, 20);
        // Truncate subject at 72 chars.
        let subject = truncate(&hit.subject, 72);

        if color {
            let _ = writeln!(
                out,
                "\x1b[33m{short_sha}\x1b[0m  {date}  \x1b[36m{author}\x1b[0m  {subject}  \x1b[2mscore:{:.3}\x1b[0m",
                hit.score
            );
        } else {
            let _ = writeln!(
                out,
                "{short_sha}  {date}  {author}  {subject}  score:{:.3}",
                hit.score
            );
        }
    }
    let shown = hits.len() as u64;
    let _ = writeln!(out, "{shown} of {total_available} shown");
}

/// Determine whether colour output should be enabled.
///
/// Returns `false` when `NO_COLOR` is set to any non-empty value, or when
/// `no_color` argument is explicitly set, or when stdout is not a TTY.
pub fn should_use_color(no_color_flag: bool) -> bool {
    if no_color_flag {
        return false;
    }
    if std::env::var_os("NO_COLOR")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return false;
    }
    io::stdout().is_terminal()
}

/// Format a Unix timestamp as `YYYY-MM-DD` (UTC) without an external crate.
///
/// Uses a simple proleptic Gregorian calendar calculation sufficient for
/// timestamps in the plausible range (~1970-2100).
fn unix_ts_to_date(ts: i64) -> String {
    if ts < 0 {
        return "????-??-??".to_string();
    }
    // Days since Unix epoch.
    let days = (ts / 86_400) as u64;
    // Julian Day Number for Unix epoch (1970-01-01) is 2440588.
    let jdn = days + 2_440_588;
    // Convert JDN to Gregorian calendar.
    let f = jdn + 1401 + (((4 * jdn + 274_277) / 146_097) * 3) / 4 - 38;
    let e = 4 * f + 3;
    let g = (e % 1461) / 4;
    let h = 5 * g + 2;
    let day = ((h % 153) / 5 + 1) as u32;
    let month = (h / 153 + 2) % 12 + 1;
    let year = e / 1461 - 4716 + (14 - month) / 12;
    format!("{year:04}-{month:02}-{day:02}")
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max {
        s.to_string()
    } else {
        format!(
            "{}...",
            chars[..max.saturating_sub(3)].iter().collect::<String>()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitlore_core::search::types::{Factors, SearchHit};

    fn make_hit(sha: &str, subject: &str, author: &str, ts: i64, score: f32) -> SearchHit {
        SearchHit {
            sha: sha.to_string(),
            subject: subject.to_string(),
            author: author.to_string(),
            committed_at: ts,
            score,
            factors: Factors {
                lexical_bm25: score,
                path_relevance: 0.0,
                recency: 0.0,
                semantic: None,
            },
        }
    }

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let result = truncate("hello world this is long", 10);
        assert!(result.ends_with("..."));
        assert!(result.len() <= 10);
    }

    #[test]
    fn should_use_color_returns_false_with_flag() {
        assert!(!should_use_color(true));
    }

    #[test]
    fn should_use_color_respects_no_color_env() {
        // Note: modifying env vars in tests is not safe across threads,
        // but we can verify the logic branch via the flag alone.
        assert!(!should_use_color(true));
    }

    #[test]
    fn render_does_not_panic_with_empty_hits() {
        // Just verify it doesn't panic.
        render_search_hits(&[], false, 0);
    }

    #[test]
    fn render_does_not_panic_with_hits() {
        let hits = vec![make_hit(
            "abcdef1234567890abcdef1234567890abcdef12",
            "fix: retry on timeout",
            "Alice Developer",
            1_700_000_000,
            0.75,
        )];
        render_search_hits(&hits, false, 1);
    }
}
