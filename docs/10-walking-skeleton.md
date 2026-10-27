# 10 — Walking skeleton: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M2 (spikes S2a–S2d), ADR-0002, ADR-0003, ADR-0004, ADR-0005, ADR-0014, `03-nfr.md` (NFR-PERF-04, NFR-PERF-32/33/34, NFR-RES-07, NFR-USE-03, NFR-MNT-01/02/03/08). **If this file disagrees with them, they win.**
> Status: §S2d and §2 measured in M2a on RM-1 (2026-10-24). §S2c and §3 measured in M2b on RM-1 (2026-10-27). S2a and S2b are added in M2c. Linux only; macOS and Windows are not verified here.
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
- M2c: the same numbers with parsers in worker processes (results cross a pipe before the writer).
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
