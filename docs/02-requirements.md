# 02 — Requirements: Mangkon AI v0.1

> Input: `01-brief.md` (approved 2026-09-22). **If this file and the brief disagree, the brief wins.**
> Status: Draft for review (2026-10-11). Nothing is built yet.
> Non-functional requirements (speed, limits, privacy, quality numbers) are in `03-nfr.md`.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. How to read this file

- **Epic** = a big area of the product (E01–E14).
- **User story** = one thing a user wants, written as "As a … I want … so that …".
- **Acceptance criteria (AC)** = testable rules in Given / When / Then form. A story is done only when all its ACs pass.
- Each story has a **Layer** (from brief §5) and a **Milestone** (from `06-roadmap.md`).
  - The milestone is where the story is done. Two or more milestones (e.g. `M0 / M4 / M9`) mean its ACs are finished in different milestones.
  - Layer 1 = must ship. Layer 2 = should ship. Layer 3 = first to cut.
- Story IDs are stable. Use them in commits, tests, and task lists (e.g. `US-07.3`).

---

## 2. Glossary

| Term | Meaning |
|---|---|
| **Project** | A folder of user files plus one SQLite database with everything the app extracted. |
| **File** | A file on disk the user added (PDF, CSV, MBOX, …). |
| **Document** | One logical unit of content. Usually one file = one document. One MBOX = many documents (one per email). An email attachment = a child document. |
| **Segment** | A small piece of a document with an exact locator: a PDF page, a text line range, a CSV row, an email part, a JSON value. |
| **Locator** | The exact address of text inside a document: file + page/line/row/column/email part/JSON path + character span (or a bounding box for OCR). |
| **Identifier** | A hard, pattern-based value: email, phone, URL/domain, IBAN, crypto address, card-like number, or a custom pattern. |
| **Entity** | A person or an organization (Layer 2). |
| **Mention** | One place where an identifier or entity appears, with its locator. |
| **Evidence** | The mention(s) that support a connection, relation, or merge. |
| **Source** | One distinct document. ASSUMPTION: "appears in 2+ sources" means 2+ distinct documents (see Open question Q1). |
| **Connection** | An identifier or entity that appears in 2+ sources. |
| **User decision** | Any choice the user makes: accept/reject/edit a merge, ignore a connection, confirm a column mapping, rename an entity. Never overwritten by re-runs. |
| **Stage A** | Fast, non-LLM processing: parsing, OCR, identifiers, CSV/JSON mapping, keyword index. |
| **Stage B** | LLM extraction of people and organizations, plus embeddings. |
| **Stage C** | LLM extraction of relations and events. |
| **Local endpoint** | A model server on this machine (loopback: `127.0.0.1`, `::1`, `localhost`). |
| **Non-local endpoint** | Any other model server: local network or internet. |

---

## 3. Users

v0.1 has one primary persona. Screens are designed for this person.

**Technical researcher** — OSINT researcher, data journalist who codes, security or fraud researcher, or a developer who investigates things.
- Can build from source and read a README.
- Has a messy folder of mixed files (PDFs, scans, emails, spreadsheets, notes).
- Cannot or will not upload these files to a cloud AI.
- Wants to see what links the files, and wants proof for every link.

Non-technical users (lawyers, NGOs, most journalists) are v0.2+. They are not designed for here.

---

## 4. Epic map

| Epic | Name | Layer | Milestone |
|---|---|---|---|
| E01 | Projects and files | 1 | M3 |
| E02 | Ingestion and parsing | 1 | M3 |
| E03 | Background job queue | 1 | M3 |
| E04 | Identifier extraction | 1 | M4 |
| E05 | CSV / JSON column mapping | 1 | M4 |
| E06 | Search (keyword: L1, semantic: L2) | 1 / 2 | M4 / M6 |
| E07 | Connections screen | 1 (+ L2 for entities) | M4 / M6–M7 |
| E08 | Source viewer | 1 | M3 |
| E09 | Model manager and privacy | 1 | M5 |
| E10 | LLM extraction: people and organizations | 2 | M6 |
| E11 | Entity resolution and review queue | 2 | M6 / M7 |
| E12 | Entity detail | 2 | M7 |
| E13 | LLM extraction: relations and events | 3 | M8 (if time) |
| E14 | Build, docs, demo corpus, responsible use | 1 | M0 / M2 / M4 / M5 / M9 |

### Core workflow → stories (brief §4)

| Workflow step | Stories |
|---|---|
| 1. Drop a mixed folder | US-01.3, E02, US-03.1 |
| 2. Shared identifiers within seconds | E04, US-03.5, US-07.1, US-07.5 |
| 3. People and companies in 2+ sources within minutes | E10, E11, US-07.7 |
| 4. One click → exact source, highlighted | US-07.3, US-08.1 |

---

## 5. Epics, user stories, and acceptance criteria

### E01 — Projects and files

**Goal:** The user can keep separate investigations apart and add files to them safely.

**Rule for the whole epic:** The app **never changes, moves, or deletes the user's original files.** It only reads them.

---

#### US-01.1 — Create a project
*Layer 1 · M3*
As a researcher, I want to create a named project, so that each investigation has its own data.

**AC1 — Create**
- **Given** the app is open
- **When** I choose "New project", enter a name, and pick a location
- **Then** the app creates a project folder with one SQLite database
- **And** the new project opens with an empty state that says "Drop files or folders here"

**AC2 — Name is required and unique**
- **Given** a project named "Acme" already exists in the app's project list
- **When** I try to create another project named "Acme" (case-insensitive)
- **Then** the app blocks it and says "A project with this name already exists"

**AC3 — Location must be writable**
- **Given** I pick a location the app cannot write to
- **When** I confirm
- **Then** the app shows "Cannot write to this folder" and does not create a project

---

#### US-01.2 — Open a project
*Layer 1 · M3*
As a researcher, I want to reopen a recent project, so that I can continue my work.

**AC1 — Recent list**
- **Given** I created or opened projects before
- **When** I start the app
- **Then** I see a list of recent projects, newest first, with name, path, and last opened date

**AC2 — Project folder missing**
- **Given** a project in the recent list was moved or deleted on disk
- **When** I click it
- **Then** the app says "Project not found at <path>" and offers "Locate…" and "Remove from list"

**AC3 — Project from a newer app version**
- **Given** a project database was created by a newer app version
- **When** I open it
- **Then** the app refuses to open it and says which app version is needed
- **And** the database is not changed

**AC4 — Project from an older app version**
- **Given** a project database uses an older schema
- **When** I open it
- **Then** the app makes a backup copy of the database first, then migrates it
- **And** if the migration fails, the backup is kept and the app says so

---

#### US-01.3 — Add files and folders
*Layer 1 · M3*
As a researcher, I want to drop a folder of mixed files into a project, so that the app processes all of them.

**AC1 — Drop or pick**
- **Given** a project is open
- **When** I drag files or folders onto the window, or use "Add files…" / "Add folder…"
- **Then** the app adds every supported file, including files in subfolders
- **And** processing starts automatically (see E03)

**AC2 — Files are referenced, not copied**
- **Given** I add files
- **Then** the app stores the original path of each file and does not copy the file
- ASSUMPTION: files are referenced in place to save disk space. Extracted text is stored in the project database. (See Open question Q2.)

**AC3 — Unsupported and hidden files**
- **Given** the folder contains unsupported files (e.g. `.exe`, `.zip`) and hidden or system files (e.g. `.DS_Store`, `Thumbs.db`)
- **When** I add the folder
- **Then** unsupported files are listed as "Skipped — unsupported type"
- **And** hidden and system files are skipped silently
- ASSUMPTION: archives (`.zip`, `.7z`) are not unpacked in v0.1.

**AC4 — Duplicate content**
- **Given** two files have the same content hash
- **When** I add both
- **Then** the content is processed once
- **And** the document shows both file paths

**AC5 — Summary**
- **Given** I added files
- **Then** I see a summary: "N added, N skipped, N duplicates"

---

#### US-01.4 — Re-scan for changes (incremental)
*Layer 1 · M3*
As a researcher, I want to re-scan my folders, so that new and changed files are processed without redoing everything.

**AC1 — Unchanged files are skipped**
- **Given** a file's content hash has not changed since the last scan
- **When** I click "Re-scan"
- **Then** the file is not processed again

**AC2 — New files**
- **Given** new files were added to a project folder on disk
- **When** I re-scan
- **Then** only the new files are processed

**AC3 — Changed files**
- **Given** a file's content hash changed
- **When** I re-scan
- **Then** the app replaces that file's old segments, mentions, and evidence with new ones
- **And** all user decisions are kept (see US-11.5)

**AC4 — Missing files**
- **Given** a file was deleted or moved on disk
- **When** I re-scan
- **Then** its documents are marked "Missing"
- **And** its evidence is hidden from the connections screen
- **And** nothing is deleted until I choose "Remove from project"
- ASSUMPTION: missing files are kept, not auto-deleted, so a temporary unplugged drive does not destroy work.

**AC5 — Automatic re-scan**
- ASSUMPTION: v0.1 has no file-system watching. Re-scan is manual only.

---

#### US-01.5 — Remove files from a project
*Layer 1 · M3*
As a researcher, I want to remove files from a project, so that wrong files do not create false links.

**AC1 — Remove**
- **Given** a project has documents
- **When** I select one or more files and choose "Remove from project"
- **Then** the app deletes their documents, segments, mentions, and evidence from the project database
- **And** connections are recalculated
- **And** the original files on disk are not touched

**AC2 — Confirmation**
- **Given** I choose "Remove from project"
- **Then** the app asks for confirmation and shows how many connections will lose evidence

---

#### US-01.6 — Project settings
*Layer 1 · M3*
As a researcher, I want per-project settings, so that parsing fits my documents.

**AC1 — Settings available**
- **Given** a project is open
- **When** I open "Project settings"
- **Then** I can set: default phone region, OCR languages, domain stop list, custom patterns (US-04.7), and allowed model endpoints (US-09.7)

**AC2 — Defaults**
- **Given** a new project
- **Then** defaults are: phone region = none, OCR language = English, standard domain stop list, no custom patterns, non-local endpoints = off
- ASSUMPTION: these defaults.

**AC3 — Changing a setting**
- **Given** I change a setting that affects extraction (e.g. phone region)
- **When** I save
- **Then** the app asks "Re-process affected files now?" and only re-runs the affected stage

---

### E02 — Ingestion and parsing

**Goal:** Every supported file becomes documents and segments with exact locators. One bad file never stops the batch.

---

#### US-02.1 — Parse text-based PDFs
*Layer 1 · M3*
As a researcher, I want text PDFs parsed page by page, so that every match points to a page and a text span.

**AC1 — Page segments**
- **Given** a PDF with a text layer
- **When** it is processed
- **Then** each page becomes at least one segment with locator = page number + character span

**AC2 — Encrypted PDF**
- **Given** a password-protected PDF
- **When** it is processed
- **Then** it fails with the reason "PDF is password-protected"
- ASSUMPTION: v0.1 does not ask for passwords.

---

#### US-02.2 — OCR for scanned PDFs and images
*Layer 1 · M3*
As a researcher, I want scans and images read with OCR, so that their text is searchable and linkable.

**AC1 — Detect pages that need OCR**
- **Given** a PDF page has less than 20 characters of extractable text
- **When** the PDF is processed
- **Then** that page is sent to OCR
- ASSUMPTION: threshold of 20 characters per page.

**AC2 — Images**
- **Given** an image file (PNG, JPG/JPEG, TIFF incl. multi-page, WEBP)
- **When** it is processed
- **Then** it is sent to OCR
- ASSUMPTION: this list of image formats.

**AC3 — Word boxes and confidence**
- **Given** a page is OCR'd
- **Then** each word is stored with its bounding box and an OCR confidence
- **And** pages with average confidence below 60% are marked "Low OCR quality"
- ASSUMPTION: 60% threshold.

**AC4 — OCR language**
- **Given** the project's OCR languages are set
- **Then** OCR uses only those languages

---

#### US-02.3 — Parse DOCX, TXT, Markdown, HTML
*Layer 1 · M3*
As a researcher, I want common text formats parsed with exact locations.

**AC1 — Locators**
- **Given** a DOCX file → **Then** locators are paragraph index + character span
- **Given** a TXT or Markdown file → **Then** locators are line number + character span
- **Given** an HTML file → **Then** visible text is extracted; locators are text offset + character span

**AC2 — HTML safety**
- **Given** an HTML file contains scripts, styles, or remote resources
- **When** it is processed
- **Then** scripts and styles are ignored and nothing is fetched from the network

**AC3 — Text encoding**
- **Given** a TXT/MD/CSV file in UTF-8, UTF-16 (with BOM), or Windows-1252
- **Then** it is decoded correctly
- ASSUMPTION: these three encodings are enough for v0.1. Other encodings fail with "Unknown text encoding".

---

#### US-02.4 — Parse CSV
*Layer 1 · M3*
As a researcher, I want spreadsheets parsed row by row, so that a link can point to "row 214, column Phone".

**AC1 — Delimiter and header**
- **Given** a CSV file
- **When** it is processed
- **Then** the app detects the delimiter (comma, semicolon, tab, pipe) and whether row 1 is a header

**AC2 — Row segments**
- **Then** each row is one segment; each cell has locator = row number + column name (or column index if no header)

**AC3 — Broken rows**
- **Given** some rows have the wrong number of columns
- **Then** those rows are still stored, marked "malformed row", and the file does not fail
- **And** the file shows a warning with the count of malformed rows

**AC4 — Excel files**
- ASSUMPTION: `.xlsx` is not supported in v0.1. The user exports to CSV. (See Open question Q5.)

---

#### US-02.5 — Parse JSON
*Layer 1 · M3*
As a researcher, I want JSON parsed with paths, so that each value can be found again.

**AC1 — JSON paths**
- **Given** a JSON file
- **When** it is processed
- **Then** every string and number value is stored with its JSON path (e.g. `$.customers[12].phone`)

**AC2 — Invalid JSON**
- **Given** the JSON is invalid
- **Then** the file fails with the reason and the line/column of the error

---

#### US-02.6 — Parse emails (EML, MBOX)
*Layer 1 · M3*
As a researcher, I want emails parsed with headers and parts, so that I can link senders, recipients, and content.

**AC1 — One document per message**
- **Given** an MBOX file with N messages
- **When** it is processed
- **Then** the app creates N documents, one per message

**AC2 — Headers**
- **Given** an email
- **Then** From, To, Cc, Bcc, Reply-To, Date, Subject, and Message-ID are stored as fields
- **And** addresses in headers are extracted as email identifiers with locator = header name

**AC3 — Body**
- **Given** an email has a text/plain part
- **Then** that part is used as the body
- **And** if there is only an HTML part, its visible text is used
- **And** the locator includes the MIME part number

**AC4 — Attachments**
- **Given** an email has attachments of supported types
- **Then** each attachment becomes a child document linked to the email
- ASSUMPTION: attachments are parsed in v0.1 (they often hold the important links).

---

#### US-02.7 — Clear error per failed file
*Layer 1 · M3*
As a researcher, I want each failed file to show a clear reason, so that I can fix it, and I want other files to keep going.

**AC1 — Batch continues**
- **Given** a batch of 100 files where 3 are corrupt
- **When** the batch is processed
- **Then** 97 files are processed and 3 are marked "Failed"

**AC2 — Plain reason**
- **Given** a failed file
- **Then** the documents list shows the path and a plain reason (e.g. "PDF is damaged", "File too large", "Parsing timed out")
- **And** technical details are available behind "Show details"

**AC3 — Retry**
- **Given** a failed file
- **When** I click "Retry"
- **Then** only that file is processed again

**AC4 — Timeout**
- **Given** one file takes longer than the per-file limit (see `03-nfr.md` NFR-REL-06)
- **Then** that file fails with "Parsing timed out" and the batch continues

---

#### US-02.8 — Documents list
*Layer 1 · M3*
As a researcher, I want to see all documents and their status, so that I know what the app has read.

**AC1 — List**
- **Given** a project has files
- **When** I open "Documents"
- **Then** I see each document with: name, path, type, size, pages/rows/messages, status (Queued / Processing / Done / Failed / Missing), and warnings

**AC2 — Filter**
- **When** I filter by status or type
- **Then** only matching documents are shown

---

### E03 — Background job queue

**Goal:** Long work runs in the background. The user can see, pause, resume, and cancel it. It survives a restart.

---

#### US-03.1 — See progress
*Layer 1 · M3*
As a researcher, I want to see what is being processed, so that I know when results are ready.

**AC1 — Progress panel**
- **Given** processing is running
- **Then** I see: current stage (A / B / C), files done / total, current file name, and an estimated time left

**AC2 — ETA**
- **Given** fewer than 5 files are done
- **Then** the ETA shows "Estimating…"
- ASSUMPTION: 5 files before showing an ETA.

**AC3 — App stays usable**
- **Given** processing is running
- **Then** I can browse connections, search, and open the source viewer at the same time

---

#### US-03.2 — Pause and resume
*Layer 1 · M3*
**AC1 — Pause**
- **Given** processing is running
- **When** I click "Pause"
- **Then** no new file or LLM call starts; the current one finishes or stops within 5 seconds
- ASSUMPTION: 5 seconds.

**AC2 — Resume**
- **Given** processing is paused
- **When** I click "Resume"
- **Then** it continues from where it stopped, without redoing finished files

---

#### US-03.3 — Cancel
*Layer 1 · M3*
**AC1 — Cancel**
- **Given** processing is running
- **When** I click "Cancel" and confirm
- **Then** queued work is removed
- **And** results for files already finished are kept
- **And** partial results for the file in progress are rolled back

---

#### US-03.4 — Survive a restart
*Layer 1 · M3*
**AC1 — Normal quit**
- **Given** jobs are queued, running, or paused
- **When** I quit the app and start it again
- **Then** the same jobs are restored in the same state (running jobs restart as running unless I paused them)

**AC2 — Crash or power loss**
- **Given** the app was killed during processing
- **When** I start it again
- **Then** the project opens without damage
- **And** processing resumes; at most the files in progress at the time of the crash are redone

---

#### US-03.5 — Fast results first
*Layer 1 · M3*
As a researcher, I want identifier results before slow AI work, so that I see value within seconds.

**AC1 — Stage order**
- **Given** new files are added
- **Then** Stage A runs for all new files before any Stage B or C work starts on them
- ASSUMPTION: strict Stage A priority over Stage B/C.

**AC2 — Text-native files first**
- **Given** a batch with text-native files and files that need OCR
- **Then** text-native files are processed first
- ASSUMPTION: this ordering, so the first connections appear fast.

---

### E04 — Identifier extraction

**Goal:** Find hard identifiers in every segment, normalize them, and keep a mention with an exact locator for each.

**Rules for the whole epic:**
- Every mention stores: raw text, normalized value, type, locator, and a confidence.
- Mentions from OCR text get confidence = their OCR word confidence.
- Validation (checksum, format) is applied where the type has one. Invalid values are not stored as identifiers.

---

#### US-04.1 — Emails
*Layer 1 · M4*
**AC1 — Detect and normalize**
- **Given** a segment contains `John.Smith@Example.COM`
- **Then** an email identifier is stored with value `john.smith@example.com`
- ASSUMPTION: only lowercase normalization. No Gmail dot/plus removal in v0.1.

**AC2 — Obfuscated emails**
- ASSUMPTION: forms like `john [at] example [dot] com` are not detected in v0.1.

---

#### US-04.2 — Phone numbers (E.164)
*Layer 1 · M4*
**AC1 — International format**
- **Given** a segment contains `+44 20 7946 0958`
- **Then** a phone identifier is stored as `+442079460958`

**AC2 — National format with default region**
- **Given** the project's default region is `GB`
- **And** a segment contains `020 7946 0958`
- **Then** it is stored as `+442079460958`

**AC3 — National format without region**
- **Given** no default region is set
- **And** a segment contains a number without a country code
- **Then** it is stored as "unnormalized", shown with a warning, and only linked to exact matches of the same digits

**AC4 — Avoid false phones**
- **Given** a value that is not a valid number for its region (per the phone-number library)
- **Then** it is not stored
- **And** values that look like dates, invoice numbers, or amounts are not stored as phones

---

#### US-04.3 — URLs and domains
*Layer 1 · M4*
**AC1 — Domain from URL**
- **Given** a segment contains `https://www.shell-co.example/invoice?id=5`
- **Then** a URL identifier is stored, and a domain identifier `shell-co.example` is stored
- ASSUMPTION: domains are reduced to the registrable domain using the Public Suffix List; `www.` is removed.
- Changed 2026-10-30 (M4b): the example was `https://www.shell-co.example.org/…` → `shell-co.example.org`, which the Public Suffix List does not give (`example.org` is registrable, so the domain is `example.org`). The list wins; the example now uses an unlisted top-level label, which counts as one label.

**AC2 — Domain from email**
- **Given** an email `a@shell-co.com`
- **Then** a domain identifier `shell-co.com` is also stored

**AC3 — Stop list**
- **Given** a domain is on the project's stop list (default: big free-mail and platform domains, e.g. gmail.com, outlook.com, yahoo.com, google.com)
- **Then** it is stored but never shown as a connection
- **And** I can edit the stop list in project settings
- ASSUMPTION: a default stop list of about 50 common domains ships with the app.

---

#### US-04.4 — IBAN
*Layer 1 · M4*
**AC1 — Valid IBAN**
- **Given** a segment contains `DE89 3704 0044 0532 0130 00`
- **Then** it is stored as `DE89370400440532013000` after the mod-97 check and country-length check pass

**AC2 — Invalid IBAN**
- **Given** the checksum fails
- **Then** it is not stored as an IBAN

---

#### US-04.5 — Crypto addresses
*Layer 1 · M4*
ASSUMPTION: v0.1 supports Bitcoin (legacy, P2SH, bech32/bech32m), EVM (Ethereum and compatible), Solana, and TRON.

**AC1 — Bitcoin**
- **Given** a Bitcoin address
- **Then** it is stored only if its Base58Check or bech32 checksum is valid

**AC2 — EVM**
- **Given** a `0x` + 40 hex address
- **Then** it is stored lowercase
- **And** if it is mixed-case, the EIP-55 checksum must be valid, or it is rejected

**AC3 — Solana**
- **Given** a base58 string of 32–44 characters
- **Then** it is stored only if it decodes to exactly 32 bytes
- **And** it gets a lower confidence (ASSUMPTION: 0.7) unless a nearby word like "wallet", "SOL", or "address" is within 50 characters
- Reason: random base58 strings cause false positives.

**AC4 — TRON**
- **Given** a `T…` address of 34 characters
- **Then** it is stored only if its Base58Check checksum is valid

---

#### US-04.6 — Card-like numbers (flag only)
*Layer 1 · M4*
**AC1 — Flag**
- **Given** a 13–19 digit number that passes the Luhn check
- **Then** the document is flagged "Contains card-like number"
- **And** the number is shown masked (e.g. `**** **** **** 1234`)

**AC2 — Not linked**
- **Then** card-like numbers are never shown as connections
- ASSUMPTION: "flag only" means no cross-source linking in v0.1. (See Open question Q4.)

---

#### US-04.7 — Custom patterns
*Layer 1 · M4*
As a researcher, I want to add my own patterns (e.g. case numbers, company IDs), so that the app links things specific to my case.

**AC1 — Add a pattern**
- **Given** project settings are open
- **When** I add a name and a regular expression
- **Then** the app tests it on a sample text box and shows matches live

**AC2 — Invalid pattern**
- **Given** the regex does not compile (or uses unsupported features like look-around)
- **Then** the app shows the error and does not save it

**AC3 — Applied to the project**
- **When** I save a new pattern
- **Then** only the custom-pattern step re-runs on existing segments, and matches appear as a new identifier type with my pattern's name

**AC4 — Too many matches**
- **Given** a pattern matches more than 10,000 times in the project
- **Then** the app warns "This pattern matches a lot. It may be too broad."
- ASSUMPTION: 10,000 matches.

---

### E05 — CSV / JSON column mapping

**Goal:** Structured files become identifiers and entities without an LLM.

---

#### US-05.1 — App suggests a mapping
*Layer 1 · M4*
As a researcher, I want the app to guess what each column means, so that I don't map 40 columns by hand.

**AC1 — Suggestion**
- **Given** a CSV or JSON file is processed
- **Then** the app suggests a type for each column / JSON key: email, phone, URL/domain, IBAN, crypto address, person name, organization name, date, or ignore

**AC2 — How it guesses**
- **Then** a column is suggested as an identifier type if at least 80% of its non-empty values in a sample of 200 rows are valid for that type
- **And** person/organization are suggested from header names (e.g. "name", "full_name", "company", "employer")
- ASSUMPTION: 80% and 200 rows.

**AC3 — Suggestions are not final**
- **Then** suggested mappings are marked "Suggested" until I confirm them
- **And** identifier columns are extracted anyway (pattern extraction runs on all cells)

---

#### US-05.2 — Confirm or edit the mapping
*Layer 1 · M4*
**AC1 — Edit**
- **Given** a mapping screen for a file with sample rows shown
- **When** I change a column's type and click "Confirm"
- **Then** the mapping is saved as a user decision and applied

**AC2 — Reuse**
- **Given** another file has the same column headers
- **Then** the app offers "Use the same mapping as <file>"

**AC3 — Kept on re-scan**
- **Given** a confirmed mapping
- **When** the file changes and is re-processed
- **Then** the confirmed mapping is applied again, not re-suggested
- **And** if the headers changed, the app asks me to review it

---

#### US-05.3 — Mapped name columns create entities
*Layer 1 · M4*
**AC1 — Entities from columns**
- **Given** a column is confirmed as "person name" or "organization name"
- **Then** each non-empty value becomes an entity mention with locator = row + column
- ASSUMPTION: this works in Layer 1 without an LLM, and these entities show on the connections screen as "Name (from table)". (See Open question Q3.)

**AC2 — Row links**
- **Given** one row has a person name and a phone
- **Then** the app stores that they appear in the same row (a "same record" link with the row as evidence)

---

### E06 — Search

---

#### US-06.1 — Keyword search
*Layer 1 · M4*
As a researcher, I want full-text search across all documents, so that I can check any word or value quickly.

**AC1 — Search**
- **Given** a project with processed documents
- **When** I type a query and press Enter
- **Then** I see matching segments with: document name, locator (e.g. "p. 3", "row 214"), and a snippet with the match highlighted

**AC2 — Query syntax**
- **Then** the search supports: plain words (all words must match), `"exact phrase"`, `prefix*`, and `OR`

**AC3 — Filters**
- **When** I filter by file type or document
- **Then** results are limited to them

**AC4 — Open result**
- **When** I click a result
- **Then** the source viewer opens at that spot, highlighted (US-08.1)

**AC5 — No results**
- **Then** the app says "No matches" and nothing else

---

#### US-06.2 — Semantic search
*Layer 2 · M6*
As a researcher, I want to search by meaning, so that I find "wire transfer to an offshore account" even when the text uses other words.

**AC1 — Search**
- **Given** embeddings exist for the project
- **When** I switch to "Meaning" search and type a query
- **Then** I see the top 20 most similar segments with a similarity score and source locator
- ASSUMPTION: top 20.

**AC2 — Not ready**
- **Given** no embedding model is available or embeddings are still running
- **Then** the Meaning tab says why (e.g. "Embeddings: 40% done") and keyword search still works

**AC3 — Embeddings respect endpoint rules**
- **Given** embeddings would use a non-local endpoint
- **Then** the same per-project permission and destination display apply (US-09.7, US-09.8)

---

### E07 — Connections screen

**Goal:** The main screen. It shows what links the files, ranked, with evidence. It must be excellent.

---

#### US-07.1 — See cross-source identifiers
*Layer 1 · M4*
As a researcher, I want a ranked list of identifiers that appear in 2+ sources, so that I see the hidden links first.

**AC1 — Only cross-source**
- **Given** an identifier appears in only one document
- **Then** it is not on the connections screen
- **Given** it appears in 2+ distinct documents
- **Then** it is on the connections screen

**AC2 — Row content**
- **Then** each row shows: value, type icon, number of sources, number of mentions, and the file types it was found in (e.g. PDF · EML · CSV)

**AC3 — Default ranking**
- **Then** rows are sorted by: number of sources (high first), then number of different file types, then number of mentions
- ASSUMPTION: this ranking. It favors links across different kinds of files, which are the hardest to find by hand.

**AC4 — Low confidence**
- **Given** all mentions of an identifier have confidence below 0.5
- **Then** the row is marked "Low confidence" and ranked below confident rows
- ASSUMPTION: 0.5.

---

#### US-07.2 — Filter and sort
*Layer 1 · M4*
**AC1 — Filters**
- **When** I filter by identifier type, minimum number of sources, or file type
- **Then** only matching rows are shown

**AC2 — Find**
- **When** I type in the "Find" box
- **Then** rows whose value contains the text are shown

**AC3 — Sort**
- **When** I click a column header
- **Then** rows sort by that column

---

#### US-07.3 — See evidence and jump to source
*Layer 1 · M4*
As a researcher, I want to see every place a value appears and open any of them in one click.

**AC1 — Expand**
- **When** I expand a row
- **Then** I see every mention: document name, locator, and a short snippet with the value highlighted, grouped by document

**AC2 — One click**
- **When** I click a mention
- **Then** the source viewer opens at that exact spot, highlighted (US-08.1)

**AC3 — No link without evidence**
- **Given** any row on the screen
- **Then** it has at least 2 mentions in 2 documents, each with a locator

---

#### US-07.4 — Ignore noise
*Layer 1 · M4*
As a researcher, I want to hide values that are not useful (e.g. my own company's phone), so that the list stays clean.

**AC1 — Ignore**
- **When** I click "Ignore" on a row
- **Then** it disappears from the list and the decision is saved

**AC2 — Kept on re-run**
- **Given** an ignored value
- **When** files are re-processed
- **Then** it stays ignored

**AC3 — Undo**
- **When** I switch on "Show ignored"
- **Then** ignored rows appear greyed out with an "Un-ignore" button

---

#### US-07.5 — Live updates
*Layer 1 · M4*
**AC1 — Updates while processing**
- **Given** processing is running
- **Then** new connections appear without a manual refresh
- **And** the list does not jump while I am scrolling or have a row expanded; instead a bar says "N new connections — Show"

---

#### US-07.6 — Honest empty state
*Layer 1 · M4*
**AC1 — Nothing found**
- **Given** processing finished and no value appears in 2+ sources
- **Then** the screen says "No cross-source connections found in N documents."
- **And** it does not show weak or guessed links to fill the screen

**AC2 — Still processing**
- **Given** processing is still running and nothing is found yet
- **Then** the screen says "Looking for connections… N of M files done"

---

#### US-07.7 — Entities on the connections screen
*Layer 2 · M6 / M7*
As a researcher, I want people and companies that appear in 2+ sources on the same screen.

**AC1 — Tabs or filter**
- **Given** entity extraction has run
- **Then** the connections screen has a type filter: Identifiers / People / Organizations / All

**AC2 — Entity rows**
- **Then** an entity row shows: name, type, number of sources, number of mentions, and its linked identifiers

**AC3 — Evidence**
- **When** I expand an entity row
- **Then** I see each mention with snippet and confidence, and one click opens the source

**AC4 — Unreviewed merges**
- **Given** an entity is built from a merge that is not yet reviewed
- **Then** the row shows "Unreviewed merge" with a link to the review queue

---

### E08 — Source viewer

**Goal:** One click opens the exact spot, highlighted. Must be excellent.

---

#### US-08.1 — Open at the exact location, highlighted
*Layer 1 · M3*
As a researcher, I want to see the original document at the exact spot, so that I can check the link with my own eyes.

**AC1 — PDF (text)**
- **Given** a mention on page 7 of a PDF
- **When** I open it
- **Then** the viewer shows the rendered page 7 with the text span highlighted and scrolled into view

**AC2 — Scanned page or image**
- **Given** a mention from OCR text
- **Then** the viewer shows the original image with the word boxes highlighted

**AC3 — CSV**
- **Given** a mention in row 214, column "Phone"
- **Then** the viewer shows the table scrolled to row 214 with that cell highlighted and the header row visible

**AC4 — Email**
- **Given** a mention in an email header or body
- **Then** the viewer shows the email (headers + body) with the span highlighted
- **Given** a mention in an attachment
- **Then** the viewer opens the attachment with the span highlighted, and shows a link back to the parent email

**AC5 — JSON**
- **Given** a mention at `$.customers[12].phone`
- **Then** the viewer shows the JSON tree expanded to that path with the value highlighted

**AC6 — DOCX, TXT, Markdown, HTML**
- **Then** the viewer shows the text with the span highlighted and scrolled into view
- ASSUMPTION: DOCX and HTML are shown as extracted text with basic formatting, not a pixel-exact rendering.

---

#### US-08.2 — Move between mentions
*Layer 1 · M3*
**AC1 — Next / previous**
- **Given** the opened value appears more than once in this document
- **Then** the viewer shows "Mention 2 of 5" with Next / Previous buttons

**AC2 — Back**
- **When** I press "Back"
- **Then** I return to the screen I came from, at the same scroll position

---

#### US-08.3 — See OCR text
*Layer 1 · M3*
**AC1 — Toggle**
- **Given** an OCR'd page
- **When** I toggle "Show OCR text"
- **Then** I see the recognized text with the same highlight, plus the page's OCR quality

---

#### US-08.4 — Safe rendering
*Layer 1 · M3*
**AC1 — No network**
- **Given** an email or HTML file has remote images, tracking pixels, or links
- **When** I view it
- **Then** nothing is loaded from the network

**AC2 — No scripts**
- **Then** no script from a document ever runs

**AC3 — Links**
- **When** I click a link inside a document
- **Then** the app shows the full URL and asks before opening it in my browser

---

#### US-08.5 — Original file missing
*Layer 1 · M3*
**AC1 — Fallback**
- **Given** the original file was moved or deleted
- **When** I open a mention from it
- **Then** the viewer shows the stored extracted text with the highlight, and a banner "Original file not found at <path>"

---

#### US-08.6 — Open the original file
*Layer 1 · M3*
**AC1 — Reveal**
- **When** I click "Show in folder" or "Open with default app"
- **Then** the OS file manager or default app opens the original file

---

### E09 — Model manager and privacy

**Goal:** Getting a model is easy. The app is always honest about where data goes.

---

#### US-09.1 — Hardware check
*Layer 1 · M5*
As a researcher, I want the app to check my hardware, so that I know if local AI will work.

**AC1 — Detect**
- **Given** the app starts the first time (or I click "Check hardware")
- **Then** it shows: OS, CPU, RAM, GPU vendor and model, VRAM (or Apple unified memory), and free disk space

**AC2 — Result**
- **Then** it shows one of: "Meets minimum", "Below minimum"
- **And** the minimum is: GPU with 8 GB VRAM + 16 GB RAM, or Apple Silicon with 32 GB unified memory, plus 30 GB free disk
- GB here means 10^9 bytes (decided 2026-11-04): a card sold as 8 GB reports about 8.5 × 10^9 bytes and meets the minimum.
- Free disk is shown, but it is not part of the verdict (decided 2026-11-04): with less than 30 GB free the check adds a note ("about 30 GB is recommended for models and project data"), and each download checks its own size (US-09.4 AC1). A nearly full disk can still take a small model.

**AC3 — AMD / Intel GPU**
- **Given** an AMD or Intel GPU that meets the VRAM minimum
- **Then** the app says "Supported on a best-effort basis (Vulkan)"

**AC4 — Apple 24 GB**
- **Given** Apple Silicon with 24 GB
- **Then** the result is "Below minimum" with the note "May work, but not supported"

---

#### US-09.2 — Below-minimum path
*Layer 1 · M5*
**AC1 — No local download**
- **Given** the result is "Below minimum"
- **Then** the app does not offer local model downloads
- **And** it shows: "This machine is too weak for local AI. Connect an external model endpoint."

**AC2 — Layer 1 still works**
- **Then** identifiers, CSV mapping, keyword search, connections, and the source viewer work fully

**AC3 — Override**
- ASSUMPTION: an advanced setting lets a user force-enable local downloads, with a warning "Not supported. It may be very slow." (See Open question Q7.)

---

#### US-09.3 — Model catalog and recommendation
*Layer 1 · M5*
**AC1 — Catalog**
- **Given** the machine meets the minimum
- **When** I open "Models"
- **Then** I see a catalog with: name, role (extraction or embeddings), size on disk, required VRAM/memory, license, and a "Recommended" badge on the best fit for my hardware

**AC2 — Catalog source**
- **Then** the catalog is a file shipped with the app version
- ASSUMPTION: the app does not fetch catalog updates from the internet in v0.1.

**AC3 — License shown**
- **Then** the model license is shown before download

---

#### US-09.4 — Download a model
*Layer 1 · M5*
**AC1 — Disk space check**
- **Given** free disk space is less than the model size + 2 GB
- **When** I click "Download"
- **Then** the app blocks it and says how much space is needed
- ASSUMPTION: 2 GB headroom.
- On a resume, "model size" means the bytes still to download (decided 2026-11-04): the partial file already holds the rest.

**AC2 — Progress**
- **Then** I see progress, speed, and time left; I can pause and cancel

**AC3 — Resume**
- **Given** the download stopped (network loss, app quit)
- **When** I resume
- **Then** it continues from where it stopped

**AC4 — Checksum**
- **Given** the download finished
- **Then** the app checks the SHA-256 against the catalog
- **And** if it does not match, the file is deleted and the app says "Download corrupted. Please retry."

**AC5 — Delete**
- **When** I delete a downloaded model
- **Then** its files are removed and the disk space shown is updated

---

#### US-09.5 — Run the local model
*Layer 1 · M5*
**AC1 — Start on demand**
- **Given** a local model is downloaded and selected
- **When** an AI stage needs it
- **Then** the app starts `llama-server` bound to `127.0.0.1` only, and waits until its health check passes

**AC2 — Stop**
- **Given** no AI work for 10 minutes, or the app quits
- **Then** `llama-server` is stopped and its memory is freed
- ASSUMPTION: 10-minute idle stop.

**AC3 — Failure**
- **Given** `llama-server` fails to start or crashes
- **Then** AI jobs pause with a clear reason (e.g. "Not enough GPU memory") and Layer 1 work continues

---

#### US-09.6 — Connect an external endpoint
*Layer 1 · M5*
As a researcher on a weak laptop, I want to use a stronger machine or a service, so that I still get AI features.

**AC1 — Add endpoint**
- **When** I add an endpoint with base URL, model name, and optional API key
- **Then** the app saves it (the API key in the OS keychain)

**AC2 — Test**
- **When** I click "Test"
- **Then** the app sends one small request and shows: success, model name, and response time — or a plain error (e.g. "Cannot reach 192.168.1.20:8080")

**AC3 — Compatibility**
- **Then** any OpenAI-compatible endpoint works (llama-server, Ollama, LM Studio, cloud APIs)
- **And** if the endpoint does not support JSON-schema output, the app says "Structured output not supported — results may be less reliable" and validates every response itself

---

#### US-09.7 — Per-project permission for non-local endpoints
*Layer 1 · M5*
**AC1 — Off by default**
- **Given** a new project
- **Then** non-local endpoints are not allowed for it

**AC2 — Turn on**
- **When** I allow an endpoint for this project
- **Then** the app shows a confirmation with the exact destination (host and whether it is local network or internet) before saving

**AC3 — Blocked**
- **Given** a project has no allowed endpoint and no local model
- **Then** AI stages do not run, and the AI areas say "No model allowed for this project" with a link to settings

**AC4 — Classification**
- **Then** loopback addresses are "This machine"; private IP ranges (10/8, 172.16/12, 192.168/16, fc00::/7) are "Local network"; everything else is "Internet"
- ASSUMPTION: hostnames are resolved and classified by their IP.

---

#### US-09.8 — Always show where data goes
*Layer 1 · M5*
**AC1 — Indicator**
- **Given** an AI stage is running
- **Then** the status bar shows the destination, e.g. "Data sent to this machine", "Data sent to 192.168.1.20 (local network)", "Data sent to api.openai.com (internet)"

**AC2 — Project view**
- **Then** project settings list every endpoint this project has ever sent data to, with dates
- ASSUMPTION: this history is kept.

---

### E10 — LLM extraction: people and organizations

**Goal:** Find people and organizations, with evidence and confidence. Never invent.

---

#### US-10.1 — Extract people and organizations
*Layer 2 · M6*
As a researcher, I want people and companies found automatically, so that I see who links my files.

**AC1 — Mentions**
- **Given** a model is available and allowed for the project
- **When** Stage B runs on a segment
- **Then** each found person or organization is stored as a mention with: name as written, type, evidence span (exact text), locator, and confidence (0–1)
- M6a: one model call may cover several adjacent segments of a document, up to 2,000 characters (decided 2026-11-05). Each mention is still stored against its exact segment and span.

**AC2 — Structured output**
- **Then** the model is called with a JSON schema (grammar-constrained), and any response that does not match the schema is retried once, then logged and skipped
- ASSUMPTION: one retry.
- M6c: a response cut at the token limit is not retried; it is logged and skipped at once (decided 2026-11-06). Retrying took 17% of the run on TD-1 and saved few names (`16-entities-acceptance.md` §4).

**AC3 — Display threshold**
- **Given** a mention has confidence below 0.5
- **Then** it is stored but hidden by default; a "Show low confidence" filter reveals it
- ASSUMPTION: 0.5.

---

#### US-10.2 — Never invent evidence
*Layer 2 · M6*
**AC1 — Span check**
- **Given** the model returns a mention whose evidence text is not found in the segment (after whitespace normalization)
- **Then** the mention is discarded and counted in a "discarded" log

**AC2 — Visible count**
- **Then** the job summary shows "N mentions discarded (no matching text)"

---

#### US-10.3 — Experimental label
*Layer 2 · M6*
**AC1 — Label**
- **Given** M1 shows local models extract poorly (brief §10, `06-roadmap.md` M1)
- **Then** all LLM features show an "Experimental" label
- **And** this is a build-time or config flag, not a code change
- M6c: the label is off by default (decided 2026-11-06): the minimum-tier model passes NFR-QA-08/09 on TD-1 (`16-entities-acceptance.md` §3). `MK_AI_EXPERIMENTAL=1` at build time turns it back on.

---

#### US-10.4 — Re-run with another model
*Layer 2 · M6*
**AC1 — Re-run**
- **When** I choose "Re-run AI extraction" with a different model
- **Then** LLM mentions are replaced
- **And** all user decisions are kept (US-11.5)
- **And** each mention stores which model produced it

---

### E11 — Entity resolution and review queue

**Goal:** Group mentions into entities. The app proposes merges. The user decides.

---

#### US-11.1 — Group exact name matches
*Layer 2 · M6*
**AC1 — Exact match**
- **Given** two mentions of the same type with the same normalized name (case, spaces, punctuation, and legal suffixes like "Ltd", "LLC", "GmbH" normalized)
- **Then** they belong to the same entity automatically
- ASSUMPTION: exact normalized matches are not "merges" and do not need review. (See Open question Q6.)

**AC2 — Can be split**
- **Then** I can still split them (US-11.4)

---

#### US-11.2 — Merge proposals
*Layer 2 · M7*
**AC1 — Propose**
- **Given** two entities have similar names (e.g. "J. Smith" / "John Smith") or share a hard identifier
- **Then** the app creates a merge proposal with a score and plain reasons, e.g. "Similar names (0.86) · Same phone +4420… in 2 documents"

**AC2 — Never auto-merge**
- **Then** no proposal is applied without my decision

**AC3 — Threshold**
- **Then** only proposals with score ≥ 0.6 are shown
- ASSUMPTION: 0.6. Tuned in M7 (spike S7, `06-roadmap.md`).

---

#### US-11.3 — Review queue
*Layer 2 · M7*
**AC1 — Queue**
- **When** I open "Review"
- **Then** I see pending proposals, highest score first, each with both entities side by side: names, aliases, identifiers, and evidence snippets

**AC2 — Accept**
- **When** I click "Accept"
- **Then** the two entities become one; the chosen name is the more frequent one; all other names become aliases

**AC3 — Reject**
- **When** I click "Reject"
- **Then** the proposal disappears and is never proposed again for this pair

**AC4 — Edit**
- **When** I click "Edit"
- **Then** I can choose the final name and type before accepting

**AC5 — Keyboard**
- **Then** I can review with the keyboard: `A` accept, `R` reject, `E` edit, `J`/`K` next/previous
- ASSUMPTION: these keys.

---

#### US-11.4 — Split or undo a merge
*Layer 2 · M7*
**AC1 — Undo**
- **Given** an accepted merge
- **When** I click "Undo merge" in the entity detail
- **Then** the entities are split back, and the pair is stored as "rejected"

**AC2 — Split**
- **When** I select some mentions of an entity and click "Split"
- **Then** they become a new entity, and this is stored as a user decision

---

#### US-11.5 — User decisions are never overwritten
*Layer 2 · M7*
**AC1 — Re-runs**
- **Given** I accepted, rejected, edited, split, renamed, or ignored something
- **When** extraction re-runs (re-scan, new model, new app version)
- **Then** my decision still applies

**AC2 — Decision log**
- **Then** every decision is stored with time and before/after values, and I can see the list in project settings

---

### E12 — Entity detail

---

#### US-12.1 — View an entity
*Layer 2 · M7*
**AC1 — Detail page**
- **When** I open an entity
- **Then** I see: name, type, aliases, linked identifiers, "Appears in" (list of documents with counts), and all evidence with snippets

**AC2 — One click**
- **When** I click any evidence
- **Then** the source viewer opens at that spot

---

#### US-12.2 — Edit an entity
*Layer 2 · M7*
**AC1 — Edit**
- **When** I rename an entity, change its type, or remove an alias
- **Then** the change is saved as a user decision (US-11.5)

**AC2 — Note**
- **When** I add a free-text note
- **Then** it is saved with the entity
- ASSUMPTION: notes are in v0.1 (small, useful for investigators).

---

### E13 — LLM extraction: relations and events (first to cut)

---

#### US-13.1 — Extract relations
*Layer 3 · M8 (if time)*
**AC1 — Relations**
- **Given** Stage B is done for a segment
- **When** Stage C runs
- **Then** relations between entities are stored with type, evidence span, locator, and confidence
- **And** relation types are a fixed list: `works_for`, `owns`, `director_of`, `paid`, `sent_to`, `related_to`
- ASSUMPTION: this list.

**AC2 — Evidence check**
- **Then** the same "never invent" rule as US-10.2 applies

---

#### US-13.2 — Extract dated events
*Layer 3 · M8 (if time)*
**AC1 — Events**
- **When** Stage C runs
- **Then** events are stored with: date (ISO 8601; partial dates like `2024-03` allowed), short description, involved entities, evidence, and confidence

**AC2 — Unclear dates**
- **Given** a date cannot be resolved (e.g. "last Tuesday" with no reference date)
- **Then** the event is stored without a date and marked "Date unknown"

---

#### US-13.3 — Show relations and events
*Layer 3 · M8 (if time)*
**AC1 — Entity detail**
- **Then** the entity detail shows its relations and events, each with evidence and one-click source

---

### E14 — Build, docs, demo corpus, responsible use

---

#### US-14.1 — Build from source
*Layer 1 · M2 / M9*
As a technical user, I want one build command per OS, so that I can run the app quickly.

**AC1 — One command**
- **Given** a supported OS with the documented prerequisites installed
- **When** I run the documented build command
- **Then** the app builds and starts without Python or Docker

**AC2 — Prerequisite check**
- **Given** a prerequisite is missing (e.g. Rust toolchain, Node)
- **Then** the build script says which one and links to install steps

**AC3 — CI**
- **Then** CI builds and runs tests on macOS, Windows, and Ubuntu for every pull request

---

#### US-14.2 — README
*Layer 1 · M9*
**AC1 — Top of README**
- **Then** the README starts with: what the app does (one paragraph), the hardware minimum, and the tested platforms (honest list)

**AC2 — Responsible use section**
- **Then** it states: what the tool is for (investigation, due diligence, research); that it only works on files the user already has; that the user is responsible for legal use; that privacy laws such as GDPR still apply

**AC3 — Privacy section**
- **Then** it states: no telemetry, no analytics, no crash reporting; network is used only for model downloads and user-set endpoints

---

#### US-14.3 — Demo corpus and benchmark
*Layer 1 · M0 / M4 / M9*
**AC1 — Public corpus**
- **Then** the synthetic corpus (~80 files) and the public slice (~20 files) are published with a ground-truth file, as a deterministic generator and a download manifest (the files themselves are built locally, not committed)

**AC2 — Reproducible numbers**
- **When** a user runs the documented benchmark command
- **Then** it prints recall and precision per identifier type and per entity type, against the ground truth

**AC3 — Demo**
- **Then** the demo video and README GIFs use only this corpus

---

#### US-14.4 — First run
*Layer 1 · M5*
**AC1 — First-run steps**
- **Given** the app starts for the first time
- **Then** it shows three steps: hardware check → choose model (download or endpoint, or "Skip — use without AI") → create first project

**AC2 — Demo project**
- **Then** the app offers "Open demo project" if the demo corpus is present on disk
- ASSUMPTION: the demo corpus is not bundled in the app; the user downloads it with the repo.

---

## 6. Out of scope for v0.1

From brief §5 ("Moved to v0.2+") and `00-idea.md` §6.2:
- Q&A with citations
- Timeline view
- Graph view
- Export (Markdown report, CSV, GraphML)
- Ready-made installers / signed builds
- Python parsing sidecar
- Web scraping, people-search APIs, social media lookups, any OSINT connectors
- Multi-user, sync, cloud hosting
- Non-technical-user UX

ASSUMPTION (also out of v0.1, not stated in the brief): `.xlsx`, archives (`.zip`), PST/MSG email files, audio/video, file-system watching, project encryption by the app, UI languages other than English.

---

## 7. Open questions

| # | Question | Assumption in this doc |
|---|---|---|
| Q1 | What is one "source": a file or a document? (Two emails in one MBOX — one source or two?) | A document. Each email is its own source. |
| Q2 | Copy files into the project, or reference them in place? | Reference in place. Viewer falls back to extracted text if the file is gone. |
| Q3 | CSV-mapped person/org names: show them in Layer 1, or wait for Layer 2 entities? | Show them in Layer 1 as "Name (from table)". |
| Q4 | Card-like numbers: flag only, or also link across sources? | Flag only, masked, never linked. |
| Q5 | Is `.xlsx` needed in v0.1? Many leaks and bank exports are Excel. | No. User exports to CSV. |
| Q6 | Exact normalized name matches: auto-group, or also go through review? | Auto-group, can be split. |
| Q7 | Allow a forced local model on below-minimum machines? | Yes, hidden advanced setting with a warning. |
| Q8 | Should the app encrypt project data at rest? | No. Rely on OS disk encryption; say so in README. |
| Q9 | OCR languages beyond English in v0.1? | English by default; user can add Tesseract/ocrs language packs if the engine supports it. |
