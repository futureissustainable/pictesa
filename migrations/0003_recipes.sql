-- Recipe runs: multi-step pipelines (compose a still, then animate it, ...).
CREATE TABLE recipe_runs (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    recipe_id TEXT NOT NULL,
    recipe_name TEXT NOT NULL,
    status TEXT NOT NULL,            -- running | succeeded | failed | canceled
    step INTEGER NOT NULL DEFAULT 0,
    steps INTEGER NOT NULL,
    job_ids TEXT NOT NULL DEFAULT '[]',
    request TEXT NOT NULL,           -- values, inputs and model overrides as sent
    error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX recipe_runs_by_user ON recipe_runs (user_id, id DESC);
