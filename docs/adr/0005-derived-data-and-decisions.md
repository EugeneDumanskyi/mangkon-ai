# ADR-0005: Derived data and decisions — rebuild in the same commit

- **Status:** Accepted
- **Date:** 2026-10-12
- **Related:** INV-CU-03/04/05, INV-LK-05, INV-AI-04, INV-CO-06, INV-ER-01..08, NFR-PERF-34, NFR-REL-09, NFR-QA-10, domain §2.4, §9.3, §9.6, ADR-0004

## Context

- Identifiers, entities, aliases, connections, and merge proposals are **derived** (domain §2.3, §2.4). They are never edited directly (INV-LK-05).
- After any re-run, all active decisions must be applied **before** results are shown (INV-CU-03). A user must never see results without their decisions.
- The connections screen must update ≤ 2 s after a file finishes (NFR-PERF-34).
- A re-run with a new model replaces all AI mentions of that stage (INV-AI-04).
- Domain §9.6 gives the order: replace output → rebuild entities → re-apply decisions → recompute connections → show.

## Options considered

**1. Compute on read (SQL views).**
- ➕ Never stale. No extra tables.
- ➖ Grouping 2M mentions on every screen load is too slow (NFR-PERF-02/04).

**2. Background projector (eventual consistency).** Commit machine output, then a separate task updates derived tables later.
- ➕ Small, fast commits.
- ➖ For a short time the UI can show new results **without** decisions applied. This breaks INV-CU-03.
- ➖ Needs an outbox and catch-up logic after a crash.

**3. Incremental rebuild in the same transaction + staged switch for big re-runs.**
- ➕ Readers only ever see a consistent state: output, derived data, and decisions together.
- ➕ No outbox. A crash simply rolls back the whole transaction.
- ➖ Write transactions are bigger. The per-key rebuild must be fast.

## Decision

Use **option 3**.

**Per work item (normal case)**
1. Write the extraction result (mentions, record links, embeddings).
2. Collect the **touched keys**: identifier keys and name keys that gained or lost mentions.
3. For those keys only: rebuild the identifier rows, regroup entities (§9.3 steps 1–5), apply active decisions whose anchors match (§9.5), and recompute connection rows and their marks (§9.1, §9.2).
4. Commit everything in **one** transaction.
5. After commit, emit a notification (`ConnectionsChanged`, `EntitiesRegrouped`). Debounced 500 ms. The UI re-queries.

**Merge proposals**
- Proposals are suggestions, not shown results, so they may lag. A background task recomputes them (debounced) after entity changes.
- Rejected pairs are filtered when a proposal is **created** (INV-ER-04). A proposal never appears for a rejected pair, even for a moment.

**Big re-runs (new model, app upgrade, stop-list change, rebuild)**
1. Create a new AI run in state **pending**. Its mentions are written with its run ID. Derived tables ignore pending runs. The user keeps seeing the old results.
2. When the run finishes, one **switch** transaction:
   - marks the old run superseded and the new run active,
   - rebuilds all derived tables from scratch,
   - re-applies all active decisions and marks orphans (INV-CU-04).
3. If the run is cancelled, its pending mentions are deleted. Nothing visible changed.
4. ASSUMPTION: old results stay visible until the new run is done (gap A4 in `05-architecture.md`). Two models' output is never mixed.

**Decision application**
- Decisions are indexed by `(kind, anchor)`. For each anchor, the newest decision wins (INV-CU-05).
- The decision log is read-only for this code (ADR-0004). Only the derived `decision_status` table changes.

**Full rebuild command**
- `mangkon-cli rebuild` (and a hidden menu item) drops every derived table and rebuilds it from machine output + decisions. It is also used after schema migrations (ADR-0004).

## Consequences

**Easier**
- INV-CU-03 holds by design, not by timing.
- Crash recovery needs no extra logic for derived data.
- Model switches are safe: the user sees either all-old or all-new results.

**Harder**
- Each commit does more work. Lookups by key need good indexes (name key, identifier key, anchor).
- A big switch transaction on a large project may take several seconds. Show "Applying new results…" while it runs.
- A pending run doubles AI mention storage for a while.

**Follow-ups**
- **Decided and done in M6a** (gap A4, roadmap §4 #12, 2026-11-05): old results stay until the new run is done, then one switch; Cancel changes nothing visible. A first run is `active` from the start, so people appear as they are found. A re-run is `pending`, and its output lives in its own table, `pending_entity_mention`, not in `entity_mention` with a run filter: every existing read stays right without a change, and a forgotten filter cannot mix two models. The switch hangs off the end of the job, so every path that finishes one triggers it (commit, fail, skip, resume, unblock). A re-run with no Done unit is dropped and the old results stay. Tested at every commit of a re-run and over random sequences (`adr_0005_*`), and in 10 kills (`apps/cli/tests/ai_kill.rs`). Results: `14-ai-extraction.md`.
- Load test in M2 (spike S2c): 10,000 decisions + TD-2, measure commit time (risk R7). **Done in M2b** (`10-walking-skeleton.md` §S2c): at 10,000 documents and ~297,000 mentions, p95 "parse finished → commit done" through the 23-thread lane is 601 ms with 0 decisions and 736 ms with 10,000 (1.10 / 1.24 s with the 500 ms debounce, against 2 s). Most of it is waiting for the writer; a commit alone takes 13–16 ms for three keys in ~9,500 documents each and 81–97 ms for a 5,000-row CSV. One item per transaction holds; no batching or extra index needed. R7 retired.
- Property test: "rebuild from scratch" equals "incremental updates" for random sequences of results and decisions. **Done in M2b:** `crates/mk-core/tests/derived_property.rs` (proptest, 48 sequences of up to 24 steps through the real pipeline: new, changed, missing, found-again and duplicate files, re-scans, Stage A runs, ignore and un-ignore, orphaned decisions). After every step the stored rows equal the `mk-graph` reference model; at the end they equal `rebuild_all`. Checked by mutation: dropping a touched key makes it fail.
- **Done in M2b** (scope): the per-key rebuild covers `identifier`, `connection` and `decision_status` for email keys, in the same transaction as the Stage A result, the re-scan or the decision. Entities (step 3) have no inputs before M7. The `ConnectionsChanged` event (step 5) is raised when a commit changes a connection row, debounced 500 ms (`mk-core`).
