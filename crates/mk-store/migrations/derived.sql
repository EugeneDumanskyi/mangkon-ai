-- Derived tables (ADR-0005). Never migrated: dropped, created from this file and rebuilt from
-- machine output + decisions after a migration, when this file changes (DERIVED_VERSION), and
-- on a full rebuild.

CREATE TABLE identifier (
    key           TEXT PRIMARY KEY,
    kind          TEXT NOT NULL,
    value         TEXT NOT NULL,
    mention_count INTEGER NOT NULL
) STRICT;

-- Entities (domain §9.3): one row per entity key, the smallest name key of a group or a split
-- group's `split:<hash>`. The display name is a user's, else the most frequent name as written, a
-- tie broken A→Z (INV-ER-07); `display_key` is that written name's key, the anchor of decisions.
CREATE TABLE entity (
    entity_key    TEXT PRIMARY KEY,
    entity_type   TEXT NOT NULL,
    display_name  TEXT NOT NULL,
    display_key   TEXT NOT NULL,
    note          TEXT,
    mention_count INTEGER NOT NULL,
    from_table    INTEGER NOT NULL
) STRICT;

-- Groups of two or more name keys (INV-ER-03). Depends only on decisions.
CREATE TABLE entity_member (
    name_key   TEXT PRIMARY KEY,
    entity_key TEXT NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX entity_member_entity ON entity_member (entity_key);

-- Every pair with a merge decision, and whether its newest decision accepts it.
CREATE TABLE merge_edge (
    anchor   TEXT PRIMARY KEY,
    a        TEXT NOT NULL,
    b        TEXT NOT NULL,
    accepted INTEGER NOT NULL
) STRICT, WITHOUT ROWID;
CREATE INDEX merge_edge_a ON merge_edge (a);
CREATE INDEX merge_edge_b ON merge_edge (b);

-- The mention keys of each split, from its newest split decision. `active`: it was not undone.
-- `present`: a stored mention has this key.
CREATE TABLE split_anchor (
    split_key    TEXT NOT NULL,
    mention_key  TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    seq          INTEGER NOT NULL,
    active       INTEGER NOT NULL,
    present      INTEGER NOT NULL,
    PRIMARY KEY (split_key, mention_key)
) STRICT, WITHOUT ROWID;
CREATE INDEX split_anchor_mention ON split_anchor (mention_key);
CREATE INDEX split_anchor_content ON split_anchor (content_hash);

-- Mentions moved out of their name key's entity (INV-ER-01), by the newest active split.
CREATE TABLE split_mention (
    mention_id INTEGER PRIMARY KEY,
    split_key  TEXT NOT NULL,
    name_key   TEXT NOT NULL,
    content_id INTEGER NOT NULL
) STRICT;
CREATE INDEX split_mention_split ON split_mention (split_key);
CREATE INDEX split_mention_name ON split_mention (name_key);
CREATE INDEX split_mention_content ON split_mention (content_id);

-- Pending merge proposals (US-11.2). May lag behind mentions (ADR-0005); never ahead of decisions,
-- which drop what they end. `a` and `b` are name keys, `left_key` and `right_key` entity keys.
CREATE TABLE merge_proposal (
    a         TEXT NOT NULL,
    b         TEXT NOT NULL,
    left_key  TEXT NOT NULL,
    right_key TEXT NOT NULL,
    score     REAL NOT NULL,
    reasons   TEXT NOT NULL,
    PRIMARY KEY (a, b)
) STRICT, WITHOUT ROWID;
CREATE INDEX merge_proposal_rank ON merge_proposal (score DESC, a, b);
CREATE INDEX merge_proposal_left ON merge_proposal (left_key);
CREATE INDEX merge_proposal_right ON merge_proposal (right_key);

CREATE TABLE connection (
    subject_key     TEXT PRIMARY KEY,
    subject_kind    TEXT NOT NULL,
    source_count    INTEGER NOT NULL,
    file_type_count INTEGER NOT NULL,
    mention_count   INTEGER NOT NULL,
    low_confidence  INTEGER NOT NULL,
    ignored         INTEGER NOT NULL,
    -- M6a: exists only because of mentions under 0.5, hidden by default (INV-AI-05).
    low_only        INTEGER NOT NULL,
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
