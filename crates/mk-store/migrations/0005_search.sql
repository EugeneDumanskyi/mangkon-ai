-- M4a: keyword search (US-06.1, ADR-0010). An FTS5 index over the stored text of every segment,
-- kept in the same transaction as the segments. It reads its text from `segment` (external
-- content), so the text is stored once.
--
-- New segments are indexed by the Stage A commit in one statement per content (`commit_stage_a`):
-- a trigger per inserted row made a 1M-row CSV commit 5x slower (spike S4). Deletes (a re-scan,
-- a removal, the cascade from `document`) go through the trigger below.
--
-- Not a derived table (ADR-0005): it indexes machine output, not decisions, so the derived
-- rebuild leaves it alone. `Store::rebuild_search_index` refills it ('rebuild'); the kill test
-- checks it ('integrity-check', NFR-REL-02).
--
-- Segments are replaced, never edited (INV-CO-06); the update trigger is there so a direct
-- edit cannot leave the index behind.
CREATE VIRTUAL TABLE segment_fts USING fts5(
    text,
    content = 'segment',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TRIGGER segment_fts_delete AFTER DELETE ON segment
BEGIN
    INSERT INTO segment_fts (segment_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;

CREATE TRIGGER segment_fts_update AFTER UPDATE OF text ON segment
BEGIN
    INSERT INTO segment_fts (segment_fts, rowid, text) VALUES ('delete', old.id, old.text);
    INSERT INTO segment_fts (rowid, text) VALUES (new.id, new.text);
END;

-- Projects from schema v4 already have segments.
INSERT INTO segment_fts (segment_fts) VALUES ('rebuild');
