# ADR-0011: Source viewer — stored text first, page view from the same engine

- **Status:** Accepted
- **Date:** 2026-10-12
- **Related:** NFR-PERF-14, NFR-QA-12, NFR-SEC-09/10, NFR-USE-04, INV-CO-03/04, US-08.2, US-08.4, US-08.5, US-08.6, domain §3.4, ADR-0003, ADR-0007, ADR-0014

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
- The highlight and any links are split into runs in the core; the UI never maps offsets.
- "Mention N of M" with Previous and Next moves between the value's mentions in the same document. Back returns to the previous connection, mention or document with the scroll of each pane (US-08.2).

**Page view (PDF and OCR'd images)**
- The core asks a worker to `RenderPage` (ADR-0007) with the **same PDF engine** used for extraction (ADR-0014). The worker returns a page image + character boxes for that page.
- OCR'd pages already have word boxes in the locator (INV-CO-03). The page image is the original image, scaled.
- Highlight boxes are drawn over the image in the UI.
- Rendered pages are cached in `cache/pages/` (derived, size-capped, LRU). ASSUMPTION: 500 MB cap.
- If the page's file is changed (hash differs), the viewer opens the text view.

**Original file (US-08.5, US-08.6)**
- Every view, of every format, checks the original by a stat: gone → "Original file not found at <path>"; size or modified time differs → "The file changed since it was read: showing the stored text". Either way the view shows the stored text. A page view also checks the hash, as above. Text formats are not hashed on a click: hashing a 2 GB MBOX would break NFR-PERF-14. ASSUMPTION: size and modified time are enough to say "changed" for text formats.
- An attachment's original is its email's file.
- "Show in folder" and "Open with default app" are Rust commands that take a document id, never a path, so the UI cannot open any other file. They start the system's own tool with arguments and no shell: `xdg-open`, `open` or `explorer` to open; the file manager's `org.freedesktop.FileManager1.ShowItems` over D-Bus (the parent folder if that fails), `open -R` or `explorer /select,` to show. No opener or shell plugin (ADR-0003). Both are off when the original is not found.

**Safety**
- HTML and email bodies are **never** rendered as live HTML in v0.1. They are converted to text during parsing (gap A8, confirmed 2026-10-29).
- The CSP from ADR-0003 blocks remote images, frames, and scripts anyway, as a second layer.
- Links are the URLs (http, https, mailto) found in the displayed text; HTML hrefs are not kept in v0.1. A link is display only: it creates no identifier (M4).
- A link is a button, not an `<a href>`. A click shows the full URL in a confirm dialog with Cancel as the default. Open calls a Rust command that checks the URL again (http, https or mailto; no control characters; ≤ 2,048 characters) and hands it to the system browser or mail app. The navigation guard stays the backstop.

## Consequences

**Easier**
- NFR-QA-12 depends on our own data, not on two engines agreeing.
- NFR-SEC-09 holds by design.
- INV-CO-04 holds: the text view never needs the original.

**Harder**
- Page view needs a worker round trip. Must stay ≤ 1 s on a 500-page PDF (open the document once, render one page).
- The PDF engine must give character boxes and render pages. This is a hard requirement for ADR-0014.
- Styled emails look plain in v0.1.

**Follow-ups**
- `RenderPage` on the 500-page TD-1 ledger: ≈ 28 × under its share of NFR-PERF-14 (M3b, `11-ingestion.md` §6). Done.
- NFR-QA-12: 200 of 200 seeded clicks on TD-1 highlight the right text in the core test, and 108 of 108 clicks in the release app (M3d, `11-ingestion.md` §9). Done.
- NFR-PERF-14 in the release app, click to highlight: p95 83 ms on TD-1, 36 ms on TD-2; 48 ms for row 999,990 of a 1M-row CSV and 27 ms for a message near the end of a 2 GB MBOX (M3d, `11-ingestion.md` §9). Done.
- The 500-page ledger has no email address, so no mention to click on its late pages until M4 adds other identifier types.
