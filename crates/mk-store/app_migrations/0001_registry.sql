-- App database v1 (M3c, architecture §6, domain §5.2): the projects the app knows.
-- `name_key` is the name case-folded, so the database keeps names unique ignoring case
-- (INV-WS-01). `path` is the project folder, absolute. `last_opened_at` in UTC milliseconds.
CREATE TABLE registry_entry (
    id             INTEGER PRIMARY KEY,
    name           TEXT NOT NULL,
    name_key       TEXT NOT NULL UNIQUE,
    path           TEXT NOT NULL UNIQUE,
    last_opened_at INTEGER NOT NULL
) STRICT;
