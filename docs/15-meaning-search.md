# 15 — Meaning search

> Input: `06-roadmap.md` §2 M6b and spike S6, ADR-0010, ADR-0005, ADR-0006, `02-requirements.md` US-06.2, `03-nfr.md` (NFR-PERF-31, NFR-RES-01, NFR-SEC-14), `04-domain.md` BC-10, `09-model-check.md` §6. **If this file disagrees with those, they win.**
> Status: M6b on RM-1. Spike S6, automated checks against the stand-in `llama-server`, real-model runs and a scripted walk of the release app are recorded here. ADR-0010 Accepted 2026-11-05, with the changes in its M6b follow-up. Linux only; macOS and Windows are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Decisions (2026-11-05)

| Decision | Choice |
|---|---|
| Vectors | One per window of up to 500 characters, split at whitespace, no overlap. Stored as f16 with the exact character span. Short rows are kept (ASSUMPTION: a search product must find them; see §6). |
| Work item | Whole adjacent segments of one document, up to 32 windows. One long segment may exceed 32 windows: it goes in several requests and commits once. |
| Start rule | Automatic up to 480,000 windows, about 20 min at 400 windows/s. Above that, nothing starts until the user clicks, with an estimate. |
| Rescoring pool | **1000**, not ADR-0010's 200 (§2). |
| Index in memory | A plain buffer, not memory-mapped: `memmap2` needs `unsafe`, and only `mk-tesseract-sys` may have it. Dropped on release. |
| Local server | `llama-server --embedding -c 8192 -np 4 -b 2048 -ub 2048 --cache-ram 0` (§2). |
| Model changed | New work is Blocked, the old vectors stay, and the Meaning tab offers "Re-run with Y" or choosing the old model again. |
| During a re-run | Meaning search is not ready, because a query must use the vectors' own model. Keyword search works in every state (INV-SR-04). Cancelling changes nothing. |

## 2. Spike S6 on TD-2

**Method.** `crates/mk-core/tests/s6_meaning_td2.rs` (`#[ignore]`, `MK_S6_PROJECT` keeps the scratch project): real TD-2 and real bge-m3 vectors, not the synthetic vectors the roadmap asks for. 100 queries with a fixed seed: M1's 50 hand-written and 50 from TD-2's own text. Each is timed as the whole service call, warm. Exact search over all vectors is the recall reference. Query timing ran on an idle RM-1.

**Embedding run.** 1,337,115 vectors over 1,113,276 segments, 1024 dimensions, mean window 182 characters. The last stretch of 1,239,357 windows took 2,940 s, or 421.5 windows/s, falling from about 495. The lane process stayed at 87 MB RSS. The index is 173 MB, built in 5.1 s. `project.db` is 6,447 MB.

**Result at pool 1000: passes.**

| Pass mark | Target | Measured |
|---|---|---|
| NFR-PERF-31, whole call, top 20 | ≤ 1 s p95 | **22.6 ms** p95 (p50 18.2, max 27.4) |
| Recall@20 against exact search (ADR-0010) | ≥ 0.9 | **0.9875** |
| Index memory when not in use | Within NFR-RES-01 | Index goes on release: see §5 |

No query returned fewer than 20 hits.

| Step | p50 (ms) | p95 (ms) |
|---|---|---|
| Query embedding | 4.2 | 5.2 |
| Binary scan | 4.2 | 7.0 |
| Loading vectors | 1.7 | 2.3 |
| Rescoring | 1.3 | 1.8 |
| Result rows | 0.7 | 1.1 |

The first query after idle is slower and is not counted in the p95, which is for warm queries: 1,412 ms with the model and the index to start (index load 154 ms), 173 ms with the index only (load 152 ms).

Process memory in the spike harness (RSS, MB): 82.1 before, 261.9 with the index loaded, 88.5 after release, 262.6 after queries, 99.3 at the end.

**Recall@20 by pool.** The 200 that M1 passed on TD-1 fails here, so the first fallback in the plan was taken: a larger pool, while p95 holds. int8 and HNSW are not needed.

| Pool | All | Near-ties allowed | Hand-written | From TD-2's text | Worst query |
|---|---|---|---|---|---|
| 100 | 0.723 | 0.746 | 0.649 | 0.797 | 0 |
| 200 | 0.836 | 0.854 | 0.804 | 0.868 | 0 |
| 500 | 0.9365 | 0.945 | 0.92 | 0.953 | 0 |
| **1000** | **0.9875** | 0.989 | 0.99 | 0.985 | 0.5 |

Pool 200 timed at p95 26.2 ms, so a larger pool costs little: the time is in steps that barely grow with it.

Of the 50 queries taken from TD-2's own text, 35 found their own text in the exact top 20. The cause of the other 15 was not verified. TD-2 is 99 near-copies of TD-1, so near-ties are one candidate.

**`-np` comparison** (6,401 windows, machine not formally idle, so read only the direction):

| | Windows/s | Query p95 during the run |
|---|---|---|
| `-np 1` | 253 | 4.3–4.7 ms |
| `-np 4` | 466 | 3.7–3.9 ms |

`-np 4` is the setting, with about 524–532 MiB of GPU memory. `install-llama-cpp.md` matches.

**Switching on TD-2.** A re-run ends in one transaction that drops the old vectors. Timed on a copy of the TD-2 database on an idle RM-1: 1,337,115 vectors deleted in 13.9 s (statements 10.0 s, commit 3.9 s), checkpoint 0.3 s. Other writes wait for that long. 1,348,306 of 1,650,640 pages were free afterwards: the file does not shrink (§6).

## 3. Kill test

`apps/cli/tests/embed_kill.rs`: `SIGKILL` during `mangkon-cli embed run`, 20 kills; all 4 tests pass: the project is healthy, nothing is stuck In progress, and going on ends equal to a run without a kill.

## 4. Whether the tests can fail

Each piece was broken on its own and `crates/mk-core/tests/m6b_meaning.rs` run against it.

| Broken on purpose | Failed by |
|---|---|
| Attempt token not checked | `nfr_rel_05_pause_while_a_call_hangs` |
| Rescoring ignored | `inv_sr_02_rerun_switches_whole`, `us_06_2_ac1_top_20_with_score_and_locator` |
| Query sent under another gate purpose | `us_06_2_ac3_endpoint_rules` |
| Status trusts any good index file | `inv_sr_02_removed_document_leaves_the_results` |
| No per-item identity check | `run_identity_change_during_the_lane_blocks_instead_of_mixing` |
| Old vectors stay at the switch | `inv_sr_02_rerun_switches_whole` |
| Query goes to the newly chosen model | `inv_sr_02_cancel_of_a_rerun_changes_nothing`, `inv_sr_02_model_changed_is_not_searched_with_the_other_model` |
| Card-like masking removed | `nfr_sec_14_card_numbers_are_masked_in_results` |
| Index reads any run; rescoring reads any run | `inv_sr_02_index_and_rescoring_read_the_active_run_only` |
| Index file of another generation accepted | `adr_0010_a_file_from_before_a_change_is_not_loaded` |
| Index checksum not checked | `adr_0010_a_damaged_index_is_rebuilt` |
| Gate accepts too few vectors | `adr_0006_too_few_vectors_fail_the_item` |

The first pass missed four of these and skipped one: real gaps. Tests were added and the last four rows are from the pass after that. A duplicate count check in `embed_item` could not be reached, because the gate refuses that reply first, so it was removed.

## 5. Real runs on RM-1 (2026-11-05)

### TD-1 through the release CLI

`testdata/td1`, 102 files, machine not formally idle.

| | bge-m3 | Re-run with nomic-embed-text v1.5 |
|---|---|---|
| Parts | 711, 0 failed | 315 after a kill part-way |
| Vectors | 19,995 | 8,415 in the part after the kill |
| Time | 33.8 s | 10.2 s after the kill |
| Index | 2.7 MB | 2.08 MB |
| `project.db` | 84 MB | 124 MB |

- A query during the re-run says "Re-running embeddings with nomic-embed-text v1.5: 55% done. Meaning search comes back when it is done." and exits 1.
- After `SIGKILL` during the re-run: no server left after 2 s, GPU memory back at 2 MiB. `embed run` went on and the project ended ready with nomic.
- `rebuild` is consistent and `same_as_incremental` is true. No server left at the end.
- A CLI query takes about 2.1 s including the model start.

### Scripted walk of the release app

48 files from TD-1 (`public` and the PDF, DOCX and JSON folders), plus an email folder added later, in a scratch data folder. A temporary script drove the screens and is not in the repository. **How the screens look was not checked, and no person used them.** The last run passed every step with no errors.

| Step | Result |
|---|---|
| Empty project, Search tab | "Embeddings have not run yet", Start embeddings |
| Add files | Embeddings start by themselves: "Embeddings: 0% done"; row "Meaning search · Embedding · 0 of 136 parts · Estimating…", "Data sent to this machine". Keyword search returns 8 rows meanwhile |
| Pause | Shown after 6 ms, still at 0 parts after 2 s |
| Resume to Ready | 13.5–13.9 s |
| Query | 20 results in 111–132 ms, with score and locator (`scotus-22-451_7m58.pdf pdf 0.63 page 11`) |
| Click a result | Source open in 48–71 ms at the page image; Show text marks the window |
| Choose nomic | "Vectors come from BGE-M3. Re-run with nomic-embed-text v1.5, or choose BGE-M3 again." |
| Re-run cancelled at once | Keep the vectors, then choose BGE again: a query answered after 1,978 ms, same top results |
| Re-run cancelled part-way | 11 of 136 parts; keyword search works during it; afterwards model-changed state with BGE-M3; same results after choosing BGE again |
| Re-run with nomic to the end | 6.4 s, 20 results |
| A folder added later | Embeds by itself, 136 → 151 parts |
| After quit | No server, GPU 2 MiB, run 1 superseded, run 2 active at 768 dimensions, only run 2's 3,313 vectors, one index file, `rebuild` ok, egress rows `embeddings` and `search_query` |

**Found by this walk, both fixed with tests that failed first:**
1. After a model change the screen stayed as it was and offered no re-run: nothing refreshed the status when there was no job to block. `AppState::refresh_ai_blocks` now always tells the UI. Test: `us_06_2_ac2_a_new_choice_tells_the_open_project`.
2. A livelock in `ensure_ready`, from M5c and exposed by meaning search. With the lane wanting nomic and a query wanting BGE, each stopped the other's server before the one that started it took its lease, so a new `llama-server` came up about every 2 s and the screen stayed on "Searching…". The lease is now taken under the same lock as the start. Test: `model_switch_two_callers_do_not_stop_each_other_for_good`.

### App memory on TD-2 (NFR-RES-01)

**Method.** Release app on an idle RM-1, TD-2 with all its vectors, sampled with `cargo xtask mem` (RSS of the app and its children, in MiB). "Reopened" is the M3d method (`MK_UI_TIMING`, no source open). The in-use rows come from a script: open, idle 90 s, five queries, then wait past the 10-minute release.

**Result: 510 MiB reopened and 513 MiB after release, both over the 500 of NFR-RES-01 and accepted for now (roadmap §4 #19).**

| State | Total | App | WebKit web | WebKit network | `llama-server` |
|---|---|---|---|---|---|
| TD-2 at M3d, reopened (earlier baseline) | 471 | 197 | 226 | 48 | none |
| Empty project, reopened | 478 | 204 | 226 | 48 | none |
| **TD-2, reopened** | **510** | 213 | 249 | 48 | none |
| TD-2, just after the queries | 1,616 | 392 | 253 | 48 | 920 |
| The same without the server | 695 | | | | |
| **TD-2, after the 10-minute release** | **513** | 219 | 244 | 48 | none |

- **Against M3d:** TD-2 reopened is 39 MiB higher, 16 in the app process and 23 in the WebKit web process. Why was not traced. An empty project today, at 478, is already 7 over M3d's TD-2 figure, so the 500 leaves little room for project data. M3d has no empty-project figure to compare.
- **In use:** for the 10 minutes after a search, the app without the server sits at about 695 MiB and the server adds 920. Release gives back about 173 MiB in the app process, and the server is gone. The maintainer decided on 2026-11-05 that this state is outside NFR-RES-01, which covers "model not loaded". No target limits memory here.
- **Queries in this run:** the first took 3,987 ms including the model start, then 100–124 ms through the screen. 0 servers at the end. GPU memory went back to 2 MiB in the earlier run of this script; it was not re-read in this one.

**Found by this measurement and fixed.** The first run gave 580 MiB reopened, with a single 64 MiB anonymous mapping in the app process that an empty project did not have. That size matches the writer connection's page cache (`crates/mk-store/src/schema.rs`). On opening a project, planning counted 0 windows on a reader and then repeated the full scan on the writer, which filled the cache. A first fix, `Store::release_memory` after planning, did not work: 577 MiB reopened. `PRAGMA shrink_memory` did not give that memory back to the system, and why was not verified. The fix that stands skips the writer pass when the reader counts nothing to plan, in both `schedule_embeddings` and `schedule_stage_b`. The mapping is gone and the total is 510. There is no automated test for the memory itself.

## 6. Open points

1. **Disk during and after a re-run.** Old and new vectors sit side by side until the switch, and the freed pages stay in the file: TD-1 went from 84 to 124 MB, and on TD-2 82% of the pages were free after the delete. A TD-2 re-run needs about twice the vector space. The app does not `VACUUM`.
2. **Short rows.** Short JSON values top some results: "paid" three times at 0.636 for an offshore query with bge-m3, from `ground-truth.json` in that run. Keeping short rows is an ASSUMPTION; M1 dropped those under 40 characters as a corpus choice.
3. **Duplicates.** TD-2's near-copies can fill the top 20 with the same text. US-06.2 does not ask to collapse them, so they are not.
4. **Self-hits.** 35 of 50 queries from TD-2's own text found that text in the exact top 20 (§2). Not explained.
5. **Start embeddings with no files.** The click does nothing visible.
6. **"No model" state.** It is a text pointer to AI models, not a link. The default fit already picks bge-m3, so the walk did not reach this state in the app; the tests cover it.
7. **Experimental label.** US-10.3's label on meaning search is not added: M6c. **Closed in M6c:** the label is dropped (`16-entities-acceptance.md` §8).
8. **CLI locators.** `mangkon-cli meaning` prints them in debug form (`PdfText { page: PageNumber(11), ...}`).
9. **ADR-0010.** Accepted 2026-11-05. Its "top 200" and "memory-mapped" differ from what was built; the ADR's M6b follow-up records both.
10. **NFR-RES-01.** 510 MiB reopened and 513 after release on TD-2, 10–13 over; about 695 MiB plus the server's 920 in the 10 minutes after a search. Decided 2026-11-05: accepted for now, and looked at again in the same hardening milestone as M3d's 512 after an ingest (roadmap §4 #18, #19).
11. **Writer page cache with work to plan.** When there are windows to plan, for example a file added to TD-2, the writer pass still fills its page cache, and that 64 MiB may stay until the project closes. Not measured.
