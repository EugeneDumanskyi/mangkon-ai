# ADR-0011: Source viewer — stored text first, page view from the same engine

- **Status:** Proposed
- **Date:** 2026-10-12
- **Related:** NFR-PERF-14, NFR-QA-12, NFR-SEC-09/10, NFR-USE-04, INV-CO-03/04, US-08.4, US-08.5, domain §3.4, ADR-0003, ADR-0007, ADR-0013

## Context

- One click from any connection, search result, or evidence item opens the source with the text highlighted (NFR-USE-04).
- It must open in ≤ 1 s p95, even for a 500-page PDF, a 1M-row CSV, or one message in a large MBOX (NFR-PERF-14).
- The highlight must land on the right text in ≥ 99% of clicks (NFR-QA-12).
- Every span lies inside the segment's **stored** text, so the highlight works even when the original file is gone (INV-CO-04).
- Viewing emails and HTML must cause 0 network requests and 0 script runs (NFR-SEC-09).
- The source viewer is a screen, not a domain context (domain §3.4).

## Options considered

**1. Open the original file in web viewers (pdf.js for PDF, iframe for HTML/email).**
- ➕ Looks like the original.
- ➖ pdf.js extracts text its own way. Our spans come from our parser. Mapping one to the other fails on ligatures, hyphens, and reading order. Puts NFR-QA-12 at risk.
- ➖ Rendering live HTML/email needs a carefully locked-down frame. A mistake loads remote images (tracking pixels).
- ➖ Does not work when the original file is missing.

**2. Show stored text only, with highlights.**
- ➕ Always correct. Always safe. Always fast. Works when the file is missing.
- ➖ Loses the page look for PDFs and scans. Users want to see the real page, especially for scans.

**3. Hybrid: stored text for every type + a page view for PDFs and scans, rendered by the same engine that extracted the text.**
- ➕ Highlights come from the same engine's geometry, so they match.
- ➕ Text view is always available as a safe fallback.
- ➖ Page rendering needs the original file and a worker round trip.

## Decision

Use **option 3**.

**Text view (all types, default for non-page formats)**
- Built from stored segments around the target locator, with the span highlighted.
- TXT/MD: lines. DOCX: paragraphs. HTML: extracted text. CSV: virtualized grid of row segments, scrolled to the row, cell highlighted. JSON: tree view at the JSON path. Email: headers + body text, with the header or MIME part highlighted.
- Large sources load only the window around the target (keyset paging), so a 1M-row CSV opens as fast as a small one.

**Page view (PDF and OCR'd images)**
- The core asks a worker to `RenderPage` (ADR-0007) with the **same PDF engine** used for extraction (ADR-0013). The worker returns a page image + character boxes for that page.
- OCR'd pages already have word boxes in the locator (INV-CO-03). The page image is the original image, scaled.
- Highlight boxes are drawn over the image in the UI.
- Rendered pages are cached in `cache/pages/` (derived, size-capped, LRU). ASSUMPTION: 500 MB cap.
- If the original file is missing or changed (hash differs), the viewer opens the text view with a note: "Original file missing — showing stored text".

**Safety**
- HTML and email bodies are **never** rendered as live HTML in v0.1. They are converted to text during parsing (gap A8, confirmed 2026-10-29).
- The CSP from ADR-0003 blocks remote images, frames, and scripts anyway, as a second layer.
- Links inside documents are shown as text. Opening one needs a click and a confirm, and it opens in the system browser.

## Consequences

**Easier**
- NFR-QA-12 depends on our own data, not on two engines agreeing.
- NFR-SEC-09 holds by design.
- INV-CO-04 holds: the text view never needs the original.

**Harder**
- Page view needs a worker round trip. Must stay ≤ 1 s on a 500-page PDF (open the document once, render one page).
- The PDF engine must give character boxes and render pages. This is a hard requirement for ADR-0013.
- Styled emails look plain in v0.1.

**Follow-ups**
- M0: measure `RenderPage` time on the largest TD-1 PDF.
- Semi-automated highlight check (200 random clicks) in the benchmark (NFR-QA-12).
