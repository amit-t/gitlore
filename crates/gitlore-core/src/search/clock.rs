//! Testable clock abstraction (TDD-001 §2.1 / grill #17).
//!
//! All search-layer code that needs "now" goes through this trait so unit
//! tests can inject a deterministic timestamp without touching system time.

use std::time::{SystemTime, UNIX_EPOCH};

/// A source of the current Unix-epoch time in seconds.
///
/// The trait is `Send + Sync` so it can be stored in the
/// `SearchOrchestrator` behind an `Arc<dyn Clock>`.
pub trait Clock: Send + Sync {
    /// Return the current time as a Unix timestamp (seconds since epoch).
    fn now(&self) -> i64;
}

// ---------------------------------------------------------------------------
// SystemClock
// ---------------------------------------------------------------------------

/// Production implementation that delegates to [`SystemTime::now`].
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
/// Test helpers for the clock module, including [`FixedClock`] for deterministic timestamps.
pub mod tests {
    use super::*;

    /// Deterministic clock for use in tests. Returns a fixed timestamp set
    /// at construction time.
    pub struct FixedClock(pub i64);

    impl Clock for FixedClock {
        fn now(&self) -> i64 {
            self.0
        }
    }

    #[test]
    fn system_clock_returns_positive_timestamp() {
        let ts = SystemClock.now();
        // 2020-01-01 in Unix seconds
        assert!(ts > 1_577_836_800, "expected a recent timestamp, got {ts}");
    }

    #[test]
    fn fixed_clock_returns_exact_value() {
        let c = FixedClock(42);
        assert_eq!(c.now(), 42);
    }
}
