# 10 — Walking skeleton: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M2 (spikes S2a–S2d), ADR-0002, ADR-0003, ADR-0014, `03-nfr.md` (NFR-PERF-04, NFR-RES-07, NFR-USE-03, NFR-MNT-01/02/03/08). **If this file disagrees with them, they win.**
> Status: §S2d and §2 measured in M2a on RM-1 (2026-10-24). S2a–S2c are added in M2b and M2c. Linux only; macOS and Windows are not verified here.
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
