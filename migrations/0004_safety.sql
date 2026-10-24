-- How a job is paid, decided once at creation: 'local' (single-user), 'byok' (the user's own keys)
-- or 'platform' (operator keys, charged to the wallet). The run uses exactly that credential source.
ALTER TABLE jobs ADD COLUMN billing TEXT NOT NULL DEFAULT 'local';
