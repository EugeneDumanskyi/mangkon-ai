//! `mangkon-cli projects`, `retry`, `remove` and `relocate` (M3c; US-01.1, US-01.2, US-01.5,
//! US-02.7 AC3, D12).

use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mangkon-cli"))
        .args(args)
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

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

#[test]
fn us_01_1_cli_projects_create_list_forget() {
    let tmp = tempfile::tempdir().unwrap();
    let app = tmp.path().join("app");
    let loc = tmp.path().join("loc");
    std::fs::create_dir_all(&loc).unwrap();

    let made = json(&cli(&[
        "projects",
        "create",
        "Case One",
        "--in",
        s(&loc),
        "--app-data",
        s(&app),
        "--json",
    ]));
    assert!(made["path"].as_str().unwrap().ends_with("Case One"));
    // INV-WS-01: names are unique, case aside.
    let clash = cli(&[
        "projects",
        "create",
        "case one",
        "--in",
        s(&loc),
        "--app-data",
        s(&app),
    ]);
    assert!(!clash.status.success());
    assert!(String::from_utf8_lossy(&clash.stderr).contains("already exists"));

    let list = json(&cli(&["projects", "list", "--app-data", s(&app), "--json"]));
    assert_eq!(list.as_array().unwrap().len(), 1);
    assert_eq!(list[0]["name"], "Case One");
    assert_eq!(list[0]["present"], true);

    json(&cli(&[
        "projects",
        "forget",
        s(&loc.join("Case One")),
        "--app-data",
        s(&app),
        "--json",
    ]));
    let list = json(&cli(&["projects", "list", "--app-data", s(&app), "--json"]));
    assert!(list.as_array().unwrap().is_empty());
    assert!(loc.join("Case One").is_dir(), "the folder is kept");
}

#[test]
fn us_01_5_cli_remove_retry_relocate() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("a.txt"), "from shared@x.example\n").unwrap();
    std::fs::write(src.join("b.csv"), "name,email\nAnn,shared@x.example\n").unwrap();
    std::fs::write(
        src.join("bad.txt"),
        b"\x89PNG\r\n\x1a\n\x00 not text \xc3\x28",
    )
    .unwrap();
    let project = tmp.path().join("project");
    let p = s(&project);

    let first = json(&cli(&["ingest", "--json", "--project", p, s(&src)]));
    assert_eq!(first["added"], 3);
    // The shared email and its domain (US-04.3 AC2).
    assert_eq!(first["connections"], 2);

    // US-02.7 AC3: only that file runs again (and fails again).
    let retried = json(&cli(&[
        "retry",
        "--project",
        p,
        "--path",
        s(&src.join("bad.txt")),
        "--json",
    ]));
    assert_eq!(retried["queued"], 1);
    assert_eq!(retried["run"]["failed"], 1);

    // US-01.5 AC1–AC2: the file is gone from the project, not from disk.
    let removed = json(&cli(&[
        "remove",
        "--project",
        p,
        "--json",
        s(&src.join("b.csv")),
    ]));
    assert_eq!(removed["removed"], 1);
    assert_eq!(removed["connections_losing_evidence"], 2);
    assert_eq!(removed["connections"], 0);
    assert!(src.join("b.csv").is_file());

    // D12: a moved source is found again with nothing read again.
    let moved = tmp.path().join("moved");
    std::fs::rename(&src, &moved).unwrap();
    let relocated = json(&cli(&[
        "relocate",
        "--project",
        p,
        "--root",
        s(&src),
        "--to",
        s(&moved),
        "--json",
    ]));
    assert_eq!(relocated["walked"], 2, "b.csv stays excluded");
    assert_eq!(relocated["run"]["done"], 0);
    assert_eq!(relocated["run"]["failed"], 0);
}
