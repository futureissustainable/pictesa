CREATE TABLE budgets (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    daily_micros INTEGER CHECK (daily_micros >= 0),
    monthly_micros INTEGER CHECK (monthly_micros >= 0)
);
-- No job foreign key: deleting history must not reset a budget.
CREATE TABLE spend_runs (
    job_id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    estimated_micros INTEGER,
    actual_micros INTEGER,
    created_at INTEGER NOT NULL
);
CREATE INDEX spend_by_user ON spend_runs(user_id, created_at);
INSERT INTO spend_runs
SELECT j.id, j.user_id, j.model, j.provider,
    CASE WHEN j.charged_micros = 0 AND j.provider IN ('comfyui', 'sdcpp', 'a1111', 'mock') THEN 0
         WHEN j.cost_usd IS NULL THEN NULL ELSE CAST(round(j.cost_usd * 1000000) AS INTEGER) END,
    CASE WHEN EXISTS (SELECT 1 FROM ledger l WHERE l.id = 'refund:' || j.id) THEN 0
         WHEN j.charged_micros > 0 THEN j.charged_micros ELSE NULL END,
    j.created_at
FROM jobs j;
