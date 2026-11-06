# 16 — Entities and M6 acceptance

> Input: `06-roadmap.md` §2 M6c, `02-requirements.md` US-07.7, US-10.1, US-10.3, US-11.1, `03-nfr.md` (NFR-QA-08/09, NFR-PERF-12/13/24, NFR-SEC-03, NFR-MNT-06), `04-domain.md` D6, D7, INV-AI-03, `09-model-check.md`, `14-ai-extraction.md`, `15-meaning-search.md`. **If this file disagrees with those, they win.**
> Status: M6c on RM-1 (2026-11-06). Automated checks, six benchmark runs of the 4B on TD-1, a run with the network off and a scripted walk of the release app are recorded here. Linux only; macOS and Windows are not verified here, and neither is RM-2. How the screens look was not checked by hand.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Decisions

| Decision | Choice |
|---|---|
| D7 (taken early, 2026-11-06) | An entity's linked identifiers come only from the same CSV row or JSON record (`record_link`) and from `Name <address>` email headers, counted contents only. No LLM linking. Domains are not linked. D6 stays open for M7a. |
| Linked identifiers | Read when a page is built, for that page's names. No schema change. A row shows the first few and the total. |
| Type filter | All types, People, Organizations, Identifiers (all identifier kinds), and the per-type identifier choices. "Name (from table)" stays as a badge only on entities whose mentions all come from tables. |
| Pass rule, fixed before measuring | NFR-QA-08/09 use the 4B on real TD-1 Stage A output with 4 slots, on shown mentions (confidence ≥ 0.5). Three runs; the median decides (ASSUMPTION). |
| Scoring cell | (document, page) for PDF and scan, (document) otherwise. Public files have no name annotation and are not scored; CSV and JSON gold is Layer 1 and left out. The whole ledger is scored, and DOCX and HTML are included. |
| Gold | Not changed after seeing results. The known gap "Port Authority" (`09-model-check.md` §11) stays, and counts against precision. 359 cell-names: person 137, org 222. |
| Network off | A network namespace: `unshare -rn` with `lo` up. It covers the CLI path, not the desktop's webview. |
| Reply cut at the token limit | **Not retried** (INV-AI-03 amended, 2026-11-06, §4). A reply that stops at the limit ends the unit with 0 mentions and counts as unread. A reply that parses is accepted whatever its finish reason. |
| `MAX_TOKENS` | Stays 2,048 (§4). |
| "Experimental" label | **Dropped** (2026-11-06, the M1 rule, §8). Off by default; `MK_AI_EXPERIMENTAL=1` at build time turns it on (US-10.3). |
| NFR-PERF-24 | Miss accepted, target unchanged (2026-11-06, §4). |
| Start rule | `STAGE_B_AUTO_UNITS` stays 1,600. TD-1 has 970 units and must start by itself for NFR-PERF-12. `STAGE_B_UNITS_PER_SECOND` is now the measured 0.82, so 1,600 units is about 32 min, not 20. |

## 2. Entities on the connections screen

**Checks.** `crates/mk-core/tests/m6c_entities.rs` and `ui/src/project/m6c.test.tsx`, against the stand-in `llama-server`:

- `us_11_1_ac1_*`: case, spaces, punctuation and legal-suffix variants across documents make one connection; a person and an organization with one name stay apart; table and AI names share a key.
- `us_07_7_ac1`, `us_07_7_ac2_*`: rows from 2+ sources; linked identifiers from a CSV row and from a header; one is gone when its content is Missing.
- `us_07_7_ac3_*`: mentions carry a snippet, a confidence and a locator that opens.

**Timing on TD-2** (`linked_identifiers_on_td2`, `#[ignore]`, RM-1, idle): TD-2 with 694 tables confirmed, 495,025 entity connections, 2,727,572 record links, 1,584 header mentions. A page of 200 rows:

| | p50 | p95 | max |
|---|---|---|---|
| Rows only | 21.6 ms | 22.4 ms | 22.8 ms |
| Linked identifiers | 16.3 ms | 17.3 ms | 18.3 ms |
| Whole page | 39.3 ms | 40.8 ms | 41.4 ms |

The AI names here came from the stand-in, which found TD-1's gold names: 15,642 mentions over 92,268 units. Real AI output on a project this size was not timed. The budget is NFR-PERF-02: a usable screen in 2 s, of which this read is one part.

## 3. LLM benchmark and results

**The tool** (NFR-MNT-06, replaces `spikes/model-check`). `crates/mk-bench/src/names.rs` scores; `apps/cli/src/llm_bench.rs` runs:

```sh
mangkon-cli llm-bench --testdata testdata/td1 --model qwen3.5-4b-q4_k_m \
  --embedding-model bge-m3-q8_0 [--runs N] [--app-data DIR --dir DIR --llama-server PATH --work DIR --json FILE]
```

One run makes a fresh project, then runs Stage A, then Stage B and embeddings on their own threads as the app does. `synthetic/` and `public/` are two roots of one project so that the ground-truth files are not read as evidence. That gives 195 embedding parts and 3,496 windows, against M6b's 711 parts and 19,995 vectors, which had included them; the 970 Stage B units are unaffected. False positives and missed names go only to `--json`. Recall@20 is taken on TD-1 with M1's 100 queries (`crates/mk-bench/data/queries.json`) at the production pool, against exact search. `apps/cli/tests/llm_bench.rs` checks it on the stand-in: inventing names must lower precision.

**Results.** Shipped settings (cap 2,048, no retry on a cut reply), release build, RM-1 idle, 4 slots, 2026-11-06 06:29–07:28 UTC. Run c is the network-off run.

| Target | a | b | c | **Median** | Result |
|---|---|---|---|---|---|
| NFR-QA-08 precision, shown, ≥ 85% | 93.29 | 93.64 | 92.73 | **93.29** | met |
| NFR-QA-09 recall, shown, ≥ 70% | 85.24 | 86.07 | 85.24 | **85.24** | met |
| NFR-PERF-12 first person/org, ≤ 300 s | 11.22 | 11.72 | 11.22 | **11.22** | met |
| NFR-PERF-13 both lanes after Stage A, ≤ 1,200 s | 1161.69 | 1184.29 | 1185.19 | **1184.29** | met, by 16 s |
| NFR-PERF-24 extraction, ≥ 1 unit/s | 0.83 | 0.82 | 0.82 | **0.82** | **not met** |
| ADR-0010 recall@20, ≥ 0.9 (worst query) | 1.0000 (1.00) | 0.9995 (0.95) | 0.9995 (0.95) | **0.9995** | met |

| Counts | a | b | c |
|---|---|---|---|
| True / false positives / missed | 306 / 22 / 53 | 309 / 21 / 50 | 306 / 24 / 53 |
| Units, failed | 970, 0 | 970, 0 | 970, 0 |
| Mentions | 3,027 | 3,023 | 2,968 |
| Distinct entities | 1,560 | 1,560 | 1,539 |
| Discarded by evidence check | 1,007 | 995 | 962 |
| Unread replies | 26 | 30 | 29 |
| On an identifier (dropped) | 44 | 47 | 45 |
| Linked pairs correct of 39 (predicted) | 36 (38) | 36 (38) | 37 (40) |

Stage A took 2.7 s each time. Embeddings took 27.4–28.0 s with 0 failed parts, while extraction ran. Files failed 0, `rebuild` matches, healthy, unmapped documents 0, in all three.

| Shown P / R | a | b | c |
|---|---|---|---|
| Person | 97.0 / 95.6 | 97.0 / 94.9 | 94.9 / 94.9 |
| Organization | 90.7 / 78.8 | 91.3 / 80.6 | 91.2 / 79.3 |
| Text PDF | 97.5 / 86.8 | 98.3 / 85.3 | 97.5 / 87.5 |
| Ledger | 100 / 100 | 100 / 100 | 100 / 100 |
| Scan | 85.1 / 79.2 | 85.3 / 80.6 | 85.1 / 79.2 |
| OCR layer | 100 / 77.8 | 100 / 77.8 | 100 / 77.8 |
| Notes | 96.8 / 81.1 | 96.8 / 81.1 | 96.8 / 81.1 |
| Email | 88.4 / 89.7 | 89.0 / 95.6 | 85.7 / 88.2 |
| DOCX | 100 / 73.3 | 100 / 73.3 | 100 / 73.3 |
| HTML | 100 / 100 | 100 / 100 | 100 / 100 |

**What the numbers say.**

- **Shown equals all.** About 70% of mentions have a confidence of 0.9 or more, and only 0–9 per run fall below 0.5 (0, 9 and 4 here). The 0.5 threshold changes nothing on the scored names. This also answers M6a's open point on OCR confidence as far as it goes: the scan row has no low-confidence mentions to hide. Several false positives look like OCR misreads (`trene vasquez`, `brightmater logistics`, `yelmora port authority`). Requirements still set the model's confidence as the stored one.
- **Errors are mostly organizations.** Most missed names are organizations, led by short forms (`brightwater`, `vpa`, `kestrel`) and `velmora port authority`. False positives include misreads, longer forms (`brightwater logistics accounts`), and odd pieces (`localhost`, `usdt`, `vpa tnd 2024 031`). Names here are name keys, from the runs' `--json`. No bare domain appears among them, so the filter from `09-model-check.md` §5.3 was not added.
- **Against M1.** M1 measured the 4B at 81.7% precision and 93.4% recall on 111 segments of reference text. These runs are on the whole of TD-1 through real Stage A output, with a different scoring cell and sources M1 left out, so the two are not directly comparable. M6a added the evidence check, the name-inside-evidence rule and the identifier overlap drop since then; how much of the rise each one gives was not tested.
- **Prompt risk.** M1 settled the prompt on 8 TD-1 segments and no fresh corpus slice exists (`09-model-check.md` §11). The numbers carry that small risk.

## 4. Getting NFR-PERF-13 down

The first measurements missed NFR-PERF-13. Units whose replies run to the 2,048-token limit take a large share of the slots' time (`14-ai-extraction.md` §4), and each was retried once. All columns use the same corpus and the idle machine.

| | Cap 2,048, retried (3 runs, median) | Cap 1,024, retried (1 run) | **Cap 2,048, not retried (3 runs, median)** |
|---|---|---|---|
| Precision, shown | 93.09 | 93.31 | **93.29** |
| Recall, shown | 86.35 | 85.52 | **85.24** |
| Both lanes | 1,429 s, not met | 1,204 s, not met | **1,184 s, met** |
| Extraction | 0.68 units/s | 0.81 units/s | **0.82 units/s** |
| Mentions | 3,085 | 2,843 | **3,023** |
| Distinct entities | 1,549 | 1,402 | **1,560** |
| Unread replies | 16 | 23 | **29** |

- **Retries saved few names.** Without them, at the median, unread replies rose by 13, mentions fell 2%, distinct entities did not fall, and recall fell by 4 gold names of 359 (310 to 306 found), which is 1.1 points. Precision did not fall. The time fell 17%.
- **A lower cap costs names gold cannot see.** A cut reply loses every name in its unit. At 1,024, `fr-2025-03613.pdf` fell from about 229 mentions to 118 and `notes/lena-research.md` from 9 to 6, and the corpus lost about 10% of its distinct entities. Public files are not scored, so precision and recall barely moved. Cap 1,024 was rejected for that, and it also missed the target by 4 s.
- **NFR-PERF-13 has a thin margin.** 16 s at the median, 15–38 s across runs, on an idle machine. Other work on the same GPU could take it over.
- **Amendment.** INV-AI-03 and US-10.1 AC2 now say that a reply cut at the token limit is not retried. `inv_ai_03_cut_reply_is_not_retried` checks it.

**NFR-PERF-24 still misses at 0.82 units/s.** The miss is accepted and the target stays (decided 2026-11-06, `06-roadmap.md` §4 #20).

## 5. Network off (NFR-SEC-03)

Run c ran inside the namespace, along with `rebuild`, `embed status` and one `meaning` query.

| Check | Result |
|---|---|
| Interfaces | `lo` only, 127.0.0.1/8 and ::1/128; no routes |
| Connect to 1.1.1.1:443 | `Network is unreachable` |
| `llm-bench` | exit 0, 970 units, 0 failed; 195 embedding parts, 0 failed |
| `rebuild` | Same as before: yes. Matches the sources: yes |
| `embed status` | ready, 195 of 195, 0 failed, "Data sent to this machine" |
| `meaning` | 20 results |
| `egress_record` | `127.0.0.1`, class `this_machine`, purposes extraction, embeddings, search_query. Nothing else. |

**Met.** One rule set in advance is missed narrowly: run c should sit within the band of the other two, and its precision (92.73) is 0.56 points below it, two more false positives with the same true positives as run a. Recall is in the band. Nothing in a run depends on the network, the first set of three had the namespace run inside the band (93.09 between 93.03 and 93.37), and replies vary run to run. The cause was not separately tested.

`embed status` and `meaning` first failed after this run because the scratch registry already held a project of that name from the earlier set (INV-WS-01, correct behaviour). They were repeated in a new namespace, on the same project, with a copy of `app.db` whose registry was empty.

## 6. Release-app walk

A scripted walk of a release build, real 4B and bge-m3, TD-1, same method as M6a (temporary code, removed; the repo holds none). The binary was built before the no-retry and rate changes, neither of which touches the connections screen.

| Step | Result |
|---|---|
| From adding files to the first person on screen, People filter | **12.6 s** (`D. Okonkwo-Hart`, Person, 2 sources) |
| An entity with linked identifiers, about 12 s later | `Irene Vasquez-Lindqvist`: `irene@brightwater-logistics.example` |
| Type filter while the run goes on | All types 66, People 6, Organizations 6, Identifiers 55, Email 13 connections |
| Entity row | Height 60 px, all lines fit; `Person · 5 sources · 6 mentions`, file types, linked address |
| Evidence | `body part 1 · 90% · Qwen3.5 4B · Experimental`, with the snippet (the label was still on in this build) |
| One click to the source | **22 ms**, highlighted `Irene Vasquez-Lindqvist` |

The desktop shows new connections after a debounce of 500 ms, which is inside the 12.6 s.

## 7. Whether the tests can fail

Each mutation was applied by hand, the named tests run, then undone.

| Mutation | Tests failing |
|---|---|
| Any text in a header pairs a name and an address | 1 (`us_07_7_ac2_linked_identifiers_from_a_row_and_a_header`) |
| Record links not read | 3 |
| Header pairs not read | 3 |
| Missing contents counted in record links | 1 |
| Missing contents counted in headers | 1 |
| Pairs taken from any segment kind | 3 |
| Low-confidence mentions always link | 1 |
| Domains linked from records | 3 |
| Domains linked from headers | 3 |
| No limit on linked identifiers | 1 (`first_few`) |
| Linked identifiers not ranked | 1 |
| Page ignored in the score | 2 |
| Low-confidence mentions counted as shown | 1 |
| CSV gold scored | 3 |
| Public files scored | 1 |
| The chosen AI model not put back after the benchmark | 1 |
| Median takes the first value | 1 |
| UI filter matches on the first kind only | 1 |
| UI linked list dropped | 3 |
| Cut reply retried | 1 (`inv_ai_03_cut_reply_is_not_retried`) |

## 8. M6 acceptance

| # | Criterion | Result |
|---|---|---|
| 1 | US-11.1 AC1 | Met: `us_11_1_ac1_*`, `us_11_1_name_key_agrees_with_ground_truth` |
| 1 | US-11.1 AC2 | **Not in M6.** The split is US-11.4 (M7). `name_key` is ready for M7a's anchor |
| 1 | US-07.7 AC1–AC3 | Met: §2, §6 |
| 1 | US-07.7 AC4 | **M7**, needs proposals |
| 1 | E10, US-06.2 | As recorded in `14-ai-extraction.md` and `15-meaning-search.md`; AC2 of US-10.1 amended (§4) |
| 2 | NFR-QA-08/09 | Met: 93.29%, 85.24% |
| 3 | NFR-QA-11 | As recorded in `14-ai-extraction.md` |
| 4 | NFR-PERF-12, NFR-PERF-13 | Met: 11.2 s (12.6 s in the app), 1,184 s |
| 5 | NFR-PERF-31 | As recorded in `15-meaning-search.md` |
| 6 | NFR-SEC-03 | Met (§5) |
| 7 | Models never mixed | As recorded in `14-ai-extraction.md` |
| Exit | "Experimental" label | **Dropped (2026-11-06).** The M1 rule says drop it if the 4B passes 08/09, and it does. Off by default now; `MK_AI_EXPERIMENTAL=1` at build time turns it on (`mk_ai::run::experimental_from`) |
| Also | NFR-PERF-24 | **Not met:** 0.82 units/s against ≥ 1. Miss accepted (2026-11-06) |

## 9. Open points

1. **What dropping the label rests on.** The score covers TD-1's synthetic files with the 4B. Public files are not scored, and a model on a user's endpoint has not been measured, yet neither shows the label now.
2. **NFR-PERF-24** misses at 0.82 units/s; accepted (roadmap §4 #20). It is measured again with each release's benchmark (NFR-MNT-06).
3. **US-11.1 AC2 and US-07.7 AC4** cannot pass in M6, though criterion 1 reads "all ACs of US-11.1". Read here as depending on M7.
4. **The start rule's time.** 1,600 units is now shown as about 32 min. If 20 min matters more than TD-1 starting by itself, the limit would have to be about 980 units, leaving TD-1 inside it by 10 units.
5. **Citations as organizations** (`1 Stat. 234`, M6a open point 11). Run b holds 7 such entities. They come from public files, which are not scored, so 93.29% does not reflect them. Not fixed.
6. **Evidence matching on rough text** (M6a open point 8). Recall passes, so the folding was not done. About 1,000 mentions per run are still discarded.
7. **Unread replies lose all names in a unit** (26–30 of 970). The user sees the count; nothing retries them.
8. **Not verified here:** macOS, Windows, RM-2, how the screens look, and a real network-off run of the desktop app (the namespace covers the CLI path).
