CREATE TABLE loras (
    asset_id TEXT PRIMARY KEY REFERENCES assets(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id),
    character_id TEXT REFERENCES characters(id) ON DELETE SET NULL,
    family TEXT NOT NULL,
    trigger_word TEXT NOT NULL
);
CREATE INDEX loras_user_family ON loras(user_id, family);
CREATE INDEX loras_character ON loras(character_id);
