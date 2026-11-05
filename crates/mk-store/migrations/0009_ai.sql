-- M6a: AI extraction (BC-06, ADR-0005, ADR-0006, domain §5.1).
--
-- A run of one stage with one model. `model_identity` is what makes two runs "the same model"
-- (`mk_ai::RunIdentity`). One active and one pending per stage. A first run is `active` from the
-- start, so people appear as they are found; a re-run is `pending` until its switch.
CREATE TABLE ai_run (
    id                 INTEGER PRIMARY KEY,
    stage              TEXT NOT NULL,
    model_identity     TEXT NOT NULL,
    model_name         TEXT NOT NULL,
    destination_host   TEXT NOT NULL,
    destination_class  TEXT NOT NULL,
    prompt_version     INTEGER NOT NULL,
    state              TEXT NOT NULL CHECK (state IN ('pending', 'active', 'superseded')),
    mention_count      INTEGER NOT NULL DEFAULT 0,
    discarded          INTEGER NOT NULL DEFAULT 0,
    invalid_responses  INTEGER NOT NULL DEFAULT 0,
    identifier_dropped INTEGER NOT NULL DEFAULT 0,
    started_at         TEXT NOT NULL,
    finished_at        TEXT
) STRICT;
CREATE UNIQUE INDEX ai_run_one_active ON ai_run (stage) WHERE state = 'active';
CREATE UNIQUE INDEX ai_run_one_pending ON ai_run (stage) WHERE state = 'pending';

-- A re-run's mentions until its switch. Kept apart from `entity_mention`, which only ever holds
-- what is visible, so no read of it needs a filter and a forgotten filter cannot mix two models.
CREATE TABLE pending_entity_mention (
    id          INTEGER PRIMARY KEY,
    ai_run_id   INTEGER NOT NULL REFERENCES ai_run (id) ON DELETE CASCADE,
    segment_id  INTEGER NOT NULL REFERENCES segment (id) ON DELETE CASCADE,
    document_id INTEGER NOT NULL,
    content_id  INTEGER NOT NULL,
    name_key    TEXT NOT NULL,
    entity_type TEXT NOT NULL CHECK (entity_type IN ('person', 'org')),
    raw         TEXT NOT NULL,
    span_start  INTEGER NOT NULL,
    span_end    INTEGER NOT NULL CHECK (span_start < span_end),
    confidence  REAL NOT NULL CHECK (confidence BETWEEN 0 AND 1)
) STRICT;
CREATE INDEX pending_entity_mention_run ON pending_entity_mention (ai_run_id);
CREATE INDEX pending_entity_mention_segment ON pending_entity_mention (segment_id);

-- The run that made an AI mention (INV-AI-02); NULL for table mentions.
ALTER TABLE entity_mention ADD COLUMN ai_run_id INTEGER REFERENCES ai_run (id);
CREATE INDEX entity_mention_run ON entity_mention (ai_run_id) WHERE ai_run_id IS NOT NULL;

-- The run a Stage B job works for. NULL until a model is bound (it may be Blocked for want of one).
ALTER TABLE job ADD COLUMN ai_run_id INTEGER REFERENCES ai_run (id) ON DELETE SET NULL;

-- A Stage B item's target: the segments `segment_first..=segment_last` of one document (a unit,
-- decided 2026-11-05), or a window `char_start..char_end` of the one segment for a long segment.
-- When the segments go, `segment_first` becomes NULL: the item has no target. Stage A items have
-- all four NULL and are not in the index.
ALTER TABLE work_item ADD COLUMN segment_first INTEGER REFERENCES segment (id) ON DELETE SET NULL;
ALTER TABLE work_item ADD COLUMN segment_last INTEGER;
ALTER TABLE work_item ADD COLUMN char_start INTEGER;
ALTER TABLE work_item ADD COLUMN char_end INTEGER;
CREATE INDEX work_item_segment ON work_item (segment_first, segment_last)
    WHERE segment_first IS NOT NULL;

-- `project_meta` key written by the app (no schema change): `ai_waiting_units`, the units Stage B
-- would do, over the start rule's limit, while the user has not picked (gap A3).
