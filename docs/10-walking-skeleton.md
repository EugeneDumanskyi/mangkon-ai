# 10 — Walking skeleton: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M2 (spikes S2a–S2d), ADR-0001, ADR-0002, ADR-0003, ADR-0004, ADR-0005, ADR-0006, ADR-0007, ADR-0014, `03-nfr.md` (NFR-PERF-04, NFR-PERF-22, NFR-PERF-32/33/34, NFR-REL-02/03/04/07, NFR-RES-01/02/07, NFR-USE-03, NFR-MNT-01/02/03/08). **If this file disagrees with them, they win.**
> Status: §S2d and §2 measured in M2a on RM-1 (2026-10-24). §S2c and §3 measured in M2b on RM-1 (2026-10-27). §S2a, §S2b, §4 and §5 measured in M2c on RM-1 (2026-10-27). Linux only; macOS and Windows are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Machine and build

Machine: **RM-1**: Linux x86_64, Ubuntu-based, Intel i7-14650HX (24 threads), 93 GB RAM, NVMe SSD. Display 2560×1440 at 240 Hz, scale 2, Wayland session; the webview ran through XWayland (user agent `X11`). WebKitGTK 2.52.6 (`webkit2gtk-4.1`), Tauri 2.12.5. Toolchain from `rust-toolchain.toml` (Rust 1.99.1), Node 24.22, CMake 3.28, GCC 13.3. Measured 2026-10-24 on an idle machine (confirmed before the timed runs), release builds.

```sh
cargo clean && time cargo xtask build --release                      # §2: clean build
cargo build --release --locked -p mangkon-desktop --features spike-s2d
MK_S2D_AUTORUN=1 MK_S2D_EXIT=1 MK_S2D_OUT=s2d.json target/release/mangkon   # §S2d: one scripted run
```

---

## 2. Build time and size

| Measure | Result | Target |
|---|---|---|
| Clean `cargo xtask build --release` (prerequisite check, source download, `npm ci`, UI build, Tesseract + Leptonica, desktop app) | **54.8 s** wall, 1.4 GB peak RSS | ≤ 15 min (NFR-USE-03) ✅ |
| Release binary `mangkon`, Linux x86_64 | **10.91 MB** | ≤ 150 MB (NFR-RES-07) |
| Same, without `mk-parsers` / `mk-tesseract-sys` | 10.77 MB | — |
| Dynamic libraries added by Tesseract | `libstdc++` only (as ADR-0014) | — |

**Shell share (NFR-RES-07): ~10.8 MB.** In M2a the app calls only `TessVersion`, so the linker keeps 144 KB of Tesseract. With the engine share measured in M0c (18.8 MB: hayro, the image path, Tesseract, `eng.traineddata`; ADR-0014), the expected app size is about 30 MB on Linux. The real figure is measured again when M3 uses the engines.

---

## S2c — Per-commit derived rebuild at TD-2 scale (R7, K4)

### Question and method

Does the per-commit derived rebuild (ADR-0005: one transaction holds a Stage A result, the item's Done state and the rebuilt `identifier` / `connection` / `decision_status` rows of the keys it touched) stay fast with many decisions (NFR-PERF-34)?

Machine RM-1 (§1), idle (confirmed before the run), release build, SQLite 3.54.1 bundled through `rusqlite` 0.41 (WAL, `synchronous=NORMAL`), project on the NVMe SSD (ext4). Measured 2026-10-27.

```sh
cargo run -p corpus-gen --release -- td2                                # TD-2, 99 copies (07-test-corpus.md §11)
cargo test -p mk-core --release -- --ignored s2c --nocapture            # prints one JSON summary
```

1. **Real data.** TD-2's TXT/MD/CSV files (1,089) go through the real pipeline: re-scan, the Stage A lane (23 threads, in process), one commit per file. 792 are stored; 297 fail alone with "Unknown text encoding" (the Windows-1252 and UTF-16 files, M3).
2. **Synthetic top-up** through the store's write API (`enqueue_contents`, `start_item`, `commit_stage_a`) to **10,000 documents**. Each synthetic document holds the three hot keys (the most mentioned real email and two synthetic ones) and five keys drawn from a Zipf spread (s = 1.1) over 20,000 keys.
3. **Measured phase**, run twice: with 0 decisions, then after 10,000 decisions.
   - 100 new files through the lane (60 small TXT, 25 TXT of 200–1,000 lines, 15 CSVs of 500–5,000 rows; 0–2 emails per line: 20 % hot, 40 % Zipf pool, 40 % new keys). Latency per item is "parse finished → commit done", which includes waiting for the one writer while 22 other threads finish parsing.
   - The same commit alone (lane width 1): 10 files with only the hot keys, 10 with only new keys, 5 CSVs of 5,000 rows.
4. **Decisions:** 10,000 `ignore_identifier` / `unignore_identifier` decisions through `Project::set_ignored`: 80 % on existing keys (about 10 % of them re-decide a decided key, so supersede chains form, including six on a hot key), 20 % on keys never seen (orphaned).
5. At the end, the incremental derived rows are compared with `rebuild_all`: equal.

**Pass mark:** p95 of the lane latency + the 500 ms debounce ≤ 2 s.

### Results

Scale at the measured phases: 10,000 documents, 823,193 segments, **296,887 identifier mentions**, 11,826 identifiers, 7,552 connections. Hot keys are in 9,505, 9,208 and 9,208 documents. Database 249 MB.

| Measure | 0 decisions | 10,000 decisions |
|---|---|---|
| Lane, 100 files, latency p50 / p95 / max | 180 / 601 / 818 ms | 211 / 736 / 817 ms |
| **p95 + 500 ms debounce** | **1.10 s** ✅ | **1.24 s** ✅ |
| Hot keys only, commit alone, p50 / max | 12.6 / 14.0 ms | 14.6 / 15.9 ms |
| CSV of 5,000 rows (~1,500 keys), commit alone, p50 / max | 80.8 / 83.2 ms | 90.2 / 97.3 ms |
| New keys only, commit alone, p50 | 0.19 ms | 0.19 ms |

| Other numbers | |
|---|---|
| Step 1 (1,089 TD-2 text files through the lane) | 9.9 s; commit latency p50 4.4 ms, p95 720 ms, max 1.10 s |
| Step 2 (9,208 synthetic commits, one at a time) | 92.7 s; commit p50 9.3 ms, p95 13.4 ms |
| Appending one decision (with its anchor rebuild) | p50 0.04 ms, p95 0.06 ms, max 12 ms; 10,000 in 0.74 s |
| Decision states after step 4 | 6,498 active, 1,504 superseded, 1,998 orphaned |
| `rebuild_all` (every key, all three tables) | 1.25 s |

### What the numbers say

- **The commit itself is small.** A hot key in ~9,500 documents costs ~4–5 ms per rebuild; the worst single commit measured (a 5,000-row CSV touching ~1,500 keys) takes 80–97 ms.
- **The lane latency is queueing.** With 23 parse threads and one writer, an item that finishes parsing waits behind the commits of the others; that wait is most of the 600–740 ms p95. It grows with the lane width and the size of the files that finish together, not with the corpus.
- **Decisions cost little.** 10,000 decisions add ~2 ms to a hot-key commit and ~10 ms to the large CSV (the derived rows and the database grew too between the phases). The `decision(kind, anchor, seq)` index is enough.
- Step 1 shows the same shape under a full ingestion: p95 720 ms, max 1.10 s while 792 files commit back to back, still under 2 s with the debounce.

### Verdict

**Met.** p95 + debounce is 1.10 s with 0 decisions and 1.24 s with 10,000, against 2 s. One item per transaction (ADR-0004) holds; no batching and no extra index is needed. R7 is retired. The incremental rows equal a full rebuild at this scale.

### Follow-ups

- The writer queue is the lever if latency grows (wider lanes on RM-2, heavier Stage A in M3): batching several items per transaction would change ADR-0004 and is **not** done. A cheaper first step is bounding how many finished results wait for the writer.
- M2c: re-run with parsers in worker processes (23 `mk-test-worker` workers, results cross a pipe before the writer), RM-1 idle, 2026-10-27. Lane p50 / p95 / max 167 / 585 / 838 ms with 0 decisions and 205 / 840 / 1,041 ms with 10,000; **p95 + debounce 1.08 s and 1.34 s** ✅. Commits alone: hot keys 12.3 / 14.3 ms p50, 5,000-row CSV 73.6 / 81.7 ms p50. Step 1: 8.6 s, commit p95 602 ms, max 920 ms. Same shape as in process; the pipe adds no visible cost.
- M4: domain mentions (INV-ID-05) roughly double the keys a commit touches; re-run S2c then.

---

## 3. Re-scan of TD-2 with no changes (NFR-PERF-32/33)

```sh
cargo test -p mk-core --release -- --ignored rescan_td2 --nocapture
```

Same machine and day as §S2c. TD-2: 10,000 files (9,999 generated + `td2-manifest.json`), 3.36 GB. Three runs each, after a first scan, so the page cache is warm (dropping it needs root; a cold-cache number is still open).

| Path | Files compared | No-change re-scan | Hashed | New work |
|---|---|---|---|---|
| App path: `Project::rescan` (walk, read stored references, diff, no commit) | 1,089 TXT/MD/CSV stored; 8,911 other files walked and reported as skipped | 0.025 / 0.014 / 0.013 s | 0 | 0 items |
| Full corpus: `mk-corpus` walk + diff with every file as a stored reference | 10,000 | 0.019 / 0.018 / 0.019 s | 0 | — |

**Met:** well under 60 s (NFR-PERF-32), and 0 files read or queued (NFR-PERF-33). In M2b only TXT/MD/CSV are stored (US-01.3 AC3), so the full-corpus number comes from `mk-corpus` alone; the app path is re-checked in M3 when every format is stored. The first scan (hashing the 1,089 text files with BLAKE3) took 0.03 s.

---

## S2d — 100,000-row virtualized list on WebKitGTK (K7)

### Question and method

Does WebKitGTK scroll a 100,000-row virtualized list at 60 fps (NFR-PERF-04)?

- **Screen:** the desktop app with feature `spike-s2d` (never in shipped builds). TanStack Virtual (row height 32 px, overscan 10), rows from the IPC command `synthetic_rows(afterKey, limit)` in keyset pages of 200, cached by TanStack Query. Each row has six cells (key, identifier, kind, source count, two sources) in a CSS grid, with ellipsis truncation.
- **Seek:** synthetic keys equal row positions, so a jump fetches the page at that position directly. Real lists need a position → key lookup for long jumps (M2d).
- **Script** (one `scrollTop` per `requestAnimationFrame`, fixed seed): 300 idle frames (control), 600 frames at 48 px/frame, top → bottom → top at 1,200 px/frame, then 40 random jumps each followed by 20 frames at 48 px/frame. 6,750 measured frames, about 113 s.
- **Recorded:** `requestAnimationFrame` deltas, page-fetch latency, written to JSON through the `record_frame_stats` command.
- **Pass mark** (plan, ASSUMPTION): p95 frame time ≤ 16.7 ms and ≤ 1 % of frames over 25 ms, on Ubuntu, release build.

### Results

Viewport 1138×445 CSS px, device pixel ratio 2.

| Run | Variant | Mean | p50 | p95 | p99 | Max | > 25 ms | Idle mean / p95 |
|---|---|---|---|---|---|---|---|---|
| 1 | baseline (no idle phase yet) | — | 17 ms | 18 ms | 20 ms | 29 ms | 0.10 % | — |
| 2 | baseline | 16.70 ms | 17 ms | 18 ms | 21 ms | 32 ms | 0.22 % | 16.65 / 17 ms |
| 3 | baseline | 16.71 ms | 17 ms | 18 ms | 20 ms | 32 ms | 0.19 % | 16.67 / 17 ms |
| 4 | + `contain: strict` on scroller and rows | 16.70 ms | 17 ms | 18 ms | 20 ms | 33 ms | 0.24 % | 16.65 / 17 ms |
| 5 | same | 16.67 ms | 17 ms | 18 ms | 20 ms | 34 ms | 0.12 % | 16.66 / 17 ms |
| 6 | one text node per row, overscan 3 | 16.67 ms | 17 ms | 19 ms | 20 ms | 33 ms | 0.04 % | 16.66 / 17 ms |
| 7 | same | 16.67 ms | 17 ms | 18 ms | 20 ms | 33 ms | 0.03 % | 16.66 / 17 ms |

Page fetches: 499 per run (every page once), p50 4 ms, p95 7–8 ms over IPC.

### What the numbers say

- **WebKitGTK paces `requestAnimationFrame` at 60 Hz** here, although the display runs at 240 Hz.
- **Its timestamps have 1 ms resolution.** A perfect 60 Hz frame (16.67 ms) reads as 16 or 17 ms, so p50 is 17 ms even with the list idle, and the idle p95 is 17 ms. Under this clock, **p95 ≤ 16.7 ms cannot be met even with nothing on screen.**
- **No frames are lost on average.** The mean frame time while scrolling (16.67–16.71 ms) equals the idle mean within 0.05 ms: about 60.0 fps over 6,750 frames. Scrolling adds about 1 ms of jitter at p95 (18 ms, once 19 ms): a callback arrives late and the next one early.
- **Jank is rare:** 0.03–0.24 % of frames over 25 ms, against a mark of 1 %. The longest frame is 29–34 ms, during the random jumps.
- **The row DOM is not the limit.** `contain: strict` changes nothing. A single text node per row lowers jank (0.2 % → 0.04 %) but not p95. The extra millisecond comes from WebKitGTK's scroll and frame timing.

### Verdict

**Met, with the pass mark read for a 1 ms clock** (ASSUMPTION, needs your OK): mean frame time within 1 % of the idle control, p95 within one vsync + 2 ms (≤ 18.7 ms), and ≤ 1 % of frames over 25 ms. Under the strict reading (p95 ≤ 16.7 ms) it fails by one timer tick, and so does the idle page. K7 is retired for list scrolling.

The baseline row (grid, six cells) stays. The single-text-node row is the recorded lever if real rows get heavier.

### Follow-ups

- NFR-PERF-04: the measure is reworded once the reading above is approved.
- M2d: the real connections list needs a position → key lookup for long jumps and scrollbar drags (keyset pages cannot seek by position).
- macOS (WKWebView) and Windows (WebView2): run the same script when those platforms are tested (platform scope: Linux first). Repeat on a 60 Hz display and in a native Wayland webview.

---

## S2a — CSV streaming worker → core → commit (R6, K3)

### Question and method

Since M2c every file is parsed in a worker process (ADR-0001, ADR-0007): the worker reads the file, checks its hash, parses it, finds the identifier mentions and streams the result to the core as postcard frames (`u32` length + payload) in chunks of segments. The core assembles the chunks of one item and commits them once, at Done (INV-PR-04). Does a 1M-row CSV get through at ≥ 50,000 rows/s including the commit (NFR-PERF-22)? How long does a worker take to start, and to be killed and replaced? How much memory does the buffering cost?

Machine RM-1 (§1), idle (confirmed before the runs), release build. Measured 2026-10-27, two runs; the table shows the second (the first agreed within 3 %).

```sh
cargo run -p corpus-gen --release -- csv-rows --rows 1000000          # testdata/s2a/rows-1m.csv
cargo xtask build --release                                          # for the desktop worker start
MK_S2A_DESKTOP=target/release/mangkon \
  cargo test -p mk-core --release --test s2a_csv_stream -- --ignored --nocapture
```

- **Input:** 1,000,000 rows + header, 74.9 MB: date, description, IBAN, amount, and an email in 30 % of rows (300,265 mentions), filled like TD-2 (`07-test-corpus.md` §9).
- **End to end:** fresh project, one lane, one worker: re-scan, then Stage A (worker: read + hash check, parse, extract, stream; core: decode, assemble, commit with the derived rebuild). Chunks of 1,024, 4,096 and 16,384 segments. Peak resident memory of the core and of core + worker, sampled every 100 ms. The same bytes once more as a `.txt` file, for the plain-text memory factor.
- **Breakdown:** the same steps in one process, timed one by one.
- **Worker start:** 100 × start to `Hello`, and 100 × kill + wait + start + `Hello`, for the test worker (`mk-test-worker`, as small as `mangkon-cli`) and the desktop binary (`mangkon --worker`, which maps the WebKitGTK libraries).

**Pass mark:** ≥ 50,000 rows/s including the commit.

### Results

| Chunk size (segments) | Stage A | Rows/s | Commit | Peak core + worker | Peak core |
|---|---|---|---|---|---|
| 1,024 | 2.57 s | 388,541 | 1.64 s | 733 MB | 317 MB |
| **4,096 (default)** | **2.59 s** | **385,386** ✅ | 1.62 s | 740 MB | 323 MB |
| 16,384 | 2.61 s | 383,486 | 1.62 s | 764 MB | 347 MB |
| Same bytes as TXT, 4,096 | 1.89 s | 528,879 lines/s | 1.50 s | 579 MB | 321 MB |

Re-scan (read + BLAKE3) of the file: 24–39 ms.

| Breakdown, in one process | Time |
|---|---|
| Read + hash | 47 ms |
| Parse (CSV, delimiter and header detection) | 496 ms |
| Extract mentions, build chunks | 200 ms |
| Encode frames (115 MB in 491 chunks) | 144 ms |
| Decode + assemble | 143 ms |
| Commit (segments, mentions, derived rebuild, Done) | 1,560 ms |

| Worker start (100 runs) | Start to `Hello` p50 / p95 / max | Kill + replace p50 / p95 / max |
|---|---|---|
| `mk-test-worker` | 4.5 / 5.1 / 5.2 ms | 5.4 / 5.9 / 6.1 ms |
| `mangkon --worker` (desktop binary) | 18.8 / 29.1 / 58.9 ms | 20.1 / 31.4 / 37.1 ms |

### What the numbers say

- **The process boundary is cheap.** Encoding, the pipe and decoding together cost ~0.3 s of 2.6 s. The commit is 60 % of Stage A, and it is the same commit as in process.
- **Chunk size does not matter** between 1k and 16k segments; larger chunks only raise the peak a little. 4,096 stays.
- **Frames are 1.5 × the file** (115 MB for 75 MB): text plus locators and cell offsets. Not worth trimming at this speed.
- **Buffering in the core costs memory, not time.** Peak core + worker is ≈ 10 × the file size for CSV and ≈ 8 × for TXT; the core alone holds ≈ 4.5 × until the commit. `Format::memory_estimate` uses these factors for the memory budget (gap A2, §5).
- **The desktop binary starts 4 × slower** than a small one (dynamic linking of WebKitGTK and GTK) but still in ~20 ms; a replacement after a kill is not noticeable.

### Verdict

**Met.** ~385,000 rows/s against 50,000, with the commit. Workers for CSV stay (ADR-0001); no CSV special case is needed. R6 is retired.

### Follow-ups

- **CSV size limit:** at ≈ 10 × the file size, a 500 MB CSV (NFR-CAP-05) would need ~2.9 GB in the worker and ~2.2 GB in the core, over the 1 GB per-file limit (NFR-REL-07) and the 2 GB of NFR-RES-02. The worker's share (≈ 5.8 × the file) puts today's practical limit at ≈ 175 MB per CSV. Raising it needs a streaming CSV parser and a commit that does not hold the whole result (ADR-0007's "write chunks into one open transaction"). **Decided (2026-10-27):** NFR-CAP-05 stays at 500 MB; M3 builds the streaming parser and commit.
- S2c with workers: re-run, still met (§S2c follow-ups: p95 + debounce 1.08 s / 1.34 s).

---

## S2b — Memory bomb stopped (R5, gap A5)

### Question and method

Is a memory bomb stopped within 200 ms after it passes 1 GB, and failed with "Needed too much memory", while other files finish (NFR-REL-07)?

Machine RM-1 (§1), idle, release build, 2026-10-27.

```sh
cargo test -p mk-core --release --test s2b_memory_bomb -- --ignored --nocapture
```

- 20 bomb files (test fault `memory-bomb`: the worker allocates 64 MB blocks and touches every page) mixed with 20 plain files, one lane, default limits.
- **Limits:** the core samples each worker's resident memory every 100 ms and kills it over 1 GB. On Linux the worker also caps its own address space at start (`RLIMIT_AS` = its size at start + 1 GB + 512 MB headroom), so an allocation that runs away between two samples fails inside the worker (Rust aborts with "memory allocation of N bytes failed", which the core reads as a memory stop). macOS: the watchdog only. Windows: the watchdog only for now (below).
- **Measured per bomb:** how it was stopped, the highest RSS sampled, and the time from the last sample under 1 GB to the stop: an upper bound on how long it ran over the limit.

**Pass mark:** every bomb stopped ≤ 200 ms after passing 1 GB; every bomb Failed with "Needed too much memory"; every plain file Done.

### Results

| | |
|---|---|
| Bombs stopped | 20 of 20, all by the watchdog (the `RLIMIT_AS` cap was never reached) |
| Time over the limit (upper bound) | 144.8–160.2 ms |
| Highest RSS sampled | 1,348–1,349 MB |
| Failure reason | "Needed too much memory" for all 20 |
| Plain files | 20 of 20 Done |
| Whole run | 7.1 s |

### Verdict

**Met on Linux.** Every bomb was stopped ≤ 160 ms after passing 1 GB, with the plain reason, and the other files finished. At 64 MB per step a bomb grows ~325 MB past the limit before the next sample sees it, so RSS can briefly reach ~1.35 GB. The `RLIMIT_AS` cap is a backstop for faster growth. R5 is retired on Linux.

### Follow-ups

- **Windows hard cap:** a Job Object with a per-process memory limit needs a Win32 call; no maintained crate offers it without `unsafe` in our code (the `win32job` crate sets only working-set limits), and ADR-0002 allows `unsafe` only in `mk-tesseract-sys`. Windows has the watchdog alone, like macOS. **Decided (2026-10-27):** accepted; no ADR-0002 exception.
- macOS and Windows: `s2b_memory_bomb_short` runs in CI on all three OSes once the repo is pushed; **not verified here**.
- NFR-REL-07 wording (gap A5): "stopped within 200 ms after its memory passes 1 GB (sampled every 100 ms); on Linux an address-space cap stops faster growth" (proposal, roadmap M2c outcome).

---

## 4. Kill test (M2 criterion 1; NFR-REL-02/03/04)

```sh
cargo build --release -p mangkon-cli
target/release/mangkon-cli kill-test --source testdata/td1/synthetic --runs 20 --seed 1
target/release/mangkon-cli kill-test --source testdata/td2 --runs 20 --seed 1
```

The driver runs `mangkon-cli kill-test-target` (open, add the folder, re-scan, Stage A in workers) three times without a kill, then 20 times with a kill (`SIGKILL`) at a seeded random moment between the start and the end of ingestion (the shortest reference run). After each kill it checks: every worker of the killed process exits by itself within 2 s; reopening (restart recovery, ADR-0006) loses at most one in-progress item per lane and no item the target saw committed; SQLite integrity and foreign keys hold and the derived rows equal a rebuild; no item stays in progress. Then it ingests again and compares the result with the reference runs.

RM-1, release build, 2026-10-27, 23 lanes.

| Source | Ingestion (shortest reference) | Kill moments | Passed | Orphan workers | Items recovered per kill |
|---|---|---|---|---|---|
| TD-1 TXT/CSV subset (8 files) | 38 ms | 12 at process start, 6 in project open, 2 in Stage A | **20 / 20** | 0 | 0–5 |
| TD-2 TXT/CSV (792 stored files) | 9.0 s | 19 in Stage A, 1 just after it | **20 / 20** | 0 | 0–23 |

**Met.** 0 corrupted projects in 40 kills; lost work never exceeded the files in progress (at most 23 = the lane width); no item stayed in progress after reopening, and every resumed project ended equal to the reference. The TD-1 subset is so small that most kills land before Stage A; the TD-2 run (the same files × 99) puts the kills in the middle of Stage A. `nfr_rel_02_kill_test_short` (3 kills on a generated folder) runs on every OS in CI; macOS and Windows are **not verified here**.

---

## 5. Memory (M2 criterion 7; NFR-RES-01/02)

| Measure | Result | Limit |
|---|---|---|
| **Idle (7a):** `target/release/mangkon` with the window open, 60 s, sum over the process tree (the app, `WebKitWebProcess`, `WebKitNetworkProcess`), sampled every 10 s | **439 MB** RSS (190 + 201 + 47); 207 MB PSS; flat from 10 s to 60 s | 500 MB ✅ |
| **Stage A on the CSV part of TD-2 (7b):** 396 CSV files, 23 lanes, core + workers sampled every 100 ms, `nfr_res_02_stage_a_memory_td2_csv` | **233 MB** with `mk-test-worker`; **908 MB** with the desktop binary as worker; core alone 58 MB; 8.6 s | 2 GB ✅ |
| Idle worker, one process | `mangkon --worker` 35 MB RSS (28 MB of it shared libraries), 22 MB PSS; `mangkon-cli --worker` 5.7 MB | — |

```sh
MK_WORKER_PROGRAM=target/release/mangkon \
  cargo test -p mk-core --release --test nfr_res_02 -- --ignored --nocapture
```

- **7a is partial:** the app has no project screens yet (M2d). It is measured again there. The margin is small (61 MB), and two thirds of it is WebKitGTK.
- **7b:** resident memory counts each worker's shared library pages again, so 23 desktop workers count ~800 MB before they read a file; PSS would be ~40 % lower. The budget counts RSS, the cautious measure (gap A2). Of the 396 files, 99 (the Windows-1252 TD-1 CSV × 99) fail alone with "Unknown text encoding" until M3.
- No worker stays after a Stage A run: the lanes shut their workers down at the end (NFR-RES-01).
- **Memory budget (gap A2):** 1.5 GB for the items in flight; an item's need is 40 MB + 8 × its size for TXT/MD and 40 MB + 10 × for CSV (§S2a). Small files keep going past a large one that waits; a file larger than the budget runs alone. Workers are replaced after 200 files, or when idle above 300 MB.

