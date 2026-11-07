//! M6a on [`Workspace`] with the fake `llama-server`: US-10.1–10.4, the evidence rules end to end
//! (NFR-QA-11), a re-run that never mixes two models and whose cancel changes nothing (ADR-0005),
//! Blocked AI jobs (INV-PR-09), 4 slots locally and 1 on an endpoint (NFR-PERF-25), pause within
//! 5 s while a call hangs (NFR-REL-05), the start rule (gap A3).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mk_core::{
    AiChoice, AiLaneState, ConnectionQuery, EgressConfig, EndpointInput, FixedProbe, Gpu,
    GpuVendor, HardwareProfile, KeyChange, ModelRole, ModelsConfig, Project, RuntimeConfig,
    RuntimeState, SpawnOptions, TlsRoots, UnitScope, WorkerConfig, Workspace,
};
use mk_egress::testkit::{FakeKeyring, ScriptedResolver};
use mk_models::testkit::llama::{CONTROL_ENV, Control, FakeExtraction, FakeName};
use mk_models::testkit::{
    FakeHf, NamedResolver, ORIGIN_HOST, test_bytes, test_catalog, test_model,
};
use mk_models::{GB, Model};

const FAKE: &str = env!("CARGO_BIN_EXE_mk-core-fake-llama-server");
const WAIT: Duration = Duration::from_secs(30);

fn machine() -> HardwareProfile {
    HardwareProfile {
        os_family: "linux".into(),
        os: "Linux test".into(),
        cpu: "Test CPU (8 threads)".into(),
        ram_bytes: 64 * GB,
        gpus: vec![Gpu {
            vendor: GpuVendor::Nvidia,
            name: "NVIDIA Test".into(),
            vram_bytes: Some(8_550_000_000),
            driver: Some("580.10".into()),
        }],
        unified_bytes: None,
        apple_silicon: false,
        free_disk_bytes: Some(100 * GB),
        disk_path: "/models".into(),
    }
}

fn name(n: &str) -> FakeName {
    FakeName {
        name: n.into(),
        ..FakeName::default()
    }
}

fn org(n: &str) -> FakeName {
    FakeName {
        name: n.into(),
        kind: "org".into(),
        ..FakeName::default()
    }
}

/// Model "x" finds these names, model "y" those.
fn two_models(x: FakeExtraction, y: FakeExtraction) -> Control {
    Control {
        extraction: HashMap::from([("x.gguf".to_string(), x), ("y.gguf".to_string(), y)]),
        ..Control::default()
    }
}

fn finding(names: Vec<FakeName>) -> FakeExtraction {
    FakeExtraction {
        names,
        ..FakeExtraction::default()
    }
}

struct Setup {
    tmp: tempfile::TempDir,
    ws: Workspace,
    project: Project,
    x: Model,
    y: Model,
    src: PathBuf,
    _hf: FakeHf,
}

const FILES: &[(&str, &str)] = &[
    (
        "a.txt",
        "Paid by Mr Corvin Hale on 2 May.\nVelmora Port Authority signed the invoice.",
    ),
    (
        "b.txt",
        "Corvin Hale wrote to Velmora Port Authority.\nContact john@brightwater.example today.",
    ),
    ("c.txt", "Nothing shared here at all."),
];

fn setup(control: Control) -> Setup {
    setup_with_files(control, FILES)
}

fn setup_with_files(control: Control, files: &[(&str, &str)]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let control_file = tmp.path().join("control.json");
    control.write(&control_file);
    let (xb, yb) = (test_bytes(64 * 1024, 1), test_bytes(64 * 1024, 2));
    let x = test_model("x", ModelRole::Extraction, std::slice::from_ref(&xb));
    let y = test_model("y", ModelRole::Extraction, std::slice::from_ref(&yb));
    let hf = FakeHf::serving(&[
        (&x, std::slice::from_ref(&xb)),
        (&y, std::slice::from_ref(&yb)),
    ]);
    let resolver = Arc::new(ScriptedResolver::new());
    let runtime = RuntimeConfig {
        path_var: Some(Default::default()),
        spawn: SpawnOptions {
            env: vec![(CONTROL_ENV.into(), control_file.clone().into())],
        },
        ..RuntimeConfig::default()
    };
    let ws = Workspace::open_with_runtime(
        &tmp.path().join("app"),
        EgressConfig {
            resolver: resolver.clone(),
            secrets: Some(Arc::new(FakeKeyring::new())),
            roots: TlsRoots::Platform,
            call_timeout: Duration::from_secs(40),
        },
        ModelsConfig {
            models_dir: Some(tmp.path().join("models")),
            catalog: test_catalog(vec![x.clone(), y.clone()]),
            probe: Arc::new(FixedProbe(machine())),
            download: hf.config(NamedResolver::new(&[ORIGIN_HOST])),
        },
        runtime,
    )
    .unwrap();
    ws.set_llama_server_path(Some(Path::new(FAKE))).unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (name, text) in files {
        std::fs::write(src.join(name), text).unwrap();
    }
    let (mut project, _) = ws.create("Case", tmp.path()).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.add_source_root(&src).unwrap();
    let s = Setup {
        tmp,
        ws,
        project,
        x,
        y,
        src,
        _hf: hf,
    };
    s.install(&s.x.clone());
    s.install(&s.y.clone());
    s.choose("x");
    s.project.ingest().unwrap();
    s
}

impl Setup {
    fn install(&self, m: &Model) {
        self.ws.start_download(&m.id).unwrap();
        wait_until("installed", || {
            self.ws
                .model_catalog()
                .unwrap()
                .iter()
                .any(|v| v.id == m.id && v.state == mk_core::DownloadState::Installed)
        });
    }

    fn choose(&self, id: &str) {
        self.ws
            .set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local(id.into())))
            .unwrap();
    }

    fn run(&self) -> mk_core::StageBSummary {
        self.ws.run_stage_b(&self.project).unwrap()
    }

    fn schedule_and_run(&self) -> mk_core::StageBSummary {
        self.ws.schedule_stage_b(&self.project).unwrap();
        self.run()
    }

    fn view(&self) -> mk_core::AiExtractionView {
        self.ws.ai_extraction_view(&self.project).unwrap()
    }

    fn keys(&self) -> Vec<String> {
        self.project
            .store()
            .derived_state()
            .unwrap()
            .connections
            .keys()
            .cloned()
            .collect()
    }

    /// Every AI mention as (name key, raw, model).
    fn ai_mentions(&self) -> Vec<(String, String, Option<i64>)> {
        self.project
            .store()
            .read_raw(|c| {
                let mut stmt = c.prepare(
                    "SELECT name_key, raw, ai_run_id FROM entity_mention
                     WHERE producer = 'model' ORDER BY id",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap()
    }

    fn healthy(&self) {
        let h = self.project.check_health().unwrap();
        assert!(h.is_healthy(), "{h:?}");
    }

    /// The stored derived rows equal the reference model and a rebuild.
    fn consistent(&self) {
        let before = self.project.store().derived_state().unwrap();
        let reference = mk_graph::model::reference(&self.project.store().facts().unwrap());
        assert_eq!(before, reference, "reference");
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn hale_and_velmora() -> FakeExtraction {
    finding(vec![name("Corvin Hale"), org("Velmora Port Authority")])
}

// ---- US-10.1, US-10.2 ----

/// US-10.1 AC1: each found person or organization is a mention with the name as written, its type,
/// its evidence span, locator and confidence; they reach the connections list.
#[test]
fn us_10_1_ac1_mentions_stored_and_connected() {
    let s = setup(two_models(hale_and_velmora(), finding(vec![])));
    let out = s.schedule_and_run();
    assert!(out.done >= 3, "{out:?}");
    assert_eq!(out.mentions, 4);
    assert_eq!(out.blocked, None);
    s.healthy();
    let keys = s.keys();
    assert!(keys.contains(&"person:corvin hale".to_string()), "{keys:?}");
    assert!(keys.contains(&"org:velmora port authority".to_string()));
    let occ = s.project.occurrences("person:corvin hale").unwrap();
    assert_eq!(occ.len(), 2);
    assert_eq!(occ[0].raw, "Corvin Hale");
    assert_eq!(occ[0].ai_model.as_deref(), Some("Test x"));
    assert!((occ[0].confidence - 0.9).abs() < 1e-6);
    assert!(occ[0].locator.span().is_some());
    // The mention opens through the same view as any other.
    let ctx = s
        .project
        .source_context(occ[0].mention_id)
        .unwrap()
        .unwrap();
    assert_eq!(ctx.hit.unwrap().segment_id, occ[0].segment_id);
    let view = s.view();
    assert_eq!(view.state, AiLaneState::Done);
    assert_eq!(view.results_model.as_deref(), Some("Test x"));
    assert_eq!(view.mentions, 4);
}

/// Found on TD-1 with the real model: this name's key did not normalize to itself, so it was left
/// out of the entities and every later rebuild, and with it a re-run's switch, failed.
#[test]
fn a_name_with_a_suffix_under_a_suffix_reaches_the_list_and_survives_a_rebuild() {
    let s = setup_with_files(
        two_models(finding(vec![org("Chevron U. S. A. Inc.")]), finding(vec![])),
        &[
            ("a.txt", "Chevron U. S. A. Inc. wrote first."),
            ("b.txt", "Then Chevron U. S. A. Inc. wrote again."),
        ],
    );
    s.schedule_and_run();
    let keys = s.keys();
    assert!(keys.contains(&"org:chevron u".to_string()), "{keys:?}");
    s.project.store().rebuild_all().unwrap();
    s.healthy();
    s.consistent();
}

/// US-10.1 AC2: the schema is sent; a bad reply is retried once with the error, then the item is
/// Done with no mentions and a count.
#[test]
fn us_10_1_ac2_schema_and_one_retry() {
    let log = tempfile::NamedTempFile::new().unwrap();
    let mut c = two_models(
        FakeExtraction {
            bad_first: true,
            ..hale_and_velmora()
        },
        FakeExtraction {
            bad_always: true,
            ..FakeExtraction::default()
        },
    );
    c.requests_log = Some(log.path().to_path_buf());
    let s = setup(c);
    let out = s.schedule_and_run();
    assert_eq!(out.invalid_responses, 0);
    assert_eq!(out.mentions, 4);
    let lines = std::fs::read_to_string(log.path()).unwrap();
    let bodies: Vec<serde_json::Value> = lines
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let first = &bodies[0]["body"];
    assert_eq!(first["response_format"]["json_schema"]["name"], "mentions");
    assert_eq!(first["response_format"]["json_schema"]["strict"], true);
    assert_eq!(first["temperature"], 0.0);
    assert_eq!(first["seed"], 1);
    assert_eq!(first["max_tokens"], 2048);
    assert_eq!(bodies.len(), out.done as usize * 2);
    let retry = bodies
        .iter()
        .find(|b| b["body"]["messages"].as_array().unwrap().len() == 4)
        .unwrap();
    assert!(
        retry["body"]["messages"][3]["content"]
            .as_str()
            .unwrap()
            .contains("did not match the JSON schema")
    );

    // Still bad after the retry: Done, no mentions, counted, the batch goes on.
    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    let out = s.run();
    assert_eq!(out.mentions, 0);
    assert!(out.invalid_responses >= 3);
    assert_eq!(s.view().invalid_responses, out.invalid_responses);
    assert_eq!(s.view().state, AiLaneState::Done);
}

/// A reply cut at the token limit is not retried: Done with no mentions and a count, the batch
/// goes on (INV-AI-03).
#[test]
fn inv_ai_03_cut_reply_is_not_retried() {
    let log = tempfile::NamedTempFile::new().unwrap();
    let mut c = two_models(
        FakeExtraction {
            cut_text: Some("Nothing shared".into()),
            ..hale_and_velmora()
        },
        FakeExtraction::default(),
    );
    c.requests_log = Some(log.path().to_path_buf());
    let s = setup(c);
    let out = s.schedule_and_run();
    assert_eq!(out.invalid_responses, 1);
    assert_eq!(out.mentions, 4);
    assert_eq!(s.view().state, AiLaneState::Done);
    let lines = std::fs::read_to_string(log.path()).unwrap();
    assert_eq!(lines.lines().count(), out.done as usize);
}

/// US-10.1 AC3: a mention under 0.5 is stored and hidden by default; "Show low confidence" reveals
/// it and the connection only such mentions make (`low_only`).
#[test]
fn us_10_1_ac3_low_confidence_hidden_by_default() {
    let low = FakeName {
        confidence: Some(0.3),
        ..name("Corvin Hale")
    };
    let s = setup(two_models(finding(vec![low]), finding(vec![])));
    let out = s.schedule_and_run();
    assert_eq!(out.mentions, 2);
    assert_eq!(s.ai_mentions().len(), 2, "stored");
    let store = s.project.store();
    let plain = ConnectionQuery::default();
    let low_on = ConnectionQuery {
        include_low: true,
        ..ConnectionQuery::default()
    };
    assert_eq!(store.connection_count(&plain).unwrap(), 0);
    assert_eq!(store.connection_count(&low_on).unwrap(), 1);
    let row = &store.connections_page(&low_on, None, 10).unwrap()[0];
    assert!(row.row.low_only && row.row.low_confidence);
    assert_eq!(row.row.source_count, 2);
    assert!(
        s.project
            .occurrences("person:corvin hale")
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        s.project
            .occurrences_with("person:corvin hale", true)
            .unwrap()
            .len(),
        2
    );
    s.healthy();
}

/// US-10.2 AC1/AC2, NFR-QA-11: a model that invents evidence, or a name not in its evidence, stores
/// nothing, and the count shows.
#[test]
fn us_10_2_invented_evidence_is_discarded_and_counted() {
    let invented = FakeName {
        evidence: Some("Corvin Hale, chief of the Moon".into()),
        when: Some("Corvin Hale".into()),
        ..name("Corvin Hale")
    };
    let wrong_name = FakeName {
        evidence: Some("Velmora Port Authority".into()),
        when: Some("Velmora Port Authority".into()),
        ..name("Daniel Okonkwo-Hart")
    };
    let s = setup(two_models(
        finding(vec![invented, wrong_name]),
        finding(vec![]),
    ));
    let out = s.schedule_and_run();
    assert_eq!(out.mentions, 0);
    assert_eq!(out.discarded, 4);
    assert!(s.ai_mentions().is_empty());
    assert_eq!(s.view().discarded, 4);
    assert_eq!(s.project.check_health().unwrap().invented_evidence, 0);
}

/// `09-model-check.md` §5.3: a name on a Stage A identifier is dropped with its own count.
#[test]
fn identifier_overlap_is_dropped_and_counted() {
    let john = FakeName {
        evidence: Some("john@brightwater.example".into()),
        ..name("john")
    };
    let s = setup(two_models(finding(vec![john]), finding(vec![])));
    let out = s.schedule_and_run();
    assert_eq!(out.identifier_dropped, 1);
    assert_eq!(out.discarded, 0);
    assert_eq!(out.mentions, 0);
    assert_eq!(s.view().identifier_dropped, 1);
}

/// US-10.3 AC1: every view follows the build's flag (both of its values are checked in `mk-ai`).
#[test]
fn us_10_3_view_carries_the_flag() {
    let s = setup(Control::default());
    assert_eq!(s.view().experimental, mk_ai::experimental());
}

// ---- US-10.4, ADR-0005 ----

/// The visible state compared at every commit of a re-run: nothing moves until the switch, then
/// everything is the new model's. Decisions stay (US-10.4 AC1).
#[test]
fn adr_0005_rerun_shows_old_results_until_the_switch() {
    let slow_y = FakeExtraction {
        delay_ms: 100,
        ..finding(vec![org("Velmora Port Authority"), name("Paid")])
    };
    let s = setup(two_models(hale_and_velmora(), slow_y));
    s.schedule_and_run();
    s.project
        .set_ignored("org:velmora port authority", true)
        .unwrap();
    let old = s.project.store().derived_state().unwrap();
    let old_mentions = s.ai_mentions();
    let old_view = s.view();
    assert!(old_view.mentions > 0);
    let x_run = s.project.store().active_ai_run().unwrap().unwrap();

    s.choose("y");
    let view = s.view();
    assert_eq!(view.rerun_with.as_deref(), Some("Test y"));
    assert_eq!(view.results_model.as_deref(), Some("Test x"));
    s.ws.start_rerun(&s.project).unwrap();
    let view = s.view();
    assert_eq!(view.rerun_model.as_deref(), Some("Test y"));
    assert_eq!(view.rerun_with, None);

    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        let mut samples = 0;
        while !runner.is_finished() {
            // Read before the run: if the run is still the old one, so were these.
            let now = s.project.store().derived_state().unwrap();
            let mentions = s.ai_mentions();
            let run = s.project.store().active_ai_run().unwrap().unwrap();
            if run.id == x_run.id {
                assert_eq!(now, old, "changed before the switch");
                assert_eq!(mentions, old_mentions, "mentions changed");
                samples += 1;
            }
            // The counts shown belong to the model named beside them, not to the re-run's.
            let shown = s.view();
            if shown.results_model.as_deref() == Some("Test x") {
                assert_eq!(shown.mentions, old_view.mentions, "count shown changed");
                assert_eq!(shown.discarded, old_view.discarded);
                assert_eq!(shown.invalid_responses, old_view.invalid_responses);
                assert_eq!(shown.identifier_dropped, old_view.identifier_dropped);
            }
            if run.id != x_run.id {
                // All new: never a mix.
                let runs: std::collections::BTreeSet<_> =
                    s.ai_mentions().iter().map(|m| m.2).collect();
                assert_eq!(runs.len(), 1);
                assert_eq!(runs.into_iter().next().unwrap(), Some(run.id));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(samples > 0);
        runner.join().unwrap();
    });
    let store = s.project.store();
    let new_run = store.active_ai_run().unwrap().unwrap();
    assert_ne!(new_run.id, x_run.id);
    assert_eq!(new_run.model_name, "Test y");
    assert!(store.pending_ai_run().unwrap().is_none());
    // The new model's only: Corvin Hale is gone, and the mentions say which model.
    let keys = s.keys();
    assert!(!keys.contains(&"person:corvin hale".to_string()));
    let occ = s.project.occurrences("org:velmora port authority").unwrap();
    assert!(!occ.is_empty());
    assert!(occ.iter().all(|o| o.ai_model.as_deref() == Some("Test y")));
    // The ignore decision kept and applied on the new run.
    let row = &store.derived_state().unwrap().connections["org:velmora port authority"];
    assert!(row.ignored);
    let view = s.view();
    assert_eq!(view.results_model.as_deref(), Some("Test y"));
    assert_eq!(view.mentions, new_run.mentions);
    s.healthy();
    s.consistent();
}

/// Cancel changes nothing visible, and the pending output is gone (ADR-0005 step 3).
#[test]
fn adr_0005_cancel_changes_nothing() {
    let hanging_y = FakeExtraction {
        delay_ms: 300,
        ..finding(vec![name("Paid")])
    };
    let s = setup(two_models(hale_and_velmora(), hanging_y));
    s.schedule_and_run();
    let before = s.project.store().derived_state().unwrap();
    let mentions = s.ai_mentions();
    let shown = s.view().mentions;
    let run = s.project.store().active_ai_run().unwrap().unwrap();
    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            s.view().mentions,
            shown,
            "the count moved before the cancel"
        );
        assert_eq!(
            s.ws.cancel_extraction(&s.project).unwrap(),
            mk_core::AiCancelled::Rerun
        );
        runner.join().unwrap();
    });
    let store = s.project.store();
    assert_eq!(store.derived_state().unwrap(), before);
    assert_eq!(s.ai_mentions(), mentions);
    assert_eq!(s.view().mentions, shown);
    assert_eq!(store.active_ai_run().unwrap().unwrap().id, run.id);
    assert!(store.pending_ai_run().unwrap().is_none());
    let pending: i64 = store
        .read_raw(|c| {
            Ok(
                c.query_row("SELECT COUNT(*) FROM pending_entity_mention", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap();
    assert_eq!(pending, 0);
    s.healthy();
}

/// What a cancelled re-run had finished counts for no later run: its output is gone, so its units
/// are neither shown as done nor skipped (ADR-0005 step 3).
#[test]
fn adr_0005_a_cancelled_rerun_leaves_nothing_for_the_next() {
    const MANY: &[(&str, &str)] = &[
        (
            "a.txt",
            "Corvin Hale met the board of Velmora Port Authority.",
        ),
        (
            "b.txt",
            "Corvin Hale wrote to Velmora Port Authority again.",
        ),
        ("c.txt", "Velmora Port Authority answered Corvin Hale."),
        ("d.txt", "Corvin Hale signed for Velmora Port Authority."),
        ("e.txt", "Velmora Port Authority paid Corvin Hale."),
        ("f.txt", "Corvin Hale left Velmora Port Authority."),
        ("g.txt", "Velmora Port Authority hired Corvin Hale."),
        ("h.txt", "Corvin Hale thanked Velmora Port Authority."),
    ];
    let slow_y = FakeExtraction {
        delay_ms: 200,
        ..finding(vec![org("Velmora Port Authority")])
    };
    let s = setup_with_files(two_models(hale_and_velmora(), slow_y), MANY);
    s.schedule_and_run();
    let first = s.view();
    assert_eq!(first.units_done, MANY.len() as u64);

    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        wait_until("some of the re-run done", || s.view().units_done >= 1);
        s.ws.cancel_extraction(&s.project).unwrap();
        runner.join().unwrap();
    });
    // Looking for new work after the cancel, as every lane start does.
    s.choose("x");
    s.ws.schedule_stage_b(&s.project).unwrap();
    s.run();
    let back = s.view();
    assert_eq!(
        back.units_done, first.units_done,
        "the re-run's work counted"
    );
    assert_eq!(back.units_total, first.units_total);

    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    s.run();
    let view = s.view();
    assert_eq!(view.results_model.as_deref(), Some("Test y"));
    assert_eq!(view.units_done, MANY.len() as u64, "units counted twice");
    assert_eq!(view.units_total, MANY.len() as u64);
    assert_eq!(view.mentions, MANY.len() as u64, "a unit was skipped");
    s.healthy();
    s.consistent();
}

/// Units cancelled before any model was bound stay ended when a later file gives the project its
/// first run (gap A3): only the user's "all" brings them back.
#[test]
fn gap_a3_units_cancelled_before_a_run_stay_ended() {
    let s = setup(two_models(hale_and_velmora(), hale_and_velmora()));
    s.ws.schedule_stage_b(&s.project).unwrap();
    s.ws.cancel_extraction(&s.project).unwrap();
    std::fs::write(s.src.join("new.txt"), "Corvin Hale joined later.").unwrap();
    s.project.ingest().unwrap();
    s.schedule_and_run();
    let view = s.view();
    assert_eq!((view.units_done, view.units_total), (1, 1));
    // Looking for new work again finds none.
    s.schedule_and_run();
    assert_eq!(s.view().units_total, 1, "cancelled units came back");
    s.healthy();
}

/// Cancelling the whole queue during a re-run does the same (US-03.3).
#[test]
fn adr_0005_queue_cancel_drops_the_rerun() {
    let s = setup(two_models(hale_and_velmora(), finding(vec![])));
    s.schedule_and_run();
    let before = s.project.store().derived_state().unwrap();
    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    s.project.cancel().unwrap();
    assert!(s.project.store().pending_ai_run().unwrap().is_none());
    assert_eq!(s.project.store().derived_state().unwrap(), before);
    s.healthy();
}

/// A re-run where every item fails keeps the old results: there is nothing to switch to.
#[test]
fn adr_0005_all_failed_rerun_keeps_the_old_results() {
    let failing_y = FakeExtraction {
        error_text: Some(" ".into()),
        ..FakeExtraction::default()
    };
    let s = setup(two_models(hale_and_velmora(), failing_y));
    s.schedule_and_run();
    let before = s.project.store().derived_state().unwrap();
    let run = s.project.store().active_ai_run().unwrap().unwrap();
    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    let out = s.run();
    assert!(out.failed >= 3, "{out:?}");
    assert_eq!(
        s.project.store().active_ai_run().unwrap().unwrap().id,
        run.id
    );
    assert!(s.project.store().pending_ai_run().unwrap().is_none());
    assert_eq!(s.project.store().derived_state().unwrap(), before);
    s.healthy();
}

/// A job's Stage A file changes during a re-run: the mentions on the old segments go with them,
/// and the new segments' units join the same run.
#[test]
fn adr_0005_file_changed_during_a_rerun_joins_it() {
    let s = setup(two_models(hale_and_velmora(), finding(vec![name("Later")])));
    s.schedule_and_run();
    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    std::fs::write(s.src.join("c.txt"), "Later wrote this new text.").unwrap();
    s.project.ingest().unwrap();
    s.ws.schedule_stage_b(&s.project).unwrap();
    let out = s.run();
    assert!(out.mentions >= 1, "{out:?}");
    assert!(s.ai_mentions().iter().any(|m| m.1 == "Later"));
    assert_eq!(
        s.project
            .store()
            .active_ai_run()
            .unwrap()
            .unwrap()
            .model_name,
        "Test y"
    );
    s.healthy();
    s.consistent();
}

/// Model changed (decided 2026-11-05): new work is Blocked with a reason and "Re-run with Y";
/// choosing X again unblocks it. Never mixed, nothing large starts by itself.
#[test]
fn model_changed_blocks_new_work_until_x_again() {
    let s = setup(two_models(hale_and_velmora(), finding(vec![name("Wrong")])));
    s.schedule_and_run();
    let mentions = s.ai_mentions();
    std::fs::write(s.src.join("d.txt"), "Corvin Hale paid again.").unwrap();
    s.choose("y");
    s.project.ingest().unwrap();
    let out = s.schedule_and_run();
    assert_eq!(out.done, 0);
    assert!(out.blocked.is_some());
    let view = s.view();
    assert_eq!(view.state, AiLaneState::Blocked);
    let reason = view.reason.unwrap();
    assert!(reason.contains("re-run with Test y"), "{reason}");
    assert!(reason.contains("choose Test x again"), "{reason}");
    assert_eq!(view.rerun_with.as_deref(), Some("Test y"));
    assert_eq!(s.ai_mentions(), mentions);

    s.choose("x");
    let out = s.run();
    assert_eq!(out.done, 1);
    assert_eq!(out.blocked, None);
    assert_eq!(s.view().state, AiLaneState::Done);
    let runs: std::collections::BTreeSet<_> = s.ai_mentions().iter().map(|m| m.2).collect();
    assert_eq!(runs.len(), 1);
    s.healthy();
}

/// Run identity on every item: an endpoint edited mid-run Blocks the job instead of mixing.
#[test]
fn run_identity_endpoint_edit_blocks_instead_of_mixing() {
    let s = setup(Control::default());
    let served = serve(
        &s,
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        finding_everywhere(hale_and_velmora()),
    );
    let id = add_endpoint(&s, format!("http://127.0.0.1:{}", served.port), "model-a");
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(id.clone())))
        .unwrap();
    s.ws.schedule_stage_b(&s.project).unwrap();
    assert_eq!(s.run().done, 3);
    let run = s.project.store().active_ai_run().unwrap().unwrap();
    assert!(run.identity.as_str().contains("model-a"));
    assert_eq!(run.destination_class, "this_machine");
    // New work, and the endpoint's model is edited.
    std::fs::write(s.src.join("d.txt"), "New work.").unwrap();
    s.project.ingest().unwrap();
    s.ws.save_endpoint(EndpointInput {
        id: Some(id.clone()),
        name: "Edited".into(),
        base_url: format!("http://127.0.0.1:{}", served.port),
        model: "model-b".into(),
        key: KeyChange::Keep,
    })
    .unwrap();
    let out = s.schedule_and_run();
    assert_eq!(out.done, 0);
    assert_eq!(s.view().state, AiLaneState::Blocked);
    assert_eq!(s.view().rerun_with.as_deref(), Some("Edited"));
}

/// Run identity on every item, while the lane runs: the gate reads an endpoint's config again on
/// each call, so without this check the next unit would go to the new model under the old run.
#[test]
fn run_identity_edit_during_a_call_sends_nothing_to_the_new_model() {
    let files: Vec<(String, String)> = (0..8)
        .map(|i| (format!("f{i}.txt"), format!("Corvin Hale wrote note {i}.")))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let s = setup_with_files(Control::default(), &refs);
    let log = s.tmp.path().join("calls.log");
    let mut control = finding_everywhere(FakeExtraction {
        delay_ms: 150,
        ..finding(vec![name("Corvin Hale")])
    });
    control.requests_log = Some(log.clone());
    let served = serve(&s, IpAddr::V4(Ipv4Addr::LOCALHOST), control);
    let url = format!("http://127.0.0.1:{}", served.port);
    let id = add_endpoint(&s, url.clone(), "model-a");
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(id.clone())))
        .unwrap();
    s.ws.schedule_stage_b(&s.project).unwrap();
    let store = s.project.store();
    let out = std::thread::scope(|scope| {
        let lane = scope.spawn(|| s.run());
        wait_until("a first unit", || store.ai_progress().unwrap().done >= 1);
        s.ws.save_endpoint(EndpointInput {
            id: Some(id.clone()),
            name: "Edited".into(),
            base_url: url.clone(),
            model: "model-b".into(),
            key: KeyChange::Keep,
        })
        .unwrap();
        lane.join().unwrap()
    });
    assert!(out.done < 8, "{out:?}");
    assert_eq!(s.view().state, AiLaneState::Blocked);
    let calls = std::fs::read_to_string(&log).unwrap();
    let models: Vec<String> = calls
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            v["body"]["model"].as_str().unwrap().to_string()
        })
        .collect();
    assert!(!models.is_empty());
    assert!(
        models.iter().all(|m| m == "model-a"),
        "the new model got a call: {models:?}"
    );
    s.healthy();
}

/// ADR-0005 with no timing in it: a pinned client sends to the run's model, and after an edit
/// sends nothing at all.
#[test]
fn run_identity_pinned_client_refuses_after_an_edit() {
    let s = setup(Control::default());
    let log = s.tmp.path().join("calls.log");
    let mut control = finding_everywhere(finding(vec![]));
    control.requests_log = Some(log.clone());
    let served = serve(&s, IpAddr::V4(Ipv4Addr::LOCALHOST), control);
    let url = format!("http://127.0.0.1:{}", served.port);
    let id = add_endpoint(&s, url.clone(), "model-a");
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(id.clone())))
        .unwrap();
    let origin = mk_egress::BaseUrl::parse(&url).unwrap().origin().key();
    let mut client = s.ws.ai_client(&s.project, ModelRole::Extraction).unwrap();
    client.pin_to_run(&mk_ai::RunIdentity::endpoint(&id, "model-a", &origin));
    let request = mk_egress::ChatRequest {
        messages: vec![mk_egress::ChatMessage::user("Hello")],
        schema: None,
        max_tokens: None,
        temperature: None,
        seed: None,
    };
    client.client.chat(&request).unwrap();
    s.ws.save_endpoint(EndpointInput {
        id: Some(id.clone()),
        name: "Edited".into(),
        base_url: url.clone(),
        model: "model-b".into(),
        key: KeyChange::Keep,
    })
    .unwrap();
    assert_eq!(
        client.client.chat(&request).unwrap_err(),
        mk_egress::GateError::PinChanged
    );
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.lines().count(), 1, "{calls}");
}

/// The retry of an unreadable reply is a call of its own: an edit during the first call stops it.
#[test]
fn run_identity_edit_before_a_retry_sends_nothing_to_the_new_model() {
    let s = setup_with_files(Control::default(), &[("a.txt", "Corvin Hale wrote this.")]);
    let log = s.tmp.path().join("calls.log");
    let mut control = finding_everywhere(FakeExtraction {
        bad_first: true,
        delay_ms: 500,
        ..finding(vec![name("Corvin Hale")])
    });
    control.requests_log = Some(log.clone());
    let served = serve(&s, IpAddr::V4(Ipv4Addr::LOCALHOST), control);
    let url = format!("http://127.0.0.1:{}", served.port);
    let id = add_endpoint(&s, url.clone(), "model-a");
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(id.clone())))
        .unwrap();
    s.ws.schedule_stage_b(&s.project).unwrap();
    let out = std::thread::scope(|scope| {
        let lane = scope.spawn(|| s.run());
        wait_until("the first call", || log.exists());
        s.ws.save_endpoint(EndpointInput {
            id: Some(id.clone()),
            name: "Edited".into(),
            base_url: url.clone(),
            model: "model-b".into(),
            key: KeyChange::Keep,
        })
        .unwrap();
        lane.join().unwrap()
    });
    assert_eq!(out.done, 0, "{out:?}");
    assert_eq!(s.view().state, AiLaneState::Blocked);
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls.lines().count(), 1, "{calls}");
    s.healthy();
}

/// NFR-QA-11, ADR-0005: the health check really finds what it says it looks for, so a kill test
/// that passes means something.
#[test]
fn nfr_qa_11_health_finds_invented_evidence_and_mixed_runs() {
    let s = setup(two_models(hale_and_velmora(), finding(vec![])));
    s.schedule_and_run();
    s.healthy();
    let store = s.project.store();
    store
        .write_raw(|c| {
            c.execute(
                "UPDATE entity_mention SET span_start = 0, span_end = 1
                 WHERE id = (SELECT MIN(id) FROM entity_mention WHERE producer = 'model')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let h = s.project.check_health().unwrap();
    assert_eq!(h.invented_evidence, 1, "{h:?}");
    assert!(!h.is_healthy());
    assert_eq!(h.mixed_ai_runs, 0);
    store
        .write_raw(|c| {
            c.execute(
                "UPDATE entity_mention SET ai_run_id = NULL
                 WHERE id = (SELECT MAX(id) FROM entity_mention WHERE producer = 'model')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let h = s.project.check_health().unwrap();
    assert_eq!(h.mixed_ai_runs, 1, "{h:?}");
    assert!(!h.is_healthy());
}

// ---- INV-PR-09 ----

/// ADR-0006: an endpoint that stops answering: each item goes back, with a backoff, and three
/// failures in a row Block the job. Nothing is lost, and the lane goes on when it answers again.
#[test]
fn adr_0006_three_network_failures_block_and_lose_nothing() {
    let s = setup(Control::default());
    let rules = || finding_everywhere(hale_and_velmora());
    let served = serve(&s, IpAddr::V4(Ipv4Addr::LOCALHOST), rules());
    let port = served.port;
    let id = add_endpoint(&s, format!("http://127.0.0.1:{port}"), "m");
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(id)))
        .unwrap();
    s.ws.schedule_stage_b(&s.project).unwrap();
    drop(served);

    let out = s.run();
    assert_eq!((out.done, out.failed), (0, 0), "{out:?}");
    assert!(out.blocked.is_some(), "{out:?}");
    assert_eq!(s.view().state, AiLaneState::Blocked);
    assert_eq!(in_progress(&s), 0);
    assert_eq!(s.view().units_total, 3);

    let _served = serve_at(&s, IpAddr::V4(Ipv4Addr::LOCALHOST), port, rules());
    let out = s.run();
    assert_eq!(out.done, 3, "{out:?}");
    assert_eq!(out.blocked, None);
    assert_eq!(s.ai_mentions().len(), 4);
    s.healthy();
}

/// INV-PR-09, US-09.7 AC3: no model at all: the job waits Blocked with the reason, Layer 1 goes on.
#[test]
fn inv_pr_09_no_model_blocks_and_stage_a_goes_on() {
    let s = setup(Control::default());
    s.ws.delete_model(&s.x.id).unwrap();
    s.ws.delete_model(&s.y.id).unwrap();
    s.ws.set_ai_choice(ModelRole::Extraction, None).unwrap();
    let scheduled = s.ws.schedule_stage_b(&s.project).unwrap();
    assert_eq!(scheduled.units, 3);
    let out = s.run();
    assert_eq!(out.done, 0);
    assert_eq!(out.blocked.as_deref(), Some(mk_core::NO_MODEL_ALLOWED));
    assert_eq!(s.view().state, AiLaneState::Blocked);
    // Layer 1 keeps going.
    std::fs::write(s.src.join("d.txt"), "a2@x.example").unwrap();
    let done = s.project.ingest().unwrap().stage_a.done;
    assert_eq!(done, 1);
    // A model comes: Retry runs it.
    s.install(&s.x.clone());
    s.choose("x");
    assert_eq!(s.run().done, 3);
    s.healthy();
}

/// INV-PR-09: a permission revoked mid-run stops the very next call, and no work is lost: allowed
/// again, the lane goes on from there. Needs a private LAN address on this machine.
#[test]
fn inv_pr_09_revoke_mid_run_blocks_and_retry_resumes() {
    let Some(lan) = mk_egress::testkit::private_lan_ip() else {
        eprintln!("no private LAN address: skipped");
        return;
    };
    let files: Vec<(String, String)> = (0..10)
        .map(|i| {
            (
                format!("f{i:02}.txt"),
                format!("Corvin Hale wrote note {i}."),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let s = setup_with_files(Control::default(), &refs);
    let served = serve(
        &s,
        lan,
        finding_everywhere(FakeExtraction {
            delay_ms: 150,
            ..finding(vec![name("Corvin Hale")])
        }),
    );
    let id = add_endpoint(&s, format!("http://{lan}:{}", served.port), "m");
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(id.clone())))
        .unwrap();
    s.ws.schedule_stage_b(&s.project).unwrap();

    // Not allowed for this project yet: nothing sent.
    let out = s.run();
    assert_eq!(out.done, 0);
    assert_eq!(out.blocked.as_deref(), Some(mk_core::NO_MODEL_ALLOWED));

    let prompt = s.ws.prepare_allow(&id).unwrap();
    s.ws.confirm_allow(
        &s.project,
        &id,
        &prompt.destination.origin,
        prompt.destination.class,
    )
    .unwrap();
    let out = std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        wait_until("some items done", || s.view().units_done >= 2);
        s.ws.revoke_endpoint(&s.project, &id).unwrap();
        runner.join().unwrap()
    });
    // The wording depends on where the revoke is seen: before the call (readiness) or at the gate.
    let reason = out.blocked.clone().unwrap_or_default();
    assert!(
        reason == mk_core::NO_MODEL_ALLOWED || reason.contains("is not allowed for this project"),
        "{out:?}"
    );
    assert_eq!(s.view().state, AiLaneState::Blocked);
    let done = out.done;
    assert!(done < 10, "{out:?}");

    s.ws.confirm_allow(
        &s.project,
        &id,
        &prompt.destination.origin,
        prompt.destination.class,
    )
    .unwrap();
    let out = s.run();
    assert_eq!(out.blocked, None);
    assert_eq!(out.done + done, 10);
    assert_eq!(s.ai_mentions().len(), 10);
    s.healthy();
}

/// NFR-REL-12: the local model killed three times with no call answered in between: two restarts,
/// then Failed, the job Blocked with the reason, Stage A going on; Retry finishes the rest, and no
/// finished item is done again.
#[cfg(unix)]
#[test]
fn nfr_rel_12_kill_during_the_lane() {
    let files: Vec<(String, String)> = (0..40)
        .map(|i| {
            (
                format!("f{i:02}.txt"),
                format!("Corvin Hale wrote note {i}."),
            )
        })
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let s = setup_with_files(
        two_models(
            FakeExtraction {
                delay_ms: 400,
                ..finding(vec![name("Corvin Hale")])
            },
            finding(vec![]),
        ),
        &refs,
    );
    s.ws.schedule_stage_b(&s.project).unwrap();
    let summary = std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        let mut last = None;
        for _ in 0..3 {
            let mut pid = None;
            wait_until("a new process", || {
                pid = s.ws.supervisor().status().into_iter().find_map(|x| {
                    let up = matches!(x.state, RuntimeState::Busy | RuntimeState::Ready);
                    x.pid.filter(|p| up && Some(*p) != last)
                });
                pid.is_some() || runner.is_finished()
            });
            let Some(p) = pid else { break };
            last = Some(p);
            let _ = std::process::Command::new("kill")
                .args(["-9", &p.to_string()])
                .status();
        }
        runner.join().unwrap()
    });
    let blocked = summary.blocked.clone().expect("blocked after the crashes");
    assert!(blocked.contains("after 2 restarts"), "{blocked}");
    assert_eq!(s.view().state, AiLaneState::Blocked);
    let done = summary.done;
    // Layer 1 goes on.
    std::fs::write(s.src.join("new.txt"), "fresh@x.example").unwrap();
    assert_eq!(s.project.ingest().unwrap().stage_a.done, 1);
    // Retry: a fresh restart count; Pending items run, Done ones do not.
    s.ws.supervisor().retry();
    let out = s.run();
    assert_eq!(out.blocked, None, "{out:?}");
    assert_eq!(out.done + done, 40, "{out:?}, {done}");
    assert_eq!(s.ai_mentions().len(), 40);
    s.healthy();
}

// ---- NFR-PERF-25, NFR-REL-05, gap A3 ----

/// NFR-PERF-25: 4 in flight on the local model.
#[test]
fn nfr_perf_25_four_slots_locally() {
    let log = tempfile::NamedTempFile::new().unwrap();
    let files: Vec<(String, String)> = (0..12)
        .map(|i| (format!("f{i:02}.txt"), format!("Text {i}.")))
        .collect();
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let mut c = two_models(
        FakeExtraction {
            delay_ms: 150,
            ..FakeExtraction::default()
        },
        finding(vec![]),
    );
    c.in_flight_log = Some(log.path().to_path_buf());
    let s = setup_with_files(c, &refs);
    assert_eq!(s.schedule_and_run().done, 12);
    assert_eq!(std::fs::read_to_string(log.path()).unwrap(), "4");
    assert_eq!(s.ws.supervisor().status()[0].state, RuntimeState::Ready);
}

/// NFR-PERF-25: 1 at a time on an endpoint.
#[test]
fn nfr_perf_25_one_on_an_endpoint() {
    let log = tempfile::NamedTempFile::new().unwrap();
    let s = setup(Control::default());
    let mut control = finding_everywhere(FakeExtraction {
        delay_ms: 100,
        ..FakeExtraction::default()
    });
    control.in_flight_log = Some(log.path().to_path_buf());
    let served = serve(&s, IpAddr::V4(Ipv4Addr::LOCALHOST), control);
    let id = add_endpoint(&s, format!("http://127.0.0.1:{}", served.port), "m");
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(id)))
        .unwrap();
    assert_eq!(s.schedule_and_run().done, 3);
    assert_eq!(std::fs::read_to_string(log.path()).unwrap(), "1");
    assert!(s.ws.supervisor().status().iter().all(|x| x.pid.is_none()));
}

/// NFR-REL-05, INV-PR-04: a pause takes effect while a call hangs. The item is Pending at once, the
/// late reply is dropped, and resume does the item again.
#[test]
fn nfr_rel_05_pause_while_a_call_hangs() {
    let c = two_models(
        FakeExtraction {
            hang_text: Some("Corvin".into()),
            hang_ms: 3_000,
            ..finding(vec![name("Corvin Hale")])
        },
        finding(vec![]),
    );
    let s = setup(c);
    s.ws.schedule_stage_b(&s.project).unwrap();
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        wait_until("a call in flight", || {
            s.view().mentions == 0 && in_progress(&s) > 0
        });
        let at = Instant::now();
        s.project.pause().unwrap();
        assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
        assert_eq!(in_progress(&s), 0);
        assert_eq!(s.view().state, AiLaneState::Paused);
        let out = runner.join().unwrap();
        assert!(out.dropped_replies >= 1, "{out:?}");
        assert_eq!(out.mentions, 0, "{out:?}");
    });
    assert!(s.ai_mentions().is_empty());
    s.healthy();
    s.project.resume().unwrap();
    let out = s.run();
    assert_eq!(out.mentions, 2);
    s.healthy();
}

fn in_progress(s: &Setup) -> u64 {
    s.project.store().ai_progress().unwrap().in_progress
}

/// Gap A3: over the limit nothing starts and the estimate shows; a pick runs, identifiers first;
/// "all" runs the rest.
#[test]
fn a3_over_the_limit_the_user_picks() {
    // 1,700 short documents: over 1,600 units.
    let tmp_files: Vec<(String, String)> = (0..1_700)
        .map(|i| {
            let text = if i == 1_699 {
                "Corvin Hale at c@x.example".to_string()
            } else {
                format!("Note number {i} with some words.")
            };
            (format!("f{i:04}.txt"), text)
        })
        .collect();
    let refs: Vec<(&str, &str)> = tmp_files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let s = setup_with_files(
        two_models(finding(vec![name("Corvin Hale")]), finding(vec![])),
        &refs,
    );
    let scheduled = s.ws.schedule_stage_b(&s.project).unwrap();
    assert_eq!(scheduled.units, 0, "nothing starts by itself");
    assert_eq!(s.run().done, 0);
    let view = s.view();
    assert_eq!(view.state, AiLaneState::WaitingForPick);
    let waiting = view.waiting.unwrap();
    assert_eq!(waiting.units, 1_700);
    assert_eq!(waiting.estimate.as_secs(), 2_073);

    // Pick two documents: the last one, with an identifier, comes first.
    let page = s
        .project
        .documents_page(&Default::default(), None, 2_000)
        .unwrap();
    let last = page.rows.last().unwrap().content_id;
    let first = page.rows[0].content_id;
    let estimate =
        s.ws.stage_b_estimate(&s.project, &UnitScope::Contents(vec![first, last]))
            .unwrap();
    assert_eq!(estimate.units, 2);
    s.ws.run_documents(&s.project, UnitScope::Contents(vec![first, last]))
        .unwrap();
    let order: Vec<i64> = s
        .project
        .store()
        .read_raw(|c| {
            let mut stmt = c.prepare(
                "SELECT w.content_id FROM work_item w JOIN job j ON j.id = w.job_id
                 WHERE j.stage = 'b' ORDER BY w.priority, w.id",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(order, vec![last, first]);
    let out = s.run();
    assert_eq!(out.done, 2);
    assert_eq!(s.ai_mentions().len(), 1);
    let view = s.view();
    assert_eq!(view.waiting.unwrap().units, 1_698);
    assert_eq!(view.state, AiLaneState::WaitingForPick);

    // Scheduling again starts nothing and redoes nothing: still over.
    assert_eq!(s.ws.schedule_stage_b(&s.project).unwrap().units, 0);

    // "All": the rest, with the estimate first.
    let all = s.ws.stage_b_estimate(&s.project, &UnitScope::All).unwrap();
    assert_eq!(all.units, 1_698);
    assert_eq!(
        s.ws.run_documents(&s.project, UnitScope::All)
            .unwrap()
            .units,
        1_698
    );
    assert!(s.view().waiting.is_none());
}

/// Units: adjacent segments are grouped; a long segment becomes windows, and a mention found in a
/// later window has its span in the segment. Spans stay exact in both.
#[test]
fn units_group_segments_and_window_long_ones() {
    let long_line = format!("{} Corvin Hale signed.", "word ".repeat(600));
    let short_lines: String = (0..60).map(|i| format!("line {i}\n")).collect();
    let files = [
        ("long.txt", long_line.as_str()),
        ("short.txt", short_lines.as_str()),
    ];
    let s = setup_with_files(
        two_models(finding(vec![name("Corvin Hale")]), finding(vec![])),
        &files,
    );
    let out = s.schedule_and_run();
    assert_eq!(
        out.done, 3,
        "2 windows of the long line, 1 unit of 60 lines"
    );
    assert_eq!(out.mentions, 1);
    let occ = s.project.occurrences("person:corvin hale").unwrap();
    assert_eq!(occ.len(), 1);
    assert_eq!(occ[0].raw, "Corvin Hale");
    // In the second window, with its place in the whole segment.
    assert!(
        occ[0].locator.span().unwrap().start() >= 2_000,
        "{:?}",
        occ[0].locator
    );
    s.healthy();
}

/// Tables are left out: their names come through column mapping (US-05.3).
#[test]
fn tables_are_left_out_and_a_mapping_keeps_ai_mentions() {
    let files = [
        ("note.txt", "Corvin Hale wrote about Anna Berg."),
        ("people.csv", "name,email\nAnna Berg,anna@x.example\n"),
    ];
    let s = setup_with_files(
        two_models(finding(vec![name("Corvin Hale")]), finding(vec![])),
        &files,
    );
    assert_eq!(s.schedule_and_run().done, 1);
    assert_eq!(s.ai_mentions().len(), 1);
    // Confirming a mapping on another content, and the table step on this one, keep them.
    let rows = s
        .project
        .documents_page(&Default::default(), None, 10)
        .unwrap()
        .rows;
    let csv = rows.iter().find(|r| r.file_type == "csv").unwrap();
    let view = s.project.mapping_view(csv.ref_id).unwrap().unwrap();
    let mut assignments = mk_core::Assignments::default();
    assignments
        .0
        .insert(view.columns[0].key.clone(), mk_core::ColumnType::PersonName);
    s.project.confirm_mapping(csv.ref_id, assignments).unwrap();
    assert_eq!(s.ai_mentions().len(), 1, "INV-AI-04");
    s.healthy();
}

/// INV-AI-04: the table step on a content with both kinds of mention only redoes its own.
#[test]
fn inv_ai_04_a_table_step_keeps_ai_mentions_of_its_content() {
    let s = setup(two_models(hale_and_velmora(), finding(vec![])));
    s.schedule_and_run();
    let before = s.ai_mentions();
    // A settings change re-runs the identifiers, with the table step, on every content.
    s.project.store().begin_identifier_rerun().unwrap();
    s.project.ingest().unwrap();
    assert_eq!(s.ai_mentions(), before);
    s.healthy();
}

/// INV-PR-05: a finished unit is never redone; scheduling twice makes nothing new.
#[test]
fn inv_pr_05_scheduling_again_redoes_nothing() {
    let s = setup(two_models(hale_and_velmora(), finding(vec![])));
    let first = s.schedule_and_run();
    assert_eq!(s.ws.schedule_stage_b(&s.project).unwrap().units, 0);
    assert_eq!(s.run().done, 0);
    assert_eq!(s.view().units_done, first.done);
    // A cancelled run does not restart by itself; the user's "all" does.
    std::fs::write(s.src.join("d.txt"), "Corvin Hale again.").unwrap();
    s.project.ingest().unwrap();
    assert_eq!(s.ws.schedule_stage_b(&s.project).unwrap().units, 1);
    assert_eq!(
        s.ws.cancel_extraction(&s.project).unwrap(),
        mk_core::AiCancelled::Run
    );
    assert_eq!(s.ws.schedule_stage_b(&s.project).unwrap().units, 0);
    assert_eq!(
        s.ws.run_documents(&s.project, UnitScope::All)
            .unwrap()
            .units,
        1
    );
    assert_eq!(s.run().done, 1);
    s.healthy();
}

/// A kill and reopen mid re-run: job states kept, in-progress items back to Pending, the old
/// results still shown, and finishing it switches once.
#[test]
fn adr_0005_reopen_mid_rerun_keeps_old_results_and_finishes() {
    let s = setup(two_models(
        hale_and_velmora(),
        finding(vec![name("Paid"), org("Velmora Port Authority")]),
    ));
    s.schedule_and_run();
    let before = s.project.store().derived_state().unwrap();
    s.choose("y");
    s.ws.start_rerun(&s.project).unwrap();
    // One item in flight when the app dies.
    let item = s.project.store().start_next_ai_item().unwrap().unwrap();
    let dir = s.project.layout().dir().to_path_buf();
    let Setup {
        ws,
        project,
        tmp,
        _hf,
        ..
    } = s;
    drop(project);
    let (project, summary) = Project::open(&dir).unwrap();
    assert_eq!(summary.recovered.items, 1);
    assert_eq!(project.store().derived_state().unwrap(), before);
    // The late reply of the dead attempt does nothing.
    let late = project
        .store()
        .commit_ai_item(item.item, item.attempt, Default::default())
        .unwrap();
    assert!(!late.committed);
    let out = ws.run_stage_b(&project).unwrap();
    assert!(out.done >= 3, "{out:?}");
    assert_eq!(
        project.store().active_ai_run().unwrap().unwrap().model_name,
        "Test y"
    );
    let h = project.check_health().unwrap();
    assert!(h.is_healthy(), "{h:?}");
    drop(tmp);
}

// ---- helpers: an endpoint served by the fake llama-server ----

/// The fake `llama-server` run by hand, as an endpoint would be. Killed on drop.
struct Served {
    child: std::process::Child,
    port: u16,
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

const ENDPOINT_KEY: &str = "endpoint-key";

fn serve(s: &Setup, host: IpAddr, control: Control) -> Served {
    let port = mk_models::runtime::process::free_port().unwrap();
    serve_at(s, host, port, control)
}

fn serve_at(s: &Setup, host: IpAddr, port: u16, control: Control) -> Served {
    let dir = s.tmp.path();
    let control_file = dir.join(format!(
        "served-{}.json",
        s.project.store().path().display().to_string().len()
    ));
    control.write(&control_file);
    let model = dir.join("m.gguf");
    std::fs::write(&model, b"gguf").unwrap();
    let child = std::process::Command::new(FAKE)
        .args(["--host", &host.to_string(), "--port", &port.to_string()])
        .args(["--api-key", ENDPOINT_KEY, "-m"])
        .arg(&model)
        .env(CONTROL_ENV, &control_file)
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    wait_until("the served fake", || {
        std::net::TcpStream::connect((host, port)).is_ok()
    });
    Served { child, port }
}

fn finding_everywhere(rules: FakeExtraction) -> Control {
    Control {
        extraction: HashMap::from([("*".to_string(), rules)]),
        ..Control::default()
    }
}

fn add_endpoint(s: &Setup, url: String, model: &str) -> String {
    s.ws.save_endpoint(EndpointInput {
        id: None,
        name: "Here".into(),
        base_url: url,
        model: model.into(),
        key: KeyChange::Set(ENDPOINT_KEY.to_string().into()),
    })
    .unwrap()
    .id
}
