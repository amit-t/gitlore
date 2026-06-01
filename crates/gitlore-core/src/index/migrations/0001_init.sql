-- gitlore index — schema migration 0001 (SPEC-001 §5.1, M3-2).
--
-- This file is embedded into the binary via `include_str!` and executed
-- inside a single transaction by `super::migrate`. Column ordering matches
-- the Rust mirrors in `super::super::schema` verbatim so a future row
-- binding layer can use field order as the canonical parameter order.
--
-- `commit_vectors` is intentionally NOT created here — it lands at M11 via
-- `gitlore setup-embeddings` once the optional `embeddings` feature is on
-- (SPEC-001 §22 row 6; OQ-T-3).

CREATE TABLE commits (
    sha                        TEXT    PRIMARY KEY,

    -- Identity (raw + resolved)
    author_name                TEXT    NOT NULL,
    author_email               TEXT    NOT NULL,
    author_identity_id         INTEGER,
    committer_name             TEXT    NOT NULL,
    committer_email            TEXT    NOT NULL,
    committer_identity_id      INTEGER,

    -- Timestamps
    authored_at                INTEGER NOT NULL,
    committed_at               INTEGER NOT NULL,
    authored_tz_offset         INTEGER NOT NULL DEFAULT 0,
    committed_tz_offset        INTEGER NOT NULL DEFAULT 0,

    -- Message
    subject                    TEXT    NOT NULL,
    body                       TEXT    NOT NULL DEFAULT '',
    expanded                   TEXT    NOT NULL DEFAULT '',

    -- Topology
    parent_shas                TEXT    NOT NULL DEFAULT '[]',
    parent_count               INTEGER NOT NULL DEFAULT 0,
    is_merge                   INTEGER NOT NULL DEFAULT 0,
    is_root                    INTEGER NOT NULL DEFAULT 0,

    -- File-level changes
    files_changed              TEXT    NOT NULL DEFAULT '[]',
    file_count                 INTEGER NOT NULL DEFAULT 0,
    insertions                 INTEGER NOT NULL DEFAULT 0,
    deletions                  INTEGER NOT NULL DEFAULT 0,
    dirs_touched               TEXT    NOT NULL DEFAULT '[]',
    dir_count                  INTEGER NOT NULL DEFAULT 0,

    -- Classification counters (SPEC-001 §5.1, nine counters)
    test_files_changed         INTEGER NOT NULL DEFAULT 0,
    config_files_changed       INTEGER NOT NULL DEFAULT 0,
    infra_files_changed        INTEGER NOT NULL DEFAULT 0,
    doc_files_changed          INTEGER NOT NULL DEFAULT 0,
    code_files_changed         INTEGER NOT NULL DEFAULT 0,
    dependency_files_changed   INTEGER NOT NULL DEFAULT 0,
    ci_files_changed           INTEGER NOT NULL DEFAULT 0,
    fixture_files_changed      INTEGER NOT NULL DEFAULT 0,
    migration_files_changed    INTEGER NOT NULL DEFAULT 0,

    -- Revert tracking
    is_revert                  INTEGER NOT NULL DEFAULT 0,
    reverted_by_sha            TEXT,

    -- Risk (cached for §15 scorer)
    risk_score                 REAL,
    risk_label                 TEXT,

    -- Story assignment + admission signals
    admission_signals          TEXT    NOT NULL DEFAULT '{}',
    story_id                   INTEGER,

    -- Bookkeeping
    indexed_at                 INTEGER NOT NULL,
    updated_at                 INTEGER NOT NULL
);

CREATE INDEX idx_commits_authored_at ON commits(authored_at);
CREATE INDEX idx_commits_committed_at ON commits(committed_at);
CREATE INDEX idx_commits_author_identity_id ON commits(author_identity_id);
CREATE INDEX idx_commits_story_id ON commits(story_id);

CREATE TABLE identities (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    canonical_name    TEXT    NOT NULL,
    canonical_email   TEXT    NOT NULL,
    first_seen_at     INTEGER NOT NULL DEFAULT 0,
    last_seen_at      INTEGER NOT NULL DEFAULT 0,
    commit_count      INTEGER NOT NULL DEFAULT 0,
    UNIQUE(canonical_email)
);

CREATE TABLE identity_aliases (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    identity_id   INTEGER NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    raw_name      TEXT    NOT NULL,
    raw_email     TEXT    NOT NULL,
    UNIQUE(raw_name, raw_email)
);
CREATE INDEX idx_identity_aliases_identity_id ON identity_aliases(identity_id);

CREATE TABLE commit_coauthors (
    sha           TEXT    NOT NULL REFERENCES commits(sha) ON DELETE CASCADE,
    identity_id   INTEGER NOT NULL REFERENCES identities(id) ON DELETE CASCADE,
    PRIMARY KEY (sha, identity_id)
);
CREATE INDEX idx_commit_coauthors_identity_id ON commit_coauthors(identity_id);

CREATE TABLE commit_refs (
    sha       TEXT NOT NULL REFERENCES commits(sha) ON DELETE CASCADE,
    ref_name  TEXT NOT NULL,
    ref_kind  TEXT NOT NULL,
    PRIMARY KEY (sha, ref_name)
);
CREATE INDEX idx_commit_refs_ref_name ON commit_refs(ref_name);

CREATE TABLE tags (
    ref_name    TEXT    PRIMARY KEY,
    sha         TEXT    NOT NULL REFERENCES commits(sha) ON DELETE CASCADE,
    annotated   INTEGER NOT NULL DEFAULT 0,
    message     TEXT    NOT NULL DEFAULT '',
    tagged_at   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_tags_sha ON tags(sha);

CREATE VIRTUAL TABLE commits_fts USING fts5(
    sha UNINDEXED,
    subject,
    body,
    expanded,
    paths,
    content='',
    tokenize='unicode61 remove_diacritics 2'
);

CREATE TABLE stories (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    title         TEXT    NOT NULL,
    date_start    INTEGER NOT NULL,
    date_end      INTEGER NOT NULL,
    member_count  INTEGER NOT NULL DEFAULT 0,
    top_paths     TEXT    NOT NULL DEFAULT '[]',
    authors       TEXT    NOT NULL DEFAULT '[]',
    risk_score    REAL    NOT NULL DEFAULT 0.0,
    risk_factors  TEXT    NOT NULL DEFAULT '{}',
    generated_at  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_stories_date_end ON stories(date_end);

CREATE TABLE story_members (
    story_id  INTEGER NOT NULL REFERENCES stories(id) ON DELETE CASCADE,
    sha       TEXT    NOT NULL REFERENCES commits(sha) ON DELETE CASCADE,
    PRIMARY KEY (story_id, sha)
);
CREATE INDEX idx_story_members_sha ON story_members(sha);

CREATE TABLE path_stats (
    path              TEXT    PRIMARY KEY,
    commit_count      INTEGER NOT NULL DEFAULT 0,
    unique_authors    INTEGER NOT NULL DEFAULT 0,
    revert_count      INTEGER NOT NULL DEFAULT 0,
    last_touched      INTEGER NOT NULL DEFAULT 0,
    cochange_paths    TEXT    NOT NULL DEFAULT '{}',
    top_contributors  TEXT    NOT NULL DEFAULT '[]'
);

CREATE TABLE repo_stats (
    id                INTEGER PRIMARY KEY CHECK (id = 1),
    commit_count      INTEGER NOT NULL DEFAULT 0,
    identity_count    INTEGER NOT NULL DEFAULT 0,
    first_commit_at   INTEGER NOT NULL DEFAULT 0,
    last_commit_at    INTEGER NOT NULL DEFAULT 0,
    last_indexed_sha  TEXT    NOT NULL DEFAULT '',
    generated_at      INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE index_state (
    key    TEXT PRIMARY KEY,
    value  TEXT NOT NULL
);

INSERT INTO index_state(key, value) VALUES ('schema_version', '1');
