//! M2 acceptance criterion 4, ADR-0005 follow-up: for random sequences of file changes,
//! re-scans, Stage A runs and decisions, the derived rows kept up to date by incremental
//! commits equal (a) the `mk-graph` reference computed from scratch after every step and
//! (b) a full `rebuild_all` at the end.
//!
//! The sequences go through the real pipeline (files on disk, re-scan, Stage A, one commit per
//! item), so they cover new, changed (replaced), missing, found-again and duplicate content,
//! ignore / un-ignore, superseded and orphaned decisions. M4c adds custom patterns and their
//! re-run, mapping confirms of the CSV files ("Name (from table)" entities, record links) and
//! ignores on entities. M7a adds every decision on entities (merge, reject, undo, split, undo-split,
//! rename, type, note, alias) in a second property, among the same file changes (M7a criterion 5).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mk_core::{Assignments, ColumnType, DecisionKind, DocumentFilter, MergeEdit, Project};
use mk_kernel::{EntityType, NameKey};
use proptest::prelude::*;

/// A small key alphabet so keys collide often. Keys 5 and 6 are never written: decisions on
/// them stay orphaned.
const KEYS: usize = 7;
const WRITTEN_KEYS: usize = 5;
const FILES: u8 = 4;

fn email(k: usize) -> String {
    format!("k{k}@x.example")
}

#[derive(Clone, Debug)]
enum Op {
    /// Writes `files/f<n>.txt` (or `.csv` when `csv`) with these keys. `salt` varies the bytes
    /// a little, so equal bytes (one content at two paths) happen too.
    Write {
        file: u8,
        keys: Vec<usize>,
        salt: u8,
        csv: bool,
    },
    /// Moves the file into a hidden folder (it goes Missing).
    Delete(u8),
    /// Moves it back with the same bytes and time (found again).
    Restore(u8),
    /// Copies one file to another path.
    Copy(u8, u8),
    Rescan,
    StageA,
    Ignore(usize),
    Unignore(usize),
    /// Adds the custom pattern `kref` (`k<n>@`) if missing, else removes it (US-04.7).
    TogglePattern,
    /// The pending identifier re-runs (built-in and custom-pattern steps).
    Rerun,
    /// Confirms the `name` column of `f<n>.csv` as this type (`None`: Ignore), US-05.2.
    Confirm(u8, Option<EntityType>),
    /// Ignores or un-ignores the entity of name `n` (US-07.4 on entities).
    IgnoreEntity(usize, EntityType, bool),
    /// Accepts the pending proposal between two names, if there is one (US-11.2 AC2), maybe with a
    /// name and a type.
    Accept(usize, usize, EntityType, Option<EntityType>, bool),
    /// Rejects the pair, whether or not it is proposed (US-11.3 AC3).
    Reject(usize, usize, EntityType),
    /// Undoes the pair, if merged (US-11.4 AC1).
    Undo(usize, usize, EntityType),
    /// Moves the mentions a mask picks out of the `n`-th entity (US-11.4 AC2).
    Split(usize, u8),
    /// Undoes the `n`-th split in the log.
    UndoSplit(usize),
    /// Renames, types or notes the `n`-th entity (US-12.2): `salt` varies the value.
    Edit(usize, EditKind, u8),
    /// Removes or restores the `a`-th alias of the `n`-th entity (US-12.2 AC1).
    Alias(usize, usize, bool),
}

#[derive(Clone, Copy, Debug)]
enum EditKind {
    Rename,
    Type,
    Note,
}

/// The name in a CSV row of key `k`: names collide across keys and files, one name key is written
/// two ways, and the three name keys look alike, so they are proposed.
const NAMES: [&str; WRITTEN_KEYS] = [
    "Acme Harbour Ltd",
    "ACME HARBOUR",
    "Acme Harbour Group",
    "Harbour Acme",
    "Acme Harbour Group Ltd",
];
/// Index 3 is never written: decisions on it stay orphaned.
const ENTITIES: [&str; 4] = [
    "Acme Harbour",
    "Acme Harbour Group",
    "Harbour Acme",
    "Never Written",
];

fn name(k: usize) -> &'static str {
    NAMES[k]
}

fn entity_key(n: usize, t: EntityType) -> String {
    NameKey::new(t, ENTITIES[n]).unwrap().to_string()
}

fn entity_type() -> impl Strategy<Value = EntityType> {
    prop_oneof![Just(EntityType::Person), Just(EntityType::Org)]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..FILES, proptest::collection::vec(0..WRITTEN_KEYS, 0..4), 0u8..3, any::<bool>())
            .prop_map(|(file, keys, salt, csv)| Op::Write { file, keys, salt, csv }),
        1 => (0..FILES).prop_map(Op::Delete),
        1 => (0..FILES).prop_map(Op::Restore),
        1 => (0..FILES, 0..FILES).prop_map(|(a, b)| Op::Copy(a, b)),
        3 => Just(Op::Rescan),
        3 => Just(Op::StageA),
        1 => (0..KEYS).prop_map(Op::Ignore),
        1 => (0..KEYS).prop_map(Op::Unignore),
        1 => Just(Op::TogglePattern),
        1 => Just(Op::Rerun),
        2 => (0..FILES, proptest::option::of(entity_type())).prop_map(|(f, t)| Op::Confirm(f, t)),
        1 => (0..4usize, entity_type(), any::<bool>())
            .prop_map(|(n, t, on)| Op::IgnoreEntity(n, t, on)),
    ]
}

/// The file changes of [`op`], with more re-scans and every decision on entities.
fn decision_op() -> impl Strategy<Value = Op> {
    let name = 0..4usize;
    prop_oneof![
        2 => (0..FILES, proptest::collection::vec(0..WRITTEN_KEYS, 1..4), 0u8..3)
            .prop_map(|(file, keys, salt)| Op::Write { file, keys, salt, csv: true }),
        1 => (0..FILES).prop_map(Op::Delete),
        1 => (0..FILES).prop_map(Op::Restore),
        1 => (0..FILES, 0..FILES).prop_map(|(a, b)| Op::Copy(a, b)),
        2 => Just(Op::Rescan),
        2 => Just(Op::StageA),
        2 => (0..FILES, proptest::option::of(entity_type())).prop_map(|(f, t)| Op::Confirm(f, t)),
        1 => (name.clone(), entity_type(), any::<bool>())
            .prop_map(|(n, t, on)| Op::IgnoreEntity(n, t, on)),
        5 => (name.clone(), name.clone(), entity_type(), proptest::option::of(entity_type()), any::<bool>())
            .prop_map(|(a, b, t, to, named)| Op::Accept(a, b, t, to, named)),
        2 => (name.clone(), name.clone(), entity_type()).prop_map(|(a, b, t)| Op::Reject(a, b, t)),
        2 => (name.clone(), name.clone(), entity_type()).prop_map(|(a, b, t)| Op::Undo(a, b, t)),
        4 => (0..8usize, any::<u8>()).prop_map(|(n, mask)| Op::Split(n, mask)),
        2 => (0..4usize).prop_map(Op::UndoSplit),
        3 => (
            0..8usize,
            prop_oneof![Just(EditKind::Rename), Just(EditKind::Type), Just(EditKind::Note)],
            0u8..3,
        )
            .prop_map(|(n, k, salt)| Op::Edit(n, k, salt)),
        2 => (0..8usize, 0..3usize, any::<bool>()).prop_map(|(n, a, on)| Op::Alias(n, a, on)),
    ]
}

/// The `n`-th entity of the project, modulo how many there are.
fn nth_entity(project: &Project, n: usize) -> Option<String> {
    let entities = project.store().derived_state().unwrap().entities;
    let i = n.checked_rem(entities.len())?;
    entities.into_keys().nth(i)
}

struct World {
    src: PathBuf,
    trash: PathBuf,
    clock: u64,
}

impl World {
    fn tick(&mut self, path: &Path) {
        self.clock += 1;
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + self.clock))
            .unwrap();
    }

    /// The file's current path, whichever extension it has.
    fn find(&self, dir: &Path, file: u8) -> Option<PathBuf> {
        ["txt", "csv"]
            .iter()
            .map(|e| dir.join(format!("f{file}.{e}")))
            .find(|p| p.exists())
    }

    fn apply(&mut self, project: &Project, op: &Op) {
        match op {
            Op::Write {
                file,
                keys,
                salt,
                csv,
            } => {
                if let Some(old) = self.find(&self.src, *file) {
                    std::fs::remove_file(old).unwrap();
                }
                let (ext, body) = if *csv {
                    let rows: String = keys
                        .iter()
                        .map(|k| format!("{},{}\n", name(*k), email(*k)))
                        .collect();
                    ("csv", format!("name,email\nsalt,{salt}\n{rows}"))
                } else {
                    let lines: String = keys
                        .iter()
                        .map(|k| format!("see {}\n", email(*k)))
                        .collect();
                    ("txt", format!("salt {salt}\n{lines}"))
                };
                let path = self.src.join(format!("f{file}.{ext}"));
                std::fs::write(&path, body).unwrap();
                self.tick(&path);
            }
            Op::Delete(file) => {
                if let Some(p) = self.find(&self.src, *file) {
                    if let Some(old) = self.find(&self.trash, *file) {
                        std::fs::remove_file(old).unwrap();
                    }
                    std::fs::rename(&p, self.trash.join(p.file_name().unwrap())).unwrap();
                }
            }
            Op::Restore(file) => {
                if self.find(&self.src, *file).is_none()
                    && let Some(p) = self.find(&self.trash, *file)
                {
                    std::fs::rename(&p, self.src.join(p.file_name().unwrap())).unwrap();
                }
            }
            Op::Copy(from, to) => {
                if from != to
                    && let Some(p) = self.find(&self.src, *from)
                {
                    if let Some(old) = self.find(&self.src, *to) {
                        std::fs::remove_file(old).unwrap();
                    }
                    let ext = p.extension().unwrap().to_str().unwrap().to_string();
                    let dst = self.src.join(format!("f{to}.{ext}"));
                    std::fs::copy(&p, &dst).unwrap();
                    self.tick(&dst);
                }
            }
            Op::Rescan => {
                project.rescan(false).unwrap();
            }
            Op::StageA => {
                project.run_stage_a().unwrap();
            }
            Op::Ignore(k) => {
                project
                    .set_ignored(&format!("email:{}", email(*k)), true)
                    .unwrap();
            }
            Op::Unignore(k) => {
                project
                    .set_ignored(&format!("email:{}", email(*k)), false)
                    .unwrap();
            }
            Op::TogglePattern => {
                if project.custom_patterns().unwrap().is_empty() {
                    project.add_pattern("kref", r"k\d@").unwrap();
                } else {
                    project.remove_pattern("kref").unwrap();
                }
            }
            Op::Rerun => {
                project.rerun_identifiers().unwrap();
            }
            Op::Confirm(file, t) => {
                let rows = project
                    .documents_page(&DocumentFilter::default(), None, 100)
                    .unwrap()
                    .rows;
                let path = format!("f{file}.csv");
                if let Some(row) = rows.iter().find(|r| r.rel_path == path)
                    && project.mapping_view(row.ref_id).unwrap().is_some()
                {
                    let ty = match t {
                        Some(EntityType::Person) => ColumnType::PersonName,
                        Some(EntityType::Org) => ColumnType::OrgName,
                        None => ColumnType::Ignore,
                    };
                    let assignments = Assignments(
                        [
                            ("name".to_string(), ty),
                            ("email".to_string(), ColumnType::Email),
                        ]
                        .into(),
                    );
                    project.confirm_mapping(row.ref_id, assignments).unwrap();
                }
            }
            Op::IgnoreEntity(n, t, on) => {
                project.set_ignored(&entity_key(*n, *t), *on).unwrap();
            }
            // A decision refused (no such proposal, no such entity) changes nothing: the comparison
            // after the step still has to hold, so these only ignore the refusal.
            Op::Accept(a, b, t, to, named) => {
                project.refresh_proposals().unwrap();
                let edit = MergeEdit {
                    name: named.then(|| "Chosen Name".to_string()),
                    entity_type: *to,
                };
                let _ = project.accept_proposal(&entity_key(*a, *t), &entity_key(*b, *t), edit);
            }
            Op::Reject(a, b, t) => {
                let _ = project.reject_proposal(&entity_key(*a, *t), &entity_key(*b, *t));
            }
            Op::Undo(a, b, t) => {
                let _ = project.undo_merge(&entity_key(*a, *t), &entity_key(*b, *t));
            }
            Op::Split(n, mask) => {
                if let Some(key) = nth_entity(project, *n) {
                    let picked: Vec<i64> = project
                        .occurrences(&key)
                        .unwrap()
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| mask >> (i % 8) & 1 == 1)
                        .map(|(_, o)| o.mention_id)
                        .collect();
                    let _ = project.split_mentions(&key, &picked);
                }
            }
            Op::UndoSplit(n) => {
                let splits: Vec<String> = project
                    .decisions(None, 1_000)
                    .unwrap()
                    .into_iter()
                    .filter(|d| d.kind == DecisionKind::Split)
                    .map(|d| d.anchor)
                    .collect();
                if let Some(split) = n.checked_rem(splits.len()).map(|i| &splits[i]) {
                    let _ = project.undo_split(split);
                }
            }
            Op::Edit(n, kind, salt) => {
                if let Some(key) = nth_entity(project, *n) {
                    match kind {
                        EditKind::Rename => {
                            project.rename_entity(&key, &format!("New Name {salt}"))
                        }
                        EditKind::Type if salt % 2 == 0 => {
                            project.change_entity_type(&key, EntityType::Org)
                        }
                        EditKind::Type => project.change_entity_type(&key, EntityType::Person),
                        EditKind::Note => {
                            project.set_entity_note(&key, &"n".repeat(*salt as usize))
                        }
                    }
                    .unwrap();
                }
            }
            Op::Alias(n, a, true) => {
                if let Some(key) = nth_entity(project, *n) {
                    let names = project.entity_names(&key).unwrap().unwrap();
                    if let Some(i) = a.checked_rem(names.aliases.len()) {
                        project.remove_alias(&key, &names.aliases[i].0).unwrap();
                    }
                }
            }
            // Any removed alias, or there would rarely be one on the entity picked.
            Op::Alias(_, _, false) => {
                for key in project
                    .store()
                    .derived_state()
                    .unwrap()
                    .entities
                    .into_keys()
                {
                    let names = project.entity_names(&key).unwrap().unwrap();
                    if let Some((alias, _)) = names.removed.first() {
                        project.restore_alias(&key, alias).unwrap();
                        break;
                    }
                }
            }
        }
    }
}

/// Runs `ops`, comparing with the reference after each and with a full rebuild at the end.
fn check_ops(prefix: &[Op], ops: &[Op]) -> Result<(), TestCaseError> {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    let trash = src.join(".trash");
    std::fs::create_dir_all(&trash).unwrap();
    let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
    *project.worker_config_mut() =
        mk_core::WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.add_source_root(&src).unwrap();
    let mut world = World {
        src,
        trash,
        clock: 0,
    };
    for (i, op) in prefix.iter().chain(ops).enumerate() {
        world.apply(&project, op);
        let incremental = project.store().derived_state().unwrap();
        let reference = mk_graph::model::reference(&project.store().facts().unwrap());
        prop_assert_eq!(&incremental, &reference, "after step {} ({:?})", i, op);
    }
    // Finish the work, then compare with a rebuild from scratch.
    project.rescan(false).unwrap();
    project.run_stage_a().unwrap();
    project.rerun_identifiers().unwrap();
    let incremental = project.store().derived_state().unwrap();
    project.store().rebuild_all().unwrap();
    let rebuilt = project.store().derived_state().unwrap();
    prop_assert_eq!(&incremental, &rebuilt);
    prop_assert_eq!(
        &rebuilt,
        &mk_graph::model::reference(&project.store().facts().unwrap())
    );
    Ok(())
}

/// Four confirmed tables holding every name, so proposals and decisions have something to act on.
fn four_tables(t: EntityType) -> Vec<Op> {
    let mut out = Vec::new();
    for file in 0..FILES {
        out.push(Op::Write {
            file,
            keys: (0..WRITTEN_KEYS).collect(),
            salt: file,
            csv: true,
        });
    }
    out.push(Op::Rescan);
    out.push(Op::StageA);
    out.extend((0..FILES).map(|f| Op::Confirm(f, Some(t))));
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// M2 criterion 4: incremental == reference after every step, and == full rebuild at the
    /// end (ADR-0005, INV-CU-03/04/05, INV-LK-01, INV-CO-06/07).
    #[test]
    fn m2_ac4_incremental_equals_rebuild(ops in proptest::collection::vec(op(), 1..25)) {
        check_ops(&[], &ops)?;
    }

    /// M7a criterion 5: the same, with every decision on entities among the file changes (ADR-0005,
    /// INV-ER-01..08, INV-CU-03/04/05).
    #[test]
    fn m7a_ac5_decisions_incremental_equals_rebuild(
        t in entity_type(),
        ops in proptest::collection::vec(decision_op(), 1..40),
    ) {
        check_ops(&four_tables(t), &ops)?;
    }
}
