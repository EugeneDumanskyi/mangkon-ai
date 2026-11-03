-- Derived tables (ADR-0005). Never migrated: dropped, created from this file and rebuilt from
-- machine output + decisions after a migration, when this file changes (DERIVED_VERSION), and
-- on a full rebuild.

CREATE TABLE identifier (
    key           TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    value         TEXT NOT NULL,
    mention_count INTEGER NOT NULL
) STRICT;

-- Entities (domain §9.3 steps 1, 2, 4): grouped by name key; the display name is the most
-- frequent name as written, a tie broken A→Z (INV-ER-07). M4c: table names only.
CREATE TABLE entity (
    name_key      TEXT PRIMARY KEY,
    entity_type   TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    mention_count INTEGER NOT NULL,
    from_table    INTEGER NOT NULL
) STRICT;

CREATE TABLE connection (
    subject_key     TEXT PRIMARY KEY,
    subject_kind    TEXT NOT NULL,
    source_count    INTEGER NOT NULL,
    file_type_count INTEGER NOT NULL,
    mention_count   INTEGER NOT NULL,
    low_confidence  INTEGER NOT NULL,
    ignored         INTEGER NOT NULL,
    -- M4d, for the Connections screen without a join (US-07.1, US-07.2): the type name, the
    -- value as shown (an entity's display name; a custom pattern's value without its label)
    -- and the counted file types, sorted, as ',csv,eml,pdf,'.
    kind            TEXT NOT NULL,
    value           TEXT NOT NULL,
    file_types      TEXT NOT NULL
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
