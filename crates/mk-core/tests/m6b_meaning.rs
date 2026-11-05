//! M6b on [`Workspace`] with the fake `llama-server`: US-06.2 (top 20, each not-ready reason, the
//! endpoint rules), a re-run that never mixes two models (INV-SR-02), keyword search that never
//! waits (INV-SR-04), pause while a call hangs (NFR-REL-05), a stale or damaged index rebuilt, the
//! idle release (ADR-0010) and the start rule.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mk_core::{
    AiChoice, EgressConfig, EndpointInput, FixedProbe, Gpu, GpuVendor, HardwareProfile, KeyChange,
    MeaningOutcome, MeaningState, ModelRole, ModelsConfig, Project, RuntimeConfig, SpawnOptions,
    TlsRoots, UnitScope, WorkerConfig, Workspace,
};
use mk_egress::testkit::{FakeKeyring, ScriptedResolver};
use mk_models::testkit::llama::{CONTROL_ENV, Control, FakeEmbeddings};
use mk_models::testkit::{
    FakeHf, NamedResolver, ORIGIN_HOST, test_bytes, test_catalog, test_model,
};
use mk_models::{GB, Model};
use mk_search::meaning::{TOP_K, WINDOW_CHARS};
use mk_store::{EmbeddingRow, SearchFilter};

const FAKE: &str = env!("CARGO_BIN_EXE_mk-core-fake-llama-server");
const WAIT: Duration = Duration::from_secs(30);
const DIMS: usize = 64;

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

fn control() -> Control {
    Control {
        dims: Some(DIMS),
        ..Control::default()
    }
}

fn rules(rules: FakeEmbeddings) -> Control {
    Control {
        embeddings: HashMap::from([("*".to_string(), rules)]),
        ..control()
    }
}

struct Setup {
    tmp: tempfile::TempDir,
    ws: Workspace,
    project: Project,
    src: PathBuf,
    requests: PathBuf,
    _hf: FakeHf,
}

const FILES: &[(&str, &str)] = &[
    ("a.txt", "Wire transfer to an offshore account in May."),
    ("b.txt", "The harbour dredging contract was signed in June."),
    ("c.txt", "Lunch menu for the staff canteen on Friday."),
    ("d.txt", "Minutes of the procurement committee meeting."),
    ("e.txt", "A note about parking behind the old warehouse."),
];

fn setup(control: Control) -> Setup {
    setup_with_files(control, FILES)
}

fn setup_with_files(mut control: Control, files: &[(&str, &str)]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let requests = tmp.path().join("requests.log");
    control.requests_log = Some(requests.clone());
    let control_file = tmp.path().join("control.json");
    control.write(&control_file);
    let (eb, fb) = (test_bytes(64 * 1024, 1), test_bytes(64 * 1024, 2));
    let e = test_model("e", ModelRole::Embeddings, std::slice::from_ref(&eb));
    let mut f = test_model("f", ModelRole::Embeddings, std::slice::from_ref(&fb));
    if let Some(card) = f.embedding.as_mut() {
        card.query_prefix = "query: ".into();
        card.document_prefix = "passage: ".into();
    }
    let hf = FakeHf::serving(&[
        (&e, std::slice::from_ref(&eb)),
        (&f, std::slice::from_ref(&fb)),
    ]);
    let ws = Workspace::open_with_runtime(
        &tmp.path().join("app"),
        EgressConfig {
            resolver: Arc::new(ScriptedResolver::new()),
            secrets: Some(Arc::new(FakeKeyring::new())),
            roots: TlsRoots::Platform,
            call_timeout: Duration::from_secs(40),
        },
        ModelsConfig {
            models_dir: Some(tmp.path().join("models")),
            catalog: test_catalog(vec![e.clone(), f.clone()]),
            probe: Arc::new(FixedProbe(machine())),
            download: hf.config(NamedResolver::new(&[ORIGIN_HOST])),
        },
        RuntimeConfig {
            path_var: Some(Default::default()),
            spawn: SpawnOptions {
                env: vec![(CONTROL_ENV.into(), control_file.clone().into())],
            },
            ..RuntimeConfig::default()
        },
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
        src,
        requests,
        _hf: hf,
    };
    s.install(&e);
    s.choose("e");
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

    fn install_f(&self) {
        let fb = test_bytes(64 * 1024, 2);
        let mut f = test_model("f", ModelRole::Embeddings, std::slice::from_ref(&fb));
        if let Some(card) = f.embedding.as_mut() {
            card.query_prefix = "query: ".into();
            card.document_prefix = "passage: ".into();
        }
        self.install(&f);
    }

    fn choose(&self, id: &str) {
        self.ws
            .set_ai_choice(ModelRole::Embeddings, Some(AiChoice::Local(id.into())))
            .unwrap();
    }

    fn run(&self) -> mk_core::EmbeddingSummary {
        self.ws.run_embeddings(&self.project).unwrap()
    }

    fn schedule_and_run(&self) -> mk_core::EmbeddingSummary {
        self.ws.schedule_embeddings(&self.project).unwrap();
        self.run()
    }

    fn view(&self) -> mk_core::MeaningView {
        self.ws.meaning_view(&self.project).unwrap()
    }

    fn results(&self, query: &str) -> Vec<mk_core::MeaningHit> {
        match self.ws.meaning_search(&self.project, query).unwrap() {
            MeaningOutcome::Results { hits, .. } => hits,
            other => panic!("not ready: {other:?}"),
        }
    }

    fn not_ready(&self) -> String {
        match self.ws.meaning_search(&self.project, "anything").unwrap() {
            MeaningOutcome::NotReady(why) => why,
            other => panic!("ready: {other:?}"),
        }
    }

    /// INV-SR-04: keyword search does not depend on any of this.
    fn keyword_works(&self) {
        let page = self
            .project
            .search("offshore", &SearchFilter::default(), None, 20)
            .unwrap();
        assert_eq!(page.hits.len(), 1);
    }

    /// Vectors per run, in run order.
    fn vectors(&self) -> Vec<(i64, i64)> {
        self.project
            .store()
            .read_raw(|c| {
                let mut stmt = c.prepare(
                    "SELECT ai_run_id, COUNT(*) FROM segment_embedding
                     GROUP BY ai_run_id ORDER BY ai_run_id",
                )?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap()
    }

    fn in_progress(&self) -> u64 {
        self.project
            .store()
            .embedding_progress()
            .unwrap()
            .in_progress
    }

    fn healthy(&self) {
        let h = self.project.check_health().unwrap();
        assert!(h.is_healthy(), "{h:?}");
    }

    fn index_file(&self) -> PathBuf {
        std::fs::read_dir(self.project.layout().index())
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("vectors-"))
            .expect("an index file")
            .path()
    }

    /// The text of every embedding request so far.
    fn sent(&self) -> Vec<String> {
        std::fs::read_to_string(&self.requests)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .flat_map(|v| match &v["body"]["input"] {
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect::<Vec<_>>(),
                t => t.as_str().map(str::to_string).into_iter().collect(),
            })
            .collect()
    }

    fn egress_purposes(&self) -> Vec<(String, String)> {
        self.project
            .store()
            .read_raw(|c| {
                let mut stmt =
                    c.prepare("SELECT purpose, host FROM egress_record ORDER BY purpose")?;
                let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap()
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn many_files(n: usize) -> Vec<(String, String)> {
    (0..n)
        .map(|i| {
            (
                format!("f{i:03}.txt"),
                format!("Harbour note {i} about dredging and word{i}."),
            )
        })
        .collect()
}

fn refs(files: &[(String, String)]) -> Vec<(&str, &str)> {
    files
        .iter()
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect()
}

// ---- US-06.2 AC1 ----

/// US-06.2 AC1: the top 20, best first, each with a score and a locator; a click opens the window.
#[test]
fn us_06_2_ac1_top_20_with_score_and_locator() {
    let files = many_files(30);
    let s = setup_with_files(control(), &refs(&files));
    let out = s.schedule_and_run();
    assert_eq!(out.done, 30);
    assert_eq!(out.blocked, None);
    assert_eq!(s.view().state, MeaningState::Ready);

    let hits = s.results("note 7 about dredging word7");
    assert_eq!(hits.len(), TOP_K);
    assert!(hits.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(
        hits[0].hit.path.as_deref().unwrap().ends_with("f007.txt"),
        "{:?}",
        hits[0].hit
    );
    assert!(hits[0].score > hits[TOP_K - 1].score, "{hits:?}");
    for h in &hits {
        assert!((-1.01..=1.01).contains(&h.score));
        assert!(h.hit.snippet.iter().any(|run| !run.text.is_empty()));
        // The whole window matched, not some words: nothing is marked.
        assert!(
            h.hit.snippet.iter().all(|run| !run.hit),
            "{:?}",
            h.hit.snippet
        );
        let window = h.hit.hit.as_ref().expect("the window");
        let context = s
            .project
            .segment_context(
                h.hit.segment_id,
                Some((window.span.start(), window.span.end())),
            )
            .unwrap();
        assert!(!format!("{context:?}").is_empty());
        // The same locator keyword search gives for this segment.
        let word = h.hit.path.as_deref().unwrap();
        let word = format!(
            "word{}",
            word.trim_end_matches(".txt")
                .rsplit('f')
                .next()
                .unwrap()
                .parse::<u32>()
                .unwrap()
        );
        let page = s
            .project
            .search(&word, &SearchFilter::default(), None, 20)
            .unwrap();
        assert_eq!(page.hits[0].segment_id, h.hit.segment_id);
        assert_eq!(page.hits[0].locator, h.hit.locator);
    }
    s.healthy();
}

/// Windows are exact spans of the stored text, and one long segment embeds in several requests
/// that commit as one.
#[test]
fn us_06_2_ac1_long_text_windows_are_exact_spans() {
    let long = "harbour dredging ".repeat(WINDOW_CHARS * 2 / 17);
    let s = setup_with_files(control(), &[("long.txt", &long)]);
    let out = s.schedule_and_run();
    assert_eq!((out.done, out.failed), (1, 0));
    assert!(out.windows >= 2, "{out:?}");
    let spans: Vec<(i64, i64, String)> = s
        .project
        .store()
        .read_raw(|c| {
            let mut stmt = c.prepare(
                "SELECT e.char_start, e.char_end, substr(s.text, e.char_start + 1,
                                                         e.char_end - e.char_start)
                 FROM segment_embedding e JOIN segment s ON s.id = e.segment_id ORDER BY e.id",
            )?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(spans.len() as u64, out.windows);
    for (start, end, text) in &spans {
        assert!(end - start <= WINDOW_CHARS as i64);
        assert!(s.sent().contains(text), "{text:?} is what was sent");
    }
}

/// NFR-SEC-14: a card-like number in a result is masked, as in keyword search.
#[test]
fn nfr_sec_14_card_numbers_are_masked_in_results() {
    let s = setup_with_files(
        control(),
        &[(
            "card.txt",
            "Paid with card 4111 1111 1111 1111 at the harbour.",
        )],
    );
    s.schedule_and_run();
    let hits = s.results("paid with card at the harbour");
    let shown: String = hits[0]
        .hit
        .snippet
        .iter()
        .map(|r| r.text.as_str())
        .collect();
    assert!(!shown.contains("4111 1111 1111 1111"), "{shown}");
}

/// The catalog's prefixes reach the model, on stored text and on queries alike.
#[test]
fn us_06_2_ac1_catalog_prefixes_are_used() {
    let s = setup(control());
    s.install_f();
    s.choose("f");
    s.schedule_and_run();
    let sent = s.sent();
    assert!(sent.iter().all(|t| t.starts_with("passage: ")), "{sent:?}");
    s.results("offshore");
    assert!(s.sent().iter().any(|t| t == "query: offshore"));
}

// ---- US-06.2 AC2, INV-SR-04 ----

/// US-06.2 AC2: no model, and nothing run yet: it says why and keyword search works.
#[test]
fn us_06_2_ac2_no_model_then_not_run_yet() {
    let s = setup(control());
    assert_eq!(s.view().state, MeaningState::Idle);
    assert_eq!(s.not_ready(), "Embeddings have not run yet");
    s.keyword_works();

    s.ws.set_ai_choice(ModelRole::Embeddings, None).unwrap();
    s.ws.delete_model("e").unwrap();
    assert_eq!(s.view().state, MeaningState::Blocked);
    assert_eq!(s.not_ready(), mk_core::NO_MODEL_ALLOWED);
    s.keyword_works();

    // Scheduled with no model: Blocked with the reason, Layer 1 goes on (INV-PR-09).
    assert_eq!(s.ws.schedule_embeddings(&s.project).unwrap().units, 5);
    let out = s.run();
    assert_eq!(out.done, 0);
    assert_eq!(out.blocked.as_deref(), Some(mk_core::NO_MODEL_ALLOWED));
    assert_eq!(s.view().state, MeaningState::Blocked);
    assert_eq!(s.not_ready(), mk_core::NO_MODEL_ALLOWED);
    s.keyword_works();
}

/// US-06.2 AC2's own example: "Embeddings: 40% done".
#[test]
fn us_06_2_ac2_says_how_far_it_got() {
    let s = setup(control());
    s.ws.schedule_embeddings(&s.project).unwrap();
    let store = s.project.store();
    let target = mk_store::RunBinding {
        identity: mk_ai::RunIdentity::local("e"),
        model_name: "e".into(),
        destination_host: "127.0.0.1".into(),
        destination_class: "this_machine".into(),
        prompt_version: 0,
    };
    store.bind_embedding_run(target).unwrap();
    for _ in 0..2 {
        let item = store.start_next_embedding_item().unwrap().unwrap();
        let rows = item
            .windows
            .iter()
            .map(|w| EmbeddingRow {
                segment: w.segment,
                char_start: w.char_start,
                char_end: w.char_end,
                vector: vec![0; DIMS * 2],
            })
            .collect();
        assert!(
            store
                .commit_embedding_item(item.item, item.attempt, rows)
                .unwrap()
        );
    }
    let view = s.view();
    assert_eq!(view.state, MeaningState::Running);
    assert_eq!((view.items_done, view.items_total), (2, 5));
    assert_eq!(s.not_ready(), "Embeddings: 40% done");
    s.keyword_works();
}

/// US-06.2 AC2: the index is being made: "Preparing", and a query does not fail.
#[test]
fn us_06_2_ac2_preparing_until_the_index_is_made() {
    let s = setup(control());
    s.schedule_and_run();
    std::fs::remove_file(s.index_file()).unwrap();
    assert_eq!(s.view().state, MeaningState::Preparing);
    assert!(matches!(
        s.ws.meaning_search(&s.project, "offshore").unwrap(),
        MeaningOutcome::Preparing
    ));
    s.keyword_works();
    assert!(s.project.prepare_meaning_index().unwrap());
    assert_eq!(s.view().state, MeaningState::Ready);
    assert!(!s.results("offshore").is_empty());
}

/// The waiting state: over the limit nothing starts, and the tab offers it with an estimate.
#[test]
fn us_06_2_ac2_over_the_limit_waits_for_a_click() {
    // One CSV with a row per segment: over 480,000 windows of one short field.
    let tmp_rows: String = (0..=mk_jobs::EMBEDDING_AUTO_WINDOWS)
        .map(|_| "a\n")
        .collect();
    let s = setup_with_files(control(), &[("rows.csv", &tmp_rows)]);
    let out = s.ws.schedule_embeddings(&s.project).unwrap();
    assert_eq!(out.units, 0, "nothing starts by itself");
    assert_eq!(s.run().done, 0);
    let view = s.view();
    assert_eq!(view.state, MeaningState::WaitingForStart);
    let waiting = view.waiting.unwrap();
    assert!(waiting.windows > mk_jobs::EMBEDDING_AUTO_WINDOWS);
    assert!(waiting.estimate >= Duration::from_secs(20 * 60));
    assert!(s.not_ready().contains("about 20 min"), "{}", s.not_ready());
    assert_eq!(
        s.ws.embedding_estimate(&s.project).unwrap().windows,
        waiting.windows
    );
    // The click schedules it all, and the waiting count goes.
    assert!(s.ws.start_embeddings(&s.project).unwrap().units > 0);
    assert_eq!(s.view().waiting, None);
    assert_eq!(s.view().state, MeaningState::Running);
    s.project.cancel().unwrap();
}

/// The rule's other side: a project under the limit starts by itself, and scheduling again redoes
/// nothing (INV-PR-05).
#[test]
fn inv_pr_05_scheduling_again_redoes_nothing() {
    let s = setup(control());
    assert_eq!(s.ws.schedule_embeddings(&s.project).unwrap().units, 5);
    s.run();
    let before = s.sent().len();
    assert_eq!(s.ws.schedule_embeddings(&s.project).unwrap().units, 0);
    s.run();
    assert_eq!(s.sent().len(), before);
    assert_eq!(s.ws.embedding_estimate(&s.project).unwrap().windows, 0);
}

// ---- US-06.2 AC3 ----

/// US-06.2 AC3: a local-network endpoint not allowed for the project Blocks and nothing is sent;
/// once allowed the destination shows, and a query is in the egress history as `search_query`
/// (INV-SR-03).
#[test]
fn us_06_2_ac3_endpoint_rules() {
    let Some(lan) = mk_egress::testkit::private_lan_ip() else {
        eprintln!("no private LAN address: skipped");
        return;
    };
    let s = setup(control());
    let served = serve(&s, lan);
    let id =
        s.ws.save_endpoint(EndpointInput {
            id: None,
            name: "Lab".into(),
            base_url: format!("http://{lan}:{}", served.port),
            model: "m".into(),
            key: KeyChange::Set(ENDPOINT_KEY.to_string().into()),
        })
        .unwrap()
        .id;
    s.ws.set_ai_choice(ModelRole::Embeddings, Some(AiChoice::Endpoint(id.clone())))
        .unwrap();
    s.ws.schedule_embeddings(&s.project).unwrap();

    // Not allowed for this project: nothing sent, and it says so.
    let out = s.run();
    assert_eq!(out.done, 0);
    assert_eq!(out.blocked.as_deref(), Some(mk_core::NO_MODEL_ALLOWED));
    assert_eq!(s.view().state, MeaningState::Blocked);
    assert_eq!(s.not_ready(), mk_core::NO_MODEL_ALLOWED);
    assert!(s.egress_purposes().is_empty());
    s.keyword_works();

    let prompt = s.ws.prepare_allow(&id).unwrap();
    s.ws.confirm_allow(
        &s.project,
        &id,
        &prompt.destination.origin,
        prompt.destination.class,
    )
    .unwrap();
    let out = s.run();
    assert_eq!((out.done, out.blocked), (5, None));
    let view = s.view();
    assert_eq!(view.state, MeaningState::Ready);
    let shown = view.destination.unwrap();
    assert!(
        shown.contains(&lan.to_string()) && shown.contains("local network"),
        "{shown}"
    );

    assert!(!s.egress_purposes().iter().any(|(p, _)| p == "search_query"));
    assert!(!s.results("offshore").is_empty());
    let history = s.egress_purposes();
    assert!(
        history.iter().any(|(p, _)| p == "search_query"),
        "{history:?}"
    );
    assert!(
        history.iter().any(|(p, _)| p == "embeddings"),
        "{history:?}"
    );
    assert!(
        history.iter().all(|(_, h)| *h == lan.to_string()),
        "{history:?}"
    );

    // Revoked: a query is refused before anything goes out, and keyword search goes on.
    s.ws.revoke_endpoint(&s.project, &id).unwrap();
    let sent = s.egress_purposes().len();
    assert_eq!(s.not_ready(), mk_core::NO_MODEL_ALLOWED);
    assert_eq!(s.egress_purposes().len(), sent);
    s.keyword_works();
}

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

fn serve(s: &Setup, host: std::net::IpAddr) -> Served {
    let port = mk_models::runtime::process::free_port().unwrap();
    let dir = s.tmp.path();
    let control_file = dir.join("served.json");
    control().write(&control_file);
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

// ---- INV-SR-02, ADR-0005 ----

/// INV-SR-02: changing the model holds new work back and the old vectors stay; the tab offers the
/// re-run or the old model. Choosing it again makes it ready as it was.
#[test]
fn inv_sr_02_model_changed_is_not_searched_with_the_other_model() {
    let s = setup(control());
    s.install_f();
    s.schedule_and_run();
    let vectors = s.vectors();
    let e_hits: Vec<i64> = s
        .results("offshore")
        .iter()
        .map(|h| h.hit.segment_id)
        .collect();

    s.choose("f");
    let view = s.view();
    assert_eq!(view.state, MeaningState::ModelChanged);
    assert_eq!(view.vectors_model.as_deref(), Some("Test e"));
    assert_eq!(view.rerun_with.as_deref(), Some("Test f"));
    let requests = s.sent().len();
    assert_eq!(
        s.not_ready(),
        "Vectors come from Test e. Re-run with Test f, or choose Test e again."
    );
    assert_eq!(s.sent().len(), requests, "no query went to the other model");

    // New work waits, Blocked, and adds nothing of f's to e's vectors.
    std::fs::write(s.src.join("new.txt"), "A brand new note.").unwrap();
    s.project.ingest().unwrap();
    assert_eq!(s.ws.schedule_embeddings(&s.project).unwrap().units, 1);
    let out = s.run();
    assert_eq!(out.done, 0);
    assert!(out.blocked.unwrap().contains("Vectors come from"));
    assert_eq!(s.vectors(), vectors);
    assert_eq!(s.view().state, MeaningState::Blocked);
    s.keyword_works();

    s.choose("e");
    let out = s.run();
    assert_eq!((out.done, out.blocked), (1, None));
    assert_eq!(s.view().state, MeaningState::Ready);
    let again: Vec<i64> = s
        .results("offshore")
        .iter()
        .map(|h| h.hit.segment_id)
        .collect();
    assert_eq!(again[0], e_hits[0]);
    s.healthy();
}

/// INV-SR-02, ADR-0005: a re-run's vectors wait beside the old ones, a query is not ready while it
/// runs, and the switch leaves only the new model's. The index follows.
#[test]
fn inv_sr_02_rerun_switches_whole() {
    let s = setup(rules(FakeEmbeddings {
        delay_ms: 30,
        ..FakeEmbeddings::default()
    }));
    s.install_f();
    s.schedule_and_run();
    let old = s.vectors();
    assert_eq!(old.len(), 1);
    let before: Vec<f32> = s.results("offshore").iter().map(|h| h.score).collect();

    s.choose("f");
    let estimate = s.ws.start_embedding_rerun(&s.project).unwrap();
    assert_eq!(estimate.windows, old[0].1 as u64);
    let view = s.view();
    assert_eq!(view.state, MeaningState::Running);
    assert_eq!(view.vectors_model.as_deref(), Some("Test e"));
    assert_eq!(view.rerun_model.as_deref(), Some("Test f"));
    assert_eq!(view.rerun_with, None);
    assert!(s.not_ready().contains("Re-running embeddings with Test f"));

    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        wait_until("part of the re-run", || {
            s.project.store().embedding_progress().unwrap().done >= 2
        });
        // Both models' vectors are stored; each stays under its own run.
        assert!(s.vectors().len() == 2, "{:?}", s.vectors());
        assert!(s.not_ready().contains("Re-running"));
        s.keyword_works();
        s.healthy();
        assert_eq!(runner.join().unwrap().blocked, None);
    });

    let new = s.vectors();
    assert_eq!(new.len(), 1, "{new:?}");
    assert_ne!(new[0].0, old[0].0);
    assert_eq!(new[0].1, old[0].1);
    let view = s.view();
    assert_eq!(view.state, MeaningState::Ready);
    assert_eq!(view.vectors_model.as_deref(), Some("Test f"));
    assert_eq!(view.rerun_model, None);
    // The index is the new run's: its file name names the model, and the query uses it.
    assert!(s.index_file().to_string_lossy().contains("local_f"));
    let after: Vec<f32> = s.results("offshore").iter().map(|h| h.score).collect();
    assert_ne!(after, before);
    s.healthy();
}

/// INV-SR-02: with a re-run's vectors stored beside the active run's, an index made now holds the
/// active run's alone, and rescoring for that run cannot read the other's.
#[test]
fn inv_sr_02_index_and_rescoring_read_the_active_run_only() {
    let s = setup(control());
    s.install_f();
    s.schedule_and_run();
    let index = std::fs::read(s.index_file()).unwrap();

    s.choose("f");
    s.ws.start_embedding_rerun(&s.project).unwrap();
    let store = s.project.store();
    let item = store.start_next_embedding_item().unwrap().unwrap();
    let rows = item
        .windows
        .iter()
        .map(|w| EmbeddingRow {
            segment: w.segment,
            char_start: w.char_start,
            char_end: w.char_end,
            vector: vec![0; DIMS * 2],
        })
        .collect();
    assert!(
        store
            .commit_embedding_item(item.item, item.attempt, rows)
            .unwrap()
    );
    assert_eq!(s.vectors().len(), 2, "{:?}", s.vectors());

    std::fs::remove_file(s.index_file()).unwrap();
    assert!(s.project.prepare_meaning_index().unwrap());
    assert_eq!(std::fs::read(s.index_file()).unwrap(), index);

    let active = store.active_embedding_run().unwrap().unwrap().id;
    let pending = store.pending_embedding_run().unwrap().unwrap().id;
    let ids: Vec<i64> = store
        .read_raw(|c| {
            let mut stmt = c.prepare("SELECT id FROM segment_embedding WHERE ai_run_id = ?1")?;
            let rows = stmt.query_map([pending], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert!(!ids.is_empty());
    assert_eq!(
        store.load_embedding_vectors(pending, &ids).unwrap().len(),
        ids.len()
    );
    assert!(
        store
            .load_embedding_vectors(active, &ids)
            .unwrap()
            .is_empty()
    );
}

/// ADR-0005: cancel of a re-run changes nothing: the old vectors, the old index, ready.
#[test]
fn inv_sr_02_cancel_of_a_rerun_changes_nothing() {
    let s = setup(control());
    s.install_f();
    s.schedule_and_run();
    let vectors = s.vectors();
    let index = std::fs::read(s.index_file()).unwrap();

    s.choose("f");
    s.ws.start_embedding_rerun(&s.project).unwrap();
    assert_eq!(
        s.ws.cancel_embeddings(&s.project).unwrap(),
        mk_core::AiCancelled::Rerun
    );
    assert_eq!(s.vectors(), vectors);
    assert_eq!(std::fs::read(s.index_file()).unwrap(), index);
    assert_eq!(s.view().state, MeaningState::ModelChanged);

    s.choose("e");
    assert_eq!(s.view().state, MeaningState::Ready);
    assert!(!s.results("offshore").is_empty());
    s.healthy();
}

/// A re-run cancelled at once, the old model chosen again, a query straight after: the query gets
/// its answer whatever state the model's start is in.
#[test]
fn inv_sr_02_query_right_after_a_cancelled_rerun_gets_an_answer() {
    let mut control = control();
    control.start_delay_ms = 300;
    let s = setup(control);
    s.install_f();
    s.schedule_and_run();
    assert!(!s.results("offshore").is_empty());
    s.choose("f");
    s.ws.start_embedding_rerun(&s.project).unwrap();
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        wait_until("the new model's start", || {
            s.ws.runtime_status()
                .models
                .iter()
                .any(|r| r.model_id == "f")
        });
        s.ws.cancel_embeddings(&s.project).unwrap();
        s.choose("e");
        let t = Instant::now();
        let hits = std::thread::scope(|inner| {
            let a = inner.spawn(|| s.results("offshore"));
            let b = inner.spawn(|| s.results("harbour"));
            (a.join().unwrap(), b.join().unwrap())
        });
        assert!(!hits.0.is_empty() && !hits.1.is_empty());
        assert!(t.elapsed() < Duration::from_secs(20), "{:?}", t.elapsed());
        runner.join().unwrap();
    });
    s.healthy();
}

/// The queue's Cancel drops a re-run too (US-03.3, ADR-0005).
#[test]
fn adr_0005_queue_cancel_drops_the_rerun() {
    let s = setup(control());
    s.install_f();
    s.schedule_and_run();
    let vectors = s.vectors();
    s.choose("f");
    s.ws.start_embedding_rerun(&s.project).unwrap();
    s.project.cancel().unwrap();
    assert_eq!(s.vectors(), vectors);
    assert_eq!(s.project.store().pending_embedding_run().unwrap(), None);
    s.healthy();
}

/// A re-run needs something to re-run, and another model.
#[test]
fn rerun_refusals() {
    let s = setup(control());
    let none = s.ws.start_embedding_rerun(&s.project).unwrap_err();
    assert!(none.to_string().contains("nothing to re-run"), "{none}");
    s.schedule_and_run();
    let same = s.ws.start_embedding_rerun(&s.project).unwrap_err();
    assert!(same.to_string().contains("already come from"), "{same}");
    assert_eq!(s.view().state, MeaningState::Ready);
}

/// A first run cancelled before any vector leaves no model behind: another model can start.
#[test]
fn cancel_before_any_vector_leaves_no_run() {
    let s = setup(control());
    s.install_f();
    s.ws.schedule_embeddings(&s.project).unwrap();
    s.project
        .store()
        .bind_embedding_run(mk_store::RunBinding {
            identity: mk_ai::RunIdentity::local("e"),
            model_name: "Test e".into(),
            destination_host: "127.0.0.1".into(),
            destination_class: "this_machine".into(),
            prompt_version: 0,
        })
        .unwrap();
    s.ws.cancel_embeddings(&s.project).unwrap();
    assert_eq!(s.project.store().active_embedding_run().unwrap(), None);
    s.choose("f");
    assert_eq!(s.ws.start_embeddings(&s.project).unwrap().units, 5);
    assert_eq!(s.run().blocked, None);
    assert_eq!(s.view().vectors_model.as_deref(), Some("Test f"));
    s.healthy();
}

/// Vectors of two sizes never share a run: the item that differs fails, the rest stay.
#[test]
fn inv_sr_02_a_size_change_fails_that_item() {
    let s = setup(control());
    s.schedule_and_run();
    std::fs::write(s.src.join("new.txt"), "A brand new note.").unwrap();
    s.project.ingest().unwrap();
    s.ws.schedule_embeddings(&s.project).unwrap();
    let store = s.project.store();
    let item = store.start_next_embedding_item().unwrap().unwrap();
    let rows = item
        .windows
        .iter()
        .map(|w| EmbeddingRow {
            segment: w.segment,
            char_start: w.char_start,
            char_end: w.char_end,
            vector: vec![0; (DIMS + 1) * 2],
        })
        .collect();
    let err = store
        .commit_embedding_item(item.item, item.attempt, rows)
        .unwrap_err();
    assert!(err.to_string().contains("dimensions"), "{err}");
}

/// `check_health` finds vectors outside the active and pending runs.
#[test]
fn inv_sr_02_health_finds_vectors_of_a_superseded_run() {
    let s = setup(control());
    s.schedule_and_run();
    s.healthy();
    s.project
        .store()
        .write_raw(|c| {
            c.execute(
                "UPDATE ai_run SET state = 'superseded' WHERE stage = 'embeddings'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let h = s.project.check_health().unwrap();
    assert_eq!(h.mixed_ai_runs, 5, "{h:?}");
    assert!(!h.is_healthy());
}

// ---- The lane ----

/// NFR-REL-05, INV-PR-04: pause while a call hangs returns at once, the item leaves In progress, and
/// the vectors that come later are dropped. Resume finishes with nothing lost.
#[test]
fn nfr_rel_05_pause_while_a_call_hangs() {
    let s = setup(rules(FakeEmbeddings {
        hang_text: Some("offshore".into()),
        hang_ms: 2_000,
        ..FakeEmbeddings::default()
    }));
    s.ws.schedule_embeddings(&s.project).unwrap();
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        // The request is logged before it hangs, so this is the hanging call, whatever came first.
        wait_until("the hanging call", || {
            s.sent().iter().any(|t| t.contains("offshore"))
        });
        let at = Instant::now();
        s.project.pause().unwrap();
        assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
        assert_eq!(s.in_progress(), 0);
        assert_eq!(s.view().state, MeaningState::Paused);
        assert!(s.not_ready().starts_with("Embeddings paused at"));
        s.keyword_works();
        let out = runner.join().unwrap();
        assert_eq!(out.dropped_replies, 1, "{out:?}");
        // Only the items finished before it have vectors: the dropped one wrote none.
        let stored: i64 = s.vectors().iter().map(|v| v.1).sum();
        assert_eq!(stored as u64, out.windows, "{out:?}");
        assert!(out.done < 5, "{out:?}");
    });
    s.healthy();
    s.project.resume().unwrap();
    let store = s.project.store();
    s.run();
    assert_eq!(store.embedding_progress().unwrap().done, 5);
    let stored: i64 = s.vectors().iter().map(|v| v.1).sum();
    assert_eq!(stored, 5, "each window once");
    assert_eq!(s.view().state, MeaningState::Ready);
    s.healthy();
}

/// ADR-0006: one request in flight on the embeddings lane.
#[test]
fn adr_0006_one_request_in_flight() {
    let tmp = tempfile::tempdir().unwrap();
    let log = tmp.path().join("in_flight.log");
    let s = setup(Control {
        in_flight_log: Some(log.clone()),
        ..rules(FakeEmbeddings {
            delay_ms: 20,
            ..FakeEmbeddings::default()
        })
    });
    assert_eq!(s.schedule_and_run().done, 5);
    assert_eq!(std::fs::read_to_string(log).unwrap(), "1");
}

/// ADR-0006: an HTTP error fails that item alone, and an unreadable reply too; a search still runs
/// over the rest, and "start" runs the failed one again.
#[test]
fn adr_0006_an_http_error_fails_that_item_alone() {
    let s = setup(rules(FakeEmbeddings {
        error_text: Some("canteen".into()),
        ..FakeEmbeddings::default()
    }));
    let out = s.schedule_and_run();
    assert_eq!((out.done, out.failed), (4, 1), "{out:?}");
    let view = s.view();
    assert_eq!(view.state, MeaningState::Ready);
    assert_eq!(view.items_failed, 1);
    assert!(!s.results("offshore").is_empty());
    s.healthy();
}

/// A reply with too few vectors is a failed item, never a shifted one.
#[test]
fn adr_0006_too_few_vectors_fail_the_item() {
    let s = setup(rules(FakeEmbeddings {
        short_reply: true,
        ..FakeEmbeddings::default()
    }));
    let out = s.schedule_and_run();
    assert_eq!((out.done, out.failed), (0, 5), "{out:?}");
    assert!(s.vectors().is_empty());
    assert_eq!(s.view().state, MeaningState::Idle);
}

/// ADR-0006: an endpoint that stops answering: three network failures in a row Block the job and
/// nothing is lost; it goes on when it answers again.
#[test]
fn adr_0006_three_network_failures_block_and_lose_nothing() {
    let s = setup(control());
    let id =
        s.ws.save_endpoint(EndpointInput {
            id: None,
            name: "Gone".into(),
            base_url: format!(
                "http://127.0.0.1:{}",
                mk_models::runtime::process::free_port().unwrap()
            ),
            model: "m".into(),
            key: KeyChange::Set(ENDPOINT_KEY.to_string().into()),
        })
        .unwrap()
        .id;
    s.ws.set_ai_choice(ModelRole::Embeddings, Some(AiChoice::Endpoint(id)))
        .unwrap();
    let out = s.schedule_and_run();
    assert_eq!(out.done, 0);
    assert!(out.blocked.is_some(), "{out:?}");
    assert_eq!(s.view().state, MeaningState::Blocked);
    assert_eq!(s.in_progress(), 0);
    assert_eq!(s.project.store().embedding_progress().unwrap().open, 5);
    s.keyword_works();
    s.healthy();
}

/// INV-SR-02 per item: a model changed during a call: its vectors commit under the run they were
/// asked for, the next item is released and the job Blocks. Nothing goes to the new model.
#[test]
fn run_identity_change_during_the_lane_blocks_instead_of_mixing() {
    let s = setup(rules(FakeEmbeddings {
        delay_ms: 150,
        ..FakeEmbeddings::default()
    }));
    s.install_f();
    s.ws.schedule_embeddings(&s.project).unwrap();
    let out = std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.run());
        wait_until("a call in flight", || s.in_progress() > 0);
        s.choose("f");
        runner.join().unwrap()
    });
    assert!(out.done < 5, "{out:?}");
    assert_eq!(s.view().state, MeaningState::Blocked);
    assert_eq!(s.vectors().len(), 1);
    assert!(!s.sent().iter().any(|t| t.starts_with("passage: ")));
    s.healthy();
}

/// Layer 1's changes: a removed document leaves the results and the index is made again.
#[test]
fn inv_sr_02_removed_document_leaves_the_results() {
    let s = setup(control());
    s.schedule_and_run();
    assert!(
        s.results("offshore")
            .iter()
            .any(|h| h.hit.path.as_deref().unwrap().ends_with("a.txt"))
    );
    let rows = s
        .project
        .documents_page(&Default::default(), None, 100)
        .unwrap();
    let row = rows
        .rows
        .iter()
        .find(|r| r.rel_path.ends_with("a.txt"))
        .unwrap();
    s.project.remove_files(&[row.ref_id]).unwrap();
    assert_eq!(s.view().state, MeaningState::Preparing);
    assert!(s.project.prepare_meaning_index().unwrap());
    let hits = s.results("offshore");
    assert_eq!(hits.len(), 4);
    assert!(
        !hits
            .iter()
            .any(|h| h.hit.path.as_deref().unwrap().ends_with("a.txt"))
    );
    s.healthy();
}

/// ADR-0010: a query that got past the status just before the vectors changed still does not load
/// the old index, from memory or from its file.
#[test]
fn adr_0010_a_file_from_before_a_change_is_not_loaded() {
    let s = setup(control());
    s.schedule_and_run();
    assert!(s.project.meaning_index_loads().unwrap());
    assert!(s.project.meaning_index_in_memory());
    let rows = s
        .project
        .documents_page(&Default::default(), None, 100)
        .unwrap();
    let row = rows
        .rows
        .iter()
        .find(|r| r.rel_path.ends_with("a.txt"))
        .unwrap();
    s.project.remove_files(&[row.ref_id]).unwrap();
    assert!(s.index_file().exists());
    assert!(!s.project.meaning_index_loads().unwrap());
    assert!(!s.project.meaning_index_in_memory());
    assert!(s.project.prepare_meaning_index().unwrap());
    assert!(s.project.meaning_index_loads().unwrap());
}

/// ADR-0010: the index is derived: a damaged file is removed and made again.
#[test]
fn adr_0010_a_damaged_index_is_rebuilt() {
    let s = setup(control());
    s.schedule_and_run();
    let path = s.index_file();
    let good = std::fs::read(&path).unwrap();
    let mut bad = good.clone();
    let last = bad.len() - 1;
    bad[last] ^= 0xff;
    std::fs::write(&path, &bad).unwrap();
    assert!(matches!(
        s.ws.meaning_search(&s.project, "offshore").unwrap(),
        MeaningOutcome::Preparing
    ));
    assert!(!path.exists(), "the damaged file went");
    assert!(s.project.prepare_meaning_index().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), good);
    assert!(!s.results("offshore").is_empty());

    // A cut-off file is not trusted either, once the index is no longer in memory.
    s.project.set_meaning_idle(Duration::from_millis(5));
    wait_until("the release", || !s.project.meaning_index_in_memory());
    std::fs::write(&path, &good[..good.len() / 2]).unwrap();
    assert_eq!(s.view().state, MeaningState::Preparing);
    assert!(s.project.prepare_meaning_index().unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), good);
}

/// ADR-0010, NFR-RES-01: the index goes from memory after its idle time, and a query loads it again.
#[test]
fn adr_0010_idle_release() {
    let s = setup(control());
    s.schedule_and_run();
    assert!(!s.project.meaning_index_in_memory());
    s.results("offshore");
    assert!(s.project.meaning_index_in_memory());
    s.project.set_meaning_idle(Duration::from_millis(30));
    wait_until("the release", || !s.project.meaning_index_in_memory());
    assert_eq!(s.view().state, MeaningState::Ready);
    s.results("offshore");
    assert!(s.project.meaning_index_in_memory());
}

/// An empty query is the caller's mistake, in plain words; it reaches no model.
#[test]
fn an_empty_query_is_refused() {
    let s = setup(control());
    s.schedule_and_run();
    let before = s.sent().len();
    let e = s.ws.meaning_search(&s.project, "   ").unwrap_err();
    assert_eq!(e.code, "empty_query");
    assert_eq!(s.sent().len(), before);
}

/// `All` runs failed items again; the first look doesn't.
#[test]
fn start_runs_failed_items_again() {
    let s = setup(rules(FakeEmbeddings {
        error_text: Some("canteen".into()),
        ..FakeEmbeddings::default()
    }));
    s.schedule_and_run();
    assert_eq!(s.ws.schedule_embeddings(&s.project).unwrap().units, 0);
    assert_eq!(
        s.project
            .store()
            .count_embedding_windows(&UnitScope::All)
            .unwrap(),
        1
    );
    assert_eq!(s.ws.start_embeddings(&s.project).unwrap().units, 1);
}
