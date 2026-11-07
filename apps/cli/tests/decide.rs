//! M7a end to end: `proposals`, `merge`, `split`, `decisions` and `proposal-bench` on a project that
//! `llm-bench --work` kept, with the stand-in `llama-server` as the model; the link and proposal
//! reports (NFR-QA-01/02, spike S7); and `SIGKILL` among decisions (NFR-REL-02/03/04).

#![cfg(unix)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use mk_core::{Catalog, EgressConfig, ModelsConfig, Project, Workspace};
use mk_models::testkit::llama::{CONTROL_ENV, Control, FakeExtraction, FakeName};
use mk_store::AppStore;
use serde_json::{Value, json};

const CLI: &str = env!("CARGO_BIN_EXE_mangkon-cli");
const FAKE: &str = env!("CARGO_BIN_EXE_mangkon-cli-fake-llama-server");
const X: &str = "qwen3.5-4b-q4_k_m";
const CORVIN: &str = "person:corvin hale";
const C_HALE: &str = "person:c hale";
const GREGOR: &str = "person:gregor almqvist";
const MR_ALMQVIST: &str = "person:mr almqvist";
const KILLS: u64 = 20;

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

fn file_of(id: &str) -> String {
    let catalog = Catalog::shipped();
    let name = &catalog.model(id).unwrap().files[0].name;
    name.rsplit('/').next().unwrap().to_string()
}

/// Four notes with their ground truth. Corvin Hale is written two ways over four documents, Gregor
/// and Mr Almqvist are proposed but are two people here, and an email is in two notes.
fn corpus(root: &Path) -> PathBuf {
    let td = root.join("td");
    std::fs::create_dir_all(td.join("synthetic")).unwrap();
    let notes = [
        (
            "a",
            "Paid by Corvin Hale on 2 May, and a@x.example was copied in.\n",
        ),
        (
            "b",
            "Corvin Hale wrote about the dredging tender, and a@x.example was copied in.\n",
        ),
        (
            "c",
            "C. Hale called about the harbour.\nGregor Almqvist visited the yard.\n",
        ),
        ("d", "C. Hale wrote again.\nMr Almqvist visited the quay.\n"),
    ];
    for (name, text) in notes {
        std::fs::write(td.join(format!("synthetic/{name}.txt")), text).unwrap();
    }
    let file = |n: &str| {
        json!({ "path": format!("synthetic/{n}.txt"), "sha256": "", "bytes": 0, "origin": "synthetic",
                "kind": "txt", "encoding": "utf-8", "annotation": "full", "purpose": "" })
    };
    let doc = |n: &str| json!({ "id": n, "file": format!("synthetic/{n}.txt"), "part": { "kind": "whole" }, "kind": "txt" });
    let names = ["a", "b", "c", "d"];
    let truth = json!({
        "schema_version": 1,
        "generator": { "name": "t", "version": 1, "seed": 1 },
        "page_text_file": "page-text.jsonl",
        "files": names.map(file),
        "documents": names.map(doc),
        "entities": [
            { "id": "corvin", "kind": "person", "canonical": "Corvin Hale", "aliases": ["C. Hale"], "role": "" },
            { "id": "gregor", "kind": "person", "canonical": "Gregor Almqvist", "aliases": [], "role": "" },
            { "id": "gregory", "kind": "person", "canonical": "Mr Almqvist", "aliases": [], "role": "" },
        ],
        "mentions": [],
        "negatives": [],
        "links": [
            { "subject": { "kind": "identifier", "key": { "type": "email", "value": "a@x.example" } },
              "documents": ["a", "b"], "mentions": [] },
            { "subject": { "kind": "identifier", "key": { "type": "domain", "value": "x.example" } },
              "documents": ["a", "b"], "mentions": [] },
            { "subject": { "kind": "entity", "entity": "corvin" },
              "documents": ["a", "b", "c", "d"], "mentions": [] },
        ],
        "merges": [{ "entity": "corvin", "variants": ["Corvin Hale", "C. Hale"] }],
        "relations": [],
        "events": [],
    });
    std::fs::write(td.join("ground-truth.json"), truth.to_string()).unwrap();
    std::fs::write(td.join("page-text.jsonl"), "").unwrap();
    td
}

struct World {
    tmp: tempfile::TempDir,
    td: PathBuf,
}

fn person(n: &str) -> FakeName {
    FakeName {
        name: n.into(),
        ..FakeName::default()
    }
}

impl World {
    /// A project the stand-in model filled, kept in `work/run-1`, and the report of that run.
    fn new() -> (World, Value) {
        let tmp = tempfile::tempdir().unwrap();
        let td = corpus(tmp.path());
        let w = World { tmp, td };
        Control {
            extraction: HashMap::from([(
                file_of(X),
                FakeExtraction {
                    names: ["Corvin Hale", "C. Hale", "Gregor Almqvist", "Mr Almqvist"]
                        .map(person)
                        .to_vec(),
                    ..FakeExtraction::default()
                },
            )]),
            ..Control::default()
        }
        .write(&w.path("control.json"));
        let store = AppStore::open(&w.path("app")).unwrap();
        install(&store, &w.path("models"), X);
        drop(store);
        let ws = Workspace::open_with_models(
            &w.path("app"),
            EgressConfig::default(),
            ModelsConfig {
                models_dir: Some(w.path("models")),
                ..ModelsConfig::default()
            },
        )
        .unwrap();
        ws.set_local_override(true).unwrap();
        ws.shut_down();
        let out = Command::new(CLI)
            .arg("llm-bench")
            .args(["--model", X])
            .arg("--testdata")
            .arg(&w.td)
            .arg("--app-data")
            .arg(w.path("app"))
            .arg("--dir")
            .arg(w.path("models"))
            .arg("--llama-server")
            .arg(FAKE)
            .arg("--work")
            .arg(w.path("work"))
            .arg("--json")
            .arg(w.path("report.json"))
            .env(CONTROL_ENV, w.path("control.json"))
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        // llm-bench prints no names.
        assert!(!stdout.contains("hale"), "{stdout}");
        let report = read(&w.path("report.json"));
        (w, report)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.tmp.path().join(name)
    }

    fn project(&self) -> PathBuf {
        self.path("work/run-1")
    }

    fn cli(&self, words: &[&str]) -> Output {
        Command::new(CLI)
            .args(words)
            .arg("--project")
            .arg(self.project())
            .output()
            .unwrap()
    }

    fn ok(&self, words: &[&str]) -> String {
        let out = self.cli(words);
        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        assert!(
            out.status.success(),
            "{words:?}: {stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    }

    fn json(&self, words: &[&str]) -> Value {
        let mut all = words.to_vec();
        all.push("--json");
        serde_json::from_str(&self.ok(&all)).unwrap()
    }

    fn refused(&self, words: &[&str]) -> String {
        let out = self.cli(words);
        assert_eq!(out.status.code(), Some(1), "{words:?}");
        String::from_utf8_lossy(&out.stderr).to_string()
    }
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn target<'a>(report: &'a Value, id: &str) -> &'a Value {
    report["targets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == id)
        .unwrap()
}

/// NFR-QA-01/02, spike S7: with no merge accepted the email and the Corvin Hale row count; decided by
/// the ground truth, the variants come together on a copy and the project itself stays as it was.
#[test]
fn nfr_qa_01_02_link_and_proposal_reports() {
    let (w, report) = World::new();
    let run = &report["runs"][0];
    let links = &run["links"];
    assert_eq!(links["gold"], 3);
    assert_eq!(links["gold_found"], 3, "{links}");
    assert_eq!(links["gold_identifier_found"], 2);
    assert_eq!(links["gold_entity_found"], 1);
    assert_eq!(links["wrong"], json!([]), "{links}");
    assert_eq!(target(&report, "NFR-QA-01")["median"], 100.0);
    assert_eq!(target(&report, "NFR-QA-02")["median"], 100.0);

    let proposals = &run["proposals"];
    assert!((proposals["threshold"].as_f64().unwrap() - 0.6).abs() < 1e-6);
    assert_eq!(proposals["variants_reached"], 1, "{proposals}");
    assert_eq!(proposals["without_reason"], 0);
    assert_eq!(run["sweep"].as_array().unwrap().len(), 8);
    assert!(!run["pairs"].as_array().unwrap().is_empty());

    let decided = &run["decided"];
    assert_eq!(decided["accepted"], 1, "{decided}");
    assert_eq!(decided["rejected"], 1, "the lookalikes: {decided}");
    assert_eq!(decided["left"], 0);
    assert_eq!(decided["rebuild_matches"], true);
    assert_eq!(decided["healthy"], true);
    assert_eq!(decided["links"]["gold_found"], 3);

    let out = Command::new(CLI)
        .arg("proposal-bench")
        .arg("--project")
        .arg(w.project())
        .arg("--testdata")
        .arg(&w.td)
        .arg("--json")
        .arg(w.path("s7.json"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("threshold"), "{stdout}");
    assert!(!stdout.contains("hale"), "no names on stdout: {stdout}");
    let s7 = read(&w.path("s7.json"));
    assert_eq!(s7["sweep"], run["sweep"]);
    assert_eq!(s7["decided"]["accepted"], 1);
    assert_eq!(s7["links"]["gold_found"], 3);
    // The bench decided on a copy.
    assert_eq!(w.json(&["decisions"]), json!([]));
}

/// US-11.2, US-11.4, US-11.5 AC2 through the command line.
#[test]
fn us_11_5_ac2_decisions_through_the_command_line() {
    let (w, _) = World::new();
    assert_eq!(w.ok(&["decisions"]).trim(), "No decisions yet.");
    let list = w.json(&["proposals"]);
    let rows = list.as_array().unwrap();
    assert_eq!(rows.len(), 2, "{list}");
    assert!(
        rows.iter()
            .all(|p| !p["reasons"].as_array().unwrap().is_empty()),
        "{list}"
    );
    let text = w.ok(&["proposals", "--limit", "1"]);
    assert!(text.contains(" / "), "{text}");
    assert_eq!(
        w.json(&["proposals", "--limit", "1"])
            .as_array()
            .unwrap()
            .len(),
        1
    );

    assert!(
        w.ok(&["merge", "reject", GREGOR, MR_ALMQVIST])
            .starts_with("Rejected")
    );
    assert_eq!(w.json(&["proposals"]).as_array().unwrap().len(), 1);
    assert!(
        w.refused(&["merge", "accept", GREGOR, MR_ALMQVIST])
            .contains("no longer pending")
    );
    let merged = w.json(&[
        "merge",
        "accept",
        C_HALE,
        CORVIN,
        "--name",
        "Corvin A. Hale",
    ]);
    assert!(merged["seq"].as_i64().unwrap() > 1);
    assert!(w.ok(&["split", C_HALE]).contains("Corvin Hale"));
    let undo = w.ok(&["merge", "undo", C_HALE, CORVIN]);
    assert!(undo.starts_with("Undone"), "{undo}");
    assert!(
        w.refused(&["merge", "undo", C_HALE, CORVIN])
            .contains("not merged")
    );

    let mentions = w.json(&["split", CORVIN]);
    let id = mentions[0]["mention"].as_i64().unwrap().to_string();
    let split = w.json(&["split", CORVIN, "--mention", &id]);
    let key = split["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("split:"), "{split}");
    assert_eq!(w.json(&["split", &key]).as_array().unwrap().len(), 1);
    assert!(w.ok(&["split", "undo", &key]).starts_with("Put"));
    assert!(w.refused(&["split", "undo", &key]).contains("not split"));

    let log = w.json(&["decisions"]);
    let kinds: Vec<&str> = log
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "undo_split",
            "split",
            "reject_merge",
            "rename",
            "accept_merge",
            "reject_merge"
        ]
    );
    for d in log.as_array().unwrap() {
        assert!(d["decided_at"].as_str().unwrap().len() >= 19, "{d}");
        assert!(d["after"].is_string(), "{d}");
    }
    assert_eq!(log[0]["status"], "Active");
    assert_eq!(log[1]["status"], "Superseded");
    assert_eq!(log[4]["status"], "Superseded");
    assert_eq!(
        w.json(&["decisions", "--limit", "2"])
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let text = w.ok(&["decisions"]);
    assert!(text.contains("reject_merge"), "{text}");
    assert!(
        text.contains("1 mentions") || text.contains("-> "),
        "{text}"
    );

    let rebuilt = w.json(&["rebuild"]);
    assert_eq!(rebuilt["same_as_incremental"], true, "{rebuilt}");
}

/// Usage errors say so and exit 2; a folder with no project exits 1.
#[test]
fn us_11_5_ac2_bad_arguments() {
    for args in [
        vec!["merge", "accept", "a"],
        vec!["merge", "reject", "a", "b", "--name", "x"],
        vec!["merge", "accept", "a", "b", "--type", "fish"],
        vec!["split"],
        vec!["split", "a", "--mention", "x"],
        vec!["decisions"],
        vec!["proposals"],
        vec!["proposal-bench"],
    ] {
        let out = Command::new(CLI).args(&args).output().unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}");
    }
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(CLI)
        .args(["decisions", "--project"])
        .arg(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
}

/// M7a: `SIGKILL` among decisions, at seeded moments. A decision and its derived rows are one
/// transaction, so every kill leaves a healthy project equal to a rebuild, with every decision the
/// process reported in the log, and the log whole.
#[test]
fn nfr_rel_02_kill_among_decisions_is_all_or_nothing() {
    let (w, _) = World::new();
    let mut state = 0x5eed_0007u64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut highest = 0;
    let mut kinds = std::collections::BTreeSet::new();
    for _ in 0..KILLS {
        let mut child = Command::new(CLI)
            .arg("kill-decide-target")
            .arg(w.project())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        // Started and deciding, then a moment that falls anywhere in a decision.
        lines.next().unwrap().unwrap();
        std::thread::sleep(Duration::from_micros(next() % 40_000));
        child.kill().unwrap();
        child.wait().unwrap();
        let reported: Vec<i64> = lines
            .map_while(Result::ok)
            .filter_map(|l| l.strip_prefix("decided ").and_then(|s| s.parse().ok()))
            .collect();

        let (project, _) = Project::open(&w.project()).unwrap();
        let health = project.check_health().unwrap();
        assert!(health.is_healthy(), "{health:?}");
        // Equal to the reference and to a rebuild: no half of a decision is visible.
        let rebuilt = project.rebuild().unwrap();
        assert!(
            rebuilt.same_as_incremental && rebuilt.consistent,
            "{rebuilt:?}"
        );
        let log = project.decisions(None, 100_000).unwrap();
        let seqs: std::collections::BTreeSet<i64> = log.iter().map(|d| d.seq).collect();
        for seq in &reported {
            assert!(seqs.contains(seq), "decision {seq} was reported, then lost");
        }
        let newest = *seqs.last().unwrap_or(&0);
        assert!(newest >= highest, "the log went back");
        highest = newest;
        kinds.extend(log.iter().map(|d| d.kind.as_str().to_string()));
    }
    assert!(
        highest > KILLS as i64,
        "the kills came too early: {highest}"
    );
    assert!(kinds.len() >= 6, "{kinds:?}");
}
