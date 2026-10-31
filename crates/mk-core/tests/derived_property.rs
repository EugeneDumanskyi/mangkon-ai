//! M2 acceptance criterion 4, ADR-0005 follow-up: for random sequences of file changes,
//! re-scans, Stage A runs and decisions, the derived rows kept up to date by incremental
//! commits equal (a) the `mk-graph` reference computed from scratch after every step and
//! (b) a full `rebuild_all` at the end.
//!
//! The sequences go through the real pipeline (files on disk, re-scan, Stage A, one commit per
//! item), so they cover new, changed (replaced), missing, found-again and duplicate content,
//! ignore / un-ignore, superseded and orphaned decisions. M4c adds custom patterns and their
//! re-run, mapping confirms of the CSV files ("Name (from table)" entities, record links) and
//! ignores on entities.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use mk_core::{Assignments, ColumnType, DocumentFilter, Project};
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
}

/// The name in a CSV row of key `k`: names collide across keys and files.
fn name(k: usize) -> String {
    format!("Name{} Ltd", k % 3)
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
                let key = NameKey::new(*t, &format!("Name{n} Ltd")).unwrap();
                project.set_ignored(&key.to_string(), *on).unwrap();
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// M2 criterion 4: incremental == reference after every step, and == full rebuild at the
    /// end (ADR-0005, INV-CU-03/04/05, INV-LK-01, INV-CO-06/07).
    #[test]
    fn m2_ac4_incremental_equals_rebuild(ops in proptest::collection::vec(op(), 1..25)) {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let trash = src.join(".trash");
        std::fs::create_dir_all(&trash).unwrap();
        let (mut project, _) = Project::open(&tmp.path().join("project")).unwrap();
        *project.worker_config_mut() = mk_core::WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
        project.add_source_root(&src).unwrap();
        let mut world = World { src, trash, clock: 0 };
        for (i, op) in ops.iter().enumerate() {
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
        prop_assert_eq!(&rebuilt, &mk_graph::model::reference(&project.store().facts().unwrap()));
    }
}
