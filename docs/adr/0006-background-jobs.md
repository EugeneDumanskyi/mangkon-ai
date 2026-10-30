# ADR-0006: Background jobs — persistent queue in the project store

- **Status:** Accepted
- **Date:** 2026-10-12
- **Related:** INV-PR-01..10, NFR-REL-03/04/05, NFR-PERF-25/33, NFR-RES-02, domain §7.1, §7.2, ADR-0004, ADR-0007

## Context

- Jobs and work items must survive restarts. A running job comes back running. A paused job stays paused (INV-PR-07, NFR-REL-04).
- A finished work item is never redone (INV-PR-05). Only items in progress at a crash are redone.
- Pause must take effect in ≤ 5 s (NFR-REL-05). Pause is project-wide (domain D11).
- Order rules: Stage A before B/C on the same content (INV-PR-01); B before C on a segment (INV-PR-02); text-native before OCR (INV-PR-03).
- An AI item starts only when the egress gate allows it. Otherwise the job is Blocked, and Layer 1 work goes on (INV-PR-09).
- Stage A uses up to cores − 1 workers; Stage B uses one request per model (NFR-PERF-25).

## Options considered

**1. In-memory queue (Tokio tasks). Re-create work on start by comparing data.**
- ➕ Simple.
- ➖ Cannot restore "paused" or "cancelled" states or attempt counts. Breaks INV-PR-07.

**2. Persistent `job` and `work_item` tables in `project.db`, scheduler in the core.**
- ➕ Job state commits in the **same transaction** as the work item's results. "Done" and "results stored" can never disagree.
- ➕ Full control over order, pause, block, and priorities.
- ➖ We write the scheduler ourselves (a few hundred lines).

**3. A job-queue library with a SQLite backend (for example `apalis`).**
- ➕ Less code to write.
- ➖ Its own schema and transactions. The results commit and the job commit become two steps.
- ➖ Our state machines (Blocked, project-wide pause, stage ordering) do not map cleanly.

## Decision

Use **option 2**.

**Data**
- Tables `job` and `work_item` follow domain §5.1 and the state machines in §7.2.
- A work item's results and its state change to `Done` commit in **one** transaction (INV-PR-04).

**Scheduler (in the core)**
- **Lanes**, each with its own limit:
  - *Stage A lane* — parser workers. Limit = `min(cores − 1, memory budget)` (ADR-0007).
  - *AI extraction lane* — 1 request in flight per loaded model.
  - *Embeddings lane* — 1 request in flight per loaded model.
  - *Light lane* — custom-pattern re-runs and other pure-Rust work on stored text, on a small thread pool in the core.
- **Order:** Stage A items with text-native content first, then OCR content. AI items for a content only after its Stage A is Done. Stage C for a segment only after its Stage B is Done.
- **Blocked:** before an AI job takes items, it asks the egress gate (ADR-0009) and the model supervisor (ADR-0008). If either says no, the job becomes Blocked with the reason. Other lanes keep going.

**Pause, cancel, restart**
- **Pause:** stop starting new items. In-progress Stage A items get up to 5 s to finish. Then the worker is killed and the item goes back to Pending (its transaction is rolled back). In-progress AI requests are aborted and go back to Pending.
- **Cancel:** like pause, then items become Cancelled and contents become NotProcessed (domain D10).
- **Restart:** on project open, every `InProgress` item becomes `Pending`. Job states are kept as they were.

**Retries and failures**
- A failed item records a plain reason (NFR-USE-05). No automatic retry for parse failures (a broken file stays broken). The user can retry a file.
- AI calls: one retry for an invalid response (INV-AI-03). Network errors: up to 3 retries with backoff, then the job becomes Blocked with the reason.

**Progress**
- The progress panel reads counts from the tables. ETA after 5 finished items (domain §7.2).

## Consequences

**Easier**
- Restart and kill behaviour is easy to test and to reason about.
- One place decides what runs next. Easy to add new stages later (Q&A, export).

**Harder**
- We own the scheduler code and its tests.
- `work_item` can grow large (1M segments → 1M Stage B items). Finished items should be compacted into per-job counts after the job completes. ASSUMPTION.

**Follow-ups**
- Restart test suite: kill during each job state, check NFR-REL-04. **Partly done in M2b:** `crates/mk-core/tests/restart.rs` drops the project with one item In progress, one Done and a Paused job; on open the item is Pending and its content Queued, the Done item is not redone, the running job stays running and the paused job stays paused (INV-PR-05/07). Real process kills come with the M2c kill test.
- **Done in M2b** (`crates/mk-jobs`, `crates/mk-store`, `crates/mk-core`): `job` and `work_item` tables with every §7.2 state and checked transitions; an item's results and its `Done` state commit in one transaction (INV-PR-04); the Stage A lane runs `cores − 1` threads in process (workers come in M2c), text-native first (INV-PR-03); a file that changed since the scan fails alone with "File changed during processing". Plain threads, not an async runtime, until IPC (M2d). Pause, cancel and ETA come in M3.
- Decide with the maintainer whether only one project can be open at a time (gap A7). This ADR assumes yes. **Decided (2026-10-28):** yes, one at a time. M2d: the desktop runs `Project::ingest` on one background thread per open project; a folder added during a run makes the run go again; opening or closing a project is refused while it is being ingested (pause and cancel come in M3); items a killed run left Pending are run when the project is opened again.
- **Done in M3c** (`crates/mk-core`, `crates/mk-store`, `apps/desktop`): pause, resume, cancel and the ETA. Pause is project-wide (D11). A busy item gets a 4 s grace, then its worker is stopped and the item goes back to Pending. This keeps the whole pause within the 5 s of NFR-REL-05 and inside this ADR's "up to 5 s". The domain adds Queued → Paused, so a pause also covers jobs that have not started and a job made while paused starts Paused. Cancel ends the open items as Cancelled and their contents as Not processed (D10). The ETA is bytes left at this run's rate, after 5 finished items. Opening, closing or switching projects no longer waits for the run: it stops the run and keeps job states (US-03.4 AC1), and on open a run starts only if a job is Queued or Running. Measured pause times: `docs/11-ingestion.md` §8.
