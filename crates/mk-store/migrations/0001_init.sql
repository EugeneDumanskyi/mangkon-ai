-- Schema v1 (ADR-0004, domain §5.1). Machine output, jobs and the decision log.
-- Derived tables are not here: they live in derived.sql and are rebuilt, never migrated.

CREATE TABLE project_meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
) STRICT;

-- A folder or single file the project reads from (domain D12).
CREATE TABLE source_root (
    id       INTEGER PRIMARY KEY,
    path     TEXT NOT NULL UNIQUE,
    kind     TEXT NOT NULL CHECK (kind IN ('folder', 'file')),
    added_at TEXT NOT NULL
) STRICT;

-- Source content: one per distinct file hash (INV-CO-02). States: domain §7.1.
CREATE TABLE content (
    id                   INTEGER PRIMARY KEY,
    hash                 BLOB NOT NULL UNIQUE CHECK (length(hash) = 32),
    file_type            TEXT NOT NULL,
    size                 INTEGER NOT NULL,
    state                TEXT NOT NULL,
    state_before_missing TEXT,
    failure_code         TEXT,
    failure_reason       TEXT,
    warnings             TEXT NOT NULL DEFAULT ''
) STRICT;

-- A path inside a source root that points to a content. `rel_path` is '/'-separated and
-- empty for a single-file root. mtime in nanoseconds since the Unix epoch.
CREATE TABLE file_ref (
    id           INTEGER PRIMARY KEY,
    root_id      INTEGER NOT NULL REFERENCES source_root (id),
    rel_path     TEXT NOT NULL,
    content_id   INTEGER NOT NULL REFERENCES content (id),
    size         INTEGER NOT NULL,
    mtime_ns     INTEGER NOT NULL,
    present      INTEGER NOT NULL CHECK (present IN (0, 1)),
    last_seen_at TEXT NOT NULL
) STRICT;
CREATE UNIQUE INDEX file_ref_path ON file_ref (root_id, rel_path);
CREATE INDEX file_ref_content ON file_ref (content_id);

CREATE TABLE document (
    id         INTEGER PRIMARY KEY,
    content_id INTEGER NOT NULL REFERENCES content (id) ON DELETE CASCADE,
    parent_id  INTEGER REFERENCES document (id) ON DELETE CASCADE,
    position   TEXT NOT NULL,
    kind       TEXT NOT NULL
) STRICT;
CREATE INDEX document_content ON document (content_id);
CREATE INDEX document_parent ON document (parent_id);

-- Locator (INV-CO-03): `loc_kind` + `loc_pos` (line for text_line, row for csv_row).
-- CSV rows keep the character offset of each cell in `text` as little-endian u32s.
CREATE TABLE segment (
    id          INTEGER PRIMARY KEY,
    document_id INTEGER NOT NULL REFERENCES document (id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL,
    loc_kind    TEXT NOT NULL,
    loc_pos     INTEGER NOT NULL,
    text        TEXT NOT NULL,
    malformed   INTEGER NOT NULL DEFAULT 0 CHECK (malformed IN (0, 1)),
    cell_starts BLOB
) STRICT;
CREATE INDEX segment_document ON segment (document_id, ordinal);

-- Span in the segment's stored text (INV-CO-04), in characters. `content_id` and
-- `document_id` are copies of the segment's, so the per-key rebuild reads one index.
CREATE TABLE identifier_mention (
    id             INTEGER PRIMARY KEY,
    segment_id     INTEGER NOT NULL REFERENCES segment (id) ON DELETE CASCADE,
    document_id    INTEGER NOT NULL,
    content_id     INTEGER NOT NULL,
    identifier_key TEXT NOT NULL,
    span_start     INTEGER NOT NULL,
    span_end       INTEGER NOT NULL CHECK (span_start < span_end),
    column_index   INTEGER,
    column_name    TEXT,
    raw            TEXT NOT NULL,
    confidence     REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    found_by       TEXT NOT NULL
) STRICT;
CREATE INDEX identifier_mention_key
    ON identifier_mention (identifier_key, content_id, document_id, confidence);
CREATE INDEX identifier_mention_segment ON identifier_mention (segment_id);

-- Jobs and work items (ADR-0006, domain §7.2).
CREATE TABLE job (
    id             INTEGER PRIMARY KEY,
    trigger        TEXT NOT NULL,
    stage          TEXT NOT NULL,
    state          TEXT NOT NULL,
    blocked_reason TEXT,
    created_at     TEXT NOT NULL
) STRICT;

CREATE TABLE work_item (
    id             INTEGER PRIMARY KEY,
    job_id         INTEGER NOT NULL REFERENCES job (id),
    content_id     INTEGER REFERENCES content (id) ON DELETE SET NULL,
    priority       INTEGER NOT NULL,
    state          TEXT NOT NULL,
    attempts       INTEGER NOT NULL DEFAULT 0,
    failure_code   TEXT,
    failure_reason TEXT
) STRICT;
CREATE INDEX work_item_state ON work_item (state, priority);
CREATE INDEX work_item_job ON work_item (job_id, state);
CREATE INDEX work_item_content ON work_item (content_id);

-- The decision log: append-only (INV-CU-01). Its states are derived (decision_status).
CREATE TABLE decision (
    seq        INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       TEXT NOT NULL,
    anchor     TEXT NOT NULL,
    before     TEXT,
    after      TEXT,
    decided_at TEXT NOT NULL
) STRICT;
CREATE INDEX decision_anchor ON decision (kind, anchor, seq);

CREATE TRIGGER decision_no_update BEFORE UPDATE ON decision
BEGIN
    SELECT RAISE(ABORT, 'the decision log is append-only');
END;

CREATE TRIGGER decision_no_delete BEFORE DELETE ON decision
BEGIN
    SELECT RAISE(ABORT, 'the decision log is append-only');
END;
