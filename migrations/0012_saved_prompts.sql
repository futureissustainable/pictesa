CREATE TABLE saved_prompts (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    name TEXT NOT NULL,
    prompt TEXT NOT NULL,
    negative_prompt TEXT,
    model TEXT,
    params TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX saved_prompts_by_user ON saved_prompts (user_id, updated_at DESC, id DESC);
CREATE INDEX prompt_history_by_user ON jobs (user_id, prompt, created_at DESC, id DESC);
