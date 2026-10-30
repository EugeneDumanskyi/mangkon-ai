-- M3c: projects, files and the queue (US-01.1/01.5/01.6, US-02.7, US-03.1–03.4).
--
-- `project_meta` keys written by the app (no schema change): `name` (the project name, so a
-- project opened from a folder registers under it), `phone_region` and `domain_stop_list`
-- (settings, INV-WS-05; used from M4), `app_version` (the app that created or last migrated the
-- project, named when a too-new project is refused, US-01.2 AC3) and `queue_paused` ('1' while
-- the queue is paused: jobs made meanwhile start Paused, INV-PR-06).

-- Paths removed from the project inside a folder root (US-01.5, ASSUMPTION M3c): re-scan does
-- not add them again. Adding the path again removes its exclusion.
CREATE TABLE root_exclusion (
    root_id  INTEGER NOT NULL REFERENCES source_root (id),
    rel_path TEXT NOT NULL,
    PRIMARY KEY (root_id, rel_path)
) STRICT;

-- The technical text behind "Show details" (US-02.7 AC2).
ALTER TABLE content ADD COLUMN failure_detail TEXT;

-- UTC milliseconds since the Unix epoch: when a work item started and finished (the ETA,
-- US-03.1 AC2) and when a job was paused.
ALTER TABLE work_item ADD COLUMN started_at INTEGER;
ALTER TABLE work_item ADD COLUMN finished_at INTEGER;
ALTER TABLE job ADD COLUMN paused_at INTEGER;
CREATE INDEX work_item_finished ON work_item (finished_at) WHERE finished_at IS NOT NULL;
