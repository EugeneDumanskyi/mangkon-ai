# ADR-0014: PDF and OCR engines: hayro and Tesseract

- **Status:** Accepted
- **Date:** 2026-10-23
- **Related:** supersedes ADR-0013; NFR-PERF-20/21, NFR-QA-06/07/12, NFR-RES-07, NFR-MNT-01, NFR-USE-03, NFR-LEG-02/05, NFR-REL-06/07, NFR-SEC-02/11, INV-CO-03/04, INV-ID-03, INV-WS-06, US-02.1, US-02.2, US-08.1, brief (Rust-native parsing by default), spike S0, ADR-0002, ADR-0007, ADR-0011; numbers in `08-engine-bakeoff.md`

## Context

- ADR-0013 fixed the rules and the M0 test (spike S0) and left the PDF and OCR engines open. M0b measured the PDF candidates, M0c the OCR candidates (`08-engine-bakeoff.md` §5, §13).
- The brief says Rust-native parsing by default, to be confirmed in M0. It names the OCR choice as open: Tesseract or `ocrs`.
- ADR-0013's order of preference: pure Rust first, native only if no pure-Rust candidate passes.
- The same PDF engine must give the stored text, the character boxes and the page render (ADR-0011, ADR-0013 rule 3).
- OCR locators are page + word boxes + span (INV-CO-03). Mention confidence comes from OCR word confidence (INV-ID-03, US-02.2 AC3). The viewer highlights word boxes (US-08.1 AC2).
- A workspace can choose its OCR languages (INV-WS-06).
- Build: one documented command per OS, no Python, ≤ 15 min (NFR-MNT-01, NFR-USE-03). App ≤ 150 MB without models (NFR-RES-07). Licenses AGPL-compatible (NFR-LEG-02).
- Network: only model download hosts and user-set endpoints (NFR-SEC-02). Gap A1 was decided in M0c: the app downloads no binaries at run time.
- Measured on Linux (RM-1) only. macOS and Windows builds are not verified here.

## Options considered

**PDF** (TD-1 text PDFs and the 15 TD-3 PDFs; `08-engine-bakeoff.md` §5)

| Criterion (pass mark) | hayro 0.8 (pure Rust) | pdfium (prebuilt C++) | pdf_oxide 0.3 (pure Rust) |
|---|---|---|---|
| Text throughput (≥ 20 pages/s) | ✅ 2,594 | ✅ 1,416 | ✅ 2,992 |
| Page view p95, 500-page PDF (≤ 1 s) | ✅ 6 ms | ✅ 6 ms | ✅ 9 ms |
| Highlight round-trip, 200 samples (≥ 99%) | ✅ 100% | ✅ 100% | ❌ 95.5% |
| TD-3 outcomes as expected | ✅ 14/14 | ✅ 14/14 | ❌ 13/14 |
| Clean build (≤ 15 min) | ✅ 19 s | ✅ 12 s | ✅ 58 s |
| Size added | 8.8 MB | 12.2 MB (with `libpdfium`) | 16.3 MB |
| Native code or download | none | prebuilt library per platform, a download host | none |

MuPDF was not measured (not in the M0b row).

**OCR** (TD-1 scans and the 7 TD-3 images; `08-engine-bakeoff.md` §13)

| Criterion (pass mark) | Tesseract 5.5.2 (C++, built from source) | `ocrs` 0.13 (pure Rust) | PP-OCRv5 on ONNX Runtime |
|---|---|---|---|
| Speed, slowest clean A4 page, 1 thread (≤ 5 s) | ✅ 0.26 s | ✅ 1.65 s | ✅ 1.32 s |
| Accuracy, clean tier (≥ 95%) | ✅ 98.5% | ✅ 96.9% | ✅ 99.2% |
| Identifier recall, clean tier (≥ 85%) | ✅ 100% | ✅ 90.5% | ✅ 100% |
| Scan round-trip, found mentions (≥ 99%) | ✅ 100% | ✅ 100% | ❌ 93.9% |
| TD-3 images as expected | ✅ 7/7 | ✅ 7/7 | ✅ 7/7 |
| Word confidence (needed) | ✅ per word | ❌ none in the API | ✅ per line |
| Clean build (≤ 15 min) | ✅ 43 s; needs cmake and a C++ compiler | ✅ 22 s | ✅ 36 s; downloads ONNX Runtime at build time |
| Size, binary + model files | 8.7 MB | 18.8 MB | 43.9 MB |
| Licenses | Apache-2.0, BSD-2-Clause | models CC-BY-SA-4.0 | MIT, Apache-2.0 |
| Languages | 125 + 37 scripts, one file each | Latin alphabet only | 12 script groups |
| Accuracy, poor tier (diagnostic) | 67.1% | 74.3% | 82.6% |

## Decision

Use **hayro for PDF** and **Tesseract for OCR**.

hayro passes every PDF criterion and is pure Rust, so ADR-0013's order picks it over pdfium. Tesseract is the only OCR candidate that passes every criterion. `ocrs`, the pure-Rust candidate, fails word confidence, which INV-ID-03 needs. So the order moves to the native option, as ADR-0013 allows.

**Carried over from ADR-0013**
- Pure Rust crates for CSV, JSON, MBOX/MIME, DOCX, HTML, TXT/MD.
- Engines sit behind `PdfEngine` and `OcrEngine` in `mk-parsers` (ADR-0002). Engines run only in parser workers (ADR-0007).

**PDF: hayro**
- `hayro` renders the page; `hayro-interpret` with a glyph-collecting device gives the characters and their boxes. The same engine does both (ADR-0011).
- Text comes from the shared layout step: lines, fragments, XY-cut, `\n` between fragments (`08-engine-bakeoff.md` §4.1).
- Locator: `Locator::PdfText { page, span }`. Boxes are rebuilt at render time through `PageGeometry::highlight`. A changed text gives `HashMismatch`, which the UI shows (INV-CO-04).
- The pdfium adapter is not carried into `mk-parsers`. The trait keeps a swap possible.

**OCR: Tesseract**
- Tesseract 5.5 and Leptonica, compiled from source into the parser worker and linked statically. Only `libstdc++` is added at run time on Linux.
- The source archives are pinned by version and SHA-256 in the repo. The build never downloads anything unpinned. The build script that ships with `tesseract-rs` 0.4 downloads its sources and the `tessdata_best` files from GitHub without checksums. It is replaced, or patched to use the pinned archives. ASSUMPTION: a small build script of our own is simpler than patching.
- Build prerequisites: cmake and a C++ compiler, documented next to the Rust toolchain (NFR-MNT-01). No Python.
- English data, `tessdata_fast` `eng.traineddata` 4.1.0 (4.1 MB), ships with the app and is pinned by SHA-256 (NFR-SEC-11). Where other languages come from is decided in M3 with the OCR language settings (INV-WS-06).
- One `TessBaseAPI` per worker, loaded once (ADR-0007 long-lived workers). Page segmentation and engine mode stay at Tesseract's defaults. Source resolution is set to the DPI of the image.
- Word confidence: Tesseract's 0–100 becomes 0.0–1.0. A mention's confidence is the lowest word confidence over its span (INV-ID-03). ASSUMPTION: minimum, not mean.

**Shared OCR path in `mk-parsers`** (engine-independent)
- The format is chosen by magic bytes, not the extension. PNG, JPEG and TIFF are decoded by the `image` crate to 8-bit grey.
- **Image guard, before decoding:** refuse images over 100 MP or a 512 MiB allocation, JPEGs without an end-of-image marker, and PDF image XObjects whose declared `/Width × /Height` exceed the same cap. Error kind `TooLarge` or `Corrupt`, with a plain reason. ASSUMPTION: the caps are spike values.
- PDF pages go to OCR when they are scanned, or are pages of a mixed PDF with fewer than 20 characters (US-02.2 AC1). hayro renders them at 300 DPI.
- OCR layout: engine lines are split at gaps, ordered by the same XY-cut, and joined with a space and `\n`. Each word keeps its pixel box, span and confidence in `OcrGeometry`.
- Locator: `Locator::OcrText { page, span, boxes, image }`. The word boxes are stored at OCR time, so a highlight needs no re-OCR (ADR-0011). `image` is the pixel size of the OCR'd image, so the viewer can scale. Highlights are whole word boxes (US-08.1 AC2).

**Limits**
- No engine caps a decompression bomb by itself (`bomb-double-flate.pdf`). The worker memory and time limits are required (ADR-0007, NFR-REL-06/07).
- The image guard stops image bombs before any engine sees a pixel.

**Notices**
- `THIRD_PARTY_NOTICES` lists the hayro crate closure, Tesseract (Apache-2.0), Leptonica (BSD-2-Clause) and `tessdata_fast` (Apache-2.0) (NFR-LEG-05).

## Consequences

**Easier**
- PDF parsing, geometry and rendering stay pure Rust, with no download host.
- Neither engine downloads anything at run time. Gap A1 needs no host for them.
- The engine share of NFR-RES-07 is small on Linux: hayro 8.8 MB + shared image path 1.3 MB + Tesseract 4.6 MB + `eng.traineddata` 4.1 MB = 18.8 MB.
- New OCR languages are one data file each.
- Scan highlights come from stored boxes, so they cannot drift when the engine changes.

**Harder**
- The build needs cmake and a C++ compiler. The C++ part builds in under a minute on RM-1, but the Windows and macOS builds are not verified.
- The pinned Tesseract and Leptonica sources are ours to bump and check.
- hayro is young (0.8). Its text extraction is our adapter (about 200 lines), and its boxes use a fixed vertical band instead of per-font ascent and descent.
- Tesseract is the weakest candidate on poor scans (67.1% vs 82.6% for PP-OCRv5). Poor scans are not under a pass mark, but users have them.

**Follow-ups**
- M2a: build the hayro adapter, the shared OCR path and the Tesseract build from pinned sources in `mk-parsers`. Add `cargo-deny` and generate `THIRD_PARTY_NOTICES`. Remove `spikes/`.
- M2c: worker memory and time limits; `bomb-double-flate.pdf` must fail with a plain reason.
- M3: OCR language settings (INV-WS-06) and where language files come from. Image clean-up before OCR (deskew, binarise) and the `tessdata_best` model, measured on the poor tier. Check NFR-PERF-11 with OCR in Stage A.
- macOS and Windows: build Tesseract and run the S0 numbers when those platforms are tested (platform scope: Linux first).
- Fallbacks: pdfium for PDF. `ocrs` for OCR once it gives word confidence. PP-OCRv5 if poor-scan quality becomes the bottleneck, after its boxes are fixed.
