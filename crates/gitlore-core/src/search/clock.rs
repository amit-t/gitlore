//! Clock abstraction for deterministic time in tests.
//!
//! This module defines the [`Clock`] trait, which provides a single method
//! [`Clock::now`] for obtaining the current time as unix epoch seconds.
//! The trait is designed to be injectable so that tests can provide a
//! deterministic [`Clock`] implementation while production code uses
//! [`SystemClock`].

use std::time::{SystemTime, UNIX_EPOCH};

/// A source of time as unix epoch seconds.
///
/// The trait is intentionally minimal: a single [`Clock::now`] method
/// returning seconds since the unix epoch. This abstraction enables
/// deterministic time in tests by injecting a mock implementation.
pub trait Clock: Send + Sync {
    /// Return the current time as unix epoch seconds (UTC).
    fn now(&self) -> i64;
}

/// Production [`Clock`] implementation backed by [`SystemTime`].
///
/// This is the default implementation used in production code. It delegates
/// to [`SystemTime::now`] and converts the duration since the unix epoch to
/// seconds, truncating any fractional part.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time must be >= unix epoch")
            .as_secs() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_returns_positive_seconds() {
        let clock = SystemClock;
        let now = clock.now();
        assert!(now > 0, "unix epoch seconds should be positive");
    }

    #[test]
    fn system_clock_is_monotonic() {
        let clock = SystemClock;
        let t1 = clock.now();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t2 = clock.now();
        assert!(t2 >= t1, "time should be monotonic");
    }

    #[test]
    fn system_clock_is_reasonable() {
        let clock = SystemClock;
        let now = clock.now();
        // Unix epoch for 2020-01-01 is 1577836800
        // Assume we're running after 2020
        assert!(now > 1577836800, "time should be after 2020-01-01");
    }
}
