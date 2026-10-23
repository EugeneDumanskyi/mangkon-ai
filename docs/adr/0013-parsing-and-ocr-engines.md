# ADR-0013: Parsing and OCR engines

- **Status:** Superseded by ADR-0014
- **Date:** 2026-10-12
- **Related:** NFR-PERF-20..23, NFR-QA-05/06/07, NFR-RES-07, NFR-USE-03, NFR-MNT-01, NFR-LEG-02/05, NFR-COMP-06, INV-CO-03, INV-WS-06, brief (Rust-native parsing by default), ADR-0007, ADR-0011

## Context

- The brief says: Rust-native parsing by default, to be confirmed in M0.
- Targets: text-PDF ≥ 20 pages/s (NFR-PERF-20); OCR ≤ 5 s per A4 page per core (NFR-PERF-21); OCR character accuracy ≥ 95% on clean 300 DPI scans (NFR-QA-07).
- Every segment needs an exact locator (INV-CO-03). For PDFs that means page + span; for OCR, page + word boxes + span.
- The page view needs **character boxes and page rendering from the same engine** that extracted the text (ADR-0011).
- A project can choose its OCR languages (INV-WS-06). So OCR quality in more than one language matters.
- Build: one command, no Python, ≤ 15 min (NFR-MNT-01, NFR-USE-03). Binary ≤ 150 MB (NFR-RES-07). Licenses compatible with AGPL-3.0 (NFR-LEG-02).
- Engines run inside parser workers (ADR-0007), so native code is acceptable from a safety point of view. The cost is build complexity.

## Options considered

**Already settled (pure Rust, low risk)**

| Format | Crates (candidates) |
|---|---|
| CSV | `csv` (streaming) |
| JSON | `serde_json` (streaming deserializer) |
| MBOX / email / MIME | `mail-parser`, plus a streaming MBOX splitter |
| DOCX | `zip` + `quick-xml` |
| HTML | `html5ever` / `scraper` → text with offsets |
| TXT / MD | plain reader with `encoding_rs` for UTF-16 and Windows-1252 (NFR-COMP-06) |

**PDF (text, geometry, rendering)**

| Option | ➕ | ➖ |
|---|---|---|
| pdfium (through `pdfium-render`) | Mature. Text with character boxes. Fast, high-quality rendering. Permissive license. | C++ library. Prebuilt binaries must be downloaded or bundled (adds size and a download host). |
| MuPDF | Excellent text and rendering. Its AGPL license is compatible with this project. | C library. Harder build on Windows. Ties future license options to AGPL. |
| Pure Rust stack (text extraction crates + a Rust renderer) | No native build. Matches the brief. | Younger. Text order, ligatures, and rendering quality are less proven. Must be benchmarked. |

**OCR**

| Option | ➕ | ➖ |
|---|---|---|
| Tesseract | Many languages. Well known. Apache-2.0. | C++ with Leptonica. Hard to build on Windows. Breaks "one command" unless prebuilt. |
| `ocrs` (pure Rust) | No native build. Easy to ship. | Young. Fewer languages (mainly Latin script at the time of writing — check in M0). |
| ONNX-based OCR models (for example PaddleOCR) through ONNX Runtime | Strong accuracy, many languages. | ONNX Runtime is a large native library. Model files to download. |

## Decision

**Not final.** The M0 spike decides. This ADR fixes the rules and the test.

**Fixed now**
1. Pure Rust crates for CSV, JSON, MBOX/MIME, DOCX, HTML, TXT/MD (table above).
2. PDF and OCR engines sit behind the `PdfEngine` and `OcrEngine` traits in `mk-parsers` (ADR-0002). Swapping an engine does not touch the core.
3. The **same** PDF engine does extraction and page rendering (ADR-0011).
4. Engines run only in parser workers (ADR-0007).

**M0 test (on TD-1 and TD-3)**

| Criterion | Pass mark |
|---|---|
| PDF text throughput | ≥ 20 pages/s (NFR-PERF-20) |
| OCR speed | ≤ 5 s per A4 page per core (NFR-PERF-21) |
| OCR accuracy | ≥ 95% characters on clean scans (NFR-QA-07) |
| Identifier recall on OCR'd files | ≥ 85% (NFR-QA-06) |
| Character boxes + page render | Available, ≤ 1 s for one page of a 500-page PDF |
| Build | Still one command, clean build ≤ 15 min, no Python |
| Size | App ≤ 150 MB without models |
| TD-3 | Every bad file fails alone with a plain reason |
| License | AGPL-compatible, listed in `THIRD_PARTY_NOTICES` |

**Default order of preference** (take the first that passes): pure Rust → pdfium (PDF) / Tesseract or ONNX OCR (OCR), with prebuilt binaries downloaded and checksum-verified like the model runtime (ADR-0008).

## Consequences

- Until M0 ends, NFR-PERF-21, NFR-QA-07, and NFR-RES-07 stay placeholders (NFR §13).
- If a native engine wins, its binary joins the catalog download flow and the network host list (gap A1).
- This ADR will be **superseded** by a new ADR that records the chosen engines and the M0 numbers.
