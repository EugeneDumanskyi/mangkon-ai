-- Derived tables (ADR-0005). Never migrated: dropped, created from this file and rebuilt from
-- machine output + decisions after a migration, when this file changes (DERIVED_VERSION), and
-- on a full rebuild.

CREATE TABLE identifier (
    key           TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    value         TEXT NOT NULL,
    mention_count INTEGER NOT NULL
) STRICT;

CREATE TABLE connection (
    subject_key     TEXT PRIMARY KEY,
    subject_kind    TEXT NOT NULL,
    source_count    INTEGER NOT NULL,
    file_type_count INTEGER NOT NULL,
    mention_count   INTEGER NOT NULL,
    low_confidence  INTEGER NOT NULL,
    ignored         INTEGER NOT NULL
) STRICT;
CREATE INDEX connection_rank ON connection (
    low_confidence, source_count DESC, file_type_count DESC, mention_count DESC, subject_key
);

CREATE TABLE decision_status (
    seq    INTEGER PRIMARY KEY,
    anchor TEXT NOT NULL,
    status TEXT NOT NULL
) STRICT;
CREATE INDEX decision_status_anchor ON decision_status (anchor);
