-- Pictesa schema. Local mode stores everything under the single user 'local'.

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    email TEXT UNIQUE,
    name TEXT,
    password_hash TEXT,
    role TEXT NOT NULL DEFAULT 'user',
    created_at INTEGER NOT NULL
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,            -- blake3 hash of the cookie value
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

CREATE TABLE api_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER
);

-- Provider credentials per user. Secret values are encrypted with the instance key.
CREATE TABLE credentials (
    user_id TEXT NOT NULL,
    provider TEXT NOT NULL,
    field TEXT NOT NULL,
    value TEXT NOT NULL,
    encrypted INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, provider, field)
);

CREATE TABLE jobs (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    model TEXT NOT NULL,            -- provider:model
    provider TEXT NOT NULL,
    model_name TEXT NOT NULL,
    output TEXT NOT NULL,           -- image | video | audio
    status TEXT NOT NULL,           -- queued | running | succeeded | failed | canceled
    prompt TEXT NOT NULL DEFAULT '',
    negative_prompt TEXT,
    params TEXT NOT NULL DEFAULT '{}',
    inputs TEXT NOT NULL DEFAULT '[]',
    preset TEXT,
    count INTEGER NOT NULL DEFAULT 1,
    remote TEXT,                    -- provider handle JSON, survives restarts
    progress REAL,
    message TEXT,
    error_kind TEXT,
    error TEXT,
    cost_usd REAL,
    attempts INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    updated_at INTEGER NOT NULL
);
CREATE INDEX jobs_by_status ON jobs (status, created_at);
CREATE INDEX jobs_by_user ON jobs (user_id, id DESC);

CREATE TABLE assets (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    job_id TEXT,
    kind TEXT NOT NULL,             -- image | video | audio
    mime TEXT NOT NULL,
    hash TEXT NOT NULL,             -- blake3 of the bytes; file path is derived from it
    ext TEXT NOT NULL,
    size INTEGER NOT NULL,
    width INTEGER,
    height INTEGER,
    duration_ms INTEGER,
    has_thumb INTEGER NOT NULL DEFAULT 0,
    source TEXT NOT NULL,           -- generated | upload | import
    model TEXT,
    prompt TEXT,
    favorite INTEGER NOT NULL DEFAULT 0,
    source_url TEXT,
    created_at INTEGER NOT NULL,
    deleted_at INTEGER
);
CREATE INDEX assets_by_user ON assets (user_id, deleted_at, id DESC);
CREATE INDEX assets_by_job ON assets (job_id);
CREATE INDEX assets_by_hash ON assets (hash);

CREATE VIRTUAL TABLE assets_fts USING fts5 (prompt, content = 'assets', content_rowid = 'rowid');
CREATE TRIGGER assets_fts_insert AFTER INSERT ON assets BEGIN
    INSERT INTO assets_fts (rowid, prompt) VALUES (new.rowid, coalesce(new.prompt, ''));
END;
CREATE TRIGGER assets_fts_delete AFTER DELETE ON assets BEGIN
    INSERT INTO assets_fts (assets_fts, rowid, prompt) VALUES ('delete', old.rowid, coalesce(old.prompt, ''));
END;
CREATE TRIGGER assets_fts_update AFTER UPDATE OF prompt ON assets BEGIN
    INSERT INTO assets_fts (assets_fts, rowid, prompt) VALUES ('delete', old.rowid, coalesce(old.prompt, ''));
    INSERT INTO assets_fts (rowid, prompt) VALUES (new.rowid, coalesce(new.prompt, ''));
END;

-- Live catalogs (gateway discovery) and lazily resolved model specs.
CREATE TABLE model_cache (
    provider TEXT NOT NULL,
    scope TEXT NOT NULL,            -- 'catalog' (whole discovery list) or a model id (resolved spec)
    data TEXT NOT NULL,
    fetched_at INTEGER NOT NULL,
    PRIMARY KEY (provider, scope)
);

CREATE TABLE settings (
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (user_id, key)
);

INSERT INTO users (id, email, name, role, created_at) VALUES ('local', NULL, 'Local', 'admin', 0);
