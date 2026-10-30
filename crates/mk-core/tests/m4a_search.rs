//! M4a keyword search over the real pipeline (US-06.1, INV-SR-01, NFR-SEC-14, domain D4): the
//! index follows ingest, re-scan and removal (ADR-0010); every result opens its source with the
//! hit marked, for every locator kind on TD-1. The TD-1 cases are skipped without `testdata/`
//! unless `MK_REQUIRE_TESTDATA=1`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use mk_core::{
    Marked, PageStatus, Project, ProjectError, SearchFilter, SearchHit, SourceBody, SourceContext,
    WorkerConfig,
};

fn project(dir: &Path) -> Project {
    let (mut project, _) = Project::open(dir).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.set_tessdata_dir(Some(dir.join("tessdata")));
    project
}

fn all() -> SearchFilter {
    SearchFilter::default()
}

fn hits(p: &Project, q: &str, filter: &SearchFilter) -> Vec<SearchHit> {
    p.search(q, filter, None, 500).unwrap().hits
}

fn snippet_text(h: &SearchHit) -> String {
    h.snippet.iter().map(|r| r.text.as_str()).collect()
}

/// Every marked text of a body.
fn texts(body: &SourceBody) -> Vec<&Marked> {
    match body {
        SourceBody::Text { lines } => lines.iter().map(|l| &l.text).collect(),
        SourceBody::Table { rows, .. } => rows.iter().flat_map(|r| &r.cells).collect(),
        SourceBody::Paragraphs { paragraphs } => paragraphs.iter().collect(),
        SourceBody::Json { rows } => rows.iter().filter_map(|r| r.value.as_ref()).collect(),
        SourceBody::Email { headers, parts, .. } => headers
            .iter()
            .map(|h| &h.value)
            .chain(parts.iter().flat_map(|p| &p.lines))
            .collect(),
        SourceBody::Page { lines, .. } => lines.iter().collect(),
    }
}

/// The highlighted text of a view.
fn marked(view: &SourceContext) -> String {
    texts(&view.body)
        .into_iter()
        .map(|m| m.hit.as_str())
        .collect()
}

fn open_hit(p: &Project, h: &SearchHit) -> SourceContext {
    let span = h.hit.map(|x| (x.span.start(), x.span.end()));
    p.segment_context(h.segment_id, span).unwrap().unwrap()
}

fn segment_count(p: &Project) -> u64 {
    p.store()
        .read_raw(|c| Ok(c.query_row("SELECT COUNT(*) FROM segment", [], |r| r.get::<_, i64>(0))?))
        .unwrap() as u64
}

fn index_in_step(p: &Project) {
    assert_eq!(
        p.store().search_index_rows().unwrap(),
        segment_count(p),
        "index rows"
    );
    p.store().check_search_index().unwrap();
}

/// ADR-0010: the index holds exactly the segments after ingest, a changed file, a deleted file
/// and a removed path; old text stops matching.
#[test]
fn fts_follows_ingest_rescan_and_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "alpha harbour\nsecond line\n").unwrap();
    std::fs::write(src.join("b.txt"), "bravo harbour\n").unwrap();
    std::fs::write(src.join("c.csv"), "name,city\nZoë,Lisbon\n").unwrap();
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    index_in_step(&p);
    assert_eq!(hits(&p, "harbour", &all()).len(), 2);
    assert_eq!(hits(&p, "zoe lisbon", &all()).len(), 1);

    // Changed: the old text goes, the new text comes.
    std::fs::write(src.join("a.txt"), "charlie harbour\n").unwrap();
    p.ingest().unwrap();
    index_in_step(&p);
    assert!(hits(&p, "alpha", &all()).is_empty());
    assert_eq!(hits(&p, "charlie", &all()).len(), 1);

    // Removed from the project: its text no longer matches (INV-CO-08).
    let b_ref: i64 = p
        .store()
        .read_raw(|c| {
            Ok(c.query_row(
                "SELECT id FROM file_ref WHERE rel_path = 'b.txt'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    p.remove_files(&[b_ref]).unwrap();
    index_in_step(&p);
    assert!(hits(&p, "bravo", &all()).is_empty());

    let rebuilt = p.rebuild().unwrap();
    assert!(rebuilt.search_index_in_step, "{rebuilt:?}");
    index_in_step(&p);
    assert!(p.check_health().unwrap().is_healthy());
}

/// Domain D4: a document whose file is gone still shows in search, marked missing; it opens
/// from the stored text.
#[test]
fn d4_missing_documents_badged() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("gone.txt"), "a ledger entry\n").unwrap();
    std::fs::write(src.join("kept.txt"), "another ledger\n").unwrap();
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    std::fs::remove_file(src.join("gone.txt")).unwrap();
    p.ingest().unwrap();
    let found = hits(&p, "ledger", &all());
    assert_eq!(found.len(), 2);
    let gone = found
        .iter()
        .find(|h| h.path.as_deref().is_some_and(|x| x.ends_with("gone.txt")))
        .expect("the missing file still matches, with its last path");
    assert!(gone.missing);
    assert!(
        found
            .iter()
            .any(|h| !h.missing && h.path.as_deref().is_some_and(|x| x.ends_with("kept.txt")))
    );
    assert_eq!(marked(&open_hit(&p, gone)).to_lowercase(), "ledger");
}

/// US-06.1 AC5 and the query errors: no matches is an empty page; a query with nothing to
/// search for is a plain message.
#[test]
fn us_06_1_ac5_no_matches() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "something\n").unwrap();
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    let page = p.search("zzqqxx nothing", &all(), None, 50).unwrap();
    assert!(page.hits.is_empty() && page.next.is_none());
    for q in ["", "OR", "\"\"", "--"] {
        let Err(ProjectError::Invalid(m)) = p.search(q, &all(), None, 50) else {
            panic!("{q:?} should be refused");
        };
        assert_eq!(m, "Search needs at least one word");
    }
}

// ---- TD-1 ----

struct Td1 {
    _tmp: tempfile::TempDir,
    project: Project,
}

fn td1() -> Option<&'static Td1> {
    static F: OnceLock<Option<Td1>> = OnceLock::new();
    F.get_or_init(|| {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td1/synthetic");
        if !dir.is_dir() {
            assert!(
                std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
                "testdata/td1 is missing"
            );
            eprintln!("skipped: testdata/td1 is missing (see docs/07-test-corpus.md §9)");
            return None;
        }
        let tmp = tempfile::tempdir().unwrap();
        let project = project(&tmp.path().join("p"));
        project.add_source_root(&dir).unwrap();
        project.ingest().unwrap();
        Some(Td1 { _tmp: tmp, project })
    })
    .as_ref()
}

/// A word of at least 6 ASCII letters from a segment of each locator kind, with its file type;
/// from a file of its own (not an attachment) when there is one.
fn words_by_kind(p: &Project) -> Vec<(String, String, String)> {
    p.store()
        .read_raw(|c| {
            let mut stmt = c.prepare(
                "SELECT s.loc_kind, c.file_type, s.text FROM segment s
                 JOIN document d ON d.id = s.document_id JOIN content c ON c.id = d.content_id
                 ORDER BY d.parent_id IS NOT NULL, s.id",
            )?;
            let mut rows = stmt.query([])?;
            let mut seen = BTreeSet::new();
            let mut out = Vec::new();
            while let Some(r) = rows.next()? {
                let (kind, file_type, text): (String, String, String) =
                    (r.get(0)?, r.get(1)?, r.get(2)?);
                if seen.contains(&kind) {
                    continue;
                }
                let word = text
                    .split(|c: char| !c.is_ascii_alphabetic())
                    .find(|w| w.len() >= 6);
                if let Some(w) = word {
                    seen.insert(kind.clone());
                    out.push((kind, file_type, w.to_string()));
                }
            }
            Ok(out)
        })
        .unwrap()
}

/// US-06.1 AC1: every result names its document and place and shows a snippet with the hit;
/// INV-SR-01: each one is a stored segment.
#[test]
fn us_06_1_ac1_results_have_document_locator_snippet() {
    let Some(f) = td1() else { return };
    let p = &f.project;
    let found = hits(p, "Okonkwo", &all());
    assert!(found.len() >= 3, "{}", found.len());
    let types: BTreeSet<&str> = found.iter().map(|h| h.file_type.as_str()).collect();
    assert!(types.len() >= 2, "{types:?}");
    for h in &found {
        assert!(h.path.is_some(), "{h:?}");
        assert!(!h.missing);
        let hit: String = h
            .snippet
            .iter()
            .filter(|r| r.hit)
            .map(|r| r.text.as_str())
            .collect();
        assert!(hit.to_lowercase().contains("okonkwo"), "{h:?}");
        assert!(h.hit.is_some());
        assert!(snippet_text(h).chars().count() <= mk_search::SNIPPET_CHARS + 2);
    }
}

/// US-06.1 AC3: a file-type filter and a one-document filter.
#[test]
fn us_06_1_ac3_filters() {
    let Some(f) = td1() else { return };
    let p = &f.project;
    let every = hits(p, "Okonkwo", &all());
    let csv = SearchFilter {
        file_types: vec!["csv".into()],
        document: None,
    };
    let only_csv = hits(p, "Okonkwo", &csv);
    assert!(!only_csv.is_empty() && only_csv.len() < every.len());
    assert!(only_csv.iter().all(|h| h.file_type == "csv"));
    let doc = every[0].document_id;
    let one = SearchFilter {
        file_types: Vec::new(),
        document: Some(doc),
    };
    let in_doc = hits(p, "Okonkwo", &one);
    assert!(!in_doc.is_empty());
    assert!(in_doc.iter().all(|h| h.document_id == doc));
    assert_eq!(
        in_doc.len(),
        every.iter().filter(|h| h.document_id == doc).count()
    );
}

/// US-06.1 AC4, US-08.1: one result of each locator kind opens its source with the word
/// marked; page hits get their boxes on the rendered page.
#[test]
fn us_06_1_ac4_hit_opens_highlighted() {
    let Some(f) = td1() else { return };
    let p = &f.project;
    let kinds = words_by_kind(p);
    let names: BTreeSet<&str> = kinds.iter().map(|(k, _, _)| k.as_str()).collect();
    for want in [
        "text_line",
        "csv_row",
        "docx_paragraph",
        "html_text",
        "json_value",
        "email_header",
        "mime_part",
        "pdf_text",
        "ocr_text",
    ] {
        assert!(names.contains(want), "TD-1 has a {want} segment: {names:?}");
    }
    let mut boxed = 0;
    for (kind, file_type, word) in &kinds {
        let filter = SearchFilter {
            file_types: vec![file_type.clone()],
            document: None,
        };
        let found = hits(p, word, &filter);
        let h = found
            .iter()
            .find(|h| h.hit.is_some())
            .unwrap_or_else(|| panic!("{kind}: no hit for {word}"));
        let view = open_hit(p, h);
        assert_eq!(
            marked(&view).to_lowercase(),
            word.to_lowercase(),
            "{kind} {word}"
        );
        assert_eq!(view.hit, h.hit, "{kind}");
        assert!(view.occurrence.is_none());
        if matches!(kind.as_str(), "pdf_text" | "ocr_text") {
            // The first hit on a page that renders (an attachment's pages show as text).
            for h in &found {
                let v = open_hit(p, h);
                let SourceBody::Page { page, .. } = v.body else {
                    continue;
                };
                let pv = p.page_view_at(v.document.id, page, v.hit).unwrap().unwrap();
                if pv.status == PageStatus::Shown {
                    assert!(!pv.boxes.is_empty(), "{kind} {word}: no boxes");
                    boxed += 1;
                    break;
                }
            }
        }
    }
    assert!(
        boxed >= 2,
        "PDF text and OCR pages placed their boxes ({boxed})"
    );
}

/// NFR-SEC-14: card-like numbers never show unmasked in search snippets, though the stored
/// text keeps them (INV-ID-06) and they can be searched for.
#[test]
fn nfr_sec_14_search_snippets_mask_card_like() {
    let Some(f) = td1() else { return };
    let p = &f.project;
    let mut shown = Vec::new();
    for q in ["5555555555554444", "card", "4111", "expense", "claim"] {
        shown.extend(hits(p, q, &all()));
    }
    let csv: Vec<&SearchHit> = shown
        .iter()
        .filter(|h| snippet_text(h).contains("4444") && h.file_type == "csv")
        .collect();
    assert!(!csv.is_empty(), "the expense CSV's card is found");
    for h in &shown {
        let text = snippet_text(h);
        assert_eq!(mk_search::mask_card_like(&text), text, "unmasked in {text}");
        assert!(!text.contains("5555555555554444"), "{text}");
        assert!(!text.contains("4111 1111 1111 1111"), "{text}");
    }
    assert!(
        shown
            .iter()
            .any(|h| snippet_text(h).contains("************4444")),
        "masked form shown"
    );
}
