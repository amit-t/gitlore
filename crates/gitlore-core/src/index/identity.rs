//! Identity resolution layer (M3-4, TDD-000 §2.2, SPEC-001 §4.4, ADR-017).
//!
//! Collapse the open set of `(name, email)` pairs observed in commit
//! history into a stable [`IdentityId`] so downstream consumers (hotspot,
//! story clusterer, ownership) can count "who has touched this file"
//! without overcounting renames, address changes, or bot noise.
//!
//! ## Three-link chain
//!
//! [`ChainedResolver`] walks three layers in order, returning the first
//! [`Ok`] answer:
//!
//! 1. [`OverrideResolver`] — manually curated `(name, email)` rewrites
//!    parsed from `<common-dir>/gitlore/identities.toml`. Highest
//!    precedence so a user can force-correct a misclassified contributor.
//! 2. [`MailmapResolver`] — defers to `git check-mailmap`, persists the
//!    canonical pair, and short-circuits the chain on success. On a
//!    subprocess error the resolver returns `Err(Error::Git)` so the
//!    chain falls through to the fuzzy layer.
//! 3. [`UnionFindResolver`] — last-mile fuzzy clustering per Q13: same
//!    lowercased email merges unconditionally; same normalised name
//!    merges within a 365-day window unless either side is a GitHub
//!    `users.noreply.github.com` address.
//!
//! ## Persistence shape
//!
//! Every resolver writes through the shared SPEC-001 §5.1 schema:
//!
//! * `identities` — `UNIQUE(canonical_email)` is the conflict key for
//!   the `INSERT ... ON CONFLICT(canonical_email) DO UPDATE ... RETURNING id`
//!   UPSERT helper.
//! * `identity_aliases` — `UNIQUE(raw_name, raw_email)` is the conflict
//!   key for the idempotent `INSERT OR IGNORE` alias write.
//! * `is_bot` column lives on `identities` (added by migration 0002) and
//!   is populated by [`is_bot`] for the fuzzy layer or by the override
//!   TOML for explicit curation.
//!
//! ## Cache
//!
//! [`UnionFindResolver`] keeps an in-memory cache hot-loaded from the two
//! tables at construction so the hot path is one HashMap lookup per
//! resolve. `&self`-method mutation is via [`RefCell`] because the
//! resolver lives behind `&'a Connection` and callers usually share it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use unicode_normalization::UnicodeNormalization;

use crate::error::{Error, Result};
use crate::git::GitProvider;

/// Opaque foreign-key into [`crate::index::schema::Identity::id`].
pub type IdentityId = i64;

/// Twelve-month window used by the union-find name-match merge rule
/// (TDD-000 §2.2 Q13).
const NAME_MERGE_WINDOW_SECS: i64 = 365 * 24 * 60 * 60;

/// Read-only resolver: map a raw `(name, email)` pair to a stable
/// [`IdentityId`].
///
/// Implementations may write to the underlying `identities` and
/// `identity_aliases` tables as a side effect of resolution; the trait
/// itself is structurally read-only so it composes inside
/// [`ChainedResolver`].
pub trait IdentityResolver {
    /// Map a raw `(name, email)` pair to a stable [`IdentityId`].
    fn resolve(&self, name: &str, email: &str) -> Result<IdentityId>;
}

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Heuristic bot detector.
///
/// Matches both the GitHub App noreply convention
/// (`<user>[bot]@users.noreply.github.com`) and the `<name>[bot]`
/// display-name suffix used by most GitHub Apps.
pub fn is_bot(name: &str, email: &str) -> bool {
    let email_lower = email.to_ascii_lowercase();
    email_lower.ends_with("[bot]@users.noreply.github.com") || name.trim().ends_with("[bot]")
}

/// `true` iff `email` lives under the GitHub `users.noreply.github.com`
/// suffix. Public so callers (story clusterer, contributor stats) can
/// apply the same "do not merge by name" rule.
pub fn is_github_noreply(email: &str) -> bool {
    email
        .to_ascii_lowercase()
        .ends_with("@users.noreply.github.com")
}

/// Normalise a display name for the union-find name-merge rule: NFKC →
/// trim → collapse internal whitespace runs → ASCII lowercase.
///
/// The order matters: NFKC first so visually equal but binary-distinct
/// inputs (`"é"` vs `"e\u{301}"`) compare equal; trimming after NFKC
/// catches whitespace that NFKC may surface; whitespace collapse keeps
/// `"Alice  Smith"` and `"Alice Smith"` clustered; lowercase last so the
/// final string is byte-stable.
pub fn normalize_name(name: &str) -> String {
    let nfkc: String = name.nfkc().collect();
    let trimmed = nfkc.trim();
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut prev_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                collapsed.push(' ');
                prev_space = true;
            }
        } else {
            collapsed.push(ch);
            prev_space = false;
        }
    }
    collapsed.to_lowercase()
}

fn now_unix() -> i64 {
    chrono::Utc::now().timestamp()
}

fn sqlite_err(e: rusqlite::Error) -> Error {
    Error::Sqlite(e.to_string())
}

/// `INSERT ... ON CONFLICT(canonical_email) DO UPDATE ... RETURNING id`.
///
/// Used by every resolver that writes through the canonical pair. The
/// conflict branch overwrites `canonical_name` and `is_bot` because both
/// override and mailmap treat their own output as the source of truth.
fn upsert_identity_by_email(
    conn: &Connection,
    canonical_name: &str,
    canonical_email: &str,
    is_bot_flag: u8,
) -> Result<IdentityId> {
    let now = now_unix();
    conn.query_row(
        "INSERT INTO identities \
         (canonical_name, canonical_email, first_seen_at, last_seen_at, commit_count, is_bot) \
         VALUES (?1, ?2, ?3, ?3, 0, ?4) \
         ON CONFLICT(canonical_email) DO UPDATE SET \
             canonical_name = excluded.canonical_name, \
             last_seen_at = excluded.last_seen_at, \
             is_bot = excluded.is_bot \
         RETURNING id",
        params![canonical_name, canonical_email, now, is_bot_flag as i64],
        |r| r.get::<_, i64>(0),
    )
    .map_err(sqlite_err)
}

fn insert_alias_if_missing(
    conn: &Connection,
    identity_id: IdentityId,
    raw_name: &str,
    raw_email: &str,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO identity_aliases (identity_id, raw_name, raw_email) \
         VALUES (?1, ?2, ?3)",
        params![identity_id, raw_name, raw_email],
    )
    .map_err(sqlite_err)?;
    Ok(())
}

fn bump_last_seen(conn: &Connection, identity_id: IdentityId) -> Result<i64> {
    let now = now_unix();
    conn.execute(
        "UPDATE identities SET last_seen_at = ?1 WHERE id = ?2",
        params![now, identity_id],
    )
    .map_err(sqlite_err)?;
    Ok(now)
}

// ---------------------------------------------------------------------------
// MailmapResolver
// ---------------------------------------------------------------------------

/// Defers to `git check-mailmap` for the canonical pair, then UPSERTs.
///
/// Behaviour per task spec:
///
/// * On `Ok(MailmapResolved)` — UPSERT `(canonical_name, canonical_email,
///   is_bot=0)` keyed on the canonical email, insert the raw alias, and
///   return the row id.
/// * On `Err(Error::Git)` (subprocess failure or a missing `.mailmap`
///   that surfaces as a subprocess error) — propagate the error so the
///   chain falls through to the fuzzy layer.
pub struct MailmapResolver<'a> {
    /// Borrowed Git provider used to invoke `git check-mailmap`.
    pub provider: &'a dyn GitProvider,
    /// Borrowed connection used for the UPSERT + alias write.
    pub conn: &'a Connection,
}

impl<'a> MailmapResolver<'a> {
    /// Bundle the provider + connection together.
    pub fn new(provider: &'a dyn GitProvider, conn: &'a Connection) -> Self {
        Self { provider, conn }
    }
}

impl<'a> IdentityResolver for MailmapResolver<'a> {
    fn resolve(&self, name: &str, email: &str) -> Result<IdentityId> {
        let resolved = self.provider.check_mailmap(name, email)?;
        let id = upsert_identity_by_email(self.conn, &resolved.name, &resolved.email, 0)?;
        insert_alias_if_missing(self.conn, id, name, email)?;
        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// UnionFindResolver
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct UnionFindCache {
    /// Exact `(name_norm, email_lower)` cache hits. Fast path.
    by_pair: HashMap<(String, String), IdentityId>,
    /// `email_lower` → first identity that registered that email. Drives
    /// rule (a): unconditional email merge.
    by_email: HashMap<String, IdentityId>,
    /// `name_norm` → identities that have appeared under that name.
    /// Drives rule (b): windowed name merge.
    by_name: HashMap<String, Vec<IdentityId>>,
    /// Most-recently observed `last_seen_at` per identity, used to gate
    /// rule (b)'s 365-day window without round-tripping to SQLite.
    last_touched: HashMap<IdentityId, i64>,
}

/// Layer-2 fuzzy resolver (TDD-000 §2.2 Q13).
///
/// Rule (a): any incoming pair whose `email.to_ascii_lowercase()`
/// matches an existing canonical or alias email merges unconditionally.
/// Rule (b): otherwise, any incoming pair whose [`normalize_name`] form
/// matches an existing identity touched within the last 365 days merges,
/// *unless* either side is a GitHub `users.noreply.github.com` address.
/// Misses INSERT a fresh identity + alias row.
///
/// The cache is hot-loaded at construction so the steady-state cost is
/// one HashMap probe per resolve.
pub struct UnionFindResolver<'a> {
    /// Borrowed connection used for the cache-miss INSERT and the
    /// `last_seen_at` bumps.
    pub conn: &'a Connection,
    cache: RefCell<UnionFindCache>,
}

impl<'a> UnionFindResolver<'a> {
    /// Hot-load the in-memory cache from `identities` + `identity_aliases`.
    pub fn new(conn: &'a Connection) -> Result<Self> {
        let cache = Self::hot_load(conn)?;
        Ok(Self {
            conn,
            cache: RefCell::new(cache),
        })
    }

    fn hot_load(conn: &Connection) -> Result<UnionFindCache> {
        let mut cache = UnionFindCache::default();

        // Canonical rows first so they win the `by_email` first-write race
        // when an alias happens to share the same lowercased email.
        {
            let mut stmt = conn
                .prepare("SELECT id, canonical_name, canonical_email, last_seen_at FROM identities")
                .map_err(sqlite_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                })
                .map_err(sqlite_err)?;
            for row in rows {
                let (id, canon_name, canon_email, last_seen) = row.map_err(sqlite_err)?;
                let name_norm = normalize_name(&canon_name);
                let email_lower = canon_email.to_ascii_lowercase();
                cache
                    .by_pair
                    .insert((name_norm.clone(), email_lower.clone()), id);
                cache.by_email.entry(email_lower).or_insert(id);
                let bucket = cache.by_name.entry(name_norm).or_default();
                if !bucket.contains(&id) {
                    bucket.push(id);
                }
                cache.last_touched.insert(id, last_seen);
            }
        }

        {
            let mut stmt = conn
                .prepare("SELECT identity_id, raw_name, raw_email FROM identity_aliases")
                .map_err(sqlite_err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(sqlite_err)?;
            for row in rows {
                let (id, raw_name, raw_email) = row.map_err(sqlite_err)?;
                let name_norm = normalize_name(&raw_name);
                let email_lower = raw_email.to_ascii_lowercase();
                cache
                    .by_pair
                    .insert((name_norm.clone(), email_lower.clone()), id);
                cache.by_email.entry(email_lower).or_insert(id);
                let bucket = cache.by_name.entry(name_norm).or_default();
                if !bucket.contains(&id) {
                    bucket.push(id);
                }
            }
        }

        Ok(cache)
    }

    fn attach_alias(
        &self,
        id: IdentityId,
        raw_name: &str,
        raw_email: &str,
        name_norm: &str,
        email_lower: &str,
    ) -> Result<()> {
        insert_alias_if_missing(self.conn, id, raw_name, raw_email)?;
        let mut cache = self.cache.borrow_mut();
        cache
            .by_pair
            .insert((name_norm.to_string(), email_lower.to_string()), id);
        cache.by_email.entry(email_lower.to_string()).or_insert(id);
        let bucket = cache.by_name.entry(name_norm.to_string()).or_default();
        if !bucket.contains(&id) {
            bucket.push(id);
        }
        Ok(())
    }

    fn touch(&self, id: IdentityId) -> Result<()> {
        let now = bump_last_seen(self.conn, id)?;
        self.cache.borrow_mut().last_touched.insert(id, now);
        Ok(())
    }
}

impl<'a> IdentityResolver for UnionFindResolver<'a> {
    fn resolve(&self, name: &str, email: &str) -> Result<IdentityId> {
        let name_norm = normalize_name(name);
        let email_lower = email.to_ascii_lowercase();
        let cache_key = (name_norm.clone(), email_lower.clone());

        // Fast path: exact `(name_norm, email_lower)` cache hit. The
        // borrow is released before `touch` reaches for `borrow_mut`.
        let pair_hit = self.cache.borrow().by_pair.get(&cache_key).copied();
        if let Some(id) = pair_hit {
            self.touch(id)?;
            return Ok(id);
        }

        // Rule (a): unconditional merge on lowercased email.
        let email_hit = self.cache.borrow().by_email.get(&email_lower).copied();
        if let Some(id) = email_hit {
            self.attach_alias(id, name, email, &name_norm, &email_lower)?;
            self.touch(id)?;
            return Ok(id);
        }

        // Rule (b): windowed merge on normalised name. Skipped for
        // GitHub noreply on either side — noreply never clusters by name.
        if !is_github_noreply(email) {
            let now = now_unix();
            let candidate_id = self.pick_name_candidate(&name_norm, now);
            if let Some(id) = candidate_id {
                if !self.candidate_is_noreply(id)? {
                    self.attach_alias(id, name, email, &name_norm, &email_lower)?;
                    self.touch(id)?;
                    return Ok(id);
                }
            }
        }

        // Miss: INSERT a fresh identity + alias.
        let bot_flag = u8::from(is_bot(name, email));
        let id = upsert_identity_by_email(self.conn, name, email, bot_flag)?;
        insert_alias_if_missing(self.conn, id, name, email)?;

        let mut cache = self.cache.borrow_mut();
        cache.by_pair.insert(cache_key, id);
        cache.by_email.entry(email_lower).or_insert(id);
        cache.by_name.entry(name_norm).or_default().push(id);
        cache.last_touched.insert(id, now_unix());
        Ok(id)
    }
}

impl<'a> UnionFindResolver<'a> {
    /// Pick the most-recently-touched candidate inside the 365-day
    /// window. Returns `None` if no candidate is in-window.
    fn pick_name_candidate(&self, name_norm: &str, now: i64) -> Option<IdentityId> {
        let cache = self.cache.borrow();
        cache.by_name.get(name_norm).and_then(|ids| {
            ids.iter()
                .copied()
                .filter_map(|id| cache.last_touched.get(&id).copied().map(|ts| (id, ts)))
                .filter(|(_, ts)| (now - ts).abs() <= NAME_MERGE_WINDOW_SECS)
                .max_by_key(|(_, ts)| *ts)
                .map(|(id, _)| id)
        })
    }

    /// Lookup the candidate's canonical email and test it against the
    /// GitHub noreply suffix.
    fn candidate_is_noreply(&self, id: IdentityId) -> Result<bool> {
        let canonical_email: Option<String> = self
            .conn
            .query_row(
                "SELECT canonical_email FROM identities WHERE id = ?1",
                params![id],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(sqlite_err)?;
        Ok(canonical_email
            .as_deref()
            .map(is_github_noreply)
            .unwrap_or(false))
    }
}

// ---------------------------------------------------------------------------
// OverrideResolver
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct OverrideFile {
    #[serde(default, rename = "override")]
    overrides: Vec<OverrideEntry>,
}

#[derive(Debug, Deserialize)]
struct OverrideEntry {
    name: String,
    email: String,
    canonical_name: String,
    canonical_email: String,
    #[serde(default)]
    is_bot: bool,
}

/// User-curated `(raw_name, raw_email) → (canonical_name, canonical_email)`
/// rewrites loaded from `<common-dir>/gitlore/identities.toml`.
///
/// Highest precedence in the [`ChainedResolver`]: a match always wins
/// over mailmap and union-find. The TOML format:
///
/// ```toml
/// [[override]]
/// name = "Alice"
/// email = "alice@old.com"
/// canonical_name = "Alice Example"
/// canonical_email = "alice@example.com"
/// is_bot = false
/// ```
///
/// On miss the resolver returns `Err(Error::ConfigInvalidKey { key })`
/// so the chain falls through to mailmap. The error variant is reused
/// (not new) so we do not enlarge the stable error catalogue.
pub struct OverrideResolver<'a> {
    /// Borrowed connection used for the UPSERT.
    pub conn: &'a Connection,
    /// `(raw_name, raw_email) → (canonical_name, canonical_email)` map.
    pub overrides: HashMap<(String, String), (String, String)>,
    bot_overrides: HashMap<(String, String), bool>,
}

impl<'a> OverrideResolver<'a> {
    /// Construct from an already-parsed map. The `bot_overrides`
    /// sibling map is left empty (i.e. no overrides are bots).
    pub fn new(
        conn: &'a Connection,
        overrides: HashMap<(String, String), (String, String)>,
    ) -> Self {
        Self {
            conn,
            overrides,
            bot_overrides: HashMap::new(),
        }
    }

    /// Read `<common_dir>/gitlore/identities.toml` and parse it. A
    /// missing file is treated as "no overrides" (`Ok` empty resolver),
    /// per the project's "useful with zero configuration" contract
    /// (SPEC-001 §8). Parse errors surface as [`Error::Io`] with
    /// `ErrorKind::InvalidData`.
    pub fn from_common_dir(conn: &'a Connection, common_dir: &Path) -> Result<Self> {
        let path = common_dir.join("gitlore").join("identities.toml");
        if !path.exists() {
            return Ok(Self::new(conn, HashMap::new()));
        }
        let raw = std::fs::read_to_string(&path)?;
        let parsed: OverrideFile = toml::from_str(&raw).map_err(|e| {
            Error::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse {}: {e}", path.display()),
            ))
        })?;
        let mut overrides: HashMap<(String, String), (String, String)> = HashMap::new();
        let mut bots: HashMap<(String, String), bool> = HashMap::new();
        for entry in parsed.overrides {
            let key = (entry.name.clone(), entry.email.clone());
            overrides.insert(key.clone(), (entry.canonical_name, entry.canonical_email));
            if entry.is_bot {
                bots.insert(key, true);
            }
        }
        Ok(Self {
            conn,
            overrides,
            bot_overrides: bots,
        })
    }
}

impl<'a> IdentityResolver for OverrideResolver<'a> {
    fn resolve(&self, name: &str, email: &str) -> Result<IdentityId> {
        let key = (name.to_string(), email.to_string());
        let Some((canon_name, canon_email)) = self.overrides.get(&key) else {
            return Err(Error::ConfigInvalidKey {
                key: format!("identity_override({name}, {email})"),
            });
        };
        let bot_flag = if self.bot_overrides.get(&key).copied().unwrap_or(false) {
            1u8
        } else {
            0u8
        };
        let id = upsert_identity_by_email(self.conn, canon_name, canon_email, bot_flag)?;
        insert_alias_if_missing(self.conn, id, name, email)?;
        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// ChainedResolver
// ---------------------------------------------------------------------------

/// Composes the three resolvers in the documented precedence order:
/// `override → mailmap → union_find`. Returns the first non-`Err`
/// answer; if every layer errors, the last error propagates.
pub struct ChainedResolver<O, M, U>
where
    O: IdentityResolver,
    M: IdentityResolver,
    U: IdentityResolver,
{
    /// Layer 1: manual overrides.
    pub override_: O,
    /// Layer 2: `.mailmap`-driven canonicalisation.
    pub mailmap: M,
    /// Layer 3: fuzzy union-find fallback.
    pub union_find: U,
}

impl<O, M, U> ChainedResolver<O, M, U>
where
    O: IdentityResolver,
    M: IdentityResolver,
    U: IdentityResolver,
{
    /// Bundle the three layers together in precedence order.
    pub fn new(override_: O, mailmap: M, union_find: U) -> Self {
        Self {
            override_,
            mailmap,
            union_find,
        }
    }
}

impl<O, M, U> IdentityResolver for ChainedResolver<O, M, U>
where
    O: IdentityResolver,
    M: IdentityResolver,
    U: IdentityResolver,
{
    fn resolve(&self, name: &str, email: &str) -> Result<IdentityId> {
        if let Ok(id) = self.override_.resolve(name, email) {
            return Ok(id);
        }
        if let Ok(id) = self.mailmap.resolve(name, email) {
            return Ok(id);
        }
        self.union_find.resolve(name, email)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::{MailmapResolved, RefEntry, RefScope, Sha, ShowOpts, WalkRange};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    // ---- helpers ----------------------------------------------------------

    fn fresh_db() -> Connection {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();
        conn
    }

    /// Minimal in-memory GitProvider for resolver tests. Only the
    /// `check_mailmap` method does real work; every other method panics
    /// because the resolvers do not call them.
    struct StubGitProvider {
        // Map (name, email) -> resolved pair.
        rules: HashMap<(String, String), MailmapResolved>,
        // When true, every check_mailmap call returns Error::Git.
        fail: AtomicBool,
        calls: AtomicUsize,
    }

    impl StubGitProvider {
        fn empty() -> Self {
            Self {
                rules: HashMap::new(),
                fail: AtomicBool::new(false),
                calls: AtomicUsize::new(0),
            }
        }

        fn with_rule(mut self, raw: (&str, &str), resolved: (&str, &str)) -> Self {
            self.rules.insert(
                (raw.0.to_string(), raw.1.to_string()),
                MailmapResolved {
                    name: resolved.0.to_string(),
                    email: resolved.1.to_string(),
                },
            );
            self
        }

        fn failing() -> Self {
            let s = Self::empty();
            s.fail.store(true, Ordering::SeqCst);
            s
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl GitProvider for StubGitProvider {
        fn common_dir(&self) -> Result<PathBuf> {
            unimplemented!("not used by identity resolver tests")
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
        fn check_mailmap(&self, name: &str, email: &str) -> Result<MailmapResolved> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(Error::Git {
                    stderr: "mock failure".to_string(),
                    code: 1,
                });
            }
            if let Some(resolved) = self.rules.get(&(name.to_string(), email.to_string())) {
                return Ok(resolved.clone());
            }
            // Default git check-mailmap behaviour: echo input unchanged.
            Ok(MailmapResolved {
                name: name.to_string(),
                email: email.to_string(),
            })
        }
        fn cat_file_exists(&self, _: &Sha) -> Result<bool> {
            unimplemented!()
        }
    }

    // ---- pure helpers -----------------------------------------------------

    #[test]
    fn normalize_name_nfkc_unifies_combined_and_precomposed() {
        // NFKC turns the decomposed "e + combining acute" into precomposed "é"
        let decomposed = "e\u{0301}".to_string(); // "é" as 2 code points
        let precomposed = "\u{00E9}".to_string(); // "é" as 1 code point
        assert_eq!(normalize_name(&decomposed), normalize_name(&precomposed));
    }

    #[test]
    fn normalize_name_collapses_whitespace_and_lowercases() {
        assert_eq!(normalize_name("  Alice   Smith  "), "alice smith");
        assert_eq!(normalize_name("\tBOB\nJONES "), "bob jones");
    }

    #[test]
    fn normalize_name_empty_input_is_empty() {
        assert_eq!(normalize_name(""), "");
        assert_eq!(normalize_name("   "), "");
    }

    #[test]
    fn is_bot_detects_github_app_email() {
        assert!(is_bot(
            "dependabot[bot]",
            "1234+dependabot[bot]@users.noreply.github.com"
        ));
        // Same suffix on the email itself triggers the heuristic even for
        // a non-`[bot]` display name.
        assert!(is_bot("plain-name", "x[bot]@users.noreply.github.com"));
        // A plain noreply email without the [bot] suffix and a plain name
        // is NOT classified as a bot.
        assert!(!is_bot("alice", "anything@users.noreply.github.com"));
    }

    #[test]
    fn is_bot_detects_name_suffix() {
        assert!(is_bot("renovate[bot]", "renovate@whatever.com"));
        assert!(is_bot("github-actions[bot]", "actions@x.com"));
    }

    #[test]
    fn is_bot_returns_false_for_humans() {
        assert!(!is_bot("Alice", "alice@example.com"));
        assert!(!is_bot("Bob Builder", "bob@bob.tld"));
    }

    #[test]
    fn is_github_noreply_matches_suffix_case_insensitively() {
        assert!(is_github_noreply("12345+alice@users.noreply.github.com"));
        assert!(is_github_noreply("X@USERS.NOREPLY.GITHUB.COM"));
        assert!(!is_github_noreply("alice@example.com"));
    }

    // ---- MailmapResolver --------------------------------------------------

    #[test]
    fn mailmap_resolver_returns_stable_id_for_canonical_pair() {
        let conn = fresh_db();
        let provider = StubGitProvider::empty()
            .with_rule(("a", "a@x"), ("Alice Example", "alice@example.com"));
        let r = MailmapResolver::new(&provider, &conn);
        let id1 = r.resolve("a", "a@x").unwrap();
        let id2 = r.resolve("a", "a@x").unwrap();
        assert_eq!(id1, id2);
        // canonical_email is what got UPSERTed
        let canon: String = conn
            .query_row(
                "SELECT canonical_email FROM identities WHERE id = ?1",
                params![id1],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(canon, "alice@example.com");
    }

    #[test]
    fn mailmap_resolver_records_raw_alias() {
        let conn = fresh_db();
        let provider =
            StubGitProvider::empty().with_rule(("a", "a@x"), ("Alice", "alice@example.com"));
        let r = MailmapResolver::new(&provider, &conn);
        let _ = r.resolve("a", "a@x").unwrap();
        let alias_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM identity_aliases WHERE raw_name = 'a' AND raw_email = 'a@x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(alias_count, 1);
    }

    #[test]
    fn mailmap_resolver_propagates_git_error() {
        let conn = fresh_db();
        let provider = StubGitProvider::failing();
        let r = MailmapResolver::new(&provider, &conn);
        let err = r.resolve("a", "a@x").unwrap_err();
        assert_eq!(err.code(), "git");
    }

    #[test]
    fn mailmap_resolver_persists_is_bot_zero() {
        let conn = fresh_db();
        // Even when the raw pair would heuristically match a bot,
        // MailmapResolver UPSERTs is_bot = 0 per spec.
        let provider = StubGitProvider::empty().with_rule(
            ("renovate[bot]", "renovate@x"),
            ("Renovate Bot", "renovate@example.com"),
        );
        let r = MailmapResolver::new(&provider, &conn);
        let id = r.resolve("renovate[bot]", "renovate@x").unwrap();
        let bot: i64 = conn
            .query_row(
                "SELECT is_bot FROM identities WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bot, 0);
    }

    // ---- UnionFindResolver ------------------------------------------------

    #[test]
    fn union_find_unconditional_email_merge() {
        let conn = fresh_db();
        let r = UnionFindResolver::new(&conn).unwrap();
        let id1 = r.resolve("Alice", "alice@example.com").unwrap();
        // Different name, same email (case-insensitive) -> same id
        let id2 = r.resolve("Alice S.", "ALICE@example.com").unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn union_find_name_merge_within_window() {
        let conn = fresh_db();
        let r = UnionFindResolver::new(&conn).unwrap();
        let id1 = r.resolve("Alice Smith", "alice@one.com").unwrap();
        // Same normalised name, different email, fresh window -> merge.
        let id2 = r.resolve("alice  smith", "alice@two.com").unwrap();
        assert_eq!(id1, id2);
    }

    #[test]
    fn union_find_name_merge_skipped_outside_window() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();
        // Seed an identity whose last_seen_at is two years ago.
        let stale_ts = now_unix() - (2 * NAME_MERGE_WINDOW_SECS);
        conn.execute(
            "INSERT INTO identities \
             (canonical_name, canonical_email, first_seen_at, last_seen_at, commit_count, is_bot) \
             VALUES ('Alice Smith', 'old@x.com', ?1, ?1, 0, 0)",
            params![stale_ts],
        )
        .unwrap();
        let r = UnionFindResolver::new(&conn).unwrap();
        let id_new = r.resolve("Alice Smith", "new@y.com").unwrap();
        let id_old: i64 = conn
            .query_row(
                "SELECT id FROM identities WHERE canonical_email = 'old@x.com'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_ne!(id_new, id_old, "stale identities must not name-merge");
    }

    #[test]
    fn union_find_noreply_never_clusters_by_name() {
        let conn = fresh_db();
        let r = UnionFindResolver::new(&conn).unwrap();
        // First seen on a normal address.
        let id1 = r.resolve("Alice", "alice@example.com").unwrap();
        // Same name, but GitHub noreply -> must NOT merge.
        let id2 = r
            .resolve("Alice", "1234+alice@users.noreply.github.com")
            .unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn union_find_noreply_canonical_blocks_inbound_name_merge() {
        let conn = fresh_db();
        let r = UnionFindResolver::new(&conn).unwrap();
        // First seen on noreply.
        let id1 = r
            .resolve("Alice", "1234+alice@users.noreply.github.com")
            .unwrap();
        // Same name from a non-noreply address -> still must not merge,
        // because the existing identity is noreply.
        let id2 = r.resolve("Alice", "alice@example.com").unwrap();
        assert_ne!(id1, id2);
    }

    #[test]
    fn union_find_miss_creates_new_identity_and_alias() {
        let conn = fresh_db();
        let r = UnionFindResolver::new(&conn).unwrap();
        let id = r.resolve("Brand New", "bn@x.com").unwrap();
        let alias_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM identity_aliases WHERE identity_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alias_count, 1);
    }

    #[test]
    fn union_find_miss_applies_bot_heuristic() {
        let conn = fresh_db();
        let r = UnionFindResolver::new(&conn).unwrap();
        let id = r
            .resolve(
                "renovate[bot]",
                "1234+renovate[bot]@users.noreply.github.com",
            )
            .unwrap();
        let bot: i64 = conn
            .query_row(
                "SELECT is_bot FROM identities WHERE id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(bot, 1);
    }

    #[test]
    fn union_find_cache_persists_across_resolves() {
        let conn = fresh_db();
        let r = UnionFindResolver::new(&conn).unwrap();
        let id1 = r.resolve("Alice", "a@x").unwrap();
        let id2 = r.resolve("Alice", "a@x").unwrap();
        assert_eq!(id1, id2);
        // Only one identity row created.
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM identities", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn union_find_hot_loads_existing_aliases() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::index::migrations::migrate(&mut conn).unwrap();
        let now = now_unix();
        conn.execute(
            "INSERT INTO identities \
             (canonical_name, canonical_email, first_seen_at, last_seen_at, commit_count, is_bot) \
             VALUES ('Alice', 'alice@example.com', ?1, ?1, 0, 0)",
            params![now],
        )
        .unwrap();
        let canonical_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO identity_aliases (identity_id, raw_name, raw_email) \
             VALUES (?1, 'alice', 'alias@x.com')",
            params![canonical_id],
        )
        .unwrap();
        let r = UnionFindResolver::new(&conn).unwrap();
        // Resolving against the *alias* email reuses the canonical id.
        let id = r.resolve("Anything", "ALIAS@X.COM").unwrap();
        assert_eq!(id, canonical_id);
    }

    // ---- OverrideResolver -------------------------------------------------

    #[test]
    fn override_resolver_returns_canonical_id_on_match() {
        let conn = fresh_db();
        let mut map: HashMap<(String, String), (String, String)> = HashMap::new();
        map.insert(
            ("a".to_string(), "a@x".to_string()),
            ("Alice".to_string(), "alice@example.com".to_string()),
        );
        let r = OverrideResolver::new(&conn, map);
        let id = r.resolve("a", "a@x").unwrap();
        let canon: String = conn
            .query_row(
                "SELECT canonical_email FROM identities WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(canon, "alice@example.com");
    }

    #[test]
    fn override_resolver_returns_err_on_miss() {
        let conn = fresh_db();
        let r = OverrideResolver::new(&conn, HashMap::new());
        let err = r.resolve("a", "a@x").unwrap_err();
        // Reuses an existing error code so the catalogue stays stable.
        assert_eq!(err.code(), "config_invalid_key");
    }

    #[test]
    fn override_resolver_parses_toml_with_is_bot_true() {
        let tmp = tempfile::tempdir().unwrap();
        let gitlore_dir = tmp.path().join("gitlore");
        std::fs::create_dir_all(&gitlore_dir).unwrap();
        let toml_path = gitlore_dir.join("identities.toml");
        std::fs::write(
            &toml_path,
            r#"
[[override]]
name = "Dependabot"
email = "dependabot@example.com"
canonical_name = "dependabot[bot]"
canonical_email = "dependabot[bot]@users.noreply.github.com"
is_bot = true
"#,
        )
        .unwrap();
        let conn = fresh_db();
        let r = OverrideResolver::from_common_dir(&conn, tmp.path()).unwrap();
        let id = r.resolve("Dependabot", "dependabot@example.com").unwrap();
        let bot: i64 = conn
            .query_row(
                "SELECT is_bot FROM identities WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(bot, 1);
    }

    #[test]
    fn override_resolver_missing_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let conn = fresh_db();
        let r = OverrideResolver::from_common_dir(&conn, tmp.path()).unwrap();
        assert!(r.overrides.is_empty());
        let err = r.resolve("a", "a@x").unwrap_err();
        assert_eq!(err.code(), "config_invalid_key");
    }

    // ---- ChainedResolver --------------------------------------------------

    #[test]
    fn chain_returns_override_first() {
        let conn = fresh_db();
        // Override map matches.
        let mut map: HashMap<(String, String), (String, String)> = HashMap::new();
        map.insert(
            ("a".to_string(), "a@x".to_string()),
            ("Alice O".to_string(), "alice@override.com".to_string()),
        );
        let or = OverrideResolver::new(&conn, map);
        // Mailmap would also succeed (but should be skipped).
        let provider =
            StubGitProvider::empty().with_rule(("a", "a@x"), ("Alice M", "alice@mailmap.com"));
        let mr = MailmapResolver::new(&provider, &conn);
        let uf = UnionFindResolver::new(&conn).unwrap();
        let chain = ChainedResolver::new(or, mr, uf);

        let id = chain.resolve("a", "a@x").unwrap();
        let canon: String = conn
            .query_row(
                "SELECT canonical_email FROM identities WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(canon, "alice@override.com");
        // Mailmap stub was NOT called because override matched.
        assert_eq!(provider.calls(), 0);
    }

    #[test]
    fn chain_falls_through_to_mailmap_when_override_misses() {
        let conn = fresh_db();
        let or = OverrideResolver::new(&conn, HashMap::new());
        let provider =
            StubGitProvider::empty().with_rule(("a", "a@x"), ("Alice M", "alice@mailmap.com"));
        let mr = MailmapResolver::new(&provider, &conn);
        let uf = UnionFindResolver::new(&conn).unwrap();
        let chain = ChainedResolver::new(or, mr, uf);

        let id = chain.resolve("a", "a@x").unwrap();
        let canon: String = conn
            .query_row(
                "SELECT canonical_email FROM identities WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(canon, "alice@mailmap.com");
    }

    #[test]
    fn chain_falls_through_to_union_find_when_mailmap_fails() {
        let conn = fresh_db();
        let or = OverrideResolver::new(&conn, HashMap::new());
        let provider = StubGitProvider::failing();
        let mr = MailmapResolver::new(&provider, &conn);
        let uf = UnionFindResolver::new(&conn).unwrap();
        let chain = ChainedResolver::new(or, mr, uf);

        // Mailmap will error -> chain falls through to union-find,
        // which inserts a fresh identity using the raw pair.
        let id = chain.resolve("Carol", "carol@example.com").unwrap();
        let canon: String = conn
            .query_row(
                "SELECT canonical_email FROM identities WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(canon, "carol@example.com");
    }
}
