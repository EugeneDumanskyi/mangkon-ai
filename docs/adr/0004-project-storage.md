# ADR-0004: Storage — one SQLite file per project, WAL, single writer

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-REL-02/03/09/11, NFR-CAP-01/03/08, NFR-COMP-07, NFR-MNT-07, NFR-PERF-30, INV-CO-06, INV-CU-01/06, INV-PR-04, INV-WS-03/04, domain §2.6 and §2.8, ADR-0005, ADR-0010

## Context

- A work item must commit all results or none (INV-PR-04). A forced kill must never corrupt a project (NFR-REL-02).
- Re-parsed content replaces old documents and segments in one step (INV-CO-06).
- The decision log is the only user-owned truth. It must survive re-runs, model changes, and upgrades (INV-CU-01/06, NFR-REL-09).
- A project folder must work when copied to another OS (NFR-COMP-07).
- Scale: 10,000 files, up to 2,000,000 segments (NFR-CAP-01/03).
- The domain has two scopes: app-level and project-level (domain §2.8).
- No database server: users build from source and run a desktop app (NFR-MNT-01).

## Options considered

**1. One SQLite file per project (+ one app-level SQLite file).**
- ➕ One ACID transaction covers everything a work item writes.
- ➕ WAL mode: readers never block the writer. Safe on app crash.
- ➕ A project is one file: easy to copy, back up, and migrate.
- ➕ FTS5 is built in (keyword search, ADR-0010).
- ➖ Only one writer at a time. Fine for one user, but writes must be short.

**2. One SQLite file for all projects.**
- ➕ One connection.
- ➖ Projects cannot be copied or backed up alone (NFR-COMP-07). A bad migration hits every project.

**3. Several stores (SQLite + Tantivy + LanceDB).**
- ➕ Best engine per job.
- ➖ No shared transaction. A crash between commits leaves stores out of sync. Needs repair logic. (Option C in ADR-0001.)

**4. Embedded key-value store (redb, RocksDB) with our own indexes.**
- ➕ Fast writes.
- ➖ We would build query, join, and full-text features ourselves. Too much work.

## Decision

Use **option 1**.

**Engine and settings**
- SQLite compiled into the app (`rusqlite` with the `bundled` feature). Same version and FTS5 on every OS.
- `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`.
  - With WAL, `NORMAL` cannot corrupt the database on an app crash or a forced kill. A power loss may lose the last few commits, but never corrupts. This meets NFR-REL-02/03.
- **One writer thread** in the core owns the only write connection. Other code sends write jobs to it through a channel. **Four read connections** serve UI queries.

**Files** (layout in `05-architecture.md` §6)
- `<project>/project.db` — all project-level data.
- `<app-data>/mangkon-ai/app.db` — project registry, endpoints (without keys), model installations, hardware profile.

**Data rules**
- **Machine output** (documents, segments, mentions, AI runs) and **derived tables** (identifiers, entities, connections, proposals) are separate table groups. Derived tables can be dropped and rebuilt at any time (ADR-0005).
- **Decision log** tables are append-only. SQLite triggers reject `UPDATE` and `DELETE` on them. A decision's state (Active / Superseded / Orphaned, domain 7.5) is computed into a separate derived table. So the log itself never changes.
- Source paths are stored as source root (absolute, relocatable) + relative path (domain D12).
- No secrets are ever stored (ADR-0012).

**Migrations**
- Forward-only, numbered SQL files, embedded in the binary (NFR-MNT-07).
- Before a migration: `VACUUM INTO backups/project-v<old>-<date>.db` (INV-WS-04). If the migration fails, the project opens from the backup.
- A newer schema than the app → the project is not opened and not changed (INV-WS-03).
- Derived tables are **not** migrated. They are dropped and rebuilt after the migration. This keeps migrations small.
- Every migration is tested in CI on a copy of TD-2 (NFR-MNT-07).

## Consequences

**Easier**
- Crash safety comes mostly from SQLite. Kill tests check our use of it, not our own storage engine.
- Backup and portability are "copy one file".
- The decision log is protected by the database itself, not only by code review.

**Harder**
- Writes are serialized. Big writes (a 1M-row CSV) hold the writer for seconds. Other commits wait. Acceptable for v0.1. Keep each transaction to one work item.
- Database size must be watched (NFR-CAP-08). FTS5 and stored segment text roughly double the text size.
- Vectors in SQLite are fine as storage, but not fast enough to search directly at 1M segments. ADR-0010 adds a derived index file.

**Follow-ups**
- Kill test harness in M2: 20 random forced kills during TD-1 ingestion (NFR-REL-02).
- Measure `project.db` size on TD-2 against NFR-CAP-08.
