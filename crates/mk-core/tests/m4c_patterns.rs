//! M4c in the core, US-04.7: custom patterns. A saved pattern runs only its own step again on
//! the stored text (AC3, INV-ID-08): no file is read, built-in mentions and card-like flags stay
//! as they were, a pause holds it and a kill resumes from its cursor (INV-PR-05). Names are
//! unique ignoring case (INV-ID-07); look-around and backreferences are refused (AC2); a pattern
//! with more than 10,000 matches is flagged (AC4).

use std::path::Path;

use mk_core::{Project, ProjectError, ProjectSettings, Reprocess, TOO_BROAD};
use mk_kernel::Locator;
use mk_parsers::{ParsedSegment, RowState};
use mk_store::{FoundMention, RerunStep, SegmentFinds};

fn worker() -> mk_core::WorkerConfig {
    mk_core::WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn project(dir: &Path, src: &Path) -> Project {
    let (mut p, _) = Project::open(dir).unwrap();
    *p.worker_config_mut() = worker();
    p.set_stage_a_workers(2);
    p.add_source_root(src).unwrap();
    p
}

fn keys(p: &Project) -> Vec<String> {
    p.store()
        .derived_state()
        .unwrap()
        .identifiers
        .into_keys()
        .collect()
}

fn consistent(p: &Project) -> bool {
    let r = p.rebuild().unwrap();
    r.same_as_incremental && r.consistent
}

/// Stored mentions as `(id, found_by, key)`, in id order.
fn stored(p: &Project) -> Vec<(i64, String, String)> {
    p.store()
        .read_raw(|c| {
            let mut stmt = c.prepare(
                "SELECT id, found_by, identifier_key FROM identifier_mention ORDER BY id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap()
}

fn builtin(p: &Project) -> Vec<(i64, String, String)> {
    stored(p).into_iter().filter(|m| m.1 == "builtin").collect()
}

const TENDER: &str = r"VPA-TND-\d{4}-\d{3}";

fn files() -> [(&'static str, &'static str); 3] {
    [
        (
            "memo.txt",
            "Re tender VPA-TND-2024-017, contact ann@harbour.example\n",
        ),
        (
            "bids.csv",
            "ref,email\nVPA-TND-2024-017,bob@harbour.example\nVPA-TND-2024-018,ann@harbour.example\n",
        ),
        (
            "card.txt",
            "card 4111 1111 1111 1111 for VPA-TND-2024-018\n",
        ),
    ]
}

/// US-04.7 AC1/AC3: a pattern added after ingest is found in the stored text of every file,
/// typed by its name, and connects files; no file is read again.
#[test]
fn us_04_7_ac1_pattern_matches_named_after_it() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &files());
    let p = project(&tmp.path().join("p"), &src);
    assert_eq!(p.ingest().unwrap().stage_a.done, 3);
    assert!(!keys(&p).iter().any(|k| k.starts_with("custom:")));

    p.add_pattern("tender_ref", TENDER).unwrap();
    assert!(p.identifiers_pending().unwrap());
    assert!(p.has_runnable_work().unwrap());
    let run = p.ingest().unwrap();
    assert_eq!(run.stage_a.done, 0, "no file is read again");
    assert_eq!(run.custom.contents, 3);
    assert_eq!(
        run.identifiers.contents, 0,
        "the built-in step does not run"
    );
    let custom: Vec<String> = keys(&p)
        .into_iter()
        .filter(|k| k.starts_with("custom:"))
        .collect();
    assert_eq!(
        custom,
        [
            "custom:tender_ref:VPA-TND-2024-017",
            "custom:tender_ref:VPA-TND-2024-018"
        ]
    );
    let connections = p.store().derived_state().unwrap().connections;
    assert!(connections.contains_key("custom:tender_ref:VPA-TND-2024-017"));
    let occ = p.occurrences("custom:tender_ref:VPA-TND-2024-017").unwrap();
    assert_eq!(occ.len(), 2);
    assert!(
        occ.iter()
            .any(|o| matches!(o.locator, Locator::CsvCell { .. }))
    );
    let listed = p.custom_patterns().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        (
            listed[0].name.as_str(),
            listed[0].match_count,
            listed[0].too_broad
        ),
        ("tender_ref", 4, false)
    );
    assert!(!p.identifiers_pending().unwrap());
    assert!(consistent(&p));

    // A file added later is read with the pattern by the worker.
    write(&src, &[("late.txt", "VPA-TND-2024-018 again\n")]);
    let run = p.ingest().unwrap();
    assert_eq!((run.stage_a.done, run.custom.contents), (1, 0));
    assert_eq!(
        p.occurrences("custom:tender_ref:VPA-TND-2024-018")
            .unwrap()
            .len(),
        3
    );
}

/// US-04.7 AC2: look-around, backreferences, empty matches and bad syntax are refused with a
/// plain reason, as pattern errors; nothing is saved. The live test box says the same.
#[test]
fn us_04_7_ac2_invalid_patterns_are_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, _) = Project::open(&tmp.path().join("p")).unwrap();
    for (expr, says) in [
        (r"(?=VPA)\w+", "not supported"),
        (r"(?<!x)VPA", "not supported"),
        (r"(\w)\1", "not supported"),
        (r"a*", "empty"),
        (r"VPA-(\d", "column"),
    ] {
        let e = p.add_pattern("p", expr).unwrap_err();
        assert!(matches!(e, ProjectError::Pattern(_)), "{expr}: {e:?}");
        assert!(e.to_string().contains(says), "{expr}: {e}");
        let t = p.test_pattern(expr, "VPA-1").unwrap_err();
        assert_eq!(t.to_string(), e.to_string());
    }
    assert!(p.custom_patterns().unwrap().is_empty());
    assert!(!p.identifiers_pending().unwrap());
    let t = p.test_pattern(TENDER, "x VPA-TND-2024-017 y").unwrap();
    assert_eq!(t.matches, 1);
}

/// US-04.7 AC3, INV-ID-08: the custom re-run reads no file (the sources are gone), keeps every
/// built-in mention (same ids) and card-like flag, and changes only custom mentions. Editing
/// the expression and removing the pattern do the same.
#[test]
fn us_04_7_ac3_only_the_custom_step_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &files());
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    let before = builtin(&p);
    assert!(!before.is_empty());
    let flags = p.store().all_card_flags().unwrap();
    assert_eq!(flags.len(), 1);

    // No file can be read: the re-run works on stored text only. The paths stay present
    // because no re-scan runs.
    std::fs::remove_dir_all(&src).unwrap();
    p.add_pattern("tender_ref", TENDER).unwrap();
    let run = p.rerun_identifiers().unwrap();
    assert_eq!(run.contents, 3);
    assert_eq!(builtin(&p), before, "built-in mentions are not touched");
    assert_eq!(p.store().all_card_flags().unwrap(), flags);
    let custom = || {
        stored(&p)
            .into_iter()
            .filter(|m| m.1 == "custom")
            .map(|m| m.2)
            .collect::<Vec<_>>()
    };
    assert_eq!(custom().len(), 4);
    assert!(consistent(&p));

    p.edit_pattern("tender_ref", r"VPA-TND-\d{4}-017").unwrap();
    p.rerun_identifiers().unwrap();
    assert_eq!(builtin(&p), before);
    assert_eq!(
        custom(),
        [
            "custom:tender_ref:VPA-TND-2024-017",
            "custom:tender_ref:VPA-TND-2024-017"
        ]
    );
    assert!(consistent(&p));

    p.remove_pattern("tender_ref").unwrap();
    p.rerun_identifiers().unwrap();
    assert_eq!(builtin(&p), before);
    assert!(custom().is_empty());
    assert!(consistent(&p));
}

/// US-03.2, INV-PR-06: a paused queue holds the custom re-run; resumed, it runs.
#[test]
fn us_04_7_ac3_paused_queue_holds_the_custom_rerun() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &files());
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    p.pause().unwrap();
    p.add_pattern("tender_ref", TENDER).unwrap();
    assert!(!p.has_runnable_work().unwrap());
    assert_eq!(p.ingest().unwrap().custom.contents, 0);
    assert!(p.identifiers_pending().unwrap());
    p.resume().unwrap();
    assert_eq!(p.ingest().unwrap().custom.contents, 3);
    assert!(!p.identifiers_pending().unwrap());
}

/// INV-PR-04/05: a re-run killed after its first content goes on from its cursor when the
/// project opens again, and ends as a run without the kill would.
#[test]
fn us_04_7_ac3_killed_rerun_resumes_from_its_cursor() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "VPA-TND-2024-001\n"),
            ("b.txt", "VPA-TND-2024-001 and VPA-TND-2024-002\n"),
            ("c.txt", "VPA-TND-2024-002\n"),
        ],
    );
    let dir = tmp.path().join("p");
    {
        let p = project(&dir, &src);
        p.ingest().unwrap();
        p.add_pattern("tender_ref", TENDER).unwrap();
        // The first content's transaction commits, then the process dies.
        let first = p.store().rerun_contents(0, 1).unwrap()[0];
        let pattern = mk_identifiers::CustomPattern::compile("tender_ref", TENDER).unwrap();
        p.store()
            .rerun(RerunStep::Custom, 0, first, move |s| {
                let seg = ParsedSegment {
                    locator: s.locator.clone(),
                    text: s.text.clone(),
                    row_state: RowState::Normal,
                    cell_starts: s.cell_starts.clone(),
                    ocr: None,
                };
                let found = mk_worker::extract::segment_custom(
                    &seg,
                    0,
                    &[],
                    std::slice::from_ref(&pattern),
                );
                SegmentFinds {
                    mentions: found
                        .into_iter()
                        .map(|m| FoundMention {
                            locator: m.locator,
                            raw: m.raw,
                            key: m.key,
                            confidence: m.confidence,
                        })
                        .collect(),
                    card_flags: Vec::new(),
                }
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            p.store().rerun_state(RerunStep::Custom).unwrap().1,
            Some(first)
        );
    }
    let (mut p, _) = Project::open(&dir).unwrap();
    *p.worker_config_mut() = worker();
    assert!(p.has_runnable_work().unwrap());
    let run = p.ingest().unwrap();
    assert_eq!(run.custom.contents, 2, "the first content is not redone");
    assert_eq!(
        p.occurrences("custom:tender_ref:VPA-TND-2024-001")
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        p.occurrences("custom:tender_ref:VPA-TND-2024-002")
            .unwrap()
            .len(),
        2
    );
    assert!(!p.identifiers_pending().unwrap());
    assert!(consistent(&p));
}

/// US-04.7 AC4: more than 10,000 stored matches flags the pattern as too broad.
#[test]
fn us_04_7_ac4_too_broad_warns() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let n = TOO_BROAD as usize + 1;
    let text: String = (0..n).map(|i| format!("q{} ", i % 10)).collect();
    write(&src, &[("many.txt", &text), ("few.txt", "z1 z2\n")]);
    let p = project(&tmp.path().join("p"), &src);
    p.add_pattern("q", r"q\d").unwrap();
    p.add_pattern("z", r"z\d").unwrap();
    p.ingest().unwrap();
    let listed = p.custom_patterns().unwrap();
    assert_eq!(
        listed
            .iter()
            .map(|l| (l.name.as_str(), l.match_count, l.too_broad))
            .collect::<Vec<_>>(),
        [("q", n as u64, true), ("z", 2, false)]
    );
}

/// INV-ID-07: names are unique ignoring case; an edit keeps the name; renaming is remove and
/// add.
#[test]
fn inv_id_07_unique_name_ignoring_case() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, _) = Project::open(&tmp.path().join("p")).unwrap();
    p.add_pattern("Tender", TENDER).unwrap();
    let e = p.add_pattern("tender", r"T-\d+").unwrap_err();
    assert!(matches!(e, ProjectError::Pattern(_)), "{e:?}");
    for bad in ["", "a:b", &"x".repeat(41)] {
        assert!(p.add_pattern(bad, r"T-\d+").is_err(), "{bad:?}");
    }
    p.edit_pattern("Tender", r"T-\d+").unwrap();
    assert!(p.edit_pattern("Other", r"T-\d+").is_err());
    p.remove_pattern("Tender").unwrap();
    p.add_pattern("tender", r"T-\d+").unwrap();
    let names: Vec<String> = p
        .custom_patterns()
        .unwrap()
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(names, ["tender"]);
    // Settings saved from the Settings tab leave the patterns alone.
    let what = p
        .save_settings(ProjectSettings {
            custom_patterns: Vec::new(),
            ..p.settings().unwrap()
        })
        .unwrap();
    assert!(what.is_empty());
    assert_eq!(p.custom_patterns().unwrap().len(), 1);
}

/// INV-ID-08: custom matches do not take part in built-in claims: a pattern over email text
/// gives its own key next to the email; a region change re-runs only the built-in step and
/// leaves custom mentions as they were (same ids).
#[test]
fn inv_id_08_steps_are_independent() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &[("a.txt", "ann@harbour.example, 020 7946 0318\n")]);
    let p = project(&tmp.path().join("p"), &src);
    p.add_pattern("mailbox", r"\w+@harbour").unwrap();
    p.ingest().unwrap();
    assert_eq!(
        keys(&p),
        [
            "custom:mailbox:ann@harbour",
            "domain:harbour.example",
            "email:ann@harbour.example",
            "phone_unnormalized:02079460318"
        ]
    );
    let custom = |p: &Project| -> Vec<(i64, String, String)> {
        stored(p).into_iter().filter(|m| m.1 == "custom").collect()
    };
    let before = custom(&p);
    let what = p
        .save_settings(ProjectSettings {
            phone_region: Some("GB".into()),
            ..p.settings().unwrap()
        })
        .unwrap();
    assert_eq!(
        what,
        Reprocess {
            ocr: false,
            identifiers: true,
            custom: false
        }
    );
    p.reprocess(what).unwrap();
    let run = p.ingest().unwrap();
    assert_eq!((run.identifiers.contents, run.custom.contents), (1, 0));
    assert_eq!(custom(&p), before);
    assert!(keys(&p).contains(&"phone:+442079460318".to_string()));
    assert!(consistent(&p));
}
