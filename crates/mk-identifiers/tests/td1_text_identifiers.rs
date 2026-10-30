//! US-04.1 – US-04.5 against the TD-1 ground truth: every built-in identifier in the UTF-8
//! TXT/MD notes (domains derived from emails and URLs included) is found at its line and
//! offsets, and nothing else is: the bad-checksum negatives there are not found either. The
//! notes are synthetic, so they are read with region GB, as `mangkon-cli bench` reads them
//! (`docs/07-test-corpus.md` §8). Other formats are scored end to end by the bench. Skipped
//! without `testdata/` unless `MK_REQUIRE_TESTDATA=1` (as in `mk-bench`).

use std::collections::BTreeSet;
use std::path::PathBuf;

use mk_bench::{IdType, Location, Negative, Subject};
use mk_identifiers::{Region, Rules};

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata")
}

#[test]
fn us_04_x_td1_text_identifiers_match_ground_truth() {
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
    let rules = Rules {
        phone_region: Region::parse("GB"),
    };
    let mut checked = 0;
    let mut kinds = BTreeSet::new();
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
            .filter(|m| m.document == doc.id)
            .filter_map(|m| match (&m.subject, &m.location) {
                (Subject::Identifier { key }, Location::TextLine { line, start, end })
                    if key.id_type != IdType::Custom =>
                {
                    let t = serde_json::to_value(key.id_type).unwrap();
                    Some((
                        *line,
                        *start,
                        *end,
                        format!("{}:{}", t.as_str().unwrap(), key.value),
                    ))
                }
                _ => None,
            })
            .collect();
        let bytes = std::fs::read(testdata().join("td1").join(&file.path)).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap();
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut got = BTreeSet::new();
        let mut cards = BTreeSet::new();
        for (i, line) in text.split('\n').enumerate() {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let found = mk_identifiers::find_all(line, &rules);
            for h in found.hits {
                kinds.insert(h.key.kind());
                got.insert((
                    i as u32 + 1,
                    h.span.start() as u32,
                    h.span.end() as u32,
                    h.key.to_string(),
                ));
            }
            for c in found.cards {
                cards.insert((i as u32 + 1, c.span.start() as u32, c.span.end() as u32));
            }
        }
        assert_eq!(got, want, "{}", file.path);
        let want_cards: BTreeSet<(u32, u32, u32)> = gt
            .negatives
            .iter()
            .filter_map(|n| match n {
                Negative::CardLike {
                    document,
                    location: Location::TextLine { line, start, end },
                    ..
                } if *document == doc.id => Some((*line, *start, *end)),
                _ => None,
            })
            .collect();
        assert_eq!(cards, want_cards, "{}", file.path);
        checked += 1;
    }
    assert!(checked >= 4, "only {checked} UTF-8 notes found");
    assert!(kinds.len() >= 6, "only {kinds:?} found in the notes");
}
