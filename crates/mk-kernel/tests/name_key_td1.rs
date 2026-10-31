//! US-11.1: the kernel's NameKey gives the ground truth's name key for every TD-1 entity
//! mention in a CSV or JSON file, and for every entity name and alias of the story (the
//! reference rules are `mk-bench` `keys.rs`). Skipped without `testdata/` unless
//! `MK_REQUIRE_TESTDATA=1`.

use std::path::PathBuf;

use mk_bench::{EntityKind, Subject};
use mk_kernel::{EntityType, NameKey};

fn entity_type(kind: EntityKind) -> EntityType {
    match kind {
        EntityKind::Person => EntityType::Person,
        EntityKind::Org => EntityType::Org,
    }
}

#[test]
fn us_11_1_name_key_agrees_with_ground_truth() {
    let gt_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td1/ground-truth.json");
    if !gt_path.exists() {
        assert!(
            std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
            "testdata/td1 is missing"
        );
        eprintln!("skipped: testdata/td1 is missing (see docs/07-test-corpus.md §9)");
        return;
    }
    let gt = mk_bench::load(&gt_path).expect("ground truth loads");
    let mut checked = 0;
    for e in &gt.entities {
        for raw in std::iter::once(&e.canonical).chain(&e.aliases) {
            let t = entity_type(e.kind);
            let want = mk_bench::keys::name_key(e.kind, raw);
            assert_eq!(NameKey::new(t, raw).unwrap().to_string(), want, "{raw}");
            checked += 1;
        }
    }
    let tables: Vec<&str> = gt
        .documents
        .iter()
        .filter(|d| d.file.ends_with(".csv") || d.file.ends_with(".json"))
        .map(|d| d.id.as_str())
        .collect();
    for m in &gt.mentions {
        let Subject::Entity { entity, name_key } = &m.subject else {
            continue;
        };
        if !tables.contains(&m.document.as_str()) {
            continue;
        }
        let kind = gt.entities.iter().find(|e| &e.id == entity).unwrap().kind;
        let got = NameKey::new(entity_type(kind), &m.raw).unwrap();
        assert_eq!(&got.to_string(), name_key, "{}", m.raw);
        checked += 1;
    }
    assert!(checked > 50, "{checked}");
}
