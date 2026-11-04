//! The runtime supervisor on the real process path (US-09.5, NFR-REL-12, NFR-SEC-06,
//! INV-MM-05/06), against `mk-fake-llama-server`.

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use mk_models::Role;
use mk_models::runtime::process::{BAD_MODEL_FILE, NO_GPU_MEMORY};
use mk_models::runtime::{
    ModelSpec, RuntimeConfig, RuntimeState, SlotStatus, SpawnOptions, Supervisor,
};
use mk_models::testkit::llama::{CONTROL_ENV, Control};

const FAKE: &str = env!("CARGO_BIN_EXE_mk-fake-llama-server");
const WAIT: Duration = Duration::from_secs(20);

struct Rig {
    dir: tempfile::TempDir,
    control: PathBuf,
    sup: Supervisor,
}

impl Rig {
    fn new(control: Control, tune: impl FnOnce(&mut RuntimeConfig)) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.json");
        control.write(&path);
        std::fs::write(dir.path().join("x.gguf"), b"GGUF").unwrap();
        std::fs::write(dir.path().join("y.gguf"), b"GGUF").unwrap();
        let mut cfg = RuntimeConfig {
            path_var: Some(Default::default()),
            spawn: SpawnOptions {
                env: vec![(CONTROL_ENV.into(), path.clone().into())],
            },
            ..RuntimeConfig::default()
        };
        tune(&mut cfg);
        let sup = Supervisor::new(cfg);
        sup.set_settings_path(Some(FAKE.into()));
        Rig {
            dir,
            control: path,
            sup,
        }
    }

    fn spec(&self, file: &str, role: Role) -> ModelSpec {
        ModelSpec {
            id: file.trim_end_matches(".gguf").into(),
            role,
            files: vec![self.dir.path().join(file)],
            context: 4096,
        }
    }

    fn control(&self, c: Control) {
        c.write(&self.control);
    }

    fn starts(&self) -> usize {
        std::fs::read_to_string(self.dir.path().join("starts.log")).map_or(0, |s| s.lines().count())
    }

    fn slot(&self, role: Role) -> SlotStatus {
        self.sup
            .status()
            .into_iter()
            .find(|s| s.role == role)
            .expect("slot")
    }

    fn wait_state(&self, role: Role, want: RuntimeState) -> bool {
        self.sup.wait_for(WAIT, |s| {
            s.iter().any(|x| x.role == role && x.state == want)
        })
    }
}

fn logged(dir: &Path) -> Control {
    Control {
        starts_log: Some(dir.join("starts.log")),
        ..Control::default()
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(Duration::from_secs(5)))
        .build()
        .into()
}

fn chat(port: u16, key: Option<&str>) -> u16 {
    let mut req = agent().post(format!("http://127.0.0.1:{port}/v1/chat/completions"));
    if let Some(k) = key {
        req = req.header("Authorization", format!("Bearer {k}"));
    }
    req.content_type("application/json")
        .send(r#"{"messages":[{"role":"user","content":"hi"}]}"#)
        .map_or(0, |r| r.status().as_u16())
}

fn port_open(port: u16) -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(300)).is_ok()
}

/// US-09.5 AC1, NFR-SEC-06: started on demand on 127.0.0.1 with a random port and key, Ready
/// only after `/health`; Busy while leased.
#[test]
fn us_09_5_ac1_start_on_demand_loopback_and_key() {
    let rig = Rig::new(Control::default(), |_| {});
    rig.control(Control {
        start_delay_ms: 600,
        ..logged(rig.dir.path())
    });
    let spec = rig.spec("x.gguf", Role::Extraction);
    assert!(
        rig.sup.status().is_empty(),
        "every app start begins at Stopped"
    );
    let lease = rig.sup.ensure_ready(&spec).unwrap();
    assert_eq!(rig.slot(Role::Extraction).state, RuntimeState::Busy);
    let port = lease.port();
    assert_eq!(lease.key().len(), 64);
    assert_eq!(chat(port, None), 401);
    assert_eq!(chat(port, Some("wrong")), 401);
    assert_eq!(chat(port, Some(lease.key())), 200);
    #[cfg(target_os = "linux")]
    {
        use mk_models::runtime::process::{LOOPBACK_HEX, listen_addrs};
        let addrs = listen_addrs(port);
        assert!(!addrs.is_empty(), "listening on {port}");
        assert!(addrs.iter().all(|a| a == LOOPBACK_HEX), "{addrs:?}");
    }
    let (key, pid) = (lease.key().to_string(), rig.slot(Role::Extraction).pid);
    drop(lease);
    assert_eq!(rig.slot(Role::Extraction).state, RuntimeState::Ready);
    // A second lease reuses the same process.
    let again = rig.sup.ensure_ready(&spec).unwrap();
    assert_eq!((again.port(), again.key()), (port, key.as_str()));
    drop(again);
    assert_eq!(rig.starts(), 1);
    // Stopped and started again: a new port and a new key.
    rig.sup.stop(Role::Extraction);
    assert_eq!(rig.slot(Role::Extraction).state, RuntimeState::Stopped);
    assert!(!port_open(port));
    let third = rig.sup.ensure_ready(&spec).unwrap();
    assert_ne!(third.port(), port);
    assert_ne!(third.key(), key);
    assert_ne!(rig.slot(Role::Extraction).pid, pid);
}

/// Callers that need the model while it loads wait for the one start.
#[test]
fn concurrent_callers_share_one_start() {
    let rig = Rig::new(Control::default(), |_| {});
    rig.control(Control {
        start_delay_ms: 500,
        ..logged(rig.dir.path())
    });
    let rig = Arc::new(rig);
    let threads: Vec<_> = (0..3)
        .map(|_| {
            let rig = rig.clone();
            std::thread::spawn(move || {
                let spec = rig.spec("x.gguf", Role::Embeddings);
                rig.sup.ensure_ready(&spec).map(|l| l.port())
            })
        })
        .collect();
    let ports: Vec<u16> = threads
        .into_iter()
        .map(|t| t.join().unwrap().unwrap())
        .collect();
    assert!(ports.windows(2).all(|w| w[0] == w[1]), "{ports:?}");
    assert_eq!(rig.starts(), 1);
}

/// US-09.5 AC2: no work for the idle time → stopped, memory freed.
#[test]
fn us_09_5_ac2_idle_stop() {
    let rig = Rig::new(Control::default(), |c| c.idle = Duration::from_millis(700));
    let lease = rig
        .sup
        .ensure_ready(&rig.spec("x.gguf", Role::Extraction))
        .unwrap();
    let port = lease.port();
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(
        rig.slot(Role::Extraction).state,
        RuntimeState::Busy,
        "never stopped while a call holds it"
    );
    drop(lease);
    assert!(rig.wait_state(Role::Extraction, RuntimeState::Stopped));
    assert!(!port_open(port));
    assert_eq!(rig.slot(Role::Extraction).pid, None);
}

/// US-09.5 AC2: the app quits → every process stopped, a start in progress too.
#[test]
fn us_09_5_ac2_quit_stops() {
    let rig = Rig::new(Control::default(), |_| {});
    let a = rig
        .sup
        .ensure_ready(&rig.spec("x.gguf", Role::Extraction))
        .unwrap();
    let b = rig
        .sup
        .ensure_ready(&rig.spec("y.gguf", Role::Embeddings))
        .unwrap();
    let ports = [a.port(), b.port()];
    assert_ne!(ports[0], ports[1], "one process per loaded model");
    drop((a, b));
    rig.sup.shut_down();
    for p in ports {
        assert!(!port_open(p), "port {p} closed");
    }
    assert!(
        rig.sup
            .status()
            .iter()
            .all(|s| s.state == RuntimeState::Stopped)
    );
    assert!(
        rig.sup
            .ensure_ready(&rig.spec("x.gguf", Role::Extraction))
            .is_err()
    );
}

#[test]
fn us_09_5_ac2_quit_while_starting() {
    let rig = Rig::new(Control::default(), |_| {});
    rig.control(Control {
        start_delay_ms: 60_000,
        ..logged(rig.dir.path())
    });
    let rig = Arc::new(rig);
    let t = {
        let rig = rig.clone();
        std::thread::spawn(move || {
            rig.sup
                .ensure_ready(&rig.spec("x.gguf", Role::Extraction))
                .err()
        })
    };
    assert!(rig.wait_state(Role::Extraction, RuntimeState::Starting));
    std::thread::sleep(Duration::from_millis(400));
    rig.sup.shut_down();
    assert!(t.join().unwrap().is_some());
    let log = std::fs::read_to_string(rig.dir.path().join("starts.log")).unwrap();
    let port: u16 = log.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(!port_open(port), "the loading process was killed");
}

/// US-09.5 AC3: a failed start is Failed with a plain reason, and stays so until Retry.
#[test]
fn us_09_5_ac3_failure_reasons() {
    let rig = Rig::new(
        Control {
            fail: Some("oom".into()),
            ..Control::default()
        },
        |_| {},
    );
    let spec = rig.spec("x.gguf", Role::Extraction);
    let e = rig.sup.ensure_ready(&spec).err().unwrap();
    assert_eq!(e.message, NO_GPU_MEMORY);
    let s = rig.slot(Role::Extraction);
    assert_eq!(s.state, RuntimeState::Failed);
    assert!(s.tail.contains("cudaMalloc failed"), "details: {}", s.tail);
    // Failed → Starting only on Retry.
    assert_eq!(
        rig.sup.ensure_ready(&spec).err().unwrap().message,
        NO_GPU_MEMORY
    );

    rig.control(Control {
        fail: Some("bad_model".into()),
        ..Control::default()
    });
    rig.sup.retry();
    assert!(rig.wait_state(Role::Extraction, RuntimeState::Failed));
    assert_eq!(
        rig.slot(Role::Extraction).failure.unwrap().message,
        BAD_MODEL_FILE
    );

    rig.control(Control {
        build: Some(9000),
        ..Control::default()
    });
    rig.sup.retry();
    assert!(rig.sup.wait_for(WAIT, |s| {
        s[0].failure.as_ref().is_some_and(|f| f.code == "too_old")
    }));
    assert_eq!(
        rig.slot(Role::Extraction).failure.unwrap().message,
        "llama.cpp build 9000 found; build 10863 or newer is needed"
    );

    rig.control(Control {
        fail: Some("exit".into()),
        ..Control::default()
    });
    rig.sup.retry();
    assert!(rig.sup.wait_for(WAIT, |s| {
        s[0].failure.as_ref().is_some_and(|f| f.code == "exited")
    }));
    assert_eq!(
        rig.slot(Role::Extraction).failure.unwrap().message,
        "llama-server stopped (exit 3)"
    );

    // ADR-0015: the user fixes it; Retry checks again, no app restart.
    rig.control(Control::default());
    let info = rig.sup.retry();
    assert_eq!(info.build.unwrap().build, 10863);
    assert!(rig.wait_state(Role::Extraction, RuntimeState::Ready));
}

#[test]
fn adr_0015_not_found_then_set_in_settings() {
    let rig = Rig::new(Control::default(), |_| {});
    rig.sup
        .set_settings_path(Some(rig.dir.path().join("nothing-here")));
    let spec = rig.spec("x.gguf", Role::Extraction);
    let e = rig.sup.ensure_ready(&spec).err().unwrap();
    assert_eq!(
        (e.code.as_str(), e.message.as_str()),
        ("not_found", "llama-server not found")
    );
    let info = rig.sup.binary();
    assert_eq!(info.error.unwrap().0, "not_found");
    rig.sup.set_settings_path(Some(FAKE.into()));
    let info = rig.sup.retry();
    assert_eq!(info.path.as_deref(), Some(Path::new(FAKE)));
    assert_eq!(info.backends, vec![mk_models::runtime::Backend::Cuda]);
    assert!(rig.wait_state(Role::Extraction, RuntimeState::Ready));
}

#[test]
fn port_in_use_is_retried_once_on_a_new_port() {
    let rig = Rig::new(Control::default(), |_| {});
    rig.control(Control {
        bind_fail_once: Some(rig.dir.path().join("bound")),
        ..logged(rig.dir.path())
    });
    let lease = rig
        .sup
        .ensure_ready(&rig.spec("x.gguf", Role::Extraction))
        .unwrap();
    assert_eq!(rig.starts(), 2);
    assert_eq!(chat(lease.port(), Some(lease.key())), 200);
}

#[test]
fn health_timeout_fails_and_kills() {
    let rig = Rig::new(Control::default(), |c| {
        c.health_timeout = Duration::from_millis(800)
    });
    rig.control(Control {
        start_delay_ms: 60_000,
        ..logged(rig.dir.path())
    });
    let e = rig
        .sup
        .ensure_ready(&rig.spec("x.gguf", Role::Extraction))
        .err()
        .unwrap();
    assert_eq!(e.code, "health_timeout");
    let log = std::fs::read_to_string(rig.dir.path().join("starts.log")).unwrap();
    let port: u16 = log.split_whitespace().nth(1).unwrap().parse().unwrap();
    assert!(!port_open(port));
}

/// NFR-REL-12, INV-MM-06: a crash restarts it, at most twice; then Failed.
#[test]
fn nfr_rel_12_crash_restarts_twice_then_failed() {
    let rig = Rig::new(Control::default(), |_| {});
    rig.control(Control {
        crash_after_ms: Some(400),
        ..logged(rig.dir.path())
    });
    let spec = rig.spec("x.gguf", Role::Extraction);
    drop(rig.sup.ensure_ready(&spec).unwrap());
    assert!(rig.wait_state(Role::Extraction, RuntimeState::Failed));
    assert_eq!(rig.starts(), 3, "first start + 2 restarts");
    let s = rig.slot(Role::Extraction);
    assert_eq!(s.restarts, 2);
    let f = s.failure.unwrap();
    assert_eq!(f.code, "crashed");
    assert!(
        f.message
            .starts_with("llama-server stopped again after 2 restarts"),
        "{}",
        f.message
    );
    assert!(s.tail.contains("fake: crashing"), "{}", s.tail);
}

/// INV-MM-06: a successful call resets the count.
#[test]
fn inv_mm_06_success_resets_restarts() {
    let rig = Rig::new(Control::default(), |_| {});
    rig.control(Control {
        crash_after_ms: Some(500),
        ..logged(rig.dir.path())
    });
    let spec = rig.spec("x.gguf", Role::Extraction);
    drop(rig.sup.ensure_ready(&spec).unwrap());
    assert!(rig.sup.wait_for(WAIT, |s| s[0].restarts == 1
        && s[0].state == RuntimeState::Ready));
    // The next process works.
    rig.control(logged(rig.dir.path()));
    assert!(rig.sup.wait_for(WAIT, |s| s[0].restarts == 2
        && s[0].state == RuntimeState::Ready));
    let lease = rig.sup.ensure_ready(&spec).unwrap();
    assert_eq!(chat(lease.port(), Some(lease.key())), 200);
    lease.succeeded();
    drop(lease);
    assert_eq!(rig.slot(Role::Extraction).restarts, 0);
}

/// Model switched (domain §7.8): the old one of the role is stopped first.
#[test]
fn model_switch_stops_the_old_one() {
    let rig = Rig::new(Control::default(), |_| {});
    let a = rig
        .sup
        .ensure_ready(&rig.spec("x.gguf", Role::Extraction))
        .unwrap();
    let old = a.port();
    drop(a);
    let b = rig
        .sup
        .ensure_ready(&rig.spec("y.gguf", Role::Extraction))
        .unwrap();
    assert_ne!(b.port(), old);
    assert!(!port_open(old));
    assert_eq!(rig.sup.status().len(), 1);
    assert_eq!(rig.slot(Role::Extraction).model_id, "y");
}

/// Killed from outside (NFR-REL-12 by hand: `pkill -9 llama-server`).
#[cfg(unix)]
#[test]
fn nfr_rel_12_kill_minus_nine() {
    let rig = Rig::new(Control::default(), |_| {});
    let spec = rig.spec("x.gguf", Role::Extraction);
    drop(rig.sup.ensure_ready(&spec).unwrap());
    for round in 1..=3 {
        let pid = rig.slot(Role::Extraction).pid.expect("running");
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap();
        let want = if round < 3 {
            RuntimeState::Ready
        } else {
            RuntimeState::Failed
        };
        assert!(
            rig.sup
                .wait_for(WAIT, |s| s[0].state == want && s[0].pid != Some(pid)),
            "round {round}: {:?}",
            rig.sup.status()
        );
    }
    assert!(
        rig.slot(Role::Extraction)
            .failure
            .unwrap()
            .message
            .ends_with("(signal 9)")
    );
}
