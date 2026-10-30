//! M3d viewer reads over the real pipeline: the mention's place in its document (US-08.2 AC1),
//! links in shown text (US-08.4 AC3), a missing or changed original (US-08.5 AC1) and the file to
//! open or show (US-08.6). The TD-1 cases are skipped without `testdata/` unless
//! `MK_REQUIRE_TESTDATA=1`.

use std::path::{Path, PathBuf};

use mk_core::{
    Marked, Original, PageStatus, Project, Run, SourceBody, SourceContext, WorkerConfig,
};

fn project(dir: &Path) -> Project {
    let (mut project, _) = Project::open(dir).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.set_tessdata_dir(Some(dir.join("tessdata")));
    project
}

/// Mention ids of a content, in id order.
fn mention_ids(p: &Project, content: i64) -> Vec<i64> {
    p.store()
        .read_raw(move |c| {
            let mut stmt =
                c.prepare("SELECT id FROM identifier_mention WHERE content_id = ?1 ORDER BY id")?;
            let rows = stmt.query_map([content], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap()
}

/// The content id of `rel` in the first source root.
fn content_of(p: &Project, rel: &str) -> i64 {
    let refs = p.store().stored_refs().unwrap();
    let r = refs.iter().find(|r| r.rel_path == rel).unwrap();
    p.store().content(&r.hash).unwrap().unwrap().id
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
fn hit(view: &SourceContext) -> String {
    // A hit that wraps (a PDF line break inside an IBAN) is highlighted on each of its lines.
    texts(&view.body)
        .into_iter()
        .map(|m| m.hit.as_str())
        .filter(|h| !h.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// US-08.2 AC1: "Mention 2 of 3" with the mentions before and after, in reading order, counting
/// only this value in this document.
#[test]
fn us_08_2_ac1_position_in_document() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.txt"),
        "one a@x.example\nother b@x.example\ntwo a@x.example and three a@x.example\n",
    )
    .unwrap();
    std::fs::write(src.join("b.txt"), "a@x.example b@x.example\n").unwrap();
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();

    let occ = p.occurrences("email:a@x.example").unwrap();
    let doc_a = p
        .source_context(occ[0].mention_id)
        .unwrap()
        .unwrap()
        .document
        .id;
    let in_a: Vec<i64> = occ
        .iter()
        .filter(|o| o.document_id == doc_a)
        .map(|o| o.mention_id)
        .collect();
    assert_eq!(in_a.len(), 3);
    let at = |id: i64| p.source_context(id).unwrap().unwrap();
    let mid = at(in_a[1]).position.unwrap();
    assert_eq!(
        (mid.index, mid.count, mid.prev, mid.next),
        (2, 3, Some(in_a[0]), Some(in_a[2]))
    );
    let first = at(in_a[0]).position.unwrap();
    assert_eq!((first.index, first.prev), (1, None));
    // Following next from the first visits every mention in reading order.
    let mut seen = vec![in_a[0]];
    let mut next = first.next;
    while let Some(id) = next {
        let v = at(id);
        assert_eq!(hit(&v), "a@x.example");
        seen.push(id);
        next = v.position.unwrap().next;
    }
    assert_eq!(seen, in_a);
    // One mention in its document: 1 of 1.
    let other = occ.iter().find(|o| o.document_id != doc_a).unwrap();
    let only = at(other.mention_id).position.unwrap();
    assert_eq!(
        (only.index, only.count, only.prev, only.next),
        (1, 1, None, None)
    );
    // A document view has none.
    assert!(p.document_view(doc_a).unwrap().unwrap().position.is_none());
}

/// US-08.4 AC3: a URL in a shown line comes as a link run; a `mailto:` holding the hit keeps
/// the hit inside the link. Links create no identifier (M4).
#[test]
fn us_08_4_ac3_link_runs() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.txt"),
        "Zoë: see https://example.org/x?y=1. Mail <mailto:zoe@x.example> now\n",
    )
    .unwrap();
    std::fs::write(src.join("b.csv"), "who\nzoe@x.example\n").unwrap();
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();

    let occ = p.occurrences("email:zoe@x.example").unwrap();
    let txt = occ.iter().find(|o| o.file_type == "txt").unwrap();
    let v = p.source_context(txt.mention_id).unwrap().unwrap();
    let SourceBody::Text { lines } = &v.body else {
        panic!("{:?}", v.body)
    };
    let line = &lines[0].text;
    assert_eq!(line.hit, "zoe@x.example");
    let links: Vec<(&str, Vec<(&str, bool)>)> = line
        .runs
        .iter()
        .filter_map(|r| match r {
            Run::Link { url, pieces } => Some((
                url.as_str(),
                pieces.iter().map(|p| (p.text.as_str(), p.hit)).collect(),
            )),
            Run::Text(_) => None,
        })
        .collect();
    assert_eq!(
        links,
        vec![
            (
                "https://example.org/x?y=1",
                vec![("https://example.org/x?y=1", false)]
            ),
            (
                "mailto:zoe@x.example",
                vec![("mailto:", false), ("zoe@x.example", true)]
            ),
        ]
    );
    // The runs say the same text as before / hit / after.
    let joined: String = line
        .runs
        .iter()
        .flat_map(|r| match r {
            Run::Text(p) => vec![p.text.as_str()],
            Run::Link { pieces, .. } => pieces.iter().map(|p| p.text.as_str()).collect(),
        })
        .collect();
    assert_eq!(joined, format!("{}{}{}", line.before, line.hit, line.after));
    // A text without a link has no runs.
    let csv = occ.iter().find(|o| o.file_type == "csv").unwrap();
    let v = p.source_context(csv.mention_id).unwrap().unwrap();
    assert!(texts(&v.body).iter().all(|m| m.runs.is_empty()));
    // Since M4b the link is a URL identifier too (US-04.3); `mailto:` is not one.
    let derived = p.store().derived_state().unwrap();
    assert!(
        derived
            .identifiers
            .contains_key("url:https://example.org/x?y=1")
    );
    assert!(!derived.identifiers.keys().any(|k| k.contains("mailto")));
}

/// US-08.5: a file whose size or modified time is not what was read shows as changed; the view
/// is still the stored text with its hit.
#[test]
fn us_08_5_changed_original() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "x a@x.example\n").unwrap();
    std::fs::write(src.join("b.txt"), "y a@x.example\n").unwrap();
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    p.ingest().unwrap();
    let id = mention_ids(&p, content_of(&p, "a.txt"))[0];
    let path = src.join("a.txt").display().to_string();
    let v = p.source_context(id).unwrap().unwrap();
    assert_eq!(v.original, Some(Original::Present { path: path.clone() }));
    std::fs::write(src.join("a.txt"), "x a@x.example\nmore\n").unwrap();
    let v = p.source_context(id).unwrap().unwrap();
    assert_eq!(v.original, Some(Original::Changed { path }));
    assert_eq!(hit(&v), "a@x.example");
}

fn td1() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td1/synthetic");
    if dir.is_dir() {
        return Some(dir);
    }
    assert!(
        std::env::var_os("MK_REQUIRE_TESTDATA").is_none(),
        "testdata/td1 is missing"
    );
    eprintln!("skipped: testdata/td1 is missing (see docs/07-test-corpus.md §9)");
    None
}

/// Copies every TD-1 file to `to`, flat. Returns the copied names.
fn copy_td1(td1: &Path, to: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![td1.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                files.push(e.path());
            }
        }
    }
    files.sort();
    std::fs::create_dir_all(to).unwrap();
    let mut out = Vec::new();
    for f in files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        std::fs::copy(&f, to.join(&name)).unwrap();
        out.push(name);
    }
    out
}

/// US-08.5 AC1 for every format TD-1 has, and US-08.6: with the original moved away, a mention
/// opens on the stored text with its hit and the banner "Original file not found at <path>",
/// before and after a re-scan; a page view shows the stored text with the same note; there is
/// no file to open. With it there, the file to open is the original, an attachment's its email.
#[test]
fn us_08_5_ac1_missing_original_every_format() {
    let Some(td1) = td1() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let mut names = copy_td1(&td1, &src);
    // TD-1's CSVs hold no email address (identifiers other than email come in M4).
    std::fs::write(src.join("people.csv"), "name,email\nAnn,ann@csv.example\n").unwrap();
    names.push("people.csv".into());
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&src).unwrap();
    let run = p.ingest().unwrap();
    assert_eq!(run.stage_a.failed, 0);

    // One mention per file that has one, and per format at least one.
    let mut picked: Vec<(String, i64)> = Vec::new();
    for name in &names {
        if let Some(id) = mention_ids(&p, content_of(&p, name)).first() {
            picked.push((name.clone(), *id));
        }
    }
    let exts: std::collections::BTreeSet<String> = picked
        .iter()
        .map(|(n, _)| n.rsplit('.').next().unwrap().to_lowercase())
        .collect();
    for want in [
        "txt", "md", "csv", "json", "docx", "html", "eml", "mbox", "pdf",
    ] {
        assert!(
            exts.contains(want),
            "no mention in a .{want} file: {exts:?}"
        );
    }
    assert!(
        exts.iter()
            .any(|e| ["png", "jpg", "tif"].contains(&e.as_str())),
        "{exts:?}"
    );

    // US-08.6: the file to open while it is there.
    for (name, id) in &picked {
        let v = p.source_context(*id).unwrap().unwrap();
        let want = src.join(name);
        assert_eq!(
            p.original_path(v.document.id).unwrap().as_deref(),
            Some(want.as_path()),
            "{name}"
        );
        assert_eq!(
            v.original,
            Some(Original::Present {
                path: want.display().to_string()
            })
        );
    }
    // An attachment's file is its email.
    let attachment: Option<(i64, i64)> = p
        .store()
        .read_raw(|c| {
            Ok(c.query_row(
                "SELECT id, content_id FROM document WHERE parent_id IS NOT NULL
                 ORDER BY id LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok())
        })
        .unwrap();
    let (att, content) = attachment.expect("TD-1 sample has an email with an attachment");
    let email = names.iter().find(|n| content_of(&p, n) == content).unwrap();
    assert_eq!(p.original_path(att).unwrap(), Some(src.join(email)));

    // Move every file away.
    let moved = tmp.path().join("moved");
    std::fs::rename(&src, &moved).unwrap();
    let check = |when: &str| {
        for (name, id) in &picked {
            let v = p.source_context(*id).unwrap().unwrap();
            let path = src.join(name).display().to_string();
            assert_eq!(
                v.original,
                Some(Original::Missing { path: path.clone() }),
                "{when} {name}"
            );
            assert_eq!(
                mk_core::not_found_note(&path),
                format!("Original file not found at {path}")
            );
            assert_eq!(p.original_path(v.document.id).unwrap(), None, "{name}");
            let raw = p.store().mention(*id).unwrap().unwrap().raw;
            match &v.body {
                SourceBody::Page { page, .. } => {
                    // The stored text with the hit, and a page view that says why.
                    assert_eq!(hit(&v), raw, "{when} {name}");
                    let pv = p
                        .page_view(v.document.id, *page, Some(*id))
                        .unwrap()
                        .unwrap();
                    if v.document.parent.is_none() {
                        assert_eq!(pv.status, PageStatus::OriginalMissing, "{when} {name}");
                        assert_eq!(
                            pv.note.as_deref(),
                            Some(mk_core::not_found_note(&path).as_str())
                        );
                    }
                }
                // HTML is matched by text: offsets count the extracted text.
                _ => assert_eq!(hit(&v), raw, "{when} {name}"),
            }
        }
    };
    check("before re-scan");
    p.rescan(false).unwrap();
    check("after re-scan");
}

/// Every file under `dir` with its content hash.
fn hashes(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let h = mk_corpus::fs::hash_file(&p).unwrap().to_hex();
                out.push((p, h));
            }
        }
    }
    out.sort();
    out
}

/// NFR-REL-10 (M3 criterion 7), INV-CO-01: TD-1 is read in place by everything M3 does (ingest,
/// rebuild, every source view, every page rendered, retry and a second run), and every file's
/// hash is the same afterwards. Release build: `cargo test --release -p mk-core --test
/// m3d_viewer -- --ignored nfr_rel_10`.
#[test]
#[ignore = "M3 acceptance run, release build"]
fn nfr_rel_10_td1_unchanged_after_full_run() {
    let Some(synthetic) = td1() else { return };
    let td1 = synthetic.parent().unwrap().to_path_buf();
    let before = hashes(&td1);
    let tmp = tempfile::tempdir().unwrap();
    let p = project(&tmp.path().join("p"));
    p.add_source_root(&synthetic).unwrap();
    let run = p.ingest().unwrap();
    assert_eq!(run.stage_a.failed, 0);
    assert!(p.rebuild().unwrap().same_as_incremental);

    // Every mention's view, every linked document, every page of every PDF and image.
    let (mut views, mut pages) = (0, 0);
    let mut docs = std::collections::BTreeSet::new();
    let mut after = None;
    loop {
        let page = p.connections_page(after.as_ref(), 500).unwrap();
        for c in &page.rows {
            for o in p.occurrences(&c.subject_key).unwrap() {
                let v = p.source_context(o.mention_id).unwrap().unwrap();
                views += 1;
                docs.insert(v.document.id);
            }
        }
        match page.next {
            Some(n) => after = Some(n),
            None => break,
        }
    }
    // Then every document, with or without a mention.
    let all: Vec<i64> = p
        .store()
        .read_raw(|c| {
            let mut stmt = c.prepare("SELECT id FROM document ORDER BY id")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert!(docs.iter().all(|d| all.contains(d)));
    for doc in all {
        p.document_view(doc).unwrap();
        let Some(first) = p.page_view(doc, 1, None).unwrap() else {
            continue;
        };
        for n in 1..=first.pages {
            let v = p.page_view(doc, n, None).unwrap().unwrap();
            if v.status == PageStatus::Shown {
                assert!(p.page_image(doc, n).unwrap().is_some());
                pages += 1;
            }
        }
    }
    let retried = p.retry_all().unwrap();
    p.ingest().unwrap();
    assert_eq!(hashes(&td1), before, "a TD-1 file changed");
    eprintln!(
        "NFR-REL-10: {} TD-1 files unchanged after {views} views, {pages} pages rendered, {retried} retried",
        before.len()
    );
}
