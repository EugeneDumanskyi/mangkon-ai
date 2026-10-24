# 07 — Test corpus: Mangkon AI v0.1

> Input: `01-brief.md` §11, `02-requirements.md` US-14.3, `03-nfr.md` §2 (TD-1, TD-3) and NFR-LEG-04, `04-domain.md` (INV-CO-03, INV-CO-05, INV-ID-06, INV-LK-01..03, INV-AI-06/07). **If this file disagrees with them, they win.**
> Status: TD-1 v1 and TD-3 v1 built in M0a. The corpus files are not committed; they are generated and downloaded locally (§9).
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. What this is

The test corpus is the data every benchmark and acceptance check runs against: the M0b/M0c engine bake-off, the identifier and link recall/precision numbers (US-14.3 AC2, NFR-QA-*), and the demo.

| Set | Where | What |
|---|---|---|
| TD-1 synthetic | `testdata/td1/synthetic/` | 81 files written by `tools/corpus-gen` from one story. Fully annotated. |
| TD-1 public | `testdata/td1/public/` | 20 US federal public-domain PDFs, downloaded by `corpus-gen fetch` from the URLs in `tools/corpus-gen/public-manifest.json`. Partly annotated (identifiers per page). |
| Ground truth | `testdata/td1/ground-truth.json`, `testdata/td1/page-text.jsonl` | What every file contains and where. Format in §5. |
| TD-3 v1 | `testdata/td3/` | 21 broken and edge-case PDF/image files plus `expected.json` (§6). |
| Schemas | `crates/mk-bench/schema/*.schema.json` | JSON Schemas for the three stored formats, made from the Rust types. |

**Nothing under `testdata/` is committed** (`.gitignore`). Test data is not a deliverable of this project; anyone who clones the repo builds it in a few seconds (§9) or points the tools at their own files. What is committed is what makes the corpus reproducible: the generator, the public-slice manifest (source URLs, SHA-256, identifiers per page), the schemas, and `tools/corpus-gen/corpus.sha256`, the hash of every generated file. Only the generator writes the synthetic files, the ground truth and TD-3, so the ground truth cannot drift from the files, and the hash list shows any change. TD-2 (scale corpus) comes in M2b and is local too.

Brief §11 asks for the corpus to be published openly so anyone can run the demo and check the numbers. Publishing the generator and the manifest meets that: everyone gets the same bytes. **ASSUMPTION.**

---

## 2. The story

The synthetic files tell one story, so that identifiers, names and money connect across formats the way they do in a real case. The places and people are fictional; Velmora is a made-up port town.

**Velmora Port Authority** runs two tenders for dredging logistics in 2024–2025. Its Director of Procurement, **Daniel Okonkwo-Hart**, steers both to **Brightwater Logistics Ltd**, a company formed weeks before the first tender. Brightwater's sole director is **Irene Vasquez-Lindqvist**, Daniel's sister-in-law. The company was formed by **Kestrel Advisory LLP**, whose managing partner **Tomas Reinholt** also provides company-formation services. Brightwater is owned by **Saltmarsh Holdings S.A.**, an offshore company whose nominee director and shareholder is **Corvin Hale**.

The money moves in a chain: the Port Authority pays Brightwater's invoices (£1.28m); Brightwater pays most of each amount on to Kestrel as "fee notes"; Kestrel pays Saltmarsh a "consultancy retainer"; Saltmarsh buys Bitcoin and USDT from the **Coralline Exchange** OTC desk and pays Daniel in crypto, to addresses he wrote on a sticky note "for the marina".

The key clue is small: from invoice 3 on, the "accounts" phone number on Brightwater's invoices is Daniel's private mobile. **Priya Ramanathan-Cole**, an accounts-payable clerk, notices and reports it to **Rosalind Achebe-Frost**, the Chief Internal Auditor. **Samuel Achterberg**, compliance officer at **Meridian Trust Bank**, flags the account pattern at the same time. **Lena Hartwig** of **The Harbour Post** publishes the story on 2 May 2025.

Background and traps:
- **Northgate Marine Supplies Inc.** is the honest bidder who lost twice. Its director **Gregor Almqvist** has his own identifiers, which must not link to the scheme.
- **Gregory Almquist**, a union spokesman quoted in the press, has a lookalike name but is a different person.
- **Brightwater Logistic Solutions GmbH**, a German firm with a lookalike name, is unrelated.
- Typing errors (an IBAN and a Bitcoin address with a bad checksum, an EVM address with one letter in the wrong case), public test-card numbers and webmail domains appear in the files and must not become identifiers or links.

What a good result shows: Daniel's private phone links the invoices to the staff list; Irene links Brightwater to Daniel; the IBANs and crypto addresses trace the money VPA → Brightwater → Kestrel → Saltmarsh → Daniel; Northgate and the two lookalikes stay apart.

---

## 3. Cast

All names are fictional. Ids are the `entities[].id` values in the ground truth; the aliases there are the name variants the files use (the merge truth for E11).

| Id | Name | Kind | Role |
|---|---|---|---|
| `daniel` | Daniel Okonkwo-Hart | person | Director of Procurement, Velmora Port Authority |
| `irene` | Irene Vasquez-Lindqvist | person | Sole director of Brightwater; Daniel's sister-in-law |
| `tomas` | Tomas Reinholt | person | Managing partner of Kestrel Advisory; formation agent |
| `corvin` | Corvin Hale | person | Nominee director and shareholder of Saltmarsh |
| `priya` | Priya Ramanathan-Cole | person | Accounts-payable clerk who spots the pattern |
| `rosalind` | Rosalind Achebe-Frost | person | Chief Internal Auditor |
| `lena` | Lena Hartwig | person | Reporter, The Harbour Post |
| `samuel` | Samuel Achterberg | person | Compliance officer, Meridian Trust Bank |
| `gregor` | Gregor Almqvist | person | Director of Northgate (honest bidder) |
| `gregory` | Gregory Almquist | person | Union spokesman; lookalike of Gregor, unrelated |
| `vpa` | Velmora Port Authority | org | Public body running the tenders |
| `bwl` | Brightwater Logistics Ltd | org | The shell company that wins |
| `kestrel` | Kestrel Advisory LLP | org | Formation agent; moves money onward |
| `saltmarsh` | Saltmarsh Holdings S.A. | org | Offshore parent; end of the money chain |
| `northgate` | Northgate Marine Supplies Inc. | org | Honest supplier and bidder |
| `meridian` | Meridian Trust Bank | org | Bank of Brightwater and the Port Authority |
| `harbourpost` | The Harbour Post | org | Local newspaper |
| `registry` | Velmora Companies Registry | org | Company register |
| `union` | Velmora Dock Workers Union | org | Trade union |
| `exchange` | Coralline Exchange | org | Crypto OTC desk |
| `bwl_gmbh` | Brightwater Logistic Solutions GmbH | org | Lookalike company, unrelated |

---

## 4. Files

Every file has a one-line `purpose` in the ground truth. By kind:

| Kind | Count | What it covers |
|---|---|---|
| Text PDF (`pdf/`) | 21 | Invoices with tables; a two-column report and article; repeated headers and footers; a `/Rotate 90` landscape page; a content stream out of reading order; identifiers split across lines (`hard`); registry extracts; a **500-page ledger** for M0b render timing (story rows on known pages). Standard-14 fonts with AFM widths, so boxes are exact. |
| Image scan (`scan/`) | 15 | PNG, JPEG and TIFF at three tiers (below): invoices, business cards, a sticky note with crypto addresses, confirmations, letters, a visitor log. |
| Scanned PDF (`scan-pdf/`) | 4 | Image-only pages (Flate and JPEG), one or two pages. |
| Mixed PDF (`mixed/`) | 2 | Text page(s) followed by a scanned page. |
| OCR-layer PDF (`ocr-layer/`) | 2 | Scan with an invisible text layer (render mode 3), one word per box, as OCR tools write. |
| EML (`email/`) | 14 | Plain 7bit, quoted-printable, base64, multipart/alternative, HTML-only, a Windows-1252 part, RFC 2047 subjects, mixed-case addresses, a PDF attachment (child document). |
| MBOX (`mbox/`) | 2 | Three messages each; mboxrd `>From ` quoting; a PDF attachment inside a message. |
| CSV (`csv/`) | 4 | Bank statement; a `;`-separated export with UTF-8 BOM and decimal commas; a Windows-1252 ledger; expense claims with a test card number. |
| JSON (`json/`) | 3 | Two registry exports and an exchange account export. |
| DOCX (`docx/`) | 4 | Paragraphs and a table; a name and an IBAN split across two runs (`hard`). |
| HTML (`html/`) | 3 | News article, company site, team page. |
| TXT / MD (`notes/`) | 7 | Markdown notes; UTF-16LE (BOM) and Windows-1252 copies (NFR-COMP-06); CRLF line ends; lookalike names in one note. |

**Scan tiers.** `clean`: 300 DPI, straight, no noise — the NFR-QA-07 tier. `office`: 300 DPI, skew up to 1.5°, light blur and noise, JPEG quality 75. `poor`: 200 DPI, about 3° skew, low contrast, speckle, blur and noise. Scans are drawn with the Noto fonts (OFL-1.1, in `tools/corpus-gen/fonts/`).

**Annotation.** Synthetic files are **fully** annotated: every printed story name and identifier is a mention with an exact location. The generator fails if a story name or identifier appears anywhere without a mark, and the files print no other person or company names and no other identifier-like strings, so anything else an extractor finds is a false positive. Public files are **partly** annotated (§8).

Ground truth totals in v1, synthetic slice: 890 mentions (467 names, 423 identifiers, 14 of them `hard`), 18 merges, 20 relations, 9 events, 10 negatives. With the public slice: 1,062 mentions and 77 links.

---

## 5. Ground-truth format

`ground-truth.json` (schema: `crates/mk-bench/schema/ground-truth.schema.json`, Rust types: `crates/mk-bench/src/model.rs`). `schema_version` is 1. The loader and checks are `mk_bench::load` and `GroundTruth::validate`.

| Field | Content |
|---|---|
| `generator` | `name`, `version` (`GENERATOR_VERSION`), `seed`. |
| `page_text_file` | Name of the JSON Lines file with the page text (next to this file). |
| `files[]` | `path` (relative to `testdata/td1/`), `sha256`, `bytes`, `origin` (`synthetic`/`public`), `kind`, `pages`, `encoding`, `scan_tier`, `dpi`, `annotation` (`full`/`partial`), `purpose`. |
| `documents[]` | `id`, `file`, `part` (`whole`, `mbox_message {index}`, `attachment {mime_part, filename}`), `kind`, `parent` (attachments only; INV-CO-05), `sha256` of decoded attachment bytes. |
| `entities[]` | The cast: `id`, `kind`, `canonical`, `aliases`, `role`. |
| `mentions[]` | `id`, `document`, `subject` (identifier `key` {`type`, `value`, `pattern`} or `entity` + `name_key`), `raw` (text as printed), `location`, `difficulty` (`normal`/`hard`), `derived_from`, `note`. |
| `negatives[]` | `bad_checksum` (IBAN/BTC/EVM that must not become identifiers), `card_like` (flag only, INV-ID-06), `stop_list_domain` (never a link, INV-LK-03), `distinct_entities` (lookalikes that must not merge). |
| `links[]` | Every subject (identifier key or entity) seen in ≥ 2 documents: the documents and mentions (INV-LK-01/02). |
| `merges[]` | Entities written in ≥ 2 variants, with the variants. |
| `relations[]` | Layer 3 truth (INV-AI-06): `type` (`works_for`, `owns`, `director_of`, `paid`, `sent_to`, `related_to`), `subject`, `object`, `document`, `evidence` (two entity mentions in that document). |
| `events[]` | Layer 3 truth (INV-AI-07): `date` (ISO 8601, partial such as `2025-02`, or `unknown`), `description`, `document`, `evidence`, `quote` (the sentence as printed). |

**Locations** follow the INV-CO-03 kinds, in each format's own terms. Character offsets count Unicode scalar values and are half-open (`start` inclusive, `end` exclusive).

| Kind | For | Meaning |
|---|---|---|
| `pdf_text` | text PDF, mixed (text pages), OCR layer | 1-based `page`, `boxes` in PDF points in the **unrotated** MediaBox, origin top-left, y down. One box per line for a mention split across lines. |
| `scan` | image file, scanned PDF, mixed (scanned pages) | `page`, `dpi`, `boxes` in pixels of the page image: tight glyph bounds, as axis-aligned boxes after skew. |
| `pdf_page` | public files | `page` only (no boxes). |
| `text_line` | TXT, MD | 1-based `line` and offsets in the decoded line (line ends removed). |
| `docx_paragraph` | DOCX | 0-based `paragraph` over every `w:p` in body order (table cells included) and offsets in its text. |
| `csv_cell` | CSV | 0-based `row` (0 is the header) and `column`, offsets in the unquoted cell value. |
| `json_value` | JSON | RFC 6901 `pointer` to a string and offsets in the decoded string. |
| `email_header` | EML, MBOX | `header` name and offsets in the decoded header value (RFC 2047 undone). |
| `mime_part` | EML, MBOX | IMAP-style `part` number (`1`, `1.2`) and offsets in the decoded body text with line ends turned into `\n`. |
| `html_source` | HTML | Offsets in the decoded source text. |

**Identifier keys.** Emails lower-case; phones E.164; IBANs compact upper-case; BTC/SOL/TRON as printed (case matters); EVM lower-case `0x…`; URLs as printed; domains lower-case; custom patterns (`tender_ref`) as printed. Every email and URL mention has a second mention for its **domain**, with the same location and `derived_from` set, because the product derives domains (US-04.3). The domain is the registrable domain (`edis.usitc.gov` → `usitc.gov`), following the US-04.3 ASSUMPTION; the generator uses a short list of two-label suffixes in place of the full Public Suffix List.

**Page text.** `page-text.jsonl` has one JSON object per line (schema: `crates/mk-bench/schema/page-text.schema.json`) for every page of every synthetic PDF and scan: the exact reference text (lines joined with `\n`), page size and unit (`pt` or `px`), `dpi`, `rotate`, `skew_deg`, and every word with its box. NFR-QA-07 (OCR character accuracy) and the M0b locator round trip read it.

**Checks** (`validate`, and test `us_14_3_ac1_ground_truth_valid` in `crates/mk-bench`): ids unique and every reference resolves; location kind fits the document kind; offsets match the raw length; boxes lie inside their page; every identifier passes its checksum and every bad-checksum negative fails it; name keys match; links have ≥ 2 documents; relation and event evidence sits in its document; dates are valid; every file exists with the listed size and SHA-256.

---

## 6. TD-3 v1 (broken-file set)

`testdata/td3/expected.json` (schema: `crates/mk-bench/schema/td3-expected.schema.json`) lists each file with `category`, `expected` outcome and the reason. `fail` means the file fails alone with a plain reason (NFR-USE-05); `success` means it is processed, possibly with zero text. Whatever the outcome, each file finishes within the per-file limits (NFR-REL-06/07) and never stops the batch (NFR-REL-01). v1 covers PDF and images; the brief's ~30 files are reached when M3 adds broken email, CSV and DOCX files.

| File | Category | Expected | What it does |
|---|---|---|---|
| `empty.pdf`, `empty.png` | empty | fail | Zero bytes. |
| `blank-pages.pdf` | blank | success | Three pages, no content. |
| `encrypted-user-password.pdf` | encrypted | fail | RC4-128 (R3) with a user password (`velmora`, for manual checks). |
| `encrypted-owner-only.pdf` | encrypted | success | Owner password only; opens with the empty user password. |
| `corrupt-xref-offsets.pdf` | corrupt | success | Every xref offset is 7 bytes off; objects intact, so the xref is rebuilt. **ASSUMPTION:** rebuilding is expected. |
| `garbage-after-header.pdf` | corrupt | fail | PDF header, then 16 KiB of random bytes. |
| `truncated.pdf` | truncated | fail | Cut inside the first content stream. |
| `truncated.png`, `truncated.jpg` | truncated | fail | Cut in half. **ASSUMPTION:** no partial decode. |
| `pages-2000.pdf` | huge | success | 2,000 text pages (NFR-CAP-04). |
| `pdf-named.png`, `jpeg-named.pdf` | wrong extension | success | Valid files with the wrong extension. **ASSUMPTION:** magic bytes win over the extension. |
| `text-named.pdf` | wrong extension | fail | Plain text named `.pdf`; no magic bytes, so the PDF parser rejects it. **ASSUMPTION.** |
| `bomb-double-flate.pdf` | memory bomb | fail | 3.7 KB; the content stream (two Flate filters) expands to 2 GiB. |
| `bomb-image-dimensions.pdf` | memory bomb | fail | An image-only page that claims 100,000 × 100,000 pixels. The app's guard refuses it before decoding. |
| `png-lying-ihdr.png` | memory bomb | fail | IHDR claims 100,000 × 100,000; 1,000 bytes of data. |
| `png-decompression-bomb.png` | memory bomb | fail | A real 1-bit 60,000 × 60,000 PNG: 0.4 MB on disk, 3.6 GB as 8-bit pixels. |
| `loop-page-tree.pdf` | endless loop | fail | Two `/Pages` nodes that list each other; no leaf pages. |
| `loop-xref-prev.pdf` | endless loop | success | An update xref whose `/Prev` points to itself; objects intact. |
| `loop-form-xobject.pdf` | endless loop | success | A Form XObject that draws itself; the page text sits outside it. |

Checked on the build machine with poppler (`pdfinfo`, `pdftotext`) under a 2 GB memory cap and a 20 s timeout: every file behaved as expected, and the user password opens the encrypted file. poppler draws `bomb-image-dimensions.pdf` scaled down instead of refusing it. Its `fail` describes our guard, not poppler.

---

## 7. Synthetic identifier rules (NFR-LEG-04)

No identifier in the corpus should belong to anyone real.

| Type | Rule |
|---|---|
| Email, URL, domain | RFC 2606 / RFC 6761 names only: `*.example`. Message-IDs use `@localhost`. |
| Phone | Ranges set aside for drama and testing: UK Ofcom `020 7946 0xxx` and `07700 900xxx`, US `555-01xx`, Australian ACMA `0491 570 xxx`. |
| IBAN | Valid check digits with made-up bank codes starting with `Q` (GB, NL, MT). **ASSUMPTION:** no real bank uses these codes, so no real account matches. |
| BTC, EVM, TRON, Solana | Encoded from seeded random bytes (ChaCha20, fixed seed). No private key exists for any of them. |
| Card numbers | Only well-known public test numbers (`4111 1111 1111 1111`, `5555555555554444`). They are negatives: flag only, never identifiers (INV-ID-06). |
| Stop-list domains | `gmail.com` and `outlook.com` appear as bare domain names only, never in an address. |
| Registry and invoice numbers | Free-form (`VC-0418822`, `BWL-2024-001`); not identifiers in v1, except the tender reference (`custom`, pattern `tender_ref`). |

---

## 8. Public slice

`testdata/td1/public/` holds US federal government works, which are in the public domain in the United States (17 U.S.C. §105). They were picked to be about agencies, companies and public officials (NFR-LEG-04). `tools/corpus-gen/public-manifest.json` lists for each file: source URL, license basis, retrieval date, SHA-256, size, kind, page count, and the identifiers on each page. `corpus-gen fetch` downloads the files with `curl` and rejects any whose hash differs; `generate` checks the hashes again. If a source changes or disappears, the file must be replaced in the manifest and re-annotated.

Brief §11 suggested public email sets "e.g. Enron" for messy real email. Those name private people, which NFR-LEG-04 rules out, so the public slice has **no email**. Messy email is synthetic only in v1. **Gap:** M3 revisits real-world email edge cases.

**Partial annotation.** For public files the ground truth lists identifiers (emails, URLs, phone numbers, with the derived domains) per page, with `pdf_page` locations and no boxes. They were found with a pattern search on `pdftotext` output and checked by hand. URLs wrapped across two lines are left out, because their printed form is ambiguous. A phone number split over a line break is recorded with a single space. Names are not annotated. Scoring treats extra finds on these files as unknown, not wrong (M4). **ASSUMPTION.**

| File | Source | Kind | Pages | Identifiers |
|---|---|---|---|---|
| `fr-2025-03344.pdf` | Federal Communications Commission: Broadband Serviceable Location Fabric | pdf_text | 12 | 5 |
| `fr-2025-03514.pdf` | Securities and Exchange Commission: Self-Regulatory Organizations; ICE Clear Credit LLC; Notice of Filing of Proposed Rule Change Relating to the ICC Procedures for Identification of Contract Reference Obligations | pdf_text | 5 | 3 |
| `fr-2025-03520.pdf` | International Trade Commission: Steel Nails From China; Scheduling of an Expedited Five-Year Review | pdf_text | 1 | 3 |
| `fr-2025-03524.pdf` | Employee Benefits Security Administration (Labor): Amendment To Extend Effective Dates for Prohibited Transaction Exemptions: Royal Bank of Canada, Toronto, Canada; Northern Trust Corporation, Chicago, Illinois | pdf_text | 4 | 2 |
| `fr-2025-03525.pdf` | International Trade Commission: Certain Medical Programmers With Printed Circuit Boards; Notice of Commission Determination Not To Review an Initial Determination Terminating the Investigation | pdf_text | 2 | 5 |
| `fr-2025-03531.pdf` | Environmental Protection Agency: Vinyl Chloride; Draft Scope of the Risk Evaluation Under TSCA; Reopening of Comment Period | pdf_text | 2 | 13 |
| `fr-2025-03536.pdf` | Postal Regulatory Commission: New Postal Products | pdf_text | 2 | 3 |
| `fr-2025-03543.pdf` | National Marine Fisheries Service (Commerce): Taking Marine Mammals Incidental to Small Boat Harbor Preconstruction Activities in St. George, Alaska | pdf_text | 21 | 8 |
| `fr-2025-03547.pdf` | Consumer Financial Protection Bureau: Protecting Americans From Harmful Data Broker Practices (Regulation V); Extension of Comment Period | pdf_text | 1 | 5 |
| `fr-2025-03548.pdf` | Federal Aviation Administration (Transportation): Airworthiness Directives; Airbus Canada Limited Partnership Airplanes | pdf_text | 4 | 17 |
| `fr-2025-03561.pdf` | National Oceanic and Atmospheric Administration (Commerce): Magnuson-Stevens Act Provisions; Fisheries Off West Coast States; Pacific Coast Groundfish Fishery Management Plan; Amendment 35; Notice of Availability | pdf_text | 5 | 21 |
| `fr-2025-03566.pdf` | Office of Space Commerce (Commerce): Advisory Committee on Excellence in Space (ACES); Notice of Cancellation | pdf_text | 1 | 8 |
| `fr-2025-03569.pdf` | U.S.-China Economic and Security Review Commission: Notice of Open Public Hearing | pdf_text | 2 | 7 |
| `fr-2025-03598.pdf` | Bureau of Industry and Security (Commerce): Implementation of Duties on Steel Pursuant to Proclamation 10896 Adjusting Imports of Steel Into the United States | pdf_text | 3 | 0 |
| `fr-2025-03613.pdf` | International Trade Administration (Commerce): Certain Softwood Lumber Products From Canada: Preliminary Results of the Administrative Review; 2023 | pdf_text | 4 | 3 |
| `ntrs-19620005134.pdf` | NASA Langley Research Center: Drag Characteristics and Dynamic Stability in Descent of a Rotary Parachute Tested in a Vertical Tunnel (1962) | pdf_ocr_layer | 19 | 0 |
| `ntrs-19640002194.pdf` | NASA Langley Research Center: Description and Initial Calibration of the Langley Hotshot Tunnel with Some Real-Gas Charts for Nitrogen (1963) | pdf_ocr_layer | 59 | 0 |
| `ntrs-19650072761.pdf` | NASA Langley Research Center: A Simplified Method for Estimating Subsonic Lift-Curve Slope at Low Angles of Attack for Irregular Planform Wings (1961) | pdf_ocr_layer | 34 | 0 |
| `scotus-22-448_o7jp.pdf` | Supreme Court of the United States: Consumer Financial Protection Bureau v. Community Financial Services Assn. of America, Ltd., 601 U.S. ___ (2024), slip opinion | pdf_text | 59 | 4 |
| `scotus-22-451_7m58.pdf` | Supreme Court of the United States: Loper Bright Enterprises v. Raimondo, 603 U.S. ___ (2024), slip opinion | pdf_text | 114 | 2 |

Total: 20 files, 13.2 MB, 109 identifiers (plus their derived domains). Retrieved 2026-10-22 with `curl`. Every source must allow plain scripted downloads, since `fetch` runs on every clone; GAO reports were dropped for that reason (gao.gov answers scripted requests with HTTP 403). The three NASA scans already carry an OCR text layer from the archive, so they are `pdf_ocr_layer`. Federal Register PDFs print whole pages, so they also hold parts of neighbouring notices; the names in them are agency officials and contacts. The margins carry the typesetter's workstation stamp, part of the official print.

---

## 9. Regenerate and check

After a fresh clone:

```sh
cargo run -p corpus-gen --release -- fetch                     # download the public slice (13.2 MB) into testdata/td1/public/
cargo run -p corpus-gen --release -- generate                  # write td1/synthetic, ground truth, td3; update corpus.sha256
cargo run -p corpus-gen --release -- check                     # regenerate into a temp dir and compare with corpus.sha256
MK_REQUIRE_TESTDATA=1 cargo test -p mk-bench                   # US-14.3 AC1 and TD-3 manifest checks
```

All commands take `--out DIR` (default `testdata`). `generate` needs the public slice, because the ground truth lists it. Without `testdata/`, the corpus tests print a notice and pass; `MK_REQUIRE_TESTDATA=1` makes them fail instead (for CI). After a change to the ground-truth types, `MK_BLESS=1 cargo test -p mk-bench` rewrites the schemas.

The generator is deterministic: the same seed and `GENERATOR_VERSION` give the same bytes. It uses ChaCha20 with a fixed seed, `libm` for trigonometry, integer or plain float maths for image work, fixed ZIP timestamps, no clock and sorted output. Change `GENERATOR_VERSION` whenever an output byte changes on purpose, then run `generate` and commit the updated `corpus.sha256` with the code change.

Byte identity has been checked on Linux only (two runs, same bytes). **Cross-OS identity is checked in M2a CI.**

---

## 10. Open points

- **ASSUMPTION:** made-up IBAN bank codes match no real account (§7).
- **ASSUMPTION:** TD-3 expected outcomes for rebuilt xrefs, partial images and wrong extensions (§6) are product policy, not yet confirmed.
- Local disk: `testdata/` takes about 37 MB (synthetic 20 MB, mostly the noisy lossless `poor` scans; public 13.2 MB; TD-3 1.3 MB). None of it is in git.
- The public slice depends on government URLs staying up. `corpus-gen fetch` fails loudly when a file is gone or changed.
- Real-world email is synthetic only (§8); revisit in M3.
- Public files are partly annotated; full name annotation of real documents is out of scope for v0.1.
- **Gold gap found in M1:** "Port Authority", printed as a short form of Velmora Port Authority, is not annotated as a mention, so extractors get false positives for it (`09-model-check.md` §5.3). Add it to the entity's aliases at the next regeneration.
