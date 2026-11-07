//! M7a on [`Workspace`] with the fake `llama-server`: merge, reject, undo, split and edit as logged
//! decisions that survive every re-run (US-11.4, US-11.5, US-12.2, INV-ER-01..08, INV-CU-01..06), and
//! scored merge proposals that are never applied on their own (US-11.2, D6).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use mk_core::{
    AiChoice, Assignments, ColumnType, ConnectionQuery, DecisionKind, DecisionRow, DecisionStatus,
    DocumentFilter, EgressConfig, EntityRow, EntityType, FixedProbe, Gpu, GpuVendor,
    HardwareProfile, MergeEdit, ModelRole, ModelsConfig, PROPOSAL_THRESHOLD, Project,
    ProposalCursor, ProposalRow, RuntimeConfig, SpawnOptions, TlsRoots, WorkerConfig, Workspace,
};
use mk_egress::testkit::{FakeKeyring, ScriptedResolver};
use mk_models::GB;
use mk_models::testkit::llama::{CONTROL_ENV, Control, FakeExtraction, FakeName};
use mk_models::testkit::{
    FakeHf, NamedResolver, ORIGIN_HOST, test_bytes, test_catalog, test_model,
};

const FAKE: &str = env!("CARGO_BIN_EXE_mk-core-fake-llama-server");
const WAIT: Duration = Duration::from_secs(30);

const CORVIN: &str = "person:corvin hale";
const C_HALE: &str = "person:c hale";
const MR_HALE: &str = "person:mr hale";
const GREGOR: &str = "person:gregor almqvist";
const MR_ALMQVIST: &str = "person:mr almqvist";
const PORT: &str = "org:velmora port authority";

/// Corvin Hale has three written forms over the first four files; Gregor and Mr Almqvist are proposed,
/// and the ground truth does not say they are one.
const FILES: &[(&str, &str)] = &[
    (
        "a.txt",
        "Paid by Corvin Hale on 2 May.\nVelmora Port Authority signed the invoice.",
    ),
    (
        "b.txt",
        "C. Hale wrote to Velmora Port Authority.\nCorvin Hale replied that day.",
    ),
    (
        "c.txt",
        "Corvin Hale called once more.\nGregor Almqvist visited the yard.",
    ),
    (
        "d.txt",
        "Mr Almqvist visited the quay.\nC. Hale was there.\nMr Hale paid.",
    ),
    ("e.txt", "Nothing shared here at all."),
];

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

fn person(n: &str) -> FakeName {
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

fn names() -> Vec<FakeName> {
    vec![
        person("Corvin Hale"),
        person("C. Hale"),
        person("Mr Hale"),
        person("Gregor Almqvist"),
        person("Mr Almqvist"),
        org("Velmora Port Authority"),
    ]
}

/// `(seq, kind, anchor, before, after, decided_at)`.
type RawDecision = (i64, String, String, Option<String>, Option<String>, String);

struct Setup {
    tmp: tempfile::TempDir,
    ws: Workspace,
    project: Project,
    src: PathBuf,
    _hf: FakeHf,
}

/// A project over `files`, Stage A done, with models `x` and `y` that find `x` and `y`.
fn setup_with(x_names: FakeExtraction, y_names: FakeExtraction, files: &[(&str, &str)]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let control_file = tmp.path().join("control.json");
    Control {
        extraction: HashMap::from([
            ("x.gguf".to_string(), x_names),
            ("y.gguf".to_string(), y_names),
        ]),
        ..Control::default()
    }
    .write(&control_file);
    let (xb, yb) = (test_bytes(64 * 1024, 1), test_bytes(64 * 1024, 2));
    let x = test_model("x", ModelRole::Extraction, std::slice::from_ref(&xb));
    let y = test_model("y", ModelRole::Extraction, std::slice::from_ref(&yb));
    let hf = FakeHf::serving(&[
        (&x, std::slice::from_ref(&xb)),
        (&y, std::slice::from_ref(&yb)),
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
            catalog: test_catalog(vec![x.clone(), y.clone()]),
            probe: Arc::new(FixedProbe(machine())),
            download: hf.config(NamedResolver::new(&[ORIGIN_HOST])),
        },
        RuntimeConfig {
            path_var: Some(Default::default()),
            spawn: SpawnOptions {
                env: vec![(CONTROL_ENV.into(), control_file.into())],
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
    for m in [&x, &y] {
        ws.start_download(&m.id).unwrap();
        wait_until("installed", || {
            ws.model_catalog()
                .unwrap()
                .iter()
                .any(|v| v.id == m.id && v.state == mk_core::DownloadState::Installed)
        });
    }
    ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("x".into())))
        .unwrap();
    project.ingest().unwrap();
    Setup {
        tmp,
        ws,
        project,
        src,
        _hf: hf,
    }
}

fn finding(names: Vec<FakeName>) -> FakeExtraction {
    FakeExtraction {
        names,
        ..FakeExtraction::default()
    }
}

/// A project with [`FILES`] and the names found.
fn setup() -> Setup {
    let s = setup_with(finding(names()), finding(names()), FILES);
    s.extract();
    s
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

impl Setup {
    fn extract(&self) {
        self.ws.schedule_stage_b(&self.project).unwrap();
        let run = self.ws.run_stage_b(&self.project).unwrap();
        assert_eq!(run.failed, 0);
    }

    /// Re-reads everything with model `y`, as a model change does.
    fn rerun_with_y(&self) {
        self.ws
            .set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("y".into())))
            .unwrap();
        self.ws.start_rerun(&self.project).unwrap();
        let run = self.ws.run_stage_b(&self.project).unwrap();
        assert_eq!(run.failed, 0);
        let active = self.project.store().active_ai_run().unwrap().unwrap();
        assert_eq!(active.model_name, "Test y");
    }

    fn rescan(&self) {
        self.project.rescan(false).unwrap();
        self.project.run_stage_a().unwrap();
    }

    /// The file goes away and a re-scan sees it (its mentions stay, Missing).
    fn hide(&self, file: &str) {
        std::fs::rename(self.src.join(file), self.tmp.path().join(file)).unwrap();
        self.rescan();
    }

    fn restore(&self, file: &str) {
        std::fs::rename(self.tmp.path().join(file), self.src.join(file)).unwrap();
        self.rescan();
    }

    /// The file's own data goes from the project (its entities can end).
    fn remove(&self, file: &str) {
        let page = self
            .project
            .documents_page(&DocumentFilter::default(), None, 100)
            .unwrap();
        let row = page.rows.iter().find(|r| r.rel_path == file).unwrap();
        self.project.remove_files(&[row.ref_id]).unwrap();
    }

    fn write(&self, sql: &'static str) -> Result<(), mk_store::StoreError> {
        self.project
            .store()
            .write_raw(|c| Ok(c.execute_batch(sql)?))
    }

    /// Closes and opens the project.
    fn reopen(&mut self) {
        let (mut project, _) = Project::open(&self.tmp.path().join("Case")).unwrap();
        *project.worker_config_mut() =
            WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
        self.project = project;
    }

    fn entity(&self, key: &str) -> Option<EntityRow> {
        self.project
            .store()
            .derived_state()
            .unwrap()
            .entities
            .get(key)
            .cloned()
    }

    fn connection_keys(&self) -> BTreeSet<String> {
        self.project
            .connections_page(&ConnectionQuery::default(), None, 100)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.row.subject_key.clone())
            .collect()
    }

    fn proposals(&self) -> Vec<ProposalRow> {
        self.project.refresh_proposals().unwrap();
        self.project.proposals(None, 100).unwrap()
    }

    fn proposed(&self, a: &str, b: &str) -> bool {
        self.proposals()
            .iter()
            .any(|p| (p.a == a && p.b == b) || (p.a == b && p.b == a))
    }

    fn accept(&self, a: &str, b: &str) -> i64 {
        self.project.refresh_proposals().unwrap();
        self.project
            .accept_proposal(a, b, MergeEdit::default())
            .unwrap()
    }

    /// Accepts the proposal between the entities holding `x` and `y`: the pair it is anchored on is
    /// the best-scoring of their names.
    fn accept_between(&self, x: &str, y: &str) -> i64 {
        let names = |key: &str| self.project.entity_names(key).unwrap().unwrap();
        let (nx, ny) = (names(x), names(y));
        let p = self
            .proposals()
            .into_iter()
            .find(|p| {
                let (l, r) = (&p.left_name, &p.right_name);
                (*l == nx.display && *r == ny.display) || (*l == ny.display && *r == nx.display)
            })
            .unwrap_or_else(|| panic!("no proposal between {x} and {y}"));
        self.accept(&p.a, &p.b)
    }

    fn log(&self) -> Vec<DecisionRow> {
        self.project.decisions(None, 1_000).unwrap()
    }

    fn decision(&self, seq: i64) -> DecisionRow {
        self.log().into_iter().find(|d| d.seq == seq).unwrap()
    }

    fn status(&self, seq: i64) -> Option<DecisionStatus> {
        self.decision(seq).status
    }

    /// Mention ids of the key, the ones in `file` only when given.
    fn mentions(&self, key: &str, file: Option<&str>) -> Vec<i64> {
        self.project
            .occurrences(key)
            .unwrap()
            .iter()
            .filter(|o| {
                file.is_none_or(|f| o.path.as_deref().is_some_and(|p| Path::new(p).ends_with(f)))
            })
            .map(|o| o.mention_id)
            .collect()
    }

    /// What the log looks like byte for byte, without the derived status.
    fn raw_log(&self) -> Vec<RawDecision> {
        self.project
            .store()
            .read_raw(|c| {
                let mut stmt = c.prepare(
                    "SELECT seq, kind, anchor, before, after, decided_at FROM decision ORDER BY seq",
                )?;
                let rows = stmt.query_map([], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                    ))
                })?;
                Ok(rows.collect::<Result<_, _>>()?)
            })
            .unwrap()
    }

    /// The stored rows equal the reference model and a rebuild, and the project is healthy.
    fn consistent(&self) {
        let incremental = self.project.store().derived_state().unwrap();
        let reference = mk_graph::model::reference(&self.project.store().facts().unwrap());
        assert_eq!(incremental, reference, "reference");
        let rebuilt = self.project.rebuild().unwrap();
        assert!(
            rebuilt.same_as_incremental && rebuilt.consistent,
            "{rebuilt:?}"
        );
        let h = self.project.check_health().unwrap();
        assert!(h.is_healthy(), "{h:?}");
    }
}

// ----- US-11.2: proposals -----

/// US-11.2 AC1: variants are proposed with a score and plain reasons; D7 links add a hard identifier.
#[test]
fn us_11_2_ac1_similar_names_get_a_score_and_plain_reasons() {
    let s = setup();
    let ps = s.proposals();
    let p = ps
        .iter()
        .find(|p| p.a == C_HALE && p.b == CORVIN)
        .unwrap_or_else(|| panic!("{ps:?}"));
    assert!(p.score >= PROPOSAL_THRESHOLD);
    assert_eq!(p.reasons, ["An initial matches a first name"]);
    assert_eq!(
        (p.left_name.as_str(), p.right_name.as_str()),
        ("C. Hale", "Corvin Hale")
    );
    assert!(
        ps.windows(2).all(|w| w[0].score >= w[1].score),
        "highest first"
    );
}

/// US-11.2 AC1: two different names with one phone in a table row are proposed for that phone, a
/// shared domain is not hard evidence.
#[test]
fn us_11_2_ac1_a_shared_hard_identifier_is_a_reason() {
    let s = setup_with(
        finding(Vec::new()),
        finding(Vec::new()),
        &[
            (
                "a.csv",
                "name,phone,site\nAnna Berg,+44 20 7946 0000,x.example\nHugo Stein,+44 20 7946 0001,y.example\n",
            ),
            (
                "b.csv",
                "name,phone,site\nTomas Reinholt,+44 20 7946 0000,z.example\nNina Holm,+44 20 7946 0002,y.example\n",
            ),
        ],
    );
    for file in ["a.csv", "b.csv"] {
        let page = s
            .project
            .documents_page(&DocumentFilter::default(), None, 100)
            .unwrap();
        let row = page.rows.iter().find(|r| r.rel_path == file).unwrap();
        s.project
            .confirm_mapping(
                row.ref_id,
                Assignments(
                    [
                        ("name".to_string(), ColumnType::PersonName),
                        ("phone".to_string(), ColumnType::Phone),
                        ("site".to_string(), ColumnType::UrlDomain),
                    ]
                    .into(),
                ),
            )
            .unwrap();
    }
    let ps = s.proposals();
    assert_eq!(ps.len(), 1, "{ps:?}");
    assert_eq!(ps[0].a, "person:anna berg");
    assert_eq!(ps[0].b, "person:tomas reinholt");
    assert!(
        ps[0].reasons[0].starts_with("Same phone"),
        "{:?}",
        ps[0].reasons
    );
    assert!(
        ps[0].reasons[0].ends_with("in 2 documents"),
        "{:?}",
        ps[0].reasons
    );
}

/// US-11.2 AC2, INV-ER-03: a proposal changes nothing, however long it stays.
#[test]
fn us_11_2_ac2_nothing_is_merged_on_its_own() {
    let s = setup();
    let before = s.project.store().derived_state().unwrap();
    assert!(!s.proposals().is_empty());
    s.rescan();
    s.project.rebuild().unwrap();
    let after = s.project.store().derived_state().unwrap();
    assert_eq!(before, after);
    assert!(after.entity_members.is_empty());
    assert!(s.log().is_empty(), "no decision was made for the user");
}

/// US-11.2 AC3, INV-ER-05: only at or above the threshold, each with a reason, none below it stored.
#[test]
fn us_11_2_ac3_only_at_or_above_the_threshold() {
    let s = setup();
    let ps = s.proposals();
    assert!(!ps.is_empty());
    for p in &ps {
        assert!(p.score >= PROPOSAL_THRESHOLD, "{p:?}");
        assert!(!p.reasons.is_empty() && p.reasons.iter().all(|r| !r.is_empty()));
    }
    let high = s.project.score_proposals(0.9).unwrap();
    assert!(high.len() < ps.len());
    assert!(high.iter().all(|p| p.score >= 0.9));
}

/// The background thread makes proposals by itself, off the writer (ADR-0005: they may lag).
#[test]
fn proposals_appear_without_being_asked_for() {
    let s = setup();
    wait_until("the proposal thread", || {
        !s.project.proposals(None, 10).unwrap().is_empty()
    });
}

/// US-11.3 AC1: pages go on where they stopped, highest score first, with no pair twice.
#[test]
fn proposals_page_by_cursor() {
    let s = setup();
    let all = s.proposals();
    assert!(all.len() >= 2, "{all:?}");
    let mut paged = Vec::new();
    let mut after = None;
    loop {
        let page = s.project.proposals(after.as_ref(), 1).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(ProposalCursor {
            score: last.score,
            a: last.a.clone(),
            b: last.b.clone(),
        });
        paged.extend(page);
    }
    assert_eq!(paged, all);
}

/// INV-ER-04: a rejected pair is not proposed again after a re-scan, a rebuild, a new model or a
/// reopen.
#[test]
fn inv_er_04_rejected_pair_never_comes_back() {
    let s = setup();
    assert!(s.proposed(GREGOR, MR_ALMQVIST));
    s.project.reject_proposal(GREGOR, MR_ALMQVIST).unwrap();
    assert!(
        !s.project
            .proposals(None, 100)
            .unwrap()
            .iter()
            .any(|p| p.a == GREGOR),
        "gone in the decision's own transaction"
    );
    assert!(!s.proposed(GREGOR, MR_ALMQVIST));
    s.rescan();
    assert!(!s.proposed(GREGOR, MR_ALMQVIST));
    s.project.rebuild().unwrap();
    assert!(!s.proposed(GREGOR, MR_ALMQVIST));
    s.rerun_with_y();
    assert!(!s.proposed(GREGOR, MR_ALMQVIST));
    s.consistent();
}

/// INV-ER-04: rejecting one name against a group keeps the whole group from that name, through a name
/// that is not the group's key, and whichever comes first.
#[test]
fn inv_er_04_a_reject_stands_between_the_groups() {
    let after_the_merge = setup();
    after_the_merge.accept(C_HALE, CORVIN);
    assert!(
        after_the_merge
            .proposals()
            .iter()
            .any(|p| p.a == MR_HALE || p.b == MR_HALE)
    );
    after_the_merge
        .project
        .reject_proposal(CORVIN, MR_HALE)
        .unwrap();
    let before_the_merge = setup();
    before_the_merge
        .project
        .reject_proposal(CORVIN, MR_HALE)
        .unwrap();
    before_the_merge.accept(C_HALE, CORVIN);
    for s in [&after_the_merge, &before_the_merge] {
        let ps = s.proposals();
        assert!(
            !ps.iter().any(|p| p.a == MR_HALE || p.b == MR_HALE),
            "{ps:?}"
        );
    }
}

impl Setup {
    /// What a proposal run has made from the project as it is now, and the generation it was made
    /// from: a run under way while the test decides something.
    fn run_under_way(&self) -> (u64, Vec<mk_core::Proposal>) {
        let input = self.project.store().proposal_input().unwrap();
        let generation = input.generation;
        let made = mk_graph::proposal::proposals(
            input.candidates,
            &input.blocked,
            PROPOSAL_THRESHOLD,
            &AtomicBool::new(false),
        );
        (generation, made)
    }
}

/// INV-ER-04: a decision made while a proposal run was comparing names still wins: the run's results
/// are filtered again when they are stored.
#[test]
fn inv_er_04_a_run_that_was_under_way_cannot_bring_a_rejected_pair_back() {
    let s = setup();
    let (generation, made) = s.run_under_way();
    assert!(made.iter().any(|p| p.a == GREGOR && p.b == MR_ALMQVIST));
    s.project.reject_proposal(GREGOR, MR_ALMQVIST).unwrap();
    let store = s.project.store();
    assert!(store.replace_proposals(generation, made).unwrap().is_some());
    let ps = s.project.proposals(None, 100).unwrap();
    assert!(!ps.iter().any(|p| p.a == GREGOR), "{ps:?}");
}

/// Nor does such a run bring back a pair merged in the meantime, which would be an entity proposed
/// with itself (INV-ER-06).
#[test]
fn inv_er_06_a_run_that_was_under_way_cannot_bring_a_merged_pair_back() {
    let s = setup();
    s.project.refresh_proposals().unwrap();
    let (generation, made) = s.run_under_way();
    assert!(made.iter().any(|p| p.a == C_HALE && p.b == CORVIN));
    s.project
        .accept_proposal(C_HALE, CORVIN, MergeEdit::default())
        .unwrap();
    let store = s.project.store();
    assert!(store.replace_proposals(generation, made).unwrap().is_some());
    let ps = s.project.proposals(None, 100).unwrap();
    assert!(!ps.iter().any(|p| p.left_key == p.right_key), "{ps:?}");
}

/// INV-ER-06: at most one proposal per pair, and an entity is never paired with itself, also when a
/// group has two names that each look like the other side.
#[test]
fn inv_er_06_one_proposal_per_pair_of_entities() {
    let s = setup();
    let pairs = |ps: &[ProposalRow]| -> usize {
        ps.iter()
            .map(|p| (&p.left_key, &p.right_key))
            .collect::<BTreeSet<_>>()
            .len()
    };
    let ps = s.proposals();
    assert_eq!(pairs(&ps), ps.len(), "{ps:?}");
    s.accept(C_HALE, CORVIN);
    let ps = s.proposals();
    assert_eq!(pairs(&ps), ps.len(), "{ps:?}");
    for p in &ps {
        assert_ne!(p.left_key, p.right_key);
    }
    assert!(!ps.iter().any(|p| p.a == C_HALE && p.b == CORVIN));
}

/// A proposal is between entities of one type; a changed type moves it.
#[test]
fn proposals_are_between_one_type() {
    let s = setup();
    assert!(s.proposed(GREGOR, MR_ALMQVIST));
    s.project
        .change_entity_type(GREGOR, EntityType::Org)
        .unwrap();
    assert!(!s.proposed(GREGOR, MR_ALMQVIST));
    s.project
        .change_entity_type(GREGOR, EntityType::Person)
        .unwrap();
    assert!(s.proposed(GREGOR, MR_ALMQVIST));
}

/// A split group is not proposed (ASSUMPTION, M7a), which keeps a split from being proposed straight
/// back.
#[test]
fn split_groups_are_not_proposed() {
    let s = setup();
    let moved = s.mentions(CORVIN, Some("b.txt"));
    let seq = s.project.split_mentions(CORVIN, &moved).unwrap().seq;
    let split = s.decision(seq).anchor;
    let ps = s.proposals();
    assert!(
        !ps.iter()
            .any(|p| p.left_key == split || p.right_key == split),
        "{ps:?}"
    );
}

/// A split ends the proposal it takes the reason from, in its own transaction, and a later run does
/// not make it again: the moved mention no longer counts for its name, so the two names sit in one
/// document (domain §7.4 "Stale", INV-ER-01).
#[test]
fn a_split_ends_the_proposals_it_takes_the_reason_from() {
    let two_names = vec![person("Corvin Hale"), person("C. Hale")];
    let s = setup_with(
        finding(two_names.clone()),
        finding(two_names),
        &[
            ("x.txt", "Corvin Hale paid.\nC. Hale signed."),
            ("y.txt", "C. Hale wrote again."),
        ],
    );
    s.extract();
    assert!(s.proposed(C_HALE, CORVIN));
    let moved = s.mentions(C_HALE, Some("y.txt"));
    assert_eq!(moved.len(), 1);
    s.project.split_mentions(C_HALE, &moved).unwrap();
    let stored = s.project.proposals(None, 100).unwrap();
    assert!(stored.is_empty(), "{stored:?}");
    assert!(!s.proposed(C_HALE, CORVIN));
}

/// A proposal is made from what is shown by default: not from a mention under 0.5 (INV-AI-05)
/// or of a file that is gone, which comes back with the file.
#[test]
fn proposals_use_the_mentions_shown_by_default() {
    let unsure = FakeName {
        confidence: Some(0.3),
        ..person("Mr Almqvist")
    };
    let found = vec![person("Gregor Almqvist"), unsure];
    let low = setup_with(finding(found.clone()), finding(found), FILES);
    low.extract();
    assert!(low.entity(MR_ALMQVIST).is_some());
    assert!(!low.proposed(GREGOR, MR_ALMQVIST));

    let s = setup();
    assert!(s.proposed(GREGOR, MR_ALMQVIST));
    s.hide("d.txt");
    assert!(!s.proposed(GREGOR, MR_ALMQVIST));
    s.restore("d.txt");
    assert!(s.proposed(GREGOR, MR_ALMQVIST));
}

/// A changed type ends the proposal in its own transaction, before any run (domain §7.4 "Stale").
#[test]
fn a_changed_type_ends_the_proposal_at_once() {
    let s = setup();
    assert!(s.proposed(GREGOR, MR_ALMQVIST));
    s.project
        .change_entity_type(GREGOR, EntityType::Org)
        .unwrap();
    let stored = s.project.proposals(None, 100).unwrap();
    assert!(!stored.iter().any(|p| p.a == GREGOR), "{stored:?}");
}

/// D6: "Unreviewed merge" is a pending proposal, read by key, and gone once it is decided.
#[test]
fn d6_a_pending_proposal_is_the_unreviewed_mark() {
    let s = setup();
    s.project.refresh_proposals().unwrap();
    let keys = [GREGOR.to_string(), PORT.to_string()];
    assert_eq!(
        s.project.unreviewed_keys(&keys).unwrap(),
        BTreeSet::from([GREGOR.to_string()])
    );
    s.project.reject_proposal(GREGOR, MR_ALMQVIST).unwrap();
    assert!(s.project.unreviewed_keys(&keys).unwrap().is_empty());
}

/// D6: the mark follows a merged entity's key, and isn't stored on the connection row.
#[test]
fn d6_the_mark_follows_the_group_and_is_not_on_the_row() {
    let s = setup();
    let before = s.project.store().derived_state().unwrap().connections;
    s.project.refresh_proposals().unwrap();
    assert_eq!(
        s.project.store().derived_state().unwrap().connections,
        before
    );
    s.accept(C_HALE, CORVIN);
    let keys = [C_HALE.to_string()];
    assert_eq!(
        s.project.unreviewed_keys(&keys).unwrap(),
        BTreeSet::new(),
        "the proposals of the two ended with the merge"
    );
    s.project.refresh_proposals().unwrap();
    assert_eq!(
        s.project.unreviewed_keys(&keys).unwrap(),
        BTreeSet::from([C_HALE.to_string()]),
        "Mr Hale is proposed against the group, whose key is its smallest name key"
    );
}

// ----- US-11.3 AC2 and AC4, US-12.2, INV-ER-07: what a merged entity shows -----

/// US-11.3 AC2: one entity, under the smallest name key; the more frequent name is shown, and the
/// others are aliases.
#[test]
fn us_11_3_ac2_accept_makes_one_entity_with_the_frequent_name() {
    let s = setup();
    let seq = s.accept(C_HALE, CORVIN);
    let keys = s.connection_keys();
    assert!(keys.contains(C_HALE), "{keys:?}");
    assert!(!keys.contains(CORVIN), "{keys:?}");
    let row = s.entity(C_HALE).unwrap();
    assert_eq!(row.display_name, "Corvin Hale");
    assert_eq!(row.mention_count, 5);
    assert_eq!(row.display_key, CORVIN);
    let names = s.project.entity_names(C_HALE).unwrap().unwrap();
    assert_eq!(names.display, "Corvin Hale");
    assert_eq!(names.aliases, [("C. Hale".to_string(), 2)]);
    // The name keys of the merged side open the same entity.
    assert_eq!(s.project.entity_names(CORVIN).unwrap().unwrap(), names);
    let d = s.decision(seq);
    assert_eq!(d.kind, DecisionKind::AcceptMerge);
    assert_eq!(d.anchor, format!("{C_HALE}|{CORVIN}"));
    assert_eq!(d.status, Some(DecisionStatus::Active));
    s.consistent();
}

/// US-11.3 AC4: the name and type chosen when accepting, in one go.
#[test]
fn us_11_3_ac4_accept_with_a_name_and_type() {
    let s = setup();
    s.project.refresh_proposals().unwrap();
    s.project
        .accept_proposal(
            C_HALE,
            CORVIN,
            MergeEdit {
                name: Some("Corvin A. Hale".into()),
                entity_type: Some(EntityType::Org),
            },
        )
        .unwrap();
    let row = s.entity(C_HALE).unwrap();
    assert_eq!(row.display_name, "Corvin A. Hale");
    assert_eq!(row.entity_type, "org");
    let log = s.log();
    assert_eq!(log[0].kind, DecisionKind::ChangeType);
    assert_eq!(log[1].kind, DecisionKind::Rename);
    assert_eq!(log[2].kind, DecisionKind::AcceptMerge);
    s.consistent();
}

/// A proposal that is not pending cannot be accepted, and a refusal leaves no decision behind.
#[test]
fn inv_er_03_accept_needs_a_pending_proposal() {
    let s = setup();
    assert!(
        s.project
            .accept_proposal(C_HALE, PORT, MergeEdit::default())
            .is_err()
    );
    s.project.refresh_proposals().unwrap();
    s.project.reject_proposal(C_HALE, CORVIN).unwrap();
    assert!(
        s.project
            .accept_proposal(C_HALE, CORVIN, MergeEdit::default())
            .is_err()
    );
    assert_eq!(s.log().len(), 1, "only the reject");
    assert!(s.project.reject_proposal(C_HALE, C_HALE).is_err());
    assert!(s.project.reject_proposal("person:", "nonsense").is_err());
    assert_eq!(s.log().len(), 1);
}

/// US-12.2 AC1, INV-ER-07: a user-set name and type win over the most frequent, across a group too;
/// undoing a merge gives each side its own decisions back.
#[test]
fn inv_er_07_user_edits_win_and_undo_gives_each_side_its_own() {
    let s = setup();
    s.project.rename_entity(CORVIN, "Corvin the Elder").unwrap();
    s.project.rename_entity(C_HALE, "Chad Hale").unwrap();
    assert_eq!(s.entity(CORVIN).unwrap().display_name, "Corvin the Elder");
    assert_eq!(s.entity(C_HALE).unwrap().display_name, "Chad Hale");
    s.accept(C_HALE, CORVIN);
    assert_eq!(
        s.entity(C_HALE).unwrap().display_name,
        "Chad Hale",
        "newest"
    );
    s.project.rename_entity(C_HALE, "Corvin Hale Sr").unwrap();
    assert_eq!(s.entity(C_HALE).unwrap().display_name, "Corvin Hale Sr");
    s.consistent();
    s.project.undo_merge(C_HALE, CORVIN).unwrap();
    assert_eq!(s.entity(C_HALE).unwrap().display_name, "Chad Hale");
    // Anchored on the display name's key when it was made: Corvin Hale's.
    assert_eq!(s.entity(CORVIN).unwrap().display_name, "Corvin Hale Sr");
    s.consistent();
}

/// US-12.2 AC2: a note is stored and an empty one clears it.
#[test]
fn us_12_2_ac2_note() {
    let s = setup();
    s.project
        .set_entity_note(CORVIN, "Harbour contact")
        .unwrap();
    assert_eq!(
        s.entity(CORVIN).unwrap().note.as_deref(),
        Some("Harbour contact")
    );
    s.project.set_entity_note(CORVIN, "  ").unwrap();
    assert_eq!(s.entity(CORVIN).unwrap().note, None);
    assert!(s.project.rename_entity(CORVIN, "   ").is_err());
    assert_eq!(s.log().len(), 2);
    s.consistent();
}

/// US-12.2 AC1: an alias goes off the list and comes back; a removal does not cancel another's, and
/// the mention stays where it is.
#[test]
fn us_12_2_ac1_remove_and_restore_an_alias() {
    let s = setup();
    s.accept(C_HALE, CORVIN);
    s.accept_between(C_HALE, MR_HALE);
    let names = s.project.entity_names(C_HALE).unwrap().unwrap();
    assert_eq!(names.aliases.len(), 2, "{names:?}");
    let count = s.entity(C_HALE).unwrap().mention_count;
    s.project.remove_alias(C_HALE, "C. Hale").unwrap();
    s.project.remove_alias(C_HALE, "Mr Hale").unwrap();
    let names = s.project.entity_names(C_HALE).unwrap().unwrap();
    assert!(names.aliases.is_empty(), "{names:?}");
    assert_eq!(names.removed.len(), 2, "{names:?}");
    assert_eq!(s.entity(C_HALE).unwrap().mention_count, count);
    s.project.restore_alias(C_HALE, "Mr Hale").unwrap();
    let names = s.project.entity_names(C_HALE).unwrap().unwrap();
    assert_eq!(names.aliases, [("Mr Hale".to_string(), 1)]);
    assert_eq!(names.removed, [("C. Hale".to_string(), 2)]);
    assert!(
        s.project.remove_alias(C_HALE, "Corvin Hale").is_err(),
        "the main name"
    );
    assert!(s.project.remove_alias(C_HALE, "Nobody").is_err());
    s.consistent();
}

/// The ignore decision applies to the entity holding its key, merged or not, and not to a split group
/// holding that key's moved mentions.
#[test]
fn ignore_follows_the_entity_and_not_a_split() {
    let s = setup();
    s.project.set_ignored(CORVIN, true).unwrap();
    assert!(!s.connection_keys().contains(CORVIN));
    s.accept(C_HALE, CORVIN);
    assert!(
        !s.connection_keys().contains(C_HALE),
        "the group is ignored"
    );
    let moved: Vec<i64> = [
        s.mentions(C_HALE, Some("c.txt")),
        s.mentions(C_HALE, Some("d.txt")),
    ]
    .concat();
    let seq = s.project.split_mentions(C_HALE, &moved).unwrap().seq;
    let split = s.decision(seq).anchor;
    assert!(
        s.connection_keys().contains(&split),
        "a new entity is not ignored"
    );
    s.project.set_ignored(C_HALE, false).unwrap();
    assert!(s.connection_keys().contains(C_HALE));
    s.consistent();
}

// ----- US-11.4: undo and split -----

/// US-11.4 AC1, INV-CU-01: undo takes them apart as a new decision and stores the pair as rejected:
/// the accept stays in the log, Superseded, and the pair is not proposed again.
#[test]
fn us_11_4_ac1_undo_stores_the_pair_as_rejected() {
    let s = setup();
    let accepted = s.accept(C_HALE, CORVIN);
    let undo = s.project.undo_merge(CORVIN, C_HALE).unwrap();
    let keys = s.project.store().derived_state().unwrap();
    assert!(keys.entity_members.is_empty());
    assert_eq!(s.entity(C_HALE).unwrap().mention_count, 2);
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 3);
    let d = s.decision(undo);
    assert_eq!(d.kind, DecisionKind::RejectMerge);
    assert_eq!(d.before.as_deref(), Some("merged"));
    assert_eq!(d.after.as_deref(), Some("rejected"));
    assert_eq!(d.status, Some(DecisionStatus::Active));
    assert_eq!(s.status(accepted), Some(DecisionStatus::Superseded));
    assert!(!s.proposed(C_HALE, CORVIN));
    assert!(
        s.project.undo_merge(C_HALE, CORVIN).is_err(),
        "not merged any more"
    );
    s.consistent();
}

/// A chain through a middle name holds until its own link is undone, and undoing one link leaves the
/// other.
#[test]
fn undoing_one_link_of_a_chain_leaves_the_other() {
    let s = setup();
    s.accept(C_HALE, CORVIN);
    s.accept(C_HALE, MR_HALE);
    assert_eq!(s.entity(C_HALE).unwrap().mention_count, 6);
    s.project.undo_merge(C_HALE, CORVIN).unwrap();
    assert_eq!(
        s.entity(C_HALE).unwrap().mention_count,
        3,
        "C. Hale and Mr Hale stay one"
    );
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 3);
    assert!(s.entity(MR_HALE).is_none());
    s.consistent();
}

/// Undoing the link between two pairs leaves a side whose new key is in neither the pair nor the old
/// group's key: it still gets its row, and the merged member loses its own.
#[test]
fn undoing_a_link_gives_the_far_side_its_new_key() {
    const ANNA: &str = "person:anna reed";
    const BERT: &str = "person:bert stone";
    const CARL: &str = "person:carl stone";
    const DANA: &str = "person:dana reed";
    let found = vec![
        person("Anna Reed"),
        person("Bert Stone"),
        person("Carl Stone"),
        person("Dana Reed"),
    ];
    let s = setup_with(
        finding(found.clone()),
        finding(found),
        &[
            ("a.txt", "Anna Reed paid."),
            ("b.txt", "Bert Stone paid."),
            ("c.txt", "Carl Stone paid."),
            ("d.txt", "Dana Reed paid."),
        ],
    );
    s.extract();
    s.accept_whatever_is_proposed(ANNA, DANA);
    s.accept_whatever_is_proposed(BERT, CARL);
    s.accept_whatever_is_proposed(CARL, DANA);
    assert_eq!(s.entity(ANNA).unwrap().mention_count, 4);
    assert!(s.entity(BERT).is_none());
    s.consistent();

    s.project.undo_merge(CARL, DANA).unwrap();
    assert_eq!(s.entity(ANNA).unwrap().mention_count, 2, "Anna and Dana");
    assert_eq!(s.entity(BERT).unwrap().mention_count, 2, "Bert and Carl");
    assert!(s.entity(CARL).is_none() && s.entity(DANA).is_none());
    s.consistent();
}

impl Setup {
    /// Accepts the pair whatever the scorer makes of it, for a test about a group's shape. The
    /// proposal thread may replace the table first, so it tries again.
    fn accept_whatever_is_proposed(&self, a: &str, b: &str) {
        let store = self.project.store();
        for _ in 0..5 {
            let generation = store.proposal_input().unwrap().generation;
            let pending = mk_core::Proposal {
                a: a.to_string(),
                b: b.to_string(),
                left: a.to_string(),
                right: b.to_string(),
                score: 1.0,
                reasons: vec!["Test".to_string()],
            };
            store.replace_proposals(generation, vec![pending]).unwrap();
            let accepted = self.project.accept_proposal(a, b, MergeEdit::default());
            if accepted.is_ok() {
                return;
            }
        }
        panic!("could not accept {a} and {b}");
    }
}

/// A chain A–B–C holds while B has no mentions; the links through B read Orphaned (ASSUMPTION, M7a,
/// domain §9.5).
#[test]
fn a_chain_holds_through_a_name_that_is_gone() {
    let s = setup_with(
        finding(vec![
            person("Corvin Hale"),
            person("C. Hale"),
            person("Mr Hale"),
        ]),
        finding(Vec::new()),
        &[
            ("p.txt", "Corvin Hale signed."),
            ("q.txt", "C. Hale signed."),
            ("r.txt", "Mr Hale signed."),
            ("s.txt", "Corvin Hale paid."),
            ("t.txt", "Mr Hale paid."),
        ],
    );
    s.extract();
    // Mr Hale fits both other names until they are one (S7), so he comes second.
    let second = s.accept(C_HALE, CORVIN);
    let first = s.accept(C_HALE, MR_HALE);
    assert_eq!(s.entity(C_HALE).unwrap().mention_count, 5);
    assert_eq!(s.status(first), Some(DecisionStatus::Active));
    s.remove("q.txt");
    let row = s.entity(C_HALE).unwrap();
    assert_eq!(row.mention_count, 4, "still one, under its stable key");
    assert_eq!(s.status(first), Some(DecisionStatus::Orphaned));
    assert_eq!(s.status(second), Some(DecisionStatus::Orphaned));
    assert!(s.entity(CORVIN).is_none());
    assert!(s.entity(MR_HALE).is_none());
    s.consistent();

    // Undone while its middle is gone: the two ends come apart, Mr Hale still under the group's key.
    s.project.undo_merge(C_HALE, CORVIN).unwrap();
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 2);
    assert_eq!(s.entity(C_HALE).unwrap().mention_count, 2);
    assert_eq!(s.entity(C_HALE).unwrap().display_name, "Mr Hale");
    assert!(s.entity(MR_HALE).is_none());
    s.consistent();
}

/// US-11.1 AC2: mentions with one name key can be split, against INV-ER-02's default.
#[test]
fn us_11_1_ac2_exact_matches_can_be_split() {
    let s = setup();
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 3);
    let moved = s.mentions(CORVIN, Some("c.txt"));
    assert_eq!(moved.len(), 1);
    let seq = s.project.split_mentions(CORVIN, &moved).unwrap().seq;
    let split = s.decision(seq).anchor;
    assert!(split.starts_with("split:"), "{split}");
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 2);
    let new = s.entity(&split).unwrap();
    assert_eq!(new.mention_count, 1);
    assert_eq!(new.display_name, "Corvin Hale");
    assert_eq!(new.display_key, CORVIN);
    assert_eq!(new.entity_type, "person");
    s.consistent();
}

/// US-11.4 AC2: the split is one decision naming its mentions; each side lists and opens its own
/// mentions; undo puts them back as a new decision.
#[test]
fn us_11_4_ac2_split_is_a_decision_and_opens_at_its_sources() {
    let s = setup();
    let moved: Vec<i64> = [
        s.mentions(CORVIN, Some("a.txt")),
        s.mentions(CORVIN, Some("c.txt")),
    ]
    .concat();
    let seq = s.project.split_mentions(CORVIN, &moved).unwrap().seq;
    let d = s.decision(seq);
    assert_eq!(d.kind, DecisionKind::Split);
    assert_eq!(d.before.as_deref(), Some("Corvin Hale"));
    assert_eq!(d.after.as_deref().unwrap().lines().count(), 2);
    assert_eq!(d.status, Some(DecisionStatus::Active));
    let split = d.anchor;

    assert!(
        s.connection_keys().contains(&split),
        "two sources: a connection row"
    );
    let occ = s.project.occurrences(&split).unwrap();
    assert_eq!(occ.len(), 2);
    for o in &occ {
        let ctx = s.project.source_context(o.mention_id).unwrap().unwrap();
        assert_eq!(ctx.occurrence.unwrap().raw, "Corvin Hale");
    }
    assert_eq!(s.mentions(CORVIN, None).len(), 1);
    let listed = s
        .project
        .connections_page(&ConnectionQuery::default(), None, 100)
        .unwrap();
    let row = listed
        .rows
        .iter()
        .find(|r| r.row.subject_key == split)
        .unwrap();
    assert_eq!(row.kind, "person");
    assert_eq!(row.value, "Corvin Hale");
    s.consistent();

    let undo = s.project.undo_split(&split).unwrap();
    assert_eq!(s.decision(undo).kind, DecisionKind::UndoSplit);
    assert_eq!(s.status(seq), Some(DecisionStatus::Superseded));
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 3);
    assert!(s.entity(&split).is_none());
    assert!(s.project.undo_split(&split).is_err(), "already put back");
    assert!(s.project.undo_split(CORVIN).is_err());
    s.consistent();
}

/// A split can take mentions of a merged entity or of an earlier split, leaving at least one behind.
#[test]
fn a_split_works_on_groups_and_on_splits() {
    let s = setup();
    s.accept(C_HALE, CORVIN);
    assert!(
        s.project
            .split_mentions(C_HALE, &s.mentions(C_HALE, None))
            .is_err(),
        "something must stay"
    );
    assert!(s.project.split_mentions(C_HALE, &[]).is_err());
    assert!(
        s.project
            .split_mentions(C_HALE, &s.mentions(PORT, None))
            .is_err()
    );
    let two = [
        s.mentions(C_HALE, Some("c.txt")),
        s.mentions(C_HALE, Some("d.txt")),
    ]
    .concat();
    assert_eq!(two.len(), 2);
    let first = s.project.split_mentions(C_HALE, &two).unwrap().seq;
    let outer = s.decision(first).anchor;
    assert_eq!(s.entity(C_HALE).unwrap().mention_count, 3);
    assert_eq!(s.entity(&outer).unwrap().mention_count, 2);
    assert_eq!(s.entity(&outer).unwrap().display_name, "C. Hale");

    // A mention in two standing splits goes to the newest; the older keeps the rest.
    let one = s.mentions(&outer, Some("d.txt"));
    let second = s.project.split_mentions(&outer, &one).unwrap().seq;
    let inner = s.decision(second).anchor;
    assert_eq!(s.entity(&inner).unwrap().mention_count, 1);
    assert_eq!(s.entity(&outer).unwrap().mention_count, 1);
    assert_eq!(s.status(first), Some(DecisionStatus::Active));
    s.consistent();
    s.project.undo_split(&inner).unwrap();
    assert_eq!(
        s.entity(&outer).unwrap().mention_count,
        2,
        "back to the older split"
    );
    s.consistent();
}

/// INV-ER-01: at every step each mention is in exactly one entity.
#[test]
fn inv_er_01_each_mention_is_in_exactly_one_entity() {
    let s = setup();
    let total = |s: &Setup| -> u64 {
        s.project
            .store()
            .read_raw(|c| {
                Ok(c.query_row("SELECT COUNT(*) FROM entity_mention", [], |r| {
                    r.get::<_, i64>(0)
                })?)
            })
            .unwrap() as u64
    };
    let check = |s: &Setup| {
        let state = s.project.store().derived_state().unwrap();
        let counted: u64 = state.entities.values().map(|e| e.mention_count).sum();
        assert_eq!(counted, total(s), "{:?}", state.entities);
        let mut seen = BTreeSet::new();
        for key in state.entities.keys() {
            for o in s.project.occurrences(key).unwrap() {
                assert!(
                    seen.insert(o.mention_id),
                    "{} is listed twice",
                    o.mention_id
                );
            }
        }
    };
    check(&s);
    s.accept(C_HALE, CORVIN);
    check(&s);
    let moved = s.mentions(C_HALE, Some("c.txt"));
    let split = s.project.split_mentions(C_HALE, &moved).unwrap().seq;
    check(&s);
    s.project.undo_merge(C_HALE, CORVIN).unwrap();
    check(&s);
    s.project.undo_split(&s.decision(split).anchor).unwrap();
    check(&s);
}

/// INV-ER-08, INV-CU-04: an entity with no mentions does not exist; its decisions stay, orphaned, and
/// apply again when the name comes back.
#[test]
fn inv_er_08_no_mentions_no_entity_and_decisions_stay() {
    let s = setup();
    let rename = s.project.rename_entity(MR_HALE, "Mister H").unwrap();
    let note = s.project.set_entity_note(MR_HALE, "seen once").unwrap();
    assert_eq!(s.status(rename), Some(DecisionStatus::Active));
    let bytes = std::fs::read(s.src.join("d.txt")).unwrap();
    s.remove("d.txt");
    assert!(s.entity(MR_HALE).is_none());
    assert_eq!(s.status(rename), Some(DecisionStatus::Orphaned));
    assert_eq!(s.status(note), Some(DecisionStatus::Orphaned));
    assert_eq!(s.log().len(), 2, "kept");
    s.consistent();

    // A removed path stays excluded, so the same content comes back under another.
    std::fs::write(s.src.join("d2.txt"), &bytes).unwrap();
    s.rescan();
    s.extract();
    assert_eq!(s.status(rename), Some(DecisionStatus::Active));
    let row = s.entity(MR_HALE).unwrap();
    assert_eq!(row.display_name, "Mister H");
    assert_eq!(row.note.as_deref(), Some("seen once"));
    s.consistent();
}

/// INV-CU-04: a split reads Orphaned while its mentions are gone, and stands again when the content
/// comes back. A changed file is different content: its mentions stay with the name.
#[test]
fn inv_cu_04_split_orphaned_and_active_again() {
    let s = setup();
    let moved = s.mentions(CORVIN, Some("c.txt"));
    let seq = s.project.split_mentions(CORVIN, &moved).unwrap().seq;
    let split = s.decision(seq).anchor;
    let bytes = std::fs::read(s.src.join("c.txt")).unwrap();
    s.remove("c.txt");
    assert!(s.entity(&split).is_none());
    assert_eq!(s.status(seq), Some(DecisionStatus::Orphaned));
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 2);
    s.consistent();

    // A removed path stays excluded, so the same content comes back under another.
    std::fs::write(s.src.join("c2.txt"), &bytes).unwrap();
    s.rescan();
    s.extract();
    assert_eq!(s.status(seq), Some(DecisionStatus::Active));
    assert_eq!(s.entity(&split).unwrap().mention_count, 1);
    assert_eq!(
        s.entity(CORVIN).unwrap().mention_count,
        2,
        "not back in the name"
    );
    s.consistent();

    std::fs::write(s.src.join("c2.txt"), "Corvin Hale called a third time.\n").unwrap();
    s.rescan();
    s.extract();
    assert_eq!(s.status(seq), Some(DecisionStatus::Orphaned));
    assert!(s.entity(&split).is_none());
    assert_eq!(s.entity(CORVIN).unwrap().mention_count, 3);
    s.consistent();
}

/// A file only hidden from the sources keeps its content's mentions, Missing, and the split holds.
#[test]
fn inv_cu_04_a_missing_file_keeps_the_split() {
    let s = setup();
    let moved = s.mentions(CORVIN, Some("c.txt"));
    let seq = s.project.split_mentions(CORVIN, &moved).unwrap().seq;
    let split = s.decision(seq).anchor;
    s.hide("c.txt");
    assert_eq!(s.entity(&split).unwrap().mention_count, 1);
    s.consistent();
    s.restore("c.txt");
    assert_eq!(s.entity(&split).unwrap().mention_count, 1);
    assert_eq!(s.status(seq), Some(DecisionStatus::Active));
    s.consistent();
}

// ----- US-11.5: decisions survive -----

/// Every kind, with what proves it still holds.
struct Kind {
    name: &'static str,
    make: fn(&Setup) -> i64,
    holds: fn(&Setup, &str),
}

fn holds_merge(s: &Setup, _: &str) {
    assert_eq!(s.entity(C_HALE).unwrap().display_name, "Corvin Hale");
    assert!(s.entity(CORVIN).is_none());
    assert!(s.entity(C_HALE).unwrap().mention_count >= 5);
}

fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            name: "accept",
            make: |s| s.accept(C_HALE, CORVIN),
            holds: holds_merge,
        },
        Kind {
            name: "reject",
            make: |s| {
                s.project.refresh_proposals().unwrap();
                s.project.reject_proposal(GREGOR, MR_ALMQVIST).unwrap()
            },
            holds: |s, _| assert!(!s.proposed(GREGOR, MR_ALMQVIST)),
        },
        Kind {
            name: "undo",
            make: |s| {
                s.accept(C_HALE, CORVIN);
                s.project.undo_merge(C_HALE, CORVIN).unwrap()
            },
            holds: |s, _| {
                assert!(s.entity(CORVIN).is_some() && s.entity(C_HALE).is_some());
                assert!(!s.proposed(C_HALE, CORVIN));
            },
        },
        Kind {
            name: "split",
            make: |s| {
                s.project
                    .split_mentions(CORVIN, &s.mentions(CORVIN, Some("c.txt")))
                    .unwrap()
                    .seq
            },
            holds: |s, anchor| {
                assert_eq!(s.entity(anchor).unwrap().mention_count, 1);
                assert_eq!(s.entity(CORVIN).unwrap().mention_count, 2);
                assert_eq!(s.mentions(anchor, Some("c.txt")).len(), 1);
            },
        },
        Kind {
            name: "undo split",
            make: |s| {
                let made = s
                    .project
                    .split_mentions(CORVIN, &s.mentions(CORVIN, Some("c.txt")))
                    .unwrap();
                s.project.undo_split(&made.key).unwrap()
            },
            holds: |s, _| assert_eq!(s.entity(CORVIN).unwrap().mention_count, 3),
        },
        Kind {
            name: "rename",
            make: |s| s.project.rename_entity(CORVIN, "The Elder Hale").unwrap(),
            holds: |s, _| assert_eq!(s.entity(CORVIN).unwrap().display_name, "The Elder Hale"),
        },
        Kind {
            name: "type",
            make: |s| {
                s.project
                    .change_entity_type(CORVIN, EntityType::Org)
                    .unwrap()
            },
            holds: |s, _| assert_eq!(s.entity(CORVIN).unwrap().entity_type, "org"),
        },
        Kind {
            name: "note",
            make: |s| {
                s.project
                    .set_entity_note(CORVIN, "Harbour contact")
                    .unwrap()
            },
            holds: |s, _| {
                assert_eq!(
                    s.entity(CORVIN).unwrap().note.as_deref(),
                    Some("Harbour contact")
                )
            },
        },
        Kind {
            name: "alias",
            make: |s| {
                s.accept(C_HALE, CORVIN);
                s.project.remove_alias(C_HALE, "C. Hale").unwrap()
            },
            holds: |s, _| {
                let names = s.project.entity_names(C_HALE).unwrap().unwrap();
                assert_eq!(names.removed.len(), 1, "{names:?}");
                assert!(names.aliases.is_empty(), "{names:?}");
            },
        },
        Kind {
            name: "ignore",
            make: |s| {
                s.project.set_ignored(CORVIN, true).unwrap();
                s.log()[0].seq
            },
            holds: |s, _| assert!(!s.connection_keys().contains(CORVIN)),
        },
    ]
}

/// US-11.5 AC1, INV-CU-06: each kind of decision still applies after a re-scan, a rebuild, a new
/// model, and a new app version (the derived rows dropped on open). The log is unchanged throughout.
#[test]
fn us_11_5_ac1_every_kind_survives_rescan_model_and_version() {
    for kind in kinds() {
        let s = setup();
        let seq = (kind.make)(&s);
        let anchor = s.decision(seq).anchor;
        let log = s.raw_log();
        let check = |s: &Setup, step: &str| {
            eprintln!("{}: {step}", kind.name);
            (kind.holds)(s, &anchor);
            assert_eq!(s.raw_log(), log, "{}: the log changed at {step}", kind.name);
            let reference = mk_graph::model::reference(&s.project.store().facts().unwrap());
            assert_eq!(
                s.project.store().derived_state().unwrap(),
                reference,
                "{step}"
            );
        };
        check(&s, "made");
        s.project.rescan(true).unwrap();
        s.project.run_stage_a().unwrap();
        check(&s, "re-scan");
        s.hide("e.txt");
        s.restore("e.txt");
        check(&s, "another file went and came back");
        s.project.rebuild().unwrap();
        check(&s, "rebuild");
        s.rerun_with_y();
        check(&s, "new model");

        // A new app version: the derived version differs, so open drops and rebuilds the rows.
        s.write("UPDATE project_meta SET value = '0' WHERE key = 'derived_version'")
            .unwrap();
        let mut s = s;
        s.reopen();
        check(&s, "new app version");
    }
}

/// US-11.5 AC2, INV-CU-01: every decision has its time in UTC and its before and after, and the log
/// is append-only: triggers refuse an update or a delete.
#[test]
fn us_11_5_ac2_the_log_has_time_and_before_and_after() {
    let s = setup();
    let accept = s.accept(C_HALE, CORVIN);
    let rename = s.project.rename_entity(C_HALE, "Corvin C. Hale").unwrap();
    let note = s.project.set_entity_note(C_HALE, "first").unwrap();
    let note2 = s.project.set_entity_note(C_HALE, "second").unwrap();
    s.project.refresh_proposals().unwrap();
    let reject = s.project.reject_proposal(GREGOR, MR_ALMQVIST).unwrap();
    let log = s.log();
    assert_eq!(
        log.iter().map(|d| d.seq).collect::<Vec<_>>(),
        [reject, note2, note, rename, accept],
        "newest first"
    );
    assert!(
        log.iter()
            .all(|d| d.decided_at.ends_with('Z') || d.decided_at.ends_with("+00:00")),
        "{log:?}"
    );
    let d = s.decision(accept);
    assert_eq!(d.before.as_deref(), Some("C. Hale / Corvin Hale"));
    assert_eq!(d.after.as_deref(), Some("Corvin Hale"));
    let d = s.decision(rename);
    assert_eq!(d.before.as_deref(), Some("Corvin Hale"));
    assert_eq!(d.after.as_deref(), Some("Corvin C. Hale"));
    assert_eq!(d.anchor, CORVIN, "the display name's key when it was made");
    assert_eq!(s.decision(note).before, None);
    assert_eq!(s.decision(note2).before.as_deref(), Some("first"));
    assert_eq!(s.status(note), Some(DecisionStatus::Superseded));
    assert_eq!(s.status(note2), Some(DecisionStatus::Active));
    assert_eq!(s.project.decisions(Some(note), 2).unwrap()[0].seq, rename);

    for sql in ["DELETE FROM decision", "UPDATE decision SET after = 'x'"] {
        assert!(s.write(sql).is_err(), "{sql}");
    }
}

/// INV-CU-03: at every commit of a re-run, a reader never sees results without the decisions.
#[test]
fn inv_cu_03_a_rerun_never_shows_results_without_decisions() {
    let slow_y = FakeExtraction {
        delay_ms: 60,
        ..finding(names())
    };
    let s = setup_with(finding(names()), slow_y, FILES);
    s.extract();
    let moved = s.mentions(CORVIN, Some("c.txt"));
    let split = s.project.split_mentions(CORVIN, &moved).unwrap().seq;
    let split = s.decision(split).anchor;
    s.accept(C_HALE, CORVIN);
    s.project.rename_entity(C_HALE, "Chosen Name").unwrap();
    s.project.set_ignored(PORT, true).unwrap();
    s.project.refresh_proposals().unwrap();
    s.project.reject_proposal(GREGOR, MR_ALMQVIST).unwrap();
    let old_run = s.project.store().active_ai_run().unwrap().unwrap();

    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("y".into())))
        .unwrap();
    s.ws.start_rerun(&s.project).unwrap();
    let reader = |s: &Setup| {
        let state = s.project.store().derived_state().unwrap();
        assert!(!state.entities.contains_key(CORVIN), "the merge is applied");
        assert_eq!(state.entities[C_HALE].display_name, "Chosen Name");
        assert_eq!(
            state.entities[&split].mention_count, 1,
            "the split is applied"
        );
        assert!(state.connections[PORT].ignored);
        let reference = mk_graph::model::reference(&s.project.store().facts().unwrap());
        assert_eq!(state.decision_status, reference.decision_status);
    };
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.ws.run_stage_b(&s.project).unwrap());
        while !runner.is_finished() {
            reader(&s);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(runner.join().unwrap().failed, 0);
    });
    reader(&s);
    assert_ne!(
        s.project.store().active_ai_run().unwrap().unwrap().id,
        old_run.id
    );
    s.consistent();
}

/// ADR-0005: a decision made while a re-run is under way is in force when it switches, and a re-run
/// that is cancelled leaves decisions alone.
#[test]
fn adr_0005_decisions_cross_the_rerun_switch() {
    let slow_y = FakeExtraction {
        delay_ms: 40,
        ..finding(names())
    };
    let s = setup_with(finding(names()), slow_y, FILES);
    s.extract();
    s.project.refresh_proposals().unwrap();
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("y".into())))
        .unwrap();
    s.ws.start_rerun(&s.project).unwrap();
    std::thread::scope(|scope| {
        let runner = scope.spawn(|| s.ws.run_stage_b(&s.project).unwrap());
        s.project
            .accept_proposal(C_HALE, CORVIN, MergeEdit::default())
            .unwrap();
        s.project.rename_entity(PORT, "The Port").unwrap();
        s.project
            .set_entity_note(GREGOR, "made during the re-run")
            .unwrap();
        assert_eq!(runner.join().unwrap().failed, 0);
    });
    assert_eq!(
        s.project
            .store()
            .active_ai_run()
            .unwrap()
            .unwrap()
            .model_name,
        "Test y"
    );
    holds_merge(&s, "");
    assert_eq!(s.entity(PORT).unwrap().display_name, "The Port");
    assert_eq!(
        s.entity(GREGOR).unwrap().note.as_deref(),
        Some("made during the re-run")
    );
    s.consistent();
}

/// INV-CU-05: for one anchor and kind the newest wins, also across accept and reject of a pair, which
/// are one family.
#[test]
fn inv_cu_05_newest_wins() {
    let s = setup();
    s.project.refresh_proposals().unwrap();
    let reject = s.project.reject_proposal(MR_HALE, CORVIN).unwrap();
    assert!(!s.proposed(MR_HALE, CORVIN));
    let accept = s.accept(C_HALE, CORVIN);
    let rename = s.project.rename_entity(C_HALE, "One").unwrap();
    let newer = s.project.rename_entity(C_HALE, "Two").unwrap();
    assert_eq!(s.status(rename), Some(DecisionStatus::Superseded));
    assert_eq!(s.status(newer), Some(DecisionStatus::Active));
    assert_eq!(s.status(reject), Some(DecisionStatus::Active));
    assert_eq!(s.status(accept), Some(DecisionStatus::Active));
    s.consistent();
}

/// A decision's derived rows are all-or-nothing with it: a refused decision leaves the log and the
/// rows exactly as they were.
#[test]
fn a_refused_decision_changes_nothing() {
    let s = setup();
    let before = s.project.store().derived_state().unwrap();
    assert!(s.project.split_mentions(CORVIN, &[i64::MAX]).is_err());
    assert!(s.project.rename_entity("person:nobody", "x").is_err());
    assert!(s.project.undo_merge(C_HALE, CORVIN).is_err());
    assert_eq!(s.project.store().derived_state().unwrap(), before);
    assert!(s.log().is_empty());
}
