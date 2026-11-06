# 14 — AI extraction

> Input: `06-roadmap.md` §2 M6a, ADR-0005, ADR-0006, `02-requirements.md` E10, `03-nfr.md` (NFR-PERF-25, NFR-QA-11, NFR-REL-02/03/04/05/12), `04-domain.md` BC-06 and §7.2, `09-model-check.md`. **If this file disagrees with those, they win.**
> Status: M6a on RM-1. Automated checks against the stand-in `llama-server` and the real-model runs are recorded here. Linux only; macOS and Windows are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Decisions (2026-11-05)

| Decision | Choice |
|---|---|
| A4 | Old results stay until the new run is done, then one switch. Cancel changes nothing visible. |
| Work unit | Adjacent segments of one document, up to 2,000 characters; a longer segment is split at whitespace. Mentions are stored against an exact segment and span. |
| Start rule | Automatic after Stage A when at most 1,600 units wait (about 20 min at 1.39/s, ASSUMPTION; M6c measured 0.82/s, about 32 min). Above that, the user picks documents or chooses all, with an estimate. |
| Model changed | New work is Blocked with a reason and "Re-run with Y". Choosing the old model again unblocks it. Never mixed, nothing large starts by itself. |

Evidence rules, in order (`crates/mk-ai/src/check.rs`): strict parse with one retry; evidence found after whitespace normalization; name inside the evidence, ignoring case; name inside one segment; name's span not overlapping a Stage A identifier (counted on its own); duplicates removed.

## 2. Kill tests

`apps/cli/tests/ai_kill.rs`: `SIGKILL` of `mangkon-cli ai run` and `ai rerun`, 10 each, against the stand-in server with 4 slots. Half of the kills fall at seeded moments anywhere in a run, the model's start and the shutdown included. The other half fall once a seeded number of requests has reached the model, so they land among the units at any machine speed. After every kill the project is healthy, at most 4 items were lost, no item stays In progress, and going on ends equal to a run without a kill. After a killed re-run, the connections are the old model's or the new one's, whole. No stand-in is left running at the end. All 20 pass, and 36 runs of the test in three parallel loops (720 kills) passed with no server or watcher left.

**Found by this test, twice:**

1. After a `SIGKILL` of the app, `llama-server` kept running. A real model would have kept the GPU's memory, and the next start could have run out of it. M5c stopped it only on a normal quit. A small `sh` process started beside the server then held a pipe from the app and stopped the server when the app was gone.
2. That first fix left two gaps, and about 1 kill in 170 left a server running for good, its parent gone and no watcher. The watcher started just after the server, and it was ended just before the server on a stop, so a kill between the two left nothing to stop it. Now the small shell starts the watcher first and then becomes `llama-server` under the same process id, and the watcher ends only after the server has. It sends SIGTERM, then a kill if the server is still there after 5 s (`crates/mk-models/src/runtime/process.rs`). The gaps are too short to hit on demand, so a stand-in checks for its watcher as its first instruction and again on SIGTERM; `nfr_rel_02_the_watcher_is_there_before_the_server_starts_and_until_it_ends` failed before the fix and fails again with the stop order broken.

Workers leave the same way (ADR-0007). Not on Windows. The start script ran under `dash` and `bash --posix` on Linux; macOS is not verified.

## 3. Whether the tests can fail

The Stage B tests passed the first time they ran, so each rule below was broken on purpose, one at a time, and the tests run again.

| Broken on purpose | Tests that failed |
|---|---|
| Attempt token ignored | 3 |
| Pending commits made visible | 3 |
| Switch keeps the old mentions | 4 |
| Switch skips the rebuild | 4 |
| Pause leaves items In progress | 1 |
| Table step deletes AI mentions | 1 |
| Low-only connections shown | 1 |
| One slot locally | 1 |
| Run identity not checked per item | **0 at first**, a real gap: an edit between runs was covered, one during a run was not. With `run_identity_edit_during_a_call_sends_nothing_to_the_new_model`: 1 |
| `verify_mention` always true (the health check) | **Only its own unit test at first.** With `nfr_qa_11_health_finds_invented_evidence_and_mixed_runs`: 2 |
| Evidence accepted anywhere | 5 |
| Name need not be inside the evidence | 2 |
| Name overlapping an identifier kept | 2 |
| Duplicates kept | 1 |
| Name may cross segments | 9 |
| Three network failures never Block | **0**, a real gap. `adr_0006_three_network_failures_block_and_lose_nothing` added; this mutation was not run again, since with it the new test would wait through thousands of backoffs |
| Cancel keeps the pending rows | 0, and rightly: `ON DELETE CASCADE` removes them too, and `adr_0005_cancel_changes_nothing` counts 0. The explicit delete did nothing, so it was removed |

## 4. Real runs on RM-1 (2026-11-05)

`llama-server` build 10863, `mangkon-cli ai run` and `ai rerun` from the release build, on a scratch copy of the app's data. Nothing else was asked of the machine, but these are not the idle-machine timings: those, and NFR-QA-08/09, NFR-PERF-12/13 and the Experimental label decision, are M6c.

### TD-1 with the 4B (Qwen3.5 4B, 4 slots)

| | |
|---|---|
| Units | 970 of 970, 0 failed |
| Mentions kept | 3,050 (organizations 2,180, people 870), 1,552 distinct keys |
| Discarded, no matching text | 1,028 |
| Replies that could not be read | 18 |
| Names dropped on an identifier | 44 |
| Connections | 193; none exists only because of low confidence |
| Wall time | 24 min 22 s, 0.66 units/s |
| Memory | peak 3.3 GB RAM, about 4.3 GB GPU; back to 2 MiB after exit, no process left |
| Listening on | 127.0.0.1 only |

**NFR-QA-11.** A separate script read all 3,050 stored mentions back against the stored text: 0 outside their segment, 0 whose name is not in the evidence span. The check holds on real output as it does on the fake.

**M6c measured the targets on an idle machine and changed the rate: `16-entities-acceptance.md` §3–§4.** What follows is M6a's view.

**The rate.** The 1.39 units/s behind the start rule came from M1's data; TD-1 gives 0.66, and it varied from 0.28 to 1.14 across the run. All 4 slots were busy for 97.7% of the run, so that is not the cause, and M1's 0.66 with one slot is a coincidence. Length does not explain it either (correlation of characters and duration 0.31). The median unit takes 2.2 s, p90 10.2 s, p99 75.6 s. 17 units of 70 s or more take 23% of the slots' time and 36 units of 40 s or more take 38%. These are units where the model writes until the limit of 2,048 tokens and is then retried; the one seen in full repeated `Northern` to the limit. The 18 unreadable replies are these units: they end Done with 0 mentions, so their names are lost.

What follows from this, all open (§5):
- 1,600 units is about 40 min at this rate, not the 20 of the start rule. 20 min would be about 790 units.
- The estimates shown to the user use 1.39/s and are about 2× too low.
- NFR-PERF-13 is ≤ 20 min including embeddings. The 4B alone took 24 min on TD-1.
- NFR-PERF-24 is ≥ 1 segment/s. This run, on units of up to 2,000 characters, is below it.

**Discards.** 25% of TD-1's mentions are discarded, against ≤ 1% in M1's run (`09-model-check.md`). M1 ran on clean synthetic text: a synthetic email folder here discards 0 of 66. 150 sampled units were sent again and the reasons counted (793 mentions, 22.7% discarded):

| | Share |
|---|---|
| Passes | 77.3% |
| Evidence not in the text; name is | 14.4% |
| Name not inside the evidence; name elsewhere in the text | 3.9% |
| Evidence and name both absent | 3.5% |
| Evidence differs only in case | 0.4% |
| Name not inside the evidence; name absent | 0.4% |
| Reply could not be read | 0.1% |

Of the 145 evidence misses, 43 would match after folding typographic quotes and dashes, 33 after joining words hyphenated at a line end, 3 with folding and case, and 66 would stay missing. Those come from multi-column PDF pages stored interleaved (the model quotes the column it reconstructs) and from OCR noise it corrects (`meqistered` written as `registered`). The check is doing what US-10.2 AC1 says; the real text is rougher than M1's. Of 179 discarded occurrences, 103 names were stored elsewhere anyway. The other 76 include noise (`§ 635.28(a)(1)`, `ISO 2017`) and real names (`Hotchkin`, `Pacific Fishery Management Council`), so loosening would raise recall and noise together.

**Found by this run:** `mangkon-cli rebuild` failed with `not a name key`. "Chevron U. S. A. Inc." gave the key `org:chevron u s a`, which is not stable: normalizing it again drops `s a` as a legal suffix. On the per-key path the name was silently left out of the entities, and a re-run's switch would have failed. Tables could reach this since M4c. `crates/mk-kernel/src/name.rs` now removes suffixes until none is left, and a proptest checks that every key parses back. The name then keys as `org:chevron u`, which reads badly but is stable; the name shown stays as written. No migration: a project with such a name in a table column keeps the old key until it is read again.

### A small folder and a re-run

`testdata/td1/synthetic/email`, 14 files, 15 units.

| | 4B | Re-run with Qwen3.6 35B-A3B |
|---|---|---|
| Mentions | 66 | 67, all from the 35B |
| Discarded | 0 | 1 |
| Names dropped on an identifier | 9 | 15 |
| Time | about 19 s | 97 s for the units, 1 min 43 s with loading |
| Peak RAM | | 37.2 GB |

Halfway through the re-run the database showed the 4B's 66 mentions visible and 22 pending, then one switch: the 4B's run `superseded`, 0 pending, and `rebuild` reported `same_as_incremental: true`. Decisions kept across a re-run are covered by the tests only (`us_10_4_*`).

### 35B with 4 slots, and a kill

`testdata/td1/synthetic/notes`, 7 units. Ready in 8 s with `-c 32768 -np 4 --cache-ram 0 --reasoning off`, listening on 127.0.0.1 only, 36.4 GB RAM and 6.5 GB GPU. This is a small sample, not a measure of four slots on bigger models.

`SIGKILL` of the CLI with 4 items in progress: the server was gone in 1.05 s and the GPU back to 2 MiB. On reopening, 7 of 7 finished with 35 mentions and `rebuild` was consistent. A new file with the 4B chosen was Blocked, exit 1: "These results came from Qwen3.6 35B-A3B. New work waits so two models are never mixed: re-run with Qwen3.5 4B, or choose Qwen3.6 35B-A3B again." Choosing the 35B again finished 8 of 8 with 37 mentions, consistent, with no server left.

Repeated with the 4B after the watcher change in §2, on the release build: the server on `127.0.0.1` with its watcher beside it, `SIGKILL` with 4 items in progress, the server gone in 1.16 s, no watcher left and the GPU back to 2 MiB. On reopening, 8 of 8 finished, `rebuild` reported `same_as_incremental: true`, and a normal end left nothing running.

### Desktop walk

The release app with both real models, on a scratch data folder, in a window of 1138 × 533. A temporary script clicked the real controls through the steps below and read the screen after each; it is not in the repository. This is a scripted walk: how the screens look was not checked, and no person used them.

| Step | Seen |
|---|---|
| Create, choose the 4B, add 14 files | Stage B starts by itself after Stage A, 0 of 15 parts |
| During the calls | Status bar: "Data sent to this machine", "People and organizations: Working · 127.0.0.1:35071" |
| Pause, then Resume, in the AI row | "Paused" shown 16 ms after the click, then finding again |
| First run ends | 66 mentions from Qwen3.5 4B, in 22 s |
| Evidence of a person in two sources | "header From · 99% · Qwen3.5 4B · Experimental", and the source highlights `Irene Vasquez-Lindqvist <irene@brightwater-logistics.example>` |
| Find in Documents with nothing new | "Nothing new to read in all files" |
| Choose the 35B | Re-run offered; "Keep the results" leaves everything as it was |
| Re-run, Cancel part-way | 66 from the 4B before, during and after; the evidence stays the 4B's |
| Re-run to the end, evidence left open | One switch, 66 mentions from the 35B. The open evidence turned to the 35B by itself, 470 ms after the switch was seen |
| A new file with the 4B chosen | Blocked in the AI row with the reason, "Re-run with Qwen3.5 4B" offered |
| Choose the 35B again | Finds it by itself, 16 of 16 parts, 68 mentions |
| Quit | `rebuild` consistent, no `llama-server` left, GPU back to 2 MiB |

**Found by this walk**, each fixed with a test that failed first, except the last two as noted:

1. **The app hung for good on making or opening a project.** It held the `open` lock while checking `llama-server` for the first time, and the check tells listeners on the same thread, which take that lock. It needs a model installed and chosen, and the check not done yet, so in the first moments after launch; the status bar's own check usually gets there first. The call under the lock dates from M5c. The work that starts a project now runs outside the lock (`us_01_1_*`, `us_01_2_*` in `apps/desktop/src/state.rs`).
2. **The AI row counted the re-run's mentions under the old model's name**: "46 mentions from Qwen3.5 4B" with 66 visible, back to 66 on Cancel, against decision A4. The four counts now belong to the results shown, and a re-run's come in at its switch (`adr_0005_rerun_shows_old_results_until_the_switch`, `adr_0005_cancel_changes_nothing`).
3. **A cancelled re-run's finished units were counted in the next run**: "17 of 17" on 16 units. An ended job with no run joined whichever run came next. Now only open jobs do (`adr_0005_a_cancelled_rerun_leaves_nothing_for_the_next`). Units cancelled before any model was bound still stay ended (`gap_a3_units_cancelled_before_a_run_stay_ended`).
4. **Find with a connection open showed an empty list until it was closed**, since M4d. A hold on the list started at a count still loading, so it held 0 (`ui/src/lib/keyset.ts`, unit test on the rule; the walk checks the effect, as the UI tests have no DOM).
5. **In a short window the connections list had no height at all**, and nothing to scroll to: at 533 px high the heading, filters and sort row take more than the column has, and the taller AI row made it worse. The list now keeps a minimum height and the column scrolls. NFR-USE-07 asks for 1280 × 800, where this does not arise. The walk checks that rows come back; how it looks was not checked.

**Not the same twice.** The 4B on the same 15 units, seed 1, temperature 0, gave 66, 67, 70, 71 or 72 mentions over 9 runs, and confidence moved by a point. Four slots share a batch, so the order of arrival changes the numbers. M6c's benchmark should allow for this.

## 5. Open points

1. **Name inside the evidence goes beyond the letter of US-10.2.** It is stricter. M1's data put its cost at about 1%; on TD-1's real units it discards 4.3% (34 of 792), many of them wrong pairings such as `Northern` with an unrelated quote.
2. **OCR mention confidence.** Domain §3.3 says OCR mentions take the OCR word confidence; US-10.1 AC1 stores the model's. Requirements win. ASSUMPTION; M6c sees the effect on scans. M6c: no scan mention is low-confidence, so nothing changes (`16-entities-acceptance.md` §3).
3. **Bare domains stay a false-positive source.** `velmora-port.example` in body text is not an identifier mention, so the overlap rule misses it. The filter from `09-model-check.md` §5.3 is the known fix if M6c's precision falls short. **Closed in M6c:** precision 93.29%, no bare domain among the false positives, filter not added (`16-entities-acceptance.md` §3).
4. **Endpoints without structured output.** The schema is still sent and replies are still strictly checked. Failures show as "N responses could not be read" rather than blocking.
5. **Long-segment windows have no overlap.** A name split at a window edge is lost; cuts prefer line breaks, then whitespace. ASSUMPTION.
6. **The type filter still says "Name (from table)"** though AI people now match it too. It stays until M6c, with the rest of that screen's work. **Closed in M6c** (`16-entities-acceptance.md` §1).
7. **The start rule and the shown estimates rest on 1.39 units/s; TD-1 gives 0.66** (§4). `STAGE_B_AUTO_UNITS` and `STAGE_B_UNITS_PER_SECOND` in `crates/mk-jobs/src/lib.rs` are unchanged. **Closed in M6c:** the rate is the measured 0.82, the limit stays 1,600, about 32 min (`16-entities-acceptance.md` §1).
8. **Evidence matching on rough text** (§4). Folding typographic quotes and dashes and joining line-end hyphens would save about 54% of the evidence misses; the span stored would still be exact in the source, so nothing could be invented. Not done: it changes how "never invent" is checked, and its effect should be judged against M6c's scoring. Column order is a parser matter.
9. **NFR-PERF-13 is at risk** (§4). 24 min for the 4B on TD-1, before embeddings, against ≤ 20 min for both. **Closed in M6c:** 1,184 s, met (`16-entities-acceptance.md` §3).
10. **Units stuck at the token limit** cost about a quarter of the slots' time and lose their names (§4). No limit other than the 2,048 tokens M1 used is applied. **Closed in M6c:** the limit stays, and a reply that reaches it is not retried (`16-entities-acceptance.md` §4).
11. **The 4B calls legal citations organizations** (`1 Stat. 234`). A precision matter for M6c. **Moved:** still happens, on public files that are not scored (`16-entities-acceptance.md` §9).
12. **A model change shows in the AI row, not in the status bar.** The bar follows whether the model can run, and it can. The Blocked reason and "Re-run with" are in the row.
