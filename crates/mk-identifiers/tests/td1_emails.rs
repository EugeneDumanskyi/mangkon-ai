//! US-04.1 AC1 against the TD-1 ground truth: every email in the UTF-8 TXT/MD notes is found at
//! its line and offsets, and nothing else is. Skipped without `testdata/` unless
//! `MK_REQUIRE_TESTDATA=1` (as in `mk-bench`). Other encodings wait for M3; CSV cells are checked
//! end to end in `mk-core`.

use std::collections::BTreeSet;
use std::path::PathBuf;

use mk_bench::{IdType, Location, Subject};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn us_04_1_ac1_td1_text_emails_match_ground_truth() {
    let gt_path = testdata().join("td1/ground-truth.json");
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
    for file in &gt.files {
        let utf8 = matches!(file.encoding.as_deref(), Some("utf-8" | "utf-8-bom"));
        let text_kind = file.path.ends_with(".txt") || file.path.ends_with(".md");
        if !utf8 || !text_kind {
            continue;
        }
        let doc = gt
            .documents
            .iter()
            .find(|d| d.file == file.path)
            .expect("document for file");
        let want: BTreeSet<(u32, u32, u32, String)> = gt
            .mentions
            .iter()
            .filter(|m| m.document == doc.id && m.derived_from.is_none())
            .filter_map(|m| match (&m.subject, &m.location) {
                (Subject::Identifier { key }, Location::TextLine { line, start, end })
                    if key.id_type == IdType::Email =>
                {
                    Some((*line, *start, *end, key.value.clone()))
                }
                _ => None,
            })
            .collect();
        let bytes = std::fs::read(testdata().join("td1").join(&file.path)).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut got = BTreeSet::new();
        for (i, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            for h in mk_identifiers::find_emails(line) {
                got.insert((
                    i as u32 + 1,
                    h.span.start() as u32,
                    h.span.end() as u32,
                    h.key.value().to_string(),
                ));
            }
        }
        assert_eq!(got, want, "{}", file.path);
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} UTF-8 notes found");
}
