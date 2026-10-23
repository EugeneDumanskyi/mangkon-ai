# 08 — Engine bake-off: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M0 (spike S0, split rows M0b/M0c), ADR-0013, ADR-0011, ADR-0007, `03-nfr.md` (NFR-PERF-20, NFR-QA-12, NFR-RES-07, NFR-MNT-01), `04-domain.md` (INV-CO-03, INV-CO-04), `07-test-corpus.md`. **If this file disagrees with them, they win.**
> Status: PDF rows measured in M0b. OCR rows, the 3-OS build probe and ADR-0014 come in M0c.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Question

Spike S0 asks which engines pass every ADR-0013 criterion, and whether the highlight round-trip holds. This file holds the **PDF half** (M0b):

- Can one PDF engine give stored text, exact character boxes and page renders that agree with each other (ADR-0011, ADR-0013 rule 3)?
- Is it fast enough, buildable with one command, small enough and AGPL-compatible?
- Does a ground-truth mention survive span → boxes → rendered glyphs in ≥ 99% of samples (early NFR-QA-12, M0 AC5)?

The numbers here are inputs to ADR-0014 (written in M0c, together with the OCR rows).

---

## 2. Candidates

| | Engine | Version | Kind | How it is used |
|---|---|---|---|---|
| A | `hayro` | 0.8.0 | Pure Rust | `hayro-interpret` runs the page through a glyph-collecting device for text and boxes; `hayro` renders through the same interpreter. |
| B | `pdf_oxide` | 0.3.78 (feature `rendering`) | Pure Rust | `extract_chars` for text and boxes, `rendering::PageRenderer` for the page image. |
| C | pdfium | `pdfium-render` 0.9.4 + `libpdfium` `chromium/7881` | C++ library, Rust binding | Prebuilt `libpdfium` from `bblanchon/pdfium-binaries`, pinned tag + SHA-256 per platform in `spikes/pdf-bakeoff/pdfium.json`, downloaded with `curl` by `pdf-bakeoff fetch-pdfium`. Bound at run time. |

Not candidates:

- **Mixed stacks** (for example `lopdf` for text + `hayro` for render). ADR-0013 rule 3 needs one engine for text and render, so the boxes match the pixels.
- **MuPDF.** Not in the M0b roadmap row (§9, conflict 4).

---

## 3. Machine and commands

Machine: **RM-1, Linux x86_64, 24 threads**, release build, `rustc 1.99.1`. All runs single-threaded. Measured 2026-10-23 03:22 UTC on an idle machine. One run per engine; every timing is at least 15× inside its pass mark, so run-to-run noise does not change a result.

Harness: `spikes/` is its own Cargo workspace (own `Cargo.lock`, excluded from the main workspace) with one crate, `pdf-bakeoff`. Each engine sits behind a cargo feature (`hayro`, `oxide`, `pdfium`), so build time and size can be measured per engine. Throwaway: removed in M2a once ADR-0014 records the numbers (roadmap M0 scope).

```sh
# needs testdata/ (07-test-corpus.md §9)
cargo run --release --manifest-path spikes/Cargo.toml -p pdf-bakeoff -- fetch-pdfium   # libpdfium into spikes/vendor/, SHA-256 checked
spikes/pdf-bakeoff/build-probe.sh                                                       # clean builds per engine → spikes/out/build-probe.json
cargo build --release --locked --manifest-path spikes/Cargo.toml -p pdf-bakeoff --features hayro,oxide,pdfium
spikes/target/release/pdf-bakeoff report --engines hayro,oxide,pdfium                    # everything below → spikes/out/report.{json,md}
```

Single measurements: `pdf-bakeoff throughput | render-timing | roundtrip | td3 --engine <e>`; `roundtrip --samples 200 --seed 1` is the AC5 sample, `--all` the full pool. `pdf-bakeoff text --engine <e> --file <pdf> --page <n>` prints one page's stored text. Outputs go to `spikes/out/` (ignored).

---

## 4. Method

Every engine runs through the same code. Only the adapter differs: it returns `page_count`, page size and `/Rotate`, raw characters with boxes, and a grey page image. The adapters' error kinds are `Encrypted`, `Corrupt`, `Unsupported` and `Other`.

### 4.1 Layout (shared, engine-independent)

So the engines are compared on glyphs, boxes and pixels, not on reading-order code, one layout step turns raw characters into the page's stored text:

1. Drop whitespace and control characters the engine reports; map boxes into display orientation through `/Rotate`.
2. Group characters into lines: a character joins a line when it overlaps ≥ 50% of the line's first character's vertical band.
3. Split a line into fragments at gaps > 1.0× the median character height; insert a space at gaps > 0.15×.
4. Order fragments by recursive XY-cut at the widest gap (vertical cut wins ties). This reads tables column by column.
5. Join with `\n`. Inserted spaces and newlines get no box.

One segment per page in M0b; finer segmenting is M3.

### 4.2 Highlight round-trip

The sample is drawn from TD-1 mentions that have `pdf_text` locations (ground-truth line boxes): all 106 identifier mentions plus 94 name mentions picked by a seeded shuffle (seed 1) — 200 in total (§9, conflict 2). The full pool of 295 mentions is reported too.

Each sample passes four steps:

| Step | What | Pass when |
|---|---|---|
| Find | Locate the mention's text in the stored page text, ignoring whitespace (so mentions split across lines match across `\n`). With several hits, take the one whose boxes best overlap the ground truth. | Found. |
| Map | Open the file **again** (the `RenderPage` path, ADR-0011), build `PageGeometry` from that open and call `highlight(page_text, span)`. | The fresh open reproduces the stored text (length + SHA-256) and returns boxes. |
| Geometry | Compare returned boxes with ground-truth line boxes, in display orientation. | Every ground-truth line is matched by a returned box, and every returned box by a line: x-extent IoU ≥ 0.8 and vertical overlap ≥ 50% of the ground-truth height. **ASSUMPTION.** |
| Render | Render the page at scale 2.0; count dark pixels (< 128) inside the ground-truth box. | ≥ 90% of them fall inside the returned box padded by 1.5 px. **ASSUMPTION.** |

### 4.3 Throughput and text quality

Throughput is pages per second for open + characters + layout over every text-layer PDF in TD-1: synthetic text PDFs, the text pages of the mixed files, the OCR-layer files and the public slice (885 pages). Identifier extraction does not exist yet (§9, conflict 3). `td3/pages-2000.pdf` is reported separately.

Text quality is a diagnostic, not a pass mark: character error rate (CER) of the stored text against `page-text.jsonl`, whitespace-normalised, over the 531 synthetic text-layer pages. It is given twice: as stored, and with lines sorted first. The reference text reads tables row by row, while our XY-cut reads them column by column, so the first number measures reading order and the second measures characters.

### 4.4 Page view

Open, characters and layout, then render one page at scale 2.0, on `synthetic/pdf/vendor-ledger-2024-2025.pdf` (500 pages). There are 20 pages picked with seed 1, and each one gets a fresh open. Reported: p50, p95, max.

### 4.5 TD-3

The 15 TD-3 files a PDF engine takes (PDF magic bytes, or a `.pdf` name without image magic) each run in their own child process (`pdf-bakeoff td3-one`). The child opens the file, parses every page and renders page 1. The parent sets a 30 s deadline (**ASSUMPTION**, spike only, until NFR-REL-06 workers exist) and reads the child's RSS from `/proc` every 100 ms, killing it above 1 GiB (NFR-REL-07 placeholder). That is the ADR-0007 Linux path, but as harness code. A file "fails" when it does not open, has no pages, or no page can be read. The result is compared with `expected.json`. Image files are M0c.

`bomb-image-dimensions.pdf` is shown but not scored: its manifest entry says the expected `fail` is the app's own guard, which refuses the image before decoding (`07-test-corpus.md` §6).

### 4.6 Build, size, licenses

- **Build:** `build-probe.sh` runs a clean `cargo build --release --locked` per engine, each in a fresh target directory, plus a no-engine baseline. Crates come from the local registry cache, so download time is not counted, and neither is the `libpdfium` download (3.6 MB archive).
- **Size:** binary size over the baseline, plus `libpdfium.so` for pdfium.
- **Licenses:** the crates each engine adds to the normal-dependency closure (`cargo metadata`, Linux target), checked against an AGPL-3.0-compatible SPDX list. `libpdfium` is checked by hand from the notices bundled in its archive. `cargo-deny` arrives in M2a (§9, conflict 7).

---

## 5. Results

### 5.1 S0 PDF criteria

| Criterion | Pass mark | hayro | pdf_oxide | pdfium |
|---|---|---|---|---|
| Text throughput, TD-1 text-layer PDFs (parse + layout, 1 thread) | ≥ 20 pages/s | ✅ 2,594 pages/s | ✅ 2,992 pages/s | ✅ 1,416 pages/s |
| Page view on the 500-page ledger: fresh open + boxes + render at 2×, p95 of 20 pages | ≤ 1,000 ms | ✅ 6 ms (p50 4, max 11) | ✅ 9 ms (p50 8, max 260) | ✅ 6 ms (p50 5, max 13) |
| Highlight round-trip, 200 samples (seed 1) | ≥ 99% | ✅ 100% (200/200) | ❌ 95.5% (191/200) | ✅ 100% (200/200) |
| TD-3 PDFs: each ends alone in its own process | all 15 | ✅ 15/15 | ✅ 15/15 | ✅ 15/15 |
| TD-3 PDFs: outcome as in `expected.json` (engine-decided files) | all 14 | ✅ 14/14 | ❌ 13/14 | ✅ 14/14 |
| Password-protected PDF reported as such (US-02.1 AC2) | yes | ✅ | ✅ | ✅ |
| Clean release build, Linux (baseline without an engine: 6 s) | ≤ 15 min | ✅ 19 s | ✅ 58 s | ✅ 12 s |
| Size added over the no-engine binary | counts toward ≤ 150 MB (NFR-RES-07) | +8.8 MB | +16.3 MB | +4.6 MB binary + 7.6 MB `libpdfium.so` = +12.2 MB |
| Licenses AGPL-3.0-compatible | all | ✅ 71 crates, none flagged | ✅ 141 crates, none flagged | ✅ 23 crates, none flagged; `libpdfium` notices all permissive (§5.5) |

**hayro and pdfium pass every PDF criterion. pdf_oxide fails the round-trip and one TD-3 file.**

No sample failed the Map step for any engine. Each fresh open reproduced the stored page text exactly, so rebuilding geometry at render time (§6) holds, and the plan's stop condition, "re-extraction drifts", did not trigger.

### 5.2 Diagnostics (no pass mark)

| Measure | hayro | pdf_oxide | pdfium |
|---|---|---|---|
| Round-trip, full pool | 100% (295/295) | 96.6% (285/295) | 100% (295/295) |
| Round-trip, identifier mentions | 100% (106/106) | 93.4% (99/106) | 100% (106/106) |
| Round-trip, `hard` mentions | 100% (9/9) | 22% (2/9) | 100% (9/9) |
| Public identifiers found on their page (`pdf_page`, no boxes) | 102/109 | 96/109 | 102/109 |
| CER, stored text as is (reading order) | 71.3% | 71.3% | 65.6% |
| CER, lines sorted first (characters) | 0.04% | 0.06% | 0.04% |
| Throughput: synthetic text PDFs (527 pages) | 5,532 pages/s | 6,610 pages/s | 2,820 pages/s |
| Throughput: public slice (354 pages) | 1,460 pages/s | 1,644 pages/s | 813 pages/s |
| Throughput: `td3/pages-2000.pdf` | 35,480 pages/s | 55,282 pages/s | 47,521 pages/s |
| Page view, mean ms: open / characters + layout / render | 1 / 0 / 4 | 0 / 1 / 19 | 0 / 1 / 4 |
| Page errors / open errors over TD-1 | 0 / 0 | 0 / 0 | 0 / 0 |

Notes:

- **The reading-order CER is our layout, not the engines.** XY-cut reads the synthetic tables column by column and the reference reads them row by row. With lines sorted, all three engines agree with the reference to within 0.06%. Reading order inside tables is M3 work.
- **The 7 public identifiers that every engine misses** are URLs and email addresses that the Federal Register wraps at a line end with a printed hyphen, for example `2024-NPRM-CONSUMER-` / `REPORTING@cfpb.gov`. The public manifest records them without the hyphen. The stored text is right to keep the printed character. Joining line-end hyphens inside identifiers is extraction work (M4, §8).
- pdf_oxide misses 6 more public identifiers. All are phone numbers wrapped across two lines; pdf_oxide's boxes make our layout put the two halves in different places.
- pdf_oxide's 260 ms page-view maximum is one outlier. Its renderer is about 5× slower than the other two on the same pages, but still far inside the mark.

### 5.3 TD-3 PDFs

| File | Expected | hayro | pdf_oxide | pdfium |
|---|---|---|---|---|
| `blank-pages.pdf` | success | ✅ 3 pages | ✅ 3 pages | ✅ 3 pages |
| `bomb-double-flate.pdf` | fail | ✅ killed at 1,034 MB | ❌ success, 1 page, 0 characters (258 MB) | ✅ killed at 1,214 MB |
| `bomb-image-dimensions.pdf` | fail (app guard) | not scored: engine success, 0 MB | not scored: engine success, 0 MB | not scored: engine success, 0 MB |
| `corrupt-xref-offsets.pdf` | success | ✅ 2 pages | ✅ 2 pages | ✅ 2 pages |
| `empty.pdf` | fail | ✅ Corrupt | ✅ Corrupt | ✅ Corrupt |
| `encrypted-owner-only.pdf` | success | ✅ 1 page | ✅ 1 page | ✅ 1 page |
| `encrypted-user-password.pdf` | fail | ✅ Encrypted | ✅ Encrypted | ✅ Encrypted |
| `garbage-after-header.pdf` | fail | ✅ Corrupt | ✅ Corrupt | ✅ Corrupt |
| `loop-form-xobject.pdf` | success | ✅ 1 page | ✅ 1 page | ✅ 1 page |
| `loop-page-tree.pdf` | fail | ✅ Corrupt | ✅ Corrupt | ✅ Other |
| `loop-xref-prev.pdf` | success | ✅ 1 page | ✅ 1 page | ✅ 1 page |
| `pages-2000.pdf` | success | ✅ 2,000 pages | ✅ 2,000 pages | ✅ 2,000 pages |
| `pdf-named.png` | success | ✅ 1 page | ✅ 1 page | ✅ 1 page |
| `text-named.pdf` | fail | ✅ Corrupt | ✅ Corrupt | ✅ Corrupt |
| `truncated.pdf` | fail | ✅ Corrupt | ✅ Corrupt | ✅ Corrupt |

Every file ended within 0.5 s, and no engine panicked, hung or crashed its process. Error kinds come from the adapters' mapping of each engine's errors. pdfium's page-tree loop arrives as a generic error (`Other`).

- **`bomb-double-flate.pdf`** (2 GiB after two Flate filters): hayro and pdfium inflate it without a cap and the watchdog kills them past 1 GiB. That matches `fail`, but the plain reason comes from the worker's memory limit, not the engine. So the app depends on ADR-0007 workers with NFR-REL-07 limits (M2c) for this file. pdf_oxide caps the inflate silently and reports a readable page with no text. That is silent data loss rather than a failure, so it does not match.
- **`bomb-image-dimensions.pdf`**: no engine decodes the image when it parses text or renders at scale 1 (memory stayed near 0). The app guard is still needed on the OCR path, which decodes page images (M0c/M3).

### 5.4 Round-trip failures

hayro and pdfium: none, in the 200 samples or in the full pool of 295.

pdf_oxide, 10 failures in the full pool:

| Step | Count | File | Cause |
|---|---|---|---|
| Geometry | 8 | `synthetic/pdf/framework-contract-bwl.pdf` p3 (`/Rotate 90`) | Character boxes of rotated text have zero width across the line (for example x 111.4–111.4), so no returned box overlaps its line. That fails six mentions of one IBAN, each on its own line, plus a name and a phone number on the same page. With zero-width boxes every hit of the IBAN scores the same, so all six resolve to the first one. 7 of the 9 `hard` mentions are on this page. |
| Render | 2 | `synthetic/ocr-layer/annual-return-saltmarsh.pdf` p1 | The boxes for the invisible OCR text layer sit about 1.6 pt above the glyphs, so 89–90% of the ink falls inside instead of ≥ 90%. Both are borderline. |

Two harness bugs were found and fixed during the spike, before these numbers were taken:

- Geometry was first compared in unrotated page space. It is now compared in display orientation, the way the user sees the highlight.
- pdfium reports a hyphen at a line end as U+0002 (with `is_hyphen` set). The pdfium adapter now stores the printed `-`, so `brightwater-` / `logistics` is found. **This mapping must move into the pdfium adapter in `mk-parsers` if pdfium is ever chosen.**

### 5.5 Licenses

- **hayro** (71 crates): MIT, Apache-2.0, BSD-2/3-Clause, Zlib, Unicode-3.0, Unlicense, 0BSD (most dual MIT/Apache-2.0).
- **pdf_oxide** (141 crates): the same families.
- **pdfium-render** (23 crates): the same families plus ISC.
- **`libpdfium`** (from the notices in the `chromium/7881` archive): pdfium itself is BSD-3-Clause. The libraries bundled with it are abseil (Apache-2.0), fast_float and simdutf (MIT), FreeType (FTL), ICU (Unicode-3.0), Little CMS, libjpeg-turbo (IJG/BSD), OpenJPEG (BSD-2-Clause), libpng, libtiff, llvm-libc (Apache-2.0 with LLVM exception) and zlib. The only mentions of the GPL are clauses quoted inside the ICU and LLVM texts.

All are compatible with AGPL-3.0. Whichever engine wins, its notices go into `THIRD_PARTY_NOTICES` (ADR-0013).

---

## 6. The PDF `Locator` and `PageGeometry`

Drafted in `crates/mk-kernel` (no I/O, no new dependencies):

| Type | Meaning |
|---|---|
| `PageNumber` | 1-based page number (`NonZeroU32`). |
| `Locator::PdfText { page, span }` | A segment's place in the page's **stored text**: page + character span (INV-CO-03, US-02.1 AC1). It holds no boxes. The other INV-CO-03 kinds are added later: OCR (page + word boxes + span) in M0c, and line/row/path/message kinds in M2/M3. |
| `Rect` | `f32` box in PDF points: unrotated page box, top-left origin, y down. This is the ground-truth convention (`07-test-corpus.md` §5). Checked constructor (finite, x0 ≤ x1, y0 ≤ y1), plus union, intersection and IoU. |
| `PageGeometry` | One entry per character of the stored page text: `Some(Rect)` for a glyph, `None` for a space or newline that layout inserted. Also stores the text's length and SHA-256. |
| `PageGeometry::highlight(page_text, span)` | Checks the length and hash against the stored text, then returns **one union box per line** the span touches (split at `\n`), the same shape as the ground truth. |
| `GeometryError` | Wrong box count, length mismatch, hash mismatch (`HashMismatch`), span past the end of the text, or a span with no boxes. |

How it is used (ADR-0011): parsing stores only the page text and `PdfText` locators. When the page view opens a page (`RenderPage`), the **same engine** re-extracts that page's characters, the same layout code rebuilds `PageGeometry`, and `highlight` turns the locator's span into boxes on the rendered image. If the engine or layout code changed and the text no longer matches the stored text, `highlight` returns `HashMismatch`. That is an error the UI must show, never a silent wrong highlight (INV-CO-04).

Tests: `inv_co_03_pdf_locator_is_page_and_span`, `inv_co_04_text_drift_is_an_error`, `inv_co_04_span_past_text_rejected`, `nfr_qa_12_one_box_per_line`, and the property test `nfr_qa_12_highlight_inside_span_boxes`, which checks that every returned box lies inside the union of the span's character boxes and that each touched line has exactly one box.

The round-trip above runs exactly this path, with a fresh open for the Map step, and holds at 100% for hayro and pdfium.

---

## 7. Inputs for ADR-0014

- **PDF engine: hayro.** It passes every PDF criterion, and ADR-0013's default order says to take the pure-Rust option first when it passes. It needs no native build, no download host and no extra catalog entry (gap A1 does not arise for PDF). It adds 8.8 MB, builds in 19 s, gave 100% on the round-trip and matched every engine-decided TD-3 outcome.
- **Fallback: pdfium.** It also passes everything: smaller in total (+12.2 MB including the library), the same round-trip, and the most mature of the three. It would mean shipping a checksum-verified prebuilt per platform (ADR-0008 pattern) and putting the download host on the network list. Keep the adapter until ADR-0014 is accepted.
- **Not pdf_oxide:** it fails the round-trip on rotated text, it places the OCR-layer boxes off the glyphs, and it silently drops the 2 GiB stream instead of failing.
- **hayro risks to weigh:**
  - It is young (0.8).
  - Text extraction is not part of its API, so the adapter builds characters from glyphs through the interpreter's device (about 200 lines we own).
  - Character box heights are a fixed band of −0.25 to 0.8 em instead of per-font ascent and descent (§8).
- **Both passing engines need worker memory limits** (ADR-0007, NFR-REL-07) to fail the decompression bomb. No engine caps it itself.
- **Numbers for NFR-RES-07:** the PDF engine adds 8.8 MB (hayro) or 12.2 MB (pdfium) to the app.

ADR-0014 is written in M0c, together with the OCR rows and the macOS and Windows build probe. ADR-0013 stays **Open** until then.

---

## 8. Open points

- **ASSUMPTION:** the round-trip thresholds (x-extent IoU ≥ 0.8, vertical overlap ≥ 50%, ≥ 90% ink at scale 2.0, 1.5 px padding) and the 30 s / 1 GiB TD-3 limits are spike values, not product limits.
- hayro boxes use a fixed vertical band (−250 to 800 per 1,000 em units) and advance width. That is enough for the highlight on TD-1. Fonts with tall or deep glyphs (non-Latin scripts) are checked again when M3 adds language coverage.
- Reading order: XY-cut reads tables column by column (§5.2). Table and multi-column order is M3 layout work. The locator does not depend on it, because spans point into whatever text was stored.
- Line-end hyphens inside identifiers (`CONSUMER-` / `REPORTING@cfpb.gov`): the stored text keeps the printed hyphen. Identifier extraction must try the joined form (M4). Whether the public manifest should list the printed or the joined form is open.
- `bomb-double-flate.pdf` is stopped only by the harness watchdog. The real worker limits are M2c.
- Build and size were measured on Linux only. macOS and Windows, including the pdfium binaries for both, are probed in M0c.
- One run per engine on RM-1. Repeat runs and RM-2 numbers come with the M2b benchmark harness.

---

## 9. Conflicts found while planning

Listed, not silently resolved.

1. **Locator shape.** Roadmap M0 says "page + span + character/word boxes"; INV-CO-03 says PDF → page + span. The domain wins: `PdfText` holds page + span, and boxes are rebuilt at render time (§6). OCR keeps word boxes in its locator (M0c). The roadmap wording was aligned in M0b.
2. **"200 random ground-truth identifiers."** TD-1 has only 106 identifier mentions with `pdf_text` boxes (142 counting derived domains, which share locations). The sample is all 106 plus 94 seeded-random name mentions; the full pool of 295 is reported too.
3. **NFR-PERF-20** counts "parsing + identifier extraction". No extractor exists before M3/M4, so M0b measures parse + layout. It is checked again in M3 (roadmap M3 AC2). The margin is about 70× at the slowest passing engine.
4. **Mixed stacks.** ADR-0013 rule 3 (one engine for text and render) excludes pure-Rust combinations such as `lopdf` text + `hayro` render. MuPDF is not in the M0b roadmap row.
5. **Build on three OSes.** The S0 build criterion is "one-command build on 3 OSes"; M0b records Linux only. M0c probes all three.
6. **Thresholds.** The round-trip thresholds, render scale, TD-3 deadline and memory cap are ASSUMPTIONs (§8).
7. **`cargo-deny`** is not set up yet (M2a). Licenses are checked from `cargo metadata` for now.

---

## OCR rows: M0c

OCR candidates (`ocrs`, Tesseract, ONNX-based OCR) behind `OcrEngine`, the scans round-trip, the 3-OS build probe and the R3 desk check are added here in M0c.
