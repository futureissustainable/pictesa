-- Hosted-mode billing: prepaid USD wallets (micro-dollars), an idempotent ledger and Stripe checkouts.

CREATE TABLE wallets (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    balance_micros INTEGER NOT NULL DEFAULT 0,
    debt_micros INTEGER NOT NULL DEFAULT 0,
    locked INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE ledger (
    id TEXT PRIMARY KEY,            -- idempotency key: charge:<job>, refund:<job>, stripe:<session>, ...
    user_id TEXT NOT NULL,
    amount_micros INTEGER NOT NULL, -- positive adds money, negative spends it
    reason TEXT NOT NULL,
    job_id TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX ledger_by_user ON ledger (user_id, created_at DESC);

CREATE TABLE checkouts (
    user_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    amount_cents INTEGER NOT NULL,
    session_id TEXT,
    url TEXT,
    payment_intent TEXT,
    paid INTEGER NOT NULL DEFAULT 0,
    refunded_cents INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, request_id)
);
CREATE INDEX checkouts_by_intent ON checkouts (payment_intent);

-- What a job charged the wallet, so failures and cancellations refund exactly that.
ALTER TABLE jobs ADD COLUMN charged_micros INTEGER NOT NULL DEFAULT 0;
