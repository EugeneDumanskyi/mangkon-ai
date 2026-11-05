//! ADR-0005, M6a: a re-run never mixes two models. Random sequences of commit, late reply, re-run,
//! cancel and changed file, checked after every step against a full rebuild and the reference model.

use mk_ai::{CheckedMention, RunIdentity};
use mk_corpus::{FileMeta, PathChange};
use mk_graph::model::reference;
use mk_jobs::Trigger;
use mk_kernel::{ContentHash, EntityType, LineNumber, Locator, NameKey, Span};
use mk_store::{
    AiCancelled, AiItemResult, DocumentOut, RescanChange, RunBinding, SegmentOut, StageAOutput,
    StartedAiItem, Store, UnitScope,
};
use proptest::prelude::*;

/// Each model finds its own name in every unit.
const NAMES: [&str; 2] = ["Alice Ash", "Bram Birch"];
const FILES: u8 = 3;

fn binding(model: usize) -> RunBinding {
    RunBinding {
        identity: RunIdentity::local(&format!("m{model}")),
        model_name: format!("Model {model}"),
        destination_host: "127.0.0.1".into(),
        destination_class: "local".into(),
        prompt_version: 1,
    }
}

fn hash(n: u8) -> ContentHash {
    ContentHash::from_bytes([n; 32])
}

fn output(version: u8) -> StageAOutput {
    let text = format!("Version {version}: {} met {}.", NAMES[0], NAMES[1]);
    let len = text.chars().count();
    StageAOutput {
        documents: vec![DocumentOut {
            kind: "txt".into(),
            position: "whole".into(),
            parent: None,
            segments: vec![SegmentOut {
                locator: Locator::TextLine {
                    line: LineNumber::new(1).unwrap(),
                    span: Span::new(0, len, len).unwrap(),
                },
                text,
                malformed: false,
                cell_starts: Vec::new(),
                ocr: None,
            }],
        }],
        mentions: Vec::new(),
        card_flags: Vec::new(),
        warnings: Vec::new(),
    }
}

fn run_stage_a(store: &Store, version: u8) {
    for w in store.pending_items().unwrap() {
        store.start_item(w.item.id).unwrap().unwrap();
        store.commit_stage_a(w.item.id, output(version)).unwrap();
    }
}

struct World {
    _dir: tempfile::TempDir,
    store: Store,
    root: i64,
    /// Each file's content now.
    contents: Vec<u8>,
    next_content: u8,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let (store, _) = Store::open(&dir.path().join("p.db"), &dir.path().join("b")).unwrap();
    let root = store.add_source_root("/src", "folder").unwrap();
    let changes = (0..FILES)
        .map(|n| RescanChange {
            change: PathChange::New {
                root,
                rel_path: format!("f{n}.txt"),
                meta: FileMeta {
                    size: 10,
                    mtime_ns: 1,
                },
                hash: hash(n + 1),
            },
            file_type: "txt".into(),
        })
        .collect();
    store.apply_rescan(changes, Trigger::Add).unwrap();
    run_stage_a(&store, 0);
    World {
        _dir: dir,
        store,
        root,
        contents: (1..=FILES).collect(),
        next_content: FILES + 1,
    }
}

/// The checks' result for `item`, finding `model`'s name.
fn reply(item: &StartedAiItem, model: usize) -> AiItemResult {
    let name = NAMES[model];
    let mut mentions = Vec::new();
    for p in &item.pieces {
        let Some(at) = p.text.find(name) else {
            continue;
        };
        let start = p.text[..at].chars().count() + p.start;
        mentions.push(CheckedMention {
            segment: p.segment,
            key: NameKey::new(EntityType::Person, name).unwrap(),
            entity_type: EntityType::Person,
            raw: name.to_string(),
            span: (start, start + name.chars().count()),
            confidence: 0.9,
        });
    }
    AiItemResult {
        mentions,
        ..AiItemResult::default()
    }
}

fn model_of(item: &StartedAiItem) -> usize {
    usize::from(item.identity != RunIdentity::local("m0"))
}

/// The names visible now.
fn visible(store: &Store) -> Vec<usize> {
    let raw: Vec<String> = store
        .read_raw(|c| {
            let mut stmt = c.prepare(
                "SELECT DISTINCT raw FROM entity_mention WHERE producer = 'model' ORDER BY raw",
            )?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap();
    raw.iter()
        .map(|r| NAMES.iter().position(|n| n == r).unwrap())
        .collect()
}

/// After every step: the stored rows equal the reference model and a rebuild, nothing is outside
/// its run, and at most one model's names show (ADR-0005, INV-AI-01).
fn check(store: &Store) {
    let state = store.derived_state().unwrap();
    assert_eq!(state, reference(&store.facts().unwrap()), "reference");
    store.rebuild_all().unwrap();
    assert_eq!(state, store.derived_state().unwrap(), "rebuild");
    let health = store.ai_health().unwrap();
    assert_eq!(health.invented_evidence, 0);
    assert_eq!(health.mixed_runs, 0);
    let names = visible(store);
    assert!(names.len() <= 1, "two models shown: {names:?}");
    if let Some(&shown) = names.first() {
        let active = store.active_ai_run().unwrap().expect("a run behind them");
        assert_eq!(active.identity, binding(shown).identity);
    }
}

#[derive(Clone, Debug)]
enum Op {
    /// Starts the next item and commits its reply.
    Commit,
    /// Starts the next item and releases it, as a pause does; the reply comes after, and is
    /// dropped by its token.
    LateReply,
    Rerun(usize),
    Cancel,
    /// A file changed: its content's items go, new ones come by the next schedule.
    Change(u8),
    Schedule,
}

fn ops() -> impl Strategy<Value = Vec<Op>> {
    prop::collection::vec(
        prop_oneof![
            8 => Just(Op::Commit),
            2 => Just(Op::LateReply),
            2 => (0usize..2).prop_map(Op::Rerun),
            1 => Just(Op::Cancel),
            1 => (0..FILES).prop_map(Op::Change),
            2 => Just(Op::Schedule),
        ],
        1..40,
    )
}

impl World {
    fn apply(&mut self, op: &Op) {
        let store = &self.store;
        match op {
            Op::Commit => {
                if let Some(item) = store.start_next_ai_item().unwrap() {
                    let before = store.derived_state().unwrap();
                    let pending = store.pending_ai_run().unwrap();
                    let out = store
                        .commit_ai_item(item.item, item.attempt, reply(&item, model_of(&item)))
                        .unwrap();
                    assert!(out.committed);
                    // Every commit of a re-run but its last shows nothing new.
                    if pending.is_some() && store.pending_ai_run().unwrap().is_some() {
                        assert_eq!(before, store.derived_state().unwrap(), "pending commit");
                    }
                }
            }
            Op::LateReply => {
                if let Some(item) = store.start_next_ai_item().unwrap() {
                    let before = store.derived_state().unwrap();
                    assert!(store.release_ai_item(item.item, item.attempt).unwrap());
                    let out = store
                        .commit_ai_item(item.item, item.attempt, reply(&item, model_of(&item)))
                        .unwrap();
                    assert!(!out.committed);
                    assert_eq!(before, store.derived_state().unwrap(), "late reply");
                }
            }
            Op::Rerun(model) => {
                let target = binding(*model);
                // Refused while one is under way, or with the model of the results.
                let _ = store.start_ai_rerun(target);
            }
            Op::Cancel => {
                let before = store.derived_state().unwrap();
                if store.cancel_ai_run().unwrap() == AiCancelled::Rerun {
                    assert_eq!(before, store.derived_state().unwrap(), "cancel");
                    assert!(store.pending_ai_run().unwrap().is_none());
                }
            }
            Op::Change(file) => {
                let f = usize::from(*file);
                let new = self.next_content;
                self.next_content += 1;
                store
                    .apply_rescan(
                        vec![RescanChange {
                            change: PathChange::Changed {
                                root: self.root,
                                rel_path: format!("f{file}.txt"),
                                meta: FileMeta {
                                    size: 11,
                                    mtime_ns: i64::from(new),
                                },
                                old: hash(self.contents[f]),
                                new: hash(new),
                            },
                            file_type: "txt".into(),
                        }],
                        Trigger::Rescan,
                    )
                    .unwrap();
                self.contents[f] = new;
                run_stage_a(store, new);
            }
            Op::Schedule => {
                store
                    .schedule_ai_units(UnitScope::Auto, Trigger::Add)
                    .unwrap();
                store.bind_ai_run(binding(0)).ok();
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn adr_0005_never_mixed_at_any_step(ops in ops()) {
        let mut w = world();
        w.store.schedule_ai_units(UnitScope::Auto, Trigger::Add).unwrap();
        w.store.bind_ai_run(binding(0)).unwrap();
        for op in &ops {
            w.apply(op);
            check(&w.store);
        }
        // Whatever happened, finishing the work leaves one whole model.
        w.store.schedule_ai_units(UnitScope::Auto, Trigger::Add).unwrap();
        if w.store.ai_progress().unwrap().open > 0 && w.store.active_ai_run().unwrap().is_none()
            && w.store.pending_ai_run().unwrap().is_none()
        {
            w.store.bind_ai_run(binding(0)).unwrap();
        }
        while let Some(item) = w.store.start_next_ai_item().unwrap() {
            let model = model_of(&item);
            w.store.commit_ai_item(item.item, item.attempt, reply(&item, model)).unwrap();
            check(&w.store);
        }
        prop_assert!(w.store.pending_ai_run().unwrap().is_none());
        prop_assert!(visible(&w.store).len() <= 1);
    }
}
