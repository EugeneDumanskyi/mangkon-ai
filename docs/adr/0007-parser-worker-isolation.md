# ADR-0007: Parser isolation — worker processes with time and memory limits

- **Status:** Accepted
- **Date:** 2026-10-12
- **Related:** NFR-REL-01/06/07, NFR-CAP-04..07, NFR-PERF-20..23/25, NFR-RES-02, NFR-REL-10, INV-CO-01/09, INV-PR-04/10, ADR-0001, ADR-0006

## Context

- Users drop messy folders. Some files are corrupt, encrypted, huge, or crafted to break parsers (TD-3).
- One bad file must never stop the batch (NFR-REL-01).
- Parse step ≤ 120 s per file; OCR ≤ 30 s per page (NFR-REL-06). One file ≤ 1 GB memory (NFR-REL-07).
- PDF and OCR libraries may include native C/C++ code (ADR-0013). Native crashes cannot be caught in Rust.
- App memory during Stage A ≤ 2 GB, without the model (NFR-RES-02).

## Options considered

**1. Threads in the core + `catch_unwind`.**
- ➕ No IPC.
- ➖ Catches Rust panics only. A native crash or abort kills the app.
- ➖ A stuck thread cannot be stopped. Memory per file cannot be limited.

**2. A new process for every file.**
- ➕ Perfect isolation. Memory is freed after each file.
- ➖ Process start cost for each of 10,000 files (and OCR model loading each time, if the engine has one).

**3. A pool of long-lived worker processes, recycled.**
- ➕ Same isolation as option 2 when something goes wrong: kill and replace.
- ➕ Start cost and engine loading are paid once per worker.
- ➖ Slow memory leaks in a library can build up. Needs recycling.

**4. WebAssembly sandbox (for example wasmtime) for parsers.**
- ➕ Strong isolation and hard memory limits on every OS.
- ➖ Native PDF/OCR libraries do not compile to WASM easily. Slower. Much more work.

## Decision

Use **option 3**: a pool of long-lived worker processes.

**Process**
- The worker is the app binary started with `--worker`. It links only `mk-parsers`, `mk-identifiers`, and `mk-kernel` code paths (ADR-0002).
- One file at a time per worker.
- A worker **never** opens the project database, never writes files, and never uses the network. It opens source files **read-only** (NFR-REL-10).
- Recycle a worker after 200 files, or when its idle memory is over 300 MB. ASSUMPTION — tune in M2.

**Protocol**
- Length-prefixed binary frames over stdin/stdout, encoded with `serde` + `postcard` (or `bincode`). Pick in M2.
- A protocol version is checked at worker start.
- Messages:
  - core → worker: `ProcessContent { path, content_hash, file_type, settings, limits }`, `RenderPage { path, page, scale }`, `Shutdown`.
  - worker → core: `Heartbeat { stage, page }`, `Chunk { documents | segments | identifier_mentions | sensitive_flags }`, `Done { warnings }`, `Failed { code, plain_reason }`, `PageImage { … }`.
- Results are **streamed** in chunks. The core writes chunks into one open transaction and commits on `Done` (INV-PR-04, ADR-0004). On `Failed`, a crash, or a kill, the transaction rolls back.

**Stage A work in the worker**
- Parse → segment with exact locators → OCR where needed → identifier extraction (`mk-identifiers`) → sensitive flags.
- Identifier extraction runs here because it is cheap and needs the fresh segments. One round trip per file.
- Custom-pattern re-runs work on **stored** text, so they run in the core's light lane (ADR-0006). The `regex` crate runs in linear time and rejects look-around (INV-ID-07), so user patterns cannot hang it.

**Limits**
- **Time:** the core sets a deadline per file (120 s parse) and per OCR page (30 s), using heartbeats. On deadline: kill, mark the file Failed with "Took too long to read", start a new worker.
- **Memory:**
  - Windows: Job Object with a hard process memory limit.
  - Linux: `RLIMIT_AS` as a coarse hard cap, plus an RSS watchdog.
  - macOS: RSS watchdog only. macOS does not enforce `RLIMIT_AS` (gap A5).
  - Watchdog samples every 100 ms. Over 1 GB → kill, mark Failed with "Needed too much memory".
- **Total budget:** the scheduler keeps a total memory budget for all workers (1.5 GB, ASSUMPTION; gap A2). It estimates each file's need from type and size. A big file waits until the budget allows it. Small files keep flowing.

## Consequences

**Easier**
- NFR-REL-01/06/07 are met by the OS, not by careful coding in every parser.
- TD-3 tests become simple: each bad file must fail alone with a plain reason.
- A future sandbox (seccomp on Linux, App Sandbox on macOS) can be added to workers without touching the core.

**Harder**
- About one week to build the protocol, supervisor, watchdog, and tests. ASSUMPTION.
- Serialization overhead for very large files (1M-row CSV). Must still meet NFR-PERF-22 (risk R6).
- Three OS-specific memory-limit code paths.

**Follow-ups**
- M2 spike (S2a): worker start time, CSV streaming throughput, kill-and-replace time. **Done in M2c** (`docs/10-walking-skeleton.md` §S2a): a 1M-row CSV at ~385,000 rows/s including the commit (NFR-PERF-22: 50,000); worker start to `Hello` 4.5 ms (small binary) and 18.8 ms (desktop binary, p50); kill and replace 5.4 / 20.1 ms.
- TD-3 must include a "memory bomb" and an "endless loop" file. **Done in M2c** as test fault files (`fault-memory-bomb.txt`, `fault-endless-loop.txt`, `fault-crash.txt`, TD-3 v2), plus the PDF and PNG bombs of TD-3 v1 for M3.
- **Done in M2c**, as decided above, with these choices:
  - **Crate `mk-worker`:** the protocol, framing and the worker loop; `mk-core` holds the supervisor. Both apps call `mk_core::worker_main_if_requested()` first in `main`.
  - **Encoding: postcard** (1.x; `bincode` is no longer developed). Frames are a `u32` little-endian length + payload, at most 256 MB. Protocol version 1, checked in `Hello { protocol, app_version, pid }`.
  - **Chunks are buffered in the core, not written into an open transaction.** The core assembles one item's chunks and commits once at `Done` with `Store::commit_stage_a` (INV-PR-04 holds unchanged; a failure, crash or kill drops the buffer, so nothing is written). The single writer is never held open while a worker is still parsing. Cost: the core holds ≈ 4.5 × the file size until the commit (S2a), which limits the CSV size (below).
  - **Chunk size:** 4,096 segments; 1k–16k measured the same.
  - **Heartbeats** every 500 ms with the stage (reading, parsing, sending) and a unit count; a worker silent for 15 s is killed and its file fails with "Took too long to read". ASSUMPTION for both values.
  - **Limits as decided:** 120 s per file; 1 GB per file, sampled every 100 ms. Linux: the worker sets `RLIMIT_AS` on itself at start to its size then + 1 GB + 512 MB headroom; an allocation failure under it is read as a memory stop. Spike S2b: every bomb stopped ≤ 160 ms after passing 1 GB, by the watchdog (§S2b).
  - **Windows Job Object: not built.** No maintained crate sets a per-process memory limit without `unsafe` in our code (`win32job` sets only working-set limits), and ADR-0002 rule 6 forbids it. Windows has the watchdog only, like macOS (decided 2026-10-27, roadmap §4 #15).
  - **Recycling** after 200 files or idle RSS over 300 MB, as decided (not tuned: no leak seen on TXT/CSV). Workers exit when their stdin closes, so a killed core leaves no worker behind (kill test: 0 orphans in 40 kills).
  - **Total budget** 1.5 GB for the items in flight (gap A2): need = 40 MB + 8 × size (TXT/MD) or 10 × size (CSV), from S2a. The first item that fits runs; if none fits and nothing runs, the first runs alone. RSS is counted, so shared library pages count once per worker.
  - **Test faults:** a first line `mk-test-fault: memory-bomb | endless-loop | crash | panic` makes the worker misbehave only when the core turns test faults on through its API; the shipped app never does, so such a file is plain text.
- **CSV size (decided 2026-10-27, roadmap §4 #16):** with whole-file parsing and buffering, a CSV needs ≈ 10 × its size in memory, so files over ≈ 175 MB hit the 1 GB per-file limit; NFR-CAP-05 asks for 500 MB. Meeting it needs a streaming CSV parser and a commit that does not hold the whole result. NFR-CAP-05 stays at 500 MB; M3 builds both.
