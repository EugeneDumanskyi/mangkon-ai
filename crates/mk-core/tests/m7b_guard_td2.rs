//! M7b guard, no NFR behind it: how long the entity page and a queue card take on TD-2's biggest
//! entities (ASSUMPTION: p95 at most 200 ms). The calls are what the screens make, without the IPC.
//!
//! Machine-dependent and slow, so `#[ignore]`. The first test keeps a project with no decisions in
//! it and needs no idle machine. The second works on a copy of that project and does:
//!
//! ```sh
//! cargo build -p mangkon-cli --release
//! MK_WORKER_PROGRAM=target/release/mangkon-cli MK_TD2_SOURCE=testdata/td2 MK_TD2_PROJECT=<dir> \
//!   cargo test -p mk-core --release --test m7b_guard_td2 -- --ignored keep_a_td2_project --nocapture
//! MK_TD2_PROJECT=<dir> \
//!   cargo test -p mk-core --release --test m7b_guard_td2 -- --ignored m7b_guard_td2 --nocapture
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

use mk_core::{Assignments, DocumentFilter, MergeEdit, Project, ProposalRow};
use serde_json::json;

/// Calls per case. The first is reported on its own, since the screen often makes only that one.
const CALLS: usize = 100;
/// `EVIDENCE_PER_SIDE`, `EVIDENCE_ON_PAGE` and `PAGE_SIZE` in the UI.
const EVIDENCE_PER_SIDE: usize = 5;
const EVIDENCE_ON_PAGE: usize = 1_000;
const QUEUE_PAGE: usize = 200;
const TARGET_MS: f64 = 200.0;

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("{name}: see the file header")))
}

/// Stage A on all of TD-2, each table confirmed as suggested, and proposals for what that gives.
#[test]
#[ignore = "M7b guard: makes a TD-2 project, MK_TD2_SOURCE, MK_TD2_PROJECT, MK_WORKER_PROGRAM"]
fn keep_a_td2_project() {
    let out = env_path("MK_TD2_PROJECT");
    let source = env_path("MK_TD2_SOURCE");
    let (mut project, _) = Project::open(&out).unwrap();
    *project.worker_config_mut() =
        mk_core::WorkerConfig::with_program(env_path("MK_WORKER_PROGRAM"));
    project.set_tessdata_dir(Some(out.with_extension("tessdata")));
    project.add_source_root(&source).unwrap();
    project.rescan(false).unwrap();

    let t = Instant::now();
    let run = project.run_stage_a().unwrap();
    eprintln!(
        "stage A: {} done, {} failed, {:.0} s",
        run.done,
        run.failed,
        t.elapsed().as_secs_f64()
    );

    let t = Instant::now();
    let mut tables = Vec::new();
    let mut after = None;
    loop {
        let page = project
            .documents_page(&DocumentFilter::default(), after.as_ref(), 500)
            .unwrap();
        tables.extend(
            page.rows
                .iter()
                .filter(|r| r.mapping_state.is_some())
                .map(|r| r.ref_id),
        );
        match page.next {
            Some(next) => after = Some(next),
            None => break,
        }
    }
    for id in &tables {
        let Some(view) = project.mapping_view(*id).unwrap() else {
            continue;
        };
        let suggested = view
            .columns
            .iter()
            .map(|c| (c.key.clone(), c.suggested))
            .collect();
        project
            .confirm_mapping(*id, Assignments(suggested))
            .unwrap();
    }
    eprintln!(
        "{} tables confirmed, {:.0} s",
        tables.len(),
        t.elapsed().as_secs_f64()
    );

    let t = Instant::now();
    let stored = project.refresh_proposals().unwrap();
    eprintln!("{stored} proposals, {:.1} s", t.elapsed().as_secs_f64());
}

fn copy_project(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
        }
    }
}

/// Times `call` and says how it did against the target.
fn time(what: &str, mut call: impl FnMut()) -> serde_json::Value {
    let mut ms: Vec<f64> = (0..CALLS)
        .map(|_| {
            let t = Instant::now();
            call();
            t.elapsed().as_secs_f64() * 1000.0
        })
        .collect();
    let first = ms[0];
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    let result = json!({
        "what": what,
        "n": CALLS,
        "first_ms": first,
        "p50_ms": at(0.5),
        "p95_ms": at(0.95),
        "max_ms": at(1.0),
        "within_target": at(0.95) <= TARGET_MS,
    });
    eprintln!("{result}");
    result
}

/// What the entity page reads when it opens.
fn open_page(project: &Project, key: &str) {
    project.entity_detail(key, false).unwrap().unwrap();
    project
        .entity_evidence(key, EVIDENCE_ON_PAGE, false)
        .unwrap();
}

/// What a queue card reads, for both its sides.
fn open_card(project: &Project, left: &str, right: &str) {
    for key in [left, right] {
        project.entity_detail(key, false).unwrap().unwrap();
        project
            .entity_evidence(key, EVIDENCE_PER_SIDE, false)
            .unwrap();
    }
}

#[test]
#[ignore = "M7b guard: needs the project from keep_a_td2_project and an idle machine, MK_TD2_PROJECT"]
fn m7b_guard_td2() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    copy_project(&env_path("MK_TD2_PROJECT"), &dir);
    let (project, _) = Project::open(&dir).unwrap();

    let state = project.store().derived_state().unwrap();
    let count = |key: &str| state.entities.get(key).map_or(0, |e| e.mention_count);
    let hottest = state
        .entities
        .iter()
        .max_by_key(|(_, e)| e.mention_count)
        .map(|(key, _)| key.clone())
        .expect("no entities: was the project made by keep_a_td2_project?");

    let mut proposals: Vec<ProposalRow> = Vec::new();
    loop {
        let page = project
            .proposals(
                proposals
                    .last()
                    .map(|r| mk_core::ProposalCursor {
                        score: r.score,
                        a: r.a.clone(),
                        b: r.b.clone(),
                    })
                    .as_ref(),
                QUEUE_PAGE,
            )
            .unwrap();
        let done = page.len() < QUEUE_PAGE;
        proposals.extend(page);
        if done {
            break;
        }
    }
    let busiest = proposals
        .iter()
        .max_by_key(|p| count(&p.left_key) + count(&p.right_key))
        .expect("no proposals: was the project made by keep_a_td2_project?")
        .clone();

    let detail = project.entity_detail(&hottest, false).unwrap().unwrap();
    let times = vec![
        time("entity page, hottest", || open_page(&project, &hottest)),
        time("queue card, hottest on both sides", || {
            open_card(&project, &hottest, &hottest);
        }),
        time("queue card, busiest proposal", || {
            open_card(&project, &busiest.left_key, &busiest.right_key);
        }),
        time("queue list, first page and count", || {
            project.proposals(None, QUEUE_PAGE).unwrap();
            project.proposal_count(None).unwrap();
        }),
    ];

    // A merged group reads its merges too; this is why the work is on a copy.
    project
        .accept_proposal(&busiest.a, &busiest.b, MergeEdit::default())
        .unwrap();
    let merged = time("entity page, busiest proposal once merged", || {
        open_page(&project, &busiest.a);
    });

    let summary = json!({
        "entities": state.entities.len(),
        "proposals": proposals.len(),
        "hottest": {
            "mentions": detail.mention_count,
            "evidence_total": detail.evidence_total,
            "documents": detail.documents_total,
            "linked_identifiers": detail.linked.total,
        },
        "busiest_proposal_mentions": [count(&busiest.left_key), count(&busiest.right_key)],
        "target_p95_ms": TARGET_MS,
        "times": times,
        "merged": merged,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
