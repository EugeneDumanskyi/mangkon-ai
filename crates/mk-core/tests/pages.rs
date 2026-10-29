//! PDFs and images through the real Stage A lane and the page view: the per-page deadline
//! (ADR-0017), the project's OCR languages (US-02.2 AC4, ADR-0018), and what the page view
//! shows when the original is there, changed or gone (US-08.1 AC1–AC2, ADR-0011).

use std::path::Path;
use std::time::Duration;

use mk_core::{PageStatus, Project, ProjectError, SourceBody, WorkerConfig};
use mk_corpus::IngestionState;
use mk_parsers::image::GrayImage;

fn project(dir: &Path, src: &Path) -> Project {
    let (mut project, _) = Project::open(&dir.join("project")).unwrap();
    *project.worker_config_mut() = WorkerConfig {
        test_faults: true,
        ..WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
    };
    project.set_tessdata_dir(Some(dir.join("tessdata")));
    project.add_source_root(src).unwrap();
    project
}

/// `(state, failure reason)` of the content stored for `name`.
fn outcome(project: &Project, name: &str) -> (IngestionState, Option<String>) {
    let stored = project.store().stored_refs().unwrap();
    let r = stored.iter().find(|r| r.rel_path == name).unwrap();
    let c = project.store().content(&r.hash).unwrap().unwrap();
    (c.state, c.failure_reason)
}

/// A blank grey PNG; `shade` keeps two of them apart (the store keeps one content per hash).
fn blank_png(width: u32, height: u32, shade: u8) -> Vec<u8> {
    GrayImage {
        width,
        height,
        pixels: vec![shade; (width * height) as usize],
    }
    .to_png()
    .unwrap()
}

/// The document stored for `name`.
fn document(project: &Project, name: &str) -> i64 {
    let stored = project.store().stored_refs().unwrap();
    let r = stored.iter().find(|r| r.rel_path == name).unwrap();
    let c = project.store().content(&r.hash).unwrap().unwrap();
    project
        .store()
        .read_raw(move |conn| {
            Ok(conn.query_row(
                "SELECT id FROM document WHERE content_id = ?1 AND parent_id IS NULL",
                [c.id],
                |r| r.get(0),
            )?)
        })
        .unwrap()
}

/// ADR-0017, NFR-REL-06: a PDF's deadline restarts at each finished page, so a PDF that keeps
/// finishing pages runs past the deadline; the same work in a TXT file is stopped.
#[test]
fn adr_0017_pdf_deadline_per_page() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for name in ["slow.pdf", "slow.txt"] {
        std::fs::write(
            src.join(name),
            format!("mk-test-fault: slow-pages\n{name}\n"),
        )
        .unwrap();
    }
    let mut project = project(tmp.path(), &src);
    // 3.2 s of work, a finished page every 400 ms (heartbeats every 500 ms).
    project.worker_config_mut().deadline = Duration::from_millis(1500);
    project.set_stage_a_workers(2);
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();

    assert_eq!((run.done, run.failed), (1, 1));
    assert_eq!(outcome(&project, "slow.pdf").0, IngestionState::Done);
    assert_eq!(
        outcome(&project, "slow.txt"),
        (IngestionState::Failed, Some("Took too long to read".into()))
    );
}

/// ADR-0017, NFR-REL-06: a page in OCR has its own deadline (30 s by default), however long
/// the file's deadline is.
#[test]
fn adr_0017_ocr_page_deadline() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("stuck.png"), "mk-test-fault: stuck-ocr-page\n").unwrap();
    let mut project = project(tmp.path(), &src);
    // The page stays in OCR for 3 s.
    project.worker_config_mut().ocr_page_deadline = Duration::from_millis(1500);
    project.rescan(false).unwrap();
    let run = project.run_stage_a().unwrap();

    assert_eq!((run.done, run.failed), (0, 1));
    assert_eq!(run.stops[0].why, mk_core::Stopped::Deadline);
    assert_eq!(
        outcome(&project, "stuck.png"),
        (IngestionState::Failed, Some("Took too long to read".into()))
    );
}

/// US-02.2 AC4, ADR-0018: OCR uses only the project's languages. A language with no installed
/// file fails the files that need OCR with a plain reason; English is built in; a code that
/// is not a language is refused.
#[test]
fn us_02_2_ac4_ocr_languages() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.png"), blank_png(64, 64, 255)).unwrap();
    let project = project(tmp.path(), &src);
    assert_eq!(project.ocr_languages().unwrap(), "eng");
    assert!(matches!(
        project.set_ocr_languages("eng+../x"),
        Err(ProjectError::Invalid(_))
    ));
    assert!(matches!(
        project.set_ocr_languages(" + "),
        Err(ProjectError::Invalid(_))
    ));
    project.set_ocr_languages("eng + zzz").unwrap();
    assert_eq!(project.ocr_languages().unwrap(), "eng+zzz");
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    assert_eq!(
        outcome(&project, "a.png"),
        (
            IngestionState::Failed,
            Some("OCR language not installed: zzz".into())
        )
    );
    assert_eq!(project.installed_ocr_languages().unwrap(), vec!["eng"]);

    // With English only, a second image is read.
    project.set_ocr_languages("eng").unwrap();
    std::fs::write(src.join("b.png"), blank_png(64, 64, 254)).unwrap();
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    assert_eq!(outcome(&project, "b.png").0, IngestionState::Done);
}

/// US-08.1 AC1–AC2, ADR-0011: the page view renders a page through the render worker and the
/// page cache; when the original changes or goes, it falls back to the stored text with a note.
#[test]
fn page_view_shows_the_page_or_falls_back_to_text() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("scan.png"), blank_png(120, 80, 250)).unwrap();
    let project = project(tmp.path(), &src);
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    assert_eq!(outcome(&project, "scan.png").0, IngestionState::Done);
    let doc = document(&project, "scan.png");

    let view = project.page_view(doc, 1, None).unwrap().unwrap();
    assert_eq!(view.status, PageStatus::Shown);
    assert_eq!((view.width, view.height, view.pages), (120, 80, 1));
    let png = project.page_image(doc, 1).unwrap().unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let cached = mk_core::page_cache_dir(&project.layout().cache());
    assert_eq!(std::fs::read_dir(&cached).unwrap().count(), 1);
    // No such page: the file has one.
    let past = project.page_view(doc, 2, None).unwrap().unwrap();
    assert_eq!(past.status, PageStatus::TextOnly);
    assert!(past.note.unwrap().contains("no page 2"));

    // The text side of the page.
    let ctx = project.document_view(doc).unwrap().unwrap();
    assert!(matches!(
        ctx.body,
        SourceBody::Page {
            page: 1,
            kind: None,
            ..
        }
    ));

    // A cached page is a picture of the stored content; with the cache cleared, a changed
    // original is not shown.
    std::fs::remove_dir_all(&cached).unwrap();
    std::fs::write(src.join("scan.png"), blank_png(120, 80, 10)).unwrap();
    let changed = project.page_view(doc, 1, None).unwrap().unwrap();
    assert_eq!(changed.status, PageStatus::Changed);
    assert!(changed.note.is_some());
    assert_eq!(project.page_image(doc, 1).unwrap(), None);

    std::fs::remove_file(src.join("scan.png")).unwrap();
    let gone = project.page_view(doc, 1, None).unwrap().unwrap();
    assert_eq!(gone.status, PageStatus::OriginalMissing);
}
