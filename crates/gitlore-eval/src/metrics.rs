//! Evaluation metrics — public API stubs.
//!
//! Concrete implementations land milestone-by-milestone via TDD specs:
//!
//! | Metric                 | Milestone | Spec     |
//! |------------------------|-----------|----------|
//! | [`mrr`]                | M4 search | TDD-001  |
//! | [`top_k_precision`]    | M4 search | TDD-001  |
//! | [`jaccard`]            | M7 story  | TDD-002  |
//! | [`mann_whitney_u`]     | M8 risk   | TDD-003  |
//!
//! Each stub returns a sentinel [`MetricUnimplemented`] error rather than a
//! placeholder number. This is deliberate: it forces scenarios that depend on
//! a metric to fail loudly until the real implementation ships, instead of
//! quietly emitting fake "passes" against `0.0`.
//!
//! Signatures are picked to match what the M4 / M7 / M8 scenarios need so
//! that landing TDD-001..003 is a body-only change, with no call-site churn.

use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;

/// Result type for every metric in this module.
pub type MetricResult<T> = Result<T, MetricUnimplemented>;

/// Sentinel error returned by unimplemented metric stubs.
///
/// Carries both the metric name and the tracking tag (milestone + TDD spec)
/// so a CI failure trace points straight at the work item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricUnimplemented {
    /// Stable metric identifier, e.g. `"mrr"`.
    pub metric: &'static str,
    /// Tracking tag, e.g. `"M4 / TDD-001"`.
    pub tracking: &'static str,
}

impl fmt::Display for MetricUnimplemented {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "metric `{}` not yet implemented (tracking: {})",
            self.metric, self.tracking
        )
    }
}

impl std::error::Error for MetricUnimplemented {}

/// Mean Reciprocal Rank across queries.
///
/// `relevant_ranks[i]` is the 1-based rank of the relevant item for query
/// `i`, or `None` if no relevant item was retrieved. The eventual
/// implementation returns `mean(1 / rank for rank in ranks if rank.is_some())`,
/// with a defined behaviour on the all-`None` case (TDD-001 pins it).
///
/// **Stub.** Lands at M4 via TDD-001.
pub fn mrr(_relevant_ranks: &[Option<usize>]) -> MetricResult<f64> {
    Err(MetricUnimplemented {
        metric: "mrr",
        tracking: "M4 / TDD-001",
    })
}

/// Top-K precision: fraction of queries that surfaced a relevant item within
/// the top `k` positions.
///
/// **Stub.** Lands at M4 via TDD-001.
pub fn top_k_precision(_relevant_ranks: &[Option<usize>], _k: usize) -> MetricResult<f64> {
    Err(MetricUnimplemented {
        metric: "top_k_precision",
        tracking: "M4 / TDD-001",
    })
}

/// Jaccard similarity `|A ∩ B| / |A ∪ B|`.
///
/// Returns a value in `[0.0, 1.0]`. TDD-002 pins behaviour for the empty
/// case (both sets empty).
///
/// **Stub.** Lands at M7 via TDD-002.
pub fn jaccard<T: Eq + Hash>(_a: &HashSet<T>, _b: &HashSet<T>) -> MetricResult<f64> {
    Err(MetricUnimplemented {
        metric: "jaccard",
        tracking: "M7 / TDD-002",
    })
}

/// Mann-Whitney U statistic for two independent samples.
///
/// Returns `(U, two_sided_p_value)`. The eventual implementation uses the
/// large-sample normal approximation with a continuity correction; TDD-003
/// pins the exact procedure and the small-sample threshold.
///
/// **Stub.** Lands at M8 via TDD-003.
pub fn mann_whitney_u(_a: &[f64], _b: &[f64]) -> MetricResult<(f64, f64)> {
    Err(MetricUnimplemented {
        metric: "mann_whitney_u",
        tracking: "M8 / TDD-003",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mrr_stub_errors_with_tracking_tag() {
        let err = mrr(&[Some(1), None]).expect_err("stub must error");
        assert_eq!(err.metric, "mrr");
        assert!(err.tracking.contains("TDD-001"));
        assert!(err.tracking.contains("M4"));
    }

    #[test]
    fn top_k_precision_stub_errors_with_tracking_tag() {
        let err = top_k_precision(&[Some(1)], 5).expect_err("stub must error");
        assert_eq!(err.metric, "top_k_precision");
        assert!(err.tracking.contains("TDD-001"));
    }

    #[test]
    fn jaccard_stub_errors_with_tracking_tag() {
        let a: HashSet<i32> = HashSet::new();
        let b: HashSet<i32> = HashSet::new();
        let err = jaccard(&a, &b).expect_err("stub must error");
        assert_eq!(err.metric, "jaccard");
        assert!(err.tracking.contains("TDD-002"));
        assert!(err.tracking.contains("M7"));
    }

    #[test]
    fn mann_whitney_u_stub_errors_with_tracking_tag() {
        let err = mann_whitney_u(&[1.0, 2.0], &[3.0, 4.0]).expect_err("stub must error");
        assert_eq!(err.metric, "mann_whitney_u");
        assert!(err.tracking.contains("TDD-003"));
        assert!(err.tracking.contains("M8"));
    }

    #[test]
    fn unimplemented_display_mentions_metric_and_tracking() {
        let e = MetricUnimplemented {
            metric: "mrr",
            tracking: "M4 / TDD-001",
        };
        let s = format!("{e}");
        assert!(s.contains("mrr"));
        assert!(s.contains("M4 / TDD-001"));
    }

    #[test]
    fn unimplemented_implements_std_error() {
        // Just check the trait bound holds at compile time.
        fn assert_error<E: std::error::Error>(_: &E) {}
        let e = MetricUnimplemented {
            metric: "x",
            tracking: "y",
        };
        assert_error(&e);
    }
}
