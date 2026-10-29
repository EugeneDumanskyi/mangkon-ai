# 11 — Ingestion: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M3 (watch items W3a-1, W3a-2 and the page render), ADR-0007, ADR-0011, ADR-0013, ADR-0014, ADR-0016, ADR-0017, ADR-0018, `03-nfr.md` (NFR-PERF-11/14/20/21/22/23, NFR-QA-07, NFR-CAP-04/05/06/07, NFR-REL-01/02/03/06/07/10, NFR-RES-02/07, NFR-SEC-09). **If this file disagrees with them, they win.**
> Status: §1–§5 measured in M3a, §6–§7 in M3b, on RM-1 (2026-10-29). Linux only; macOS and Windows are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Machine and build

Machine: **RM-1** (`10-walking-skeleton.md` §1): Linux x86_64, Intel i7-14650HX (24 threads), 93 GB RAM, NVMe SSD. Release builds, idle machine (confirmed before the timed runs). Measured 2026-10-29.

Since M3a every text format but EML is parsed as a stream, and the core stages an item's result in memory up to 32 MB, then in a spill file, and commits it from there at `Done` (ADR-0016). The watch items check that this keeps memory flat whatever the file size.

```sh
cargo run -p corpus-gen --release -- csv-rows --rows 1000000          # s2a/rows-1m.csv
cargo run -p corpus-gen --release -- csv-rows --rows 6700000          # s2a/rows-6700000.csv
cargo run -p corpus-gen --release -- mbox-messages --bytes 2G         # w3a/messages-2g.mbox
cargo test -p mk-core --release --test w3a_streaming -- --ignored --nocapture --test-threads 1
MK_WORKER_PROGRAM=target/release/mangkon cargo test ...               # workers from the desktop binary
```

Each run: a fresh project, one lane, one worker. Re-scan (read + BLAKE3), then Stage A (worker: read + hash check, streamed parse, extract mentions, chunks of 4,096 segments; core: staging with the spill file, one commit with the derived rebuild). Peak resident memory of the core and of core + worker, sampled every 100 ms through the whole Stage A, commit included.

---

## 2. W3a-1 — 500 MB CSV (NFR-CAP-05, NFR-PERF-22)

### Question

Does a 500 MB CSV stream with bounded memory, and finish within 120 s (NFR-REL-06)? Is NFR-PERF-22 still met on the streaming path?

**Pass marks:** the worker within 1 GB (NFR-REL-07); the app ≤ 2 GB (NFR-RES-02); done ≤ 120 s; ≥ 50,000 rows/s on `rows-1m.csv`.

### Inputs

| File | Rows | Size | Rows with an email |
|---|---|---|---|
| `s2a/rows-1m.csv` (S2a) | 1,000,000 | 74.9 MB | 300,265 |
| `s2a/rows-6700000.csv` | 6,700,000 | 501.9 MB (478.7 MiB, under the 500 MB limit) | 2,008,906 |

Columns as in S2a: date, description, IBAN, amount, and an email in 30 % of rows.

### Results

| File | Worker | Stage A | Rows/s | Commit | Peak core + worker | Peak core | Database |
|---|---|---|---|---|---|---|---|
| `rows-1m.csv` | `mk-test-worker` | 2.21 s | **452,797** ✅ | 1.75 s | 62 MB | 54 MB | 189 MB |
| `rows-6700000.csv` | `mk-test-worker` | **16.02 s** ✅ | 418,178 | 13.05 s | **44 MB** ✅ | 36 MB | 1,266 MB |
| `rows-1m.csv` | `mangkon --worker` | 2.21 s | 451,798 | 1.74 s | 92 MB | 54 MB | 189 MB |
| `rows-6700000.csv` | `mangkon --worker` | 15.99 s | 418,987 | 13.06 s | 74 MB | 36 MB | 1,266 MB |

Re-scan of the 500 MB file: 0.14–0.15 s. The desktop binary as worker (`MK_WORKER_PROGRAM=target/release/mangkon`) adds ≈ 30 MB of resident shared libraries (WebKitGTK, GTK), as in S2a, and nothing else.

### What the numbers say

- **Memory no longer grows with the file.** S2a needed 740 MB (core + worker) for the 75 MB CSV, ≈ 10 × its size. The streamed path needs 62 MB for it and 44 MB for a file 6.7 × larger. The core holds at most the 32 MB staging buffer; the rest goes to the spill file.
- **Speed did not drop.** 453,000 rows/s against 385,000 in S2a: the parser no longer builds the whole result first. The commit is still 80 % of Stage A, and grows linearly (1.75 s for 1M rows, 13.05 s for 6.7M).
- **The spill file costs little.** It is written while the worker parses and read once by the commit. The commit of the 1M-row file took 1.75 s against 1.62 s in memory (S2a), ≈ 8 % more.

### Verdict

**Met.** A 500 MB CSV is read in 16 s with 44 MB peak. NFR-CAP-05 holds at 500 MB; the ≈ 175 MB practical limit of S2a is gone.

---

## 3. W3a-2 — 2 GB MBOX (NFR-CAP-06, NFR-PERF-23)

### Question

Does a 2 GB MBOX stream with bounded memory, and at what speed?

**Pass marks:** the worker within 1 GB (NFR-REL-07); the app ≤ 2 GB (NFR-RES-02); ≥ 200 messages/s, no attachment OCR (NFR-PERF-23).

### Input

`w3a/messages-2g.mbox`: 1,286,855 messages, 2,147,483,300 bytes (just under the 2 GB limit, NFR-CAP-06). Built by `corpus-gen mbox-messages` from the TD-2 name pools: From, To, Cc, Date, Subject, Message-ID, 3–8 paragraphs; every 20th message has a small base64 CSV attachment (a child document, US-02.6 AC4). The generator stops before the next message would pass the target size.

### Results

| Worker | Stage A | Messages/s | Commit | Peak core + worker | Peak core | Database |
|---|---|---|---|---|---|---|
| `mk-test-worker` | **92.15 s** | **13,964** ✅ | 71.12 s | **82 MB** ✅ | 74 MB | 4,753 MB |
| `mangkon --worker` | 92.26 s | 13,949 | 71.13 s | 133 MB | 95 MB | 4,753 MB |

Re-scan of the file: 0.59–0.60 s.

### What the numbers say

- **Memory stays flat.** 82 MB (133 MB with the desktop binary as worker) for a 2 GB file: the worker holds one message at a time, and the core the 32 MB staging buffer.
- **The commit dominates.** The worker read and parsed the file in ≈ 21 s (≈ 61,000 messages/s); the commit took the other 71 s. NFR-PERF-23 is met 70 × over.
- **The per-message deadline (ADR-0016) was not needed here.** On RM-1 even a whole-file deadline of 120 s would have passed for this file. The rule still covers slower machines, mailboxes of larger messages, and the floor NFR-PERF-23 allows (200 messages/s would take 107 minutes for this file).
- **Disk:** the database grew to 4.75 GB, 2.2 × the mailbox (documents, segments with their locators, mentions, derived rows). The spill file needs room too while the item runs; its size was not measured.

### Verdict

**Met.** A 2 GB MBOX is read in 92 s with 82 MB peak; NFR-CAP-06 and NFR-PERF-23 hold.

### Follow-ups

- **Budget estimates** (`Format::memory_estimate`, gap A2): they book 328 MB for the 500 MB CSV (a 64 MB window) and 1.1 GB for the 2 GB MBOX (a 256 MB message window), against 44 MB and 82 MB measured. The spill threshold (32 MB) caps the core's share as planned. The estimates stay cautious: a mailbox with one very large message can still need its window. ASSUMPTION: kept as they are; lower them if large files wait too long behind the budget.
- The per-message deadline (ADR-0016) stays as decided; this run did not exercise it.

---

## 4. Memory on TD-2's text formats (NFR-RES-02)

```sh
cargo test -p mk-core --release --test nfr_res_02 -- --ignored --nocapture --test-threads 1
MK_WORKER_PROGRAM=$PWD/target/release/mangkon cargo test ...
```

Stage A on TD-2's files with the default lane (23 workers, the 1.5 GB budget), core + workers sampled every 100 ms. The text formats are every file M3a reads; all of TD-2, PDFs and images included, is in §7.

| Files | Worker | Files | Done | Stage A | Peak core + workers | Peak core |
|---|---|---|---|---|---|---|
| CSV (M2 criterion 7b) | `mk-test-worker` | 396 | 396 | 12.9 s | 214 MB | 53 MB |
| CSV | `mangkon --worker` | 396 | 396 | 13.4 s | 841 MB | 54 MB |
| TXT, MD, CSV, JSON, HTML, DOCX, EML, MBOX | `mk-test-worker` | 3,664 | 3,664 | 16.4 s | **260 MB** ✅ | 71 MB |
| Same | `mangkon --worker` | 3,664 | 3,664 | 15.8 s | **952 MB** ✅ | 74 MB |

- **Met** (≤ 2 GB). Every file is read; none fails. In M2 the 99 Windows-1252 CSVs failed; now they are read, so the CSV run takes 12.9 s against 8.6 s then.
- As in M2, resident memory counts each desktop worker's shared libraries again (≈ 30 MB × 23 workers); the test worker shows what the parsing itself needs.

---

## 5. Other M3a checks

### Kill test (NFR-REL-02/03/04)

```sh
target/release/mangkon-cli kill-test --source testdata/td1/synthetic --runs 20 --seed 1
target/release/mangkon-cli kill-test --source testdata/td2 --runs 20 --seed 1
```

The driver as in `10-walking-skeleton.md` §4; since M3a it ingests every text format (TXT, MD, CSV, JSON, HTML, DOCX, EML, MBOX).

| Source | Ingestion (shortest reference) | Kill moments | Passed | Orphan workers | Items recovered per kill |
|---|---|---|---|---|---|
| TD-1 text formats (37 files) | 52 ms, 23 lanes | 6 at process start, 4 in project open, 10 in Stage A | **20 / 20** | 0 | 0–15 |
| Same, seed 2 | 67 ms | 32 at start, 19 in open, 42 in Stage A, 7 after it | **100 / 100** | 0 | — |
| TD-2 text formats | 15.6 s, 23 lanes | 20 in Stage A, after 3,340–3,655 committed items | **20 / 20** | 0 | 7–14 |

**Met.** No corrupted project; lost work never exceeded the files in progress.

**Found and fixed in M3a.** One TD-1 run first failed: the kill landed while a new project's file was written, before WAL was on, and left a hot rollback journal. The open path reads the schema version through a read-only connection first (INV-WS-03), and a read-only connection cannot roll a hot journal back ("attempt to write a readonly database"), so the project never opened again. Since M2b; the window is a few milliseconds at creation. The version check now rolls the journal back through a read-write connection in that case (`mk_store::schema::open_write`; test `nfr_rel_03_hot_journal_from_creation_opens`). The two TD-1 rows above ran after the fix.

### TD-3 text formats (NFR-REL-01, NFR-CAP-07)

- `nfr_rel_01_td3_text_formats_fail_alone`: every TD-3 text-format file (TD-3 v3, `07-test-corpus.md` §6) goes through worker processes in one batch with the TD-1 text files. Each gets the outcome and the reason `expected.json` gives; the TD-1 files all finish; addresses hidden in scripts or behind test faults are not found.
- `nfr_cap_07_over_limit_csv_mbox`: sparse files of 500 MB + 1 byte (CSV) and 2 GiB + 1 byte (MBOX) fail with "File too large (limit: 500 MB)" and "File too large (limit: 2 GB)" before a worker starts; the other files finish.
- `adr_0016_mbox_deadline_per_message`: with a 1.5 s deadline, a mailbox that finishes a message every 0.4 s for 3.2 s is read; the same work in a TXT file is stopped with "Took too long to read".

### No network while viewing (NFR-SEC-09, M3 criterion 6)

```sh
MK_UI_TIMING=1 MK_VIEW_SWEEP=1 MK_UI_TIMING_PROJECT=<empty dir> \
  MK_UI_TIMING_SOURCE=testdata/td1/synthetic MK_VIEW_SWEEP_OUT=sweep.json \
  strace -f -e trace=connect -o connect.log target/release/mangkon
```

With `MK_VIEW_SWEEP=1` (only together with `MK_UI_TIMING=1`) the app ingests the folder, then opens every mention of every connection in the source view, follows each attachment link once, writes what it saw and exits. `strace -f` follows every process the app starts, WebKit's included.

| | |
|---|---|
| Mentions opened | **55 of 55** in the project (email 44, JSON 3, DOCX / HTML paragraphs 5, TXT / MD 3); 0 errors |
| Attachment links | 0: TD-1's attachments are PDFs (M3b) |
| `connect` calls | 11, all `AF_UNIX`: the session and system D-Bus, the accessibility bus, Wayland |
| `AF_INET` / `AF_INET6` | **0** |

The views render text nodes only; a static-markup test feeds each body kind `<script>`, `<img src=http…>` and similar markup and finds it escaped (`ui/src/project/SourceBody.test.tsx`). The content security policy is unchanged.

**Met on Linux.**

### Rebuild equals incremental

`mangkon-cli ingest` of TD-1's synthetic files and TD-3 (55 done, 13 failed), then `mangkon-cli rebuild`: "Same as before: yes", exit 0.

---

## 6. W3b-1 — page view on a 500-page PDF (NFR-PERF-14 share)

### Question

Does a click on a PDF mention show its page within 1 s through the real path: core → render worker → PNG → page cache (ADR-0011, ADR-0017)? Is the 10 s render deadline (ADR-0017, ASSUMPTION) far enough from the real time?

**Pass mark:** p95 ≤ 1 s, worker start included.

### Method

```sh
cargo build -p mangkon-desktop --release
MK_WORKER_PROGRAM=$PWD/target/release/mangkon \
  cargo test -p mk-core --release --test w3b_page_render -- --ignored --nocapture
```

TD-1's 500-page `vendor-ledger-2024-2025.pdf` is read into a fresh project. Twenty pages are picked at random (fixed seed). Each view is what the desktop app does on a click: `page_view` (status, size, highlight boxes), then `page_image` (the PNG, 1190 × 1684 px at scale 2.0). Three cases:

- **Cold:** a fresh project and an empty page cache for each page; the render worker starts on the view.
- **Warm:** one project, the worker already started on another page; the page is not cached.
- **Cached:** the same pages again, from `cache/pages/`.

### Results

| Worker | Cold p50 / p95 | Warm p50 / p95 | Cached p50 / p95 |
|---|---|---|---|
| `mangkon --worker` | 25 / **36 ms** ✅ | 6 / 6 ms | < 1 / < 1 ms |
| `mk-test-worker` | 20 / 37 ms | 6 / 6 ms | < 1 / < 1 ms |

A check outside the test: `mangkon-cli render` with the cache removed, each a new process (open the project, start the worker, render, write the PNG), took 20–40 ms for ledger pages 1, 250 and 497, and 12–70 ms for each of TD-1's 11 OCR hits (scanned PDF pages at 1190 × 1684, scans up to 2479 × 3508), each with its one highlight box.

### What the numbers say

- **The page view is ≈ 28 × under its share of NFR-PERF-14.** Most of the cold time is the worker start; a render of a ledger page takes ≈ 6 ms.
- **The ledger's pages are light:** one text table each. A page with a large embedded image costs more; TD-1's scanned PDF pages still took ≤ 70 ms per cold process.
- **The 10 s render deadline is ≈ 270 × the slowest cold view.** It stays as a stop for stuck renders; nothing measured here comes near it.
- None of the "if it fails" steps (keep the worker warm, lower the scale, pre-render the hit page) is needed.

### Verdict

**Met.** p95 36 ms cold, 6 ms warm, against 1 s. The full NFR-PERF-14 check (50 random clicks on TD-1 and TD-2, in the app) is M3 criterion 5, in M3d.

---

## 7. Other M3b checks

### W3b-2 — TD-1 Stage A incl. OCR (NFR-PERF-11)

```sh
target/release/mangkon-cli ingest --project <new dir> --tessdata <new dir> \
  testdata/td1/synthetic testdata/td1/public
```

All 101 TD-1 files into a fresh project, 23 lanes, English OCR, three runs:

| Run | Wall clock | Files | Done | Failed | Connections | Peak core |
|---|---|---|---|---|---|---|
| 1 | 2.42 s | 101 | 101 | 0 | 13 | 146 MB |
| 2 | 2.32 s | 101 | 101 | 0 | 13 | 131 MB |
| 3 | 2.38 s | 101 | 101 | 0 | 13 | 139 MB |

**Met:** ≈ 2.4 s against 3 min. TD-1's 22 OCR pages run on several lanes at once; a page takes 0.2–0.4 s.

### Parse speed (NFR-PERF-20, NFR-PERF-21)

```sh
cargo test -p mk-worker --release --test w3b_parse -- --ignored --nocapture --test-threads 1
```

The worker's parse + identifier step on one thread, no store, three runs each.

| Mark | Input | Result | Pass mark |
|---|---|---|---|
| NFR-PERF-20 | TD-1's text PDFs, 527 pages | **5,633–5,742 pages/s**; the 500-page ledger alone 5,694–5,755 | ≥ 20 pages/s ✅ |
| NFR-PERF-21 | TD-1's 22 OCR pages (scans, scanned PDF pages, mixed PDFs), render or decode + OCR + layout, engine loaded first | p50 0.18 s, p95 0.31 s, **slowest 0.42 s** | ≤ 5 s ✅ |

The OCR numbers match M0 (slowest clean page 0.26 s, ADR-0014); the slowest pages here are the poor tier and the 2479 × 3508 scans.

### OCR accuracy (NFR-QA-07)

`nfr_qa_07_ocr_accuracy_clean_tier` (`mk-parsers`, release): the production OCR path on TD-1's 6 clean-tier pages against `page-text.jsonl`, 1 − CER, whitespace-normalised, lines in reference order (the metric moved from the M0 spike into `mk-bench`). **98.46%** against ≥ 95% (M0: 98.5%). **Met.**

### Binary size (NFR-RES-07)

`cargo xtask build --release` on RM-1:

| Binary | Size | M2a |
|---|---|---|
| `mangkon` (desktop) | **37.35 MB** | 10.91 MB |
| `mangkon-cli` | 26.90 MB | — |

hayro, Tesseract, Leptonica and the embedded English data add ≈ 26 MB (M0 measured 18.8 MB for the engines alone; ADR-0014). Tesseract adds only `libstdc++` to the linked libraries. **Met:** ≤ 150 MB. An incremental release build took 26 s.

### TD-3 PDFs and images (NFR-REL-01, NFR-CAP-04)

- `nfr_rel_01_td3_pdf_images_fail_alone`: every TD-3 PDF and image file (v1 and v4, `07-test-corpus.md` §6) goes through worker processes in one batch with TD-1, English OCR. Each gets the outcome and the reason `expected.json` gives, both bombs and the page-tree loops included; the TD-1 files all finish.
- NFR-CAP-04: `pages-2000.pdf` is read; `pages-2001.pdf` fails with "File too large (limit: 2,000 pages)", checked in the worker after the file opens.
- `us_02_1_ac2_encrypted_pdf_fails`: a PDF with a user password fails with "PDF is password-protected"; one with only an owner password is read.
- ADR-0017: `adr_0017_pdf_deadline_per_page`: with a 1.5 s deadline, a PDF that finishes a page every 0.4 s for 3.2 s is read; the same work in a TXT file is stopped with "Took too long to read". `adr_0017_ocr_page_deadline`: with a 1.5 s OCR page deadline, an image whose page stays in OCR for 3 s is stopped with the same reason.

### Kill test (NFR-REL-02/03/04)

```sh
target/release/mangkon-cli kill-test --source testdata/td1/synthetic --runs 20 --seed 1
target/release/mangkon-cli kill-test --source testdata/td2 --runs 20 --seed 1
```

Since M3b the runs read every format, PDFs and OCR included.

| Source | Ingestion (shortest reference) | Kill moments | Passed | Orphan workers | Items recovered per kill |
|---|---|---|---|---|---|
| TD-1 synthetic (81 files) | 1.47 s, 23 lanes | 20 in Stage A (0.25–1.43 s) | **20 / 20** | 0 | 1–4 |
| Same, seed 2 | 1.44 s | 4 at process start, 96 in Stage A | **100 / 100** | 0 | — |
| TD-2 (10,000 files) | 164 s, 23 lanes | 20 in Stage A (27–159 s), after 6,163–9,982 committed items | **20 / 20** | 0 | 4 |

**Met.** No corrupted project; lost work never exceeded the files in progress. The TD-2 run took 64 min.

### Memory on all of TD-2 (NFR-RES-02)

```sh
cargo test -p mk-core --release --test nfr_res_02 nfr_res_02_stage_a_memory_td2_all -- --ignored --nocapture
MK_WORKER_PROGRAM=$PWD/target/release/mangkon cargo test ...
```

As in §4, now on all 10,000 TD-2 files, PDFs and images (OCR) included; 23 lanes, the 1.5 GB budget, workers recycled after 200 files or 300 MB idle (ADR-0007).

| Code | Worker | Stage A | Peak core + workers | Workers started |
|---|---|---|---|---|
| First build | `mk-test-worker` | — | 2,064 MB ❌ | — |
| Same | `mangkon --worker` | — | 2,675 MB ❌ | — |
| + Tesseract clears each page | `mk-test-worker` | 166.8 s | 1,750 MB | — |
| Same | `mangkon --worker` | 164.9 s | 2,462 MB ❌ | — |
| + workers' fixed mmap threshold (final) | `mk-test-worker` | 171.0 s | **1,218 MB** ✅ | 65 |
| Same | `mangkon --worker` | 169.9 s | **1,906 MB** ✅ | 66 |

All final runs: 10,000 done, 0 failed, peak core 76–78 MB.

**Found and fixed in M3b.** The first build was over the limit. Sampling every process during a CLI ingest showed each of the 23 workers keeping 60–85 MB after PDF/OCR work (median 84 MB, top 146 MB), well under the 300 MB recycle threshold, so they were never replaced. Two causes:

- **Tesseract kept the last page:** its image and results stay in the engine until the next page. `mk-tesseract-sys` now calls `TessBaseAPIClear` once a page's words are read, so an idle worker keeps only the loaded language data.
- **glibc kept freed page buffers:** glibc raises its mmap threshold after the first large buffer is freed (up to 32 MB), so later page buffers (8.7 MB for a 300 DPI A4 page) come from the heap and stay with the process. The core now starts workers with `GLIBC_TUNABLES=glibc.malloc.mmap_threshold=1048576` (`malloc_tunables` in `mk-core/src/workers.rs`), placed before any tunables the user set. Other C libraries ignore it. Linux with glibc only; macOS and Windows memory is not measured here.

The fix costs ≈ 3% of Stage A time (171 s against 166.8 s). Recycling idle workers at 64 MB instead was rejected: 1,412 / 1,587 MB, but 174 / 195 s and 649 / 1,726 workers started (test / desktop worker). A 96 MB threshold still left the desktop worker at 2,120 MB. The recycle rule of ADR-0007 is unchanged.

**Met** (≤ 2 GB) with both workers. As in §4, the desktop worker counts its shared libraries again (≈ 27 MB × 23).

### Rebuild equals incremental

`mangkon-cli ingest` of TD-1 (synthetic and public) and TD-3 (155 files: 130 done, 25 failed as expected, 16 connections), then `mangkon-cli rebuild`: 112 identifiers, 16 connections, "Same as before: yes", "Matches the sources: yes", exit 0.

### No network while viewing pages (NFR-SEC-09, M3 criterion 6)

The §5 run, now with PDFs and scans; the app's data folder in a fresh directory (`XDG_DATA_HOME`), so the English OCR data is written on first open:

```sh
XDG_DATA_HOME=<empty dir> MK_UI_TIMING=1 MK_VIEW_SWEEP=1 MK_UI_TIMING_PROJECT=<empty dir> \
  MK_UI_TIMING_SOURCE=testdata/td1/synthetic MK_VIEW_SWEEP_OUT=sweep.json \
  strace -f -e trace=connect -o connect.log target/release/mangkon
```

Since M3b the sweep waits for each page view's image, then turns on the text toggle and waits for the text.

| | |
|---|---|
| Mentions opened | **86** (email 46, page 33 with documents, DOCX / HTML paragraphs 5, JSON 3, TXT / MD 3); 0 failed to open |
| Attachment links | 4 followed (two emails and their PDF attachments) |
| Page views | **29** images shown and toggled to text; the 4 others are the pages of the two PDF attachments, shown as text by design ("Pages of attachments are shown as text") |
| `connect` calls | 11, all `AF_UNIX`: the session and system D-Bus, the accessibility bus, Wayland |
| `AF_INET` / `AF_INET6` | **0** |

The whole run, ingestion with OCR included, took 6 s. **Met on Linux.**

### TD-1 unchanged (NFR-REL-10, INV-CO-01)

SHA-256 of all 103 files under `testdata/td1/` before the M3b runs and after them (ingestion, kill tests, page views, view sweep): identical.
