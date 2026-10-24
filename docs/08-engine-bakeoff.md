# 08 — Engine bake-off: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M0 (spike S0, split rows M0b/M0c), ADR-0013, ADR-0011, ADR-0007, `03-nfr.md` (NFR-PERF-20/21, NFR-QA-06/07/12, NFR-RES-07, NFR-MNT-01), `04-domain.md` (INV-CO-03, INV-CO-04, INV-ID-03), `07-test-corpus.md`. **If this file disagrees with them, they win.**
> Status: PDF rows measured in M0b (§1–§9). OCR rows and the R3 desk check measured in M0c (§10–§17). Builds were measured on Linux only; macOS and Windows are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Question

Spike S0 asks which engines pass every ADR-0013 criterion, and whether the highlight round-trip holds. This file holds the **PDF half** (M0b):

- Can one PDF engine give stored text, exact character boxes and page renders that agree with each other (ADR-0011, ADR-0013 rule 3)?
- Is it fast enough, buildable with one command, small enough and AGPL-compatible?
- Does a ground-truth mention survive span → boxes → rendered glyphs in ≥ 99% of samples (early NFR-QA-12, M0 AC5)?

The numbers here are inputs to ADR-0014. The OCR half (M0c) starts at §10.

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

Harness: `spikes/` is its own Cargo workspace (own `Cargo.lock`, excluded from the main workspace) with one crate, `pdf-bakeoff`. Each engine sits behind a cargo feature (`hayro`, `oxide`, `pdfium`), so build time and size can be measured per engine. Throwaway: removed in M3, once the hayro adapter and the OCR path are ported to `mk-parsers` (moved from M2a, decided 2026-10-24).

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
- **Licenses:** the crates each engine adds to the normal-dependency closure (`cargo metadata`, Linux target), checked against an AGPL-3.0-compatible SPDX list. `libpdfium` is checked by hand from the notices bundled in its archive. `cargo-deny` arrives in M2a (§9, conflict 7); set up there (`deny.toml`).

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

ADR-0014 is written in M0c, together with the OCR rows (§17). ADR-0013 stays **Open** until then.

---

## 8. Open points

- **ASSUMPTION:** the round-trip thresholds (x-extent IoU ≥ 0.8, vertical overlap ≥ 50%, ≥ 90% ink at scale 2.0, 1.5 px padding) and the 30 s / 1 GiB TD-3 limits are spike values, not product limits.
- hayro boxes use a fixed vertical band (−250 to 800 per 1,000 em units) and advance width. That is enough for the highlight on TD-1. Fonts with tall or deep glyphs (non-Latin scripts) are checked again when M3 adds language coverage.
- Reading order: XY-cut reads tables column by column (§5.2). Table and multi-column order is M3 layout work. The locator does not depend on it, because spans point into whatever text was stored.
- Line-end hyphens inside identifiers (`CONSUMER-` / `REPORTING@cfpb.gov`): the stored text keeps the printed hyphen. Identifier extraction must try the joined form (M4). Whether the public manifest should list the printed or the joined form is open.
- `bomb-double-flate.pdf` is stopped only by the harness watchdog. The real worker limits are M2c.
- Build and size were measured on Linux only. macOS and Windows, including the pdfium binaries for both, are not verified here (platform scope: Linux first).
- One run per engine on RM-1. Repeat runs and RM-2 numbers come with the M2b benchmark harness.

---

## 9. Conflicts found while planning

Listed, not silently resolved.

1. **Locator shape.** Roadmap M0 says "page + span + character/word boxes"; INV-CO-03 says PDF → page + span. The domain wins: `PdfText` holds page + span, and boxes are rebuilt at render time (§6). OCR keeps word boxes in its locator (M0c). The roadmap wording was aligned in M0b.
2. **"200 random ground-truth identifiers."** TD-1 has only 106 identifier mentions with `pdf_text` boxes (142 counting derived domains, which share locations). The sample is all 106 plus 94 seeded-random name mentions; the full pool of 295 is reported too.
3. **NFR-PERF-20** counts "parsing + identifier extraction". No extractor exists before M3/M4, so M0b measures parse + layout. It is checked again in M3 (roadmap M3 AC2). The margin is about 70× at the slowest passing engine.
4. **Mixed stacks.** ADR-0013 rule 3 (one engine for text and render) excludes pure-Rust combinations such as `lopdf` text + `hayro` render. MuPDF is not in the M0b roadmap row.
5. **Build on three OSes.** The S0 build criterion is "one-command build on 3 OSes"; M0b and M0c record Linux only. macOS and Windows are not verified here (platform scope: Linux first). The 3-OS wording in the roadmap and NFRs is unchanged for now.
6. **Thresholds.** The round-trip thresholds, render scale, TD-3 deadline and memory cap are ASSUMPTIONs (§8).
7. **`cargo-deny`** is not set up yet (M2a). Licenses are checked from `cargo metadata` for now. Done in M2a: `deny.toml`, run in CI.

---

## 10. OCR: question and candidates

The OCR half of spike S0 (M0c) asks which OCR engine passes every ADR-0013 criterion on the TD-1 scans and the TD-3 images, and whether a scan mention survives span → stored word boxes → pixels on the image that was OCR'd (early NFR-QA-12, M0 AC5).

| | Engine | Version | Kind | Assets (pinned by SHA-256 in `spikes/pdf-bakeoff/ocr-models.json`, fetched by `pdf-bakeoff fetch-ocr`) |
|---|---|---|---|---|
| A | `ocrs` | 0.13.1 on `rten` 0.26.0 | Pure Rust | `text-detection.rten` + `text-recognition.rten`, 12.2 MB (`ocrs-models.s3-accelerate.amazonaws.com`) |
| B | Tesseract 5.5.2 + Leptonica 1.87.0 | `tesseract-rs` 0.4.0, its built-in static build | C++ compiled from source, Rust binding | `eng.traineddata` from `tessdata_fast` 4.1.0, 4.1 MB (`github.com`) |
| C | PP-OCRv5 mobile on ONNX Runtime | `oar-ocr` 0.10.1, `ort` 2.0.0-rc.13 | Prebuilt C++ runtime, Rust binding | det + English rec `.onnx` + dictionary from the `oar-ocr` v0.3.0 release, 12.7 MB (`github.com`) |

ocrs (S3) and `tessdata_fast` publish no digest, so their SHA-256 values were pinned on first download (2026-10-23). The PP-OCRv5 values match the GitHub release digests.

Every adapter ran inside its ½-day timebox. Tesseract built on the first try, so the system-`tesseract` fallback was not needed.

---

## 11. OCR: machine and commands

Same machine and rules as §3 (RM-1, Linux x86_64, release build, idle machine), measured 2026-10-23 04:35 UTC. Each OCR run is single-threaded: `RAYON_NUM_THREADS`, `RTEN_NUM_THREADS` and `OMP_THREAD_LIMIT` are 1, and ONNX Runtime uses 1 intra-op and 1 inter-op thread. CPU time is read from `/proc/self/stat`, and it matches wall time within 0.01 s, so "per page" means "per core".

```sh
cargo run --release --manifest-path spikes/Cargo.toml -p pdf-bakeoff --features ocr -- fetch-ocr   # models into spikes/vendor/ocr/<engine>/, SHA-256 checked
spikes/pdf-bakeoff/build-probe.sh                                                                  # clean builds per feature, fresh HOME each
cargo build --release --locked --manifest-path spikes/Cargo.toml -p pdf-bakeoff --features hayro,pdfium,ocrs,tesseract,onnx
spikes/target/release/pdf-bakeoff report --engines hayro,pdfium,ocrs,tesseract,onnx                 # §5 and §13 → spikes/out/report.{json,md}
```

Single measurements: `ocr-quality | ocr-roundtrip | ocr-td3 --engine <e>`; `ocr-text --engine <e> --file <path> [--page <n>] [--raw]` prints one page's OCR text (with word boxes and confidences when `--raw` is given).

`tesseract-rs` and `ort` cache their build-time downloads in `~/.tesseract-rs` and `~/.cache/ort.pyke.io`. The probe gives every build a fresh empty `HOME`, so those downloads happen inside the timed build, as on a first checkout.

---

## 12. OCR: method

All three engines sit behind one `OcrEngine` trait: models load once in `new()` (ADR-0007 long-lived workers), and `recognize(&GrayImage, langs)` returns lines of words, each with text, a pixel box and an optional confidence. Error kinds: `TooLarge`, `Corrupt`, `Unsupported`, `Model`, `Other`. Everything around the engine is shared, so the engines are compared on recognition alone.

### 12.1 Image input (shared)

- **Format by content.** Magic bytes win over the extension (TD-3 ASSUMPTION). PNG, JPEG and TIFF are decoded by the `image` crate to 8-bit grey.
- **App guard.** The header is read first, and the image is refused before decoding above 100 MP or a 512 MiB allocation (`image::Limits`; **ASSUMPTION**, spike values). A JPEG without an end-of-image marker is refused. Without that check, the decoder fills the missing half with grey and OCR of half a page passes as the whole page (TD-3 `truncated.jpg`).
- **PDF pages** (scanned PDFs, and mixed-PDF pages with fewer than 20 characters, the US-02.2 AC1 rule) are rendered by hayro at 300 DPI. Before rendering, the page size at 300 DPI and every image XObject's declared `/Width × /Height` go through the same guard. Ground-truth pixel boxes are scaled by 300 / the file's DPI.

### 12.2 OCR layout (shared)

1. Split each engine line into fragments at gaps > 1.0× the median word height (as §4.1 step 3).
2. Order fragments by the same XY-cut as the PDF layout (§4.1 step 4).
3. Join words with a space and fragments with `\n`. Each word keeps its pixel box and confidence, mapped to its character span in the page text (`OcrGeometry`, §14). A word with whitespace inside is split, and its box is divided by character count.

### 12.3 Criteria

| Criterion | Method | Pass mark |
|---|---|---|
| OCR speed (NFR-PERF-21) | Decode (or render) + OCR + layout, one thread, on the 5 clean-tier A4 300 DPI pages. The slowest page counts. Model load is reported separately. | ≤ 5 s/page/core |
| Accuracy (NFR-QA-07) | 1 − CER against `page-text.jsonl`, whitespace-normalised, clean tier (6 pages, 2,461 characters), with the OCR lines put in reference order (below). | ≥ 95% |
| Identifier recall (NFR-QA-06, proxy) | Identifier mentions on clean-tier OCR pages whose raw text (whitespace-insensitive) is in the OCR page text. There is no extractor yet (§15, conflict 3). | ≥ 85% |
| Highlight round-trip on scans (M0 AC5, early NFR-QA-12) | All 166 `scan` mentions. Steps as §4.2: **Find** in the stored OCR text. **Map** through the stored `Locator::OcrText` boxes, with no re-OCR (ADR-0011). **Geometry**: each returned word box goes to the ground-truth line it overlaps most (as a share of the lower height); each line's union must meet x-IoU ≥ 0.8 and vertical overlap ≥ 50%. **Render**: ≥ 90% of the ink in the ground-truth box falls inside the returned boxes padded by 1.5 px, on the image that was OCR'd. The rate counts **found** mentions; Find misses are OCR misreads and count as recall (§15, conflict 2). | ≥ 99% |
| TD-3 images | The 7 files that reach the OCR path: `empty.png`, `truncated.png`, `truncated.jpg`, `jpeg-named.pdf`, `png-lying-ihdr.png`, `png-decompression-bomb.png`, and `bomb-image-dimensions.pdf` (now scored, because this path decodes page images). Each runs in its own child process with the §4.5 deadline and RSS watchdog. | all as expected |
| Word confidence (US-02.2 AC3, INV-ID-03) | Does every word come with a confidence? | yes |
| Build | Clean `cargo build --release --locked` per feature, fresh target dir and fresh `HOME`. Everything beyond cargo is recorded. | ≤ 15 min, one command, no Python |
| Size (NFR-RES-07) | Binary over the `ocr` build (hayro + the shared image path) + model files | counts toward ≤ 150 MB |
| Licenses | Crates over the `ocr` build (`cargo metadata`), plus native libraries and model files by hand | AGPL-compatible |
| Determinism | Every clean page OCR'd twice: same text and boxes? | diagnostic |
| Languages | Desk check | diagnostic |

**Why not "lines sorted" for accuracy.** M0b sorted both sides' lines to take reading order out of the CER (§4.3). That does not work for OCR. One misread first character ("I. Services" for "1. Services") moves the line to another place in the sort and costs its whole length: ocrs scored 177 edits on a page whose real error count is 2. So the OCR CER puts the **OCR lines in reference order** first: OCR and reference lines are paired cheapest first (edit distance as a share of the longer line), one OCR line per reference line, so repeated lines each find their own place. Lines left over join their best match. Only the OCR side is permuted, whole lines at a time, so the result is still a real edit distance. On every page it is ≤ the sorted-lines number. Both, and the as-stored CER, are in §13.2.

---

## 13. OCR: results

### 13.1 S0 OCR criteria

| Criterion | Pass mark | ocrs | Tesseract | PP-OCRv5 (ONNX) |
|---|---|---|---|---|
| OCR speed, clean A4 300 DPI, 1 thread, slowest of 5 pages | ≤ 5 s/page | ✅ 1.65 s (mean 1.09) | ✅ 0.26 s (mean 0.19) | ✅ 1.32 s (mean 0.85) |
| Accuracy, clean tier, 1 − CER (reference order) | ≥ 95% | ✅ 96.9% | ✅ 98.5% | ✅ 99.2% |
| Identifier recall, clean tier (proxy) | ≥ 85% | ✅ 90.5% (19/21) | ✅ 100% (21/21) | ✅ 100% (21/21) |
| Highlight round-trip on scans, over found mentions | ≥ 99% | ✅ 100% (127/127; found 127/166) | ✅ 100% (134/134; found 134/166) | ❌ 93.9% (139/148; found 148/166) |
| TD-3 images: each ends alone in its own process | all 7 | ✅ 7/7 | ✅ 7/7 | ✅ 7/7 |
| TD-3 images: outcome as in `expected.json` | all 7 | ✅ 7/7 | ✅ 7/7 | ✅ 7/7 |
| Word confidence | yes | ❌ no: not in the 0.13 API | ✅ per word | ✅ per line, copied to its words |
| Clean release build, Linux, one command, no Python | ≤ 15 min | ✅ 22 s, nothing beyond cargo | ✅ 43 s; needs cmake and a C++ compiler; downloads sources at build time (§13.5) | ✅ 36 s; downloads ONNX Runtime at build time (§13.5) |
| Size: binary + model files | counts toward ≤ 150 MB | 6.6 + 12.2 = **18.8 MB** | 4.6 + 4.1 = **8.7 MB** | 31.2 + 12.7 = **43.9 MB** |
| Licenses AGPL-3.0-compatible | all | ⚠️ crates ✅; models CC-BY-SA-4.0 (§13.6) | ✅ | ✅ |

**Only Tesseract passes every OCR criterion.** ocrs fails word confidence, and its model license needs review. PP-OCRv5 fails the round-trip.

All three engines were deterministic: the same text and boxes on every clean page, OCR'd twice.

### 13.2 Diagnostics (no pass mark)

| Measure | ocrs | Tesseract | PP-OCRv5 |
|---|---|---|---|
| Accuracy, office tier (9 pages) | 98.0% | 99.2% | 99.6% |
| Accuracy, poor tier (7 pages) | 74.3% | 67.1% | 82.6% |
| Accuracy, all 22 pages | 90.0% | 88.5% | 94.0% |
| Accuracy, clean tier: lines sorted (M0b) / as stored | 84.6% / 72.7% | 97.3% / 75.8% | 99.2% / 77.0% |
| Identifier recall: office / poor / all tiers | 85% / 29% / 71% (41/58) | 85% / 41% / 78% (45/58) | 100% / 53% / 86% (50/58) |
| Round-trip over found: identifier mentions | 100% (58/58) | 100% (62/62) | 98.5% (65/66) |
| Round-trip, all 166: failures by step | Find 39 | Find 32 | Find 18, Geometry 7, Render 2 |
| Model load | 0.01 s | 0.03 s | 0.05 s |
| Dynamic libraries beyond libc, libm, libgcc_s | none | `libstdc++` | `libstdc++` |
| Written into `HOME` by build scripts (downloads + their build cache) | 0 | 86 MB | 105 MB |

- **The as-stored CER is our reading order, not the engines** (as in §5.2). XY-cut reads the invoice tables column by column.
- **The poor tier** (skewed, noisy, 200 DPI) separates the engines most: PP-OCRv5 is far ahead (82.6%), and Tesseract is lowest (67.1%). Poor scans are not under a pass mark, but users will have them. ASSUMPTION: the tessdata_best model or image clean-up before OCR (deskew, binarise) closes part of the gap; not tried here.
- **Find misses** are mostly long identifiers with one misread character (IBAN digits, `0`/`O`, `1`/`I`), mainly on the poor tier.

### 13.3 TD-3 images

| File | Expected | ocrs | Tesseract | PP-OCRv5 |
|---|---|---|---|---|
| `bomb-image-dimensions.pdf` | fail | ✅ TooLarge (image 100,000 × 100,000 px) | ✅ same | ✅ same |
| `empty.png` | fail | ✅ Corrupt | ✅ Corrupt | ✅ Corrupt |
| `jpeg-named.pdf` | success | ✅ 6 words | ✅ 6 words | ✅ 7 words |
| `png-decompression-bomb.png` | fail | ✅ TooLarge (60,000 × 60,000 px) | ✅ same | ✅ same |
| `png-lying-ihdr.png` | fail | ✅ TooLarge (100,000 × 100,000 px) | ✅ same | ✅ same |
| `truncated.jpg` | fail | ✅ Corrupt (no end-of-image marker) | ✅ same | ✅ same |
| `truncated.png` | fail | ✅ Corrupt | ✅ Corrupt | ✅ Corrupt |

Every file ended within 0.4 s with a clean error or a result; no engine panicked or was killed. The three bombs are stopped by the app guard before any engine sees a pixel, so the guard (§12.1) must move into the OCR worker in `mk-parsers`. The engines are never asked to cope with them.

### 13.4 Round-trip failures after Find

ocrs and Tesseract: none.

PP-OCRv5, 9 of 148 found mentions:

| Step | Count | Cause |
|---|---|---|
| Geometry | 7 | PP-OCR's detector grows each text box ("unclip"), so its boxes are up to about 45 px taller than the glyphs at 300 DPI. The word boxes are cut from CTC character positions inside that grown box and are off at the line ends. The last word can run to the end of the box (`Brightwater Logistics Ltd` ends at x 807 against 621), and a short word can come out narrower than its glyphs (`VPA`: x 846–917 against 837–926, x-IoU 0.80, just under). |
| Render | 2 | A tilted line on a skewed scan: 81% and 88% of the ink inside the padded boxes. |

ASSUMPTION: shrinking the detector boxes by the unclip ratio in the adapter would fix most of the Geometry failures. That was not tried inside the timebox.

Three harness bugs were found and fixed before these numbers were taken; all three only matter on misread or skewed scans:

- A mention whose text was misread, while the same text occurs elsewhere on the page, matched that other occurrence. A hit that does not touch the mention's ground-truth area is now a Find miss.
- On a skewed scan, a line's axis-aligned box is taller than any one word in it, so words failed "≥ 50% of the line height". A word now goes to the line it overlaps most, measured against the lower of the two heights. The line checks themselves are unchanged.
- The same tall box takes in ink from the next line. The Render step now ignores dark pixels inside other words' ground-truth boxes.

### 13.5 Build

| | ocrs | Tesseract | PP-OCRv5 |
|---|---|---|---|
| Clean build (baseline without an engine: 6 s; `ocr` build: 16 s) | 22 s | 43 s | 36 s |
| Beyond cargo | nothing | cmake (3.28.3 here), a C++ compiler (g++ 13.3 here) | nothing |
| Build-time downloads | none | Leptonica 1.87.0 and Tesseract 5.5.2 source zips, and `tessdata_best` `eng` + `tur` from the `main` branch, all from GitHub, **none checksum-verified**. The `tessdata_best` files are not used or embedded (feature `embed-tessdata` is off). | A prebuilt ONNX Runtime 1.28.0 from `cdn.pyke.io`, checked against the SHA-256 listed in `ort-sys`'s `dist.tsv`. |
| Notes | — | Static link; only `libstdc++` added at run time. | Static link. `oar-ocr`'s default features need system OpenSSL headers for that download; the spike uses `ort`'s rustls TLS instead. 177 crates. |

All three meet "≤ 15 min, one command, no Python" on Linux. For Tesseract, "one command" holds only with cmake and a C++ compiler installed and network access during the build. Its unverified source downloads conflict with pinning everything by hash (ADR-0008 pattern); §17 has the fix.

### 13.6 Licenses

| Component | License | Ships as | Checked at |
|---|---|---|---|
| ocrs crates (23 over `ocr`) | MIT / Apache-2.0 | binary | `cargo metadata` |
| ocrs models | **CC-BY-SA-4.0** | data files; trained on HierText (CC-BY-SA-4.0) | Hugging Face model card `robertknight/ocrs`; the `ocrs-models` repo has no license file |
| `tesseract-rs` crates (2 over `ocr`) | MIT, MIT / Apache-2.0 | binary | `cargo metadata` |
| Tesseract 5.5.2 | Apache-2.0 | compiled into the binary | `tesseract-ocr/tesseract` |
| Leptonica 1.87.0 | BSD-2-Clause | compiled into the binary | `leptonica-license.txt` |
| `tessdata_fast` `eng.traineddata` 4.1.0 | Apache-2.0 | data file | `tesseract-ocr/tessdata_fast` |
| `oar-ocr` + `ort` crates (177 over `ocr`) | MIT, Apache-2.0, BSD, ISC, Zlib, BSL-1.0, CDLA-Permissive-2.0 (none flagged) | binary | `cargo metadata` |
| ONNX Runtime | MIT, plus its ThirdPartyNotices | linked statically | `microsoft/onnxruntime` |
| PP-OCRv5 models (ONNX export) | Apache-2.0 | data files | `PaddlePaddle/PaddleOCR`, `GreatV/oar-ocr` |
| Shared OCR path (`image` crate, 7 crates over hayro) | MIT / Apache-2.0, BSD-2-Clause | binary | `cargo metadata` |

CC-BY-SA-4.0 is a share-alike license for the model files. Shipped as separate, unmodified data files with attribution, they can sit next to an AGPL-3.0 app, but a fine-tuned model would have to stay CC-BY-SA. **ASSUMPTION**, not a legal review. Everything else is permissive.

### 13.7 Languages (desk check)

| Engine | Scripts and languages | How a language is added |
|---|---|---|
| ocrs | Latin alphabet only (ocrs README; more languages are planned upstream) | not possible yet |
| Tesseract | 125 languages and 37 script models in `tessdata_fast` 4.1.0, including Cyrillic, Greek, Arabic, Hebrew, Devanagari, CJK and Thai | one `.traineddata` file per language (1–10 MB) |
| PP-OCRv5 | One rec model per script group: Chinese/Japanese/English, English, Latin, Cyrillic, East Slavic, Greek, Arabic, Devanagari, Korean, Tamil, Telugu, Thai | one rec model + dictionary per group (`oar-ocr` release assets) |

TD-1 is English only, so recognition quality in other languages was not measured. INV-WS-06 and Q9 (OCR languages per workspace) need an engine with per-language files. ocrs cannot provide that yet.

---

## 14. The OCR `Locator` and `OcrGeometry`

Drafted in `crates/mk-kernel` (no I/O, no new dependencies):

| Type | Meaning |
|---|---|
| `Locator::OcrText { page, span, boxes, image }` | Page + **word boxes** + span in the stored OCR page text (INV-CO-03). `boxes` are the boxes of the words the span touches, in pixels of the image that was OCR'd. `image` is that image's size, so the viewer can scale the boxes to any render. Boxes are stored at OCR time: no re-OCR to highlight (ADR-0011). `Locator` is now `Clone` (was `Copy`), because this kind holds a `Vec`. |
| `ImageSize` | Width and height in pixels. |
| `OcrWord` | A word's span in the page text, its pixel `Rect` and its confidence (`Option<f32>` in [0, 1]). |
| `OcrGeometry` | The page's words, sorted and non-overlapping, plus the text's length and SHA-256. |
| `OcrGeometry::highlight(page_text, span)` | Checks length and hash against the stored text (INV-CO-04), then returns the **word boxes** the span touches, whole words (US-08.1 AC2). |
| `OcrGeometry::confidence(page_text, span)` | Lowest word confidence over the span (INV-ID-03; **ASSUMPTION**: minimum, not mean). `None` if the engine gives none. |
| `OcrGeometry::locator(page, page_text, span)` | Builds the `OcrText` locator. |
| `Rect` | Units now depend on the locator kind: PDF points for `PdfText`, image pixels for `OcrText`. Top-left origin, y down, in both. |

Tests: `inv_co_03_ocr_locator_is_page_boxes_span`, `inv_co_04_ocr_text_drift_is_an_error`, `inv_co_04_ocr_span_past_text_rejected`, `us_08_1_ac2_highlight_returns_word_boxes`, `inv_id_03_confidence_is_lowest_word`, and the property test `nfr_qa_12_ocr_highlight_covers_span_words` (every word the span touches is returned, and no other).

The round-trip in §13.1 runs exactly this path: its Map step reads the boxes from `geometry.locator(...)`.

---

## 15. OCR: open points and conflicts

1. **Sample size.** "200 random identifiers" in PDFs and scans, but TD-1 has 166 `scan` mentions (78 identifiers, 21 of them on clean-tier pages). All 166 are used (as §9, conflict 2).
2. **Round-trip denominator.** A misread mention is never extracted, so no one clicks it. The round-trip rate counts found mentions; Find misses are reported as recall. M0b's text PDFs never failed Find, so the two definitions agree there. **ASSUMPTION.**
3. **NFR-QA-06** counts extracted identifiers. No extractor exists (M4), so the raw-text presence is the proxy, as M0b did for NFR-PERF-20.
4. **NFR-QA-07 "≥ 95% characters"** is read as 1 − CER, whitespace-normalised, OCR lines in reference order, clean tier (§12.3). **ASSUMPTION.** The clean tier holds 6 pages (2,461 characters); the office tier (9 pages) gives the same verdict for every engine.
5. **Scanned-PDF pages are rendered by hayro at 300 DPI**, not taken from the embedded image. This ties OCR of scanned PDFs to the PDF engine. **ASSUMPTION.**
6. **NFR-RES-07** cannot be measured whole before the Tauri shell exists (M2a). Engine and model shares are measured here. M2a measured the shell share: ~10.8 MB (`10-walking-skeleton.md` §2).
7. **Thresholds.** 100 MP / 512 MiB image guard, 300 DPI render and the round-trip thresholds are spike values. **ASSUMPTION.**
8. **ocrs confidence.** `TextChar` holds only the character and its box. Getting a confidence would mean running the recognition model a second time from the adapter, or an upstream change.
9. **Build on three OSes**: as §9, conflict 5. Linux only; Tesseract's cmake build on macOS and Windows is not verified here.

---

## 16. R3 desk check: prebuilt `llama-server`

GitHub releases API for `ggml-org/llama.cpp`, read 2026-10-23. Nothing was downloaded or run.

- **Newest build: `b11829`** (2026-10-22 23:43 UTC), 36 assets. Every asset carries a SHA-256 digest in the release metadata, and the release links a GitHub build-provenance attestation.
- **Release channels.** Every `bNNNNN` build is now marked *pre-release*. The newest non-pre-release is **`v0.7.0`** (2026-10-15). It holds only `nightly-tag.txt`, which names the build it stands for: `b11669`, the same 36-asset layout.
- **Host chain.** `github.com/ggml-org/llama.cpp/releases/download/<tag>/<asset>` answers 302 to `release-assets.githubusercontent.com`.

Desktop targets in `b11829` (sizes in MB):

| OS | Arch | Backend | Asset | MB | SHA-256 |
|---|---|---|---|---|---|
| Linux | x64 | CPU | `llama-b11829-bin-ubuntu-x64.tar.gz` | 22.3 | `281df0193466d75022958c331697ba3d702bbecb62c5c82e09178facbc851f5c` |
| Linux | x64 | Vulkan | `llama-b11829-bin-ubuntu-vulkan-x64.tar.gz` | 35.0 | `656bd1425bdde09565f2191c2a62940a1992441497fc5fb5fd45cdfa337e621f` |
| Linux | x64 | CUDA 12.8 | `llama-b11829-bin-ubuntu-cuda-12.8-x64.tar.gz` | 177.9 | `49a59bc9ffb80d3e92e0ca59d00d3a6ffd83f1c010f4ceca72c0c7d9bae71a40` |
| Linux | x64 | CUDA 12.8 runtime | `cudart-llama-b11829-bin-ubuntu-cuda-12.8-x64.tar.gz` | 594.4 | `f359ce69496da8b7afb0526e21a02bf319f2eb9937a514900b6d368dc9afc116` |
| Linux | x64 | CUDA 13.4 | `llama-b11829-bin-ubuntu-cuda-13.4-x64.tar.gz` | 158.8 | `028784adcad34582e233e99f4b63316f646a5f675b3bc325ba1953ad369164fd` |
| Linux | x64 | CUDA 13.4 runtime | `cudart-llama-b11829-bin-ubuntu-cuda-13.4-x64.tar.gz` | 440.2 | `78ef25824f2c4b9b9872de684f1f723e8c12adad679193f837f442fe933a7cc6` |
| Linux | x64 | ROCm 10.0 | `llama-b11829-bin-ubuntu-rocm-10.0-x64.tar.gz` | 249.9 | `2fecff1da15c5073e16d65f9fbeec8e9e0a22a596456185a20afa31fb8e363a4` |
| Linux | x64 | OpenVINO 2026.4.1 | `llama-b11829-bin-ubuntu-openvino-2026.4.1-x64.tar.gz` | 121.9 | `103d2bf01794955c2320723d8045365d854b13306acb0a75bda6f4bcb98491c7` |
| Linux | x64 | SYCL fp16 / fp32 | `llama-b11829-bin-ubuntu-sycl-fp16-x64.tar.gz` / `…-sycl-fp32-x64.tar.gz` | 63.7 / 63.6 | `9b1ce1b191b554d196db1acbd414557e5d63e619a318d9d07f8c103175e9fab2` / `529b4e07a3c6bd51a9d488593a743888c0e5b5e6cf2d44c3cf64cdbba6061678` |
| Linux | arm64 | CPU | `llama-b11829-bin-ubuntu-arm64.tar.gz` | 18.0 | `dbb878830da146b4cf131b869107cd25b313994f85c4e112fcb00b3739d0a2e0` |
| Linux | arm64 | Vulkan | `llama-b11829-bin-ubuntu-vulkan-arm64.tar.gz` | 28.3 | `2039f7ec5cd88e068db6f809b3c19be0b7a9db0daf4ba5a5a9bee345704865a0` |
| Linux | arm64 | CUDA 13.4 (+ runtime) | `llama-b11829-bin-ubuntu-cuda-13.4-arm64.tar.gz` (+ `cudart-…-arm64.tar.gz`, 552.5 MB) | 153.9 | `2ffc932df1f0072f9e4b5f26e2fa2ab94c86fdbb55d70a7244f19cbc682aecd4` (runtime `10044b7eeded22764788ffbfeb01c60f9cdbd03d19c5961600fe0db7ff98475e`) |
| macOS | arm64 | CPU + Metal (ASSUMPTION: Metal built in, as in upstream builds) | `llama-b11829-bin-macos-arm64.tar.gz` | 16.2 | `e006ea32abe757c10a93718d246796b9bb7eafd309398927ff7b69139e4ba27e` |
| macOS | x64 | CPU | `llama-b11829-bin-macos-x64.tar.gz` | 11.9 | `e4d34d663b44bcae30690f2cc62e1a7837f3d3dafe82b77ca3307d7b6ebdaea4` |
| Windows | x64 | CPU | `llama-b11829-bin-win-cpu-x64.zip` | 24.1 | `c534f35ce9c842f3bc224fadaecc82e1b49768f63e8320922f6dbd603f66395e` |
| Windows | x64 | Vulkan | `llama-b11829-bin-win-vulkan-x64.zip` | 36.9 | `839ff55ce9cbee6a099b403b640ca6b694ee5d74b815e39f77fc5d2b20962f0d` |
| Windows | x64 | CUDA 12.4 (+ runtime) | `llama-b11829-bin-win-cuda-12.4-x64.zip` (+ `cudart-llama-bin-win-cuda-12.4-x64.zip`, 391.4 MB) | 272.3 | `fe65d813d828eb3af2016e0c28ccb14ea12ae7be153e28d1b5044b29b8481def` (runtime `8c79a9b226de4b3cacfd1f83d24f962d0773be79f1e7b75c6af4ded7e32ae1d6`) |
| Windows | x64 | CUDA 13.4 (+ runtime) | `llama-b11829-bin-win-cuda-13.4-x64.zip` (+ `cudart-llama-bin-win-cuda-13.4-x64.zip`, 423.5 MB) | 159.9 | `838cd16df87fd9de11e044629da88fb872a875298dfdb20b246128926819301c` (runtime `738f8c251ac22b70c3ae6f83a10cf222725df0395246a2cf58f32bdb85fbe668`) |
| Windows | x64 | ROCm 10.0 / OpenVINO / SYCL | `llama-b11829-bin-win-rocm-10.0-x64.zip` / `…-win-openvino-2026.4.1-x64.zip` / `…-win-sycl-x64.zip` | 263.5 / 104.0 / 155.0 | `fa1c7a9e4f4c30dfcf5a702fbaa3229eb0b0859523a3e21daa12e7d0b884f105` / `5a7d3af408d18cb8acf4c6d5cb58a865febf3769ed464dfbed985195e514e600` / `db37f1ef45a70c086791587afe18ad359b58b24d1edcfdba69d8d7cf2680c22f` |
| Windows | arm64 | CPU | `llama-b11829-bin-win-cpu-arm64.zip` | 12.7 | `82efa8a2a55bc4b57895465bd95d2979822b9da13938f95577460a5d36828141` |
| Windows | arm64 | Vulkan | `llama-b11829-bin-win-vulkan-arm64.zip` | 25.1 | `0c74d5fe4d2a44512dd734ba2756c1062bc7c0b52c3b0149075236c43506eb47` |
| Windows | arm64 | CUDA 13.4 (+ runtime) | `llama-b11829-bin-win-cuda-13.4-arm64.zip` (+ `cudart-llama-bin-win-cuda-13.4-arm64.zip`, 153.3 MB) | 147.6 | `f5c7e2d575774595b1d45989527ac49a3c0e7ef59050ea41500916e79ad84a8f` (runtime `642dcde8805b3e3165ca710a5443b3b4044b27d96bd3ee3132473988c9bcb774`) |

Also in the release, not desktop targets: Android, Linux s390x, Snapdragon/Adreno builds, an Apple xcframework and the web UI. The Windows CUDA runtime archives have no build number in their names, and their digests are the same in `b11669` and `b11829`.

Findings:

- **R3 is retired.** Linux x64 has CUDA 12.8 and 13.4 builds, plus Vulkan, ROCm, SYCL and OpenVINO. Linux arm64 has CUDA 13.4 and Vulkan too.
- **CUDA builds need a second archive**, the CUDA runtime (440–594 MB on Linux), unless CUDA is already installed. Vulkan (35 MB) is the smaller GPU option on Linux and Windows.
- **Checksums are in the release metadata**, so a user who installs from the releases page can verify the archive.
- **Which build to require.** Builds are now pre-releases, and `vX.Y.Z` releases point to a build through `nightly-tag.txt`. ASSUMPTION: the minimum build is one a `vX.Y.Z` release names (now `b11669`), not the newest nightly.
- **Outcome (gap A1, decided 2026-10-23):** the app does not download `llama-server`. llama.cpp is a user-installed prerequisite with a minimum version, and this matrix becomes install guidance (ADR-0015).
- Whether every archive holds `llama-server` was not checked (no download). Upstream archives have included it for years. It is checked in M1, when the sidecar is first run.

---

## 17. Inputs for ADR-0014 (OCR and M0 exit)

- **OCR engine: Tesseract** is the only candidate that passes every OCR criterion: 98.5% clean-tier accuracy, 100% clean-tier identifier recall, a 100% round-trip over found mentions, per-word confidence, 0.26 s per page, and +8.7 MB. It covers 125 languages through per-language files. It is native: a C++ build with cmake.
- **ADR-0013's default order** says pure Rust first. ocrs, the pure-Rust candidate, fails word confidence (US-02.2 AC3, INV-ID-03), passes accuracy only narrowly (96.9%), reads only the Latin alphabet, and its models are CC-BY-SA-4.0. It is the fallback if a native build becomes a problem, but only after it gains confidences.
- **PP-OCRv5** is the most accurate (99.2% clean, 82.6% poor) and the strongest on poor scans. It fails the round-trip on loose detector boxes, which an adapter change may fix. It is also the heaviest: +43.9 MB, 177 crates and a prebuilt runtime download. Worth a second look if poor-scan quality becomes the bottleneck (M3).
- **If Tesseract is chosen,** the build must not download unverified sources. ASSUMPTION: `mk-parsers` builds Tesseract and Leptonica from source archives pinned by SHA-256 (its own build script, or `tesseract-rs` with its download step replaced). `eng.traineddata` (4.1 MB, `tessdata_fast`) is bundled with the app. Other languages are downloaded on demand from a pinned, checksum-verified source, which adds a host (gap A1).
- **Shared OCR path for `mk-parsers`:** the `OcrEngine` trait, the image guard (format by magic bytes, 100 MP / 512 MiB before decoding, JPEG end marker, PDF image `/Width × /Height`), the 300 DPI hayro render of scanned pages, the OCR layout and `OcrGeometry`.
- **Size for NFR-RES-07 (Linux):** hayro 8.8 MB + shared OCR path 1.3 MB + Tesseract 4.6 MB + `eng.traineddata` 4.1 MB = **18.8 MB** of the 150 MB budget, before the shell and the models.
- **Download hosts (gap A1):** the PDF engine and Tesseract need none at run time. A1 was decided on 2026-10-23: the app downloads no binaries, and `llama-server` is user-installed (ADR-0015). Where extra OCR language files come from is decided in M3.
- **Decided:** ADR-0014 (hayro, Tesseract) and ADR-0015 (user-installed `llama-server`), both Accepted.
- **Follow-up timing (decided 2026-10-24):** M2a builds Tesseract 5.5.2 and Leptonica 1.87.0 from the pinned archives in `crates/mk-tesseract-sys` (its own `build.rs`, system `curl`, SHA-256 checked; `tesseract-rs` is not used) and links them, with one smoke test. The hayro adapter, the shared OCR path and removing `spikes/pdf-bakeoff` move to M3 (roadmap M3). The ADR-0014 decision is unchanged.
