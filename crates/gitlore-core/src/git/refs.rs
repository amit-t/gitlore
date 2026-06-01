//! Ref enumeration helpers built on top of [`GitProvider`].
//!
//! The trait's [`GitProvider::list_refs`] is scope-narrow; the indexer wants a
//! single deduplicated union of "things worth walking" with the spec's
//! excluded namespaces filtered out. Q8 nails down which namespaces those
//! are.
//!
//! # Q8 inclusion rules
//!
//! Included scopes: `refs/heads/`, `refs/remotes/`, `refs/tags/`.
//!
//! Excluded namespaces:
//!
//! * `refs/stash` — local-only working state, never long-lived history.
//! * `refs/notes/*` — annotation overlays, not commit reachability.
//! * `refs/keep/*` — pack-keep refs, internal repo maintenance.
//! * `refs/replace/*` — local rewrite mappings; following them would
//!   produce a non-repository view of history.
//! * `refs/pull/*` — GitHub/GitLab pull-request synthetic refs; sources of
//!   force-push churn we explicitly don't want to index.
//!
//! Exclusion is applied even when a backend's [`GitProvider::list_refs`]
//! ever expands its scope to include them: [`enumerate_refs`] is the
//! authoritative filter.
//!
//! # Force-push retention
//!
//! [`force_push_retention`] takes a list of SHAs the index believes it has
//! seen and asks the provider whether each still resolves. The returned
//! vector lists orphans: SHAs whose underlying objects have been pruned out
//! of the repository (typically by a force-push followed by `git gc`).
//! Callers use this to decide which rows to evict from the index.

use crate::error::Result;
use crate::git::{GitProvider, RefEntry, RefScope, Sha};

/// Threshold for the one-shot "too many remote refs" warning. Hit when the
/// union contains more than this many `refs/remotes/*` entries.
pub const REMOTE_REF_WARN_THRESHOLD: usize = 200;

/// Names whose entire namespace must be excluded from [`enumerate_refs`]
/// (matched on the full ref name, prefix-style).
const EXCLUDED_PREFIXES: &[&str] = &["refs/notes/", "refs/keep/", "refs/replace/", "refs/pull/"];

/// Exact ref names to exclude (no trailing `/`).
const EXCLUDED_EXACT: &[&str] = &["refs/stash"];

/// Return the deduplicated union of heads, remotes, and tags after applying
/// the Q8 exclusions.
///
/// Order is preserved per scope: heads first (sorted by `for-each-ref`'s
/// natural lexicographic order from the backend), then remotes, then tags.
/// Duplicate `(name, sha)` entries are dropped on first sight (a ref name
/// uniquely keys the dedup).
///
/// Emits a one-shot WARN tracing event when the remote-ref count exceeds
/// [`REMOTE_REF_WARN_THRESHOLD`] so monorepos with stale forks get a
/// nudge toward pruning.
pub fn enumerate_refs(provider: &dyn GitProvider) -> Result<Vec<RefEntry>> {
    let heads = provider.list_refs(RefScope::Heads)?;
    let remotes = provider.list_refs(RefScope::Remotes)?;
    let tags = provider.list_refs(RefScope::Tags)?;

    if remotes.len() > REMOTE_REF_WARN_THRESHOLD {
        tracing::warn!(
            count = remotes.len(),
            "more than {REMOTE_REF_WARN_THRESHOLD} remote refs detected; consider pruning"
        );
    }

    let mut out: Vec<RefEntry> = Vec::with_capacity(heads.len() + remotes.len() + tags.len());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in heads.into_iter().chain(remotes).chain(tags) {
        if !ref_is_included(&entry.name) {
            continue;
        }
        if seen.insert(entry.name.clone()) {
            out.push(entry);
        }
    }
    Ok(out)
}

/// True when the supplied full ref name (`refs/heads/main`, `refs/notes/foo`,
/// etc.) should be retained by [`enumerate_refs`].
pub fn ref_is_included(name: &str) -> bool {
    if EXCLUDED_EXACT.contains(&name) {
        return false;
    }
    if EXCLUDED_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return false;
    }
    true
}

/// Return the subset of `known_shas` that no longer resolve in the
/// repository.
///
/// Probes each SHA via [`GitProvider::cat_file_exists`]; SHAs that come back
/// as missing (the well-formed-but-absent case) are returned. Order in the
/// returned vector mirrors the input order to make diff-style consumption
/// predictable.
///
/// Wall-clock cost is linear in the input length. Callers feeding very
/// large lists should batch upstream — there is no internal batching today
/// (none of the M3 callers exceed a few hundred SHAs).
pub fn force_push_retention(provider: &dyn GitProvider, known_shas: &[Sha]) -> Result<Vec<Sha>> {
    let mut orphans = Vec::new();
    for sha in known_shas {
        if !provider.cat_file_exists(sha)? {
            orphans.push(sha.clone());
        }
    }
    Ok(orphans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{RefEntry, RefType};
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Minimal in-memory GitProvider double for unit testing.
    struct FakeProvider {
        heads: Vec<RefEntry>,
        remotes: Vec<RefEntry>,
        tags: Vec<RefEntry>,
        // SHAs the fake repo "has". cat_file_exists returns Ok(true) for these.
        present: Mutex<std::collections::HashSet<String>>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                heads: vec![],
                remotes: vec![],
                tags: vec![],
                present: Mutex::new(Default::default()),
            }
        }
        fn with_present(self, shas: &[&str]) -> Self {
            {
                let mut p = self.present.lock().unwrap();
                for s in shas {
                    p.insert((*s).to_string());
                }
            }
            self
        }
        fn mk_ref(name: &str, sha: &str, ref_type: RefType) -> RefEntry {
            RefEntry {
                name: name.into(),
                sha: Sha::new(sha).unwrap(),
                ref_type,
            }
        }
    }

    impl GitProvider for FakeProvider {
        fn common_dir(&self) -> Result<PathBuf> {
            Ok(PathBuf::from("/tmp/.git"))
        }
        fn rev_parse(&self, _refname: &str) -> Result<Sha> {
            unimplemented!()
        }
        fn list_refs(&self, scope: RefScope) -> Result<Vec<RefEntry>> {
            Ok(match scope {
                RefScope::Heads => self.heads.clone(),
                RefScope::Remotes => self.remotes.clone(),
                RefScope::Tags => self.tags.clone(),
            })
        }
        fn walk_commits(
            &self,
            _range: crate::git::WalkRange,
        ) -> Result<Vec<crate::git::RawCommit>> {
            unimplemented!()
        }
        fn show(&self, _sha: &Sha, _opts: crate::git::ShowOpts) -> Result<String> {
            unimplemented!()
        }
        fn check_mailmap(&self, _name: &str, _email: &str) -> Result<crate::git::MailmapResolved> {
            unimplemented!()
        }
        fn cat_file_exists(&self, sha: &Sha) -> Result<bool> {
            Ok(self.present.lock().unwrap().contains(sha.as_str()))
        }
    }

    #[test]
    fn ref_filter_excludes_documented_namespaces() {
        assert!(ref_is_included("refs/heads/main"));
        assert!(ref_is_included("refs/remotes/origin/main"));
        assert!(ref_is_included("refs/tags/v1.0"));
        assert!(!ref_is_included("refs/stash"));
        assert!(!ref_is_included("refs/notes/commits"));
        assert!(!ref_is_included("refs/keep/abc"));
        assert!(!ref_is_included("refs/replace/abc"));
        assert!(!ref_is_included("refs/pull/42/head"));
    }

    #[test]
    fn enumerate_unions_three_scopes() {
        let mut p = FakeProvider::new();
        p.heads = vec![FakeProvider::mk_ref(
            "refs/heads/main",
            "aa11",
            RefType::Branch,
        )];
        p.remotes = vec![FakeProvider::mk_ref(
            "refs/remotes/origin/main",
            "aa11",
            RefType::RemoteBranch,
        )];
        p.tags = vec![FakeProvider::mk_ref("refs/tags/v1.0", "bb22", RefType::Tag)];
        let refs = enumerate_refs(&p).unwrap();
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].name, "refs/heads/main");
        assert_eq!(refs[1].name, "refs/remotes/origin/main");
        assert_eq!(refs[2].name, "refs/tags/v1.0");
    }

    #[test]
    fn enumerate_dedupes_by_name() {
        let mut p = FakeProvider::new();
        p.heads = vec![
            FakeProvider::mk_ref("refs/heads/main", "aa11", RefType::Branch),
            FakeProvider::mk_ref("refs/heads/main", "aa11", RefType::Branch),
        ];
        let refs = enumerate_refs(&p).unwrap();
        assert_eq!(refs.len(), 1);
    }

    #[test]
    fn force_push_retention_returns_orphans_in_order() {
        let p = FakeProvider::new().with_present(&["aa11", "cc33"]);
        let shas = vec![
            Sha::new("aa11").unwrap(),
            Sha::new("bb22").unwrap(),
            Sha::new("cc33").unwrap(),
            Sha::new("dd44").unwrap(),
        ];
        let orphans = force_push_retention(&p, &shas).unwrap();
        assert_eq!(
            orphans,
            vec![Sha::new("bb22").unwrap(), Sha::new("dd44").unwrap()]
        );
    }

    #[test]
    fn force_push_retention_empty_input() {
        let p = FakeProvider::new();
        assert!(force_push_retention(&p, &[]).unwrap().is_empty());
    }
}
