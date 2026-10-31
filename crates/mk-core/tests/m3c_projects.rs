//! M3c criterion 1, E01: create and open projects through the registry (US-01.1, US-01.2), the
//! add summary (US-01.3 AC5), removing files (US-01.5) and settings (US-01.6).

use std::path::Path;

use mk_core::{DocumentFilter, Project, ProjectSettings, Reprocess, Workspace, WorkspaceError};
use mk_corpus::IngestionState;
use mk_kernel::ContentHash;
use mk_store::{MIGRATIONS, Store};

fn worker() -> mk_core::WorkerConfig {
    mk_core::WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"))
}

/// A project in `dir` with the test worker, `src` added and ingested.
fn ingested(dir: &Path, src: &Path) -> Project {
    let (mut p, _) = Project::open(dir).unwrap();
    *p.worker_config_mut() = worker();
    p.set_stage_a_workers(2);
    p.add_source_root(src).unwrap();
    p.ingest().unwrap();
    p
}

fn write(dir: &Path, files: &[(&str, &str)]) {
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn ref_id(p: &Project, rel: &str) -> i64 {
    let page = p
        .documents_page(&DocumentFilter::default(), None, 1_000)
        .unwrap();
    page.rows
        .iter()
        .find(|r| r.rel_path == rel)
        .unwrap_or_else(|| panic!("{rel} not listed"))
        .ref_id
}

/// US-01.1 AC1: "New project" makes `<location>/<name>` with one database and opens it empty.
#[test]
fn us_01_1_ac1_create_opens_empty() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    let (project, entry) = ws.create("Acme: case 1", tmp.path()).unwrap();
    let dir = tmp.path().join("Acme_ case 1");
    assert!(dir.join("project.db").is_file());
    assert_eq!(entry.name, "Acme: case 1");
    assert_eq!(Path::new(&entry.path), dir.canonicalize().unwrap());
    assert_eq!(project.counts().unwrap().total, 0);
    assert_eq!(project.name().unwrap().as_deref(), Some("Acme: case 1"));
    assert_eq!(
        project.store().meta("app_version").unwrap().as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    // Only project.db and its WAL files, no stray probe file at the location.
    assert!(std::fs::read_dir(tmp.path()).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .contains("write-check")
    }));
}

/// US-01.1 AC2, INV-WS-01: names are unique ignoring case; nothing is created.
#[test]
fn us_01_1_ac2_name_clash_ignoring_case() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    ws.create("Acme", tmp.path()).unwrap();
    let other = tmp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    let err = ws.create("ACME", &other).err().expect("refused");
    assert!(matches!(err, WorkspaceError::NameTaken), "{err}");
    assert_eq!(err.to_string(), "A project with this name already exists");
    assert_eq!(std::fs::read_dir(&other).unwrap().count(), 0);
    assert!(matches!(
        ws.create("  ", tmp.path()),
        Err(WorkspaceError::Name(_))
    ));
}

/// US-01.1 AC3, INV-WS-02: a location the app cannot write to is refused, nothing created.
#[cfg(unix)]
#[test]
fn us_01_1_ac3_read_only_location() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    let ro = tmp.path().join("ro");
    std::fs::create_dir(&ro).unwrap();
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::write(ro.join("probe"), b"x").is_ok() {
        eprintln!("skipped: running with rights that ignore folder permissions");
        return;
    }
    let err = ws.create("Acme", &ro).err().expect("refused");
    assert_eq!(err.to_string(), "Cannot write to this folder");
    assert_eq!(std::fs::read_dir(&ro).unwrap().count(), 0);
    assert!(ws.recent().unwrap().is_empty());
    std::fs::set_permissions(&ro, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// US-01.2 AC1: the recent list is newest first, with name, path and last opened time.
#[test]
fn us_01_2_ac1_recent_newest_first() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    let (a, _) = ws.create("A", tmp.path()).unwrap();
    drop(a);
    std::thread::sleep(std::time::Duration::from_millis(5));
    drop(ws.create("B", tmp.path()).unwrap());
    std::thread::sleep(std::time::Duration::from_millis(5));
    drop(ws.open_project(&tmp.path().join("A")).unwrap());
    let recent = ws.recent().unwrap();
    let names: Vec<&str> = recent.iter().map(|r| r.entry.name.as_str()).collect();
    assert_eq!(names, vec!["A", "B"]);
    assert!(
        recent
            .iter()
            .all(|r| r.present && r.entry.last_opened_at > 0)
    );
}

/// US-01.2 AC2: a moved project is "not found" at its old path; Locate… points the entry at
/// the new folder; Remove from list forgets it and leaves the folder alone.
#[test]
fn us_01_2_ac2_missing_folder_not_found_locate_forget() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    let (p, entry) = ws.create("Acme", tmp.path()).unwrap();
    drop(p);
    let moved = tmp.path().join("moved");
    std::fs::rename(tmp.path().join("Acme"), &moved).unwrap();

    assert!(!ws.recent().unwrap()[0].present);
    let err = ws.open_entry(entry.id).err().expect("not found");
    assert!(matches!(err, WorkspaceError::NotFound(_)));
    assert!(err.to_string().starts_with("Project not found at "));

    // Locate… needs a folder that holds a project.
    assert!(ws.locate(entry.id, tmp.path()).is_err());
    let located = ws.locate(entry.id, &moved).unwrap();
    assert_eq!(Path::new(&located.path), moved.canonicalize().unwrap());
    let (p, _, e) = ws.open_entry(entry.id).unwrap();
    assert_eq!(e.id, entry.id);
    drop(p);

    ws.forget(entry.id).unwrap();
    assert!(ws.recent().unwrap().is_empty());
    assert!(moved.join("project.db").is_file());
}

/// US-01.2 AC3, INV-WS-03: a project from a newer app is refused, names the app version it
/// needs, and is not changed (byte compare).
#[test]
fn us_01_2_ac3_too_new_names_app_version_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    let (p, _) = ws.create("Acme", tmp.path()).unwrap();
    p.store().stamp(None, "99.1.0").unwrap();
    p.store()
        .write_raw(|c| {
            c.pragma_update(None, "user_version", 99)?;
            Ok(())
        })
        .unwrap();
    drop(p);
    let db = tmp.path().join("Acme/project.db");
    let before = std::fs::read(&db).unwrap();
    let err = ws
        .open_project(&tmp.path().join("Acme"))
        .err()
        .expect("refused");
    assert!(
        matches!(&err, WorkspaceError::TooNew { needs: Some(v) } if v == "99.1.0"),
        "{err}"
    );
    assert_eq!(
        err.to_string(),
        "This project needs Mangkon AI 99.1.0 or newer. It was not opened or changed."
    );
    assert_eq!(std::fs::read(&db).unwrap(), before);
}

/// US-01.2 AC4, INV-WS-04: an M3b project (schema v3) opened from the list is backed up, then
/// migrated, and registered under its folder name (it stored no name).
#[test]
fn us_01_2_ac4_backup_then_migrate() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("Old case");
    std::fs::create_dir(&dir).unwrap();
    {
        let (store, _) = Store::open_with(
            &dir.join("project.db"),
            &dir.join("backups"),
            &MIGRATIONS[..3],
        )
        .unwrap();
        store
            .enqueue_contents(vec![mk_store::NewContent {
                hash: ContentHash::from_bytes([3; 32]),
                file_type: "txt".into(),
                size: 1,
            }])
            .unwrap();
    }
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    let (p, summary, entry) = ws.open_project(&dir).unwrap();
    assert_eq!(
        (summary.schema.from_version, summary.schema.to_version),
        (3, 7)
    );
    assert!(summary.schema.backup.unwrap().is_file());
    assert_eq!(entry.name, "Old case");
    assert_eq!(p.name().unwrap().as_deref(), Some("Old case"));
    assert_eq!(p.counts().unwrap().queued, 1);
}

/// US-01.3 AC4/AC5: "N added, N skipped, N duplicates"; a duplicate is read once.
#[test]
fn us_01_3_ac5_summary_counts_duplicates() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "a@x.example\n"),
            ("copy/a.txt", "a@x.example\n"),
            ("b.txt", "b@x.example\n"),
            ("tool.exe", "MZ"),
        ],
    );
    let (mut p, _) = Project::open(&tmp.path().join("p")).unwrap();
    *p.worker_config_mut() = worker();
    p.add_source_root(&src).unwrap();
    let s = p.ingest().unwrap();
    assert_eq!(s.rescan.outcome.added, 3);
    assert_eq!(s.rescan.outcome.duplicates, 1);
    assert_eq!(s.rescan.skipped.len(), 1);
    assert_eq!(s.stage_a.done, 2, "the duplicate is read once");
    let rows = p
        .documents_page(&DocumentFilter::default(), None, 100)
        .unwrap()
        .rows;
    let a = rows.iter().find(|r| r.rel_path == "a.txt").unwrap();
    assert_eq!(a.other_paths, 1, "the document shows both paths");
}

fn hash(path: &Path) -> ContentHash {
    ContentHash::of(&std::fs::read(path).unwrap())
}

/// US-01.5 AC1, INV-CO-08, INV-CO-01: removing a file deletes its data and evidence and
/// rebuilds the connections; the file on disk is untouched; re-scan does not bring it back.
#[test]
fn us_01_5_ac1_remove_deletes_data_rebuilds_keeps_original() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "shared@x.example\nonly@a.example\n"),
            ("b.txt", "shared@x.example\n"),
        ],
    );
    let before = hash(&src.join("a.txt"));
    let p = ingested(&tmp.path().join("p"), &src);
    assert_eq!(p.connection_count().unwrap(), 2, "the email and its domain");

    let a = ref_id(&p, "a.txt");
    assert_eq!(p.removal_impact(&[a]).unwrap(), 2);
    let out = p.remove_files(&[a]).unwrap();
    assert_eq!((out.refs, out.contents), (1, 1));
    assert!(out.connections_changed);
    assert_eq!(p.connection_count().unwrap(), 0);
    assert_eq!(hash(&src.join("a.txt")), before);
    let mentions: i64 = p
        .store()
        .read_raw(|c| {
            Ok(c.query_row(
                "SELECT COUNT(*) FROM identifier_mention WHERE identifier_key = 'email:only@a.example'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(mentions, 0);
    assert!(p.rebuild().unwrap().same_as_incremental);

    // US-01.5 "excluded on re-scan": the file is not added again.
    let again = p.ingest().unwrap();
    assert_eq!(again.rescan.outcome.added, 0);
    assert_eq!(p.counts().unwrap().total, 1);

    // Adding the path again clears its exclusion.
    p.add_source_root(&src.join("a.txt")).unwrap();
    let back = p.ingest().unwrap();
    assert_eq!(back.rescan.outcome.added, 1);
    assert_eq!(p.connection_count().unwrap(), 2);
    assert_eq!(p.source_roots().unwrap().len(), 1, "no second root");
}

/// INV-CO-08, D3: removing one of two paths of the same content keeps the content and its data.
#[test]
fn us_01_5_shared_content_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "shared@x.example\n"),
            ("dup.txt", "shared@x.example\n"),
            ("b.txt", "shared@x.example\nb@x.example\n"),
        ],
    );
    let p = ingested(&tmp.path().join("p"), &src);
    let dup = ref_id(&p, "dup.txt");
    assert_eq!(p.removal_impact(&[dup]).unwrap(), 0);
    let out = p.remove_files(&[dup]).unwrap();
    assert_eq!((out.refs, out.contents), (1, 0));
    assert_eq!(p.connection_count().unwrap(), 2, "the email and its domain");
    let c = p
        .store()
        .content(&hash(&src.join("a.txt")))
        .unwrap()
        .unwrap();
    assert_eq!(c.state, IngestionState::Done);
}

/// US-01.5 AC2: the confirmation counts the connections that lose evidence.
#[test]
fn us_01_5_ac2_impact_counts_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "one@x.example\ntwo@x.example\nsolo@x.example\n"),
            ("b.txt", "one@x.example\n"),
            ("c.txt", "two@x.example\n"),
        ],
    );
    let p = ingested(&tmp.path().join("p"), &src);
    assert_eq!(
        p.connection_count().unwrap(),
        3,
        "two emails and their domain"
    );
    let (a, b) = (ref_id(&p, "a.txt"), ref_id(&p, "b.txt"));
    assert_eq!(p.removal_impact(&[a]).unwrap(), 3);
    assert_eq!(p.removal_impact(&[b]).unwrap(), 2);
    assert_eq!(p.removal_impact(&[]).unwrap(), 0);
}

/// US-01.6 AC2, INV-WS-05: a new project has the default settings.
#[test]
fn us_01_6_ac2_defaults() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    let (p, _) = ws.create("Acme", tmp.path()).unwrap();
    assert_eq!(p.settings().unwrap(), ProjectSettings::default());
    assert_eq!(p.ocr_languages().unwrap(), "eng");
    let saved = ProjectSettings {
        phone_region: Some("de".into()),
        ..ProjectSettings::default()
    };
    // US-04.2 AC2: a new region can be applied to what is already read.
    assert_eq!(
        p.save_settings(saved).unwrap(),
        Reprocess {
            ocr: false,
            identifiers: true,
            custom: false,
        }
    );
    assert_eq!(p.settings().unwrap().phone_region.as_deref(), Some("DE"));
    assert!(
        p.save_settings(ProjectSettings {
            ocr_languages: vec![],
            ..ProjectSettings::default()
        })
        .is_err(),
        "INV-WS-06"
    );
}

/// US-01.6 AC3, INV-WS-07: a new OCR language set re-queues only contents with OCR'd pages and
/// those that failed for a missing language; the rest are not touched.
#[test]
fn us_01_6_ac3_ocr_change_reprocesses_only_ocr_contents() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("scan.txt", "scan@x.example\n"),
            ("plain.txt", "plain@x.example\n"),
            ("nolang.txt", "nolang@x.example\n"),
        ],
    );
    let p = ingested(&tmp.path().join("p"), &src);
    let id = |name: &str| {
        p.store()
            .content(&hash(&src.join(name)))
            .unwrap()
            .unwrap()
            .id
    };
    let (scan, nolang) = (id("scan.txt"), id("nolang.txt"));
    // Stand-ins: one content read with OCR, one that failed for a missing OCR language.
    p.store()
        .write_raw(move |c| {
            c.execute(
                "UPDATE segment SET loc_kind = 'ocr_text' WHERE document_id IN
                     (SELECT id FROM document WHERE content_id = ?1)",
                [scan],
            )?;
            c.execute(
                "UPDATE content SET state = 'failed', failure_code = 'ocr_language_missing'
                 WHERE id = ?1",
                [nolang],
            )?;
            Ok(())
        })
        .unwrap();
    let new = ProjectSettings {
        ocr_languages: vec!["eng".into(), "deu".into()],
        ..p.settings().unwrap()
    };
    let what = p.save_settings(new).unwrap();
    assert_eq!(
        what,
        Reprocess {
            ocr: true,
            identifiers: false,
            custom: false,
        }
    );
    assert_eq!(p.ocr_languages().unwrap(), "eng+deu");
    assert_eq!(p.reprocess(what).unwrap(), 2);
    let state = |n: &str| {
        p.store()
            .content(&hash(&src.join(n)))
            .unwrap()
            .unwrap()
            .state
    };
    assert_eq!(state("scan.txt"), IngestionState::Queued);
    assert_eq!(state("nolang.txt"), IngestionState::Queued);
    assert_eq!(state("plain.txt"), IngestionState::Done);
}

/// D12: a moved source root is relocated; files with the same size and time are not read again.
#[test]
fn d12_relocate_root_redoes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    write(
        &src,
        &[
            ("a.txt", "a@x.example\n"),
            ("b.txt", "a@x.example\nb@x.example\n"),
        ],
    );
    let p = ingested(&tmp.path().join("p"), &src);
    let moved = tmp.path().join("moved");
    std::fs::rename(&src, &moved).unwrap();
    let roots = p.source_roots().unwrap();
    assert!(!roots[0].present);
    p.relocate_root(roots[0].id, &moved).unwrap();
    assert!(p.source_roots().unwrap()[0].present);
    let s = p.ingest().unwrap();
    assert_eq!((s.rescan.hashed, s.stage_a.done), (0, 0));
    assert_eq!(s.rescan.unchanged, 2);
    assert_eq!(p.connection_count().unwrap(), 2, "the email and its domain");
}
