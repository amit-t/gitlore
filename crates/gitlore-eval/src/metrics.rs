//! Metric computations for evaluation scenarios.
//!
//! * [`mrr`]      — mean reciprocal rank over a list of judged result lists.
//! * [`ndcg_at`]  — nDCG@k over a single ranked list of relevance scores.
//! * [`jaccard`]  — set Jaccard for story-grouping fidelity.

/// Mean reciprocal rank across `queries`, where each query is a list of
/// per-rank relevance booleans ordered from rank 1 onward.
pub fn mrr<I>(queries: I) -> f64
where
    I: IntoIterator<Item = Vec<bool>>,
{
    let mut count = 0usize;
    let mut total = 0.0f64;
    for results in queries {
        count += 1;
        if let Some((idx, _)) = results.iter().enumerate().find(|(_, hit)| **hit) {
            total += 1.0 / (idx as f64 + 1.0);
        }
    }
    if count == 0 {
        0.0
    } else {
        total / count as f64
    }
}

/// nDCG@k for a ranked list of relevance scores (graded or binary).
pub fn ndcg_at(relevances: &[f64], k: usize) -> f64 {
    let cut = relevances.len().min(k);
    if cut == 0 {
        return 0.0;
    }
    let dcg: f64 = relevances[..cut]
        .iter()
        .enumerate()
        .map(|(i, rel)| rel / ((i as f64 + 2.0).log2()))
        .sum();
    let mut sorted = relevances.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let idcg: f64 = sorted[..cut]
        .iter()
        .enumerate()
        .map(|(i, rel)| rel / ((i as f64 + 2.0).log2()))
        .sum();
    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

/// Set Jaccard similarity between two iterables of comparable items.
pub fn jaccard<T, A, B>(a: A, b: B) -> f64
where
    T: Eq + std::hash::Hash,
    A: IntoIterator<Item = T>,
    B: IntoIterator<Item = T>,
{
    use std::collections::HashSet;
    let sa: HashSet<T> = a.into_iter().collect();
    let sb: HashSet<T> = b.into_iter().collect();
    let inter = sa.intersection(&sb).count();
    let union = sa.union(&sb).count();
    if union == 0 {
        0.0
    } else {
        inter as f64 / union as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mrr_empty_is_zero() {
        assert_eq!(mrr(Vec::<Vec<bool>>::new()), 0.0);
    }

    #[test]
    fn mrr_two_queries() {
        let qs = vec![vec![true, false, false], vec![false, true, false]];
        assert!((mrr(qs) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn ndcg_perfect_ranking() {
        let r = vec![1.0, 1.0, 0.0];
        assert!((ndcg_at(&r, 3) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ndcg_empty_returns_zero() {
        assert_eq!(ndcg_at(&[], 5), 0.0);
    }

    #[test]
    fn jaccard_half_overlap() {
        assert!((jaccard(vec![1, 2, 3], vec![2, 3, 4]) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn jaccard_disjoint() {
        assert_eq!(jaccard(vec![1, 2], vec![3, 4]), 0.0);
    }
}
