-- M4c: custom patterns (US-04.7) and column mapping (E05).
--
-- A project's custom patterns, in the order they were added. Names are unique ignoring case
-- (INV-ID-07). Their matches are `identifier_mention` rows with `found_by = 'custom'` and keys
-- `custom:<name>:<value>`; a change re-runs only that step (INV-ID-08).
CREATE TABLE custom_pattern (
    id         INTEGER PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    expression TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

-- A CSV or JSON file read as a table (BC-05): its header signature and column count. One per
-- top-level table document. Machine output: rewritten with the document's table step.
CREATE TABLE table_document (
    document_id  INTEGER PRIMARY KEY REFERENCES document (id) ON DELETE CASCADE,
    content_id   INTEGER NOT NULL,
    signature    TEXT NOT NULL,
    column_count INTEGER NOT NULL
) STRICT;
CREATE INDEX table_document_content ON table_document (content_id);
CREATE INDEX table_document_signature ON table_document (signature);

-- The suggested type of each column (US-05.1), with up to 5 sample values joined by U+001F.
-- Card-like numbers in the samples are masked: the number itself is stored only in segment
-- text (NFR-SEC-14). Columns past the first 1,000 have no row (ASSUMPTION M4c).
CREATE TABLE column_suggestion (
    document_id    INTEGER NOT NULL REFERENCES document (id) ON DELETE CASCADE,
    content_id     INTEGER NOT NULL,
    ordinal        INTEGER NOT NULL,
    column_key     TEXT NOT NULL,
    header         TEXT NOT NULL,
    suggested_type TEXT NOT NULL,
    share          REAL NOT NULL CHECK (share BETWEEN 0 AND 1),
    samples        TEXT NOT NULL,
    PRIMARY KEY (document_id, ordinal)
) STRICT;
CREATE INDEX column_suggestion_content ON column_suggestion (content_id);

-- An entity mention (domain §3.3): in M4c only from a confirmed person or organization column
-- (US-05.3, INV-CM-03). Span in the segment's stored text, in characters (INV-CO-04), inside
-- the cell or value. `column_key` is the mapping's column, `record` the row number (CSV) or
-- the pointer of the JSON record (INV-CM-05).
CREATE TABLE entity_mention (
    id           INTEGER PRIMARY KEY,
    segment_id   INTEGER NOT NULL REFERENCES segment (id) ON DELETE CASCADE,
    document_id  INTEGER NOT NULL,
    content_id   INTEGER NOT NULL,
    name_key     TEXT NOT NULL,
    entity_type  TEXT NOT NULL CHECK (entity_type IN ('person', 'org')),
    raw          TEXT NOT NULL,
    span_start   INTEGER NOT NULL,
    span_end     INTEGER NOT NULL CHECK (span_start < span_end),
    column_index INTEGER,
    column_name  TEXT,
    column_key   TEXT NOT NULL,
    record       TEXT NOT NULL,
    confidence   REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1),
    producer     TEXT NOT NULL
) STRICT;
CREATE INDEX entity_mention_key
    ON entity_mention (name_key, content_id, document_id, confidence);
CREATE INDEX entity_mention_content ON entity_mention (content_id, producer);
CREATE INDEX entity_mention_segment ON entity_mention (segment_id);

-- A record link (INV-CM-05): an entity and an identifier in the same CSV row or JSON record.
-- One per (document, record, entity, identifier).
CREATE TABLE record_link (
    id             INTEGER PRIMARY KEY,
    content_id     INTEGER NOT NULL,
    document_id    INTEGER NOT NULL REFERENCES document (id) ON DELETE CASCADE,
    segment_id     INTEGER NOT NULL REFERENCES segment (id) ON DELETE CASCADE,
    record         TEXT NOT NULL,
    name_key       TEXT NOT NULL,
    identifier_key TEXT NOT NULL
) STRICT;
CREATE INDEX record_link_content ON record_link (content_id);
CREATE INDEX record_link_name ON record_link (name_key);
CREATE INDEX record_link_identifier ON record_link (identifier_key);
CREATE INDEX record_link_segment ON record_link (segment_id);

-- `project_meta` keys written by the app (no schema change): `custom_rerun_after` (the last
-- content an unfinished custom-pattern re-run committed, INV-PR-05) and `custom_patterns_applied`
-- (a hash of the pattern set the stored custom mentions were made with; missing = no patterns).
