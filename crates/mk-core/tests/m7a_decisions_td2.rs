//! M7a criterion 2 (NFR-REL-09, NFR-MNT-07, INV-CU-06): 100% of decisions survive a re-scan, a model
//! change and a schema migration on a copy of TD-2.
//!
//! The project comes from the app as it was before M7a (schema v10, derived rows v5), with its own
//! decisions in it, and this opens it with today's code. Then it adds every kind of decision in bulk,
//! re-scans with files changed, gone and restored, changes the model through the store with a
//! stand-in's answers, and migrates to a made-up v11. After each step the log is byte for byte what
//! it was, and each decision still has its effect. Proposal time and memory on all of TD-2's entities
//! are reported too (ASSUMPTION, M7a: at most 60 s, off the writer).
//!
//! Machine-dependent and slow, so `#[ignore]`; run on RM-1 when it is idle:
//!
//! ```sh
//! MK_TD2_V10_PROJECT=<dir> cargo test -p mk-core --release -- --ignored m7a_decisions_td2 --nocapture
//! ```
//!
//! `docs/17-entity-resolution.md` §3 says how the project was made. A real model is not used: TD-2
//! would take it about 31 hours. The real model-change path is on TD-1 (`llm-bench`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use mk_ai::{CheckedMention, RunIdentity};
use mk_core::{
    Assignments, ColumnType, DecisionStatus, DocumentFilter, EntityType, MergeEdit, Project,
    WorkerConfig,
};
use mk_jobs::Trigger;
use mk_kernel::NameKey;
use mk_store::{AiItemResult, MIGRATIONS, Migration, RunBinding, Store, UnitScope};
use serde_json::json;

/// Decisions per kind (merges as there are proposals, up to this).
const BULK: usize = 1_000;
const V11: Migration = Migration {
    version: 11,
    name: "made up for this test",
    sql: "CREATE TABLE later_feature (x INTEGER);",
};

type RawDecision = (i64, String, String, Option<String>, Option<String>, String);

fn raw_log(c: &rusqlite::Connection) -> Vec<RawDecision> {
    let mut stmt = c
        .prepare("SELECT seq, kind, anchor, before, after, decided_at FROM decision ORDER BY seq")
        .unwrap();
    stmt.query_map([], |r| {
        Ok((
            r.get(0)?,
            r.get(1)?,
            r.get(2)?,
            r.get(3)?,
            r.get(4)?,
            r.get(5)?,
        ))
    })
    .unwrap()
    .collect::<Result<_, _>>()
    .unwrap()
}

fn store_log(store: &Store) -> Vec<RawDecision> {
    store.read_raw(|c| Ok(raw_log(c))).unwrap()
}

/// What each decision should still be doing, on entities no other decision here touches.
#[derive(Default)]
struct Expected {
    merged: Vec<(String, String)>,
    rejected: Vec<(String, String)>,
    undone: Vec<(String, String)>,
    /// Split key → how many mentions it holds.
    splits: Vec<(String, u64)>,
    undone_splits: Vec<String>,
    names: Vec<(String, String)>,
    types: Vec<String>,
    notes: Vec<(String, String)>,
    aliases: Vec<(String, String)>,
    ignored: Vec<String>,
    /// From the old log.
    old_ignored: Vec<String>,
}

impl Expected {
    fn total(&self) -> usize {
        self.merged.len()
            + self.rejected.len()
            + self.undone.len()
            + self.splits.len()
            + self.undone_splits.len()
            + self.names.len()
            + self.types.len()
            + self.notes.len()
            + self.aliases.len()
            + self.ignored.len()
            + self.old_ignored.len()
    }

    /// Every decision's effect, or a panic naming the one that lost it.
    fn check(&self, store: &Store, step: &str) {
        let t = Instant::now();
        let state = store.derived_state().unwrap();
        let of = |key: &str| {
            state
                .entity_members
                .get(key)
                .cloned()
                .unwrap_or_else(|| key.to_string())
        };
        for (a, b) in &self.merged {
            assert_eq!(of(a), of(b), "{step}: merge {a} {b}");
        }
        let anchor = |a: &str, b: &str| {
            if a <= b {
                format!("{a}|{b}")
            } else {
                format!("{b}|{a}")
            }
        };
        for (a, b) in self.rejected.iter().chain(&self.undone) {
            assert_eq!(
                state.merge_edges.get(&anchor(a, b)),
                Some(&false),
                "{step}: {a} {b}"
            );
            assert_ne!(of(a), of(b), "{step}: {a} {b} stay apart");
        }
        for (split, count) in &self.splits {
            assert_eq!(
                state.entities.get(split).map(|e| e.mention_count),
                Some(*count),
                "{step}: split {split}"
            );
        }
        for split in &self.undone_splits {
            assert!(
                !state.entities.contains_key(split),
                "{step}: undone {split}"
            );
        }
        for (key, name) in &self.names {
            assert_eq!(
                &state.entities[key].display_name, name,
                "{step}: rename {key}"
            );
        }
        for key in &self.types {
            assert_eq!(state.entities[key].entity_type, "org", "{step}: type {key}");
        }
        for (key, note) in &self.notes {
            assert_eq!(
                state.entities[key].note.as_ref(),
                Some(note),
                "{step}: note {key}"
            );
        }
        for (key, alias) in &self.aliases {
            let names = store.entity_names(key).unwrap().unwrap();
            assert!(
                names.removed.iter().any(|(n, _)| n == alias),
                "{step}: alias {alias} of {key}: {names:?}"
            );
        }
        for key in self.ignored.iter().chain(&self.old_ignored) {
            if let Some(row) = state.connections.get(key) {
                assert!(row.ignored, "{step}: ignore {key}");
            }
        }
        eprintln!(
            "{step}: {} decisions hold ({:.1} s to check)",
            self.total(),
            t.elapsed().as_secs_f64()
        );
    }
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

fn peak_rss_mib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok())
        })
        .map_or(0, |kib| kib / 1024)
}

fn binding(name: &str) -> RunBinding {
    RunBinding {
        identity: RunIdentity::local(name),
        model_name: name.into(),
        destination_host: "127.0.0.1".into(),
        destination_class: "local".into(),
        prompt_version: 1,
    }
}

/// Every item a job gave, answered as a stand-in model would: it finds `names` wherever the text has
/// them.
fn answer_all(store: &Store, names: &[&str]) {
    while let Some(item) = store.start_next_ai_item().unwrap() {
        let mut mentions = Vec::new();
        for piece in &item.pieces {
            for name in names {
                for (at, _) in piece.text.match_indices(name) {
                    let start = piece.text[..at].chars().count() + piece.start;
                    mentions.push(CheckedMention {
                        segment: piece.segment,
                        key: NameKey::new(EntityType::Person, name).unwrap(),
                        entity_type: EntityType::Person,
                        raw: name.to_string(),
                        span: (start, start + name.chars().count()),
                        confidence: 0.9,
                    });
                }
            }
        }
        store
            .commit_ai_item(
                item.item,
                item.attempt,
                AiItemResult {
                    mentions,
                    ..AiItemResult::default()
                },
            )
            .unwrap();
    }
}

fn confirm(project: &Project, file: &str) {
    let rows = project
        .documents_page(
            &DocumentFilter {
                ..DocumentFilter::default()
            },
            None,
            100_000,
        )
        .unwrap()
        .rows;
    let row = rows
        .iter()
        .find(|r| r.rel_path == file && r.mapping_state.is_some())
        .unwrap_or_else(|| panic!("{file} is not a table"));
    project
        .confirm_mapping(
            row.ref_id,
            Assignments(
                [
                    ("name".to_string(), ColumnType::PersonName),
                    ("email".to_string(), ColumnType::Email),
                ]
                .into(),
            ),
        )
        .unwrap();
}

/// How long each kind of decision took, the call alone.
#[derive(Default)]
struct Timings(BTreeMap<&'static str, Vec<f64>>);

impl Timings {
    fn time<T>(&mut self, kind: &'static str, call: impl FnOnce() -> T) -> T {
        let t = Instant::now();
        let out = call();
        self.0
            .entry(kind)
            .or_default()
            .push(t.elapsed().as_secs_f64() * 1000.0);
        out
    }

    fn report(mut self) -> serde_json::Value {
        let by_kind: BTreeMap<&str, serde_json::Value> = self
            .0
            .iter_mut()
            .map(|(kind, ms)| {
                ms.sort_by(f64::total_cmp);
                let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
                (
                    *kind,
                    json!({"n": ms.len(), "p50_ms": at(0.5), "p95_ms": at(0.95), "max_ms": at(1.0)}),
                )
            })
            .collect();
        json!(by_kind)
    }
}

/// Makes every kind of decision on real entities, each kind on a part of its own.
fn decide_in_bulk(project: &Project, expected: &mut Expected) -> serde_json::Value {
    let t = Instant::now();
    let mut times = Timings::default();
    let state = project.store().derived_state().unwrap();
    let pending = project.proposals(None, 3 * BULK).unwrap();
    let mut used: BTreeSet<String> = BTreeSet::new();
    let mut fresh = |a: &str, b: &str| used.insert(a.to_string()) & used.insert(b.to_string());
    let free_pairs: Vec<(String, String)> = pending
        .iter()
        .filter(|p| fresh(&p.a, &p.b))
        .map(|p| (p.a.clone(), p.b.clone()))
        .collect();
    let third = free_pairs.len() / 3;
    for (a, b) in free_pairs.iter().take(third) {
        times.time("merge", || {
            project.accept_proposal(a, b, MergeEdit::default()).unwrap();
        });
        expected.merged.push((a.clone(), b.clone()));
    }
    for (a, b) in free_pairs.iter().skip(third).take(third) {
        times.time("reject", || project.reject_proposal(a, b).unwrap());
        expected.rejected.push((a.clone(), b.clone()));
    }
    for (a, b) in free_pairs.iter().skip(2 * third) {
        times.time("merge", || {
            project.accept_proposal(a, b, MergeEdit::default()).unwrap();
        });
        times.time("undo merge", || project.undo_merge(a, b).unwrap());
        expected.undone.push((a.clone(), b.clone()));
    }
    // A single name has no alias, so aliases come off the merged groups.
    for (a, b) in free_pairs.iter().take(third) {
        let key = a.min(b);
        if let Some(names) = project.entity_names(key).unwrap()
            && let Some((alias, _)) = names.aliases.first()
        {
            times.time("remove alias", || project.remove_alias(key, alias).unwrap());
            expected.aliases.push((key.clone(), alias.clone()));
        }
    }
    let singles: Vec<&String> = state
        .entities
        .iter()
        .filter(|(key, e)| {
            e.mention_count >= 3
                && !used.contains(*key)
                && !key.starts_with("split:")
                && !state.entity_members.contains_key(*key)
        })
        .map(|(key, _)| key)
        .collect();
    eprintln!(
        "{} proposals used, {} entities free for edits",
        free_pairs.len(),
        singles.len()
    );
    let mut parts = singles.chunks(BULK.min(singles.len() / 7).max(1));
    let split_one = |times: &mut Timings, key: &str| {
        let one: Vec<i64> = project
            .occurrences(key)
            .unwrap()
            .iter()
            .take(1)
            .map(|o| o.mention_id)
            .collect();
        times.time("split", || project.split_mentions(key, &one).unwrap().key)
    };
    for key in parts.next().unwrap_or_default() {
        let split = split_one(&mut times, key);
        expected.splits.push((split, 1));
    }
    for key in parts.next().unwrap_or_default() {
        let split = split_one(&mut times, key);
        times.time("undo split", || project.undo_split(&split).unwrap());
        expected.undone_splits.push(split);
    }
    for (i, key) in parts.next().unwrap_or_default().iter().enumerate() {
        let name = format!("Renamed {i}");
        times.time("rename", || project.rename_entity(key, &name).unwrap());
        expected.names.push(((*key).clone(), name));
    }
    for key in parts.next().unwrap_or_default() {
        times.time("change type", || {
            project.change_entity_type(key, EntityType::Org).unwrap()
        });
        expected.types.push((*key).clone());
    }
    for (i, key) in parts.next().unwrap_or_default().iter().enumerate() {
        let note = format!("Note {i}");
        times.time("note", || project.set_entity_note(key, &note).unwrap());
        expected.notes.push(((*key).clone(), note));
    }
    for key in parts.next().unwrap_or_default() {
        times.time("ignore", || project.set_ignored(key, true).unwrap());
        expected.ignored.push((*key).clone());
    }
    eprintln!(
        "{} decisions made in {:.1} s",
        expected.total(),
        t.elapsed().as_secs_f64()
    );
    assert!(
        !expected.merged.is_empty()
            && !expected.rejected.is_empty()
            && !expected.undone.is_empty()
            && !expected.splits.is_empty()
            && !expected.undone_splits.is_empty()
            && !expected.names.is_empty()
            && !expected.types.is_empty()
            && !expected.notes.is_empty()
            && !expected.aliases.is_empty()
            && !expected.ignored.is_empty(),
        "every kind has to be in the run"
    );
    times.report()
}

#[test]
#[ignore = "M7a criterion 2: needs a TD-2 project made before M7a, MK_TD2_V10_PROJECT"]
fn m7a_decisions_td2() {
    let from = PathBuf::from(
        std::env::var_os("MK_TD2_V10_PROJECT").expect("MK_TD2_V10_PROJECT: see the file header"),
    );
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("project");
    copy_project(&from, &dir);

    // What the old code left, read before today's code touches it.
    let db = dir.join("project.db");
    let (old_log, version, derived): (Vec<RawDecision>, u32, String) = {
        let c = rusqlite::Connection::open(&db).unwrap();
        (
            raw_log(&c),
            c.query_row("PRAGMA user_version", [], |r| r.get(0))
                .unwrap(),
            c.query_row(
                "SELECT value FROM project_meta WHERE key = 'derived_version'",
                [],
                |r| r.get(0),
            )
            .unwrap(),
        )
    };
    assert_eq!(
        (version, derived.as_str()),
        (10, "5"),
        "not a project from before M7a"
    );
    assert!(!old_log.is_empty(), "the project needs its own decisions");

    let t = Instant::now();
    let (mut project, _) = Project::open(&dir).unwrap();
    let open_s = t.elapsed().as_secs_f64();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.set_tessdata_dir(Some(tmp.path().join("tessdata")));
    assert_eq!(
        store_log(project.store()),
        old_log,
        "opening changed the log"
    );
    let mut expected = Expected::default();
    let statuses = project.store().derived_state().unwrap().decision_status;
    for (seq, kind, anchor, ..) in &old_log {
        if kind.starts_with("ignore") && statuses.get(seq) == Some(&DecisionStatus::Active) {
            expected.old_ignored.push(anchor.clone());
        }
    }
    assert!(!expected.old_ignored.is_empty());
    expected.check(project.store(), "opened old project");

    // Proposal time and memory on all of it (ASSUMPTION: at most 60 s).
    let rss_before = peak_rss_mib();
    let t = Instant::now();
    let stored = project.refresh_proposals().unwrap();
    let proposal_s = t.elapsed().as_secs_f64();
    let rss_after = peak_rss_mib();
    let entities = project.store().derived_state().unwrap().entities.len();
    eprintln!("proposals: {stored} from {entities} entities in {proposal_s:.1} s");

    let decision_times = decide_in_bulk(&project, &mut expected);
    let log = store_log(project.store());
    assert_eq!(&log[..old_log.len()], &old_log[..]);
    expected.check(project.store(), "decided");

    // A root of its own whose files change, go and come back, holding decisions of their own.
    let extra = tmp.path().join("extra");
    std::fs::create_dir_all(&extra).unwrap();
    let table = |n: &str| {
        format!("name,email\nZed Extra,zed{n}@extra.example\nYan Extra,yan@extra.example\n")
    };
    for n in 1..=3 {
        std::fs::write(extra.join(format!("x{n}.csv")), table(&n.to_string())).unwrap();
    }
    std::fs::write(
        extra.join("note1.txt"),
        "Zelda Quorn signed.\nZ. Quorn paid.\n",
    )
    .unwrap();
    std::fs::write(
        extra.join("note2.txt"),
        "Zelda Quorn wrote.\nZ. Quorn called.\n",
    )
    .unwrap();
    let before_extra: i64 = project
        .store()
        .read_raw(|c| Ok(c.query_row("SELECT MAX(id) FROM content", [], |r| r.get(0))?))
        .unwrap();
    project.add_source_root(&extra).unwrap();
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    for n in 1..=3 {
        confirm(&project, &format!("x{n}.csv"));
    }
    let zed = NameKey::new(EntityType::Person, "Zed Extra")
        .unwrap()
        .to_string();
    let zed_rename = project.rename_entity(&zed, "Zed the Extra").unwrap();
    let log = store_log(project.store());
    let status = |p: &Project, seq: i64| p.store().derived_state().unwrap().decision_status[&seq];
    let check_log = |p: &Project, step: &str| {
        assert_eq!(store_log(p.store()), log, "{step}: the log changed");
    };

    // Re-scan: nothing, then a changed file, a gone file, a restored file.
    project.rescan(true).unwrap();
    project.run_stage_a().unwrap();
    check_log(&project, "re-scan");
    expected.check(project.store(), "re-scan");
    std::fs::write(
        extra.join("x1.csv"),
        "name,email\nOther Extra,other@extra.example\n",
    )
    .unwrap();
    std::fs::rename(extra.join("x2.csv"), tmp.path().join("x2.csv")).unwrap();
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    check_log(&project, "changed and gone");
    expected.check(project.store(), "changed and gone");
    assert_eq!(
        status(&project, zed_rename),
        DecisionStatus::Active,
        "x2 and x3 still hold it"
    );
    std::fs::write(
        extra.join("x3.csv"),
        "name,email\nOther Extra,other3@extra.example\n",
    )
    .unwrap();
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    check_log(&project, "changed again");
    std::fs::write(extra.join("x1.csv"), table("1")).unwrap();
    std::fs::rename(tmp.path().join("x2.csv"), extra.join("x2.csv")).unwrap();
    std::fs::write(extra.join("x3.csv"), table("3")).unwrap();
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    check_log(&project, "restored");
    expected.check(project.store(), "restored");
    assert_eq!(status(&project, zed_rename), DecisionStatus::Active);
    assert_eq!(
        project.store().derived_state().unwrap().entities[&zed].display_name,
        "Zed the Extra"
    );

    // Model change, through the store with a stand-in's answers, on the new notes.
    let contents: Vec<i64> = project
        .store()
        .read_raw(move |c| {
            let mut s = c.prepare("SELECT id FROM content WHERE id > ?1 AND file_type = 'txt'")?;
            let rows = s.query_map([before_extra], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    assert_eq!(contents.len(), 2);
    let store = project.store();
    store
        .schedule_ai_units(UnitScope::Contents(contents), Trigger::Add)
        .unwrap();
    store.bind_ai_run(binding("model-one")).unwrap();
    // Names TD-2 does not have, so the bulk step cannot have decided them already.
    let quorn = ["Zelda Quorn", "Z. Quorn"];
    answer_all(store, &quorn);
    project.refresh_proposals().unwrap();
    let zelda = NameKey::new(EntityType::Person, quorn[0])
        .unwrap()
        .to_string();
    let z_quorn = NameKey::new(EntityType::Person, quorn[1])
        .unwrap()
        .to_string();
    project
        .accept_proposal(&z_quorn, &zelda, MergeEdit::default())
        .unwrap();
    expected.merged.push((z_quorn.clone(), zelda.clone()));
    project.rename_entity(&z_quorn, "Zelda the One").unwrap();
    expected
        .names
        .push((z_quorn.clone(), "Zelda the One".into()));
    let log = store_log(project.store());
    let store = project.store();
    let units_before: i64 = store
        .read_raw(|c| Ok(c.query_row("SELECT COUNT(*) FROM ai_run", [], |r| r.get(0))?))
        .unwrap();
    store.start_ai_rerun(binding("model-two")).unwrap();
    // A re-run covers what the first run covered, the two notes.
    answer_all(store, &quorn);
    let active = store.active_ai_run().unwrap().unwrap();
    assert_eq!(active.model_name, "model-two", "the switch happened");
    assert!(units_before >= 1);
    assert_eq!(store_log(store), log, "model change: the log changed");
    expected.check(store, "model change");

    // The incremental rows still equal a full rebuild at this size.
    let t = Instant::now();
    let incremental = store.derived_state().unwrap();
    store.rebuild_all().unwrap();
    let rebuild_s = t.elapsed().as_secs_f64();
    assert_eq!(incremental, store.derived_state().unwrap());
    assert_eq!(store_log(store), log);
    let decisions = log.len();
    let by_status: BTreeMap<String, usize> =
        incremental
            .decision_status
            .values()
            .fold(BTreeMap::new(), |mut m, s| {
                *m.entry(s.to_string()).or_default() += 1;
                m
            });
    drop(project);

    // Schema migration: a made-up v11 carries the log forward; the rows stand on it.
    let migrations = [MIGRATIONS, &[V11]].concat();
    let (store, report) = Store::open_with(&db, &dir.join("backups-v11"), &migrations).unwrap();
    assert_eq!((report.from_version, report.to_version), (10, 11));
    assert!(report.backup.is_some());
    assert_eq!(store_log(&store), log, "the migration changed the log");
    expected.check(&store, "schema migration");

    let summary = json!({
        "open_old_project_s": open_s,
        "decisions": decisions,
        "effects_checked": expected.total(),
        "status": by_status,
        "proposals": {
            "entities": entities,
            "stored": stored,
            "run_s": proposal_s,
            "peak_rss_mib_before": rss_before,
            "peak_rss_mib_after": rss_after,
            "target_s": 60,
        },
        "decision_times": decision_times,
        "rebuild_all_s": rebuild_s,
    });
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
