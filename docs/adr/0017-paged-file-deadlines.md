# ADR-0017: PDFs and images — deadline per page, 30 s per OCR page

- **Status:** Proposed
- **Date:** 2026-10-29
- **Related:** NFR-REL-06/07, NFR-CAP-04, NFR-PERF-20/21, NFR-PERF-14, US-02.1, US-02.2, US-08.1 AC1–AC2, INV-PR-04, ADR-0007 (amends its time limit for PDFs and images), ADR-0011, ADR-0014, ADR-0016 (same rule for MBOX), M3b

## Context

- ADR-0007 stops a worker 120 s after its file started (NFR-REL-06). ADR-0016 changed this for MBOX only: 120 s from the last finished message.
- NFR-REL-06 also says "OCR: ≤ 30 s per page". Until M3b nothing enforced it.
- NFR-CAP-04 allows a PDF of 2,000 pages. NFR-PERF-21 allows up to 5 s of OCR per page. A 2,000-page scanned PDF can take ≈ 2.8 hours at that floor, and still be honest work.
- A whole-file deadline of 120 s therefore fails every long scanned PDF and every long multi-page TIFF. NFR-REL-06 and NFR-CAP-04 conflict, as NFR-REL-06 and NFR-CAP-06 did for MBOX (ADR-0016).
- Text-native pages are fast: ≥ 20 pages/s (NFR-PERF-20); hayro gave ≈ 2,600 pages/s in M0b (ADR-0014). A 2,000-page text PDF fits a whole-file deadline on RM-1, but not on a machine near the floor (100 s for 2,000 pages, and slower files exist).
- The deadline exists to stop a parser that is stuck (TD-3 `loop-*.pdf`, endless-loop faults). A loop inside one page, or an OCR call that never returns, must still be stopped.
- The page view renders one page on demand (ADR-0011, US-08.1 AC1–AC2). It runs in a worker too, with a mark of ≤ 1 s per page (NFR-PERF-14 share, watch item W3b-1).
- The maintainer decided on 2026-10-29 that the per-page rule goes into a new ADR, and ADR-0016 is accepted as written.

## Options considered

**1. Keep the whole-file deadline for PDFs and images** (do nothing).
- ➕ One rule for every format except MBOX.
- ➖ A long scanned PDF or TIFF fails with "Took too long to read" while it makes progress. That breaks NFR-CAP-04 for scans.
- ➖ The 30 s per OCR page in NFR-REL-06 stays unenforced.

**2. A whole-file deadline scaled by page count** (for example 120 s + 5 s per page).
- ➕ One number per file, known after open.
- ➖ The page count is known only inside the worker, after open. The core would have to trust a number from the process it is guarding.
- ➖ A loop on page 1 of a 2,000-page file runs for ≈ 2.8 hours before it is stopped.

**3. The deadline restarts at each finished page, and a page in OCR has its own 30 s cap** (the ADR-0016 rule, per page).
- ➕ A stuck page is stopped 120 s after the last finished page, or 30 s into its OCR.
- ➕ Total time grows with the file; it is bounded by the page limit (NFR-CAP-04) and the per-page caps.
- ➕ The same supervisor code as MBOX: heartbeat `units` count finished pages.
- ➖ A file of many slow but honest pages can run a long time. It shows progress (the page count).

**4. One work item per page.**
- ➕ Each page gets the normal limits and its own commit.
- ➖ The queue would need parts of files, and INV-PR-04 would hold per page, not per file. The same reasons ADR-0016 rejected it for MBOX.

## Decision

Use **option 3**.

**Stage A (parse)**
- PDF and image files: the 120 s deadline runs from the last finished page, not from the start of the file. The worker's heartbeat `units` count finished pages (`Sink::progress`); the core restarts the deadline when `units` rises (`mk_core::workers`, as for MBOX).
- While a page is in OCR (render or decode, then OCR, then layout), the worker reports the stage `Ocr` in its heartbeats. The core stops the worker when one page stays in `Ocr` for more than 30 s (`WorkerConfig::ocr_page_deadline`).
- Both stops give the same failure as before: "Took too long to read". The file fails; the batch continues (NFR-REL-01).
- Text formats keep the whole-file deadline of ADR-0007; MBOX keeps ADR-0016.
- The silence timeout (15 s) and the memory limit (1 GB per file, NFR-REL-07) are unchanged for every format.
- Over 2,000 pages → "File too large (limit: 2,000 pages)" (NFR-CAP-04). The worker checks it right after open, before any page is read.
- A PDF or image attachment is read inside its email's item, under the email's deadline: whole file for EML, per message for MBOX (ADR-0016). Its pages do not restart that deadline and have no separate OCR cap. ASSUMPTION: attachments are small enough for that; a long scanned attachment can fail its email with "Took too long to read".

**Page view (render)**
- One render worker per open project, started on the first page view, separate from the Stage A lane. It has the same memory limit.
- Each render has a 10 s deadline (`WorkerConfig::render_deadline`). ASSUMPTION: 10 s (W3b-1 measures the real time). A render past it is stopped, and the page falls back to the stored text with a note (ADR-0011).

## Consequences

**Easier**
- Long scans and long text PDFs up to the page limit can finish (NFR-CAP-04).
- NFR-REL-06's 30 s per OCR page is enforced, and tested (`adr_0017_ocr_page_deadline`).
- A loop inside one page is stopped within 120 s, or 30 s if it is in OCR.
- MBOX, PDF and images share one supervisor rule: "deadline per unit of progress".

**Harder**
- Three time-limit rules: whole file, per message, per page with an OCR cap. The supervisor must know which formats count units.
- The total time of a long PDF has no upper limit apart from its page count.
- The 30 s cap trusts the worker's own `Ocr` stage report. A worker that hangs before it reports the stage is caught by the 120 s per-page deadline instead.

**Follow-ups**
- Change NFR-REL-06 to "per file; per message for MBOX (ADR-0016); per page for PDFs and images, ≤ 30 s per OCR page (ADR-0017)" (M3b).
- Tests (M3b): the deadline restart with a `slow-pages` test fault (`adr_0017_pdf_deadline_per_page`), and a page stuck in OCR (`adr_0017_ocr_page_deadline`, test fault `stuck-ocr-page`).
- W3b-1 (M3b, `11-ingestion.md` §6) checks the 10 s render deadline against the measured render time.
- M3c: progress and ETA (US-03.1) can use the same page count.
