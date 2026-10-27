//! M2 acceptance criterion 1, short form (runs in CI): 3 forced kills of `mangkon-cli` during
//! ingestion of a generated TXT/CSV folder leave healthy projects that resume to the same result
//! as a run without a kill (NFR-REL-02/03/04). The 20-kill run on the TD-1 TXT/CSV subset is
//! `mangkon-cli kill-test --source testdata/td1/synthetic --runs 20 --seed 1`.

use std::fmt::Write as _;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// 40 files that share some addresses, large enough that Stage A takes a moment.
fn write_source(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    for f in 0..30 {
        let mut text = String::new();
        for line in 0..2_000 {
            let _ = writeln!(
                text,
                "note {f}.{line} from p{}@team.example to shared{}@x.example",
                line % 97,
                (f + line) % 13
            );
        }
        std::fs::write(dir.join(format!("notes-{f:02}.txt")), text).unwrap();
    }
    for f in 0..10 {
        let mut csv = String::from("date,name,email,amount\n");
        for row in 0..2_000 {
            let _ = writeln!(
                csv,
                "2024-01-{:02},Name {f}.{row},shared{}@x.example,{}.00",
                row % 28 + 1,
                (f * 7 + row) % 13,
                row * 3
            );
        }
        std::fs::write(dir.join(format!("ledger-{f:02}.csv")), csv).unwrap();
    }
}

#[test]
fn nfr_rel_02_kill_test_short() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("source");
    write_source(&source);
    let out = Command::new(env!("CARGO_BIN_EXE_mangkon-cli"))
        .args([
            "kill-test",
            "--runs",
            "3",
            "--seed",
            "7",
            "--json",
            "--source",
        ])
        .arg(&source)
        .arg("--work")
        .arg(tmp.path().join("work"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(summary["passed"], 3, "{stdout}");
    assert_eq!(summary["reference_done"], 40);
    for r in summary["results"].as_array().unwrap() {
        assert_eq!(r["failures"].as_array().unwrap().len(), 0, "{r}");
        assert_eq!(r["orphans"].as_array().unwrap().len(), 0, "{r}");
    }
}

#[test]
fn kill_test_needs_a_source() {
    let out = Command::new(env!("CARGO_BIN_EXE_mangkon-cli"))
        .arg("kill-test")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--source"));
}
