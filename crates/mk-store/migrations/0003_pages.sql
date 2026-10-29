-- M3b: PDF pages and OCR'd pages (US-02.1, US-02.2, INV-CO-03). `loc_pos` holds the 1-based page
-- of a `pdf_text` or `ocr_text` segment. An `ocr_text` segment keeps its word geometry in `ocr`
-- (image size, then each word's span, box and confidence; layout in `codec.rs`), because OCR
-- is the only source of its boxes (ADR-0011). A mention keeps only its span; its boxes are the
-- boxes of the words the span touches.
--
-- `project_meta` key `ocr_languages` (written by the app, default `eng`, INV-WS-06) names the
-- project's OCR languages, `+`-joined (US-02.2 AC4).
ALTER TABLE segment ADD COLUMN ocr BLOB;
