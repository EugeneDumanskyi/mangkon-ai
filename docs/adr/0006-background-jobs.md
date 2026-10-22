# ADR-0006: Background jobs — persistent queue in the project store

- **Status:** Proposed
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
- Restart test suite: kill during each job state, check NFR-REL-04.
- Decide with the maintainer whether only one project can be open at a time (gap A7). This ADR assumes yes.
