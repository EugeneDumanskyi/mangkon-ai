//! `mangkon-cli ingest` and `rebuild` on a temporary project (M2d; ADR-0005, NFR-USE-05).

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn cli(args: &[&str], paths: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mangkon-cli"))
        .args(args)
        .args(paths)
        .output()
        .unwrap()
}

fn json(out: &Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn cli_ingest_then_rebuild_keeps_connections() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "from shared@x.example\nonly@a.example\n").unwrap();
    std::fs::write(src.join("b.csv"), "name,email\nAnn,shared@x.example\n").unwrap();
    std::fs::write(src.join("bad.txt"), b"\xff\xfe\x00 not text \xc3\x28").unwrap();
    let project = tmp.path().join("project");
    let p = project.to_str().unwrap();

    let first = json(&cli(&["ingest", "--json", "--project", p], &[&src]));
    assert_eq!(first["walked"], 3);
    assert_eq!(first["run"]["done"], 2);
    assert_eq!(first["run"]["failed"], 1);
    assert_eq!(first["connections"], 1);
    // NFR-USE-05: the failed file is listed with a plain reason.
    let failed = first["failed_files"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert!(failed[0]["path"].as_str().unwrap().ends_with("bad.txt"));
    assert!(!failed[0]["reason"].as_str().unwrap().is_empty());

    // The same source again: kept once, nothing re-read.
    let again = json(&cli(&["ingest", "--json", "--project", p], &[&src]));
    assert_eq!(
        (again["hashed"].as_u64(), again["run"]["done"].as_u64()),
        (Some(0), Some(0))
    );
    assert_eq!(again["files"]["total"], 3);
    assert_eq!(again["connections"], 1);

    let rebuilt = json(&cli(&["rebuild", "--json", "--project", p], &[]));
    assert_eq!(rebuilt["connections"], 1);
    assert_eq!(rebuilt["same_as_incremental"], true);
    assert_eq!(rebuilt["consistent"], true);

    let text = cli(&["rebuild", "--project", p], &[]);
    assert!(text.status.success());
    assert!(String::from_utf8_lossy(&text.stdout).contains("Same as before       yes"));
}

#[test]
fn cli_rebuild_needs_a_project_and_ingest_needs_its_source() {
    let tmp = tempfile::tempdir().unwrap();
    let none = tmp.path().join("none");
    let out = cli(&["rebuild", "--project", none.to_str().unwrap()], &[]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!none.exists(), "rebuild never creates a project");

    let p = tmp.path().join("p");
    let out = cli(
        &["ingest", "--project", p.to_str().unwrap()],
        &[&tmp.path().join("missing")],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing"));

    let out = cli(&["ingest", "SRC"], &[]);
    assert_eq!(out.status.code(), Some(2), "no --project is a usage error");
}
