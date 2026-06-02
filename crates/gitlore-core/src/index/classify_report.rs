//! Read-only classifier reporters (M3-7b, SPEC-001 §4.1 + §4.4).
//!
//! Powers `gitlore classify`:
//!
//! * [`ClassifyGlobReport::for_paths`] — classify a list of repo-relative
//!   paths against the embedded defaults + ecosystem overlays for
//!   `repo_root`. Used by the glob form once the CLI handler has walked
//!   `git ls-files` and applied the glob filter.
//! * [`ClassifyExplainReport::read_for_sha`] — open the index read-only,
//!   look up `commits.files_changed` for the supplied SHA (exact or
//!   prefix), and classify every file in the commit.
//!
//! Both helpers keep `rusqlite` and `Classifier` plumbing inside
//! `gitlore-core` so the binary crate stays free of the SQLite dependency
//! (matches the M3-7a [`crate::index::status::StatusReport`] split).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git::GitProvider;
use crate::index::classify::{Category, Classifier};
use crate::index::indexer::INDEX_DB_FILENAME;
use crate::index::schema::parse_file_changes;
use crate::index::storage::resolve_index_path;

/// One `(path, category)` pair, classification output for either entry
/// point in this module.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifiedFile {
    /// Repo-relative path.
    pub path: String,
    /// Stable kebab-case category id (`code`, `test`, …) per
    /// [`Category::as_str`].
    pub category: String,
}

/// Payload for `gitlore classify <glob>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifyGlobReport {
    /// Echo of the input glob (verbatim).
    pub glob: String,
    /// Classified entries, one per matching path.
    pub matched_files: Vec<ClassifiedFile>,
    /// Single category iff every matched file resolves to the same one;
    /// `None` when the glob spans multiple categories or matched nothing.
    pub category: Option<String>,
}

impl ClassifyGlobReport {
    /// Build a report by classifying every path in `paths` against the
    /// classifier `Classifier::default_for(repo_root)` resolves to.
    ///
    /// `paths` is the caller's filtered output (e.g. `git ls-files` matched
    /// against the user-supplied glob); this helper does no I/O beyond
    /// loading the embedded defaults + ecosystem overlays.
    pub fn for_paths(repo_root: &Path, glob: &str, paths: &[String]) -> Result<Self> {
        let classifier = Classifier::default_for(repo_root)?;
        let mut matched_files: Vec<ClassifiedFile> = paths
            .iter()
            .map(|p| ClassifiedFile {
                path: p.clone(),
                category: classifier.classify(p).as_str().to_string(),
            })
            .collect();
        matched_files.sort_by(|a, b| a.path.cmp(&b.path));

        let category = homogeneous_category(&matched_files);
        Ok(Self {
            glob: glob.to_string(),
            matched_files,
            category,
        })
    }
}

/// Payload for `gitlore classify --explain <sha>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassifyExplainReport {
    /// Full SHA resolved from the user-supplied (possibly abbreviated)
    /// input.
    pub sha: String,
    /// Classified entries, one per file the commit touched.
    pub files: Vec<ClassifiedFile>,
}

impl ClassifyExplainReport {
    /// Resolve `sha` against the SQLite index and classify every file the
    /// commit touched.
    ///
    /// Resolution rule (per task spec):
    ///
    /// ```text
    /// SELECT sha, files_changed FROM commits WHERE sha = ? OR sha LIKE ?
    /// ```
    ///
    /// — exact match wins; otherwise any commit whose SHA starts with the
    /// supplied prefix qualifies. Zero hits raise [`Error::ShaNotFound`];
    /// more than one hit raises [`Error::ShaAmbiguousPrefix`].
    pub fn read_for_sha(repo_root: &Path, provider: &dyn GitProvider, sha: &str) -> Result<Self> {
        let location = resolve_index_path(repo_root, provider)?;
        let db_path = location.path().join(INDEX_DB_FILENAME);
        if !db_path.exists() {
            return Err(Error::ShaNotFound {
                sha: sha.to_string(),
            });
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&db_path, flags)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let needle = sha.to_ascii_lowercase();
        let like = format!("{needle}%");
        let mut stmt = conn
            .prepare(
                "SELECT sha, files_changed FROM commits \
                 WHERE sha = ?1 OR sha LIKE ?2",
            )
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        let rows = stmt
            .query_map([&needle, &like], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let mut matches: Vec<(String, String)> = Vec::new();
        for r in rows {
            matches.push(r.map_err(|e| Error::Sqlite(e.to_string()))?);
        }

        // Dedup the case where `sha = ?1` and `sha LIKE ?2` both match the
        // same row (exact-match SHAs satisfy both predicates).
        matches.sort_by(|a, b| a.0.cmp(&b.0));
        matches.dedup_by(|a, b| a.0 == b.0);

        match matches.len() {
            0 => Err(Error::ShaNotFound {
                sha: sha.to_string(),
            }),
            1 => {
                let (full_sha, files_changed) = matches.into_iter().next().unwrap();
                let classifier = Classifier::default_for(repo_root)?;
                let records = parse_file_changes(&files_changed);
                let files = records
                    .into_iter()
                    .map(|r| ClassifiedFile {
                        category: classifier.classify(&r.path).as_str().to_string(),
                        path: r.path,
                    })
                    .collect();
                Ok(Self {
                    sha: full_sha,
                    files,
                })
            }
            n => Err(Error::ShaAmbiguousPrefix {
                prefix: sha.to_string(),
                count: n,
            }),
        }
    }
}

fn homogeneous_category(items: &[ClassifiedFile]) -> Option<String> {
    let first = items.first()?.category.clone();
    if items.iter().all(|i| i.category == first) {
        Some(first)
    } else {
        None
    }
}

/// Convenience helper: stable kebab-case identifier for a [`Category`].
/// Re-exported so callers in upper-tier crates do not need to import
/// `index::classify` for the simple `Category` → `&'static str` map.
pub fn category_label(category: Category) -> &'static str {
    category.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{MailmapResolved, RefEntry, RefScope, Sha, ShowOpts, WalkRange};
    use crate::index::schema::{serialize_file_changes, FileChangeRecord};
    use rusqlite::params;
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU32;

    struct StubProvider {
        common: PathBuf,
        _called: AtomicU32,
    }

    impl StubProvider {
        fn new(common: PathBuf) -> Self {
            Self {
                common,
                _called: AtomicU32::new(0),
            }
        }
    }

    impl GitProvider for StubProvider {
        fn common_dir(&self) -> Result<PathBuf> {
            Ok(self.common.clone())
        }
        fn rev_parse(&self, _: &str) -> Result<Sha> {
            unimplemented!()
        }
        fn list_refs(&self, _: RefScope) -> Result<Vec<RefEntry>> {
            unimplemented!()
        }
        fn walk_commits(&self, _: WalkRange) -> Result<Vec<crate::git::RawCommit>> {
            unimplemented!()
        }
        fn show(&self, _: &Sha, _: ShowOpts) -> Result<String> {
            unimplemented!()
        }
        fn check_mailmap(&self, _: &str, _: &str) -> Result<MailmapResolved> {
            unimplemented!()
        }
        fn cat_file_exists(&self, _: &Sha) -> Result<bool> {
            unimplemented!()
        }
    }

    fn insert_commit(conn: &Connection, sha: &str, files: &[FileChangeRecord]) {
        let json = serialize_file_changes(files);
        conn.execute(
            "INSERT INTO commits ( \
                 sha, author_name, author_email, author_identity_id, \
                 committer_name, committer_email, committer_identity_id, \
                 authored_at, committed_at, authored_tz_offset, committed_tz_offset, \
                 subject, body, expanded, parent_shas, parent_count, is_merge, is_root, \
                 files_changed, file_count, insertions, deletions, dirs_touched, dir_count, \
                 test_files_changed, config_files_changed, infra_files_changed, doc_files_changed, \
                 code_files_changed, dependency_files_changed, ci_files_changed, fixture_files_changed, \
                 migration_files_changed, is_revert, reverted_by_sha, risk_score, risk_label, \
                 admission_signals, story_id, indexed_at, updated_at \
             ) VALUES ( \
                 ?1, 'x', 'x@x', NULL, 'x', 'x@x', NULL, 0, 0, 0, 0, 's', 'b', 's', '[]', 0, 0, 0, \
                 ?2, ?3, 0, 0, '[]', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, NULL, NULL, NULL, '{}', NULL, 0, 0 \
             )",
            params![sha, json, files.len() as i64],
        )
        .unwrap();
    }

    fn seed_index_with(common: &Path) -> PathBuf {
        let dir = common.join("gitlore");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(INDEX_DB_FILENAME);
        let mut conn = Connection::open(&db_path).unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();
        insert_commit(
            &conn,
            "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111",
            &[
                FileChangeRecord {
                    path: "tests/foo.rs".to_string(),
                    status: 'M',
                    insertions: 1,
                    deletions: 0,
                },
                FileChangeRecord {
                    path: "docs/intro.md".to_string(),
                    status: 'A',
                    insertions: 1,
                    deletions: 0,
                },
            ],
        );
        insert_commit(
            &conn,
            "bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222",
            &[FileChangeRecord {
                path: "README.md".to_string(),
                status: 'M',
                insertions: 1,
                deletions: 0,
            }],
        );
        // Two commits sharing a 4-hex prefix to trigger the
        // ShaAmbiguousPrefix branch.
        insert_commit(&conn, "abcd1234abcd1234abcd1234abcd1234abcd1234", &[]);
        insert_commit(&conn, "abcd5678abcd5678abcd5678abcd5678abcd5678", &[]);
        db_path
    }

    #[test]
    fn explain_resolves_exact_sha_and_classifies_files() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        seed_index_with(&common);
        let provider = StubProvider::new(common);

        let r = ClassifyExplainReport::read_for_sha(
            tmp.path(),
            &provider,
            "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111",
        )
        .unwrap();
        assert_eq!(r.sha, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
        assert_eq!(r.files.len(), 2);
        // Test category beats docs in the Q14 chain — but each path here
        // matches its own category, so we just check both labels are
        // present.
        let cats: Vec<&str> = r.files.iter().map(|f| f.category.as_str()).collect();
        assert!(cats.contains(&"test"), "tests/foo.rs → test, got {cats:?}");
        assert!(cats.contains(&"docs"), "docs/intro.md → docs, got {cats:?}");
    }

    #[test]
    fn explain_resolves_unique_prefix() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        seed_index_with(&common);
        let provider = StubProvider::new(common);

        let r = ClassifyExplainReport::read_for_sha(tmp.path(), &provider, "aaaa1111aaaa").unwrap();
        assert_eq!(r.sha, "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111");
    }

    #[test]
    fn explain_returns_sha_not_found_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        seed_index_with(&common);
        let provider = StubProvider::new(common);

        let err =
            ClassifyExplainReport::read_for_sha(tmp.path(), &provider, "ffff9999").unwrap_err();
        assert_eq!(err.code(), "sha_not_found");
    }

    #[test]
    fn explain_returns_ambiguous_for_short_prefix_with_multiple_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        seed_index_with(&common);
        let provider = StubProvider::new(common);

        let err = ClassifyExplainReport::read_for_sha(tmp.path(), &provider, "abcd").unwrap_err();
        assert_eq!(err.code(), "sha_ambiguous_prefix");
    }

    #[test]
    fn explain_returns_sha_not_found_when_index_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let provider = StubProvider::new(common);

        let err = ClassifyExplainReport::read_for_sha(
            tmp.path(),
            &provider,
            "aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111",
        )
        .unwrap_err();
        assert_eq!(err.code(), "sha_not_found");
    }

    #[test]
    fn glob_report_homogeneous_category_collapses_to_top_level() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ClassifyGlobReport::for_paths(
            tmp.path(),
            "docs/**/*.md",
            &["docs/intro.md".to_string(), "docs/usage.md".to_string()],
        )
        .unwrap();
        assert_eq!(r.category.as_deref(), Some("docs"));
        assert_eq!(r.matched_files.len(), 2);
    }

    #[test]
    fn glob_report_heterogeneous_category_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ClassifyGlobReport::for_paths(
            tmp.path(),
            "**/*",
            &["tests/foo.rs".to_string(), "docs/intro.md".to_string()],
        )
        .unwrap();
        assert!(r.category.is_none(), "mixed bag → no top-level category");
    }

    #[test]
    fn glob_report_no_matches_has_none_category() {
        let tmp = tempfile::tempdir().unwrap();
        let r = ClassifyGlobReport::for_paths(tmp.path(), "src/missing/*.rs", &Vec::new()).unwrap();
        assert!(r.matched_files.is_empty());
        assert!(r.category.is_none());
    }
}
