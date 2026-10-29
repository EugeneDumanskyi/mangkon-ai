# ADR-0016: Streamed files — deadline per unit of progress, staged commit

- **Status:** Accepted
- **Date:** 2026-10-29
- **Related:** NFR-REL-01/03/06/07, NFR-CAP-05/06/07, NFR-PERF-22/23, NFR-RES-02, INV-PR-04, US-02.4, US-02.6, ADR-0004, ADR-0007 (amends its time limit for MBOX and its chunk buffering), roadmap §4 #16, M3a

## Context

- ADR-0007 stops a worker 120 s after its file started (NFR-REL-06: "Parse step ≤ 120 s per file").
- NFR-CAP-06 asks for a 2 GB MBOX, streamed. NFR-PERF-23 asks for ≥ 200 messages/s.
- A 2 GB MBOX holds hundreds of thousands of messages. At 200 messages/s, 500,000 messages take ≈ 42 minutes. At 2,000 messages/s they still take ≈ 4 minutes. A whole-file deadline of 120 s fails every large mailbox, so NFR-REL-06 and NFR-CAP-06 conflict.
- Measured later in M3a (W3a-2, RM-1, `11-ingestion.md` §3): a worker read a 2 GB MBOX of 1.29M messages in ≈ 21 s (≈ 61,000 messages/s). On that machine a whole-file deadline would also hold at this size. The numbers above use the NFR-PERF-23 floor, which slower machines and larger messages can approach.
- The deadline exists to stop a parser that is stuck (NFR-REL-06, TD-3 endless loops). It is not meant to cap honest work on a big file.
- The memory limit is per file: 1 GB (NFR-REL-07). Streaming keeps the worker's memory flat, whatever the file size.
- ADR-0007 (M2c follow-up): the core buffers all of an item's chunks and commits once at `Done` (INV-PR-04). The buffer is ≈ 4.5 × the file size. That rules out a 500 MB CSV (NFR-CAP-05) and a 2 GB MBOX.
- Roadmap §4 #16 (decided 2026-10-27) asks M3 for "a commit that does not hold the whole result".
- The maintainer decided on 2026-10-29 that for MBOX the 120 s apply per message. This ADR records that decision and the commit that goes with it.
- **Conflict with a higher doc:** NFR-REL-06 says "per file". Its wording changes with this ADR, as decided on 2026-10-29: per file, and per message for MBOX.

## Options considered

### Time limit for a mailbox

**1. Keep the whole-file deadline** (do nothing).
- ➕ One rule for every format.
- ➖ Every MBOX over ≈ 120 s of work fails. That breaks NFR-CAP-06.

**2. A whole-file deadline scaled by size** (for example 120 s + 60 s per 100 MB).
- ➕ Still a single number per file.
- ➖ The scale is a guess about speed. A slow machine fails big files, and a fast one waits minutes for a stuck parser.
- ➖ A loop inside one message is caught only after the whole file's budget is spent.

**3. No deadline for MBOX; only the 15 s silence timeout.**
- ➕ Simple.
- ➖ The heartbeat thread keeps beating while the parser loops. A stuck parser is never stopped (TD-3 `fault-endless-loop.txt` shows this).

**4. One work item per message.**
- ➕ Each message gets the normal limits and its own commit.
- ➖ The queue needs byte offsets into a file, and a re-scan model for parts of a file (a changed mailbox shifts every offset).
- ➖ Thousands of items per file. INV-PR-04 then holds per message, not per file.

**5. The deadline restarts at each finished message.**
- ➕ A stuck parser is stopped 120 s after the last message it finished.
- ➕ The total time grows with the file. It is bounded by the size limit (NFR-CAP-06) and the per-message cap.
- ➖ A mailbox of many slow but honest messages can run a long time. It still shows progress (the unit count).

### Holding the result until the commit

**A. Keep buffering in memory** (ADR-0007 as built).
- ➖ ≈ 4.5 × the file size in the core. It fails NFR-CAP-05/06 and NFR-RES-02.

**B. Write chunks into an open transaction as they come** (ADR-0007's first design).
- ➕ No buffer.
- ➖ The single writer (ADR-0004) is held while a worker parses, for minutes on a big file. Other items and the derived-data rebuild wait.

**C. Stage chunks in memory, spill to a file above a threshold, commit from the stage at `Done`.**
- ➕ The writer is held only for the commit itself, as now.
- ➕ The core's memory per item stays at the threshold.
- ➖ A large file's result is written twice: once to the spill file and once to SQLite.

## Decision

Use **option 5** for the time limit and **option C** for the commit.

**Time limit**
- MBOX: the 120 s deadline runs from the last finished message, not from the start of the file. The worker's heartbeat `units` count finished messages; the core restarts the deadline when `units` rises (`mk_core::workers`).
- Every other format keeps the whole-file deadline of ADR-0007.
- The silence timeout (15 s) and the memory limit (1 GB per file, NFR-REL-07) are unchanged for every format.
- The failure text is unchanged: "Took too long to read".
- A message over 256 MB is skipped with the warning "Message N is larger than 256 MB and was skipped". The rest of the mailbox is read. ASSUMPTION: 256 MB.
- A message with no known header is skipped with a warning. The rest of the mailbox is read.
- No overall cap on a mailbox's total time. ASSUMPTION: the 2 GB size limit is enough of a bound.

**Size limits (NFR-CAP-05/06/07)**
- Checked in the core before a worker starts: CSV over 500 MB, MBOX over 2 GB.
- Such a file fails with "File too large (limit: 500 MB)" or "File too large (limit: 2 GB)". The batch continues.

**Staged commit**
- The core keeps an item's chunks in a staging buffer (`mk_core::stage_a::Staging`).
- Above 32 MB of chunks, the buffer is written as postcard frames to `<project>/cache/staging/item-<id>.bin`, and later chunks are appended there. ASSUMPTION: 32 MB.
- The chunk order is checked as chunks arrive: a document before its segments, and mentions only into the segment batch just before them (worker protocol 2).
- At `Done`, the writer reads the staged parts back in order inside one transaction (`Store::commit_stage_a_staged`). It keeps segment ids only for the current batch, so its memory is O(one chunk) (INV-PR-04 holds unchanged).
- On `Failed`, a crash, a kill or a stop, the stage is dropped: nothing is written, and the spill file is deleted.
- `cache/staging/` is emptied when a project opens. It is derived and safe to delete, like the rest of `cache/`.

## Consequences

**Easier**
- A 2 GB MBOX and a 500 MB CSV fit the per-file and app memory limits (NFR-CAP-05/06, NFR-REL-07, NFR-RES-02).
- A loop inside one message is still stopped in 120 s.
- A crash during a big file still leaves nothing half-written (NFR-REL-03, INV-PR-04).

**Harder**
- Two time-limit rules. The supervisor must know which formats count units.
- A big file's result is written twice: to the spill file, then to SQLite. The disk needs free space for the spill file.
- The total time of a big mailbox has no upper limit apart from its size.

**Follow-ups**
- Change NFR-REL-06 to "per file; per message for MBOX (ADR-0016)" (M3a).
- Add `cache/staging/` to architecture §6 (M3a).
- Watch items W3a-1 (500 MB CSV) and W3a-2 (2 GB MBOX) check the memory and speed of this path (`docs/11-ingestion.md`, M3a). They also check the 32 MB threshold and the `Format::memory_estimate` windows.
- Tests (M3a): the deadline restart with a `slow-messages` test fault (`adr_0016_mbox_deadline_per_message`), the spill file round trip and its removal (`stage_a`), the staged commit and its rollback (`inv_pr_04_staged_commit_*`), and over-limit files (`nfr_cap_07_over_limit_csv_mbox`).
- M3b: decide whether long PDFs need the same rule per page (NFR-REL-06 already has 30 s per OCR page).
