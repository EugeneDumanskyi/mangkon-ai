//! M5c criteria 1, 3, 4 and 5 on [`Workspace`]: the local runtime started on demand for an AI
//! stage and for Try (US-09.5 AC1), stopped on quit (AC2), failures that block AI jobs while
//! Layer 1 goes on (AC3, NFR-REL-12), "No model allowed for this project" (US-09.7 AC3), where
//! data goes during every call (US-09.8 AC1, NFR-SEC-05), and the first run with the demo
//! project (US-14.4). Fakes only: `mk-core-fake-llama-server`, a fake Hugging Face for the
//! model files, a scripted resolver, an in-process OpenAI-compatible server.

use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mk_core::{
    AiActivity, AiChoice, AiReadiness, ChatMessage, ChatRequest, DestinationClass, DownloadState,
    EgressConfig, EndpointInput, FixedProbe, Gpu, GpuVendor, HardwareProfile, JobStage, KeyChange,
    ModelRole, ModelsConfig, NO_MODEL_ALLOWED, Project, RuntimeConfig, RuntimeSource, RuntimeState,
    SpawnOptions, TlsRoots, WorkerConfig, Workspace,
};
use mk_egress::testkit::{Behavior, FakeKeyring, FakeServer, ScriptedResolver, private_lan_ip};
use mk_models::runtime::process::NO_GPU_MEMORY;
use mk_models::testkit::llama::{CONTROL_ENV, Control};
use mk_models::testkit::{
    FakeHf, NamedResolver, ORIGIN_HOST, test_bytes, test_catalog, test_model,
};
use mk_models::{GB, Model};

const FAKE: &str = env!("CARGO_BIN_EXE_mk-core-fake-llama-server");
const WAIT: Duration = Duration::from_secs(20);
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

fn machine() -> HardwareProfile {
    HardwareProfile {
        os_family: "linux".into(),
        os: "Linux test".into(),
        cpu: "Test CPU (8 threads)".into(),
        ram_bytes: 64 * GB,
        gpus: vec![Gpu {
            vendor: GpuVendor::Nvidia,
            name: "NVIDIA Test".into(),
            vram_bytes: Some(8_550_000_000),
            driver: Some("580.10".into()),
        }],
        unified_bytes: None,
        apple_silicon: false,
        free_disk_bytes: Some(100 * GB),
        disk_path: "/models".into(),
    }
}

struct Setup {
    tmp: tempfile::TempDir,
    control: PathBuf,
    resolver: Arc<ScriptedResolver>,
    ws: Workspace,
    project: Project,
    ext: Model,
    emb: Model,
    _hf: FakeHf,
}

/// A workspace whose runtime is the fake `llama-server` (set in settings, `PATH` empty), with
/// an extraction and an embedding model in the catalog, nothing installed yet, and a project
/// with three text files.
fn setup(control: Control) -> Setup {
    setup_with(control, |_| {})
}

fn setup_with(control: Control, tune: impl FnOnce(&mut RuntimeConfig)) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let control_file = tmp.path().join("control.json");
    control.write(&control_file);
    let ext_body = test_bytes(64 * 1024, 1);
    let emb_body = test_bytes(64 * 1024, 2);
    let ext = test_model(
        "ext",
        ModelRole::Extraction,
        std::slice::from_ref(&ext_body),
    );
    let emb = test_model(
        "emb",
        ModelRole::Embeddings,
        std::slice::from_ref(&emb_body),
    );
    let hf = FakeHf::serving(&[
        (&ext, std::slice::from_ref(&ext_body)),
        (&emb, std::slice::from_ref(&emb_body)),
    ]);
    let resolver = Arc::new(ScriptedResolver::new());
    let mut runtime = RuntimeConfig {
        path_var: Some(Default::default()),
        spawn: SpawnOptions {
            env: vec![(CONTROL_ENV.into(), control_file.clone().into())],
        },
        ..RuntimeConfig::default()
    };
    tune(&mut runtime);
    let ws = Workspace::open_with_runtime(
        &tmp.path().join("app"),
        EgressConfig {
            resolver: resolver.clone(),
            secrets: Some(Arc::new(FakeKeyring::new())),
            roots: TlsRoots::Platform,
            call_timeout: Duration::from_secs(10),
        },
        ModelsConfig {
            models_dir: Some(tmp.path().join("models")),
            catalog: test_catalog(vec![ext.clone(), emb.clone()]),
            probe: Arc::new(FixedProbe(machine())),
            download: hf.config(NamedResolver::new(&[ORIGIN_HOST])),
        },
        runtime,
    )
    .unwrap();
    ws.set_llama_server_path(Some(Path::new(FAKE))).unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    for (name, text) in [
        (
            "a.txt",
            "Invoice INV-2024-0042 paid by Brightwater Ltd, call +44 20 7946 0958.",
        ),
        (
            "b.txt",
            "Brightwater Ltd wrote to ops@brightwater.example about INV-2024-0042.",
        ),
        ("c.txt", "Nothing shared here."),
    ] {
        std::fs::write(src.join(name), text).unwrap();
    }
    let (mut project, _) = ws.create("Case", tmp.path()).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.add_source_root(&src).unwrap();
    Setup {
        tmp,
        control: control_file,
        resolver,
        ws,
        project,
        ext,
        emb,
        _hf: hf,
    }
}

impl Setup {
    fn control(&self, c: Control) {
        c.write(&self.control);
    }

    fn install(&self, m: &Model) {
        self.ws.start_download(&m.id).unwrap();
        wait_until("installed", || {
            self.ws
                .model_catalog()
                .unwrap()
                .iter()
                .any(|v| v.id == m.id && v.state == DownloadState::Installed)
        });
    }

    fn slot(&self, role: ModelRole) -> Option<mk_core::SlotStatus> {
        self.ws
            .supervisor()
            .status()
            .into_iter()
            .find(|s| s.role == role)
    }

    fn wait_state(&self, role: ModelRole, want: RuntimeState) -> bool {
        self.ws.supervisor().wait_for(WAIT, |s| {
            s.iter().any(|x| x.role == role && x.state == want)
        })
    }

    /// A Stage B job over every content of the project (a stand-in for M6's extraction).
    fn stage_b_job(&self) -> i64 {
        let ids: Vec<i64> = self
            .project
            .documents_page(&Default::default(), None, 100)
            .unwrap()
            .rows
            .iter()
            .map(|r| r.content_id)
            .collect();
        assert!(!ids.is_empty());
        self.project
            .store()
            .create_stage_job(JobStage::B, ids)
            .unwrap()
    }

    fn job(&self, id: i64) -> (String, Option<String>) {
        self.project.store().job_state(id).unwrap().unwrap()
    }
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + WAIT;
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn hello() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("Project text: INV-2024-0042")],
        ..ChatRequest::default()
    }
}

#[cfg(unix)]
fn alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(unix)]
fn kill_9(pid: u32) {
    std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
}

/// A recorder for the status bar's events.
fn record(ws: &Workspace) -> Arc<Mutex<Vec<Option<AiActivity>>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    ws.on_ai_activity(move |a| s.lock().unwrap().push(a));
    seen
}

// ---- ADR-0015 follow-up ----

/// The settings path, build and backend are shown; the path is kept in `app.db`.
#[test]
fn adr_0015_runtime_view_shows_path_build_and_backend() {
    let s = setup(Control::default());
    let v = s.ws.runtime_status();
    assert_eq!(v.binary.path.as_deref(), Some(Path::new(FAKE)));
    assert_eq!(v.binary.source, Some(RuntimeSource::Settings));
    assert_eq!(v.binary.build.as_ref().map(|b| b.build), Some(10863));
    assert_eq!(v.binary.backends, vec![mk_core::Backend::Cuda]);
    assert_eq!(v.binary.error, None);
    assert_eq!(v.backend_note, None);
    assert_eq!(v.settings_path.as_deref(), Some(Path::new(FAKE)));
    assert!(v.models.is_empty(), "nothing started before AI work");

    // A CPU-only build on a GPU machine: the hint.
    s.control(Control {
        devices: Some(String::new()),
        ..Control::default()
    });
    let v = s.ws.recheck_runtime();
    assert_eq!(v.binary.backends, vec![mk_core::Backend::Cpu]);
    assert!(v.backend_note.unwrap().contains("CUDA build"));

    // Not a program: refused, the old path stays.
    let e =
        s.ws.set_llama_server_path(Some(&s.tmp.path().join("nope")))
            .unwrap_err();
    assert_eq!(e.code, "not_executable");
    assert_eq!(
        s.ws.runtime_status().settings_path.as_deref(),
        Some(Path::new(FAKE))
    );

    // Cleared, with an empty PATH: not found, with the install guide.
    let v = s.ws.set_llama_server_path(None).unwrap();
    let (code, message) = v.binary.error.unwrap();
    assert_eq!(code, "not_found");
    assert_eq!(message, "llama-server not found");
    assert_eq!(v.install_guide, "docs/install-llama-cpp.md");
}

// ---- US-09.5 ----

/// US-09.5 AC1: an AI stage's client starts the model on demand on 127.0.0.1 with its key,
/// Busy while held, Ready after; the call goes through the gate as "this machine".
#[test]
fn us_09_5_ac1_ai_client_starts_on_demand() {
    let s = setup(Control::default());
    s.install(&s.ext);
    assert_eq!(
        s.slot(ModelRole::Extraction),
        None,
        "not started before it is needed"
    );
    match s.ws.ai_readiness(&s.project, ModelRole::Extraction) {
        AiReadiness::Ready {
            destination,
            class,
            name,
        } => {
            assert_eq!(destination, "Data sent to this machine");
            assert_eq!(class, DestinationClass::ThisMachine);
            assert_eq!(name, "Test ext");
        }
        r => panic!("{r:?}"),
    }
    let seen = record(&s.ws);
    let client = s.ws.ai_client(&s.project, ModelRole::Extraction).unwrap();
    let port = client.local_port().unwrap();
    let slot = s.slot(ModelRole::Extraction).unwrap();
    assert_eq!((slot.state, slot.port), (RuntimeState::Busy, Some(port)));
    let reply = client.client.chat(&hello()).unwrap();
    assert!(!reply.content.is_empty());
    client.succeeded();
    assert!(
        s.slot(ModelRole::Extraction)
            .unwrap()
            .tail
            .contains("auth=ok"),
        "the per-start key was sent"
    );
    drop(client);
    assert_eq!(
        s.slot(ModelRole::Extraction).unwrap().state,
        RuntimeState::Ready
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let a = seen[0].as_ref().unwrap();
    assert_eq!(
        (a.class, a.purpose),
        (DestinationClass::ThisMachine, "extraction")
    );
    assert_eq!(seen[1], None);
    // INV-EG-05: "This machine" is recorded too.
    let records = s.project.egress_records().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        (records[0].host.as_str(), records[0].class),
        ("127.0.0.1", DestinationClass::ThisMachine)
    );
}

/// US-09.5 AC1 by hand: Try starts the model and sends the fixed texts with the key.
#[test]
fn us_09_5_ac1_try_both_roles() {
    let s = setup(Control {
        dims: Some(16),
        ..Control::default()
    });
    s.install(&s.ext);
    s.install(&s.emb);
    let seen = record(&s.ws);
    let r = s.ws.try_local_model("ext").unwrap();
    assert!(r.ok, "{}", r.summary);
    // The catalog name, not the file path the runtime reports.
    assert!(
        r.summary.starts_with("Connected: Test ext answered in "),
        "{}",
        r.summary
    );
    assert_eq!(r.structured_output, Some(true));
    let port = s.slot(ModelRole::Extraction).unwrap().port.unwrap();
    assert_eq!(r.bound, Some(format!("127.0.0.1:{port}")));
    let r = s.ws.try_local_model("emb").unwrap();
    assert!(r.ok, "{}", r.summary);
    assert!(
        r.summary
            .starts_with("Connected: Test emb, 16 dimensions in "),
        "{}",
        r.summary
    );
    assert_eq!(r.dimensions, Some(16));
    assert_eq!(
        s.ws.supervisor().status().len(),
        2,
        "one process per loaded model"
    );
    let seen = seen.lock().unwrap().clone();
    assert!(seen.len() >= 4, "{seen:?}");
    assert!(
        seen.iter()
            .flatten()
            .all(|a| a.label() == "Data sent to this machine")
    );
    assert_eq!(seen.last(), Some(&None));

    // Not installed: plain words.
    let e = s.ws.try_local_model("nope").unwrap_err();
    assert_eq!(e.code, "model_not_found");
}

/// US-09.5 AC2: quitting the app stops and reaps every `llama-server`.
#[cfg(unix)]
#[test]
fn us_09_5_ac2_quit_stops() {
    let s = setup(Control::default());
    s.install(&s.ext);
    assert!(s.ws.try_local_model("ext").unwrap().ok);
    let pid = s.slot(ModelRole::Extraction).unwrap().pid.unwrap();
    assert!(alive(pid));
    s.ws.shut_down();
    assert!(!alive(pid), "llama-server outlived the app");
    s.ws.shut_down();
}

/// US-09.5 AC2: idle stop with a short timer; the next need starts it again on a new port.
#[test]
fn us_09_5_ac2_idle_stop() {
    let s = setup_with(Control::default(), |c| c.idle = Duration::from_millis(400));
    s.install(&s.ext);
    assert!(s.ws.try_local_model("ext").unwrap().ok);
    assert!(s.wait_state(ModelRole::Extraction, RuntimeState::Stopped));
    assert!(
        s.ws.try_local_model("ext").unwrap().ok,
        "started again on demand"
    );
}

/// US-09.5 AC3: out of GPU memory → the plain reason; the project's AI jobs are Blocked with it
/// while Stage A of the project's files finishes; after Retry they run again.
#[test]
fn us_09_5_ac3_failure_blocks_ai_jobs_layer1_goes_on() {
    let s = setup(Control {
        fail: Some("oom".into()),
        ..Control::default()
    });
    s.install(&s.ext);
    s.project.rescan(false).unwrap();
    let job = s.stage_b_job();
    let e = match s.ws.ai_client(&s.project, ModelRole::Extraction) {
        Err(e) => e,
        Ok(_) => panic!("started"),
    };
    assert_eq!(
        (e.code.as_str(), e.message.as_str(), e.blocked),
        ("runtime_failed", NO_GPU_MEMORY, true)
    );
    assert_eq!(s.job(job), ("blocked".into(), Some(NO_GPU_MEMORY.into())));
    match s.ws.ai_readiness(&s.project, ModelRole::Extraction) {
        AiReadiness::Blocked { code, reason } => {
            assert_eq!((code, reason.as_str()), ("runtime_failed", NO_GPU_MEMORY));
        }
        r => panic!("{r:?}"),
    }
    let slot = s.slot(ModelRole::Extraction).unwrap();
    assert_eq!(slot.state, RuntimeState::Failed);
    assert!(
        slot.tail.contains("out of memory"),
        "Show details has the output: {}",
        slot.tail
    );

    // Layer 1 goes on.
    let summary = s.project.ingest().unwrap();
    assert_eq!(summary.stage_a.done, 3, "{summary:?}");
    let p = s.project.progress_view().unwrap();
    assert_eq!(p.stage, None, "Stage A is done");
    assert_eq!(p.blocked.as_deref(), Some(NO_GPU_MEMORY));
    assert_eq!(s.job(job).0, "blocked", "Stage A left the AI job alone");

    // The user frees memory and presses Retry.
    s.control(Control::default());
    s.ws.retry_runtime();
    assert!(s.wait_state(ModelRole::Extraction, RuntimeState::Ready));
    assert!(s.ws.refresh_ai_blocks(&s.project).unwrap());
    assert_eq!(s.job(job), ("running".into(), None));
    assert_eq!(s.project.progress_view().unwrap().blocked, None);
}

/// NFR-REL-12, INV-MM-06: `kill -9` three times: restarted twice, then Failed with a reason;
/// AI jobs Blocked, Layer 1 goes on.
#[cfg(unix)]
#[test]
fn nfr_rel_12_kill_three_times() {
    let s = setup(Control::default());
    s.install(&s.ext);
    s.project.rescan(false).unwrap();
    let job = s.stage_b_job();
    drop(s.ws.ai_client(&s.project, ModelRole::Extraction).unwrap());
    for n in 1..=3u8 {
        let before = s.slot(ModelRole::Extraction).unwrap();
        assert_eq!(before.state, RuntimeState::Ready);
        kill_9(before.pid.unwrap());
        if n < 3 {
            assert!(
                s.ws.supervisor().wait_for(WAIT, |all| all.iter().any(|x| {
                    x.role == ModelRole::Extraction
                        && x.state == RuntimeState::Ready
                        && x.restarts == n
                })),
                "restart {n}"
            );
            assert_ne!(s.slot(ModelRole::Extraction).unwrap().pid, before.pid);
        }
    }
    assert!(s.wait_state(ModelRole::Extraction, RuntimeState::Failed));
    let failure = s.slot(ModelRole::Extraction).unwrap().failure.unwrap();
    assert_eq!(failure.code, "crashed");
    assert!(
        failure.message.contains("after 2 restarts"),
        "{}",
        failure.message
    );
    assert!(s.ws.refresh_ai_blocks(&s.project).unwrap());
    assert_eq!(
        s.job(job),
        ("blocked".into(), Some(failure.message.clone()))
    );
    assert_eq!(s.project.ingest().unwrap().stage_a.done, 3);
    assert_eq!(s.job(job).0, "blocked");
}

// ---- US-09.7 AC3 ----

/// US-09.7 AC3: no model installed and no endpoint, or an endpoint not allowed for this
/// project: "No model allowed for this project", and the AI jobs wait with that reason.
#[test]
fn us_09_7_ac3_no_model_allowed() {
    let s = setup(Control::default());
    s.project.rescan(false).unwrap();
    let job = s.stage_b_job();
    assert_eq!(
        s.ws.ai_readiness(&s.project, ModelRole::Extraction),
        AiReadiness::Blocked {
            code: "no_model",
            reason: NO_MODEL_ALLOWED.into()
        }
    );
    assert!(s.ws.refresh_ai_blocks(&s.project).unwrap());
    assert_eq!(
        s.job(job),
        ("blocked".into(), Some(NO_MODEL_ALLOWED.into()))
    );
    assert!(s.ws.ai_client(&s.project, ModelRole::Extraction).is_err());

    // An endpoint on the local network, chosen for AI but not allowed for this project.
    s.resolver.then("llm.lan.test", &["192.168.1.20"]);
    let e =
        s.ws.save_endpoint(EndpointInput {
            id: None,
            name: "Box".into(),
            base_url: "http://llm.lan.test:8080".into(),
            model: "m".into(),
            key: KeyChange::Keep,
        })
        .unwrap();
    let view =
        s.ws.set_ai_choice(
            ModelRole::Extraction,
            Some(AiChoice::Endpoint(e.id.clone())),
        )
        .unwrap();
    assert!(view.explicit);
    assert_eq!(view.name.as_deref(), Some("Box"));
    assert_eq!(
        s.ws.ai_readiness(&s.project, ModelRole::Extraction),
        AiReadiness::Blocked {
            code: "no_model",
            reason: NO_MODEL_ALLOWED.into()
        }
    );
    // Allowed: Ready, and the job runs again.
    let d = e.destination.unwrap();
    s.ws.confirm_allow(&s.project, &e.id, &d.origin, d.class)
        .unwrap();
    assert_eq!(s.job(job), ("running".into(), None));
    // Revoked: blocked again.
    s.ws.revoke_endpoint(&s.project, &e.id).unwrap();
    assert_eq!(
        s.job(job),
        ("blocked".into(), Some(NO_MODEL_ALLOWED.into()))
    );

    // A local model of the wrong role is refused.
    let e =
        s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Local("emb".into())))
            .unwrap_err();
    assert_eq!(e.code, "wrong_role");
}

/// INV-MM-04: unset, a role uses the recommended installed model; a switch away stops it.
#[test]
fn inv_mm_04_default_choice_and_switch_stops() {
    let s = setup(Control::default());
    assert_eq!(s.ws.ai_choice(ModelRole::Embeddings).unwrap().choice, None);
    s.install(&s.emb);
    let v = s.ws.ai_choice(ModelRole::Embeddings).unwrap();
    assert_eq!(
        (v.choice, v.explicit),
        (Some(AiChoice::Local("emb".into())), false)
    );
    assert!(s.ws.try_local_model("emb").unwrap().ok);
    let pid = s.slot(ModelRole::Embeddings).unwrap().pid.unwrap();
    s.resolver.then("127.0.0.1", &["127.0.0.1"]);
    let e =
        s.ws.save_endpoint(EndpointInput {
            id: None,
            name: "Here".into(),
            base_url: "http://127.0.0.1:9".into(),
            model: "m".into(),
            key: KeyChange::Keep,
        })
        .unwrap();
    s.ws.set_ai_choice(ModelRole::Embeddings, Some(AiChoice::Endpoint(e.id)))
        .unwrap();
    assert!(s.wait_state(ModelRole::Embeddings, RuntimeState::Stopped));
    #[cfg(unix)]
    assert!(!alive(pid));
    let _ = pid;
}

// ---- US-09.8 AC1, NFR-SEC-05 ----

/// US-09.8 AC1, NFR-SEC-05: every call through the gate is shown before it is sent, with its
/// host and class, and cleared when it ends, for each of the three classes. The LAN and
/// Internet hosts resolve to loopback first, so nothing leaves this machine.
#[test]
fn nfr_sec_05_every_call_published() {
    let s = setup(Control::default());
    let server = FakeServer::start(LOOPBACK, Behavior::openai("m"));
    let mut cases = vec![
        (
            "here.test",
            vec!["127.0.0.1"],
            DestinationClass::ThisMachine,
        ),
        (
            "box.lan.test",
            vec!["127.0.0.1", "192.168.1.20"],
            DestinationClass::LocalNetwork,
        ),
        (
            "api.example.test",
            vec!["127.0.0.1", "93.184.215.14"],
            DestinationClass::Internet,
        ),
    ];
    if let Some(ip) = private_lan_ip() {
        eprintln!("this machine's LAN address: {ip}");
    }
    let seen = record(&s.ws);
    for (host, ips, class) in cases.drain(..) {
        s.resolver.then(host, &ips);
        let e =
            s.ws.save_endpoint(EndpointInput {
                id: None,
                name: host.into(),
                base_url: format!("http://{host}:{}", server.port()),
                model: "m".into(),
                key: KeyChange::Keep,
            })
            .unwrap();
        let d = e.destination.clone().unwrap();
        assert_eq!(d.class, class, "{host}");
        s.ws.confirm_allow(&s.project, &e.id, &d.origin, d.class)
            .unwrap();
        s.ws.set_ai_choice(
            ModelRole::Extraction,
            Some(AiChoice::Endpoint(e.id.clone())),
        )
        .unwrap();
        seen.lock().unwrap().clear();
        let client = s.ws.ai_client(&s.project, ModelRole::Extraction).unwrap();
        let sent_before = server.count();
        client.client.chat(&hello()).unwrap();
        assert_eq!(server.count(), sent_before + 1);
        let events = seen.lock().unwrap().clone();
        assert_eq!(events.len(), 2, "{host}: {events:?}");
        let a = events[0].clone().unwrap();
        assert_eq!(
            (a.host.as_str(), a.class, a.purpose),
            (host, class, "extraction")
        );
        let want = match class {
            DestinationClass::ThisMachine => "Data sent to this machine".to_string(),
            DestinationClass::LocalNetwork => format!("Data sent to {host} (local network)"),
            DestinationClass::Internet => format!("Data sent to {host} (internet)"),
        };
        assert_eq!(a.label(), want);
        assert_eq!(events[1], None, "cleared when the call ended");
        assert_eq!(s.ws.ai_activity(), None);
    }

    // A failed call is cleared too.
    let dead = FakeServer::start(LOOPBACK, Behavior::openai("m"));
    let port = dead.port();
    drop(dead);
    s.resolver.then("gone.test", &["127.0.0.1"]);
    let e =
        s.ws.save_endpoint(EndpointInput {
            id: None,
            name: "gone".into(),
            base_url: format!("http://gone.test:{port}"),
            model: "m".into(),
            key: KeyChange::Keep,
        })
        .unwrap();
    s.ws.set_ai_choice(ModelRole::Extraction, Some(AiChoice::Endpoint(e.id)))
        .unwrap();
    seen.lock().unwrap().clear();
    let client = s.ws.ai_client(&s.project, ModelRole::Extraction).unwrap();
    assert!(client.client.chat(&hello()).is_err());
    let events = seen.lock().unwrap().clone();
    assert_eq!(events.len(), 2, "{events:?}");
    assert_eq!(events[1], None);
}

// ---- US-14.4 ----

fn demo_corpus(dir: &Path) {
    std::fs::create_dir_all(dir.join("synthetic/txt")).unwrap();
    std::fs::create_dir_all(dir.join("public")).unwrap();
    std::fs::write(
        dir.join("ground-truth.json"),
        r#"{"schema_version":1,"generator":{"name":"corpus-gen","version":6}}"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("synthetic/txt/memo.txt"),
        "Memo about INV-2024-0042.",
    )
    .unwrap();
    std::fs::write(dir.join("public/note.txt"), "INV-2024-0042 again.").unwrap();
}

/// US-14.4 AC1: the first-run screen shows on a fresh app and not after it is done or skipped,
/// nor once a project exists.
#[test]
fn us_14_4_ac1_first_run_shows_once() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    assert!(ws.first_run().unwrap().show);
    ws.finish_first_run().unwrap();
    assert!(!ws.first_run().unwrap().show);
    drop(ws);
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    assert!(!ws.first_run().unwrap().show, "kept in app.db");

    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(&tmp.path().join("app")).unwrap();
    ws.create("Mine", tmp.path()).unwrap();
    assert!(
        !ws.first_run().unwrap().show,
        "an app with a project is not new"
    );
}

/// US-14.4 AC2: "Open demo project" makes the demo project in the app's data folder with
/// TD-1's folders as sources, and its files are read. Opening it again reuses it.
#[test]
fn us_14_4_ac2_open_demo_project() {
    let tmp = tempfile::tempdir().unwrap();
    let corpus = tmp.path().join("repo/testdata/td1");
    demo_corpus(&corpus);
    let app = tmp.path().join("app");
    let ws = Workspace::open(&app).unwrap();
    // The user picks the repository folder: TD-1 is found inside it.
    let (mut project, _, entry) = ws.open_demo(&tmp.path().join("repo")).unwrap();
    assert_eq!(entry.name, mk_core::DEMO_NAME);
    assert_eq!(
        Path::new(&entry.path),
        std::fs::canonicalize(app.join(mk_core::DEMO_FOLDER)).unwrap()
    );
    assert!(!ws.first_run().unwrap().show);
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    let summary = project.ingest().unwrap();
    assert_eq!(summary.stage_a.done, 2, "{summary:?}");
    let roots = project.store().source_roots().unwrap();
    assert_eq!(roots.len(), 2);
    drop(project);

    let (project, _, again) = ws.open_demo(&corpus).unwrap();
    assert_eq!(again.id, entry.id);
    assert_eq!(
        project.store().source_roots().unwrap().len(),
        2,
        "no second copy"
    );
    assert_eq!(ws.recent().unwrap().len(), 1);

    // A folder without the demo files is refused in plain words.
    let Err(e) = ws.open_demo(tmp.path()) else {
        panic!("opened");
    };
    assert!(
        e.to_string().contains("does not hold the demo files"),
        "{e}"
    );
}

/// US-14.4 AC2 with the real TD-1, when it was generated in this checkout: found from the
/// test binary's folder (as from the app's).
#[test]
fn us_14_4_ac2_finds_td1_from_the_binary() {
    let td1 = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/td1");
    if !td1.join("ground-truth.json").is_file() {
        eprintln!("skipped: testdata/td1 is missing (see docs/07-test-corpus.md §9)");
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let found = mk_core::find_demo_corpus(&[exe.parent().unwrap().to_path_buf()]).unwrap();
    assert_eq!(found, std::fs::canonicalize(td1).unwrap());
}
