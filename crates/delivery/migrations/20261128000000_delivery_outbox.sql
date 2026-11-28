-- Outbound channel deliveries (replies, errors, notices) with their state.
-- Timestamps are Unix milliseconds (UTC).
CREATE TABLE IF NOT EXISTS delivery_outbox (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    session_key     TEXT    NOT NULL,
    run_id          TEXT,
    kind            TEXT    NOT NULL,
    target_key      TEXT    NOT NULL,
    target          TEXT    NOT NULL,
    body            TEXT    NOT NULL,
    reply_to        TEXT,
    state           TEXT    NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    redelivered     INTEGER NOT NULL DEFAULT 0,
    next_attempt_at INTEGER NOT NULL,
    lease_until     INTEGER,
    last_error      TEXT,
    message_ids     TEXT,
    created_at      INTEGER NOT NULL,
    updated_at      INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_delivery_outbox_due ON delivery_outbox (state, next_attempt_at);
CREATE INDEX IF NOT EXISTS idx_delivery_outbox_target ON delivery_outbox (target_key, id);
CREATE INDEX IF NOT EXISTS idx_delivery_outbox_session ON delivery_outbox (session_key, id);
