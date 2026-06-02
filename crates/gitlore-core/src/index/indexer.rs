//! Indexer engine (M3-6, TDD-000 §2.2, SPEC-001 §5, AC-IDX-1..6).
//!
//! Bridges the [`crate::git::GitProvider`] read-only walk surface with the
//! SPEC-001 §5.1 SQLite schema. One [`Indexer`] owns the writer lock, the
//! database connection, and the file classifier; its three driving entry
//! points are [`Indexer::run_initial`], [`Indexer::run_incremental`], and
//! [`Indexer::rebuild`].
//!
//! ## Walk model
//!
//! For each ref returned by [`crate::git::refs::enumerate_refs`] we call
//! [`crate::git::GitProvider::walk_commits`] for either the full reachable
//! set (initial) or the delta `watermark[ref]..tip` (incremental). The
//! resulting [`RawCommit`]s are deduplicated by SHA, identity-resolved in
//! a single up-front pass (writes happen in autocommit so the resolver
//! does not contend with the per-chunk transaction), then persisted in
//! batches of [`CHUNK_SIZE`] commits per `BEGIN`/`COMMIT` (ADR-004).
//!
//! ## Idempotency
//!
//! Every write uses `INSERT ... ON CONFLICT DO UPDATE`/`INSERT OR REPLACE`
//! so a sigkill-mid-chunk restart is safe (AC-IDX-4): the resumed run
//! re-walks from the per-ref watermark and silently overwrites already-
//! persisted rows.
//!
//! ## Revert detection (Q18 / ADR-020)
//!
//! After the walk we scan freshly-persisted commits for the
//! `^Revert\b` subject pattern or the `This reverts commit <sha>` body
//! trailer. Hits set `commits.is_revert = 1` and back-link
//! `commits.reverted_by_sha` on the target commit when the body trailer
//! resolves a known SHA within the ±30-day window
//! ([`REVERT_WINDOW_SECS`]).
//!
//! ## Force-push retention (Q8 / AC-IDX-5)
//!
//! After every walk we probe every indexed SHA via
//! [`crate::git::GitProvider::cat_file_exists`] (delegated to
//! [`crate::git::refs::force_push_retention`]). Orphans (objects still in
//! the index but no longer in the repo) get their `commit_refs` rows
//! dropped while the `commits` row stays — the `--include-unreachable`
//! query at M3-7 will surface them.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use regex::Regex;
use rusqlite::{params, Connection};

use crate::config::Config;
use crate::error::{Error, Result};
use crate::git::cli::GitCliProvider;
use crate::git::refs::{enumerate_refs, force_push_retention};
use crate::git::{
    top_level_dirs, FileChange, GitProvider, RawCommit, RefEntry, RefType, Sha, WalkRange,
};
use crate::index::classify::{Category, Classifier};
use crate::index::identity::{
    ChainedResolver, IdentityResolver, MailmapResolver, OverrideResolver, UnionFindResolver,
};
use crate::index::lock::{
    acquire, wal_checkpoint_if_large, LockMode, WriterLock, DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES,
};
use crate::index::schema::{serialize_file_changes, serialize_string_list, FileChangeRecord};
use crate::index::storage::resolve_index_path;

/// Commits per `BEGIN`/`COMMIT` transaction (ADR-004 §4.5).
pub const CHUNK_SIZE: usize = 500;

/// ±30-day window used for bidirectional revert linking (Q18 / ADR-020).
pub const REVERT_WINDOW_SECS: i64 = 30 * 24 * 60 * 60;

/// SQLite database filename inside the resolved index directory.
pub const INDEX_DB_FILENAME: &str = "index.sqlite";

/// Lockfile name inside the resolved index directory.
pub const INDEX_LOCK_FILENAME: &str = "index.lock";

/// `index_state` key under which the per-ref watermark JSON is stored.
pub const WATERMARK_KEY: &str = "refs_watermark";

/// Output of [`Indexer::dry_run`] — ref enumeration plus a deduplicated
/// commit estimate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPlan {
    /// Refs in the order [`enumerate_refs`] returned them.
    pub refs: Vec<RefEntry>,
    /// Total commits reachable from the union of ref tips (de-duplicated).
    pub estimated_commits: u64,
}

/// Summary of a single indexer run.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexReport {
    /// Number of commit rows written (UPSERTed) during this run.
    pub commits_indexed: u64,
    /// Total commits the walker visited for this run (equals
    /// `commits_indexed` on a fresh database).
    pub commits_total: u64,
    /// Number of refs enumerated for this run.
    pub ref_count: usize,
    /// Wall-clock duration in milliseconds.
    pub duration_ms: u64,
    /// Per-ref watermark as persisted at the end of this run.
    pub watermark: BTreeMap<String, Sha>,
}

/// Index walker wired to a [`GitProvider`] + a SQLite index.
///
/// Build via [`Indexer::open`]; drive with one of [`Indexer::run_initial`],
/// [`Indexer::run_incremental`], [`Indexer::rebuild`], or
/// [`Indexer::dry_run`]. Drop the value to release the writer lock.
pub struct Indexer {
    provider: Box<dyn GitProvider>,
    conn: Connection,
    /// User-facing config. Read at open and stashed for downstream
    /// callers; the indexer itself does not enforce
    /// `max_initial_commits` (the M3-7 CLI does).
    config: Config,
    classifier: Classifier,
    lock: Option<WriterLock>,
    repo_root: PathBuf,
    db_path: PathBuf,
    lock_path: PathBuf,
}

impl Indexer {
    /// Open the indexer rooted at `repo_root`, acquiring the writer lock
    /// per `lock_mode`. Uses [`GitCliProvider`] as the Git backend.
    pub fn open(repo_root: &Path, lock_mode: LockMode) -> Result<Self> {
        let provider: Box<dyn GitProvider> = Box::new(GitCliProvider::new(repo_root.to_path_buf()));
        Self::open_with_provider(repo_root, provider, lock_mode)
    }

    /// Variant of [`Indexer::open`] that accepts a caller-supplied
    /// provider — useful for tests that wire in a stub or for the
    /// future `git2-rs` backend swap.
    pub fn open_with_provider(
        repo_root: &Path,
        provider: Box<dyn GitProvider>,
        lock_mode: LockMode,
    ) -> Result<Self> {
        let common_dir = provider.common_dir()?;
        let config = Config::load(&common_dir).map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("config load failed: {e}"),
            ))
        })?;
        let location = resolve_index_path(repo_root, provider.as_ref())?;
        let dir = location.path().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        let db_path = dir.join(INDEX_DB_FILENAME);
        let lock_path = dir.join(INDEX_LOCK_FILENAME);
        let lock = acquire(&lock_path, lock_mode)?;
        let mut conn = Connection::open(&db_path).map_err(|e| Error::Sqlite(e.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        crate::index::migrations::migrate(&mut conn)?;
        wal_checkpoint_if_large(&conn, &db_path, DEFAULT_WAL_CHECKPOINT_THRESHOLD_BYTES)?;
        let classifier = Classifier::default_for(repo_root)?;
        Ok(Self {
            provider,
            conn,
            config,
            classifier,
            lock: Some(lock),
            repo_root: repo_root.to_path_buf(),
            db_path,
            lock_path,
        })
    }

    /// Walk every reachable commit from every ref and persist the result.
    /// Always runs the force-push retention sweep after the walk.
    pub fn run_initial(&mut self, progress: &mut dyn FnMut(u64, u64)) -> Result<IndexReport> {
        let start = Instant::now();
        let mut watermark: BTreeMap<String, Sha> = BTreeMap::new();
        let refs = enumerate_refs(self.provider.as_ref())?;
        let (commits_indexed, commits_total, touched_shas) =
            self.walk_and_persist(&refs, &mut watermark, progress, false)?;
        self.populate_tags(&refs)?;
        if !touched_shas.is_empty() {
            self.detect_reverts(&touched_shas)?;
        }
        self.prune_orphans()?;
        self.persist_watermark(&watermark)?;
        Ok(IndexReport {
            commits_indexed,
            commits_total,
            ref_count: refs.len(),
            duration_ms: start.elapsed().as_millis() as u64,
            watermark,
        })
    }

    /// Walk only the per-ref deltas above the persisted watermark. Force-
    /// push retention runs unconditionally so reset-then-prune flows are
    /// covered by AC-IDX-5.
    pub fn run_incremental(&mut self, progress: &mut dyn FnMut(u64, u64)) -> Result<IndexReport> {
        let start = Instant::now();
        let mut watermark = self.load_watermark()?;
        let refs = enumerate_refs(self.provider.as_ref())?;
        let (commits_indexed, commits_total, touched_shas) =
            self.walk_and_persist(&refs, &mut watermark, progress, true)?;
        if commits_indexed > 0 {
            self.populate_tags(&refs)?;
            self.detect_reverts(&touched_shas)?;
        }
        self.prune_orphans()?;
        self.persist_watermark(&watermark)?;
        Ok(IndexReport {
            commits_indexed,
            commits_total,
            ref_count: refs.len(),
            duration_ms: start.elapsed().as_millis() as u64,
            watermark,
        })
    }

    /// Drop the SQLite database file and re-run [`Indexer::run_initial`]
    /// from scratch. The writer lock is reacquired in `NoWait` mode after
    /// the reset.
    pub fn rebuild(&mut self, progress: &mut dyn FnMut(u64, u64)) -> Result<IndexReport> {
        // Swap the live connection out for an in-memory placeholder so
        // SQLite releases its file handles, then drop the writer lock so
        // the lockfile can be re-created.
        let placeholder = Connection::open_in_memory().map_err(|e| Error::Sqlite(e.to_string()))?;
        let old_conn = std::mem::replace(&mut self.conn, placeholder);
        drop(old_conn);
        self.lock.take();

        for suffix in ["", "-wal", "-shm"] {
            let mut name = self
                .db_path
                .file_name()
                .map(|s| s.to_os_string())
                .unwrap_or_default();
            name.push(suffix);
            let path = self.db_path.with_file_name(name);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
        }

        let lock = acquire(&self.lock_path, LockMode::NoWait)?;
        let mut conn = Connection::open(&self.db_path).map_err(|e| Error::Sqlite(e.to_string()))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        crate::index::migrations::migrate(&mut conn)?;
        self.lock = Some(lock);
        self.conn = conn;

        self.run_initial(progress)
    }

    /// Enumerate refs + estimate the number of unique commits without
    /// touching the database. Bounded by
    /// [`crate::config::IndexConfig::max_initial_commits`] so a runaway
    /// monorepo dry-run still terminates.
    pub fn dry_run(&self) -> Result<RefPlan> {
        let refs = enumerate_refs(self.provider.as_ref())?;
        let cap = self.config.index.max_initial_commits as usize;
        let mut seen: HashSet<String> = HashSet::new();
        for r in &refs {
            if seen.len() >= cap {
                break;
            }
            let remaining = cap.saturating_sub(seen.len());
            let commits = self.provider.walk_commits(WalkRange {
                from: None,
                to: r.sha.clone(),
                max: Some(remaining),
            })?;
            for c in commits {
                seen.insert(c.sha.as_str().to_string());
                if seen.len() >= cap {
                    break;
                }
            }
        }
        Ok(RefPlan {
            refs,
            estimated_commits: seen.len() as u64,
        })
    }

    /// DELETE FROM commit_refs for every indexed SHA the repository no
    /// longer resolves (force-push detection per Q8 / AC-IDX-5). The
    /// orphaned `commits` row stays so `--include-unreachable` can find
    /// it at the M3-7 CLI layer.
    pub fn prune_orphans(&self) -> Result<u64> {
        let known: Vec<Sha> = {
            let mut stmt = self
                .conn
                .prepare("SELECT sha FROM commits")
                .map_err(|e| Error::Sqlite(e.to_string()))?;
            let mapped = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|e| Error::Sqlite(e.to_string()))?;
            let mut out = Vec::new();
            for r in mapped {
                let s = r.map_err(|e| Error::Sqlite(e.to_string()))?;
                if let Ok(sha) = Sha::new(s) {
                    out.push(sha);
                }
            }
            out
        };
        let orphans = force_push_retention(self.provider.as_ref(), &known)?;
        if orphans.is_empty() {
            return Ok(0);
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        for o in &orphans {
            tx.execute(
                "DELETE FROM commit_refs WHERE sha = ?1",
                params![o.as_str()],
            )
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        }
        tx.commit().map_err(|e| Error::Sqlite(e.to_string()))?;
        Ok(orphans.len() as u64)
    }

    /// Borrow the resolved user-facing config.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Borrow the file classifier built at open time.
    pub fn classifier(&self) -> &Classifier {
        &self.classifier
    }

    /// Repository root the indexer was opened against.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// On-disk path of the SQLite database file.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Borrow the underlying SQLite connection (read-only helper for
    /// integration tests and the eval harness).
    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    /// `true` when the per-ref watermark row exists in `index_state` —
    /// i.e. a previous indexer run has persisted state and an
    /// incremental walk can resume from it. `false` on a fresh
    /// database, which is the M3-7 CLI's cue to dispatch
    /// [`Indexer::run_initial`] instead.
    pub fn has_watermark(&self) -> Result<bool> {
        let map = self.load_watermark()?;
        Ok(!map.is_empty())
    }

    // ---------------------------------------------------------------------
    // Internal: walk + persist pipeline
    // ---------------------------------------------------------------------

    fn walk_and_persist(
        &mut self,
        refs: &[RefEntry],
        watermark: &mut BTreeMap<String, Sha>,
        progress: &mut dyn FnMut(u64, u64),
        incremental: bool,
    ) -> Result<(u64, u64, Vec<String>)> {
        // Phase 1: walk every ref, accumulate unique RawCommit values +
        // sha → refs reverse map.
        let mut all_commits: Vec<RawCommit> = Vec::new();
        let mut sha_to_refs: HashMap<String, Vec<(String, RefType)>> = HashMap::new();
        let mut seen: HashSet<String> = HashSet::new();
        for r in refs {
            let mut from = if incremental {
                watermark.get(&r.name).cloned()
            } else {
                None
            };
            // Stale watermark guard: if the persisted `from` sha has
            // been GC'd out of the repo (force-push aftermath), the
            // backend's revision-range query would error out. Fall back
            // to a full walk for this ref so the new history still
            // gets indexed; the prune_orphans pass at the end of the
            // run handles the now-unreachable rows.
            if let Some(ref f) = from {
                if !self.provider.cat_file_exists(f)? {
                    from = None;
                }
            }
            let tip_unchanged = matches!(&from, Some(s) if s.as_str() == r.sha.as_str());
            if !tip_unchanged {
                let commits = self.provider.walk_commits(WalkRange {
                    from: from.clone(),
                    to: r.sha.clone(),
                    max: None,
                })?;
                for c in &commits {
                    sha_to_refs
                        .entry(c.sha.as_str().to_string())
                        .or_default()
                        .push((r.name.clone(), r.ref_type));
                }
                for c in commits {
                    if seen.insert(c.sha.as_str().to_string()) {
                        all_commits.push(c);
                    }
                }
            }
            // Watermark always advances to the live tip — even on
            // tip_unchanged so a no-op pass still rewrites the same
            // watermark and the JSON stays canonical.
            watermark.insert(r.name.clone(), r.sha.clone());
        }

        let commits_total = all_commits.len() as u64;
        progress(0, commits_total);
        if commits_total == 0 {
            return Ok((0, 0, Vec::new()));
        }

        // Phase 2: identity resolution (autocommit). Build the chained
        // resolver inside a scope so its borrows on self.conn release
        // before Phase 3 opens a transaction.
        let common_dir = self.provider.common_dir()?;
        let resolved: HashMap<String, ResolvedIds> = {
            let or = OverrideResolver::from_common_dir(&self.conn, &common_dir)
                .unwrap_or_else(|_| OverrideResolver::new(&self.conn, HashMap::new()));
            let mr = MailmapResolver::new(self.provider.as_ref(), &self.conn);
            let uf = UnionFindResolver::new(&self.conn)?;
            let chain = ChainedResolver::new(or, mr, uf);
            let mut map: HashMap<String, ResolvedIds> = HashMap::new();
            for c in &all_commits {
                let author_id = chain.resolve(&c.author_name, &c.author_email).ok();
                let committer_id = chain.resolve(&c.committer_name, &c.committer_email).ok();
                let coauthor_ids: Vec<i64> = c
                    .coauthors
                    .iter()
                    .filter_map(|(n, e)| chain.resolve(n, e).ok())
                    .collect();
                map.insert(
                    c.sha.as_str().to_string(),
                    ResolvedIds {
                        author_id,
                        committer_id,
                        coauthor_ids,
                    },
                );
            }
            map
        };

        // Phase 3: per-chunk transactional inserts.
        let now = Utc::now().timestamp();
        let mut commits_indexed = 0u64;
        let mut touched: Vec<String> = Vec::with_capacity(all_commits.len());

        for chunk in all_commits.chunks(CHUNK_SIZE) {
            {
                let tx = self
                    .conn
                    .transaction()
                    .map_err(|e| Error::Sqlite(e.to_string()))?;
                for c in chunk {
                    let sha_str = c.sha.as_str().to_string();
                    let rids = resolved.get(&sha_str);
                    let counters = compute_counters(&self.classifier, &c.files_changed);
                    let file_records: Vec<FileChangeRecord> = c
                        .files_changed
                        .iter()
                        .map(|f| FileChangeRecord {
                            path: f.path.clone(),
                            status: f.status,
                            insertions: f.insertions,
                            deletions: f.deletions,
                        })
                        .collect();
                    let parent_strs: Vec<String> = c
                        .parent_shas
                        .iter()
                        .map(|s| s.as_str().to_string())
                        .collect();
                    let dirs = top_level_dirs(&c.files_changed);
                    let total_ins: u64 = c.files_changed.iter().map(|f| f.insertions).sum();
                    let total_del: u64 = c.files_changed.iter().map(|f| f.deletions).sum();
                    let is_merge: i64 = if c.parent_shas.len() >= 2 { 1 } else { 0 };
                    let is_root: i64 = if c.parent_shas.is_empty() { 1 } else { 0 };
                    let expanded = build_expanded(&c.subject, &c.body, &dirs);

                    tx.execute(
                        INSERT_COMMIT_SQL,
                        params![
                            sha_str,
                            c.author_name,
                            c.author_email,
                            rids.and_then(|r| r.author_id),
                            c.committer_name,
                            c.committer_email,
                            rids.and_then(|r| r.committer_id),
                            c.authored_at,
                            c.committed_at,
                            c.subject,
                            c.body,
                            expanded,
                            serialize_string_list(&parent_strs),
                            c.parent_shas.len() as i64,
                            is_merge,
                            is_root,
                            serialize_file_changes(&file_records),
                            c.files_changed.len() as i64,
                            total_ins as i64,
                            total_del as i64,
                            serialize_string_list(&dirs),
                            dirs.len() as i64,
                            counters[0],
                            counters[1],
                            counters[2],
                            counters[3],
                            counters[4],
                            counters[5],
                            counters[6],
                            counters[7],
                            counters[8],
                            now,
                            now,
                        ],
                    )
                    .map_err(|e| Error::Sqlite(e.to_string()))?;
                    touched.push(sha_str.clone());

                    if let Some(refs_for_sha) = sha_to_refs.get(&sha_str) {
                        for (name, kind) in refs_for_sha {
                            let kind_str = match kind {
                                RefType::Branch => "branch",
                                RefType::RemoteBranch => "remote_branch",
                                RefType::Tag => "tag",
                            };
                            tx.execute(
                                "INSERT OR REPLACE INTO commit_refs \
                                 (sha, ref_name, ref_kind) VALUES (?1, ?2, ?3)",
                                params![sha_str, name, kind_str],
                            )
                            .map_err(|e| Error::Sqlite(e.to_string()))?;
                        }
                    }

                    if let Some(r) = rids {
                        for cid in &r.coauthor_ids {
                            tx.execute(
                                "INSERT OR IGNORE INTO commit_coauthors \
                                 (sha, identity_id) VALUES (?1, ?2)",
                                params![sha_str, cid],
                            )
                            .map_err(|e| Error::Sqlite(e.to_string()))?;
                        }
                    }

                    commits_indexed += 1;
                    progress(commits_indexed, commits_total);
                }
                tx.commit().map_err(|e| Error::Sqlite(e.to_string()))?;
            }
            // Persist the watermark after every chunk so a sigkill
            // between chunks still leaves a usable resume point.
            self.persist_watermark(watermark)?;
        }

        Ok((commits_indexed, commits_total, touched))
    }

    fn populate_tags(&self, refs: &[RefEntry]) -> Result<()> {
        for r in refs {
            if !r.name.starts_with("refs/tags/") {
                continue;
            }
            let sha = r.sha.as_str();
            // Skip when the target commit row never made it in (e.g.
            // walked-and-dropped by an unreachable tag).
            let exists = self
                .conn
                .query_row("SELECT 1 FROM commits WHERE sha = ?1", params![sha], |_| {
                    Ok(())
                })
                .is_ok();
            if !exists {
                continue;
            }
            // Annotated detection + tagger fields land at Phase 3 once
            // GitProvider gains a `cat_file` surface. For now pull
            // tagged_at from the target commit's committed_at so
            // downstream sort queries still order tags chronologically.
            let tagged_at: i64 = self
                .conn
                .query_row(
                    "SELECT committed_at FROM commits WHERE sha = ?1",
                    params![sha],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            self.conn
                .execute(
                    "INSERT OR REPLACE INTO tags \
                     (ref_name, sha, annotated, message, tagged_at) \
                     VALUES (?1, ?2, 0, '', ?3)",
                    params![r.name, sha, tagged_at],
                )
                .map_err(|e| Error::Sqlite(e.to_string()))?;
        }
        Ok(())
    }

    fn detect_reverts(&self, candidate_shas: &[String]) -> Result<()> {
        if candidate_shas.is_empty() {
            return Ok(());
        }
        let re_subject = Regex::new(r"^Revert\b").expect("static regex");
        let re_body =
            Regex::new(r"(?i)This reverts commit ([0-9a-f]{7,64})").expect("static regex");

        // Load (sha, subject, body, committed_at) for every candidate.
        let placeholders = (1..=candidate_shas.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT sha, subject, body, committed_at FROM commits \
             WHERE sha IN ({placeholders})"
        );
        let mut stmt = self
            .conn
            .prepare(&sql)
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(candidate_shas.iter()), |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            })
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        let mut candidates: Vec<(String, String, String, i64)> = Vec::new();
        for r in rows {
            candidates.push(r.map_err(|e| Error::Sqlite(e.to_string()))?);
        }
        drop(stmt);

        for (sha, subject, body, committed_at) in &candidates {
            let is_revert = re_subject.is_match(subject) || re_body.is_match(body);
            if !is_revert {
                continue;
            }
            self.conn
                .execute(
                    "UPDATE commits SET is_revert = 1 WHERE sha = ?1",
                    params![sha],
                )
                .map_err(|e| Error::Sqlite(e.to_string()))?;
            // Back-link reverted_by_sha on the target commit, within the
            // ±30-day window per ADR-020.
            if let Some(cap) = re_body.captures(body) {
                if let Some(m) = cap.get(1) {
                    let prefix = m.as_str().to_ascii_lowercase();
                    let lower = committed_at - REVERT_WINDOW_SECS;
                    let upper = committed_at + REVERT_WINDOW_SECS;
                    let pattern = format!("{prefix}%");
                    self.conn
                        .execute(
                            "UPDATE commits SET reverted_by_sha = ?1 \
                             WHERE sha LIKE ?2 \
                               AND sha != ?1 \
                               AND committed_at BETWEEN ?3 AND ?4",
                            params![sha, pattern, lower, upper],
                        )
                        .map_err(|e| Error::Sqlite(e.to_string()))?;
                }
            }
        }
        Ok(())
    }

    fn load_watermark(&self) -> Result<BTreeMap<String, Sha>> {
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT value FROM index_state WHERE key = ?1",
                params![WATERMARK_KEY],
                |row| row.get::<_, String>(0),
            )
            .ok();
        let Some(raw) = raw else {
            return Ok(BTreeMap::new());
        };
        let parsed: BTreeMap<String, String> = serde_json::from_str(&raw).unwrap_or_default();
        let mut out = BTreeMap::new();
        for (k, v) in parsed {
            if let Ok(s) = Sha::new(v) {
                out.insert(k, s);
            }
        }
        Ok(out)
    }

    fn persist_watermark(&self, w: &BTreeMap<String, Sha>) -> Result<()> {
        let serialised: BTreeMap<String, String> = w
            .iter()
            .map(|(k, v)| (k.clone(), v.as_str().to_string()))
            .collect();
        let payload = serde_json::to_string(&serialised).unwrap_or_else(|_| "{}".to_string());
        self.conn
            .execute(
                "INSERT INTO index_state (key, value) VALUES (?1, ?2) \
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![WATERMARK_KEY, payload],
            )
            .map_err(|e| Error::Sqlite(e.to_string()))?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct ResolvedIds {
    author_id: Option<i64>,
    committer_id: Option<i64>,
    coauthor_ids: Vec<i64>,
}

/// Map a [`Category`] to its position in the SPEC-001 §5.1 counter array:
/// `[test, config, infra, doc, code, dependency, ci, fixture, migration]`.
///
/// The asymmetry between Q14 categories and schema counters
/// (`Generated`/`Asset` have no counter; `dependency`/`fixture` have no
/// classifier hit yet) is intentional — those slots are populated by
/// later ecosystem overlays.
fn compute_counters(classifier: &Classifier, files: &[FileChange]) -> [u32; 9] {
    let mut out = [0u32; 9];
    for f in files {
        match classifier.classify(&f.path) {
            Category::Test => out[0] += 1,
            Category::Config => out[1] += 1,
            Category::Infra => out[2] += 1,
            Category::Docs => out[3] += 1,
            Category::Code => out[4] += 1,
            Category::Ci => out[6] += 1,
            Category::Migration => out[8] += 1,
            // dependency/fixture slots (5, 7) are reserved for future
            // ecosystem overlays; generated/asset never persist.
            Category::Generated | Category::Asset => {}
        }
    }
    out
}

/// Extract code-like identifiers from text and expand them into multiple
/// case variants for improved lexical search (ADR-006).
///
/// Emits camelCase, snake_case, kebab-case, and path-component splits
/// from identifiers found in the input. Capped at 4 KiB per commit.
fn expand_code_tokens(text: &str, out: &mut String) {
    const MAX_BYTES: usize = 4096;

    // Regex to match code-like identifiers: alphanumeric with underscores,
    // hyphens, or camelCase transitions. Matches things like:
    // "foo_bar", "foo-bar", "fooBar", "FooBar", "foo_bar_baz", etc.
    let re = Regex::new(r"[a-zA-Z][a-zA-Z0-9_-]*").unwrap();

    for cap in re.captures_iter(text) {
        if out.len() >= MAX_BYTES {
            break;
        }

        if let Some(m) = cap.get(0) {
            let ident = m.as_str();

            // Skip if too short or already common
            if ident.len() < 3 {
                continue;
            }

            // Generate variants
            let variants = generate_case_variants(ident);
            for variant in variants {
                if out.len() + variant.len() + 1 >= MAX_BYTES {
                    break;
                }
                if !variant.is_empty() {
                    out.push(' ');
                    out.push_str(&variant);
                }
            }

            // Path component splits (e.g., "src/lib.rs" -> "src", "lib", "rs")
            for part in ident.split(['/', '\\', '.', '-', '_']) {
                if part.len() >= 3 && out.len() + part.len() + 1 < MAX_BYTES {
                    out.push(' ');
                    out.push_str(part);
                }
            }
        }
    }
}

/// Generate case variants (camelCase, snake_case, kebab-case) from an identifier.
fn generate_case_variants(ident: &str) -> Vec<String> {
    let mut variants = Vec::new();

    // Skip if the identifier is purely numeric or too short
    if ident.len() < 3 || ident.chars().all(|c| c.is_ascii_digit()) {
        return variants;
    }

    let original = ident.to_string();

    // Extract words from the identifier (handles camelCase, snake_case, kebab-case)
    let words: Vec<String> = extract_words(ident);

    if words.is_empty() {
        return variants;
    }

    // camelCase: first word lowercase, rest capitalized
    let camel: String = words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if i == 0 {
                w.to_lowercase()
            } else {
                capitalize_first(w)
            }
        })
        .collect();
    if camel != original.to_lowercase() && !camel.is_empty() {
        variants.push(camel);
    }

    // snake_case: all lowercase with underscores
    let snake: String = words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_");
    if snake != original.to_lowercase() && !snake.is_empty() {
        variants.push(snake);
    }

    // kebab-case: all lowercase with hyphens
    let kebab: String = words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    if kebab != original.to_lowercase() && !kebab.is_empty() {
        variants.push(kebab);
    }

    variants
}

/// Extract words from an identifier, handling camelCase, snake_case, kebab-case.
fn extract_words(ident: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_was_lowercase = false;

    for c in ident.chars() {
        if c == '_' || c == '-' {
            if !current.is_empty() {
                words.push(current.clone());
                current.clear();
            }
        } else if c.is_uppercase() {
            if prev_was_lowercase && !current.is_empty() {
                // camelCase transition: push previous word
                words.push(current.clone());
                current.clear();
            }
            current.push(c.to_ascii_lowercase());
            prev_was_lowercase = false;
        } else if c.is_lowercase() || c.is_ascii_digit() {
            current.push(c);
            prev_was_lowercase = true;
        }
    }

    if !current.is_empty() {
        words.push(current);
    }

    words
}

/// Capitalize the first character of a string.
fn capitalize_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Synthesise the `expanded` FTS column from subject + body + dirs so
/// the lexical search at M4 has a single pre-flattened column to BM25
/// against.
///
/// Extended per ADR-006 to include code token expansion (camelCase,
/// snake_case, kebab-case, path-component splits) for improved search
/// recall. Capped at 4 KiB per commit.
fn build_expanded(subject: &str, body: &str, dirs: &[String]) -> String {
    let mut s = String::with_capacity(subject.len() + body.len() + 512);
    s.push_str(subject);
    if !body.is_empty() {
        s.push('\n');
        s.push_str(body);
    }
    for d in dirs {
        s.push(' ');
        s.push_str(d);
    }

    // Expand code tokens from subject and body
    expand_code_tokens(subject, &mut s);
    expand_code_tokens(body, &mut s);

    // Expand directory paths
    for d in dirs {
        expand_code_tokens(d, &mut s);
    }

    s
}

/// Hot-path INSERT for the `commits` table. Column order matches
/// `0001_init.sql` verbatim; the ON CONFLICT branch covers idempotent
/// resume after a SIGKILL-mid-chunk crash.
const INSERT_COMMIT_SQL: &str = "INSERT INTO commits ( \
    sha, author_name, author_email, author_identity_id, \
    committer_name, committer_email, committer_identity_id, \
    authored_at, committed_at, authored_tz_offset, committed_tz_offset, \
    subject, body, expanded, \
    parent_shas, parent_count, is_merge, is_root, \
    files_changed, file_count, insertions, deletions, dirs_touched, dir_count, \
    test_files_changed, config_files_changed, infra_files_changed, \
    doc_files_changed, code_files_changed, dependency_files_changed, \
    ci_files_changed, fixture_files_changed, migration_files_changed, \
    is_revert, reverted_by_sha, risk_score, risk_label, \
    admission_signals, story_id, indexed_at, updated_at \
) VALUES ( \
    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, 0, \
    ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, \
    ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, \
    0, NULL, NULL, NULL, '{}', NULL, ?32, ?33 \
) ON CONFLICT(sha) DO UPDATE SET \
    author_identity_id = excluded.author_identity_id, \
    committer_identity_id = excluded.committer_identity_id, \
    subject = excluded.subject, \
    body = excluded.body, \
    expanded = excluded.expanded, \
    parent_shas = excluded.parent_shas, \
    parent_count = excluded.parent_count, \
    is_merge = excluded.is_merge, \
    is_root = excluded.is_root, \
    files_changed = excluded.files_changed, \
    file_count = excluded.file_count, \
    insertions = excluded.insertions, \
    deletions = excluded.deletions, \
    dirs_touched = excluded.dirs_touched, \
    dir_count = excluded.dir_count, \
    test_files_changed = excluded.test_files_changed, \
    config_files_changed = excluded.config_files_changed, \
    infra_files_changed = excluded.infra_files_changed, \
    doc_files_changed = excluded.doc_files_changed, \
    code_files_changed = excluded.code_files_changed, \
    dependency_files_changed = excluded.dependency_files_changed, \
    ci_files_changed = excluded.ci_files_changed, \
    fixture_files_changed = excluded.fixture_files_changed, \
    migration_files_changed = excluded.migration_files_changed, \
    updated_at = excluded.updated_at";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::FileChange;
    use tempfile::tempdir;

    #[test]
    fn compute_counters_maps_categories_into_correct_slots() {
        let dir = tempdir().unwrap();
        let classifier = Classifier::default_for(dir.path()).unwrap();
        let files = vec![
            FileChange {
                path: "tests/it.rs".into(),
                status: 'M',
                insertions: 1,
                deletions: 0,
            },
            FileChange {
                path: "docs/readme.md".into(),
                status: 'A',
                insertions: 5,
                deletions: 0,
            },
            FileChange {
                path: "infra/Dockerfile".into(),
                status: 'M',
                insertions: 1,
                deletions: 0,
            },
            FileChange {
                path: ".github/workflows/ci.yml".into(),
                status: 'A',
                insertions: 10,
                deletions: 0,
            },
            FileChange {
                path: "db/migrations/001.sql".into(),
                status: 'A',
                insertions: 2,
                deletions: 0,
            },
        ];
        let counters = compute_counters(&classifier, &files);
        assert_eq!(counters[0], 1, "test slot");
        assert_eq!(counters[2], 1, "infra slot");
        assert_eq!(counters[3], 1, "doc slot");
        assert_eq!(counters[6], 1, "ci slot");
        assert_eq!(counters[8], 1, "migration slot");
    }

    #[test]
    fn build_expanded_concatenates_subject_body_dirs() {
        let s = build_expanded("fix: thing", "body line", &["src".to_string()]);
        assert!(s.contains("fix: thing"));
        assert!(s.contains("body line"));
        assert!(s.contains(" src"));
    }

    #[test]
    fn build_expanded_omits_body_when_empty() {
        let s = build_expanded("subject only", "", &[]);
        assert!(s.contains("subject only"));
    }

    #[test]
    fn expand_code_tokens_generates_case_variants() {
        let mut s = String::new();
        expand_code_tokens("fooBar", &mut s);
        assert!(s.contains("foo_bar") || s.contains("foo-bar"));
    }

    #[test]
    fn expand_code_tokens_splits_paths() {
        let mut s = String::new();
        expand_code_tokens("src/lib.rs", &mut s);
        assert!(s.contains("src"));
        assert!(s.contains("lib"));
    }

    #[test]
    fn expand_code_tokens_respects_4kib_cap() {
        let long_text = "a".repeat(5000);
        let mut s = String::new();
        expand_code_tokens(&long_text, &mut s);
        assert!(s.len() <= 4096);
    }

    #[test]
    fn extract_words_handles_camel_case() {
        let words = extract_words("fooBarBaz");
        assert_eq!(words, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn extract_words_handles_snake_case() {
        let words = extract_words("foo_bar_baz");
        assert_eq!(words, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn extract_words_handles_kebab_case() {
        let words = extract_words("foo-bar-baz");
        assert_eq!(words, vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn generate_case_variants_camel_input() {
        let variants = generate_case_variants("fooBar");
        assert!(variants
            .iter()
            .any(|v| v.contains("foo_bar") || v.contains("foo-bar")));
    }

    #[test]
    fn generate_case_variants_snake_input() {
        let variants = generate_case_variants("foo_bar");
        assert!(variants
            .iter()
            .any(|v| v == "fooBar" || v.contains("foo-bar")));
    }
}
