//! M4c in the core, E05: column mapping. A CSV or JSON file gets a suggested type per column
//! and makes no entities until confirmed (US-05.1, INV-CM-02/03); confirming applies at once,
//! a file with the same headers can reuse the mapping, a re-scan with the same headers keeps
//! it and changed headers ask for review (US-05.2, D5); confirmed name columns give "Name (from
//! table)" entities whose mentions open at their cell (US-05.3).

use std::path::Path;

use mk_core::{
    Assignments, ColumnType, DocumentFilter, Locator, MappingState, Project, ProjectError,
};

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

fn consistent(p: &Project) -> bool {
    let r = p.rebuild().unwrap();
    r.same_as_incremental && r.consistent
}

/// `(ref_id, mapping state)` of a file by its relative path.
fn row(p: &Project, rel: &str) -> (i64, Option<MappingState>) {
    let rows = p
        .documents_page(&DocumentFilter::default(), None, 100)
        .unwrap()
        .rows;
    let r = rows.iter().find(|r| r.rel_path == rel).unwrap();
    (r.ref_id, r.mapping_state)
}

fn entity_keys(p: &Project) -> Vec<String> {
    p.store()
        .derived_state()
        .unwrap()
        .entities
        .into_keys()
        .collect()
}

const PAYMENTS: &str = "Supplier,Contact email,Amount\n\
    Brightwater Logistics Ltd,accounts@brightwater.example,1200\n\
    Harbour Freight GmbH,billing@harbour.example,800\n\
    BRIGHTWATER LOGISTICS LIMITED,accounts@brightwater.example,300\n";

const LEDGER: &str = "Supplier,Contact email,Amount\n\
    Brightwater Logistics Ltd,ops@brightwater.example,50\n";

const REGISTRY: &str = r#"{"companies": [
    {"name": "Brightwater Logistics Ltd", "email": "info@brightwater.example",
     "officers": [{"name": "Ann Lee"}, {"name": "Bob Stone"}]}
]}"#;

fn assignments(pairs: &[(&str, ColumnType)]) -> Assignments {
    Assignments(pairs.iter().map(|(k, t)| (k.to_string(), *t)).collect())
}

/// US-05.1 AC1–AC3, INV-CM-02/03: each column gets a suggested type and up to 5 samples; the
/// file is "Suggested" and no entity exists until a confirm.
#[test]
fn us_05_1_suggestions_make_no_entities() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[("payments.csv", PAYMENTS), ("notes.txt", "hello\n")],
    );
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    let (id, state) = row(&p, "payments.csv");
    assert_eq!(state, Some(MappingState::Suggested));
    assert_eq!(row(&p, "notes.txt").1, None, "not a table");
    let view = p.mapping_view(id).unwrap().unwrap();
    assert_eq!(view.state, MappingState::Suggested);
    let cols: Vec<(&str, ColumnType)> = view
        .columns
        .iter()
        .map(|c| (c.key.as_str(), c.suggested))
        .collect();
    assert_eq!(
        cols,
        [
            ("Supplier", ColumnType::OrgName),
            ("Contact email", ColumnType::Email),
            ("Amount", ColumnType::Ignore),
        ]
    );
    assert_eq!(view.columns[0].samples.len(), 3);
    assert!(view.columns.iter().all(|c| c.assigned.is_none()));
    assert!(view.reuse.is_empty());
    assert!(entity_keys(&p).is_empty(), "suggestions make no entities");
    assert!(consistent(&p));
}

/// US-05.2 AC1/AC2, US-05.3 AC1/AC2: confirming applies at once; the second file with the
/// same headers is offered the mapping and reuses it; the entity connects both files under its
/// most frequent name, and each mention opens at its cell.
#[test]
fn us_05_2_confirm_and_reuse() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &[("payments.csv", PAYMENTS), ("ledger.csv", LEDGER)]);
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    let (payments, _) = row(&p, "payments.csv");
    let (ledger, _) = row(&p, "ledger.csv");
    assert!(p.mapping_view(ledger).unwrap().unwrap().reuse.is_empty());

    let events = p.subscribe();
    p.confirm_mapping(
        payments,
        assignments(&[
            ("Supplier", ColumnType::OrgName),
            ("Contact email", ColumnType::Email),
            ("Amount", ColumnType::Ignore),
        ]),
    )
    .unwrap();
    assert!(
        events
            .recv_timeout(std::time::Duration::from_secs(2))
            .is_ok()
    );
    assert_eq!(row(&p, "payments.csv").1, Some(MappingState::Confirmed));
    assert_eq!(
        entity_keys(&p),
        ["org:brightwater logistics", "org:harbour freight"]
    );

    let offered = p.mapping_view(ledger).unwrap().unwrap().reuse;
    assert_eq!(offered.len(), 1);
    assert_eq!(offered[0].ref_id, payments);
    assert!(offered[0].path.ends_with("payments.csv"));
    p.reuse_mapping(ledger, payments).unwrap();
    assert_eq!(row(&p, "ledger.csv").1, Some(MappingState::Confirmed));

    let page = p.connections_page(None, 100).unwrap();
    let c = page
        .rows
        .iter()
        .find(|r| r.subject_key == "org:brightwater logistics")
        .expect("a connection across both files");
    assert_eq!(c.source_count, 2);
    let e = &page.entities["org:brightwater logistics"];
    assert_eq!(e.display_name, "Brightwater Logistics Ltd");
    assert!(e.from_table);
    assert_eq!(e.entity_type, "org");

    let occ = p.occurrences("org:brightwater logistics").unwrap();
    assert_eq!(occ.len(), 3);
    for o in &occ {
        assert!(o.mention_id < 0, "entity mentions have their own ids");
        let Locator::CsvCell { column, span, .. } = &o.locator else {
            panic!("{:?}", o.locator);
        };
        assert_eq!(column, &mk_core::Column::Name("Supplier".into()));
        let ctx = p.source_context(o.mention_id).unwrap().unwrap();
        let hit = ctx.hit.unwrap();
        assert_eq!(hit.span, *span);
    }
    assert!(consistent(&p));

    // A key that is not a column is refused; so is reuse from a file without a mapping.
    let e = p
        .confirm_mapping(ledger, assignments(&[("Nope", ColumnType::OrgName)]))
        .unwrap_err();
    assert!(matches!(e, ProjectError::Invalid(_)), "{e:?}");
    assert!(p.reuse_mapping(payments, payments).is_err());
}

/// US-05.2 AC3, D5: a re-scan of a changed file with the same headers applies the mapping
/// again; changed headers make it "Needs review" and keep the unchanged columns' types.
#[test]
fn us_05_2_ac3_rescan_reapplies_or_needs_review() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &[("payments.csv", PAYMENTS)]);
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    let (id, _) = row(&p, "payments.csv");
    p.confirm_mapping(
        id,
        assignments(&[
            ("Supplier", ColumnType::OrgName),
            ("Contact email", ColumnType::Email),
        ]),
    )
    .unwrap();

    std::fs::write(
        src.join("payments.csv"),
        "Supplier,Contact email,Amount\nQuayside Marine Ltd,q@quay.example,1\n",
    )
    .unwrap();
    p.ingest().unwrap();
    let (id, state) = row(&p, "payments.csv");
    assert_eq!(state, Some(MappingState::Confirmed));
    assert_eq!(entity_keys(&p), ["org:quayside marine"]);

    std::fs::write(
        src.join("payments.csv"),
        "Supplier,Email address,Total\nQuayside Marine Ltd,q@quay.example,1\n",
    )
    .unwrap();
    p.ingest().unwrap();
    let (id2, state) = row(&p, "payments.csv");
    assert_eq!(id2, id);
    assert_eq!(state, Some(MappingState::NeedsReview));
    let view = p.mapping_view(id).unwrap().unwrap();
    let assigned: Vec<(&str, Option<ColumnType>)> = view
        .columns
        .iter()
        .map(|c| (c.key.as_str(), c.assigned))
        .collect();
    assert_eq!(
        assigned,
        [
            ("Supplier", Some(ColumnType::OrgName)),
            ("Email address", None),
            ("Total", None),
        ]
    );
    assert_eq!(
        entity_keys(&p),
        ["org:quayside marine"],
        "unchanged columns stay"
    );
    assert!(consistent(&p));
}

/// US-05.3, INV-CM-05: JSON keys are mapped by pointer with `*` for array items; a person
/// column inside an array item makes one mention per item.
#[test]
fn us_05_3_json_names_from_table() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &[("registry.json", REGISTRY)]);
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    let (id, _) = row(&p, "registry.json");
    let view = p.mapping_view(id).unwrap().unwrap();
    let cols: Vec<(&str, ColumnType)> = view
        .columns
        .iter()
        .map(|c| (c.key.as_str(), c.suggested))
        .collect();
    assert!(
        cols.contains(&("/companies/*/name", ColumnType::OrgName)),
        "{cols:?}"
    );
    assert!(
        cols.contains(&("/companies/*/officers/*/name", ColumnType::PersonName)),
        "{cols:?}"
    );
    let confirm: Vec<(&str, ColumnType)> = cols.clone();
    p.confirm_mapping(id, assignments(&confirm)).unwrap();
    assert_eq!(
        entity_keys(&p),
        [
            "org:brightwater logistics",
            "person:ann lee",
            "person:bob stone"
        ]
    );
    let occ = p.occurrences("person:ann lee").unwrap();
    assert_eq!(occ.len(), 1);
    assert!(matches!(occ[0].locator, Locator::JsonValue { .. }));
    assert!(p.source_context(occ[0].mention_id).unwrap().is_some());
    assert!(consistent(&p));
}

/// US-07.4 on entities, INV-CU-01/05: an ignored entity connection stays ignored through a
/// re-scan that changes the file and a full rebuild.
#[test]
fn ignore_on_an_entity_survives_rescan_and_rebuild() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(&src, &[("payments.csv", PAYMENTS), ("ledger.csv", LEDGER)]);
    let p = project(&tmp.path().join("p"), &src);
    p.ingest().unwrap();
    let mapping = assignments(&[("Supplier", ColumnType::OrgName)]);
    p.confirm_mapping(row(&p, "payments.csv").0, mapping.clone())
        .unwrap();
    p.confirm_mapping(row(&p, "ledger.csv").0, mapping).unwrap();
    let key = "org:brightwater logistics";
    let visible = |p: &Project| {
        p.connections_page(None, 100)
            .unwrap()
            .rows
            .iter()
            .any(|r| r.subject_key == key)
    };
    assert!(visible(&p));
    p.set_ignored(key, true).unwrap();
    assert!(!visible(&p));

    std::fs::write(
        src.join("ledger.csv"),
        format!("{LEDGER}Brightwater Logistics Ltd,x@y.example,1\n"),
    )
    .unwrap();
    p.ingest().unwrap();
    assert!(!visible(&p));
    assert!(consistent(&p));
    assert!(!visible(&p));
    p.set_ignored(key, false).unwrap();
    assert!(visible(&p));
}
