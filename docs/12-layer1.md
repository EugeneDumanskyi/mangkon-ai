# 12 — Layer 1: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M4 (spike S4), ADR-0010, `03-nfr.md` (NFR-PERF-22/30/34, NFR-CAP-05/08, NFR-REL-02, NFR-QA-03..06, NFR-MNT-05, NFR-SEC-14), `07-test-corpus.md` §8. **If this file disagrees with them, they win.**
> Status: §1–§2 measured in M4a, §3 in M4b, on RM-1 (2026-10-30). Linux only; macOS and Windows are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Spike S4 — FTS5 at 1M segments (R1, NFR-PERF-30)

### Question

Does FTS5 ranked search return the first 50 results in ≤ 200 ms p95 on TD-2, with and without a filter? What does the index cost in size and ingest time?

**Pass mark:** ≤ 200 ms p95, unfiltered and filtered (NFR-PERF-30).

### Machine and build

**RM-1** (`10-walking-skeleton.md` §1), release build, idle machine (confirmed before the timed runs). Measured 2026-10-30.

```sh
cargo test -p mk-core --release --test s4_fts_td2 -- --ignored --nocapture
MK_S4_PROJECT=/path/to/dir cargo test ...   # keep the TD-2 project and reuse it next time
```

### Index (schema v5)

- `segment_fts`: an external-content FTS5 table over `segment.text` (`content='segment'`, tokenizer `unicode61 remove_diacritics 2`), migration `0005_search.sql`. The migration fills it with `'rebuild'`.
- New segments are indexed in the Stage A commit by one `INSERT … SELECT` over the content's segments, in the same transaction (ADR-0010). Delete and update triggers keep the index in step when segments go.
- Ranked by `bm25`, keyset paging on (score, segment id), 50 per page. `highlight()` with private-use markers runs on the page's rows only and gives the hit spans in characters of the stored text (INV-CO-04).
- `mangkon-cli rebuild` refills the index and reports "Search index in step"; `Project::check_health` runs FTS5 `'integrity-check'` (NFR-REL-02).

### Queries

100 queries drawn with a fixed seed from TD-2's own terms, then the same 100 with a file-type filter. Each is timed as the whole service call (`Project::search`: parse, `MATCH` + `bm25`, highlight, snippet, card-like masking, document join), warm (every query runs once first).

| Kind | Count | Drawn from |
|---|---|---|
| Rare words | 35 | The 2,000 least frequent words |
| Common words | 30 | The most frequent words |
| Phrases | 20 | Two or three adjacent, distinct words from segment text |
| Prefixes | 10 | `stem*` |
| OR | 5 | `a OR b` |

All 100 have results.

### Results

| | p50 | p95 | Max |
|---|---|---|---|
| Unfiltered | 2.8 ms | **18.8 ms** | 40.7 ms |
| Filtered (file type) | 1.4 ms | **11.2 ms** | 71.2 ms |

| p95 by kind | Rare | Common | Phrase | Prefix | OR |
|---|---|---|---|---|---|
| Unfiltered | 2.8 ms | 15.0 ms | 20.2 ms | 40.7 ms | 4.6 ms |
| Filtered | 2.2 ms | 22.7 ms | 11.3 ms | 71.2 ms | 2.1 ms |

**Met.** R1 is retired; the Tantivy fallback (ADR-0010) is not needed.

**Worst cases, outside the 100.** Ranking scores every match, as ADR-0010 expected:

| Query | Segments matched | Time |
|---|---|---|
| `generated` (TD-2's most common word) | ≈ 990,000 (89%) | 278–281 ms |
| `c*` | — | 135 ms |
| `co*` | — | 84 ms |
| `"generated example"` | — | 20 ms |

A single word found in almost every segment is over 200 ms. **ASSUMPTION:** real documents rarely have such a word; TD-2's text is generated. Recorded, not fixed.

### Index size (NFR-CAP-08)

TD-2 with the index: 10,000 files done, 0 failed, 1,113,276 segments.

| | Bytes | × stored text |
|---|---|---|
| Stored text (`segment.text`) | 233,738,861 | 1 |
| FTS5 index | 100,132,094 | 0.43 |
| `project.db` | 540,565,504 | 2.31 |

Within ≤ 3× (no vectors yet). Refilling the index (`'rebuild'`, what the v4 → v5 migration does): **4.05 s**.

### Ingest cost (NFR-PERF-22/34, NFR-CAP-05)

Same machine, same day; "No index" is the M3 code.

| | No index | Trigger per row | Bulk per commit (built) | Target |
|---|---|---|---|---|
| TD-2 Stage A (`nfr_res_02_stage_a_memory_td2_all`) | 171.6 s | 192.1 s | **178.8 s** (+4%), peak 1,252 MB | — |
| W3a-1 `rows-1m.csv` (NFR-PERF-22) | 439,463 rows/s | 76,709 rows/s | **230,298 rows/s** | ≥ 50,000 |
| W3a-1 500 MB CSV (NFR-CAP-05) | 16.2 s | 101.0 s | **32.4 s**, peak 70 MB | ≤ 120 s |

TD-2 ingest with the index (re-scan + Stage A): 180.2 s; commit p95 29.7 ms, max 1,097 ms (NFR-PERF-34: ≤ 2 s). `project.db` after the 500 MB CSV: 1,611 MB (1,272 MB without the index).

**Met.** Every target holds; the CSV path is about half as fast as in M3.

### Found and fixed during the spike

- **Prefix highlight.** `highlight()` expands a prefix over the whole index again for every row: `con*` took 407 ms, ≈ 8 ms per row. Each prefix is now replaced, in the highlight query only, by the words that start with it in the page's texts (at most 256; more, or a stem with punctuation, stays a prefix): `con*` 40 ms (`Query::highlight_fts5`). The match itself still uses the prefix. A word the index folds and this does not (`cafe*` and `café`) is a result without its mark. Rejected: FTS5 prefix indexes (`prefix='2 3'` grew the index from 100 to 238 MB and only helps those lengths; `prefix='3'` 162 MB, `co*` still 640 ms).
- **Insert trigger.** A trigger per inserted segment made commits ≈ 7× slower (500 MB CSV 101 s against 120 s). One `INSERT … SELECT` per commit: 1M segments in 2.8 s against 13.2 s (a SQLite check outside the app; `'rebuild'` 2.74 s, no index 0.56 s).

### Deviations from the plan

- **Rare words.** No TD-2 word is in 2–20 segments (generated text), so the rare pool is the 2,000 least frequent words; each is in ≈ 99 segments or more.
- **Phrases.** Adjacent words must be distinct; before, most phrases were the same repeated filler word.

---

## 2. Other M4a checks

### Kill test (NFR-REL-02/03/04)

```sh
target/release/mangkon-cli kill-test --source testdata/td1/synthetic --runs 20 --seed 1
```

**20 / 20** passed, all kills during ingestion, 2–4 items recovered per kill, 0 orphan workers. The health check now includes the FTS5 integrity check. **Met.**

### Rebuild equals incremental

`mangkon-cli ingest` of TD-1 (synthetic and public) and TD-3 (155 files: 130 done, 25 failed as expected, 16 connections), then `mangkon-cli rebuild`: 112 identifiers, 16 connections, "Same as before: yes", "Matches the sources: yes", "Search index in step: yes", exit 0.

### Tests (US-06.1, NFR-SEC-14)

| Check | Test |
|---|---|
| AC1: every result has its document, locator and snippet (INV-SR-01) | `us_06_1_ac1_results_have_document_locator_snippet`, `inv_sr_01_every_result_has_segment_and_locator` |
| AC2: words, phrases, prefixes, OR; no FTS5 syntax gets through; any typed text is a valid FTS5 query | `us_06_1_ac2_syntax`, `us_06_1_ac2_no_fts5_syntax_gets_through`, `us_06_1_ac2_empty_queries`, `us_06_1_ac2_any_query_is_valid_fts5` (proptest on a real index), `us_06_1_ac2_syntax_on_the_index` |
| AC3: file type and document filters | `us_06_1_ac3_filters` (store and TD-1) |
| AC4: a hit opens its place, highlighted, for each locator kind on TD-1 | `us_06_1_ac4_hit_opens_highlighted`, `search.test.tsx` (one click, one `search_context` call) |
| AC5: no matches | `us_06_1_ac5_no_matches` |
| Missing documents badged (domain D4) | `d4_missing_documents_badged` |
| Index follows ingest, re-scan, remove, re-commit, migration | `fts_follows_ingest_rescan_and_remove`, `index_follows_recommits`, `migration_v4_to_v5_fills_the_index` |
| Card-like numbers masked in snippets (NFR-SEC-14) | `nfr_sec_14_masks_card_like_numbers`, `nfr_sec_14_search_snippets_mask_card_like` (TD-1 expense CSV), `search.test.tsx` |

---

## 3. M4b — built-in identifiers and the benchmark

### Pre-checks

| | P1 phone library | P2 Public Suffix List |
|---|---|---|
| Question | Does `phonenumber` 0.3.10 take every TD-1 phone, in its file's region, as valid, and reject dates, amounts and invoice numbers? | Does `psl` 2.1.245 give the ground-truth registrable domain for every TD-1 domain? |
| Result | Generator v4: **79 / 104.** The 22 Ofcom `07700 900xxx` numbers fail (libphonenumber leaves that drama range out of its metadata on purpose), and so do 3 US national numbers in GB files. `rlibphonenumber` gave the same 79. A length-only check passes dates under GB, so it was rejected. Generator v5 uses the Ofcom `0113 496 0xxx` and `0161 496 0xxx` ranges instead (`07-test-corpus.md` §7): **104 / 104**, and **0 of 13** traps (dates, amounts, invoice and reference numbers) valid. | **185 / 185**, including `edis.usitc.gov` → `usitc.gov`, `tc.gc.ca`, `abc.airbus` and the unlisted `x.example` → `x.example`. |
| Verdict | Passed after the corpus change; no ADR (the identifiers stay fictional, only the range moved). | Passed. |

**Build size.** The release `mangkon-cli`, stripped: 22.85 MB at M4a (`16fa037`), 26.14 MB now, **+3.29 MB**. Most of it is the compiled-in libphonenumber metadata and Public Suffix List; the CLI also gained the bench scorer. The desktop app gains the same identifier crates, without the scorer.

### Benchmark on TD-1 (NFR-QA-03..06)

```sh
cargo run --release -p mangkon-cli -- bench --baseline crates/mk-bench/baseline/td1.json
```

`synthetic/` with the phone region GB, `public/` with US, scored together (`07-test-corpus.md` §8). Recall counts every annotated mention; precision only finds in fully annotated files. About 3 s on RM-1, not a timing.

| Type | Expected | Recall | Text-native | OCR'd | Precision |
|---|---|---|---|---|---|
| All | 556 | 95.3 | 98.6 | 73.2 | 99.7 |
| Email | 121 | 95.9 | 97.2 | 84.6 | 100.0 |
| Domain | 185 | 97.8 | 100.0 | 80.0 | 100.0 |
| URL | 63 | 88.9 | 92.9 | 57.1 | 96.7 |
| Phone | 104 | 99.0 | 100.0 | 92.3 | 100.0 |
| IBAN | 68 | 94.1 | 100.0 | 69.2 | 100.0 |
| BTC | 6 | 66.7 | 100.0 | 0.0 | 100.0 |
| EVM | 4 | 75.0 | 100.0 | 0.0 | 100.0 |
| TRON | 3 | 66.7 | 100.0 | 0.0 | 100.0 |
| SOL | 2 | 50.0 | 100.0 | 0.0 | 100.0 |
| Checksum types (IBAN, BTC, EVM, TRON) | 81 | 90.1 | | | 100.0 |
| Other types | 475 | 96.2 | | | 99.7 |

OCR'd recall by scan tier: **clean 100.0** (24 mentions), office 85.2 (27), poor 25.0 (20).

| Target | Result | |
|---|---|---|
| NFR-QA-03 precision, checksum types ≥ 99% | 100.0 | **Met** |
| NFR-QA-04 precision, other types ≥ 95% | 99.7 | **Met** |
| NFR-QA-05 recall, text-native ≥ 95% | 98.6 | **Met** |
| NFR-QA-06 recall, OCR'd clean scans ≥ 85% | 100.0 | **Met** |

Negatives: 8 checked, 0 failed (bad checksums not stored, card-like numbers only flags, stop-list domains not connected). Failed files: 0. Every stored document maps to a ground-truth document.

- **Misses (26 mentions).** Text-native: URLs and emails wrapped across a line in the public PDFs, where the printed hyphen may or may not belong to the value. OCR'd: damage on `office` and `poor` scans (`www_`, `TL` for `77`, swapped letters in crypto addresses, so a checksum fails, as it should). The one false positive is a URL cut short by OCR.
- **Found and fixed by the first run** (OCR'd 71.8%, phone text-native 89%): phones wrapped after `)` or a dash in PDF text; IBANs regrouped or wrapped by OCR (any grouping now, separated by one space or a line break; the country length and mod-97 decide); an email domain wrapped after a hyphen. Unit tests for each in `mk-identifiers`.
- **Poor scans** stay at 25%: recorded as an open point (`07-test-corpus.md` §10); no OCR correction in M4b.

### Gate (NFR-MNT-05)

`crates/mk-bench/baseline/td1.json` holds the numbers above. The run exits 1 when a negative check fails, a stored document maps to no ground-truth document, or any metric falls more than 2 points below the baseline (`compare`, unit-tested with a raised baseline). CI job `bench` (Ubuntu 22.04) caches `testdata/td1/public`, fetches it on a miss, generates and checks TD-1, runs the corpus tests with `MK_REQUIRE_TESTDATA=1` and then the gate. `cargo xtask ci` runs the gate when `testdata/td1` exists and says it skipped it otherwise. **Not verified here:** the CI job itself (it runs on push).

### Kill test and rebuild

```sh
target/release/mangkon-cli kill-test --source testdata/td1/synthetic --runs 20
```

**20 / 20** passed with the new identifier types and card-like flags. **Met.**

`mangkon-cli ingest` of TD-1 (synthetic and public) and TD-3 (155 files: 130 done, 25 failed as expected), then `mangkon-cli rebuild`: 267 identifiers, 62 connections (M4a: 112 and 16), "Same as before: yes", "Matches the sources: yes", "Search index in step: yes".

### Tests

| Check | Test |
|---|---|
| US-04.1 AC1–AC2 | `us_04_1_ac1_email_is_lower_cased`, `us_04_1_ac2_obfuscated_email_is_not_detected` |
| US-04.2 AC1–AC4 | `us_04_2_ac1_international_format`, `us_04_2_ac2_national_format_with_region`, `us_04_2_ac3_national_format_without_region`, `us_04_2_ac4_no_false_phones`; region change re-runs from the stored text: `us_04_2_ac2_region_change_reruns` (core) |
| US-04.3 AC1–AC3, INV-LK-03 | `us_04_3_ac1_url_and_domain`, `us_04_3_ac1_registrable_domain_by_psl`, `us_04_3_ac2_domain_from_email`, `us_04_3_ac3_default_stop_list`, `us_04_3_ac3_stop_list_edit_applies_at_once`, `inv_lk_03_stop_list_domain_is_not_a_connection` |
| US-04.4 AC1–AC2 | `us_04_4_ac1_valid_iban_is_stored_compact`, `us_04_4_ac2_invalid_iban_is_not_stored` |
| US-04.5 AC1–AC4 | `us_04_5_ac1_bitcoin_checksum`, `us_04_5_ac2_evm_eip55`, `us_04_5_ac3_solana_confidence`, `us_04_5_ac4_tron_checksum` |
| US-04.6, NFR-SEC-14 | `us_04_6_ac1_card_like_is_flagged_masked`, `us_04_6_ac2_card_like_is_never_a_key`, `us_04_6_card_like_is_a_masked_flag` (core, also after a re-run), `nfr_sec_14_td1_card_like_is_only_a_flag` (TD-1), the bench negatives, `m4b.test.tsx` (badge, masked numbers) |
| Every type on TD-1 text | `td1_text_identifiers.rs` (`mk-identifiers`) |
| Spans slice back to the raw text (INV-CO-04) | proptests per finder, `inv_co_04_roundtrip_*` on TD-1 |

## 4. M4c — custom patterns and column mapping

### Benchmark on TD-1 (NFR-QA-13, US-04.7, US-05.3)

`mangkon-cli bench` adds the pattern `tender_ref` = `VPA-TND-\d{4}-\d{3}` before ingest. After ingest it records the suggested type of every CSV and JSON column. Then it confirms the suggestions as they are and scores the "Name (from table)" mentions. Custom matches have their own rows, so the NFR-QA-03..06 rows stay built-in only and are unchanged. The baseline (`crates/mk-bench/baseline/td1.json`) was refreshed on purpose; the refresh only added rows.

| Row | Expected | Result |
|---|---|---|
| `mapping/csv` (NFR-QA-13, target ≥ 90%) | 23 columns | 23 correct, **100%**, met |
| `mapping/json` | 38 columns | 38 correct, 100% |
| `mapping/all` | 61 columns | 100% |
| `custom/all` (text 32, OCR 7) | 39 mentions | recall 100%, precision 100% |
| `table-names/all` | 42 mentions | recall 100%, precision 100% |

The column types are labelled by hand in the generator (v6, `07-test-corpus.md` §5). The suggester's header words are generic, and the unit tests use headers that are not in TD-1. JSON columns are the keys of string and number values. `true`/`false` and `null` values are not read as text, so they are not columns.

### Kill test and rebuild

- **Kill test.** `mangkon-cli kill-test` on TD-1 now adds the pattern `tender_ref` before each run, so the custom finder and the table step run in every Stage A commit. Result: 20/20 runs passed.
- **Mapping confirms.** The CLI has no mapping command, so no confirmed mapping took part in the kill test. A confirm is one transaction (decision and apply). Its survival across re-scans and rebuilds is checked by `ignore_on_an_entity_survives_rescan_and_rebuild`, `us_05_2_ac3_rescan_reapplies_or_needs_review` and the property test.
- **Rebuild.** It equals the incremental rows and matches the sources on TD-1 + TD-3 (316 identifiers, 155 connections; the search index is in step).

### Measurements

Pending: the custom-pattern re-run on TD-2, Stage A with 3 patterns on the 500 MB CSV (NFR-CAP-05, NFR-PERF-22) and the confirm on `s2a/rows-1m.csv`. They need an idle machine.

### Tests

| Check | Test |
|---|---|
| US-04.7 AC1 live test, type named after the pattern | `us_04_7_ac1_matches_named_after_the_pattern` (`mk-identifiers`), `us_04_7_ac1_pattern_matches_named_after_it`, `us_04_7_ac1_test_marks_matches` (core), `m4c.test.tsx` |
| US-04.7 AC2 refused syntax, linear time | `us_04_7_ac2_invalid_patterns_are_refused`, `us_04_7_ac2_test_refuses_look_around`, `us_04_7_ac2_linear_time` (`(a*)*b` on 100k `a`) |
| US-04.7 AC3 only the custom step | `us_04_7_ac3_custom_rerun_leaves_builtin_alone` (store), `us_04_7_ac3_only_the_custom_step_runs` (the source files are deleted first; built-in mention ids and card flags stay), `us_04_7_ac3_paused_queue_holds_the_custom_rerun`, `us_04_7_ac3_killed_rerun_resumes_from_its_cursor` |
| US-04.7 AC4 | `us_04_7_ac4_too_broad_warns` (10,001 matches) |
| INV-ID-07/08 | `inv_id_07_unique_name_ignoring_case`, `inv_id_08_steps_are_independent` |
| US-05.1 | `us_05_1_ac2_identifier_columns_by_share`, `us_05_1_ac2_names_from_header_words` (`mk-mapping`), `us_05_1_suggestions_make_no_entities` (INV-CM-02/03) |
| US-05.2 | `us_05_2_confirm_and_reuse`, `us_05_2_confirm_and_reuse_make_table_entities`, `us_05_2_ac3_rescan_reapplies_or_needs_review` (D5) |
| US-05.3 | `us_05_3_ac1_entity_span_inside_cell`, `us_05_3_json_names_from_table` (CsvCell / JsonValue locators, record links) |
| Decisions survive | `ignore_on_an_entity_survives_rescan_and_rebuild`; `derived_property.rs` gains pattern, re-run, confirm and entity-ignore operations (48 cases in CI, 400 run once) |
| UI | `m4c.test.tsx`: pattern label, locked name, too-broad text, mapping pill, mapping form, "Name (from table)" |

## 5. M4d — Connections screen and acceptance

### What changed

- **Derived `connection` columns** (`DERIVED_VERSION` 4, no schema migration, ADR-0005): `kind`, `value` (an entity's display name; a custom pattern's value without its label) and `file_types` (counted file types, sorted, `,csv,eml,pdf,`). Filter, find, sort and the row need no join. They are computed in the same rebuild step as the counts, so a full rebuild gives the same rows. A project made by an older app is rebuilt on first open.
- **Query.** `ConnectionQuery { kinds, min_sources, file_types, find, sort, descending, include_ignored }`. The default order is domain §9.2. Any other column (value without case, type, sources, mentions, file types) breaks ties by the subject key, and the keyset cursor carries the last row's value and type. `find` is a case-insensitive `LIKE` contains with `%`, `_` and `\` escaped. `connection_count(query)` and `ignored_count()` feed the list length and the "Show ignored (N)" label.
- **Evidence.** Each occurrence carries a snippet (`mk_search::snippet` over the card-masked segment text, the value marked) and its document id; the panel groups by document (name, file type, count, then place and snippet per mention).
- **Screen.** Filter bar (type incl. "Name (from table)", minimum sources, file type, find with a 250 ms delay, Show ignored), sortable headers with `aria-sort` (click, flip, back to the ranking), Ignore / Un-ignore per row, empty states, and the "N new connections — Show" bar. The list holds still while the user has scrolled or has a connection open: `useKeysetList` stops refetching the pages (the count keeps following) and `refresh()` loads them on "Show" or after an ignore.
- **Keyboard.** List: ↑ ↓ Home End PageUp PageDown Enter (as before), `i` ignore / un-ignore the active row, `/` focuses Find from anywhere on the screen, Esc in Find returns to the list. Tab order is DOM order: filter bar, sort headers, list, evidence, viewer. Ignore buttons inside rows are not tab stops (`i` does the same).
- **NFR-QA-10 in `check_health`.** `HealthReport.weak_connections` lists connection keys with fewer than two counted documents; the kill test uses `is_healthy`, so it now checks this too.

### Deviations from the plan

- `kind`, `value` and `file_types` are stored in the `connection` table only, not in the `mk-graph` `ConnectionRow` or reference model. They are functions of the same stored mentions, and the rebuild-equals-incremental check still compares the ranking row; the new columns are rebuilt by the same code in both paths and are covered by `us_07_1_filters_and_find` and `nfr_rel_09_decisions_survive`.
- No index was added for the sort columns yet; see Measurements.
- "N documents" in the empty state counts files read (done), not skipped or failed ones.

### Tests

| Check | Test |
|---|---|
| US-07.1 AC2/AC3, find | `us_07_1_filters_and_find` (core), `connections.test.tsx` (row, filter bar) |
| US-07.2 sort and paging | `us_07_2_sort_pages_without_gaps` (every column, both directions, page sizes 1–3 equal one page), `connections.test.tsx` (header cycle, `aria-sort`) |
| US-07.3 AC1 snippets | `us_07_3_occurrence_snippets` (value marked; card-like number masked, NFR-SEC-14), `connections.test.tsx` (grouping, one button per mention) |
| US-07.4 | `us_07_4_ignore_and_show_ignored`, `connections.test.tsx` |
| US-07.5 | `connections.test.tsx` (bar, count). The hold itself (`useKeysetList` with `paused`) has no automated test: the UI tests have no DOM. Check it by hand (below). |
| US-07.6 | `us_07_6_summary_counts_documents`, `connections.test.tsx` (messages) |
| NFR-QA-10 | `nfr_qa_10_synthetic`, `nfr_qa_10_td1` (health check and ≥ 2 mentions in ≥ 2 documents per connection, ignored ones too) |
| NFR-REL-09 | `nfr_rel_09_decisions_survive`: ignore / un-ignore of an identifier and of a mapped entity survive a re-scan with a changed file, "Re-process" of identifiers, a full rebuild, and a re-open after an older derived version (rebuilt on open) |

Rebuild: equals the incremental rows and the sources on TD-1 + TD-3 (`mangkon-cli rebuild`: 138 identifiers, 51 connections in this run; the search index is in step).

### Measurements (RM-1, idle, 2026-11-03, `cargo xtask build --release`, Wayland)

**Kill test and bench.** `mangkon-cli kill-test --source testdata/td1/synthetic --runs 20 --seed 1`: **20 / 20** passed; the same on TD-3 (hostile files, `--source testdata/td3`): **20 / 20**. `mangkon-cli bench --baseline crates/mk-bench/baseline/td1.json`: gate passed, NFR-QA-03..06 and 13 unchanged (100 / 99.7 / 98.6 / 100 / 100), 0 failed files.

**NFR-PERF-10 (TD-1, first cross-source identifier ≤ 10 s).** `cargo run --release -p mk-core --example m4d_ingest -- testdata/td1/synthetic` (fresh project, `MK_WORKER_PROGRAM` set to `mangkon-cli`), 3 runs: a poll of the store sees the first connection **0.02–0.04 s** after the run starts; the app shows it after the next 500 ms debounce plus one render, so about 0.55 s. The whole run takes 1.6–1.7 s (81 files). The run starts after the folder is added, so the folder dialog and the window's own start-up are not in the figure. **Met.**

**NFR-PERF-34 (≤ 2 s after a file finishes).** Commit side, same example on TD-2 (10,000 files, 247.6 s, 8,698 commits that changed connections): commit latency p50 5.9 ms, **p95 38.5 ms**, max 4,183 ms (M4a: p95 29.7, max 1,097 ms). Screen side in the release app on a TD-2 ingest: 343 events, p50 8 ms, **p95 19 ms**, max 33 ms. So p95 end to end is about 0.56 s. The pass mark is stated on p95, but the max is above 2 s: one commit waited 4 s (the writer was busy with something large, probably one of the big files); I did not trace it. **Met on p95; the outlier is open.**

**NFR-PERF-02 (TD-2 opens to a usable connections screen ≤ 2 s).** `cargo run --release -p mk-core --example m4d_timing -- <project> 10` on a TD-2 project (10,000 files, 255,600 connections): open + summary + count + first page of 60, 10 runs, p50 28 ms, **p95 48 ms**. This is the core path with a warm file cache; the UI adds one render and the 60-row page. **Met** (not the window start-up, which is the shell's).

**Filter / find / sort at 255,600 connections (pass mark ≤ 100 ms p95).** Count + first page of 60, 10 runs each, same project:

| Query | Matches | p95 |
|---|---|---|
| default order | 255,600 | 9.2 ms |
| type = email | 4,954 | 11.6 ms |
| ≥ 3 sources | 255,416 | 9.1 ms |
| file type = pdf | 160 | 17.6 ms |
| find "acme" (no match, full scan) | 0 | 37.9 ms |
| find "1" (broad) | 167,839 | 16.6 ms |
| sort value ↑ / ↓ | 255,600 | 24.9 / 23.9 ms |
| sort type | 255,600 | 24.4 ms |
| sort sources ↓ / mentions ↓ / file types | 255,600 | 22.0 / 22.2 / 22.8 ms |
| type + find + sort value | 4,947 | 24.8 ms |

All under the mark, so no index on the sort columns and no `connection_find` table. **Met.**

**NFR-PERF-04 / NFR-QA-12 on the new list (`MK_VIEW_SWEEP=clicks`, 50 seeded clicks).** TD-1 (81 files, in-app ingest): p50 33 ms, **p95 67 ms**, max 100 ms; 50 / 50 highlighted the right text; Next then Back restored the scroll in 11 / 11; kinds email 14, page 18, paragraphs 6, text 7, table 4, json 1. TD-2 project: 50 clicks (all table), p95 50 ms, 50 / 50. **Met.**

**Observation: a click-sweep run that never finished (not reproduced).** Twice during the first idle run the sweep on a TD-1 project made with `mangkon-cli ingest`, in a fresh app data folder, never produced output (killed after 60–120 s). I then ran it 7 times more on fresh data folders and CLI-made projects (first open each time) and every run finished in 5–9 s with 50 / 50 right, so a missing page cache or `tessdata/` is not the cause. A run with `MK_UI_TIMING` unset also sits idle, because the scripted open needs it; that may explain some of the first hangs. The sweep waits on animation frames, which a hidden window may not get; unconfirmed. Not an M4d change.

### Not done yet (needs a person)

- **Timings:** done above. Open: the 4.2 s commit outlier on TD-2.
- **Hand walk on Linux (NFR-USE-06):** Tab through filter bar → list → evidence → viewer; `/`, `i`, arrows; scroll the list or open a connection, add files, see "N new connections — Show", click Show.
- **First demo (criterion 8):** drop TD-1 → connections → pick a cross-source email → evidence by document → one click to the source → Ignore → Show ignored → Un-ignore. The recording moved to M9 (2026-11-03) and is the maintainer's.
