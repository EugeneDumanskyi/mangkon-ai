//! M6c on [`Workspace`] with the fake `llama-server`: names that normalize alike are one connection
//! (US-11.1 AC1), an entity row has its linked identifiers (US-07.7 AC2, D7), and its evidence has
//! a snippet, a confidence and a locator that opens (US-07.7 AC3).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mk_core::{
    AiChoice, Assignments, ColumnType, ConnectionQuery, DocumentFilter, EgressConfig, FixedProbe,
    Gpu, GpuVendor, HardwareProfile, Locator, ModelRole, ModelsConfig, Project, RuntimeConfig,
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

struct Setup {
    _tmp: tempfile::TempDir,
    _hf: FakeHf,
    ws: Workspace,
    project: Project,
    src: PathBuf,
}

/// A project over `files`, Stage A done, with a model that finds `names`.
fn setup(names: Vec<FakeName>, files: &[(&str, &str)]) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let control_file = tmp.path().join("control.json");
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
        &tmp.path().join("app"),
        EgressConfig {
            resolver: Arc::new(ScriptedResolver::new()),
            secrets: Some(Arc::new(FakeKeyring::new())),
            roots: TlsRoots::Platform,
            call_timeout: Duration::from_secs(40),
        },
        ModelsConfig {
            models_dir: Some(tmp.path().join("models")),
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
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (name, text) in files {
        std::fs::write(src.join(name), text).unwrap();
    }
    let (mut project, _) = ws.create("Case", tmp.path()).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.add_source_root(&src).unwrap();
    ws.start_download(&x.id).unwrap();
    let end = Instant::now() + WAIT;
    while !ws
        .model_catalog()
        .unwrap()
        .iter()
        .any(|v| v.id == x.id && v.state == mk_core::DownloadState::Installed)
    {
        assert!(Instant::now() < end, "timed out installing the model");
        std::thread::sleep(Duration::from_millis(5));
    }
    ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("x".into())))
        .unwrap();
    project.ingest().unwrap();
    Setup {
        _tmp: tmp,
        _hf: hf,
        ws,
        project,
        src,
    }
}

impl Setup {
    fn extract(&self) {
        self.ws.schedule_stage_b(&self.project).unwrap();
        let run = self.ws.run_stage_b(&self.project).unwrap();
        assert_eq!(run.failed, 0);
    }

    /// Confirms the file's columns as `pairs` say.
    fn confirm(&self, file: &str, pairs: &[(&str, ColumnType)]) {
        let page = self
            .project
            .documents_page(&DocumentFilter::default(), None, 100)
            .unwrap();
        let row = page.rows.iter().find(|r| r.rel_path == file).unwrap();
        self.project
            .confirm_mapping(
                row.ref_id,
                Assignments(pairs.iter().map(|(k, t)| (k.to_string(), *t)).collect()),
            )
            .unwrap();
    }

    fn page(&self, query: &ConnectionQuery) -> mk_core::ConnectionsPage {
        self.project.connections_page(query, None, 100).unwrap()
    }

    fn keys_of_kind(&self, kind: &str) -> Vec<String> {
        self.page(&ConnectionQuery {
            kinds: vec![kind.into()],
            ..ConnectionQuery::default()
        })
        .rows
        .iter()
        .map(|r| r.row.subject_key.clone())
        .collect()
    }

    /// The linked identifier keys of `name_key` and their total.
    fn linked(&self, name_key: &str) -> (Vec<String>, u64) {
        let page = self.page(&ConnectionQuery::default());
        assert!(
            page.rows.iter().any(|r| r.row.subject_key == name_key),
            "{name_key} is not listed"
        );
        page.linked.get(name_key).map_or((Vec::new(), 0), |l| {
            (l.first.iter().map(|i| i.key.clone()).collect(), l.total)
        })
    }
}

/// US-11.1 AC1: case, spaces, punctuation and legal suffixes make one connection, without review.
#[test]
fn us_11_1_ac1_variants_across_documents_make_one_connection() {
    let s = setup(
        vec![
            org("Velmora Port Authority Ltd"),
            org("VELMORA  PORT AUTHORITY"),
            org("Velmora-Port Authority, LLC"),
        ],
        &[
            ("a.txt", "Signed by Velmora Port Authority Ltd today."),
            ("b.txt", "Paid to VELMORA  PORT AUTHORITY on Friday."),
            ("c.txt", "Letter from Velmora-Port Authority, LLC again."),
        ],
    );
    s.extract();
    assert_eq!(s.keys_of_kind("org"), ["org:velmora port authority"]);
    let page = s.page(&ConnectionQuery::default());
    let row = &page.rows[0];
    assert_eq!(row.row.source_count, 3);
    assert_eq!(row.row.mention_count, 3);
}

/// US-11.1 AC1: "of the same type": a person and an organization with one name stay apart.
#[test]
fn us_11_1_ac1_a_person_and_an_org_with_one_name_stay_apart() {
    let s = setup(
        vec![
            FakeName {
                when: Some("Mx Morgan Hale".into()),
                ..person("Morgan Hale")
            },
            FakeName {
                when: Some("firm Morgan Hale".into()),
                ..org("Morgan Hale")
            },
        ],
        &[
            ("a.txt", "Mx Morgan Hale wrote."),
            ("b.txt", "Mx Morgan Hale called."),
            ("c.txt", "The firm Morgan Hale billed."),
            ("d.txt", "The firm Morgan Hale paid."),
        ],
    );
    s.extract();
    assert_eq!(s.keys_of_kind("person"), ["person:morgan hale"]);
    assert_eq!(s.keys_of_kind("org"), ["org:morgan hale"]);
}

/// US-11.1 AC1, US-05.3: a name from a table and the same name from a model share one key. The
/// "from table" mark stays only while every mention comes from tables.
#[test]
fn us_11_1_ac1_table_and_model_names_share_a_key() {
    let s = setup(
        vec![org("Saltmarsh Holdings")],
        &[
            (
                "suppliers.csv",
                "Supplier,Contact\nSaltmarsh Holdings Ltd,ops@saltmarsh.example\n",
            ),
            (
                "more.csv",
                "Supplier,Contact\nSaltmarsh Holdings Ltd,accounts@saltmarsh.example\n",
            ),
            ("note.txt", "Nothing about it yet."),
        ],
    );
    let columns = [
        ("Supplier", ColumnType::OrgName),
        ("Contact", ColumnType::Email),
    ];
    s.confirm("suppliers.csv", &columns);
    s.confirm("more.csv", &columns);
    let key = "org:saltmarsh holdings";
    let from_table = |s: &Setup| s.page(&ConnectionQuery::default()).entities[key].from_table;
    assert!(from_table(&s));

    std::fs::write(s.src.join("note.txt"), "Saltmarsh Holdings wrote back.").unwrap();
    s.project.ingest().unwrap();
    s.extract();
    let page = s.page(&ConnectionQuery::default());
    let row = page.rows.iter().find(|r| r.row.subject_key == key).unwrap();
    assert_eq!(row.row.source_count, 3);
    assert!(!from_table(&s), "a model found it too");
}

const HALE_MAIL: &str = "From: Corvin Hale <corvin@velmora.example>\r\n\
    To: Marta Quill <marta@velmora.example>, \"Oduya, Sam\" <sam@velmora.example>\r\n\
    Subject: Corvin Hale asks about billing@velmora.example\r\n\r\nSee you.\r\n";

/// US-07.7 AC2, D7: an entity's linked identifiers come from its CSV row and its email headers.
/// In a list of addresses each name pairs only with its own; an address in a Subject does not
/// pair; a body mention does not pair.
#[test]
fn us_07_7_ac2_linked_identifiers_from_a_row_and_a_header() {
    let s = setup(
        vec![
            person("Corvin Hale"),
            person("Marta Quill"),
            person("Oduya, Sam"),
        ],
        &[
            ("hale.eml", HALE_MAIL),
            (
                "body.txt",
                "Corvin Hale <body@velmora.example> and Marta Quill and Oduya, Sam met.",
            ),
            (
                "people.csv",
                "Name,Mail\nCorvin Hale,row@velmora.example\nSomeone Else,else@velmora.example\n",
            ),
        ],
    );
    s.confirm(
        "people.csv",
        &[
            ("Name", ColumnType::PersonName),
            ("Mail", ColumnType::Email),
        ],
    );
    s.extract();
    let (hale, total) = s.linked("person:corvin hale");
    assert_eq!(
        hale,
        ["email:corvin@velmora.example", "email:row@velmora.example"]
    );
    assert_eq!(total, 2);
    assert_eq!(
        s.linked("person:marta quill").0,
        ["email:marta@velmora.example"]
    );
    assert_eq!(
        s.linked("person:oduya sam").0,
        ["email:sam@velmora.example"]
    );
}

/// US-07.7 AC2, INV-CO-07: a link whose source is Missing is not shown. The row itself remains
/// while two other documents hold the name.
#[test]
fn us_07_7_ac2_a_missing_source_takes_its_link_away() {
    let s = setup(
        vec![person("Corvin Hale")],
        &[
            ("hale.eml", HALE_MAIL),
            ("a.txt", "Corvin Hale wrote."),
            ("b.txt", "Corvin Hale called."),
            ("people.csv", "Name,Mail\nCorvin Hale,row@velmora.example\n"),
        ],
    );
    s.confirm(
        "people.csv",
        &[
            ("Name", ColumnType::PersonName),
            ("Mail", ColumnType::Email),
        ],
    );
    s.extract();
    assert_eq!(s.linked("person:corvin hale").1, 2);

    std::fs::remove_file(s.src.join("hale.eml")).unwrap();
    s.project.ingest().unwrap();
    assert_eq!(
        s.linked("person:corvin hale").0,
        ["email:row@velmora.example"]
    );
    std::fs::remove_file(s.src.join("people.csv")).unwrap();
    s.project.ingest().unwrap();
    assert_eq!(s.linked("person:corvin hale"), (Vec::new(), 0));
}

/// US-07.7 AC2, INV-AI-05: a mention under 0.5 links only with "Show low confidence".
#[test]
fn us_07_7_ac2_a_low_confidence_mention_links_only_when_shown() {
    let s = setup(
        vec![
            FakeName {
                confidence: Some(0.3),
                when: Some("<corvin@velmora.example>".into()),
                ..person("Corvin Hale")
            },
            FakeName {
                when: Some("wrote".into()),
                ..person("Corvin Hale")
            },
        ],
        &[
            ("hale.eml", HALE_MAIL),
            ("a.txt", "Corvin Hale wrote."),
            ("b.txt", "Corvin Hale wrote again."),
        ],
    );
    s.extract();
    assert_eq!(s.linked("person:corvin hale"), (Vec::new(), 0));
    let page = s.page(&ConnectionQuery {
        include_low: true,
        ..ConnectionQuery::default()
    });
    assert_eq!(
        page.linked["person:corvin hale"].first[0].key,
        "email:corvin@velmora.example"
    );
}

/// US-07.7 AC2: the row carries the first few and counts the rest, most documents first.
#[test]
fn us_07_7_ac2_first_few_and_a_total() {
    let mail = |n: u32| format!("Name,Mail\nCorvin Hale,m{n}@velmora.example\n");
    let (a, b, c, d) = (mail(1), mail(2), mail(3), mail(4));
    let e = format!("{d}Someone Else,else@velmora.example\n");
    let s = setup(
        Vec::new(),
        &[
            ("a.csv", a.as_str()),
            ("b.csv", b.as_str()),
            ("c.csv", c.as_str()),
            ("d.csv", d.as_str()),
            ("e.csv", e.as_str()),
        ],
    );
    let columns = [
        ("Name", ColumnType::PersonName),
        ("Mail", ColumnType::Email),
    ];
    for f in ["a.csv", "b.csv", "c.csv", "d.csv", "e.csv"] {
        s.confirm(f, &columns);
    }
    let (first, total) = s.linked("person:corvin hale");
    assert_eq!(total, 4);
    assert_eq!(first.len(), mk_core::LINKED_SHOWN);
    assert_eq!(first[0], "email:m4@velmora.example");
}

/// US-07.7 AC3: each mention of an expanded entity has a snippet with the name marked, a
/// confidence and a model, and its locator opens the exact source.
#[test]
fn us_07_7_ac3_mentions_carry_snippet_confidence_and_a_locator_that_opens() {
    let s = setup(
        vec![FakeName {
            confidence: Some(0.8),
            ..person("Corvin Hale")
        }],
        &[
            ("hale.eml", HALE_MAIL),
            ("a.txt", "First line.\nThen Corvin Hale wrote."),
        ],
    );
    s.extract();
    let all = s.project.occurrences("person:corvin hale").unwrap();
    assert!(all.len() >= 2, "{all:?}");
    for o in &all {
        assert!((o.confidence - 0.8).abs() < 1e-6);
        assert_eq!(o.ai_model.as_deref(), Some("Test x"));
        assert!(
            o.snippet
                .iter()
                .any(|r| r.hit && r.text.contains("Corvin Hale")),
            "{:?}",
            o.snippet
        );
        let source = s.project.source_context(o.mention_id).unwrap().unwrap();
        assert_eq!(source.occurrence.unwrap().locator, o.locator);
        assert!(source.hit.is_some());
    }
    assert!(
        all.iter()
            .any(|o| matches!(&o.locator, Locator::EmailHeader { name, .. } if name == "From"))
    );
    assert!(
        all.iter()
            .any(|o| matches!(&o.locator, Locator::TextLine { line, .. } if line.get() == 2))
    );
}

/// The linked-identifier read on the largest local project against NFR-PERF-02 (a usable screen in
/// 2 s, of which this read is one part): TD-2 with its tables confirmed and the fake model finding
/// TD-1's story names. Needs TD-1 and TD-2, and an idle machine for the numbers:
///
/// ```sh
/// TMPDIR=/some/big/dir cargo test -p mk-core --release --test m6c_entities -- --ignored --nocapture
/// ```
#[test]
#[ignore = "NFR-PERF-02: timing on RM-1 with testdata/td1 and testdata/td2"]
fn linked_identifiers_on_td2() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata");
    let gt = mk_bench::load(&root.join("td1/ground-truth.json")).unwrap();
    let mut seen = std::collections::BTreeSet::new();
    let names: Vec<FakeName> = gt
        .entities
        .iter()
        .filter(|e| seen.insert(e.canonical.clone()))
        .map(|e| match e.kind {
            mk_bench::EntityKind::Person => person(&e.canonical),
            mk_bench::EntityKind::Org => org(&e.canonical),
        })
        .collect();
    let s = setup(names, &[]);
    s.project.add_source_root(&root.join("td2")).unwrap();
    let t = Instant::now();
    s.project.ingest().unwrap();
    eprintln!("ingest {:.1} s", t.elapsed().as_secs_f64());

    let t = Instant::now();
    let mut tables = Vec::new();
    let mut after = None;
    loop {
        let page = s
            .project
            .documents_page(&DocumentFilter::default(), after.as_ref(), 500)
            .unwrap();
        tables.extend(
            page.rows
                .iter()
                .filter(|r| r.mapping_state.is_some())
                .map(|r| r.ref_id),
        );
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    for ref_id in &tables {
        let Some(view) = s.project.mapping_view(*ref_id).unwrap() else {
            continue;
        };
        let assignments = view
            .columns
            .iter()
            .map(|c| (c.key.clone(), c.suggested))
            .collect();
        s.project
            .confirm_mapping(*ref_id, Assignments(assignments))
            .unwrap();
    }
    eprintln!(
        "{} tables confirmed, {:.1} s",
        tables.len(),
        t.elapsed().as_secs_f64()
    );

    let t = Instant::now();
    s.ws.run_documents(&s.project, mk_core::UnitScope::All)
        .unwrap();
    let run = s.ws.run_stage_b(&s.project).unwrap();
    eprintln!(
        "{} units, {} mentions, {:.1} s",
        run.done,
        run.mentions,
        t.elapsed().as_secs_f64()
    );

    let (links, headers): (i64, i64) = s
        .project
        .store()
        .read_raw(|c| {
            Ok((
                c.query_row("SELECT COUNT(*) FROM record_link", [], |r| r.get(0))?,
                c.query_row(
                    "SELECT COUNT(*) FROM entity_mention m JOIN segment s ON s.id = m.segment_id
                     WHERE s.loc_kind = 'email_header'",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    let query = ConnectionQuery {
        kinds: vec!["person".into(), "org".into()],
        ..ConnectionQuery::default()
    };
    let total = s.project.connection_count_for(&query).unwrap();
    let keys: Vec<String> = s
        .project
        .connections_page(&query, None, 200)
        .unwrap()
        .rows
        .iter()
        .map(|r| r.row.subject_key.clone())
        .collect();
    let timed = |f: &dyn Fn()| {
        let mut all: Vec<Duration> = (0..100)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed()
            })
            .collect();
        all.sort();
        (all[49], all[94], all[99])
    };
    let read = timed(&|| {
        s.project
            .store()
            .linked_identifiers(&keys, mk_core::LINKED_SHOWN, false)
            .unwrap();
    });
    let page = timed(&|| {
        s.project.connections_page(&query, None, 200).unwrap();
    });
    let rows_only = timed(&|| {
        s.project
            .store()
            .connections_page(&query, None, 200)
            .unwrap();
    });
    eprintln!(
        "{total} entity connections, {} on the page, {links} record links, {headers} header mentions",
        keys.len()
    );
    eprintln!("linked read  p50/p95/max {read:?}");
    eprintln!("whole page   p50/p95/max {page:?}");
    eprintln!("rows only    p50/p95/max {rows_only:?}");
    assert!(page.1 < Duration::from_secs(2), "{page:?}");
}
