//! Recency scoring using exponential decay.
//!
//! This module implements a time-decay function that assigns higher scores to
//! more recent commits. The decay follows an exponential curve with a configurable
//! half-life.

use std::time::SystemTime;

/// Computes a recency score using exponential decay.
///
/// The score follows the formula:
/// ```text
/// score = 0.5^((now - committed_at) / (half_life_days * 86_400))
/// ```
///
/// where `86_400` is the number of seconds in a day. The result is clamped to
/// the range `[0.0, 1.0]`. If `now < committed_at` (clock skew), the function
/// returns `1.0`.
///
/// # Arguments
///
/// * `committed_at` - The timestamp when the commit was created (Unix timestamp in seconds)
/// * `now` - The current timestamp (Unix timestamp in seconds)
/// * `half_life_days` - The half-life in days for the exponential decay
///
/// # Returns
///
/// A score in the range `[0.0, 1.0]` where `1.0` represents a commit made at
/// `now` and lower values represent older commits.
///
/// # Examples
///
/// ```
/// use gitlore_core::search::recency::score;
///
/// // Commit made exactly now
/// assert_eq!(score(1000, 1000, 180), 1.0);
///
/// // Clock skew: now < committed_at
/// assert_eq!(score(1000, 900, 180), 1.0);
///
/// // Commit made one half-life ago
/// let half_life_seconds = 180 * 86_400;
/// let result = score(0, half_life_seconds, 180);
/// assert!((result - 0.5).abs() < 1e-6);
/// ```
pub fn score(committed_at: u64, now: u64, half_life_days: u32) -> f32 {
    // Handle clock skew: if now < committed_at, treat as if they're equal
    if now < committed_at {
        return 1.0;
    }

    let elapsed_seconds = now - committed_at;
    let half_life_seconds = half_life_days as f32 * 86_400.0;

    // Compute exponential decay: 0.5^(elapsed / half_life)
    let decay = 0.5_f32.powf(elapsed_seconds as f32 / half_life_seconds);

    // Clamp to [0.0, 1.0] (though the formula should never exceed 1.0)
    decay.clamp(0.0, 1.0)
}

/// Computes a recency score using the current system time.
///
/// This is a convenience wrapper around [`score`] that uses `SystemTime::now()`
/// converted to a Unix timestamp.
///
/// # Arguments
///
/// * `committed_at` - The timestamp when the commit was created (Unix timestamp in seconds)
/// * `half_life_days` - The half-life in days for the exponential decay
///
/// # Returns
///
/// A score in the range `[0.0, 1.0]`, or `None` if the system time is unavailable
/// or is before the Unix epoch.
pub fn score_with_system_time(committed_at: u64, half_life_days: u32) -> Option<f32> {
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    Some(score(committed_at, now, half_life_days))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_now() {
        // Commit made exactly now should score 1.0
        assert_eq!(score(1000, 1000, 180), 1.0);
        assert_eq!(score(0, 0, 180), 1.0);
    }

    #[test]
    fn test_score_clock_skew() {
        // Clock skew: now < committed_at should clamp to 1.0
        assert_eq!(score(1000, 900, 180), 1.0);
        assert_eq!(score(5000, 1000, 180), 1.0);
    }

    #[test]
    fn test_score_half_life() {
        // At exactly one half-life, score should be 0.5
        let half_life_seconds = 180 * 86_400;
        let result = score(0, half_life_seconds, 180);
        assert!((result - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_score_two_half_lives() {
        // At two half-lives, score should be 0.25
        let half_life_seconds = 180 * 86_400;
        let result = score(0, 2 * half_life_seconds, 180);
        assert!((result - 0.25).abs() < 1e-6);
    }

    #[test]
    fn test_score_very_old() {
        // Very old commits should approach 0.0
        let half_life_seconds = 180 * 86_400;
        let result = score(0, 10 * half_life_seconds, 180);
        assert!(result < 0.01);
        assert!(result >= 0.0);
    }

    #[test]
    fn test_score_different_half_lives() {
        // Same elapsed time, different half-lives
        let elapsed = 86_400; // 1 day
        let score_90 = score(0, elapsed, 90);
        let score_180 = score(0, elapsed, 180);
        let score_360 = score(0, elapsed, 360);

        // Shorter half-life = faster decay = lower score
        assert!(score_90 < score_180);
        assert!(score_180 < score_360);
    }

    #[test]
    fn test_score_clamping() {
        // The formula should never produce values outside [0.0, 1.0]
        // Test a range of values
        for half_life_days in [1, 30, 180, 365] {
            for elapsed_days in [0, 1, 10, 100, 1000, 10000] {
                let elapsed_seconds = elapsed_days * 86_400;
                let result = score(0, elapsed_seconds, half_life_days);
                assert!((0.0..=1.0).contains(&result));
            }
        }
    }

    #[test]
    fn test_score_with_system_time() {
        // Test that score_with_system_time produces a valid result
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let result = score_with_system_time(now, 180);
        assert!(result.is_some());
        assert!(result.unwrap() >= 0.0 && result.unwrap() <= 1.0);
    }

    #[test]
    fn test_score_with_system_time_old_commit() {
        // Test with a commit from the past
        let now = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let one_day_ago = now.saturating_sub(86_400);

        let result = score_with_system_time(one_day_ago, 180);
        assert!(result.is_some());
        // Should be slightly less than 1.0 but close
        let score = result.unwrap();
        assert!(score < 1.0);
        assert!(score > 0.99);
    }
}
