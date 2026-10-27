//! Kill-test driver (M2 acceptance criterion 1; NFR-REL-02/03/04, ADR-0006).
//!
//! 1. Three reference runs: `kill-test-target` ingests the source into a fresh project and
//!    exits. They must end with the same derived rows; the shortest ingestion time (start to
//!    `phase finished`) is `T`.
//! 2. Each run ingests into a fresh project and kills the target at a seeded random moment in
//!    `[0, T)`, after listing its workers. Then:
//!    - every worker of the killed target exits by itself within 2 s (no orphans);
//!    - reopening recovers at most one in-progress item per lane (lost work, NFR-REL-03) and
//!      keeps every Done item the target had seen committed;
//!    - the project is healthy: SQLite integrity and foreign keys, derived rows equal a rebuild
//!      (NFR-REL-02), no item left in progress (NFR-REL-04);
//!    - ingesting again finishes the project, healthy, with the reference's derived rows and
//!      Done/Failed counts.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use mk_core::{ItemState, Project, ProjectError};
use serde_json::{Value, json};

/// How long the workers of a killed target may take to notice and exit.
const ORPHAN_WAIT: Duration = Duration::from_secs(2);

const REFERENCE_RUNS: usize = 3;

pub struct Options {
    pub source: PathBuf,
    pub runs: usize,
    pub seed: u64,
    pub work: Option<PathBuf>,
    pub json: bool,
}

/// What a target printed last: its phase and the highest Done count it saw committed.
#[derive(Clone, Default)]
struct Seen {
    phase: String,
    done: u64,
    /// When the `phase finished` line came.
    finished_at: Option<Instant>,
}

/// One kill and what the checks found.
struct RunResult {
    run: usize,
    kill_at: Duration,
    seen: Seen,
    /// The target exited before the kill moment.
    finished_first: bool,
    /// Workers still running right after the kill (they exit on their own soon after).
    workers_after_kill: usize,
    orphans: Vec<u32>,
    done_after_kill: u64,
    recovered: usize,
    in_progress_after_reopen: u64,
    healthy_after_kill: bool,
    healthy_after_resume: bool,
    same_as_reference: bool,
}

impl RunResult {
    fn failures(&self, lanes: usize) -> Vec<&'static str> {
        let checks = [
            (
                self.orphans.is_empty(),
                "workers outlived the killed process",
            ),
            (
                self.done_after_kill >= self.seen.done,
                "committed items were lost",
            ),
            (
                self.recovered <= lanes,
                "more items were lost than were in progress",
            ),
            (
                self.in_progress_after_reopen == 0,
                "items stayed in progress after reopening",
            ),
            (
                self.healthy_after_kill,
                "the project was unhealthy after the kill",
            ),
            (
                self.healthy_after_resume,
                "the project was unhealthy after resuming",
            ),
            (
                self.same_as_reference,
                "the resumed project differs from the reference",
            ),
        ];
        checks
            .iter()
            .filter(|(ok, _)| !ok)
            .map(|(_, why)| *why)
            .collect()
    }

    fn json(&self, lanes: usize) -> Value {
        json!({
            "run": self.run,
            "kill_at_ms": self.kill_at.as_millis() as u64,
            "phase": self.seen.phase,
            "finished_before_kill": self.finished_first,
            "workers_after_kill": self.workers_after_kill,
            "orphans": self.orphans,
            "done_seen": self.seen.done,
            "done_after_kill": self.done_after_kill,
            "recovered_items": self.recovered,
            "healthy_after_kill": self.healthy_after_kill,
            "healthy_after_resume": self.healthy_after_resume,
            "same_as_reference": self.same_as_reference,
            "failures": self.failures(lanes),
        })
    }
}

/// `kill-test-target PROJECT SOURCE`: opens (or creates) the project, adds the source, re-scans
/// and runs Stage A, printing `phase …` and `done N` lines for the driver.
pub fn target(project: &Path, source: &Path) -> ExitCode {
    match ingest(project, source, true) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Open, add the source (a no-op when it is there), re-scan, Stage A.
fn ingest(dir: &Path, source: &Path, report: bool) -> Result<Project, ProjectError> {
    let say = |line: &str| {
        if report {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    };
    say("phase open");
    let (project, _) = Project::open(dir)?;
    say("phase rescan");
    project.add_source_root(source)?;
    project.rescan(false)?;
    say("phase stage-a");
    let stop = AtomicBool::new(false);
    let run = std::thread::scope(|s| {
        if report {
            // A read connection sees only committed rows, so each count printed here is durable.
            s.spawn(|| {
                let mut last = 0;
                while !stop.load(Ordering::Relaxed) {
                    if let Ok(p) = project.store().progress() {
                        let done = p.items_in(ItemState::Done);
                        if done > last {
                            last = done;
                            say(&format!("done {done}"));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            });
        }
        let run = project.run_stage_a();
        stop.store(true, Ordering::Relaxed);
        run
    });
    run?;
    say(&format!(
        "done {}",
        project.store().progress()?.items_in(ItemState::Done)
    ));
    say("phase finished");
    Ok(project)
}

/// splitmix64: a small seeded generator for the kill moments.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, max)`.
    fn below(&mut self, max: Duration) -> Duration {
        let unit = (self.next() >> 11) as f64 / (1u64 << 53) as f64;
        max.mul_f64(unit)
    }
}

fn spawn_target(
    project: &Path,
    source: &Path,
) -> std::io::Result<(Child, JoinHandle<()>, Arc<Mutex<Seen>>)> {
    let mut child = Command::new(std::env::current_exe()?)
        .arg("kill-test-target")
        .arg(project)
        .arg(source)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let seen = Arc::new(Mutex::new(Seen::default()));
    let out = child.stdout.take().expect("piped stdout");
    let s = Arc::clone(&seen);
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let mut seen = s.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(phase) = line.strip_prefix("phase ") {
                if phase == "finished" {
                    seen.finished_at = Some(Instant::now());
                }
                seen.phase = phase.to_string();
            } else if let Some(n) = line.strip_prefix("done ").and_then(|n| n.parse().ok()) {
                seen.done = n;
            }
        }
    });
    Ok((child, reader, seen))
}

/// Derived rows and Done/Failed counts of a finished project, for comparison.
fn fingerprint(project: &Project) -> Result<(String, u64, u64), ProjectError> {
    let p = project.store().progress()?;
    Ok((
        format!("{:?}", project.store().derived_state()?),
        p.items_in(ItemState::Done),
        p.items_in(ItemState::Failed),
    ))
}

fn fresh(dir: &Path) -> std::io::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)
}

pub fn run(opts: &Options) -> ExitCode {
    let (work, temporary) = match &opts.work {
        Some(w) => (w.clone(), false),
        None => (
            std::env::temp_dir().join(format!("mangkon-kill-test-{}", std::process::id())),
            true,
        ),
    };
    match drive(opts, &work) {
        Ok(true) => {
            if temporary {
                let _ = std::fs::remove_dir_all(&work);
            }
            ExitCode::SUCCESS
        }
        Ok(false) => {
            eprintln!("projects kept in {}", work.display());
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Runs the reference and every kill; prints the report. `Ok(true)` when every run passed.
fn drive(opts: &Options, work: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    let source = std::fs::canonicalize(&opts.source)?;
    let exe = std::fs::canonicalize(std::env::current_exe()?)?;

    // Three reference runs: the first warms the caches, the shortest sets the kill window, and
    // all three must end the same.
    let mut reference = None;
    let mut reference_time = Duration::MAX;
    let mut lanes = 0;
    for i in 1..=REFERENCE_RUNS {
        let dir = work.join(format!("reference-{i}"));
        fresh(&dir)?;
        let started = Instant::now();
        let (mut child, reader, seen) = spawn_target(&dir, &source)?;
        let status = child.wait()?;
        let _ = reader.join();
        // Kills land between the start and the end of ingestion, not while the process closes.
        let finished_at = seen.lock().unwrap_or_else(|e| e.into_inner()).finished_at;
        match (status.success(), finished_at) {
            (true, Some(t)) => reference_time = reference_time.min(t - started),
            _ => return Err(format!("reference run {i} failed ({status})").into()),
        }
        let (project, _) = Project::open(&dir)?;
        let got = fingerprint(&project)?;
        lanes = project.stage_a_workers();
        match &reference {
            None => reference = Some(got),
            Some(r) if *r != got => {
                return Err(
                    "the reference runs ended differently; ingestion is not repeatable".into(),
                );
            }
            Some(_) => {}
        }
    }
    let reference = reference.expect("at least one reference run");

    let mut rng = Rng(opts.seed);
    let mut results = Vec::new();
    for run in 1..=opts.runs {
        let dir = work.join(format!("run-{run:02}"));
        fresh(&dir)?;
        let kill_at = rng.below(reference_time);
        let started = Instant::now();
        let before = mk_core::worker_processes(&exe);
        let (mut child, reader, seen) = spawn_target(&dir, &source)?;
        let mut finished_first = false;
        while started.elapsed() < kill_at {
            if child.try_wait()?.is_some() {
                finished_first = true;
                break;
            }
            std::thread::sleep(
                (kill_at - started.elapsed().min(kill_at)).min(Duration::from_millis(2)),
            );
        }
        let _ = child.kill();
        child.wait()?;
        let _ = reader.join();
        let seen = seen.lock().unwrap_or_else(|e| e.into_inner()).clone();

        // The killed target's workers: `--worker` processes of this program that were not
        // there before it started.
        let deadline = Instant::now() + ORPHAN_WAIT;
        let mut workers = None;
        let orphans = loop {
            let left: Vec<u32> = mk_core::worker_processes(&exe)
                .into_iter()
                .filter(|p| !before.contains(p))
                .collect();
            workers.get_or_insert(left.len());
            if left.is_empty() || Instant::now() >= deadline {
                break left;
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        // Reopening runs the restart recovery (ADR-0006).
        let (project, opened) = Project::open(&dir)?;
        let health = project.check_health()?;
        let done_after_kill = health.progress.items_in(ItemState::Done);
        let in_progress_after_reopen = health.progress.items_in(ItemState::InProgress);
        drop(project);

        let project = ingest(&dir, &source, false)?;
        let healthy_after_resume = project.check_health()?.is_healthy();
        let same_as_reference = fingerprint(&project)? == reference;
        drop(project);

        let result = RunResult {
            run,
            kill_at,
            seen,
            finished_first,
            workers_after_kill: workers.unwrap_or(0),
            orphans,
            done_after_kill,
            recovered: opened.recovered.items,
            in_progress_after_reopen,
            healthy_after_kill: health.is_healthy(),
            healthy_after_resume,
            same_as_reference,
        };
        if !opts.json {
            print_row(&result, lanes, results.is_empty(), reference_time);
        }
        results.push(result);
    }

    let passed = results
        .iter()
        .filter(|r| r.failures(lanes).is_empty())
        .count();
    if opts.json {
        let summary = json!({
            "source": source.display().to_string(),
            "seed": opts.seed,
            "runs": opts.runs,
            "lanes": lanes,
            "reference_ms": reference_time.as_millis() as u64,
            "reference_done": reference.1,
            "reference_failed": reference.2,
            "results": results.iter().map(|r| r.json(lanes)).collect::<Vec<_>>(),
            "passed": passed,
        });
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!("\n{passed}/{} runs passed", results.len());
    }
    Ok(passed == results.len())
}

fn print_row(r: &RunResult, lanes: usize, first: bool, reference: Duration) {
    if first {
        println!(
            "shortest reference ingestion: {} ms, {lanes} lanes\n\n{:>3}  {:>8}  {:<9} {:>9}  {:>9}  {:>7}  {:>7}  result",
            reference.as_millis(),
            "run",
            "kill at",
            "phase",
            "done seen",
            "recovered",
            "orphans",
            "healthy",
        );
    }
    let failures = r.failures(lanes);
    let phase = if r.finished_first {
        "finished"
    } else {
        r.seen.phase.as_str()
    };
    println!(
        "{:>3}  {:>5} ms  {:<9} {:>9}  {:>9}  {:>7}  {:>7}  {}",
        r.run,
        r.kill_at.as_millis(),
        if phase.is_empty() { "start" } else { phase },
        r.seen.done,
        r.recovered,
        r.orphans.len(),
        if r.healthy_after_kill { "yes" } else { "no" },
        if failures.is_empty() {
            "ok".to_string()
        } else {
            failures.join("; ")
        },
    );
}
