-- `submitting` is 1 only while a submit call is in flight (a refused submit resets it), so a
-- cancel or crash during a retry pause can be refunded. `owner` is the process running the job:
-- after another process adopts it, the first one's writes no longer match and it stops.
ALTER TABLE jobs ADD COLUMN submitting INTEGER NOT NULL DEFAULT 0;
ALTER TABLE jobs ADD COLUMN owner TEXT;
