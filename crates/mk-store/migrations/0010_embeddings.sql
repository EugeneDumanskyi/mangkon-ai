-- M6b: meaning search (BC-10, ADR-0010, US-06.2).
--
-- One vector per window of at most 500 characters of a segment's stored text (`mk_search::meaning`,
-- decided 2026-11-05). `vector` is little-endian half floats, normalized, the source of truth; the
-- index file under `index/` is derived from it. A vector reaches its model through its run, so
-- reads go by run id and two models never meet (INV-SR-02, ADR-0005).
CREATE TABLE segment_embedding (
    id         INTEGER PRIMARY KEY,
    ai_run_id  INTEGER NOT NULL REFERENCES ai_run (id) ON DELETE CASCADE,
    segment_id INTEGER NOT NULL REFERENCES segment (id) ON DELETE CASCADE,
    char_start INTEGER NOT NULL,
    char_end   INTEGER NOT NULL CHECK (char_start < char_end),
    vector     BLOB NOT NULL
) STRICT;
CREATE INDEX segment_embedding_run ON segment_embedding (ai_run_id);
CREATE INDEX segment_embedding_segment ON segment_embedding (segment_id);

-- The size of a run's vectors, from its first reply.
ALTER TABLE ai_run ADD COLUMN dims INTEGER;

-- Changes when the active run's vectors change, on every path, cascades from a removed segment
-- included. The index file's header holds the value it was built from: a difference means rebuild.
-- A pending run's vectors do not touch it, so a cancelled re-run leaves the index good.
INSERT OR IGNORE INTO project_meta (key, value) VALUES ('embedding_generation', '0');
CREATE TRIGGER segment_embedding_added AFTER INSERT ON segment_embedding
WHEN NEW.ai_run_id = (SELECT id FROM ai_run WHERE stage = 'embeddings' AND state = 'active')
BEGIN
    UPDATE project_meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'embedding_generation';
END;
CREATE TRIGGER segment_embedding_removed AFTER DELETE ON segment_embedding
WHEN OLD.ai_run_id = (SELECT id FROM ai_run WHERE stage = 'embeddings' AND state = 'active')
BEGIN
    UPDATE project_meta SET value = CAST(value AS INTEGER) + 1 WHERE key = 'embedding_generation';
END;

-- `project_meta` key written by the app (no schema change): `embedding_waiting_windows`, the
-- windows over the start rule's limit while the user has not started them.
