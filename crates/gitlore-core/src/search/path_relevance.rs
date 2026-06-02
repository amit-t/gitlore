//! Path relevance scoring for commit search results.
//!
//! This module provides deterministic scoring functions that rank commits
//! based on how well their touched directories match a path filter.

use std::path::Path;

/// Compute a deterministic relevance score for a commit's touched directories
/// against a path filter.
///
/// # Arguments
///
/// * `commit_dirs` - Slice of directory paths touched by the commit
/// * `filter_path` - Optional path filter to match against. If `None`, returns `0.0`.
///
/// # Returns
///
/// * `1.0` - Full prefix match: the filter path is a prefix of at least one commit directory
/// * `0.5` - Sibling top-level directory: the filter is a top-level directory and the commit has other top-level directories
/// * `0.0` - No match, or no filter provided
///
/// # Examples
///
/// ```rust
/// use gitlore_core::search::path_relevance::score;
///
/// // Full prefix match
/// let dirs = vec!["src/foo/bar", "src/baz"];
/// assert_eq!(score(&dirs, Some("src/foo")), 1.0);
///
/// // Sibling top-level directory
/// let dirs = vec!["docs", "src"];
/// assert_eq!(score(&dirs, Some("tests")), 0.5);
///
/// // No match
/// let dirs = vec!["docs/readme"];
/// assert_eq!(score(&dirs, Some("src/foo")), 0.0);
///
/// // No filter
/// let dirs = vec!["src/foo"];
/// assert_eq!(score(&dirs, None), 0.0);
/// ```
pub fn score(commit_dirs: &[&str], filter_path: Option<&str>) -> f32 {
    let filter = match filter_path {
        Some(f) if !f.is_empty() => f,
        _ => return 0.0,
    };

    let filter_path = Path::new(filter);
    let filter_components: Vec<_> = filter_path.components().collect();

    for dir in commit_dirs {
        let dir_path = Path::new(dir);

        // Check for full prefix match
        if dir_path.starts_with(filter_path) {
            return 1.0;
        }
    }

    // If no prefix match, check for sibling top-level directory
    // This applies when the filter is a top-level directory (single component)
    // and the commit has other top-level directories
    if filter_components.len() == 1 {
        let filter_top = filter_components
            .first()
            .and_then(|c| c.as_os_str().to_str())
            .unwrap_or("");

        for dir in commit_dirs {
            let dir_path = Path::new(dir);
            let dir_components: Vec<_> = dir_path.components().collect();
            let dir_top = dir_components
                .first()
                .and_then(|c| c.as_os_str().to_str())
                .unwrap_or("");

            // Check for sibling top-level directory:
            // Both are top-level (single component) but different names
            if !dir_top.is_empty()
                && dir_components.len() == 1
                && dir_top != filter_top
            {
                return 0.5;
            }
        }
    }

    0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_full_prefix_match() {
        let dirs = vec!["src/foo/bar", "src/baz"];
        assert_eq!(score(&dirs, Some("src/foo")), 1.0);
    }

    #[test]
    fn test_full_prefix_match_exact() {
        let dirs = vec!["src/foo", "src/bar"];
        assert_eq!(score(&dirs, Some("src/foo")), 1.0);
    }

    #[test]
    fn test_sibling_top_level_dir() {
        let dirs = vec!["docs", "src"];
        assert_eq!(score(&dirs, Some("tests")), 0.5);
    }

    #[test]
    fn test_no_match() {
        let dirs = vec!["docs/readme"];
        assert_eq!(score(&dirs, Some("src/foo")), 0.0);
    }

    #[test]
    fn test_no_filter_none() {
        let dirs = vec!["src/foo"];
        assert_eq!(score(&dirs, None), 0.0);
    }

    #[test]
    fn test_no_filter_empty() {
        let dirs = vec!["src/foo"];
        assert_eq!(score(&dirs, Some("")), 0.0);
    }

    #[test]
    fn test_empty_commit_dirs() {
        let dirs: Vec<&str> = vec![];
        assert_eq!(score(&dirs, Some("src/foo")), 0.0);
    }

    #[test]
    fn test_root_directory_match() {
        let dirs = vec!["."];
        assert_eq!(score(&dirs, Some(".")), 1.0);
    }

    #[test]
    fn test_root_directory_sibling() {
        let dirs = vec!["src/foo"];
        assert_eq!(score(&dirs, Some(".")), 0.0);
    }

    #[test]
    fn test_nested_prefix_match() {
        let dirs = vec!["a/b/c/d"];
        assert_eq!(score(&dirs, Some("a/b/c")), 1.0);
    }

    #[test]
    fn test_partial_no_match() {
        let dirs = vec!["src/foobar"];
        assert_eq!(score(&dirs, Some("src/foo")), 0.0);
    }
}
