//! Human-readable output rendering for gitlore CLI subcommands.
//!
//! This module provides rendering functions for displaying search results
//! and other data in a human-friendly format. It respects NO_COLOR and
//! explicit color flags to control terminal color output.

use std::env;
use std::io::{self, Write};

use anstream::{AutoStream, ColorChoice};

/// A search hit with all fields needed for human-readable display.
///
/// Combines the lexical search score with commit metadata (SHA, date,
/// author, subject) for formatted output.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Full commit SHA.
    pub sha: String,
    /// Author timestamp (unix epoch seconds).
    pub authored_at: i64,
    /// Author name.
    pub author: String,
    /// Commit subject line.
    pub subject: String,
    /// Relevance score (higher is better).
    pub score: f64,
}

/// Render search hits in human-readable format.
///
/// Emits one row per hit with columns:
/// - Short SHA (first 10 characters)
/// - Date in YYYY-MM-DD format
/// - Author name
/// - Subject line
/// - Score formatted to 3 decimal places
///
/// A footer line shows "N of M shown" where N is the number of hits
/// rendered and M is the total available.
///
/// # Arguments
///
/// * `hits` - Slice of search hits to render
/// * `color` - Whether to use colored output (honors NO_COLOR=1)
/// * `total_available` - Total number of hits available (for footer)
///
/// # Color handling
///
/// Colors are suppressed when:
/// - `color` is false
/// - NO_COLOR environment variable is set to "1"
/// - Output is not a TTY (handled by anstream::auto)
pub fn render_search_hits(hits: &[SearchHit], color: bool, total_available: usize) {
    let should_color = color && env::var("NO_COLOR").ok().as_deref() != Some("1");
    let stdout = io::stdout();
    let choice = if should_color {
        ColorChoice::Auto
    } else {
        ColorChoice::Never
    };
    let mut stdout = AutoStream::new(stdout.lock(), choice);

    for hit in hits {
        let short_sha = &hit.sha[..hit.sha.len().min(10)];
        let date = format_timestamp(hit.authored_at);
        let score_formatted = format!("{:.3}", hit.score);

        if should_color {
            // Colorize the short SHA in cyan
            let _ = writeln!(
                stdout,
                "\x1b[36m{}\x1b[0m\t{}\t{}\t{}\t{}",
                short_sha, date, hit.author, hit.subject, score_formatted
            );
        } else {
            let _ = writeln!(
                stdout,
                "{}\t{}\t{}\t{}\t{}",
                short_sha, date, hit.author, hit.subject, score_formatted
            );
        }
    }

    // Footer
    let shown = hits.len();
    let _ = writeln!(stdout, "{} of {} shown", shown, total_available);
}

/// Format a unix timestamp as YYYY-MM-DD.
///
/// # Arguments
///
/// * `timestamp` - Unix epoch seconds
///
/// # Returns
///
/// Date string in YYYY-MM-DD format, or "????-??-??" if the timestamp
/// is invalid.
fn format_timestamp(timestamp: i64) -> String {
    use std::time::{Duration, UNIX_EPOCH};

    if timestamp < 0 {
        return "????-??-??".to_string();
    }

    let duration = Duration::from_secs(timestamp as u64);
    if let Some(datetime) = UNIX_EPOCH.checked_add(duration) {
        // Convert SystemTime to seconds since epoch
        if let Ok(duration_since_epoch) = datetime.duration_since(UNIX_EPOCH) {
            let secs = duration_since_epoch.as_secs();
            // Convert to UTC datetime (simplified - for production use chrono)
            let days_since_epoch = (secs / 86400) as i64;
            // Unix epoch: 1970-01-01
            let epoch_year = 1970;
            let mut year = epoch_year;
            let mut remaining_days = days_since_epoch;

            // Account for leap years (simplified calculation)
            while remaining_days >= 366 {
                let days_in_year = if is_leap_year(year) { 366 } else { 365 };
                if remaining_days >= days_in_year {
                    remaining_days -= days_in_year;
                    year += 1;
                } else {
                    break;
                }
            }

            // Calculate month and day (simplified)
            let mut month = 1;
            let mut dim = days_in_month(month, year);
            while remaining_days >= dim {
                remaining_days -= dim;
                month += 1;
                if month > 12 {
                    month = 1;
                    year += 1;
                }
                dim = days_in_month(month, year);
            }

            let day = remaining_days + 1; // 1-indexed

            format!("{:04}-{:02}-{:02}", year, month, day)
        } else {
            "????-??-??".to_string()
        }
    } else {
        "????-??-??".to_string()
    }
}

/// Check if a year is a leap year.
fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

/// Get the number of days in a month (1-indexed).
fn days_in_month(month: i64, year: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => if is_leap_year(year) { 29 } else { 28 },
        _ => 30,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_timestamp_epoch() {
        let result = format_timestamp(0);
        assert!(result.starts_with("1970-01-"));
    }

    #[test]
    fn test_format_timestamp_negative() {
        let result = format_timestamp(-1);
        assert_eq!(result, "????-??-??");
    }

    #[test]
    fn test_format_timestamp_reasonable() {
        // 2024-01-01 00:00:00 UTC = 1704067200
        let result = format_timestamp(1704067200);
        assert!(result.starts_with("2024-01-"));
    }

    #[test]
    fn test_is_leap_year() {
        assert!(is_leap_year(2000));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(1900));
        assert!(!is_leap_year(2023));
    }

    #[test]
    fn test_days_in_month() {
        assert_eq!(days_in_month(1, 2024), 31);
        assert_eq!(days_in_month(2, 2024), 29);
        assert_eq!(days_in_month(2, 2023), 28);
        assert_eq!(days_in_month(4, 2024), 30);
    }

    #[test]
    fn test_search_hit_short_sha_truncates() {
        let hit = SearchHit {
            sha: "abcdef1234567890abcdef12".to_string(),
            authored_at: 0,
            author: "Test Author".to_string(),
            subject: "Test subject".to_string(),
            score: 0.5,
        };
        assert_eq!(&hit.sha[..10.min(hit.sha.len())], "abcdef1234");
    }
}
