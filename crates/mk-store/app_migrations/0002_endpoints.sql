-- App database v2 (M5a, domain §5.2, US-09.6, ADR-0012): external model endpoints and their
-- tests. There is no column for an API key: `secret_ref` is the account of the key in the OS
-- secret store (service `mangkon-ai`), NULL when the endpoint has no key (INV-EG-07).
-- `id` is 128 random bits in hex, so an id from another machine never matches by chance.
-- `structured_output` is NULL until a test has run, then 0 or 1 (US-09.6 AC3).
-- Times in UTC milliseconds.
CREATE TABLE endpoint (
    id                TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    base_url          TEXT NOT NULL,
    model_name        TEXT NOT NULL,
    secret_ref        TEXT,
    structured_output INTEGER CHECK (structured_output IN (0, 1)),
    created_at        INTEGER NOT NULL,
    updated_at        INTEGER NOT NULL
) STRICT;

-- One row per test (US-09.6 AC2). `message` is the plain result line; it never holds the key.
CREATE TABLE endpoint_test (
    id             INTEGER PRIMARY KEY,
    endpoint_id    TEXT NOT NULL REFERENCES endpoint (id) ON DELETE CASCADE,
    tested_at      INTEGER NOT NULL,
    ok             INTEGER NOT NULL CHECK (ok IN (0, 1)),
    message        TEXT NOT NULL,
    response_ms    INTEGER,
    reported_model TEXT
) STRICT;
CREATE INDEX endpoint_test_by_endpoint ON endpoint_test (endpoint_id, tested_at);
