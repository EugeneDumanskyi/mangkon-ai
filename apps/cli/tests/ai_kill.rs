//! M6a, NFR-REL-02/03/04, ADR-0005: `SIGKILL` of `mangkon-cli ai run` and `ai rerun`, 10 each,
//! against the stand-in `llama-server`, half at seeded moments and half among the units. After every
//! kill the project is healthy, lost work is at most what was in progress, a re-run shows one model's
//! results and never a mix, and going on ends equal to a run without a kill. No stand-in stays.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mk_core::{Catalog, EgressConfig, ItemState, ModelsConfig, Project, Workspace};
use mk_models::testkit::llama::{CONTROL_ENV, Control, FakeExtraction, FakeName};
use mk_store::AppStore;

const CLI: &str = env!("CARGO_BIN_EXE_mangkon-cli");
const FAKE: &str = env!("CARGO_BIN_EXE_mangkon-cli-fake-llama-server");
const SLOTS: usize = 4;
const KILLS: usize = 10;

const X: &str = "qwen3.5-4b-q4_k_m";
const Y: &str = "qwen3.6-35b-a3b-q8_0";

struct World {
    tmp: tempfile::TempDir,
    source: PathBuf,
    control: PathBuf,
    starts: PathBuf,
    requests: PathBuf,
}

fn name(n: &str, kind: &str) -> FakeName {
    FakeName {
        name: n.into(),
        kind: kind.into(),
        ..FakeName::default()
    }
}

/// Stands a shipped model in with an empty file of the size the catalog says, so the CLI, which
/// knows only the shipped catalog, finds it installed.
fn install(store: &AppStore, dir: &Path, id: &str) {
    let catalog = Catalog::shipped();
    let m = catalog.model(id).unwrap();
    let mut total = 0;
    for f in &m.files {
        let path = mk_models::download::file_path(dir, m, f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::File::create(&path)
            .unwrap()
            .set_len(f.bytes)
            .unwrap();
        store.model_file_verified(id, &f.name, f.bytes).unwrap();
        total += f.bytes;
    }
    store
        .set_model_download(id, "installed", total, total, None)
        .unwrap();
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    for f in 0..40 {
        std::fs::write(
            source.join(format!("note-{f:02}.txt")),
            format!(
                "Note {f}. Corvin Hale paid Velmora Port Authority.\nMarta Quill checked it for \
                 Brightwater Freight. Lines {f}.\n"
            ),
        )
        .unwrap();
    }
    let rules = |names: Vec<FakeName>| FakeExtraction {
        names,
        delay_ms: 20,
        ..FakeExtraction::default()
    };
    let catalog = Catalog::shipped();
    let file_of = |id: &str| {
        let m = catalog.model(id).unwrap();
        let name = &m.files[0].name;
        name.rsplit('/').next().unwrap().to_string()
    };
    let starts = tmp.path().join("starts.log");
    let requests = tmp.path().join("requests.log");
    let control = tmp.path().join("control.json");
    Control {
        starts_log: Some(starts.clone()),
        requests_log: Some(requests.clone()),
        extraction: HashMap::from([
            (
                file_of(X),
                rules(vec![
                    name("Corvin Hale", "person"),
                    name("Velmora Port Authority", "org"),
                ]),
            ),
            (
                file_of(Y),
                rules(vec![
                    name("Marta Quill", "person"),
                    name("Brightwater Freight", "org"),
                ]),
            ),
        ]),
        ..Control::default()
    }
    .write(&control);
    let w = World {
        source,
        control,
        starts,
        requests,
        tmp,
    };
    let store = AppStore::open(&w.app()).unwrap();
    install(&store, &w.models(), X);
    install(&store, &w.models(), Y);
    drop(store);
    // The test machine may be below the local minimum; that check is not what runs here.
    let ws = Workspace::open_with_models(
        &w.app(),
        EgressConfig::default(),
        ModelsConfig {
            models_dir: Some(w.models()),
            ..ModelsConfig::default()
        },
    )
    .unwrap();
    ws.set_local_override(true).unwrap();
    ws.shut_down();
    w
}

impl World {
    fn app(&self) -> PathBuf {
        self.tmp.path().join("app")
    }

    fn models(&self) -> PathBuf {
        self.tmp.path().join("models")
    }

    fn project(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    fn ai(&self, project: &Path, words: &[&str]) -> Command {
        let mut c = Command::new(CLI);
        c.arg("ai")
            .args(words)
            .arg("--project")
            .arg(project)
            .arg("--app-data")
            .arg(self.app())
            .arg("--dir")
            .arg(self.models())
            .arg("--llama-server")
            .arg(FAKE)
            .arg("--json")
            .env(CONTROL_ENV, &self.control)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }

    fn ingest(&self, project: &Path) {
        let out = Command::new(CLI)
            .args(["ingest", "--json", "--project"])
            .arg(project)
            .arg(&self.source)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    }

    fn finish(&self, project: &Path, words: &[&str]) {
        let out = self.ai(project, words).output().unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Pids of the stand-ins started so far.
    fn started(&self) -> Vec<u32> {
        std::fs::read_to_string(&self.starts)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_whitespace().next()?.parse().ok())
            .collect()
    }
}

fn alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| {
        !s.rsplit(')')
            .next()
            .unwrap_or("")
            .trim_start()
            .starts_with('Z')
    })
}

/// Connections and Done/Failed counts, for comparison.
fn fingerprint(project: &Project) -> (String, u64, u64) {
    let p = project.store().progress().unwrap();
    (
        format!("{:?}", project.store().derived_state().unwrap().connections),
        p.items_in(ItemState::Done),
        p.items_in(ItemState::Failed),
    )
}

fn connections(project: &Project) -> String {
    format!("{:?}", project.store().derived_state().unwrap().connections)
}

/// The kill fell among the units: some are finished and some are not.
fn landed_mid_work(project: &Project) -> bool {
    let p = project.store().ai_progress().unwrap();
    p.done > 0 && p.open > 0
}

/// A seeded moment in `[0, max)` (splitmix64).
fn moment(state: &mut u64, max: Duration) -> Duration {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    max.mul_f64(((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64)
}

/// When a kill falls.
#[derive(Clone, Copy, Debug)]
enum When {
    /// Anywhere in a run, the model's start and the shutdown included.
    At(Duration),
    /// Among the units, whatever the machine's speed: this many requests reached the model. A slot
    /// asks again only after committing its last unit, so 5 mean at least one is done, and the 40
    /// units leave plenty open at 30.
    AfterRequests(usize),
}

/// Half of the kills are seeded moments, half fall among the units.
fn when(run: usize, seed: &mut u64, window: Duration) -> When {
    if run.is_multiple_of(2) {
        When::At(moment(seed, window))
    } else {
        When::AfterRequests(5 + moment(seed, Duration::from_secs(26)).as_secs() as usize)
    }
}

/// Spawns `words`, kills it at `when`; true if it was still running then.
fn kill_at(w: &World, project: &Path, words: &[&str], when: When) -> bool {
    std::fs::write(&w.requests, "").unwrap();
    let started = Instant::now();
    let mut child = w
        .ai(project, words)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let reached = || match when {
        When::At(at) => started.elapsed() >= at,
        When::AfterRequests(n) => {
            let log = std::fs::read_to_string(&w.requests).unwrap_or_default();
            log.lines().count() >= n
        }
    };
    while !reached() {
        if child.try_wait().unwrap().is_some() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let running = child.try_wait().unwrap().is_none();
    let _ = child.kill();
    child.wait().unwrap();
    running
}

/// The stand-ins the killed process left are gone within a moment.
fn no_orphans(w: &World, before: &[u32]) -> Vec<u32> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left: Vec<u32> = w
            .started()
            .into_iter()
            .filter(|p| !before.contains(p) && alive(*p))
            .collect();
        if left.is_empty() || Instant::now() >= deadline {
            return left;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// NFR-REL-02/03/04: 10 kills of `ai run`.
#[test]
fn nfr_rel_02_kill_during_ai_run() {
    let w = world();
    let reference_dir = w.project("reference");
    w.ingest(&reference_dir);
    let t = Instant::now();
    w.finish(&reference_dir, &["run", "--model", X]);
    let window = t.elapsed();
    let (reference, _) = Project::open(&reference_dir).unwrap();
    let want = fingerprint(&reference);
    assert!(want.0.contains("corvin hale"), "{}", want.0);
    drop(reference);

    let mut seed = 11;
    let mut killed_running = 0;
    let mut mid_work = 0;
    for run in 0..KILLS {
        let dir = w.project(&format!("run-{run}"));
        w.ingest(&dir);
        let before = w.started();
        let at = when(run, &mut seed, window);
        killed_running += usize::from(kill_at(&w, &dir, &["run", "--model", X], at));
        let orphans = no_orphans(&w, &before);
        assert!(orphans.is_empty(), "run {run}: {orphans:?} left running");

        let (project, opened) = Project::open(&dir).unwrap();
        let health = project.check_health().unwrap();
        assert!(health.is_healthy(), "run {run}: {health:?}");
        assert_eq!(health.progress.items_in(ItemState::InProgress), 0);
        assert!(opened.recovered.items <= SLOTS, "run {run}: {opened:?}");
        mid_work += usize::from(landed_mid_work(&project));
        drop(project);

        w.finish(&dir, &["run", "--model", X]);
        let (project, _) = Project::open(&dir).unwrap();
        assert!(project.check_health().unwrap().is_healthy(), "run {run}");
        assert_eq!(fingerprint(&project), want, "run {run}");
    }
    assert!(killed_running >= KILLS / 2, "only {killed_running} hit");
    assert!(
        mid_work >= 2,
        "only {mid_work} of {KILLS} fell among the units"
    );
    // A stand-in that logged its start after its run's check still counts.
    assert_eq!(no_orphans(&w, &[]), Vec::<u32>::new(), "left running");
}

/// NFR-REL-02/03/04 and ADR-0005: 10 kills of `ai rerun`. Whenever it dies, the connections are
/// the old model's or the new one's, whole.
#[test]
fn adr_0005_kill_during_ai_rerun_never_mixes() {
    let w = world();
    let reference_dir = w.project("reference");
    w.ingest(&reference_dir);
    w.finish(&reference_dir, &["run", "--model", X]);
    let old = connections(&Project::open(&reference_dir).unwrap().0);
    let t = Instant::now();
    w.finish(&reference_dir, &["rerun", "--model", Y]);
    let window = t.elapsed();
    let (reference, _) = Project::open(&reference_dir).unwrap();
    let want = fingerprint(&reference);
    assert!(want.0.contains("marta quill"), "{}", want.0);
    assert!(!want.0.contains("corvin hale"), "{}", want.0);
    drop(reference);

    let mut seed = 23;
    let mut killed_running = 0;
    let mut mid_work = 0;
    for run in 0..KILLS {
        let dir = w.project(&format!("run-{run}"));
        w.ingest(&dir);
        w.finish(&dir, &["run", "--model", X]);
        let before = w.started();
        let at = when(run, &mut seed, window);
        killed_running += usize::from(kill_at(&w, &dir, &["rerun", "--model", Y], at));
        let orphans = no_orphans(&w, &before);
        assert!(orphans.is_empty(), "run {run}: {orphans:?} left running");

        let (project, opened) = Project::open(&dir).unwrap();
        let health = project.check_health().unwrap();
        assert!(health.is_healthy(), "run {run}: {health:?}");
        assert_eq!(health.progress.items_in(ItemState::InProgress), 0);
        assert!(opened.recovered.items <= SLOTS, "run {run}: {opened:?}");
        let shown = connections(&project);
        assert!(shown == old || shown == want.0, "run {run}: mixed {shown}");
        mid_work += usize::from(landed_mid_work(&project));
        drop(project);

        // Whether the kill fell before the re-run job existed or after, this ends it.
        let (project, _) = Project::open(&dir).unwrap();
        let pending = project.store().pending_ai_run().unwrap().is_some();
        drop(project);
        if pending {
            w.finish(&dir, &["run", "--model", Y]);
        } else if connections(&Project::open(&dir).unwrap().0) == old {
            w.finish(&dir, &["rerun", "--model", Y]);
        }
        let (project, _) = Project::open(&dir).unwrap();
        assert!(project.check_health().unwrap().is_healthy(), "run {run}");
        assert_eq!(connections(&project), want.0, "run {run}");
    }
    assert!(killed_running >= KILLS / 2, "only {killed_running} hit");
    assert!(
        mid_work >= 2,
        "only {mid_work} of {KILLS} fell among the units"
    );
    // A stand-in that logged its start after its run's check still counts.
    assert_eq!(no_orphans(&w, &[]), Vec::<u32>::new(), "left running");
}

/// The command line's own errors.
#[test]
fn ai_refuses_a_missing_project_and_bad_arguments() {
    let w = world();
    let out = w.ai(&w.project("none"), &["status"]).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no project"));
    let out = Command::new(CLI)
        .args(["ai", "run", "--all", "--documents"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}
