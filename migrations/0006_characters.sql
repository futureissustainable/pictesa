-- Characters: named sets of reference images (a person, a product, a style) reused across runs.
CREATE TABLE characters (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    asset_ids TEXT NOT NULL DEFAULT '[]',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX characters_by_user ON characters (user_id, updated_at DESC);
