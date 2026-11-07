//! M7b on [`Workspace`] with the fake `llama-server`: what the review queue and the entity page read
//! (US-07.7 AC4, US-11.4, US-11.5 AC2, US-12.1), and the event a proposal run sends.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mk_core::{
    AiChoice, ConnectionQuery, DecisionKind, EgressConfig, FixedProbe, Gpu, GpuVendor,
    HardwareProfile, MergeEdit, ModelRole, ModelsConfig, Project, ProposalCursor, RuntimeConfig,
    SpawnOptions, TlsRoots, WorkerConfig, Workspace,
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
const PORT: &str = "org:velmora port authority";

/// Plus `c.txt` with Corvin Hale twice, in parts of its own: [`two_mentions`].
const FILES: &[(&str, &str)] = &[
    (
        "a.txt",
        "Paid by Corvin Hale on 2 May.\nVelmora Port Authority signed the invoice.",
    ),
    (
        "b.txt",
        "C. Hale wrote to Velmora Port Authority.\nCorvin Hale replied that day.",
    ),
    ("d.txt", "C. Hale was there.\nMr Hale paid."),
    (
        "e.eml",
        "From: Corvin Hale <corvin@velmora.example>\r\nTo: desk@velmora.example\r\n\
         Subject: Invoice\r\n\r\nPaid.\r\n",
    ),
];

/// The same name twice, far enough apart for two extraction units.
fn two_mentions() -> String {
    let filler = "Nothing here at all.\n".repeat(150);
    format!("Corvin Hale called once more.\n{filler}Corvin Hale called again.\n")
}

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

fn names() -> Vec<FakeName> {
    let person = |n: &str| FakeName {
        name: n.into(),
        ..FakeName::default()
    };
    vec![
        person("Corvin Hale"),
        person("C. Hale"),
        person("Mr Hale"),
        FakeName {
            name: "Velmora Port Authority".into(),
            kind: "org".into(),
            ..FakeName::default()
        },
    ]
}

struct Setup {
    tmp: tempfile::TempDir,
    _ws: Workspace,
    project: Project,
    src: PathBuf,
    _hf: FakeHf,
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A project over [`FILES`], Stage A and Stage B done, proposals made.
fn setup() -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let mut files: Vec<(String, String)> = FILES
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect();
    files.push(("c.txt".into(), two_mentions()));
    let (ws, project, src, hf) = build(tmp.path(), &files, names());
    Setup {
        tmp,
        _ws: ws,
        project,
        src,
        _hf: hf,
    }
}

fn build(
    root: &Path,
    files: &[(String, String)],
    names: Vec<FakeName>,
) -> (Workspace, Project, PathBuf, FakeHf) {
    let control_file = root.join("control.json");
    Control {
        extraction: HashMap::from([(
            "x.gguf".to_string(),
            FakeExtraction {
                names,
                ..FakeExtraction::default()
            },
        )]),
        ..Control::default()
    }
    .write(&control_file);
    let bytes = test_bytes(64 * 1024, 1);
    let x = test_model("x", ModelRole::Extraction, std::slice::from_ref(&bytes));
    let hf = FakeHf::serving(&[(&x, std::slice::from_ref(&bytes))]);
    let ws = Workspace::open_with_runtime(
        &root.join("app"),
        EgressConfig {
            resolver: Arc::new(ScriptedResolver::new()),
            secrets: Some(Arc::new(FakeKeyring::new())),
            roots: TlsRoots::Platform,
            call_timeout: Duration::from_secs(40),
        },
        ModelsConfig {
            models_dir: Some(root.join("models")),
            catalog: test_catalog(vec![x.clone()]),
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
    let src = root.join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (name, text) in files {
        std::fs::write(src.join(name), text).unwrap();
    }
    let (mut project, _) = ws.create("Case", root).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.add_source_root(&src).unwrap();
    ws.start_download(&x.id).unwrap();
    wait_until("installed", || {
        ws.model_catalog()
            .unwrap()
            .iter()
            .any(|v| v.id == x.id && v.state == mk_core::DownloadState::Installed)
    });
    ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("x".into())))
        .unwrap();
    project.ingest().unwrap();
    ws.schedule_stage_b(&project).unwrap();
    assert_eq!(ws.run_stage_b(&project).unwrap().failed, 0);
    project.refresh_proposals().unwrap();
    (ws, project, src, hf)
}

/// Makes the project the release app's review walk (`MK_REVIEW_WALK=1`) works on, in `MK_KEEP_PROJECT`:
/// a stand-in with the fake model, for when no project with real model output is kept.
#[test]
#[ignore = "makes a project to keep; run by hand"]
fn keep_a_project_for_the_review_walk() {
    let root = PathBuf::from(std::env::var_os("MK_KEEP_PROJECT").expect("MK_KEEP_PROJECT"));
    std::fs::create_dir_all(&root).unwrap();
    let people = [
        ("Corvin", "Hale"),
        ("Mirela", "Stroud"),
        ("Tobias", "Renwick"),
        ("Ines", "Calder"),
        ("Oskar", "Vane"),
        ("Petra", "Lund"),
        ("Dario", "Finch"),
    ];
    let mut files = Vec::new();
    let mut names = Vec::new();
    let person = |n: String| FakeName {
        name: n,
        ..FakeName::default()
    };
    for (first, last) in people {
        let full = format!("{first} {last}");
        let short = format!("{}. {last}", &first[..1]);
        let mr = format!("Mr {last}");
        let mail = format!("{}@velmora.example", first.to_lowercase());
        files.push((
            format!("{last}-1.txt"),
            format!("Paid by {full} on 2 May, from {mail}.\nVelmora Port Authority signed it."),
        ));
        files.push((
            format!("{last}-2.txt"),
            format!("{short} wrote to Velmora Port Authority.\n{full} replied, from {mail}."),
        ));
        files.push((
            format!("{last}-3.txt"),
            format!("{short} was there.\n{mr} paid."),
        ));
        names.extend([person(full), person(short), person(mr)]);
    }
    names.push(FakeName {
        name: "Velmora Port Authority".into(),
        kind: "org".into(),
        ..FakeName::default()
    });
    let (_ws, project, _src, _hf) = build(&root, &files, names);
    let n = project.proposal_count(None).unwrap();
    assert!(n >= 5, "the walk needs 5 proposals, got {n}");
    println!("{} proposals in {}", n, root.join("Case").display());
}

impl Setup {
    fn detail(&self, key: &str) -> mk_core::EntityDetail {
        self.project.entity_detail(key, false).unwrap().unwrap()
    }

    fn accept(&self, a: &str, b: &str) {
        self.project.refresh_proposals().unwrap();
        self.project
            .accept_proposal(a, b, MergeEdit::default())
            .unwrap();
    }

    fn evidence_in(&self, key: &str, file: &str) -> Vec<i64> {
        self.project
            .entity_evidence(key, 1_000, false)
            .unwrap()
            .iter()
            .filter(|o| {
                o.path
                    .as_deref()
                    .is_some_and(|p| Path::new(p).ends_with(file))
            })
            .map(|o| o.mention_id)
            .collect()
    }

    fn proposal_pair(&self, x: &str, y: &str) -> (String, String) {
        self.project.refresh_proposals().unwrap();
        let p = self
            .project
            .proposals(None, 100)
            .unwrap()
            .into_iter()
            .find(|p| {
                (p.left_key == x && p.right_key == y) || (p.left_key == y && p.right_key == x)
            })
            .unwrap_or_else(|| panic!("no proposal between {x} and {y}"));
        (p.a, p.b)
    }
}

fn file_of(d: &mk_core::EntityDocument) -> &str {
    Path::new(d.path.as_deref().unwrap())
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
}

#[test]
fn us_12_1_ac1_the_page_has_names_type_identifiers_documents_and_counts() {
    let s = setup();
    let d = s.detail(CORVIN);
    assert_eq!(d.key, CORVIN);
    assert_eq!(d.names.display, "Corvin Hale");
    assert_eq!(d.entity_type, "person");
    assert_eq!(d.note, None);
    assert!(!d.from_table);
    assert!(!d.is_split);
    assert_eq!(
        d.linked
            .first
            .iter()
            .map(|i| i.key.as_str())
            .collect::<Vec<_>>(),
        ["email:corvin@velmora.example"]
    );
    assert_eq!(d.linked.total, 1);

    // Most mentions first; each document's own count; the total is all of them.
    let counts: HashMap<&str, u64> = d
        .documents
        .iter()
        .map(|x| (file_of(x), x.mentions))
        .collect();
    assert_eq!(counts["c.txt"], 2);
    assert_eq!(counts["a.txt"], 1);
    assert_eq!(counts["b.txt"], 1);
    assert_eq!(file_of(&d.documents[0]), "c.txt");
    assert_eq!(d.documents[0].file_type, "txt");
    assert_eq!(d.documents_total, d.documents.len() as u64);
    assert_eq!(
        d.evidence_total,
        d.documents.iter().map(|x| x.mentions).sum::<u64>()
    );
    assert_eq!(d.evidence_total, d.mention_count);
    assert_eq!(
        s.project
            .entity_evidence(CORVIN, 1_000, false)
            .unwrap()
            .len() as u64,
        d.evidence_total
    );
}

#[test]
fn us_12_1_a_merged_entity_has_aliases_and_a_name_key_reads_as_its_group() {
    let s = setup();
    let (a, b) = s.proposal_pair(CORVIN, C_HALE);
    s.accept(&a, &b);
    let d = s.detail(C_HALE);
    let same = s.detail(CORVIN);
    assert_eq!(d, same, "either name key reads the group");
    assert_eq!(d.names.display, "Corvin Hale");
    assert!(d.names.aliases.iter().any(|(n, _)| n == "C. Hale"));
    // Evidence through a member's key is the group's too.
    assert_eq!(
        s.project
            .entity_evidence(C_HALE, 1_000, false)
            .unwrap()
            .len() as u64,
        d.evidence_total
    );
    assert_eq!(d.evidence_total, d.mention_count);
}

#[test]
fn a_gone_entity_has_no_page() {
    let s = setup();
    assert_eq!(
        s.project.entity_detail("person:nobody", false).unwrap(),
        None
    );
    assert_eq!(
        s.project.entity_detail("split:nothing", false).unwrap(),
        None
    );
    assert!(
        s.project
            .entity_evidence("person:nobody", 10, false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_missing_document_is_not_counted() {
    let s = setup();
    let before = s.detail(CORVIN);
    std::fs::rename(s.src.join("c.txt"), s.tmp.path().join("c.txt")).unwrap();
    s.project.rescan(false).unwrap();
    s.project.run_stage_a().unwrap();
    let after = s.detail(CORVIN);
    assert_eq!(after.documents_total, before.documents_total - 1);
    assert!(after.documents.iter().all(|d| file_of(d) != "c.txt"));
    assert_eq!(after.evidence_total, before.evidence_total - 2);
    assert!(s.evidence_in(CORVIN, "c.txt").is_empty());
}

#[test]
fn evidence_keeps_to_the_callers_limit() {
    let s = setup();
    assert_eq!(
        s.project.entity_evidence(CORVIN, 2, false).unwrap().len(),
        2
    );
}

#[test]
fn us_11_4_ac1_a_merge_lists_its_pair_and_undo_takes_it_apart() {
    let s = setup();
    let (a, b) = s.proposal_pair(CORVIN, C_HALE);
    assert!(s.detail(CORVIN).merges.is_empty());
    s.accept(&a, &b);
    let d = s.detail(CORVIN);
    assert_eq!(d.merges.len(), 1);
    let pair = &d.merges[0];
    assert_eq!((pair.a.as_str(), pair.b.as_str()), (C_HALE, CORVIN));
    assert_eq!(
        (pair.a_name.as_str(), pair.b_name.as_str()),
        ("C. Hale", "Corvin Hale")
    );

    s.project.undo_merge(&pair.a, &pair.b).unwrap();
    assert!(s.detail(CORVIN).merges.is_empty());
    assert_eq!(s.detail(C_HALE).key, C_HALE);
    assert_eq!(s.detail(CORVIN).key, CORVIN);
    // Stored as rejected, so it does not come back.
    s.project.refresh_proposals().unwrap();
    let found = s.project.proposals(None, 100).unwrap().iter().any(|p| {
        (p.left_key == CORVIN && p.right_key == C_HALE)
            || (p.left_key == C_HALE && p.right_key == CORVIN)
    });
    assert!(!found);
}

#[test]
fn us_11_4_ac1_a_chain_lists_each_merge_and_undoes_one_at_a_time() {
    let s = setup();
    let (a, b) = s.proposal_pair(CORVIN, C_HALE);
    s.accept(&a, &b);
    let (group, mr) = (s.detail(CORVIN).key, MR_HALE);
    let (c, d) = s.proposal_pair(&group, mr);
    s.accept(&c, &d);
    let page = s.detail(MR_HALE);
    assert_eq!(page.merges.len(), 2, "A–B and B–C");

    let first = page.merges[0].clone();
    s.project.undo_merge(&first.a, &first.b).unwrap();
    let left: u64 = [CORVIN, C_HALE, MR_HALE]
        .iter()
        .map(|k| s.detail(k).merges.len() as u64)
        .max()
        .unwrap();
    assert_eq!(left, 1, "the other merge still holds");
}

#[test]
fn us_11_4_ac2_a_split_gives_its_key_and_both_sides_name_each_other() {
    let s = setup();
    let moved = s.evidence_in(CORVIN, "c.txt");
    assert_eq!(moved.len(), 2);
    let made = s.project.split_mentions(CORVIN, &moved).unwrap();
    assert!(made.key.starts_with("split:"));

    let new = s.detail(&made.key);
    assert!(new.is_split);
    assert_eq!(new.key, made.key);
    assert_eq!(new.names.display, "Corvin Hale");
    assert_eq!(new.mention_count, 2);
    assert_eq!(new.split_from.len(), 1);
    assert_eq!(new.split_from[0].key, CORVIN);
    assert!(new.splits.is_empty());
    assert!(new.merges.is_empty());

    // One document only, so it is no connection: the page of its source still reaches it.
    let source = s.detail(CORVIN);
    assert_eq!(source.splits.len(), 1);
    assert_eq!(source.splits[0].key, made.key);
    assert_eq!(source.splits[0].mentions, 2);
    assert!(source.split_from.is_empty());
    assert!(
        !s.project
            .connections_page(&ConnectionQuery::default(), None, 100)
            .unwrap()
            .rows
            .iter()
            .any(|r| r.row.subject_key == made.key)
    );

    s.project.undo_split(&made.key).unwrap();
    assert_eq!(s.project.entity_detail(&made.key, false).unwrap(), None);
    assert!(s.detail(CORVIN).splits.is_empty());
}

#[test]
fn a_split_out_of_a_group_returns_to_the_group() {
    let s = setup();
    let (a, b) = s.proposal_pair(CORVIN, C_HALE);
    s.accept(&a, &b);
    let moved = s.evidence_in(CORVIN, "d.txt");
    assert!(!moved.is_empty());
    let made = s.project.split_mentions(CORVIN, &moved).unwrap();
    let group = s.detail(CORVIN).key;
    let new = s.detail(&made.key);
    assert_eq!(new.split_from[0].key, group);
    assert_eq!(s.detail(&group).splits[0].key, made.key);
}

#[test]
fn us_07_7_ac4_the_mark_the_filter_and_the_count_agree() {
    let s = setup();
    let all = s.project.proposals(None, 100).unwrap();
    assert!(!all.is_empty());
    assert_eq!(s.project.proposal_count(None).unwrap(), all.len() as u64);
    let filtered = s.project.proposals_of(Some(CORVIN), None, 100).unwrap();
    assert!(!filtered.is_empty());
    assert!(
        filtered
            .iter()
            .all(|p| p.left_key == CORVIN || p.right_key == CORVIN)
    );
    assert_eq!(
        s.project.proposal_count(Some(CORVIN)).unwrap(),
        filtered.len() as u64
    );
    assert_eq!(s.detail(CORVIN).pending_proposals, filtered.len() as u64);
    assert_eq!(s.project.proposal_count(Some(PORT)).unwrap(), 0);
    assert_eq!(s.detail(PORT).pending_proposals, 0);

    let page = s
        .project
        .connections_page(&ConnectionQuery::default(), None, 100)
        .unwrap();
    assert!(page.unreviewed.contains(CORVIN));
    assert!(!page.unreviewed.contains(PORT));
}

#[test]
fn the_filter_pages_and_finds_the_group_of_a_name_key() {
    let s = setup();
    let all = s.project.proposals_of(Some(CORVIN), None, 100).unwrap();
    let mut paged = Vec::new();
    let mut after: Option<ProposalCursor> = None;
    loop {
        let page = s
            .project
            .proposals_of(Some(CORVIN), after.as_ref(), 1)
            .unwrap();
        let Some(last) = page.last() else { break };
        after = Some(ProposalCursor {
            score: last.score,
            a: last.a.clone(),
            b: last.b.clone(),
        });
        paged.extend(page);
    }
    assert_eq!(paged, all);

    let (a, b) = s.proposal_pair(CORVIN, C_HALE);
    s.accept(&a, &b);
    let (group, mr) = (s.detail(C_HALE).key, MR_HALE);
    let _ = s.proposal_pair(&group, mr);
    assert_eq!(
        s.project.proposals_of(Some(C_HALE), None, 100).unwrap(),
        s.project.proposals_of(Some(&group), None, 100).unwrap(),
    );
}

#[test]
fn us_11_5_ac2_a_log_row_names_its_entity_and_what_a_split_moved() {
    let s = setup();
    let (a, b) = s.proposal_pair(CORVIN, C_HALE);
    s.accept(&a, &b);
    let moved = s.evidence_in(CORVIN, "c.txt");
    let made = s.project.split_mentions(CORVIN, &moved).unwrap();
    s.project
        .set_ignored("email:desk@velmora.example", true)
        .unwrap();
    let log = s.project.decisions(None, 10).unwrap();
    let of = |k: DecisionKind| log.iter().find(|d| d.kind == k).unwrap();

    let split = of(DecisionKind::Split);
    assert_eq!(split.moved_mentions(), Some(moved.len() as u64));
    assert_eq!(split.entity_key().as_deref(), Some(made.key.as_str()));
    let merge = of(DecisionKind::AcceptMerge);
    assert_eq!(merge.moved_mentions(), None);
    assert_eq!(merge.entity_key().as_deref(), Some(C_HALE));
    assert_eq!(of(DecisionKind::IgnoreIdentifier).entity_key(), None);
}

/// ADR-0003: a proposal run tells the screens when it changed the rows, and only then.
#[test]
fn a_proposal_run_says_when_it_changed_the_rows() {
    let s = setup();
    let events = s.project.subscribe();
    std::thread::sleep(mk_core::DEBOUNCE * 2);
    while events.try_recv().is_ok() {}

    s.project.refresh_proposals().unwrap();
    assert!(
        events.recv_timeout(mk_core::DEBOUNCE * 3).is_err(),
        "nothing changed"
    );

    let before = s.project.proposals(None, 100).unwrap();
    s.project
        .store()
        .write_raw(|c| Ok(c.execute_batch("DELETE FROM merge_proposal")?))
        .unwrap();
    s.project.refresh_proposals().unwrap();
    assert_eq!(s.project.proposals(None, 100).unwrap(), before);
    events
        .recv_timeout(Duration::from_secs(5))
        .expect("the run changed the rows");
}
