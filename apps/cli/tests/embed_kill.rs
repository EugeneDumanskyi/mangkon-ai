//! M6b, NFR-REL-02/03/04, ADR-0005, INV-SR-02: `SIGKILL` of `mangkon-cli embed run` and
//! `embed rerun`, 10 each, against the stand-in `llama-server`, half at seeded moments and half among
//! the parts. After every kill the project is healthy, lost work is at most the one part in progress,
//! a re-run leaves one model's vectors searchable and never a mix, and going on ends equal to a run
//! without a kill. No stand-in stays.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mk_core::{Catalog, EgressConfig, ItemState, ModelsConfig, Project, Workspace};
use mk_models::testkit::llama::{CONTROL_ENV, Control, FakeEmbeddings};
use mk_store::AppStore;
use serde_json::Value;

const CLI: &str = env!("CARGO_BIN_EXE_mangkon-cli");
const FAKE: &str = env!("CARGO_BIN_EXE_mangkon-cli-fake-llama-server");
/// One request in flight (ADR-0006).
const IN_PROGRESS: usize = 1;
const KILLS: usize = 10;

const X: &str = "bge-m3-q8_0";
const Y: &str = "nomic-embed-text-v1.5-q8_0";

struct World {
    tmp: tempfile::TempDir,
    source: PathBuf,
    control: PathBuf,
    starts: PathBuf,
    requests: PathBuf,
}

/// Stands a shipped model in with an empty file of the size the catalog says.
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
            format!("Note {f}. The harbour dredging was paid.\nChecked by the committee {f}.\n"),
        )
        .unwrap();
    }
    let starts = tmp.path().join("starts.log");
    let requests = tmp.path().join("requests.log");
    let control = tmp.path().join("control.json");
    Control {
        starts_log: Some(starts.clone()),
        requests_log: Some(requests.clone()),
        dims: Some(32),
        embeddings: HashMap::from([(
            "*".to_string(),
            FakeEmbeddings {
                delay_ms: 20,
                ..FakeEmbeddings::default()
            },
        )]),
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

    fn cli(&self, project: &Path, words: &[&str]) -> Command {
        let mut c = Command::new(CLI);
        c.args(words)
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

    fn finish(&self, project: &Path, words: &[&str]) -> Value {
        let out = self.cli(project, words).output().unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
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

/// Every vector with what it is of, independent of row ids: equal means the same vectors, once each.
fn vectors(project: &Project) -> Vec<(String, i64, i64, String, String)> {
    project
        .store()
        .read_raw(|c| {
            let mut stmt = c.prepare(
                "SELECT s.text, e.char_start, e.char_end, hex(e.vector), a.model_identity
                 FROM segment_embedding e
                 JOIN segment s ON s.id = e.segment_id
                 JOIN ai_run a ON a.id = e.ai_run_id
                 ORDER BY 1, 2, 3, 5",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .unwrap()
}

/// The vectors of the run a query would search, without the model column.
fn searchable(project: &Project) -> Vec<(String, i64, i64, String)> {
    let Some(run) = project.store().active_embedding_run().unwrap() else {
        return Vec::new();
    };
    vectors(project)
        .into_iter()
        .filter(|v| v.4 == run.identity.as_str())
        .map(|v| (v.0, v.1, v.2, v.3))
        .collect()
}

fn landed_mid_work(project: &Project) -> bool {
    let p = project.store().embedding_progress().unwrap();
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
    /// Among the parts, whatever the machine's speed: this many requests reached the model. The
    /// lane asks again only after committing its last part, so 2 mean at least one is done.
    AfterRequests(usize),
}

fn when(run: usize, seed: &mut u64, window: Duration) -> When {
    if run.is_multiple_of(2) {
        When::At(moment(seed, window))
    } else {
        When::AfterRequests(2 + moment(seed, Duration::from_secs(30)).as_secs() as usize)
    }
}

/// Spawns `words`, kills it at `when`; true if it was still running then.
fn kill_at(w: &World, project: &Path, words: &[&str], when: When) -> bool {
    std::fs::write(&w.requests, "").unwrap();
    let started = Instant::now();
    let mut child = w
        .cli(project, words)
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

/// NFR-REL-02/03/04: 10 kills of `embed run`.
#[test]
fn nfr_rel_02_kill_during_embed_run() {
    let w = world();
    let reference_dir = w.project("reference");
    w.ingest(&reference_dir);
    let t = Instant::now();
    let done = w.finish(&reference_dir, &["embed", "run", "--model", X]);
    let window = t.elapsed();
    assert_eq!(done["state"], "ready", "{done}");
    let (reference, _) = Project::open(&reference_dir).unwrap();
    let want = vectors(&reference);
    assert!(want.len() >= 40, "{}", want.len());
    drop(reference);

    let mut seed = 31;
    let mut killed_running = 0;
    let mut mid_work = 0;
    for run in 0..KILLS {
        let dir = w.project(&format!("run-{run}"));
        w.ingest(&dir);
        let before = w.started();
        let at = when(run, &mut seed, window);
        killed_running += usize::from(kill_at(&w, &dir, &["embed", "run", "--model", X], at));
        let orphans = no_orphans(&w, &before);
        assert!(orphans.is_empty(), "run {run}: {orphans:?} left running");

        let (project, opened) = Project::open(&dir).unwrap();
        let health = project.check_health().unwrap();
        assert!(health.is_healthy(), "run {run}: {health:?}");
        assert_eq!(health.progress.items_in(ItemState::InProgress), 0);
        assert!(
            opened.recovered.items <= IN_PROGRESS,
            "run {run}: {opened:?}"
        );
        mid_work += usize::from(landed_mid_work(&project));
        drop(project);

        let done = w.finish(&dir, &["embed", "run", "--model", X]);
        assert_eq!(done["state"], "ready", "run {run}: {done}");
        let (project, _) = Project::open(&dir).unwrap();
        assert!(project.check_health().unwrap().is_healthy(), "run {run}");
        assert_eq!(vectors(&project), want, "run {run}");
    }
    assert!(killed_running >= KILLS / 2, "only {killed_running} hit");
    assert!(
        mid_work >= 2,
        "only {mid_work} of {KILLS} fell among the parts"
    );
    assert_eq!(no_orphans(&w, &[]), Vec::<u32>::new(), "left running");
}

/// NFR-REL-02/03/04, ADR-0005, INV-SR-02: 10 kills of `embed rerun`. Whenever it dies, what a query
/// would search is the old model's vectors or the new one's, whole.
#[test]
fn adr_0005_kill_during_embed_rerun_never_mixes() {
    let w = world();
    let reference_dir = w.project("reference");
    w.ingest(&reference_dir);
    w.finish(&reference_dir, &["embed", "run", "--model", X]);
    let old = searchable(&Project::open(&reference_dir).unwrap().0);
    let t = Instant::now();
    w.finish(&reference_dir, &["embed", "rerun", "--model", Y]);
    let window = t.elapsed();
    let (reference, _) = Project::open(&reference_dir).unwrap();
    let want = searchable(&reference);
    assert_eq!(want.len(), old.len());
    assert_ne!(want, old);
    assert_eq!(
        vectors(&reference).len(),
        want.len(),
        "only the new model's stay"
    );
    drop(reference);

    let mut seed = 43;
    let mut killed_running = 0;
    let mut mid_work = 0;
    for run in 0..KILLS {
        let dir = w.project(&format!("run-{run}"));
        w.ingest(&dir);
        w.finish(&dir, &["embed", "run", "--model", X]);
        let before = w.started();
        let at = when(run, &mut seed, window);
        killed_running += usize::from(kill_at(&w, &dir, &["embed", "rerun", "--model", Y], at));
        let orphans = no_orphans(&w, &before);
        assert!(orphans.is_empty(), "run {run}: {orphans:?} left running");

        let (project, opened) = Project::open(&dir).unwrap();
        let health = project.check_health().unwrap();
        assert!(health.is_healthy(), "run {run}: {health:?}");
        assert_eq!(health.progress.items_in(ItemState::InProgress), 0);
        assert!(
            opened.recovered.items <= IN_PROGRESS,
            "run {run}: {opened:?}"
        );
        let shown = searchable(&project);
        assert!(shown == old || shown == want, "run {run}: mixed");
        mid_work += usize::from(landed_mid_work(&project));
        let pending = project.store().pending_embedding_run().unwrap().is_some();
        drop(project);

        // Whether the kill fell before the re-run job existed or after, this ends it.
        if pending {
            w.finish(&dir, &["embed", "run", "--model", Y]);
        } else if shown == old {
            w.finish(&dir, &["embed", "rerun", "--model", Y]);
        }
        let (project, _) = Project::open(&dir).unwrap();
        assert!(project.check_health().unwrap().is_healthy(), "run {run}");
        assert_eq!(searchable(&project), want, "run {run}");
        assert_eq!(vectors(&project).len(), want.len(), "run {run}");
    }
    assert!(killed_running >= KILLS / 2, "only {killed_running} hit");
    assert!(
        mid_work >= 2,
        "only {mid_work} of {KILLS} fell among the parts"
    );
    assert_eq!(no_orphans(&w, &[]), Vec::<u32>::new(), "left running");
}

/// `mangkon-cli meaning`: not ready says why and exits 1; ready gives the top 20 with scores.
#[test]
fn us_06_2_meaning_from_the_command_line() {
    let w = world();
    let dir = w.project("query");
    w.ingest(&dir);
    let out = w
        .cli(&dir, &["meaning", "harbour dredging", "--model", X])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2), "--model is for embed");

    let out = w
        .cli(&dir, &["meaning", "harbour dredging"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let said: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(said["not_ready"], "Embeddings have not run yet");

    w.finish(&dir, &["embed", "run", "--model", X]);
    let found = w.finish(&dir, &["meaning", "note 7 harbour dredging"]);
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 20);
    assert!(hits[0]["path"].as_str().unwrap().contains("note-"));
    assert!(hits[0]["score"].as_f64().unwrap() >= hits[19]["score"].as_f64().unwrap());

    let status = w.finish(&dir, &["embed", "status"]);
    assert_eq!(status["state"], "ready");
    assert_eq!(status["rerun_model"], Value::Null);
    let cancel = w.finish(&dir, &["embed", "cancel"]);
    assert_eq!(cancel["cancelled"], false);
}

/// The command line's own errors.
#[test]
fn embed_refuses_a_missing_project_and_bad_arguments() {
    let w = world();
    let out = w
        .cli(&w.project("none"), &["embed", "status"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("no project"));
    let out = Command::new(CLI)
        .args(["embed", "status", "--all"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let out = Command::new(CLI).args(["meaning"]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
}
