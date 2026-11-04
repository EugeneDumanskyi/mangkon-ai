-- App database v3 (M5b, domain §5.3 MODEL_INSTALLATION, §7.7, US-09.2, US-09.4): model
-- downloads. One row per model that left "Available"; no row means Available. `state` is a
-- §7.7 state name. A row in `downloading` or `verifying` when the app opens is set to
-- `interrupted` (the app quit). `error_code`/`error_message` hold the last failure in plain
-- words. Times in UTC milliseconds.
CREATE TABLE model_download (
    model_id      TEXT PRIMARY KEY,
    state         TEXT NOT NULL CHECK (state IN
                      ('downloading', 'paused', 'interrupted', 'verifying', 'installed')),
    bytes_done    INTEGER NOT NULL DEFAULT 0,
    total_bytes   INTEGER NOT NULL,
    error_code    TEXT,
    error_message TEXT,
    updated_at    INTEGER NOT NULL
) STRICT;

-- Files whose SHA-256 matched the catalog (INV-MM-01), so they are not hashed again on start.
CREATE TABLE model_file (
    model_id    TEXT NOT NULL,
    name        TEXT NOT NULL,
    bytes       INTEGER NOT NULL,
    verified_at INTEGER NOT NULL,
    PRIMARY KEY (model_id, name)
) STRICT;

-- App settings as key/value. M5b: `local_models_override` = '1' (US-09.2 AC3, Q7).
CREATE TABLE app_setting (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;
