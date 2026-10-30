//! Repo tasks, run as `cargo xtask <task>` (alias in `.cargo/config.toml`).
//!
//! - `build [--release] [-- <cargo args>]`: the one-command build (US-14.1 AC1/AC2, NFR-MNT-01).
//! - `ci`: every local gate, in the same order as `.github/workflows/ci.yml`; the TD-1 bench gate
//!   only when `testdata/td1` is there (it is not in git).
//! - `check-layers`: the dependency rules (ADR-0002, ADR-0009).
//! - `check-bindings`: the committed IPC bindings match the Rust types (ADR-0003).
//! - `notices [--check]`: writes or checks `THIRD_PARTY_NOTICES` (NFR-LEG-05).
//! - `mem [--pid <pid>] [--seconds <s>] [--every <s>]`: samples the running app's memory (NFR-RES-01).

mod layers;
mod mem;
mod notices;
mod prereq;

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const USAGE: &str = "\
Usage: cargo xtask <task>

Tasks:
  build [--release] [-- <cargo args>]  Check prerequisites, build the UI, build the desktop app
  ci                                   Run every CI gate locally, in CI order
  check-layers                         Check the dependency rules (ADR-0002, ADR-0009)
  check-bindings                       Check ui/src/ipc/bindings matches the Rust IPC types
  notices [--check]                    Write (or check) THIRD_PARTY_NOTICES
  mem [--pid P] [--seconds S] [--every S]  Sample the running app's memory (Linux, NFR-RES-01)";

type Result<T = ()> = std::result::Result<T, String>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") => build(&rest),
        Some("ci") => ci(),
        Some("check-layers") => check_layers(),
        Some("check-bindings") => check_bindings(&root()),
        Some("notices") => notices(rest.contains(&"--check")),
        Some("mem") => mem::mem(&rest),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("\nerror: {e}");
            ExitCode::FAILURE
        }
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .unwrap()
        .to_path_buf()
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn run<I, S>(program: &str, args: I, dir: &Path) -> Result
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let args: Vec<_> = args.into_iter().map(|a| a.as_ref().to_owned()).collect();
    let shown: Vec<_> = args.iter().map(|a| a.to_string_lossy()).collect();
    // `cargo` arrives as a full toolchain path; the bare name reads better.
    let name = Path::new(program)
        .file_stem()
        .map_or(program.into(), |n| n.to_string_lossy());
    println!("\n==> {name} {}", shown.join(" "));
    let status = Command::new(program)
        .args(&args)
        .current_dir(dir)
        .status()
        .map_err(|e| format!("could not run {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("`{name} {}` failed ({status})", shown.join(" ")))
    }
}

fn check_prerequisites() -> Result {
    let missing = prereq::check();
    if missing.is_empty() {
        return Ok(());
    }
    eprintln!("Missing build prerequisites:");
    for m in &missing {
        eprintln!("  - {}\n    Install: {}", m.what, m.how);
    }
    Err(format!(
        "{} prerequisite(s) missing. See README, \"Build from source\".",
        missing.len()
    ))
}

fn build_ui(root: &Path) -> Result {
    let ui = root.join("ui");
    run(prereq::npm(), ["ci", "--no-audit", "--no-fund"], &ui)?;
    run(prereq::npm(), ["run", "build"], &ui)
}

fn build(args: &[&str]) -> Result {
    let root = root();
    let (ours, extra) = match args.iter().position(|a| *a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[][..]),
    };
    let release = ours.contains(&"--release");
    if let Some(bad) = ours.iter().find(|a| **a != "--release") {
        return Err(format!("unknown build option {bad:?}\n\n{USAGE}"));
    }
    check_prerequisites()?;
    build_ui(&root)?;
    let mut cargo_args = vec!["build", "--locked", "-p", "mangkon-desktop"];
    if release {
        cargo_args.push("--release");
    }
    cargo_args.extend(extra);
    run(&cargo(), &cargo_args, &root)?;
    let exe = format!("mangkon{}", std::env::consts::EXE_SUFFIX);
    let profile = if release { "release" } else { "debug" };
    println!(
        "\nBuilt: {}",
        root.join("target").join(profile).join(exe).display()
    );
    Ok(())
}

fn ci() -> Result {
    let root = root();
    let ui = root.join("ui");
    let npm = prereq::npm();
    let cargo = cargo();
    check_prerequisites()?;

    // The UI first: the desktop crate embeds ui/dist at compile time.
    run(npm, ["ci", "--no-audit", "--no-fund"], &ui)?;
    for script in [
        "typecheck",
        "lint",
        "format:check",
        "test",
        "licenses",
        "build",
    ] {
        run(npm, ["run", script], &ui)?;
    }

    run(&cargo, ["fmt", "--all", "--", "--check"], &root)?;
    run(
        &cargo,
        [
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--locked",
            "--",
            "-D",
            "warnings",
        ],
        &root,
    )?;
    // Before `cargo test`, which rewrites ui/src/ipc/bindings in place.
    check_bindings(&root)?;
    run(
        &cargo,
        ["test", "--workspace", "--all-features", "--locked"],
        &root,
    )?;
    run(&cargo, ["deny", "--locked", "check"], &root)
        .map_err(|e| format!("{e}\n  (install with: cargo install --locked cargo-deny)"))?;
    check_layers()?;
    notices(true)?;
    run(
        &cargo,
        ["build", "--locked", "-p", "mangkon-desktop"],
        &root,
    )?;
    bench(&root)?;
    println!("\nAll CI gates passed.");
    Ok(())
}

/// The TD-1 identifier benchmark against the committed baseline (US-14.3 AC2, NFR-MNT-05), when
/// the corpus has been fetched and generated (`docs/07-test-corpus.md` §9).
fn bench(root: &Path) -> Result {
    if !root.join("testdata/td1/ground-truth.json").exists() {
        println!(
            "\n==> TD-1 bench gate skipped: testdata/td1 is missing (corpus-gen fetch, generate)"
        );
        return Ok(());
    }
    run(
        &cargo(),
        [
            "run",
            "--locked",
            "--release",
            "-p",
            "mangkon-cli",
            "--",
            "bench",
            "--baseline",
            "crates/mk-bench/baseline/td1.json",
        ],
        root,
    )
}

/// Exports the IPC bindings into a scratch folder and compares them with `ui/src/ipc/bindings`.
fn check_bindings(root: &Path) -> Result {
    let scratch = root.join("target/bindings-check");
    let _ = std::fs::remove_dir_all(&scratch);
    println!("\n==> IPC bindings drift check");
    let status = Command::new(cargo())
        .args([
            "test",
            "--locked",
            "--all-features",
            "-p",
            "mangkon-desktop",
            "export_bindings",
        ])
        .env("TS_RS_EXPORT_DIR", &scratch)
        .current_dir(root)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("exporting the IPC bindings failed".into());
    }
    let committed = root.join("ui/src/ipc/bindings");
    let (a, b) = (dir_files(&scratch)?, dir_files(&committed)?);
    let differ: BTreeSet<_> = a
        .iter()
        .chain(&b)
        .filter(|name| {
            std::fs::read(scratch.join(name)).ok() != std::fs::read(committed.join(name)).ok()
        })
        .collect();
    if differ.is_empty() {
        return Ok(());
    }
    Err(format!(
        "ui/src/ipc/bindings is out of date ({}). Run `cargo test -p mangkon-desktop` and commit \
         the regenerated files.",
        differ.into_iter().cloned().collect::<Vec<_>>().join(", ")
    ))
}

fn dir_files(dir: &Path) -> Result<BTreeSet<String>> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .collect())
}

/// `cargo metadata` resolved for one target, with every feature on.
fn metadata(root: &Path, target: &str) -> Result<layers::Metadata> {
    let out = Command::new(cargo())
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
            "--filter-platform",
            target,
        ])
        .current_dir(root)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned());
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))
}

fn check_layers() -> Result {
    let root = root();
    println!("\n==> check-layers ({} targets)", layers::TARGETS.len());
    let mut all = BTreeSet::new();
    for target in layers::TARGETS {
        all.extend(layers::violations(&metadata(&root, target)?));
    }
    if all.is_empty() {
        println!(
            "Dependency rules hold: Tauri only in mangkon-desktop; HTTP clients only via mk-egress or mk-models."
        );
        return Ok(());
    }
    for v in &all {
        eprintln!("  {v}");
    }
    Err(format!("{} dependency rule violation(s)", all.len()))
}

fn notices(check: bool) -> Result {
    let root = root();
    let metas = layers::TARGETS
        .iter()
        .map(|t| metadata(&root, t))
        .collect::<Result<Vec<_>>>()?;
    let text = notices::render(&root, &metas)?;
    let path = root.join("THIRD_PARTY_NOTICES");
    if check {
        println!("\n==> notices --check");
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            return Err(
                "THIRD_PARTY_NOTICES is out of date. Run `cargo xtask notices` and commit it."
                    .into(),
            );
        }
        return Ok(());
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    println!("Wrote {}", path.display());
    Ok(())
}
