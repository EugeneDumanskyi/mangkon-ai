//! M6c, NFR-MNT-06: `mangkon-cli llm-bench` end to end on a small corpus, with the stand-in
//! `llama-server` in place of the models: both lanes run, the names are scored on real Stage A
//! output, invented names lower precision, and the AI choice goes back.

#![cfg(unix)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use mk_core::{Catalog, EgressConfig, ModelRole, ModelsConfig, Workspace};
use mk_models::testkit::llama::{CONTROL_ENV, Control, FakeExtraction, FakeName};
use mk_store::AppStore;
use serde_json::{Value, json};

const CLI: &str = env!("CARGO_BIN_EXE_mangkon-cli");
const FAKE: &str = env!("CARGO_BIN_EXE_mangkon-cli-fake-llama-server");
const X: &str = "qwen3.5-4b-q4_k_m";
const E: &str = "bge-m3-q8_0";

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

fn entity(id: &str, doc: &str, kind: &str, raw: &str, key: &str, line: u32) -> Value {
    json!({
        "id": id, "document": doc, "raw": raw, "difficulty": "normal",
        "location": { "kind": "text_line", "line": line, "start": 0, "end": 1 },
        "subject": { "kind": "entity", "entity": kind, "name_key": key },
    })
}

/// A two-note corpus with its ground truth: Corvin Hale and Velmora Port Authority in both.
fn corpus(root: &Path) -> PathBuf {
    let td = root.join("td");
    std::fs::create_dir_all(td.join("synthetic")).unwrap();
    let a = "Paid by Corvin Hale on 2 May.\nVelmora Port Authority signed for Invented Holdings.\n";
    let b =
        "Corvin Hale wrote to Velmora Port Authority about the dredging tender for the harbour.\n";
    std::fs::write(td.join("synthetic/a.txt"), a).unwrap();
    std::fs::write(td.join("synthetic/b.txt"), b).unwrap();
    let file = |path: &str| {
        json!({ "path": path, "sha256": "", "bytes": 0, "origin": "synthetic", "kind": "txt",
                "encoding": "utf-8", "annotation": "full", "purpose": "" })
    };
    let doc = |id: &str, f: &str| json!({ "id": id, "file": f, "part": { "kind": "whole" }, "kind": "txt" });
    let (hale, port) = ("person:corvin hale", "org:velmora port authority");
    let truth = json!({
        "schema_version": 1,
        "generator": { "name": "t", "version": 1, "seed": 1 },
        "page_text_file": "page-text.jsonl",
        "files": [file("synthetic/a.txt"), file("synthetic/b.txt")],
        "documents": [doc("a", "synthetic/a.txt"), doc("b", "synthetic/b.txt")],
        "entities": [],
        "mentions": [
            entity("m1", "a", "e1", "Corvin Hale", hale, 1),
            entity("m2", "a", "e2", "Velmora Port Authority", port, 2),
            entity("m3", "b", "e1", "Corvin Hale", hale, 1),
            entity("m4", "b", "e2", "Velmora Port Authority", port, 1),
        ],
        "negatives": [], "links": [], "merges": [], "relations": [], "events": [],
    });
    std::fs::write(td.join("ground-truth.json"), truth.to_string()).unwrap();
    std::fs::write(td.join("page-text.jsonl"), "").unwrap();
    td
}

struct World {
    tmp: tempfile::TempDir,
    td: PathBuf,
}

fn name(n: &str, kind: &str) -> FakeName {
    FakeName {
        name: n.into(),
        kind: kind.into(),
        ..FakeName::default()
    }
}

impl World {
    fn new(names: Vec<FakeName>) -> World {
        let tmp = tempfile::tempdir().unwrap();
        let td = corpus(tmp.path());
        let w = World { tmp, td };
        Control {
            extraction: HashMap::from([(
                file_of(X),
                FakeExtraction {
                    names,
                    ..FakeExtraction::default()
                },
            )]),
            ..Control::default()
        }
        .write(&w.control());
        let store = AppStore::open(&w.app()).unwrap();
        install(&store, &w.models(), X);
        install(&store, &w.models(), E);
        drop(store);
        let ws = w.workspace();
        ws.set_local_override(true).unwrap();
        ws.shut_down();
        w
    }

    fn app(&self) -> PathBuf {
        self.tmp.path().join("app")
    }
    fn models(&self) -> PathBuf {
        self.tmp.path().join("models")
    }
    fn control(&self) -> PathBuf {
        self.tmp.path().join("control.json")
    }

    fn workspace(&self) -> Workspace {
        Workspace::open_with_models(
            &self.app(),
            EgressConfig::default(),
            ModelsConfig {
                models_dir: Some(self.models()),
                ..ModelsConfig::default()
            },
        )
        .unwrap()
    }

    fn bench(&self, runs: u32, out: &str, embeddings: bool) -> (Output, PathBuf) {
        let json = self.tmp.path().join(out);
        let mut cmd = Command::new(CLI);
        cmd.arg("llm-bench")
            .args(["--model", X, "--runs", &runs.to_string()])
            .arg("--testdata")
            .arg(&self.td)
            .arg("--app-data")
            .arg(self.app())
            .arg("--dir")
            .arg(self.models())
            .arg("--llama-server")
            .arg(FAKE)
            .arg("--work")
            .arg(self.tmp.path().join(format!("work-{out}")))
            .arg("--json")
            .arg(&json)
            .env(CONTROL_ENV, self.control());
        if embeddings {
            cmd.args(["--embedding-model", E]);
        }
        (cmd.output().unwrap(), json)
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

#[test]
fn nfr_mnt_06_scores_both_lanes_times_and_median() {
    let w = World::new(vec![
        name("Corvin Hale", "person"),
        name("Velmora Port Authority", "org"),
    ]);
    let (out, json) = w.bench(3, "right.json", true);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = read(&json);
    let runs = report["runs"].as_array().unwrap();
    assert_eq!(runs.len(), 3);
    for run in runs {
        let shown = &run["names"]["overall"]["shown"];
        assert_eq!(
            (&shown["tp"], &shown["fp"], &shown["fn"]),
            (&json!(4), &json!(0), &json!(0))
        );
        assert_eq!(run["names"]["links_shown"]["correct"], 2);
        assert_eq!(run["units_failed"], 0);
        assert_eq!(run["rebuild_matches"], true);
        assert_eq!(run["healthy"], true);
        assert_eq!(run["unmapped"], json!([]));
        assert!(run["first_connection_s"].as_f64().unwrap() > 0.0);
        assert!(run["embeddings"]["windows"].as_u64().unwrap() > 0);
        assert!(run["embeddings"]["queries"].as_u64().unwrap() >= 50);
        let recall = run["embeddings"]["recall_at_20"].as_f64().unwrap();
        assert!((0.9..=1.0).contains(&recall), "{recall}");
    }
    assert_eq!(target(&report, "NFR-QA-08")["met"], true);
    assert_eq!(target(&report, "NFR-QA-08")["median"], 100.0);
    assert_eq!(target(&report, "NFR-QA-09")["met"], true);
    assert_eq!(target(&report, "NFR-PERF-12")["met"], true);
    // The report holds the names; stdout does not.
    assert!(!stdout.contains("corvin hale"), "{stdout}");
}

#[test]
fn nfr_qa_08_invented_names_lower_precision_and_are_listed() {
    let w = World::new(vec![
        name("Corvin Hale", "person"),
        name("Velmora Port Authority", "org"),
        name("Invented Holdings", "org"),
    ]);
    let (out, json) = w.bench(1, "noisy.json", false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = read(&json);
    let run = &report["runs"][0];
    let shown = &run["names"]["overall"]["shown"];
    assert_eq!((&shown["tp"], &shown["fp"]), (&json!(4), &json!(1)));
    assert_eq!(target(&report, "NFR-QA-08")["median"], 80.0);
    assert_eq!(target(&report, "NFR-QA-08")["met"], false);
    assert_eq!(target(&report, "NFR-QA-09")["met"], true);
    assert_eq!(
        run["names"]["false_positives"][0],
        json!(["org:invented holdings", 1])
    );
    // No embedding model: that lane and its targets are left out, not passed.
    assert_eq!(run["embeddings"], Value::Null);
    assert_eq!(target(&report, "NFR-PERF-13")["met"], false);
    assert_eq!(target(&report, "NFR-PERF-13")["median"], Value::Null);
}

#[test]
fn nfr_qa_09_a_model_that_finds_nothing_is_a_miss_not_a_failure() {
    let w = World::new(Vec::new());
    let (out, json) = w.bench(1, "none.json", false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let report = read(&json);
    assert_eq!(target(&report, "NFR-QA-09")["median"], 0.0);
    assert_eq!(target(&report, "NFR-QA-09")["met"], false);
    assert_eq!(report["runs"][0]["first_connection_s"], Value::Null);
}

#[test]
fn the_ai_choice_goes_back_to_what_it_was() {
    let w = World::new(vec![name("Corvin Hale", "person")]);
    let ws = w.workspace();
    assert!(!ws.ai_choice(ModelRole::Extraction).unwrap().explicit);
    ws.shut_down();
    let (out, _) = w.bench(1, "choice.json", true);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ws = w.workspace();
    assert!(!ws.ai_choice(ModelRole::Extraction).unwrap().explicit);
    assert!(!ws.ai_choice(ModelRole::Embeddings).unwrap().explicit);
    ws.shut_down();
}

#[test]
fn a_model_that_is_not_installed_stops_with_a_reason() {
    let w = World::new(Vec::new());
    let out = Command::new(CLI)
        .arg("llm-bench")
        .args(["--model", "qwen3.6-35b-a3b-q8_0"])
        .arg("--testdata")
        .arg(&w.td)
        .arg("--app-data")
        .arg(w.app())
        .arg("--dir")
        .arg(w.models())
        .arg("--llama-server")
        .arg(FAKE)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
}
