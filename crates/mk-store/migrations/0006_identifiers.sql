-- M4b: built-in identifiers (US-04.2 – US-04.6).
--
-- Card-like numbers (US-04.6, INV-ID-06): never identifiers, only a flag on their document with
-- the place and the last four digits. The number itself is not stored anywhere outside the
-- segment text (NFR-SEC-14). A flag goes with its segment (INV-CO-06): re-parsing a content
-- deletes its documents, and the cascade takes the flags.
CREATE TABLE sensitive_flag (
    id           INTEGER PRIMARY KEY,
    segment_id   INTEGER NOT NULL REFERENCES segment (id) ON DELETE CASCADE,
    document_id  INTEGER NOT NULL,
    content_id   INTEGER NOT NULL,
    span_start   INTEGER NOT NULL,
    span_end     INTEGER NOT NULL CHECK (span_start < span_end),
    column_index INTEGER,
    column_name  TEXT,
    last4        TEXT NOT NULL CHECK (length(last4) = 4 AND last4 NOT GLOB '*[^0-9]*')
) STRICT;
CREATE INDEX sensitive_flag_document ON sensitive_flag (document_id);
CREATE INDEX sensitive_flag_segment ON sensitive_flag (segment_id);
CREATE INDEX sensitive_flag_content ON sensitive_flag (content_id);

-- An identifier re-run replaces one content's built-in mentions at a time.
CREATE INDEX identifier_mention_content ON identifier_mention (content_id, found_by);

-- `project_meta` keys written by the app (no schema change): `identifier_version` (the
-- identifier rules the stored mentions were made with, `mk_identifiers::RULES_VERSION`; missing
-- in a project from before M4b) and `identifier_rerun_after` (the last content an unfinished
-- identifier re-run committed, so a killed re-run resumes after it, INV-PR-05).
