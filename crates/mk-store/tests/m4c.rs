//! M4c store checks: the table step (suggestions, confirmed mappings, entity mentions, record
//! links), the custom-pattern re-run, and derived rows equal to the reference model.

use std::collections::BTreeMap;
use std::path::Path;

use mk_corpus::{FileMeta, PathChange};
use mk_graph::DecisionKind;
use mk_graph::model::reference;
use mk_jobs::Trigger;
use mk_kernel::{Column, ContentHash, IdentifierKey, IdentifierKind, Locator, Span};
use mk_mapping::{Assignments, ColumnType, MappingState};
use mk_store::{
    DocumentFilter, DocumentOut, FoundMention, MentionOut, RerunStep, RescanChange, SegmentFinds,
    SegmentOut, StageAOutput, Store,
};

fn open(dir: &Path) -> Store {
    Store::open(&dir.join("project.db"), &dir.join("backups"))
        .unwrap()
        .0
}

fn hash(n: u8) -> ContentHash {
    ContentHash::from_bytes([n; 32])
}

/// A CSV document with `rows` (row 0 first); every cell holding `@` is an email mention.
fn csv(rows: &[&[&str]]) -> StageAOutput {
    let header: Vec<String> = rows[0].iter().map(|c| c.to_string()).collect();
    let names = mk_kernel::csv_columns(&header);
    let mut doc = DocumentOut {
        kind: "csv".into(),
        position: "whole".into(),
        parent: None,
        segments: Vec::new(),
    };
    let mut mentions = Vec::new();
    for (r, row) in rows.iter().enumerate() {
        let mut text = String::new();
        let mut starts = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            if i > 0 {
                text.push('\t');
            }
            starts.push(text.chars().count() as u32);
            text.push_str(cell);
        }
        let len = text.chars().count();
        for (i, cell) in row.iter().enumerate() {
            if !cell.contains('@') || r == 0 {
                continue;
            }
            let start = starts[i] as usize;
            let column = match names.get(i).cloned().flatten() {
                Some(n) => Column::Name(n),
                None => Column::Index(i as u32),
            };
            let locator = Locator::CsvCell {
                row: r as u64,
                column,
                span: Span::new(start, start + cell.chars().count(), len).unwrap(),
            };
            mentions.push(MentionOut {
                document: 0,
                segment: r,
                locator,
                raw: cell.to_string(),
                key: IdentifierKey::new(IdentifierKind::Email, *cell),
                confidence: 1.0,
            });
        }
        doc.segments.push(SegmentOut {
            locator: Locator::CsvRow { row: r as u64 },
            text,
            malformed: false,
            cell_starts: starts,
            ocr: None,
        });
    }
    StageAOutput {
        documents: vec![doc],
        mentions,
        card_flags: Vec::new(),
        warnings: Vec::new(),
    }
}

/// A JSON document with `(pointer, value)` segments; values holding `@` are email mentions.
fn json(values: &[(&str, &str)]) -> StageAOutput {
    let mut doc = DocumentOut {
        kind: "json".into(),
        position: "whole".into(),
        parent: None,
        segments: Vec::new(),
    };
    let mut mentions = Vec::new();
    for (i, (pointer, value)) in values.iter().enumerate() {
        let len = value.chars().count();
        let locator = Locator::JsonValue {
            pointer: pointer.to_string(),
            span: Span::new(0, len, len).unwrap(),
        };
        if value.contains('@') {
            mentions.push(MentionOut {
                document: 0,
                segment: i,
                locator: locator.clone(),
                raw: value.to_string(),
                key: IdentifierKey::new(IdentifierKind::Email, *value),
                confidence: 1.0,
            });
        }
        doc.segments.push(SegmentOut {
            locator,
            text: value.to_string(),
            malformed: false,
            cell_starts: Vec::new(),
            ocr: None,
        });
    }
    StageAOutput {
        documents: vec![doc],
        mentions,
        card_flags: Vec::new(),
        warnings: Vec::new(),
    }
}

/// Runs the Stage A items a re-scan queued, committing `output` for each.
fn run_items(store: &Store, mut output: impl FnMut(i64) -> StageAOutput) {
    for w in store.pending_items().unwrap() {
        let started = store.start_item(w.item.id).unwrap().unwrap();
        store
            .commit_stage_a(w.item.id, output(started.content))
            .unwrap();
    }
}

/// Adds `rel` under `root` with content `n` and commits `output` for it.
fn add(store: &Store, root: i64, rel: &str, n: u8, file_type: &str, output: StageAOutput) {
    store
        .apply_rescan(
            vec![RescanChange {
                change: PathChange::New {
                    root,
                    rel_path: rel.into(),
                    meta: FileMeta {
                        size: 10,
                        mtime_ns: 1,
                    },
                    hash: hash(n),
                },
                file_type: file_type.into(),
            }],
            Trigger::Add,
        )
        .unwrap();
    let mut output = Some(output);
    run_items(store, |_| output.take().unwrap());
}

/// The documents list row of `rel`: `(ref id, mapping state)`.
fn row(store: &Store, rel: &str) -> (i64, Option<MappingState>) {
    let rows = store
        .documents_page(&DocumentFilter::default(), None, 100)
        .unwrap();
    let r = rows.iter().find(|r| r.rel_path == rel).unwrap();
    (r.ref_id, r.mapping_state)
}

fn entity_count(store: &Store) -> i64 {
    store
        .read_raw(|c| Ok(c.query_row("SELECT COUNT(*) FROM entity_mention", [], |r| r.get(0))?))
        .unwrap()
}

/// The derived rows equal the reference model (ADR-0005), before and after a full rebuild.
fn assert_derived_matches(store: &Store) {
    let expected = reference(&store.facts().unwrap());
    assert_eq!(store.derived_state().unwrap(), expected);
    store.rebuild_all().unwrap();
    assert_eq!(store.derived_state().unwrap(), expected);
}

const LEDGER: &[&[&str]] = &[
    &["date", "supplier", "contact", "amount"],
    &[
        "2024-01-02",
        "Brightwater Logistics Ltd",
        "ap@brightwater.example",
        "100",
    ],
    &[
        "2024-01-09",
        "Kestrel Advisory LLP",
        "fees@kestrel.example",
        "250",
    ],
];

const PAYMENTS: &[&[&str]] = &[
    &["date", "supplier", "contact", "amount"],
    &[
        "2024-02-02",
        "BRIGHTWATER LOGISTICS LIMITED",
        "ap@brightwater.example",
        "90",
    ],
];

fn confirmed() -> Assignments {
    Assignments(BTreeMap::from([
        ("date".to_string(), ColumnType::Date),
        ("supplier".to_string(), ColumnType::OrgName),
        ("contact".to_string(), ColumnType::Email),
        ("amount".to_string(), ColumnType::Ignore),
    ]))
}

/// US-05.1 AC1/AC3, INV-CM-02/03: every column gets a suggestion and samples; suggestions make
/// no entities.
#[test]
fn us_05_1_suggestions_make_no_entities() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let root = store.add_source_root("/data", "folder").unwrap();
    add(&store, root, "ledger.csv", 1, "csv", csv(LEDGER));
    let (ref_id, state) = row(&store, "ledger.csv");
    assert_eq!(state, Some(MappingState::Suggested));
    let view = store.mapping_view(ref_id).unwrap().unwrap();
    let got: Vec<(&str, ColumnType)> = view
        .columns
        .iter()
        .map(|c| (c.key.as_str(), c.suggested))
        .collect();
    assert_eq!(
        got,
        [
            ("date", ColumnType::Date),
            ("supplier", ColumnType::OrgName),
            ("contact", ColumnType::Email),
            ("amount", ColumnType::Ignore),
        ]
    );
    assert_eq!(view.columns[1].samples.len(), 2);
    assert!(view.columns.iter().all(|c| c.assigned.is_none()));
    assert_eq!(view.column_count, 4);
    assert_eq!(entity_count(&store), 0);
    assert!(store.derived_state().unwrap().entities.is_empty());
}

/// US-05.2 AC1/AC2, US-05.3 AC1/AC2, INV-CM-05: confirm = decision + apply; a file with the
/// same headers is offered the mapping; the entity is one connection across both files; every
/// mention points into its cell; record links join it to the row's identifiers.
#[test]
fn us_05_2_confirm_and_reuse_make_table_entities() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let root = store.add_source_root("/data", "folder").unwrap();
    add(&store, root, "ledger.csv", 1, "csv", csv(LEDGER));
    add(&store, root, "payments.csv", 2, "csv", csv(PAYMENTS));
    let (ledger, _) = row(&store, "ledger.csv");
    let (payments, _) = row(&store, "payments.csv");

    store.confirm_mapping(ledger, confirmed()).unwrap();
    assert_eq!(row(&store, "ledger.csv").1, Some(MappingState::Confirmed));
    assert_eq!(entity_count(&store), 2);
    // One file only: not a connection yet (INV-LK-01).
    let key = "org:brightwater logistics";
    assert!(!store.derived_state().unwrap().connections.contains_key(key));

    let view = store.mapping_view(payments).unwrap().unwrap();
    assert_eq!(view.state, MappingState::Suggested);
    assert_eq!(view.reuse.len(), 1);
    assert_eq!(view.reuse[0].ref_id, ledger);
    assert_eq!(view.reuse[0].assignments, confirmed());
    let (_, changed) = store
        .confirm_mapping(payments, view.reuse[0].assignments.clone())
        .unwrap();
    assert!(changed);

    let derived = store.derived_state().unwrap();
    let conn = &derived.connections[key];
    assert_eq!(conn.source_count, 2);
    assert_eq!(conn.subject_kind, mk_graph::SubjectKind::Entity);
    let entity = &derived.entities[key];
    assert!(entity.from_table);
    assert_eq!(entity.entity_type, "org");
    // A tie (one each): A→Z.
    assert_eq!(entity.display_name, "BRIGHTWATER LOGISTICS LIMITED");

    let occurrences = store.mentions_of(key, 10, false).unwrap();
    assert_eq!(occurrences.len(), 2);
    for o in &occurrences {
        assert!(o.mention_id < 0);
        let Locator::CsvCell { row, column, span } = &o.locator else {
            panic!("{:?}", o.locator);
        };
        assert_eq!(*row, 1);
        assert_eq!(*column, Column::Name("supplier".into()));
        let back = store.mention(o.mention_id).unwrap().unwrap();
        assert_eq!(back.raw, o.raw);
        let window = store.segment_window(o.segment_id, 0, 0).unwrap().unwrap();
        let text: String = window.segments[0]
            .text
            .chars()
            .skip(span.start())
            .take(span.len())
            .collect();
        assert_eq!(text, o.raw);
        assert_eq!(
            store
                .mentions_in_document(key, o.document_id, false)
                .unwrap(),
            vec![o.mention_id]
        );
    }

    let ledger_content = store.content(&hash(1)).unwrap().unwrap().id;
    let links: Vec<(String, String, String)> = store
        .record_links(ledger_content)
        .unwrap()
        .into_iter()
        .map(|l| (l.record, l.name_key, l.identifier_key))
        .collect();
    assert_eq!(
        links,
        [
            (
                "1".into(),
                key.into(),
                "email:ap@brightwater.example".into()
            ),
            (
                "2".into(),
                "org:kestrel advisory".into(),
                "email:fees@kestrel.example".into()
            ),
        ]
    );
    assert_derived_matches(&store);
}

/// US-05.2 AC3, D5: a re-scan with the same headers applies the mapping again; changed headers
/// make it Needs review with the columns still there kept.
#[test]
fn us_05_2_ac3_rescan_reapplies_or_needs_review() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let root = store.add_source_root("/data", "folder").unwrap();
    add(&store, root, "ledger.csv", 1, "csv", csv(LEDGER));
    let (ledger, _) = row(&store, "ledger.csv");
    store.confirm_mapping(ledger, confirmed()).unwrap();

    let change = |old: u8, new: u8| RescanChange {
        change: PathChange::Changed {
            root,
            rel_path: "ledger.csv".into(),
            meta: FileMeta {
                size: 11,
                mtime_ns: 2,
            },
            old: hash(old),
            new: hash(new),
        },
        file_type: "csv".into(),
    };
    // Same headers, new rows.
    let more: Vec<&[&str]> = LEDGER
        .iter()
        .copied()
        .chain([&[
            "2024-03-01",
            "Saltmarsh Holdings S.A.",
            "x@salt.example",
            "1",
        ][..]])
        .collect();
    store
        .apply_rescan(vec![change(1, 3)], Trigger::Rescan)
        .unwrap();
    run_items(&store, |_| csv(&more));
    assert_eq!(row(&store, "ledger.csv").1, Some(MappingState::Confirmed));
    assert_eq!(entity_count(&store), 3);

    // A column renamed: the others keep their types; the new one is only suggested.
    let renamed: &[&[&str]] = &[
        &["date", "vendor", "contact", "amount"],
        &[
            "2024-01-02",
            "Brightwater Logistics Ltd",
            "ap@brightwater.example",
            "100",
        ],
    ];
    store
        .apply_rescan(vec![change(3, 4)], Trigger::Rescan)
        .unwrap();
    run_items(&store, |_| csv(renamed));
    let (ref_id, state) = row(&store, "ledger.csv");
    assert_eq!(state, Some(MappingState::NeedsReview));
    assert_eq!(entity_count(&store), 0);
    let view = store.mapping_view(ref_id).unwrap().unwrap();
    let assigned: Vec<Option<ColumnType>> = view.columns.iter().map(|c| c.assigned).collect();
    assert_eq!(
        assigned,
        [
            Some(ColumnType::Date),
            None,
            Some(ColumnType::Email),
            Some(ColumnType::Ignore)
        ]
    );
    assert_eq!(view.columns[1].suggested, ColumnType::OrgName);
    assert_derived_matches(&store);
}

/// INV-CM-05 (M4c): a JSON record is the nearest array item, else the whole top-level object.
#[test]
fn inv_cm_05_json_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let root = store.add_source_root("/data", "folder").unwrap();
    let values = [
        ("/name", "Brightwater Logistics Ltd"),
        ("/contact/email", "info@brightwater.example"),
        ("/officers/0/name", "Daniel Okonkwo"),
        ("/officers/0/email", "d.okonkwo@brightwater.example"),
        ("/officers/1/name", "Irene Vasquez"),
    ];
    add(&store, root, "registry.json", 1, "json", json(&values));
    let (ref_id, _) = row(&store, "registry.json");
    let view = store.mapping_view(ref_id).unwrap().unwrap();
    let suggested: BTreeMap<String, ColumnType> = view
        .columns
        .iter()
        .map(|c| (c.key.clone(), c.suggested))
        .collect();
    assert_eq!(suggested["/officers/*/name"], ColumnType::PersonName);
    assert_eq!(suggested["/contact/email"], ColumnType::Email);
    let mut assignments = Assignments(suggested);
    assignments.0.insert("/name".into(), ColumnType::OrgName);
    store.confirm_mapping(ref_id, assignments).unwrap();

    let content = store.content(&hash(1)).unwrap().unwrap().id;
    let links: Vec<(String, String, String)> = store
        .record_links(content)
        .unwrap()
        .into_iter()
        .map(|l| (l.record, l.name_key, l.identifier_key))
        .collect();
    assert_eq!(
        links,
        [
            (
                "".into(),
                "org:brightwater logistics".into(),
                "email:info@brightwater.example".into()
            ),
            (
                "/officers/0".into(),
                "person:daniel okonkwo".into(),
                "email:d.okonkwo@brightwater.example".into()
            ),
        ]
    );
    let occurrences = store
        .mentions_of("person:irene vasquez", 10, false)
        .unwrap();
    assert!(matches!(
        &occurrences[0].locator,
        Locator::JsonValue { pointer, .. } if pointer == "/officers/1/name"
    ));
    assert_derived_matches(&store);
}

/// US-04.7 AC3, INV-ID-08: a custom re-run replaces only custom mentions; built-in mention ids
/// stay; the pattern set is stored and counted.
#[test]
fn us_04_7_ac3_custom_rerun_leaves_builtin_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let root = store.add_source_root("/data", "folder").unwrap();
    let rows: &[&[&str]] = &[
        &["ref", "contact"],
        &["VPA-TND-2024-017", "a@x.example"],
        &["VPA-TND-2024-018", "b@x.example"],
    ];
    add(&store, root, "a.csv", 1, "csv", csv(rows));
    let content = store.content(&hash(1)).unwrap().unwrap().id;
    let ids = |store: &Store| -> Vec<i64> {
        store
            .read_raw(|c| {
                let mut stmt = c.prepare(
                    "SELECT id FROM identifier_mention WHERE found_by = 'builtin' ORDER BY id",
                )?;
                Ok(stmt
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<i64>, _>>()?)
            })
            .unwrap()
    };
    let before = ids(&store);

    store
        .set_custom_patterns(vec![("tender_ref".into(), r"VPA-TND-\d{4}-\d{3}".into())])
        .unwrap();
    assert_eq!(
        store.rerun_state(RerunStep::Custom).unwrap(),
        (None, Some(0))
    );
    let find = |seg: &mk_store::StoredSegment| {
        let mut finds = SegmentFinds::default();
        let len = seg.text.chars().count();
        if let Some(i) = seg.text.find("VPA-") {
            let raw = &seg.text[i..i + 16];
            let Locator::CsvRow { row } = seg.locator else {
                return finds;
            };
            finds.mentions.push(FoundMention {
                locator: Locator::CsvCell {
                    row,
                    column: Column::Name("ref".into()),
                    span: Span::new(0, 16, len).unwrap(),
                },
                raw: raw.into(),
                key: IdentifierKey::custom("tender_ref", raw).unwrap(),
                confidence: 1.0,
            });
        }
        finds
    };
    let out = store
        .rerun(RerunStep::Custom, 0, content, find)
        .unwrap()
        .unwrap();
    assert_eq!(out.mentions, 2);
    assert!(
        store
            .finish_rerun(RerunStep::Custom, Some(content), "h1")
            .unwrap()
    );
    assert_eq!(
        store.rerun_state(RerunStep::Custom).unwrap(),
        (Some("h1".into()), None)
    );
    assert_eq!(ids(&store), before);
    assert_eq!(
        store.pattern_counts().unwrap(),
        [(
            "tender_ref".to_string(),
            r"VPA-TND-\d{4}-\d{3}".to_string(),
            2
        )]
    );
    assert_eq!(store.settings().unwrap().custom_patterns.len(), 1);

    // A built-in re-run leaves the custom mentions alone in turn.
    store.begin_rerun(RerunStep::Builtin).unwrap();
    store
        .rerun(RerunStep::Builtin, 0, content, |_| SegmentFinds::default())
        .unwrap()
        .unwrap();
    assert_eq!(store.pattern_counts().unwrap()[0].2, 2);
    // A finder of the wrong step is refused.
    store.begin_rerun(RerunStep::Builtin).unwrap();
    assert!(store.rerun(RerunStep::Builtin, 0, content, find).is_err());
    assert_derived_matches(&store);
}

/// US-07.4 on entities: an ignore decision on a name key survives a re-scan and a rebuild.
#[test]
fn ignore_entity_survives_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(dir.path());
    let root = store.add_source_root("/data", "folder").unwrap();
    add(&store, root, "ledger.csv", 1, "csv", csv(LEDGER));
    add(&store, root, "payments.csv", 2, "csv", csv(PAYMENTS));
    for rel in ["ledger.csv", "payments.csv"] {
        store
            .confirm_mapping(row(&store, rel).0, confirmed())
            .unwrap();
    }
    let key = "org:brightwater logistics";
    store
        .append_decision(DecisionKind::IgnoreEntity, key, None, None)
        .unwrap();
    assert!(store.derived_state().unwrap().connections[key].ignored);
    assert_derived_matches(&store);
    assert!(store.derived_state().unwrap().connections[key].ignored);
    // Removing one file drops the connection; the decision stays Active on the entity.
    store
        .remove_file_refs(vec![row(&store, "payments.csv").0])
        .unwrap();
    let derived = store.derived_state().unwrap();
    assert!(!derived.connections.contains_key(key));
    assert!(derived.entities.contains_key(key));
    assert_derived_matches(&store);
}
