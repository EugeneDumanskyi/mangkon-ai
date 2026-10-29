-- M3a: locators of the new text formats (INV-CO-03). `loc_ref` names the place for the kinds
-- that have no number: the JSON Pointer of a `json_value`, the header name of an
-- `email_header`, the MIME part number of a `mime_part`. `loc_pos` holds the paragraph of a
-- `docx_paragraph` and the text offset of an `html_text` (0 for the named kinds).
-- `document.parent_id` (since 0001) links an attachment to its message (INV-CO-05).
ALTER TABLE segment ADD COLUMN loc_ref TEXT;
