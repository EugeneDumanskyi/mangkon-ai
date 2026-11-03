-- M5a: the project's egress policy (BC-11, ADR-0009, domain §7.6). Not derived: a rebuild
-- never touches these tables, and they live as long as the project.
--
-- A permission for one app-level endpoint, with the origin (`scheme://host:port`) and class the
-- user confirmed. No row means Not allowed (INV-EG-01): a new project has none. A revoke
-- deletes the row. "This machine" destinations never get a row (they need no permission).
CREATE TABLE endpoint_permission (
    endpoint_id      TEXT PRIMARY KEY,
    confirmed_origin TEXT NOT NULL,
    confirmed_class  TEXT NOT NULL CHECK (confirmed_class IN ('local_network', 'internet')),
    state            TEXT NOT NULL CHECK (state IN ('allowed', 'needs_reconfirmation')),
    reason           TEXT,
    confirmed_at     TEXT NOT NULL,
    updated_at       TEXT NOT NULL
) STRICT;

-- Every destination that received project data, per purpose, with the first and last time
-- (INV-EG-05, US-09.8 AC2). Written before each send. "This machine" too.
CREATE TABLE egress_record (
    host          TEXT NOT NULL,
    class         TEXT NOT NULL CHECK (class IN ('this_machine', 'local_network', 'internet')),
    purpose       TEXT NOT NULL CHECK (purpose IN ('extraction', 'embeddings', 'search_query')),
    first_sent_at TEXT NOT NULL,
    last_sent_at  TEXT NOT NULL,
    PRIMARY KEY (host, class, purpose)
) STRICT, WITHOUT ROWID;
