# 12 — Layer 1: Mangkon AI v0.1

> Input: `06-roadmap.md` §2 M4 (spike S4), ADR-0010, `03-nfr.md` (NFR-PERF-22/30/34, NFR-CAP-05/08, NFR-REL-02, NFR-SEC-14). **If this file disagrees with them, they win.**
> Status: §1–§2 measured in M4a on RM-1 (2026-10-30). Linux only; macOS and Windows are not verified here.
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
