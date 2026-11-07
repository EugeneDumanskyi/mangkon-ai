# 17 — Entity resolution

> Input: `06-roadmap.md` §2 M7a and spike S7, `02-requirements.md` US-11.1, US-11.2, US-11.4, US-11.5, US-12.2, `03-nfr.md` (NFR-QA-01/02, NFR-REL-09, NFR-MNT-07, NFR-PERF-34), `04-domain.md` D6, §9.1, §9.5, INV-ER-*, INV-CU-*, ADR-0004, ADR-0005. **If this file disagrees with those, they win.**
> Status: M7a done on RM-1 (2026-11-07). M7b built, walked by script and timed on TD-2 on RM-1 (2026-11-07), hand walk moved to M9 or M10 (§11). Linux only; macOS, Windows and RM-2 are not verified here.
> Anything marked **ASSUMPTION:** is a guess. Please confirm or correct it.

---

## 1. Spike S7: merge proposals

**Question (K9).** Can scoring cover TD-1's real name variants without flooding the queue?

**Pass marks, fixed before measuring (ASSUMPTION: the roadmap sets none).** On the real 4B's output, at the chosen threshold:

1. At least 80% of the gold variants the model found can reach their entity's main key through proposals.
2. At least 60% precision on judged pairs (both sides in scored documents).
3. At most 100 proposals in total on TD-1.
4. Every proposal has a plain reason.

The lookalike pairs (Gregor/Gregory, Brightwater GmbH) may be proposed but count as wrong. A name not in the gold counts as wrong too.

**Scoring** (`crates/mk-graph/src/proposal.rs`, no new dependency): Jaro-Winkler written here, titles stripped, comma order, initials, token subset, organization acronym. Shared hard identifiers come from D7 links only: email, phone, IBAN, crypto and custom, not domains or URLs, and an identifier shared by too many entities is skipped. A pair whose two sides together sit in one document is skipped, since merging it links nothing.

Two limits keep the work small on a large project, with the same results on TD-2 (§7): a word of only digits makes no name block, and only identifiers held by 2 to 4 entities reach the scorer.

**What may propose, after the first real run below.** The threshold stays 0.6.

| Match | Proposes |
|---|---|
| Shared hard identifier | On its own |
| Same name without a title or joining word, reordered name, initial, part of a name of two or more words | On its own |
| One-word name that is part of a longer name | Only if it fits one longer entity. Rejecting a wrong fit does not unlock it; accepting a merge can. |
| Organization name that is part of a longer one | Only if exactly one word apart; otherwise as support |
| Similar spelling (Jaro-Winkler), acronym | Only as support: raises a pair that already shares an identifier |

**Tool.** No model is needed for the sweep:

```sh
mangkon-cli proposal-bench --project <kept> --testdata testdata/td1 [--json FILE]
```

**Gold names (repeatable, `crates/mk-bench/tests/s7_gold_names.rs`).** The stand-in's names, run in CI:

| Threshold | Proposals | Precision | Variants reached |
|---|---|---|---|
| **0.60** | 34 | 100% (34 of 34) | 83.3% (25 of 30) |
| 0.70 | 33 | | 80.0% |
| 0.75 | 25 | | 63.3% (fails mark 1) |
| 0.80 | 18 | | 53.3% |
| 0.85 | 14 | | 40.0% |
| 0.90 | 3 | | 6.7% |

The stand-in's names are cleaner than a model's, so this table only guards against regressions. The rules are judged on the real output below.

**Real 4B output, first run** (2026-11-07, one run, no embeddings, 2,880 mentions). The scorer as first built passed at no threshold. Those that kept mark 1 gave far more than 100 proposals, and only 0.70 reached 60% precision:

| Threshold | Proposals | Judged | Correct | Precision | Variants reached |
|---|---|---|---|---|---|
| 0.60 | 651 | 74 | 42 | 56.8% | 100% (28 of 28) |
| 0.70 | 467 | 65 | 40 | 61.5% | 100% |
| 0.75 | 381 | 52 | 30 | 57.7% | 89.3% |
| 0.80 | 349 | 41 | 22 | 53.7% | 78.6% |
| 0.85 | 234 | 30 | 14 | 46.7% | 50.0% |
| 0.90 | 38 | 13 | 3 | 23.1% | 7.1% |

Most of the noise came from organizations: similar spelling gave 248 proposals with 0 of 10 judged correct, and part of a name gave 270 with 7 of 23. The roadmap's fallback (exact alias and shared identifier only) still gave 361 proposals, so two further limits were added: an organization part must be one word apart, and a one-word name must fit one longer entity.

The same project with the rules above:

| Threshold | Proposals | Judged | Correct | Precision | Variants reached |
|---|---|---|---|---|---|
| **0.60** | 82 | 37 | 32 | 86.5% | 82.1% (23 of 28) |
| 0.70 | 77 | 37 | 32 | 86.5% | 82.1% |
| 0.75 | 60 | 30 | 25 | 83.3% | 67.9% (fails mark 1) |
| 0.80 | 44 | 21 | 18 | 85.7% | 57.1% |
| 0.85 | 25 | 15 | 14 | 93.3% | 42.9% |
| 0.90 | 9 | 4 | 3 | 75.0% | 7.1% |

All have plain reasons. Working through the queue with gold answers accepted 26 and left 50, in 5 rounds; the rebuild matched and the project was healthy.

**Limits of this result.**

- The two extra limits were fitted on this one project. Fresh runs test model noise, not a new corpus.
- Mark 1 passes by one variant: 22 of 28 would fail. The count is static and conservative, since `mr reinholt`, `mr almqvist`, `mr hale` and `irene` become proposable once a merge is accepted.
- Correct merges are lost: OCR slips such as `yelmora`/`velmora` are no longer proposed without a shared identifier, nor is `VPA` for Velmora Port Authority.
- Wrong pairs left at 0.60: `brightwater logistics` with `…accounts` (they share an email), `kestrel advisory` with `…lp`, `board of the velmora port authority` with `velmora port authority`, and `port authority` with `velmora` and `yelmora`.

**Three further runs** (2026-11-07, same rules, threshold 0.60, new model output on the same corpus):

| Run | Proposals | Judged | Correct | Precision | Variants reached |
|---|---|---|---|---|---|
| 1 | 95 | 38 | 32 | 84.2% | 82.1% (23 of 28) |
| 2 | 89 | 38 | 32 | 84.2% | 82.1% |
| 3 | 99 | 39 | 32 | 82.1% | 82.1% |

All four marks are met on every run, but mark 3 only just: run 3 is one proposal under the limit. Only 62 of the 136 different pairs seen across all four runs appear in every run, and most of the rest are unjudged pairs from the unannotated public files. Five wrong pairs recur in every run: the same five listed above. At 0.70 the three runs give 89, 82 and 93 proposals with the same number of correct pairs and variants reached.

**Decision (2026-11-07): S7 passes. The threshold stays 0.60, with the rules above.** INV-ER-05 and US-11.2 AC3 keep 0.6, and US-11.2 AC1 notes that similar spelling alone no longer proposes. 0.70 would give a little more room under the limit but gains no correct pair; it stays the first thing to try if the queue floods on a real corpus.

## 2. Link scores (NFR-QA-01, NFR-QA-02)

**How they are scored (ASSUMPTION: neither NFR has a written formula).** `crates/mk-bench/src/links.rs` compares connection rows with the 77 gold links:

- Identifier rows match by key.
- Entity rows map through canonical and alias name keys, and must cover at least two of that gold link's documents.
- Rows only in unannotated public files are counted but not judged, as in M6c.

**First run** (same project as §1, default state): recall 80.5% (62 of 77; identifiers 43 of 56, entities 19 of 21), precision 96.4% (80 of 83; identifiers 43 of 43, entities 37 of 40), 86 not judged. After gold-driven decisions, for information: recall 81.8%, precision 95.5%. The recall margin is thin.

**Median of 3** (`llm-bench --runs 3`, real 4B on TD-1, default state with no merges accepted): **recall 80.52%, precision 97.59%. Both targets are met.**

| Run | Recall | Identifiers | Entities | Precision | Not judged | After gold-driven decisions, for information |
|---|---|---|---|---|---|---|
| 1 | 80.5% (62 of 77) | 43 of 56 | 19 of 21 | 98.8% (80 of 81) | 92 | 81.8%, 98.5% |
| 2 | 81.8% (63 of 77) | 43 of 56 | 20 of 21 | 97.6% (81 of 83) | 103 | 81.8%, 97.0% |
| 3 | 80.5% (62 of 77) | 43 of 56 | 19 of 21 | 96.4% (80 of 83) | 94 | 81.8%, 95.5% |

Recall passes by one link: 61 of 77 would fail. The identifier count is 43 of 56 on every run; only the entity count moves. Rebuilds matched and all projects were healthy on every run.

## 3. Decisions survive (criterion 2, NFR-REL-09, NFR-MNT-07)

`crates/mk-core/tests/m7a_decisions_td2.rs` (`#[ignore]`) opens a project built by the code before M7a (commit `39a7780`: schema v10, derived rows v5), with that code's own decisions in it. It adds every kind of decision in bulk, then:

1. Re-scans with files changed, gone and restored.
2. Changes the model through the store with a stand-in's answers. A real model on TD-2 would take about 31 hours; the real model-change path is on TD-1 (§2).
3. Migrates to a made-up v11 through `Store::open_with`. M7a adds no numbered migration, so the real path is the old project's derived rebuild, plus this injected step.

After each step the log is byte for byte what it was, and every decision still has its effect.

**How the project is made.** With `39a7780` checked out, build its `mangkon-cli`, add the code below as `crates/mk-core/tests/zz_td2_v10.rs` and run it in release. The real worker is needed for OCR, and the language folder is created on first use. Without it, TD-2's 2,178 scanned files fail with "OCR is not available for this file".

```sh
cargo build -p mangkon-cli --release
MK_WORKER_PROGRAM=target/release/mangkon-cli MK_V10_OUT=<dir> MK_V10_SOURCE=<TD-2> \
  cargo test -p mk-core --release --test zz_td2_v10 -- --ignored --nocapture
```

```rust
use std::path::PathBuf;
use mk_core::{Assignments, DocumentFilter, Project};

#[test]
#[ignore = "by hand"]
fn build() {
    let out = PathBuf::from(std::env::var_os("MK_V10_OUT").unwrap());
    let source = PathBuf::from(std::env::var_os("MK_V10_SOURCE").unwrap());
    let (mut project, _) = Project::open(&out).unwrap();
    let worker = PathBuf::from(std::env::var_os("MK_WORKER_PROGRAM").unwrap());
    *project.worker_config_mut() = mk_core::WorkerConfig::with_program(worker);
    project.set_tessdata_dir(Some(out.with_extension("tessdata")));
    project.add_source_root(&source).unwrap();
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();
    eprintln!("stage A: {} done, {} failed", run.done, run.failed);
    let mut tables = Vec::new();
    let mut after = None;
    loop {
        let page = project.documents_page(&DocumentFilter::default(), after.as_ref(), 500).unwrap();
        tables.extend(page.rows.iter().filter(|r| r.mapping_state.is_some()).map(|r| r.ref_id));
        match page.next { Some(n) => after = Some(n), None => break }
    }
    for id in &tables {
        let Some(view) = project.mapping_view(*id).unwrap() else { continue };
        let a = view.columns.iter().map(|c| (c.key.clone(), c.suggested)).collect();
        project.confirm_mapping(*id, Assignments(a)).unwrap();
    }
    let state = project.store().derived_state().unwrap();
    let identifiers: Vec<String> = state.identifiers.keys().take(2000).cloned().collect();
    for (i, k) in identifiers.iter().enumerate() {
        project.set_ignored(k, true).unwrap();
        if i % 10 == 0 {
            project.set_ignored(k, false).unwrap();
            project.set_ignored(k, true).unwrap();
        }
    }
    let entities: Vec<String> = state.entities.keys().step_by(7).take(2000).cloned().collect();
    for k in &entities {
        project.set_ignored(k, true).unwrap();
    }
    for i in 0..200 {
        project.set_ignored(&format!("email:orphan{i}@old.example"), true).unwrap();
    }
}
```

**Result on TD-2 (2026-11-07, RM-1, idle machine, release build): met.** The project: 10,000 documents, all done, none failed; 1,113,276 segments, 750,613 connections, 694 confirmed tables, 255,669 identifiers, 495,013 entities, 5,294 decisions, 2.0 GB.

| Step | Effects that hold |
|---|---|
| Opened by today's code (derived rows v5 to v6, 29.9 s) | 4,000 of 4,000 |
| Every kind of decision added | 10,862 of 10,862 |
| Re-scan with nothing changed | 10,862 of 10,862 |
| Files changed and gone | 10,862 of 10,862 |
| Files restored | 10,862 of 10,862 |
| Model change through the stand-in | 10,864 of 10,864 |
| Migration to the made-up v11 | 10,864 of 10,864 |

The log holds 13,379 decisions at the end, and it stayed byte for byte the same across each step. Status: 10,721 active, 1,761 superseded and 200 orphaned, the same count as the 200 identifiers the old project ignored without having them. The other 697 are confirmed mappings (694 in the old project, 3 from the test), which have no status. 647 proposals were used, and 246,443 entities stayed free for edits. `rebuild_all` takes 32.4 s.

| Decision, on the writer | n | p50 | p95 | Max |
|---|---|---|---|---|
| Merge | 432 | 1.36 ms | 2.76 ms | 21.2 ms |
| Reject | 215 | 1.19 ms | 2.32 ms | 4.91 ms |
| Undo merge | 217 | 1.51 ms | 4.45 ms | 5.88 ms |
| Split | 2,000 | 93.2 ms | 158.6 ms | 169.6 ms |
| Undo split | 1,000 | 144.4 ms | 162.7 ms | 172.8 ms |
| Remove alias | 215 | 0.29 ms | 0.47 ms | 3.58 ms |
| Rename | 1,000 | 0.21 ms | 0.24 ms | 3.90 ms |
| Change type | 1,000 | 0.23 ms | 0.26 ms | 3.75 ms |
| Note | 1,000 | 0.20 ms | 0.23 ms | 3.67 ms |
| Ignore | 1,000 | 0.20 ms | 0.24 ms | 5.16 ms |

Limits of this result:

- Model change uses a stand-in, because a real model on TD-2 would take about 31 hours. The real path is on TD-1.
- M7a adds no numbered migration, so the v11 step is injected. The real path is the old project's derived rebuild.
- A split holds the writer about 100 to 170 ms at this size, against about 30 ms on a small project (§9).
- Split times vary between runs. An earlier run, before the proposal memory fix (§7), gave a p95 of 113 ms for a split and 116 ms for its undo, and took 261 s for the decisions rather than 334 s. The split path did not change, and the cause was not looked into. ASSUMPTION: the proposal thread now runs about every 2 minutes rather than every 10, since each run is shorter, and that is part of it.

`us_11_5_ac1_every_kind_survives_rescan_model_and_version` covers each kind on the stand-in at small size.

## 4. Rebuild equals incremental, and S2c (criterion 5, NFR-PERF-34)

`crates/mk-core/tests/derived_property.rs`, `m7a_ac5_decisions_incremental_equals_rebuild`: merge, reject, undo, split, undo-split, rename, type, note and alias steps, compared with the reference model after every step and with `rebuild_all` at the end. 48 sequences in CI; 600 passed once in a release build. `adr_0005_decisions_cross_the_rerun_switch` carries decisions across a re-run switch.

**S2c re-run** (2026-11-07, RM-1, idle machine, release build). `crates/mk-core/tests/s2c_derived_rebuild.rs` now has entity mentions, a hot merged group and 10,000 mixed real decisions, timed by kind. Pass: p95 + 500 ms debounce ≤ 2 s, for both the lane and a serial run of 50 files with names. **Met.** 10,000 documents, 3.89 M mentions, 257,828 identifiers, 2,162 entities, 16,645 entity mentions, 1.3 GB database.

| Measure | Before decisions | After 10,000 decisions |
|---|---|---|
| Lane, 100 files on 23 workers: p95 + debounce | 1,219 ms | **1,479 ms** |
| Lane, slowest of the 100 files | 1,077 ms | 1,533 ms |
| Serial, 50 files with names: p95 + debounce | 504 ms | **509 ms** |

The 10,000 are counted from the log, not from attempts: 3,277 ignore identifier, 308 unignore identifier, 1,032 ignore entity, 1,548 rename, 516 change type, 1,032 note, 180 remove alias, 355 accept, 1,384 reject (undo is one of these), 184 split, 184 undo split. Status: 6,487 active, 1,943 superseded, 1,570 orphaned.

| Decision, on the writer | n tried | p50 | p95 | Max |
|---|---|---|---|---|
| Merge | 355 | 0.95 ms | 2.55 ms | 14.07 ms |
| Reject | 1,031 | 1.80 ms | 5.57 ms | 7.61 ms |
| Undo merge | 353 | 1.70 ms | 5.22 ms | 9.40 ms |
| Split | 516 | 0.14 ms | 1.34 ms | 16.15 ms |
| Undo split | 184 | 0.61 ms | 1.63 ms | 8.94 ms |
| Rename | 1,548 | 0.22 ms | 0.33 ms | 4.96 ms |
| All | 10,668 | 0.21 ms | 2.05 ms | 16.15 ms |

`rebuild_all` takes 7.38 s. A proposal recompute takes 0.008 s here, since the fixture has few entities; §7 has the size that matters.

Limits of this run:

- The fixture gives only 354 proposals under the S7 rules, so 355 accepts is all it can make, not the 1,000 first planned. The other kinds make up the 10,000.
- 668 of the 10,668 timed steps logged nothing: 332 split attempts the store refused, such as an entity with one mention, and 336 alias steps on entities with no alias to remove. They are in the timings, so a split's p50 is lower than a real split's.
- The fixture has no OCR language folder, so its 2,178 scanned files fail: 7,822 done, 2,178 failed. The figures above are for what the 7,822 produced.
- This run predates the proposal memory fix (§7), which changes only code off the writer.
- The hot-key line of the report shows 0 documents for the IBAN key because the report prefixes `email:` to a key that isn't an email. The lane and serial figures don't use that line.

## 5. Kill test

`apps/cli/tests/decide.rs`, `nfr_rel_02_kill_among_decisions_is_all_or_nothing`: 20 SIGKILLs among decisions. After each, a decision and its derived rows are both there or both absent.

## 6. Mutation table

Each row is a deliberate source edit; the tests are `m7a_decisions` and `derived_property`.

| Mutation | Caught by |
|---|---|
| Merge newest-wins takes the oldest (INV-CU-05) | `us_11_4_ac1_*`, `a_chain_holds_*`, `undoing_one_link_*` |
| A mention in two splits goes to the oldest | `a_split_works_on_groups_and_on_splits` |
| The oldest edit in a group wins (INV-ER-07) | `inv_er_07_*` |
| A merge rebuilds the pair only, not its groups | `m7a_ac5_*` |
| A merge forgets the group key from before it | `m7a_ac5_*` |
| A merge decision leaves stale proposals (domain §7.4) | `d6_*`, `inv_er_03_*`, `inv_er_04_rejected_*` |
| A split leaves stale proposals | `a_split_ends_the_proposals_*` |
| A type change leaves stale proposals | `a_changed_type_ends_the_proposal_at_once` |
| A run under way stores a rejected pair (INV-ER-04) | `inv_er_04_a_run_that_was_under_way_*` |
| A run under way stores a merged pair (INV-ER-06) | `inv_er_06_a_run_that_was_under_way_*` |
| A reject blocks the pair only, not the groups | `inv_er_04_a_reject_stands_between_the_groups` |
| Proposals use mentions under 0.5 | `proposals_use_the_mentions_shown_by_default` |
| Proposals use mentions of Missing files | `proposals_use_the_mentions_shown_by_default` |
| Moved mentions count for their name in proposals | `a_split_ends_the_proposals_*` |
| Moved mentions still count for their name (INV-ER-01) | `m7a_ac5_*` |
| Group members ignore moved mentions (INV-ER-01) | `m7a_ac5_*` |
| Undo of a merge never accepted is allowed | `us_11_4_ac1_*`, `a_refused_decision_changes_nothing` |
| Accept without a pending proposal (INV-ER-03) | `inv_er_03_*` |
| A split may take every mention | `a_split_works_on_groups_and_on_splits` |
| An edit anchors on the group key, not the display key (§9.5) | `inv_er_07_*`, `us_11_5_ac2_*` |
| Re-scan does not refresh splits (INV-CU-04) | `m7a_ac5_*` |
| A Stage B commit does not refresh splits | `inv_cu_04_split_orphaned_and_active_again` |
| Settling contents does not refresh splits | `inv_cu_04_split_orphaned_and_active_again` |
| A table step does not refresh splits | `m7a_ac5_*` |
| A member keeps its own row after a merge | `m7a_ac5_*` |
| The proposal thread never starts | `proposals_appear_without_being_asked_for` |
| Similar names propose alone | `s7_a_similar_spelling_only_adds_to_an_identifier` |
| An acronym proposes alone | `s7_an_acronym_only_adds_to_an_identifier` |
| An organization part at any distance proposes | `s7_an_organization_is_part_of_a_name_one_word_longer` |
| One-word names are never ambiguous | `s7_a_one_word_name_must_fit_one_entity` |
| Two fits are allowed | `s7_a_one_word_name_must_fit_one_entity` |
| A rejected fit stops counting | `s7_a_rejected_fit_still_counts` |
| A match that stands has no priority | `s7_a_match_that_stands_goes_before_a_higher_one_word_match` |
| An identifier pair counts as a fit | `s7_an_identifier_stands_for_an_ambiguous_word` |
| The fit counts the short side | `s7_a_one_word_name_must_fit_one_entity` |

All 35 are caught. Six of the mutations first survived and showed test gaps, now closed by the tests above. Two others survived because the code they broke was redundant, so that code is gone and they are not listed:

- `regroup` returned the names it changed, and a merge also read each name's group after regrouping. Neither added anything: one decision changes the groups of its own pair only, only a group's key has rows, and `rebuild_keys` already maps a name to its current group. Only the group key from before the decision needs adding. `undoing_a_link_gives_the_far_side_its_new_key` keeps the shape where one side gets a new key that is not in the undone pair.

## 7. Proposal recompute and memory

**Time (TD-2, 495,013 entities, 1,112 proposals): met.** ASSUMPTION: at most 60 s, off the writer. 3.6 to 3.7 s on its own, 6.8 s inside `m7a_decisions_td2`. Reads go through their own connection, and only the short final step takes the writer.

The first build took 15.4 s on its own and peaked at about 1.1 GB, and 30.3 s inside the test. Two changes brought it down: links are held by identifier, so an identifier of one entity costs nothing later, and a word of only digits makes no name block. The 1,112 proposals are identical row by row, also after emptying the table first so that they were really rewritten. `proposal-bench` on the three TD-1 runs of §1 is unchanged.

**Memory (NFR-RES-01, at most 500 MiB).** `cargo xtask mem`, release app, idle RM-1, the §3 project with tables confirmed and no vectors, no model, no source open. It is not directly comparable with M6b's 510 MiB, which had all vectors.

| State | `mangkon` | WebKit | Total RSS |
|---|---|---|---|
| Reopened, no recompute | 212 MiB | 302 MiB | **513 MiB**, flat over 60 s |
| After a recompute, first build | 704 MiB | 304 MiB | 1,008 MiB, flat over 2.5 minutes |
| After a recompute, final build | 331 MiB | 304 MiB | **635 MiB**, flat over 60 s |

**Not met: 135 MiB over the limit after a recompute**, and about 120 MiB above the same project reopened. Four recomputes in one session gave 636, 636, 635 and 635 MiB, so it does not grow. Reopening the project clears it, since a plain reopen does not recompute. The app's own peak during a run is 816 to 928 MiB. Small projects barely see any of this. Recorded as roadmap §4 #22.

What the allocator showed, through a preload library in the running app:

| Build | App RSS | Heap | In use | Free, not returned | App RSS after `malloc_trim(0)` |
|---|---|---|---|---|---|
| First | 704 MiB | 524 MiB | 24 MiB | 499 MiB | 210 MiB |
| Run on its own thread and connection | 333 MiB | 153 MiB | 24 MiB | 129 MiB | 209 MiB |

So the memory is freed heap that glibc keeps, not data the app still holds. Each run now has its own thread and its own read connection, so what it allocates sits in a heap nothing else uses and can be returned. No `unsafe`, no new dependency.

Tried for the rest, without effect and not kept: one arena for the whole process (`glibc.malloc.arena_max=1`, 693 MiB against 703, on the first build), and freeing a chunk over 64 KiB at the end of a run (326 MiB against 333). Where the last 120 MiB or so sits was not found. `malloc_trim` would return it, but it needs `unsafe`, which only `mk-tesseract-sys` may use. A separate process for the run would also return it, at the cost of more code.

## 8. Choices made in M7a

| Choice | Reason |
|---|---|
| D6 (2026-11-07): "Unreviewed merge" means a pending proposal | Read per page by key, not stored on `connection`, since proposals may lag (ADR-0005). |
| No numbered migration; `DERIVED_VERSION` 6 | `decision.kind` is free text and the log is unchanged. Derived tables are dropped and rebuilt on open. |
| Chains hold through a key with no mentions (ASSUMPTION) | A–B–C stays one group while B is absent. Its decision's status still reads Orphaned. |
| Split groups are not proposed (ASSUMPTION) | Proposals are made from name keys, which also keeps a split from being proposed straight back. |
| Any rejected pair between two groups blocks a proposal between them (INV-ER-04) | Stronger than blocking the pair alone. |
| A proposal is made only from mentions shown by default | Not from mentions under 0.5 or of Missing files. It comes back with the file. |
| Merges come only from proposals | No story has "merge any two entities". |
| Similar spelling and acronyms only support an identifier; organization parts must be one word apart; a one-word name must fit one entity (S7, §1) | The scorer as first built gave 651 proposals at 56.8% on real output. |
| Each proposal run has its own thread and read connection (NFR-RES-01, §7) | Otherwise the app keeps about 500 MiB of freed heap on TD-2 for as long as it runs. |
| The decision list's screen moves to M7b | M7a has no new screens; `mangkon-cli decisions` shows the log now (US-11.5 AC2). |

## 9. Open points

- **NFR-RES-01 after a recompute:** 635 MiB on TD-2 against 500 (§7, roadmap §4 #22).
- **Memory during Stage A, not measured.** On a project with confirmed tables, a recompute during a run adds to what NFR-RES-02's 2 GB covers. ASSUMPTION: its peak is about 600 MiB on TD-2's size.
- **A split holds the writer** about 100 to 170 ms on TD-2 and about 30 ms on a small project (§3). That is well under the 2 s limit, so no fix is planned. M7b's screen makes one per click.
- **A merge can read Orphaned while it holds.** If every mention of one member is split out, that name key has no mentions left, so the merge's status is Orphaned until the split is undone. This follows the chain rule (§8). The decision list has wording for it (§11).
- **Thin margins:** link recall passes by one link (§2), S7 mark 1 by one variant and mark 3 by one proposal (§1). The S7 limits were fitted on one corpus.
- **Proposals after `rebuild`:** the table is empty until the next proposal run, a moment later in an open app and on next use from the command line. This is the lag ADR-0005 allows.
- **Not verified:** macOS, Windows, RM-2, and a real model change on TD-2 (about 31 hours).

## 10. Hand check

2026-11-07, Linux, RM-1, a copy of a kept TD-1 project with the real 4B's output (101 files, 173 connections, 95 proposals), release build.

| Step | Result |
|---|---|
| `merge accept` C. Hale + Corvin Hale | One row "Corvin Hale", 10 sources and 12 mentions (4 and 8 before); one fewer connection and proposal |
| `merge reject` G. Almqvist + Gregor Almqvist | Rows unchanged; the pair is gone from `proposals` |
| `merge undo` of the first | Back to two rows at 4 and 8; logged as rejected, the accept reads Superseded |
| `merge accept` of the undone pair | Refused: "This merge proposal is no longer pending" |
| `merge accept` D. Okonkwo-Hart + Daniel Okonkwo-Hart, then `split` the 8 "D." mentions | 19 sources and 21 mentions become 13 and 8, under a `split:` key |
| `rebuild` | Same as before: yes. Matches the sources: yes. Rows unchanged |
| Re-scan of the same two roots | 0 files read again, 173 connections, rows unchanged |
| `split undo` | Back to 19 and 21; the split reads Superseded |
| `decisions` at each step | Time, kind, status, before and after, newest first |

**Connections screen.** The release app's view sweep (`MK_VIEW_SWEEP=1`), which goes through the real panes, on this project with one split and two merged rows: all 1,408 mentions opened, with none failing to open. It logged 26 "page not shown" lines, all pages of two PDF attachments, shown as text by design (`11-ingestion.md` §7). An untouched copy with no decisions gives the same 1,408 and the same 26 lines. This drives the screen by script: a person has not clicked through merged and split rows.

## 11. M7b: review queue and entity page

No schema change and no derived rows bump: reads, one event and screens. Linux on RM-1 only.

**Scripted walk** (`ui/src/project/reviewWalk.ts`, `MK_REVIEW_WALK=1`). The release app presses the queue's keys as real `KeyboardEvent`s and clicks real buttons in WebKit, and checks the core after each step. All 20 steps passed:

| Step | Covers |
|---|---|
| Review tab, highest score first; both sides with aliases, identifiers, evidence, score and reasons | US-11.3 AC1, US-11.2 AC1 |
| `J`, `K`, caps lock, stop at the first row, a modifier does nothing | US-11.3 AC5 |
| A snippet opens the source in one click | NFR-USE-04 |
| `A` merges with the more frequent side's name, the other name becomes an alias, the index stays | US-11.3 AC2 |
| `R` removes the pair, and the proposal thread does not make it again | US-11.3 AC3 |
| `E` + Escape decides nothing and returns focus; typing in the field decides nothing | US-11.3 AC5 |
| `E` + new name + Enter | US-11.3 AC4 |
| Every control in the tab takes focus, with no Tab position of its own | NFR-USE-06, in part |
| "Unreviewed merge" opens the queue filtered to that entity; Show all | US-07.7 AC4 |
| The page's fields; Alt+← leaves it | US-12.1 AC1, US-08.2 AC2 |
| A document and an evidence row open the source, from the decision list's Open | US-12.1 AC2 |
| Rename, type, remove and restore an alias, note | US-12.2 |
| Split one mention, open the new entity, put it back | US-11.4 AC2, US-11.1 AC2 |
| Undo merge | US-11.4 AC1 |
| The decision list, newest first, with time, kind and status | US-11.5 AC2 |

**The project was a stand-in.** No project with real model output was kept, so the walk ran on one made by `keep_a_project_for_the_review_walk` (`crates/mk-core/tests/m7b_entity_detail.rs`, `#[ignore]`) with the fake model: 21 files, 22 names, 7 proposals. It proves the screens and keys work, not how they read on real 4B output or at TD-1's or TD-2's size.

**After the walk.** `mangkon-cli rebuild` on the changed copy: same as incremental, consistent. The view sweep (`MK_VIEW_SWEEP=1`) on that copy with a split added through the CLI: all 68 mentions opened, 0 errors; `rebuild` was the same again afterwards.

**What a script cannot prove.** A synthetic Tab moves no focus, so the walk checks that each control can take focus, not the order a person meets them in. It also never looked at the screens. Both need the hand walk below, which NFR-USE-06 calls for anyway.

**Hand walk: moved to M9 or M10 (2026-11-07).** Not done yet, so nobody has looked at these screens. Steps, in the release app on a project with proposals:

1. Tab through the Review tab: every control is reachable, in a sensible order, with visible focus.
2. `J` and `K` move; `A` and `R` decide, and the next proposal takes the same place in the list.
3. `E` then Escape returns focus to the queue; `E`, a new name and Enter accepts with that name.
4. In a text field, `A`, `R` and the other keys type letters and decide nothing.
5. Open an entity page from the status line; a document and a piece of evidence each open the right spot.
6. Undo a merge, split two mentions and put them back; rename, change type, remove and restore an alias, add a note.
7. In Settings, the decision list has the newest first and reads plainly.
8. Alt+← goes back through the entity page, with list filters and scroll intact.
9. The layout holds at 1280 × 800, in light and dark.

**Guard timing (2026-11-07, RM-1, idle machine, release build): met.** ASSUMPTION: p95 at most 200 ms; no NFR depends on it. `crates/mk-core/tests/m7b_guard_td2.rs` (`#[ignore]`, its header has the commands) makes the same size of project as §3 with today's code, without decisions: 10,000 documents, none failed, 694 confirmed tables, 495,013 entities, 1,112 proposals, 2.0 GB. Stage A took 240 s, the confirmations 75 s, 5 min 23 s in all. The timing runs on a copy, 100 calls per case, one run:

| What the screen reads | First call | p50 | p95 | Max |
|---|---|---|---|---|
| Entity page, the biggest entity (990 mentions, 297 documents) | 6.8 ms | 6.1 ms | 6.5 ms | 8.2 ms |
| Entity page, the busiest proposal once merged (990 + 594 mentions) | 11.6 ms | 9.6 ms | 10.2 ms | 11.6 ms |
| Queue card, the biggest entity on both sides | 4.9 ms | 4.9 ms | 5.3 ms | 6.3 ms |
| Queue card, the busiest proposal | 5.1 ms | 4.7 ms | 5.4 ms | 7.3 ms |
| Queue list, first 200 rows and the count | 2.1 ms | 1.6 ms | 1.8 ms | 2.1 ms |

Limits of this result:

- These are the core's reads alone. The IPC and the drawing are not in them, and the screens have not been timed at this size.
- The copy had just been written, so the disk cache was warm. A first open after a restart will be slower, by an amount not measured.
- Entities here come from tables only, with no model output. A real model on TD-2 would make the biggest ones bigger, and it would take about 31 hours (§3). The evidence read stops at 1,000, so its time should not grow much past that. The documents and identifiers reads have not been timed on bigger entities.

### Choices made in M7b

| Choice | Why |
|---|---|
| A table mention reads "From a table" (US-07.7 AC3) | It has no model confidence to show. |
| An orphaned merge reads "One of these names has no mentions now. It applies again when they come back." Other orphaned decisions keep "No longer applies — the source is gone or changed" (ASSUMPTION, `04-domain.md` §7.5) | The first wording would be untrue of a merge that still holds (§9). |
| The page's evidence stops at the first 1,000 with "first 1,000 of N"; the documents list stops at 1,000 and has its true total (ASSUMPTION, US-12.1) | `MAX_OCCURRENCES` since M2. |
| The queue holds still while open; only your own decisions move it. A change from the proposal thread offers "Queue updated — Show" | US-07.5's rule. An accept often changes other proposals, so the offer tends to follow it about a second later. The next decision refreshes the list anyway. |
| A proposal run that changed the stored rows sends the connections event | Otherwise the mark and the tab's count stay stale until something else fires (ADR-0003). |

### Open points from M7b

- **Mentions past the first 1,000 cannot be picked for a split.** TD-1 does not reach the cap. On TD-2 no single entity does (the biggest has 990 mentions from tables), but a merged group can: the busiest proposal makes one of 1,584.
- **A row held on screen can be out of date.** If the thread has changed a proposal and you decide on the old row without pressing Show, the decision is refused with "no longer pending", stays on screen, and the list refreshes.
- **Not verified:** macOS, Windows, RM-2; the screens on real model output or at TD-2's size.
