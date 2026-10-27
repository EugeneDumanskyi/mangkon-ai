# ADR-0001: Architecture style — modular monolith with isolated parser workers

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-REL-01/02/03/06/07, NFR-PERF-03/30/31, NFR-RES-01/02/07, NFR-SEC-06/10, NFR-USE-03, `05-architecture.md` §2

## Context

Mangkon AI reads messy folders. Some files are broken, huge, or hostile (TD-3). The NFRs say:

- One bad file never stops the batch (NFR-REL-01).
- Each file has a time limit (NFR-REL-06) and a 1 GB memory limit (NFR-REL-07).
- A forced kill never corrupts a project (NFR-REL-02). Work lost is at most the files in progress (NFR-REL-03).
- The UI never blocks for more than 100 ms (NFR-PERF-03).

There is also a fixed release date (brief). Every extra moving part costs time.

Tauri, Rust, and `llama-server` are already fixed by the brief and NFRs. The open question is **process layout and storage**.

## Options considered

**A. Single-process monolith.** All Rust code in the Tauri main process. Parsers and OCR on a thread pool. One SQLite file per project.
- ➕ Least code. Least memory. One process to debug.
- ➖ A stuck parser thread cannot be stopped from outside. Rust has no safe way to kill a thread.
- ➖ Memory per file cannot be limited inside one process.
- ➖ A crash in native code (PDF or OCR library) kills the whole app, including the UI.
- ➖ So NFR-REL-01/06/07 cannot be met.

**B. Modular monolith + isolated parser workers.** One Rust core process holds all domain logic and is the only DB writer. Parsing and OCR run in child processes (same binary, `--worker` mode). One SQLite file per project.
- ➕ The core kills a worker on timeout or memory limit. Only that file fails.
- ➕ Workers never touch the database, so crash safety stays simple.
- ➕ Heavy CPU work runs outside the core, so the UI stays responsive.
- ➖ About one extra week of work for the worker protocol and supervision. ASSUMPTION.
- ➖ More memory: each worker has its own base memory.
- ➖ Large results must be serialized between processes.

**C. Headless engine service + specialised stores.** The engine runs as a local daemon with a localhost API. The Tauri app is a thin client. SQLite + Tantivy + LanceDB.
- ➕ Fastest search. Ready for a CLI, scripts, or remote UI.
- ➖ Three stores cannot commit together. Crash safety needs repair logic.
- ➖ A local API port is a new attack surface. It needs a token and origin checks.
- ➖ Arrow/LanceDB dependencies add build time and binary size (NFR-USE-03, NFR-RES-07).
- ➖ The most work.

The full NFR-by-NFR comparison is in `05-architecture.md` §2.4.

## Decision

Use **Option B**.

- **UI** — React in the Tauri webview. It talks only to the core, through Tauri IPC.
- **Core** — the Tauri main process. It holds all domain logic, the job scheduler, the egress gate, and the model supervisor. It is the **only writer** to the project store.
- **Parser workers** — a small pool of child processes started from the same binary with `--worker`. Each one does Stage A for one source content at a time and returns the result as a stream of messages. Details in ADR-0007.
- **Local model runtime** — `llama-server` sidecar processes. Details in ADR-0008.
- **Storage** — one SQLite file per project, plus derived index files. Details in ADR-0004 and ADR-0010.
- Modules inside the core talk through plain function calls. Domain events (domain §8) go through an in-process event bus. They are **notifications**, not data carriers. The database is the truth.

## Consequences

**Easier**
- NFR-REL-01/06/07 are met with an OS mechanism (kill the process), not with hope.
- Kill tests are simple: kill a worker, kill the core, check the project.
- The core has no Tauri code (ADR-0002). It can become a service later if v0.2+ needs one. Option C's benefits stay possible.
- Tantivy or an ANN index can be added later as **derived** indexes. The source of truth does not change.

**Harder**
- A worker protocol must be designed, versioned, and tested.
- The scheduler needs a total memory budget for all workers (gap A2 in `05-architecture.md`).
- There are more processes to supervise: workers and `llama-server`.

**Follow-ups**
- M2 spike (S2a): measure worker start time and the cost to stream a 1M-row CSV (risk R6). **Done in M2c** (`docs/10-walking-skeleton.md` §S2a): ~385,000 rows/s end to end including the commit, against 50,000 (NFR-PERF-22); the pipe and frames cost ~0.3 s of 2.6 s. Worker start 4.5–18.8 ms (p50), kill and replace 5.4–20.1 ms.
- Re-check this decision if M2 shows the worker overhead breaks NFR-PERF-20/22. **Checked in M2c:** it does not; workers stay for every format, CSV included. NFR-PERF-20 is measured with PDFs in M3.
