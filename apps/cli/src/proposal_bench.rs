//! `proposal-bench` (M7a, spike S7, K9): the merge proposals of a project a model already filled, with
//! no model: a sweep of thresholds against the ground truth, the link score (NFR-QA-01/02) as it
//! stands, and the link score after accepting and rejecting by the ground truth, for information.
//! `llm-bench` reports the same on each run. Counts go to stdout; names only into the `--json` file.
//!
//! The decisions are made on a copy, so the project itself stays as it was.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mk_bench::links::{
    FoundLink, LinkScore, ProposalPoint, ScoredPair, annotated_documents, entity_names,
    proposal_point, score_links,
};
use mk_bench::{EntityKind, GroundTruth, keys};
use mk_core::{ConnectionQuery, MergeEdit, PROPOSAL_THRESHOLD, Project, ProposalCursor};
use serde::Serialize;
use serde_json::{Value, json};

use crate::bench::gt_document;

/// The two parts of TD-1, each a root of the one project.
pub(crate) const PARTS: [&str; 2] = ["synthetic", "public"];

const SWEEP: [f32; 8] = [0.55, 0.6, 0.65, 0.7, 0.75, 0.8, 0.85, 0.9];
const PAGE: usize = 1_000;
/// Rounds of accepting by the ground truth: each round's merges can make new proposals.
const MAX_ROUNDS: usize = 8;

pub struct Options {
    pub project: PathBuf,
    pub testdata: PathBuf,
    pub json: Option<PathBuf>,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Stored document → its ground-truth ids.
pub(crate) type GtDocuments = BTreeMap<i64, Vec<String>>;

pub fn run(opts: &Options) -> ExitCode {
    match run_inner(opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run_inner(opts: &Options) -> Result<()> {
    if !mk_core::project_exists(&opts.project) {
        return Err(format!("no project in {}", opts.project.display()).into());
    }
    let gt = mk_bench::load(&opts.testdata.join("ground-truth.json"))?;
    let (project, _) = Project::open(&opts.project)?;
    let roots = roots_of(&project)?;
    let (documents, _) = stored_documents(&project, &gt, &roots)?;
    let sweep = sweep(&project, &gt, &documents)?;
    let links = link_score(&project, &gt, &documents)?;
    drop(project);
    let decided = decided(&opts.project, &gt)?;

    println!(
        "{:>9} {:>9} {:>6} {:>7} {:>9} {:>8} {:>9} {:>6}",
        "threshold", "proposals", "judged", "correct", "precision", "variants", "reached", "passes"
    );
    for p in &sweep.points {
        println!(
            "{:>9.2} {:>9} {:>6} {:>7} {:>8.1}% {:>8} {:>8.1}% {:>6}",
            p.threshold,
            p.proposals,
            p.judged,
            p.correct,
            p.precision(),
            p.variants_found,
            p.coverage(),
            if p.passes() { "yes" } else { "no" },
        );
    }
    print_links("links, as they stand", &links);
    print_links("links, decided by the ground truth", &decided.links);
    println!(
        "decided        {} accepted, {} rejected, {} left in {} rounds; rebuild matches: {}, healthy: {}",
        decided.accepted,
        decided.rejected,
        decided.left,
        decided.rounds,
        decided.rebuild_matches,
        decided.healthy,
    );
    if let Some(path) = &opts.json {
        let report = json!({
            "sweep": sweep.points,
            "pairs": sweep.pairs,
            "links": links,
            "decided": decided,
        });
        let mut text = serde_json::to_string_pretty(&report)?;
        text.push('\n');
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, text)?;
    }
    Ok(())
}

pub(crate) fn print_links(title: &str, s: &LinkScore) {
    println!(
        "{title}: recall {:.1}% ({} of {}; identifiers {} of {}, entities {} of {}), precision {:.1}% ({} of {}; identifiers {} of {}, entities {} of {}), {} not judged",
        s.recall(),
        s.gold_found,
        s.gold,
        s.gold_identifier_found,
        s.gold_identifier,
        s.gold_entity_found,
        s.gold_entity,
        s.precision(),
        s.correct,
        s.judged,
        s.correct_identifier,
        s.judged_identifier,
        s.correct_entity,
        s.judged_entity,
        s.not_judged,
    );
}

/// Root id → TD-1 part, by the root's folder name.
fn roots_of(project: &Project) -> Result<BTreeMap<i64, String>> {
    let mut out = BTreeMap::new();
    for root in project.store().source_roots()? {
        let name = Path::new(&root.path).file_name().and_then(|n| n.to_str());
        if let Some(part) = name.filter(|n| PARTS.contains(n)) {
            out.insert(root.id, part.to_string());
        }
    }
    Ok(out)
}

/// Stored document → its ground-truth ids, and the stored documents the ground truth does not have.
/// A file found under two paths counts for both.
pub(crate) fn stored_documents(
    project: &Project,
    gt: &GroundTruth,
    roots: &BTreeMap<i64, String>,
) -> Result<(GtDocuments, Vec<String>)> {
    let store = project.store();
    let mut ids = GtDocuments::new();
    let mut unmapped = Vec::new();
    for r in store.stored_refs()? {
        let Some(part) = roots.get(&r.root).filter(|_| r.present) else {
            continue;
        };
        let file = format!("{part}/{}", r.rel_path);
        let Some(content) = store.content(&r.hash)? else {
            continue;
        };
        let mut documents: Vec<i64> = store
            .segments(content.id)?
            .iter()
            .map(|s| s.document_id)
            .collect();
        documents.sort_unstable();
        documents.dedup();
        for d in documents {
            match gt_document(project, gt, &file, d)? {
                Ok(id) => ids.entry(d).or_default().push(id),
                Err(position) => unmapped.push(format!("{file} ({position})")),
            }
        }
    }
    Ok((ids, unmapped))
}

/// The connection rows shown by default, in ground-truth terms.
fn found_links(project: &Project, documents: &GtDocuments) -> Result<Vec<FoundLink>> {
    let query = ConnectionQuery::default();
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let page = project.connections_page(&query, after.as_ref(), PAGE)?;
        for c in &page.rows {
            let kind = match c.kind.as_str() {
                "person" => Some(EntityKind::Person),
                "org" => Some(EntityKind::Org),
                _ => None,
            };
            let mut link = FoundLink {
                key: c.row.subject_key.clone(),
                names: BTreeSet::new(),
                documents: BTreeSet::new(),
            };
            for o in project.occurrences(&c.row.subject_key)? {
                link.documents
                    .extend(documents.get(&o.document_id).into_iter().flatten().cloned());
                if let Some(kind) = kind {
                    link.names.insert(keys::name_key(kind, &o.raw));
                }
            }
            out.push(link);
        }
        match page.next {
            Some(next) => after = Some(next),
            None => return Ok(out),
        }
    }
}

/// NFR-QA-01/02 on the rows shown by default.
pub(crate) fn link_score(
    project: &Project,
    gt: &GroundTruth,
    documents: &GtDocuments,
) -> Result<LinkScore> {
    Ok(score_links(gt, &found_links(project, documents)?))
}

pub(crate) struct Sweep {
    pub points: Vec<ProposalPoint>,
    /// Every pair at the lowest threshold swept, names included (JSON only).
    pub pairs: Vec<Value>,
}

impl Sweep {
    /// The point at the threshold in use.
    pub fn in_use(&self) -> ProposalPoint {
        self.points
            .iter()
            .find(|p| p.threshold == PROPOSAL_THRESHOLD)
            .cloned()
            .unwrap_or_default()
    }
}

/// Spike S7: the proposals at each threshold, against the ground truth.
pub(crate) fn sweep(project: &Project, gt: &GroundTruth, documents: &GtDocuments) -> Result<Sweep> {
    let annotated = annotated_documents(gt);
    let store = project.store();
    let present: BTreeSet<String> = store
        .proposal_input()?
        .candidates
        .into_iter()
        .flat_map(|c| c.name_keys)
        .collect();
    let rows: Vec<(i64, String)> = store.read_raw(|c| {
        let mut stmt = c.prepare("SELECT DISTINCT document_id, name_key FROM entity_mention")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    })?;
    let scored: BTreeSet<String> = rows
        .into_iter()
        .filter(|(d, _)| {
            documents
                .get(d)
                .is_some_and(|ids| ids.iter().any(|id| annotated.contains(id.as_str())))
        })
        .map(|(_, key)| key)
        .filter(|key| present.contains(key))
        .collect();
    let proposals = project.score_proposals(SWEEP[0])?;
    let pairs: Vec<ScoredPair> = proposals
        .iter()
        .map(|p| ScoredPair {
            a: p.a.clone(),
            b: p.b.clone(),
            score: p.score,
            reasons: p.reasons.len(),
        })
        .collect();
    let truth = entity_names(gt);
    Ok(Sweep {
        points: SWEEP
            .iter()
            .map(|t| proposal_point(gt, &pairs, &present, &scored, *t))
            .collect(),
        pairs: proposals
            .iter()
            .map(|p| {
                json!({
                    "a": p.a,
                    "b": p.b,
                    "score": p.score,
                    "reasons": p.reasons,
                    "judged": scored.contains(&p.a) && scored.contains(&p.b),
                    "correct": truth.get(&p.a).zip(truth.get(&p.b))
                        .is_some_and(|(a, b)| a.intersection(b).next().is_some()),
                })
            })
            .collect(),
    })
}

/// The link score after accepting and rejecting by the ground truth (information only).
#[derive(Serialize, Clone, PartialEq, Debug)]
pub(crate) struct Decided {
    pub links: LinkScore,
    pub accepted: usize,
    pub rejected: usize,
    /// Proposals left: a side the ground truth does not have.
    pub left: usize,
    pub rounds: usize,
    pub rebuild_matches: bool,
    pub healthy: bool,
}

/// Copies the closed project at `dir` and decides its proposals by the ground truth: accepted when
/// both sides are names of one gold entity, rejected when both are names and they are not, and left
/// when a side is not a gold name.
pub(crate) fn decided(dir: &Path, gt: &GroundTruth) -> Result<Decided> {
    let copy = tempfile::tempdir()?;
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            std::fs::copy(entry.path(), copy.path().join(entry.file_name()))?;
        }
    }
    let (project, _) = Project::open(copy.path())?;
    let truth = entity_names(gt);
    let (mut accepted, mut rejected, mut left, mut rounds) = (0, 0, 0, 0);
    while rounds < MAX_ROUNDS {
        rounds += 1;
        project.refresh_proposals()?;
        let mut acted = false;
        left = 0;
        for p in all_proposals(&project)? {
            let (Some(ea), Some(eb)) = (truth.get(&p.a), truth.get(&p.b)) else {
                left += 1;
                continue;
            };
            if ea.intersection(eb).next().is_some() {
                // A merge earlier in this round can have ended this proposal.
                if project
                    .accept_proposal(&p.a, &p.b, MergeEdit::default())
                    .is_ok()
                {
                    accepted += 1;
                    acted = true;
                }
            } else {
                project.reject_proposal(&p.a, &p.b)?;
                rejected += 1;
                acted = true;
            }
        }
        if !acted {
            break;
        }
    }
    let roots = roots_of(&project)?;
    let (documents, _) = stored_documents(&project, gt, &roots)?;
    let links = link_score(&project, gt, &documents)?;
    let rebuilt = project.rebuild()?;
    Ok(Decided {
        links,
        accepted,
        rejected,
        left,
        rounds,
        rebuild_matches: rebuilt.same_as_incremental && rebuilt.consistent,
        healthy: project.check_health()?.is_healthy(),
    })
}

fn all_proposals(project: &Project) -> Result<Vec<mk_core::ProposalRow>> {
    let mut rows = Vec::new();
    let mut after: Option<ProposalCursor> = None;
    loop {
        let page = project.proposals(after.as_ref(), PAGE)?;
        let Some(last) = page.last() else {
            return Ok(rows);
        };
        after = Some(ProposalCursor {
            score: last.score,
            a: last.a.clone(),
            b: last.b.clone(),
        });
        rows.extend(page);
    }
}
