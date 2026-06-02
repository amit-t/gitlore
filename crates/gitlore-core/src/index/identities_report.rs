//! Read-only identities reader (M3-7b, SPEC-001 §4.3.1).
//!
//! Powers `gitlore identities`. Opens the SQLite index with
//! `SQLITE_OPEN_READ_ONLY` so concurrent indexer writes do not contend
//! with the read path, then aggregates resolved identities with their
//! alias counts and authored-commit counts.
//!
//! ## Wire shape
//!
//! ```text
//! {
//!   clustered_count: <count of identities rows>,
//!   raw_count:       <count of identity_aliases rows>,
//!   identities: [
//!     { canonical_name, canonical_email, aliases, is_bot, commit_count },
//!     ...
//!   ]
//! }
//! ```
//!
//! `clustered_count` is the post-resolution identity count. `raw_count` is
//! the number of distinct `(raw_name, raw_email)` pairs observed across
//! commits — every raw author/committer pair the indexer saw becomes one
//! alias row, so `raw_count` is exactly the alias-table row count.
//!
//! ## Bot filtering
//!
//! `include_bots = false` (the default) excludes `is_bot = 1` rows from
//! the identity list but does **not** alter `clustered_count` /
//! `raw_count`. Those totals describe the full cluster space; the list
//! is what gets rendered.

use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::git::GitProvider;
use crate::index::indexer::INDEX_DB_FILENAME;
use crate::index::storage::resolve_index_path;

/// One identity row in the SPEC-001 §4.3.1 payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentityRow {
    /// Canonical display name (post-mailmap / override).
    pub canonical_name: String,
    /// Canonical email (post-mailmap / override).
    pub canonical_email: String,
    /// Number of `(raw_name, raw_email)` alias rows pointing at this identity.
    pub aliases: u64,
    /// `true` iff the identity is flagged as a bot (GitHub App, CI worker,
    /// override-marked, …).
    pub is_bot: bool,
    /// Number of commits whose `author_identity_id` resolves to this identity.
    pub commit_count: u64,
}

/// Aggregate payload returned by [`IdentitiesReport::read`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitiesReport {
    /// Total number of resolved (clustered) identities — `COUNT(*) FROM identities`.
    pub clustered_count: u64,
    /// Total number of raw `(name, email)` pairs observed —
    /// `COUNT(*) FROM identity_aliases`.
    pub raw_count: u64,
    /// Identity rows. Sorted by `commit_count DESC, canonical_email ASC`.
    /// Bots are filtered out when `include_bots = false`.
    pub identities: Vec<IdentityRow>,
}

impl IdentitiesReport {
    /// Read identities for the repository rooted at `repo_root`.
    ///
    /// Opens the SQLite database with `SQLITE_OPEN_READ_ONLY` so a
    /// concurrent indexer's writer lock is not contended. Returns an empty
    /// report when no index exists yet (the same shape used by
    /// [`crate::index::status::StatusReport::read`] for the
    /// pre-`gitlore index` case).
    pub fn read(repo_root: &Path, provider: &dyn GitProvider, include_bots: bool) -> Result<Self> {
        let location = resolve_index_path(repo_root, provider)?;
        let db_path = location.path().join(INDEX_DB_FILENAME);
        if !db_path.exists() {
            return Ok(Self {
                clustered_count: 0,
                raw_count: 0,
                identities: Vec::new(),
            });
        }

        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(&db_path, flags)
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        let clustered_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM identities", [], |row| row.get(0))
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        let raw_count: u64 = conn
            .query_row("SELECT COUNT(*) FROM identity_aliases", [], |row| {
                row.get(0)
            })
            .map_err(|e| Error::Sqlite(e.to_string()))?;

        // One pass that joins both the alias-count and the commit-count
        // subqueries; both LEFT-JOINed so identities with zero aliases or
        // zero commits still surface (post-rewrite races, override-only
        // entries, etc.).
        let mut sql = String::from(
            "SELECT \
                 i.canonical_name, \
                 i.canonical_email, \
                 i.is_bot, \
                 COALESCE(ac.alias_count, 0) AS alias_count, \
                 COALESCE(cc.commit_count, 0) AS commit_count \
             FROM identities i \
             LEFT JOIN ( \
                 SELECT identity_id, COUNT(*) AS alias_count \
                 FROM identity_aliases \
                 GROUP BY identity_id \
             ) ac ON ac.identity_id = i.id \
             LEFT JOIN ( \
                 SELECT author_identity_id, COUNT(*) AS commit_count \
                 FROM commits \
                 WHERE author_identity_id IS NOT NULL \
                 GROUP BY author_identity_id \
             ) cc ON cc.author_identity_id = i.id",
        );
        if !include_bots {
            sql.push_str(" WHERE i.is_bot = 0");
        }
        sql.push_str(" ORDER BY commit_count DESC, i.canonical_email ASC");

        let mut stmt = conn
            .prepare(&sql)
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        let rows = stmt
            .query_map([], |row| {
                Ok(IdentityRow {
                    canonical_name: row.get::<_, String>(0)?,
                    canonical_email: row.get::<_, String>(1)?,
                    is_bot: row.get::<_, i64>(2)? != 0,
                    aliases: row.get::<_, i64>(3)? as u64,
                    commit_count: row.get::<_, i64>(4)? as u64,
                })
            })
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        let mut identities = Vec::new();
        for r in rows {
            identities.push(r.map_err(|e| Error::Sqlite(e.to_string()))?);
        }

        Ok(Self {
            clustered_count,
            raw_count,
            identities,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{MailmapResolved, RefEntry, RefScope, Sha, ShowOpts, WalkRange};
    use rusqlite::params;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// `GitProvider` stub that only implements `common_dir` — every other
    /// method panics so an accidental call is loud rather than silent.
    struct StubProvider {
        common: PathBuf,
        called: AtomicU32,
    }

    impl StubProvider {
        fn new(common: PathBuf) -> Self {
            Self {
                common,
                called: AtomicU32::new(0),
            }
        }
    }

    impl GitProvider for StubProvider {
        fn common_dir(&self) -> Result<PathBuf> {
            self.called.fetch_add(1, Ordering::SeqCst);
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

    fn seed_index(common: &Path) -> std::path::PathBuf {
        let dir = common.join("gitlore");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(INDEX_DB_FILENAME);
        let mut conn = Connection::open(&db_path).unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();

        // Identity 1: human "Alice" with two aliases, three commits.
        conn.execute(
            "INSERT INTO identities (canonical_name, canonical_email, first_seen_at, last_seen_at, commit_count, is_bot) \
             VALUES ('Alice', 'alice@example.com', 100, 100, 0, 0)",
            [],
        )
        .unwrap();
        let alice_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO identity_aliases (identity_id, raw_name, raw_email) VALUES (?1, 'a', 'a@x')",
            params![alice_id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO identity_aliases (identity_id, raw_name, raw_email) VALUES (?1, 'Alice S', 'alice@new.com')",
            params![alice_id],
        )
        .unwrap();
        // Identity 2: bot "dependabot[bot]" with one alias, one commit.
        conn.execute(
            "INSERT INTO identities (canonical_name, canonical_email, first_seen_at, last_seen_at, commit_count, is_bot) \
             VALUES ('dependabot[bot]', 'bot@noreply.github.com', 200, 200, 0, 1)",
            [],
        )
        .unwrap();
        let bot_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO identity_aliases (identity_id, raw_name, raw_email) VALUES (?1, 'dep', 'dep@x')",
            params![bot_id],
        )
        .unwrap();

        // Three commits for Alice, one for the bot.
        for (sha, identity) in [
            ("aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111", alice_id),
            ("bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222", alice_id),
            ("cccc3333cccc3333cccc3333cccc3333cccc3333", alice_id),
            ("dddd4444dddd4444dddd4444dddd4444dddd4444", bot_id),
        ] {
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
                     ?1, 'x', 'x@x', ?2, 'x', 'x@x', ?2, 0, 0, 0, 0, 's', 'b', 's', '[]', 0, 0, 0, \
                     '[]', 0, 0, 0, '[]', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, NULL, NULL, NULL, '{}', NULL, 0, 0 \
                 )",
                params![sha, identity],
            )
            .unwrap();
        }

        db_path
    }

    #[test]
    fn read_returns_empty_when_index_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let provider = StubProvider::new(common);
        let r = IdentitiesReport::read(tmp.path(), &provider, true).unwrap();
        assert_eq!(r.clustered_count, 0);
        assert_eq!(r.raw_count, 0);
        assert!(r.identities.is_empty());
    }

    #[test]
    fn read_excludes_bots_by_default() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let _db = seed_index(&common);
        let provider = StubProvider::new(common);

        let r = IdentitiesReport::read(tmp.path(), &provider, false).unwrap();
        // Totals reflect the full cluster space — not affected by the
        // is_bot filter.
        assert_eq!(r.clustered_count, 2);
        assert_eq!(r.raw_count, 3);
        // But the bot row is hidden.
        assert_eq!(r.identities.len(), 1);
        let alice = &r.identities[0];
        assert_eq!(alice.canonical_name, "Alice");
        assert_eq!(alice.aliases, 2);
        assert_eq!(alice.commit_count, 3);
        assert!(!alice.is_bot);
    }

    #[test]
    fn read_includes_bots_when_requested() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let _db = seed_index(&common);
        let provider = StubProvider::new(common);

        let r = IdentitiesReport::read(tmp.path(), &provider, true).unwrap();
        assert_eq!(r.identities.len(), 2);
        // Sorted by commit_count DESC — Alice (3) before the bot (1).
        assert_eq!(r.identities[0].canonical_email, "alice@example.com");
        assert_eq!(r.identities[1].canonical_email, "bot@noreply.github.com");
        assert!(r.identities[1].is_bot);
        assert_eq!(r.identities[1].commit_count, 1);
        assert_eq!(r.identities[1].aliases, 1);
    }

    #[test]
    fn identity_with_no_commits_still_surfaces() {
        let tmp = tempfile::tempdir().unwrap();
        let common = tmp.path().join("git-common");
        std::fs::create_dir_all(&common).unwrap();
        let dir = common.join("gitlore");
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join(INDEX_DB_FILENAME);
        let mut conn = Connection::open(&db_path).unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();
        conn.execute(
            "INSERT INTO identities (canonical_name, canonical_email, first_seen_at, last_seen_at, commit_count, is_bot) \
             VALUES ('Orphan', 'orphan@x.com', 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        let provider = StubProvider::new(common);

        let r = IdentitiesReport::read(tmp.path(), &provider, true).unwrap();
        assert_eq!(r.identities.len(), 1);
        assert_eq!(r.identities[0].commit_count, 0);
        assert_eq!(r.identities[0].aliases, 0);
    }
}
