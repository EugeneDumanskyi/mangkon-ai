//! M4b in the core: a region change finds identifiers again from the stored text (US-04.2 AC2),
//! the stop list applies at once (US-04.3 AC3, INV-LK-03), a project with older rules is
//! brought up to date on its next run, and card-like numbers never become keys (US-04.6,
//! NFR-SEC-14).

use std::path::Path;
use std::time::Duration;

use mk_core::{DocumentFilter, Project, ProjectSettings, Reprocess};

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

/// A project in `dir` with the test worker and `src` added, not yet ingested.
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

fn connections(p: &Project) -> Vec<String> {
    p.store()
        .derived_state()
        .unwrap()
        .connections
        .into_keys()
        .collect()
}

fn consistent(p: &Project) -> bool {
    let r = p.rebuild().unwrap();
    r.same_as_incremental && r.consistent
}

/// US-04.2 AC2/AC3: without a region a national number is kept as digits; setting the region
/// and accepting "Re-process" finds it again as E.164 from the stored text, so it now connects
/// with the same number written with its country code. No file is read again.
#[test]
fn us_04_2_ac2_region_change_reruns() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "Call 020 7946 0318 today\n"),
            ("b.txt", "Office: +44 20 7946 0318\n"),
        ],
    );
    let p = project(&tmp.path().join("p"), &src);
    let first = p.ingest().unwrap();
    assert_eq!(first.stage_a.done, 2);
    assert_eq!(
        first.identifiers.contents, 0,
        "a new project has nothing to redo"
    );
    assert_eq!(
        keys(&p),
        ["phone:+442079460318", "phone_unnormalized:02079460318"]
    );
    assert!(connections(&p).is_empty());

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
            identifiers: true
        }
    );
    assert!(!p.identifiers_pending().unwrap(), "nothing until accepted");
    assert_eq!(p.reprocess(what).unwrap(), 0, "no file is queued");
    assert!(p.identifiers_pending().unwrap());
    assert!(p.has_runnable_work().unwrap());
    let events = p.subscribe();
    let run = p.ingest().unwrap();
    assert_eq!(run.identifiers.contents, 2);
    assert_eq!(run.stage_a.done, 0, "no file is read again");
    assert_eq!(keys(&p), ["phone:+442079460318"]);
    assert_eq!(connections(&p), ["phone:+442079460318"]);
    assert!(events.recv_timeout(Duration::from_secs(2)).is_ok());
    assert!(!p.identifiers_pending().unwrap());
    assert!(!p.has_runnable_work().unwrap());
    assert!(consistent(&p));

    // A file added later is read with the region.
    write(&src, &[("c.txt", "Fax 020 7946 0318\n")]);
    p.ingest().unwrap();
    assert_eq!(
        p.occurrences("phone:+442079460318").unwrap().len(),
        3,
        "{:?}",
        keys(&p)
    );
}

/// US-04.2: a region the phone metadata does not know is refused with a plain reason.
#[test]
fn unknown_region_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (p, _) = Project::open(&tmp.path().join("p")).unwrap();
    let err = p
        .save_settings(ProjectSettings {
            phone_region: Some("ZZ".into()),
            ..ProjectSettings::default()
        })
        .unwrap_err();
    assert!(err.to_string().contains("ZZ"), "{err}");
    assert_eq!(p.settings().unwrap().phone_region, None);
}

/// US-04.3 AC3, INV-LK-03: a domain on the stop list is never a connection; editing the list
/// rebuilds that domain's connection at once and raises `ConnectionsChanged`.
#[test]
fn us_04_3_ac3_stop_list_edit_applies_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            (
                "a.txt",
                "ann@acme.example and zoe@gmail.com, see https://acme.example/x\n",
            ),
            ("b.txt", "bob@acme.example and yan@gmail.com\n"),
        ],
    );
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    // gmail.com is on the default list: stored, never a connection.
    assert!(keys(&p).contains(&"domain:gmail.com".to_string()));
    assert_eq!(connections(&p), ["domain:acme.example"]);

    let events = p.subscribe();
    let mut stop = p.settings().unwrap().domain_stop_list;
    stop.push("acme.example".into());
    stop.retain(|d| d != "gmail.com");
    let what = p
        .save_settings(ProjectSettings {
            domain_stop_list: stop,
            ..p.settings().unwrap()
        })
        .unwrap();
    assert!(what.is_empty(), "nothing to re-read");
    assert!(events.recv_timeout(Duration::from_secs(2)).is_ok());
    assert_eq!(connections(&p), ["domain:gmail.com"]);
    assert!(consistent(&p));
}

/// M4b: a project whose mentions were made with older rules (an M3 project) has them found
/// again on its next run, and only once.
#[test]
fn older_rules_rerun_on_next_run() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "Pay GB86 QMTB 6099 1248 2133 77\n"),
            ("b.txt", "IBAN GB86QMTB60991248213377\n"),
        ],
    );
    let dir = tmp.path().join("p");
    {
        let p = project(&dir, &src);
        p.ingest().unwrap();
        // As M3 left it: email mentions only and no rules version.
        p.store()
            .write_raw(|c| {
                c.execute_batch(
                    "DELETE FROM identifier_mention WHERE identifier_key NOT LIKE 'email:%';
                     DELETE FROM project_meta WHERE key = 'identifier_version';",
                )?;
                Ok(())
            })
            .unwrap();
        p.rebuild().unwrap();
        assert!(keys(&p).is_empty());
    }
    let (mut p, _) = Project::open(&dir).unwrap();
    *p.worker_config_mut() = worker();
    assert!(p.identifiers_pending().unwrap());
    assert!(p.has_runnable_work().unwrap(), "the desktop starts a run");
    let run = p.ingest().unwrap();
    assert_eq!((run.identifiers.contents, run.stage_a.done), (2, 0));
    assert_eq!(connections(&p), ["iban:GB86QMTB60991248213377"]);
    assert!(consistent(&p));
    assert_eq!(p.ingest().unwrap().identifiers.contents, 0);
}

/// US-03.2: a paused queue runs no re-run either; resumed, it does.
#[test]
fn paused_queue_holds_the_rerun() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &[("a.txt", "Call 020 7946 0318\n")]);
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    p.save_settings(ProjectSettings {
        phone_region: Some("GB".into()),
        ..p.settings().unwrap()
    })
    .unwrap();
    p.reprocess(Reprocess {
        ocr: false,
        identifiers: true,
    })
    .unwrap();
    p.pause().unwrap();
    assert!(!p.has_runnable_work().unwrap());
    assert_eq!(p.ingest().unwrap().identifiers.contents, 0);
    assert_eq!(keys(&p), ["phone_unnormalized:02079460318"]);
    p.resume().unwrap();
    assert_eq!(p.ingest().unwrap().identifiers.contents, 1);
    assert_eq!(keys(&p), ["phone:+442079460318"]);
}

/// US-04.6 AC1/AC2, INV-ID-06, NFR-SEC-14: a card-like number is a flag with its last four
/// digits only: no key, no stored mention, no derived row holds its digits; the documents list
/// counts it and the viewer gets it masked, also after a re-run.
#[test]
fn us_04_6_card_like_is_a_masked_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "card 4111 1111 1111 1111 exp 12/29\n"),
            ("b.csv", "name,card\nAnn,5500-0000-0000-0004\n"),
            ("c.txt", "not a card 4111 1111 1111 1112\n"),
        ],
    );
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    let check = |p: &Project| {
        let mut flags = p.store().all_card_flags().unwrap();
        flags.sort_by(|a, b| a.2.cmp(&b.2));
        assert_eq!(
            flags.iter().map(|f| f.2.as_str()).collect::<Vec<_>>(),
            ["0004", "1111"]
        );
        for (_, document, _) in &flags {
            for f in p.card_flags(*document).unwrap() {
                assert!(f.masked().starts_with("**** **** **** "));
                assert!(f.locator.span().is_some());
            }
        }
        let rows = p
            .documents_page(&DocumentFilter::default(), None, 100)
            .unwrap()
            .rows;
        let count = |rel: &str| {
            rows.iter()
                .find(|r| r.rel_path == rel)
                .unwrap()
                .card_like_count
        };
        assert_eq!((count("a.txt"), count("b.csv"), count("c.txt")), (1, 1, 0));
        let stored: Vec<String> = p
            .store()
            .read_raw(|c| {
                let mut stmt = c.prepare(
                    "SELECT identifier_key || ' ' || raw FROM identifier_mention
                     UNION ALL SELECT key FROM identifier
                     UNION ALL SELECT subject_key FROM connection
                     UNION ALL SELECT last4 FROM sensitive_flag",
                )?;
                let rows = stmt.query_map([], |r| r.get(0))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap();
        for s in &stored {
            let digits: String = s.chars().filter(char::is_ascii_digit).collect();
            assert!(
                !digits.contains("411111") && !digits.contains("550000"),
                "{s}"
            );
        }
        assert!(consistent(p));
    };
    check(&p);
    p.reprocess(Reprocess {
        ocr: false,
        identifiers: true,
    })
    .unwrap();
    assert_eq!(p.ingest().unwrap().identifiers.contents, 3);
    check(&p);
}
