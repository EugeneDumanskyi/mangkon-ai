//! `bench` (M4b, US-14.3 AC2, NFR-MNT-05): ingest TD-1 into fresh projects, score the
//! identifiers they found against the ground truth with `mk_bench::score`, and compare with a
//! baseline. `synthetic/` is read with the phone region GB and `public/` with US, each in its
//! own project, and scored together (`docs/07-test-corpus.md` §8). Nothing printed holds an
//! identifier value: the report names types, counts and ground-truth ids only (NFR-SEC-14).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mk_bench::score::{Class, Found, FoundMention, Place, Report, compare, score};
use mk_bench::{DocPart, GroundTruth};
use mk_core::{Assignments, DocumentFilter, Locator, Project, ProjectError, ProjectSettings};

/// The gate: a metric may fall this many points below the baseline (NFR-MNT-05).
const MAX_DROP: f64 = 2.0;

/// The custom pattern of TD-1's tender references (US-04.7), added before ingest.
const TENDER_REF: (&str, &str) = ("tender_ref", r"VPA-TND-\d{4}-\d{3}");

/// The TD-1 folders and the phone region each is read with.
const PARTS: [(&str, &str); 2] = [("synthetic", "GB"), ("public", "US")];

pub struct Options {
    pub testdata: PathBuf,
    pub baseline: Option<PathBuf>,
    pub update_baseline: bool,
    pub json: Option<PathBuf>,
    pub tessdata: Option<PathBuf>,
    pub work: Option<PathBuf>,
}

fn fail(e: &dyn std::fmt::Display) -> ExitCode {
    eprintln!("error: {e}");
    ExitCode::FAILURE
}

pub fn run(opts: &Options) -> ExitCode {
    let gt = match mk_bench::load(&opts.testdata.join("ground-truth.json")) {
        Ok(gt) => gt,
        Err(e) => return fail(&e),
    };
    let temp;
    let work = match &opts.work {
        Some(dir) => dir.clone(),
        None => match tempfile::tempdir() {
            Ok(dir) => {
                temp = dir;
                temp.path().to_path_buf()
            }
            Err(e) => return fail(&e),
        },
    };
    let mut found = Found::default();
    let mut failed = 0;
    for (part, region) in PARTS {
        let source = opts.testdata.join(part);
        if !source.is_dir() {
            eprintln!(
                "note: {} is missing; its files score as misses",
                source.display()
            );
            continue;
        }
        match ingest_part(
            opts,
            &gt,
            &work.join(part),
            &source,
            part,
            region,
            &mut found,
        ) {
            Ok(n) => failed += n,
            Err(e) => return fail(&e),
        }
    }
    let report = score(&gt, &found);
    print_report(&report, failed);

    if let Some(out) = &opts.json
        && let Err(e) = write_json(out, &report)
    {
        return fail(&e);
    }
    let mut ok = report.negative_failures.is_empty() && report.unmapped.is_empty();
    match (&opts.baseline, opts.update_baseline) {
        (Some(path), true) => {
            if let Err(e) = write_json(path, &report) {
                return fail(&e);
            }
            println!("Baseline     written to {}", path.display());
        }
        (Some(path), false) => {
            let baseline: Report = match std::fs::read_to_string(path)
                .map_err(|e| e.to_string())
                .and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()))
            {
                Ok(b) => b,
                Err(e) => return fail(&format!("{}: {e}", path.display())),
            };
            let drops = compare(&report, &baseline, MAX_DROP);
            if drops.is_empty() {
                println!("Gate         passed (no metric fell more than {MAX_DROP} points)");
            } else {
                ok = false;
                println!("Gate         FAILED, fell more than {MAX_DROP} points:");
                for d in &drops {
                    println!(
                        "             {} {}: {:.1} -> {:.1}",
                        d.row, d.metric, d.baseline, d.now
                    );
                }
            }
        }
        (None, _) => {}
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn write_json(path: &Path, report: &Report) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = serde_json::to_string_pretty(report).expect("report serializes");
    text.push('\n');
    std::fs::write(path, text)
}

/// Ingests `source` into a new project in `dir` with the phone region `region` and adds what
/// it found to `found`. Returns the number of files that failed.
fn ingest_part(
    opts: &Options,
    gt: &GroundTruth,
    dir: &Path,
    source: &Path,
    part: &str,
    region: &str,
    found: &mut Found,
) -> Result<usize, ProjectError> {
    let (mut project, _) = Project::open(dir)?;
    project.set_tessdata_dir(opts.tessdata.clone().or_else(mk_core::tessdata_dir));
    project.save_settings(ProjectSettings {
        phone_region: Some(region.into()),
        ..project.settings()?
    })?;
    project.add_pattern(TENDER_REF.0, TENDER_REF.1)?;
    let root = project.add_source_root(source)?;
    project.ingest()?;
    map_columns(&project, gt, part, found)?;
    collect(&project, gt, root, part, found)?;
    Ok(project.counts()?.failed as usize)
}

/// Records the suggested type of every column of the CSV and JSON files (NFR-QA-13), then
/// confirms the suggestions as they are, so the table names can be scored (US-05.3).
fn map_columns(
    project: &Project,
    gt: &GroundTruth,
    part: &str,
    found: &mut Found,
) -> Result<(), ProjectError> {
    let mut tables = Vec::new();
    let mut after = None;
    loop {
        let page = project.documents_page(&DocumentFilter::default(), after.as_ref(), 500)?;
        tables.extend(
            page.rows
                .into_iter()
                .filter(|r| r.mapping_state.is_some())
                .map(|r| (r.ref_id, format!("{part}/{}", r.rel_path))),
        );
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    for (ref_id, file) in tables {
        let Some(view) = project.mapping_view(ref_id)? else {
            continue;
        };
        if let Some(d) = gt
            .documents
            .iter()
            .find(|d| d.file == file && d.part == DocPart::Whole)
        {
            for c in &view.columns {
                found.columns.push((
                    d.id.clone(),
                    c.key.clone(),
                    c.suggested.as_str().to_string(),
                ));
            }
        }
        let assignments = Assignments(
            view.columns
                .iter()
                .map(|c| (c.key.clone(), c.suggested))
                .collect(),
        );
        project.confirm_mapping(ref_id, assignments)?;
    }
    Ok(())
}

/// The ground-truth id of each stored document of `project`'s root `root`, and what was found
/// in them, added to `found`. A file found under two paths counts for both.
fn collect(
    project: &Project,
    gt: &GroundTruth,
    root: i64,
    part: &str,
    found: &mut Found,
) -> Result<(), ProjectError> {
    let store = project.store();
    // Stored document → ground-truth ids, for the card flags.
    let mut doc_ids: BTreeMap<i64, Vec<String>> = BTreeMap::new();
    for r in store.stored_refs()? {
        if r.root != root || !r.present {
            continue;
        }
        let file = format!("{part}/{}", r.rel_path);
        let Some(content) = store.content(&r.hash)? else {
            continue;
        };
        let segments = store.segments(content.id)?;
        // CSV rows: where each cell starts, to turn a span in the row into one in its cell.
        let mut rows: BTreeMap<(i64, u64), &[u32]> = BTreeMap::new();
        let mut documents = BTreeSet::new();
        for s in &segments {
            documents.insert(s.document_id);
            if let Locator::CsvRow { row } = s.locator {
                rows.insert((s.document_id, row), &s.cell_starts);
            }
        }
        let mut ids = BTreeMap::new();
        for d in documents {
            match gt_document(project, gt, &file, d)? {
                Ok(id) => {
                    doc_ids.entry(d).or_default().push(id.clone());
                    ids.insert(d, id);
                }
                Err(position) => found.unmapped.push(format!("{file} ({position})")),
            }
        }
        for m in store.mentions(content.id)? {
            let Some(document) = ids.get(&m.document_id) else {
                continue;
            };
            let Some(place) = place(&m.locator, |row| rows.get(&(m.document_id, row)).copied())
            else {
                continue;
            };
            found.mentions.push(FoundMention {
                document: document.clone(),
                key: m.key.to_string(),
                raw: m.raw,
                place,
                class: if matches!(m.locator, Locator::OcrText { .. }) {
                    Class::Ocr
                } else {
                    Class::Text
                },
            });
        }
    }
    for (_, document, last4) in store.all_card_flags()? {
        for id in doc_ids.get(&document).into_iter().flatten() {
            found.card_flags.push((id.clone(), last4.clone()));
        }
    }
    let names: Vec<(i64, String)> = store.read_raw(|c| {
        let mut stmt = c.prepare("SELECT document_id, name_key FROM entity_mention")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    })?;
    for (document, key) in names {
        for id in doc_ids.get(&document).into_iter().flatten() {
            found.table_names.push((id.clone(), key.clone()));
        }
    }
    let derived = store.derived_state()?;
    found.keys.extend(derived.identifiers.into_keys());
    found.connections.extend(derived.connections.into_keys());
    Ok(())
}

/// The ground-truth id of stored document `document` of `file`: `Ok(id)`, or `Err(position)`
/// when the ground truth has no such document.
pub(crate) fn gt_document(
    project: &Project,
    gt: &GroundTruth,
    file: &str,
    document: i64,
) -> Result<Result<String, String>, ProjectError> {
    let Some(info) = project.store().document_info(document)? else {
        return Ok(Err(format!("document {document}")));
    };
    let position = info.position;
    let id = match info.parent {
        None => gt.documents.iter().find(|d| {
            d.file == file
                && d.parent.is_none()
                && match &d.part {
                    DocPart::Whole => position == "whole",
                    DocPart::MboxMessage { index } => position == format!("message {index}"),
                    DocPart::Attachment { .. } => false,
                }
        }),
        Some(parent) => {
            let Ok(parent) = gt_document(project, gt, file, parent)? else {
                return Ok(Err(position));
            };
            // "part 2: invoice.pdf"
            let section = position
                .strip_prefix("part ")
                .and_then(|p| p.split_once(": "))
                .map(|(s, _)| s);
            gt.documents.iter().find(|d| {
                d.parent.as_deref() == Some(parent.as_str())
                    && matches!(&d.part, DocPart::Attachment { mime_part, .. }
                        if Some(mime_part.as_str()) == section)
            })
        }
    };
    Ok(id.map(|d| d.id.clone()).ok_or(position))
}

/// A stored locator in ground-truth terms. `cells` gives a CSV row's cell starts.
fn place<'a>(locator: &Locator, cells: impl Fn(u64) -> Option<&'a [u32]>) -> Option<Place> {
    let span = locator.span();
    let (start, end) = span.map_or((0, 0), |s| (s.start() as u32, s.end() as u32));
    Some(match locator {
        Locator::PdfText { page, .. } | Locator::OcrText { page, .. } => {
            Place::Page { page: page.get() }
        }
        Locator::TextLine { line, .. } => Place::Line {
            line: line.get(),
            start,
            end,
        },
        Locator::CsvCell { row, .. } => {
            let starts = cells(*row)?;
            // The cell's index in its row, whatever the column is named.
            let i = starts.partition_point(|c| *c <= start).checked_sub(1)?;
            Place::Cell {
                row: *row as u32,
                column: i as u32,
                start: start - starts[i],
                end: end - starts[i],
            }
        }
        Locator::DocxParagraph { paragraph, .. } => Place::Paragraph {
            paragraph: *paragraph,
            start,
            end,
        },
        Locator::JsonValue { pointer, .. } => Place::Json {
            pointer: pointer.clone(),
            start,
            end,
        },
        Locator::EmailHeader { name, .. } => Place::Header {
            name: name.to_lowercase(),
            start,
            end,
        },
        Locator::MimePart { part, .. } => Place::Part {
            part: part.clone(),
            start,
            end,
        },
        Locator::HtmlText { .. } => Place::Html,
        Locator::CsvRow { .. } => return None,
    })
}

fn percent(v: Option<f64>) -> String {
    v.map_or_else(|| "-".into(), |v| format!("{v:.1}"))
}

fn print_report(report: &Report, failed: usize) {
    println!("Identifiers on TD-1 (recall over all files, precision over fully annotated ones)");
    println!(
        "{:<10} {:>8} {:>8} {:>10} {:>8} {:>10}",
        "type", "expected", "recall", "text", "OCR'd", "precision"
    );
    let types: BTreeSet<&str> = report
        .rows
        .keys()
        .filter_map(|k| k.strip_suffix("/all"))
        .collect();
    for t in types {
        let all = report.rows[&format!("{t}/all")];
        let class = |c: &str| {
            percent(
                report
                    .rows
                    .get(&format!("{t}/{c}"))
                    .and_then(|r| r.recall()),
            )
        };
        println!(
            "{:<10} {:>8} {:>8} {:>10} {:>8} {:>10}",
            t,
            all.expected,
            percent(all.recall()),
            class("text"),
            class("ocr"),
            percent(all.precision()),
        );
    }
    let tiers: Vec<String> = ["clean", "office", "poor"]
        .iter()
        .filter_map(|t| {
            let r = report.rows.get(&format!("all/ocr-{t}"))?;
            Some(format!("{t} {} ({})", percent(r.recall()), r.expected))
        })
        .collect();
    if !tiers.is_empty() {
        println!("OCR'd recall by scan tier: {}", tiers.join(", "));
    }
    println!();
    for t in &report.targets {
        println!(
            "{:<10} {:<26} {:>6} (target {}) {}",
            t.id,
            t.what,
            percent(t.value),
            t.goal,
            if t.met { "met" } else { "NOT MET" }
        );
    }
    println!();
    println!(
        "Negatives    {} checked, {} failed{}",
        report.negatives_checked,
        report.negative_failures.len(),
        if report.negative_failures.is_empty() {
            String::new()
        } else {
            format!(": {}", report.negative_failures.join(", "))
        }
    );
    if !report.unmapped.is_empty() {
        println!(
            "Unmapped     {} stored documents match no ground-truth document:",
            report.unmapped.len()
        );
        for u in &report.unmapped {
            println!("             {u}");
        }
    }
    if !report.missed.is_empty() {
        println!("Missed       {}", report.missed.join(" "));
    }
    println!("Failed files {failed}");
}
