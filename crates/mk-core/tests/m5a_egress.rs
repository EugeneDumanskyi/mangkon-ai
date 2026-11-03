//! M5a criteria 1, 5 and 6 (E09): endpoints with keys only in the secret store (US-09.6), the
//! endpoint test (US-09.6 AC2/AC3, INV-EG-08), per-project permissions (US-09.7 AC1/AC2,
//! NFR-SEC-04), egress records (US-09.8 AC2) and no key on disk (NFR-SEC-07). Fakes only: a
//! scripted resolver, an in-process OpenAI-compatible server, an in-memory keychain.

use std::net::{IpAddr, Ipv4Addr, TcpListener};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mk_core::{
    Access, ChatMessage, ChatRequest, Destination, DestinationClass, EgressConfig, EndpointInput,
    GateError, KeyChange, KeyState, Project, Purpose, SecretStoreKind, SecretString, TlsRoots,
    Workspace,
};
use mk_egress::testkit::{Behavior, FakeKeyring, FakeServer, ScriptedResolver, private_lan_ip};
use mk_egress::{SecretStore, SessionStore, TEST_EMBED_TEXT, TEST_TEXT};

/// A synthetic key that appears nowhere else.
const KEY: &str = "mk-test-key-Vq7RzT2pLx9sNw4cH8dF";
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

struct Setup {
    _tmp: tempfile::TempDir,
    root: std::path::PathBuf,
    ws: Workspace,
    project: Project,
    resolver: Arc<ScriptedResolver>,
    keyring: Arc<FakeKeyring>,
}

fn setup() -> Setup {
    let keyring = Arc::new(FakeKeyring::new());
    setup_with(keyring.clone(), keyring)
}

fn setup_with(secrets: Arc<dyn SecretStore>, keyring: Arc<FakeKeyring>) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let resolver = Arc::new(ScriptedResolver::new());
    let ws = Workspace::open_with(
        &root.join("app"),
        EgressConfig {
            resolver: resolver.clone(),
            secrets: Some(secrets),
            roots: TlsRoots::Platform,
            call_timeout: Duration::from_secs(10),
        },
    )
    .unwrap();
    let (project, _) = ws.create("Case", &root).unwrap();
    Setup {
        _tmp: tmp,
        root,
        ws,
        project,
        resolver,
        keyring,
    }
}

fn input(id: Option<&str>, url: &str, model: &str, key: KeyChange) -> EndpointInput {
    EndpointInput {
        id: id.map(str::to_string),
        name: "Box".into(),
        base_url: url.into(),
        model: model.into(),
        key,
    }
}

fn set_key() -> KeyChange {
    KeyChange::Set(SecretString::from(KEY))
}

fn hello() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("Project text: ACME-4471 paid invoice 99")],
        ..ChatRequest::default()
    }
}

/// Every file under `dir` (recursively) whose bytes contain `needle`.
fn files_containing(dir: &Path, needle: &str) -> Vec<String> {
    let mut hits = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = std::fs::read(&p)
                && bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
            {
                hits.push(p.display().to_string());
            }
        }
    }
    hits
}

fn file_count(dir: &Path) -> usize {
    let mut n = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            if e.path().is_dir() {
                stack.push(e.path());
            } else {
                n += 1;
            }
        }
    }
    n
}

fn dead_port() -> u16 {
    TcpListener::bind((LOOPBACK, 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// US-09.6 AC1, INV-EG-07: the key goes to the secret store; `app.db` keeps only the account.
#[test]
fn us_09_6_ac1_key_only_in_secret_store() {
    let s = setup();
    assert_eq!(s.ws.secret_store_status().kind, SecretStoreKind::Keyring);
    assert_eq!(s.ws.secret_store_status().notice, None);
    s.resolver.then("llm.lan.test", &["192.168.1.20"]);
    let e =
        s.ws.save_endpoint(input(None, "http://llm.lan.test:8080", "m", set_key()))
            .unwrap();
    assert_eq!(e.id.len(), 32, "128-bit hex id");
    assert_eq!(e.base_url, "http://llm.lan.test:8080/v1");
    assert_eq!(e.key, KeyState::Stored);
    let account = format!("endpoint:{}", e.id);
    assert_eq!(s.keyring.accounts(), vec![account.clone()]);
    assert_eq!(
        s.keyring.get(&account).unwrap().map(|k| {
            use mk_egress::ExposeSecret;
            k.expose_secret().to_string()
        }),
        Some(KEY.to_string())
    );
    let d = e.destination.clone().unwrap();
    assert_eq!(
        (d.class, d.encrypted),
        (DestinationClass::LocalNetwork, false)
    );
    // Shown info never carries the key.
    assert!(!format!("{e:?}").contains(KEY));

    // Keep, then remove, then set again; deleting the endpoint deletes the key.
    let e =
        s.ws.save_endpoint(input(
            Some(&e.id),
            "http://llm.lan.test:8080",
            "m2",
            KeyChange::Keep,
        ))
        .unwrap();
    assert_eq!((e.key, e.model.as_str()), (KeyState::Stored, "m2"));
    let e =
        s.ws.save_endpoint(input(
            Some(&e.id),
            "http://llm.lan.test:8080",
            "m2",
            KeyChange::Remove,
        ))
        .unwrap();
    assert_eq!(e.key, KeyState::None);
    assert!(s.keyring.accounts().is_empty());
    let e =
        s.ws.save_endpoint(input(
            Some(&e.id),
            "http://llm.lan.test:8080",
            "m2",
            set_key(),
        ))
        .unwrap();
    assert_eq!(s.keyring.accounts().len(), 1);
    assert!(files_containing(&s.root, KEY).is_empty());
    s.ws.delete_endpoint(&e.id).unwrap();
    assert!(s.keyring.accounts().is_empty());
    assert!(s.ws.endpoints().unwrap().is_empty());

    // Bad input is refused with plain words, nothing stored.
    let err =
        s.ws.save_endpoint(input(None, "ftp://x", "m", set_key()))
            .unwrap_err();
    assert_eq!(err.code, "bad_url");
    let err =
        s.ws.save_endpoint(input(None, "http://x:1", " ", set_key()))
            .unwrap_err();
    assert_eq!(err.code, "bad_model");
    assert!(s.keyring.accounts().is_empty());
}

/// ADR-0012: with no OS keychain the key is kept for this session only, with the notice; a new
/// session (a restart) has no key, and the endpoint says so.
#[test]
fn us_09_6_ac1_session_only_store() {
    let s = setup_with(Arc::new(SessionStore::new()), Arc::new(FakeKeyring::new()));
    let status = s.ws.secret_store_status();
    assert_eq!(status.kind, SecretStoreKind::SessionOnly);
    assert!(status.notice.unwrap().contains("never written to disk"));
    s.resolver.then("127.0.0.1", &["127.0.0.1"]);
    let e =
        s.ws.save_endpoint(input(None, "http://127.0.0.1:9", "m", set_key()))
            .unwrap();
    assert_eq!(e.key, KeyState::Stored);
    let app = s.root.join("app");
    drop(s.ws);
    // A restart: same app.db, an empty session store.
    let ws = Workspace::open_with(
        &app,
        EgressConfig {
            resolver: s.resolver.clone(),
            secrets: Some(Arc::new(SessionStore::new())),
            ..EgressConfig::default()
        },
    )
    .unwrap();
    assert_eq!(ws.endpoint(&e.id).unwrap().key, KeyState::Missing);
    let err = ws.test_endpoint(&e.id).unwrap_err();
    assert_eq!(err.code, "key_missing");
    assert!(files_containing(&s.root, KEY).is_empty());
}

/// US-09.6 AC2, INV-EG-08: the test reports the model and time, or a plain error; it sends only
/// the fixed texts and records nothing in any project.
#[test]
fn us_09_6_ac2_test_reports_model_or_plain_error() {
    let s = setup();
    let ok = FakeServer::start(
        LOOPBACK,
        Behavior::OpenAi {
            model: "qwen-test".into(),
            structured: true,
            embeddings: true,
            key: Some(KEY.into()),
        },
    );
    let url = |port: u16| format!("http://127.0.0.1:{port}");
    let e =
        s.ws.save_endpoint(input(None, &url(ok.port()), "qwen-test", set_key()))
            .unwrap();
    let r = s.ws.test_endpoint(&e.id).unwrap();
    assert!(r.ok(), "{}", r.summary());
    let chat = r.chat.as_ref().unwrap();
    assert_eq!(chat.reported_model.as_deref(), Some("qwen-test"));
    assert!(r.summary().starts_with("Connected: qwen-test answered in "));
    assert_eq!(
        r.embeddings.as_ref().unwrap().as_ref().unwrap().dimensions,
        3
    );
    assert_eq!(r.class, Some(DestinationClass::ThisMachine));
    // The test sent the key, and only the fixed texts.
    let seen = ok.seen();
    assert_eq!(seen.len(), 3);
    assert!(
        seen.iter()
            .all(|q| q.header("authorization") == Some(&format!("Bearer {KEY}")))
    );
    assert!(seen[..2].iter().all(|q| q.body.contains(TEST_TEXT)));
    assert!(seen[2].body.contains(TEST_EMBED_TEXT));
    let last = s.ws.endpoint(&e.id).unwrap().last_test.unwrap();
    assert!(last.ok);
    assert_eq!(last.reported_model.as_deref(), Some("qwen-test"));
    assert!(last.response_ms.is_some());
    // Not project egress.
    assert!(s.project.egress_records().unwrap().is_empty());

    let cases: Vec<(String, &str, KeyChange, &str, String)> = vec![
        (
            url(ok.port()),
            "qwen-test",
            KeyChange::Set(SecretString::from("wrong")),
            "key_refused",
            "The endpoint refused the API key".into(),
        ),
        (
            url(ok.port()),
            "other-model",
            set_key(),
            "model_not_found",
            "Model 'other-model' not found".into(),
        ),
        (
            url(dead_port()),
            "m",
            KeyChange::Remove,
            "cannot_reach",
            "Cannot reach 127.0.0.1:".into(),
        ),
        (
            "http://nowhere.test:8080".into(),
            "m",
            KeyChange::Remove,
            "host_not_found",
            "Cannot find host nowhere.test".into(),
        ),
    ];
    for (u, model, key, code, words) in cases {
        let e =
            s.ws.save_endpoint(input(Some(&e.id), &u, model, key))
                .unwrap();
        let r = s.ws.test_endpoint(&e.id).unwrap();
        let err = r.chat.as_ref().unwrap_err();
        assert_eq!(err.code.as_str(), code, "{u}: {}", err.message);
        assert!(err.message.starts_with(&words), "{u}: {}", err.message);
        assert!(!err.message.contains(KEY));
        let last = s.ws.endpoint(&e.id).unwrap().last_test.unwrap();
        assert!(!last.ok);
        assert_eq!(last.message, err.message);
    }
    let not_api = FakeServer::start(
        LOOPBACK,
        Behavior::Fixed {
            status: 404,
            body: "<html>Not here</html>".into(),
        },
    );
    let e =
        s.ws.save_endpoint(input(
            Some(&e.id),
            &url(not_api.port()),
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    let r = s.ws.test_endpoint(&e.id).unwrap();
    let err = r.chat.unwrap_err();
    assert_eq!(
        (err.code.as_str(), err.message.as_str()),
        ("not_openai", "No OpenAI-compatible API at this URL")
    );
    assert!(s.project.egress_records().unwrap().is_empty());
}

/// US-09.6 AC3: an endpoint without structured output gets the flag and the warning; a new
/// model forgets the flag until the next test.
#[test]
fn us_09_6_ac3_structured_output_flag() {
    let s = setup();
    let plain = FakeServer::start(
        LOOPBACK,
        Behavior::OpenAi {
            model: "m".into(),
            structured: false,
            embeddings: false,
            key: None,
        },
    );
    let e =
        s.ws.save_endpoint(input(
            None,
            &format!("http://127.0.0.1:{}", plain.port()),
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    assert_eq!(e.structured_output, None);
    let r = s.ws.test_endpoint(&e.id).unwrap();
    assert!(r.ok());
    assert_eq!(r.structured_output, Some(false));
    assert!(
        r.summary()
            .ends_with("Structured output not supported — results may be less reliable"),
        "{}",
        r.summary()
    );
    // Embeddings are reported, not required.
    assert!(r.embeddings.unwrap().is_err());
    assert_eq!(s.ws.endpoint(&e.id).unwrap().structured_output, Some(false));

    let full = FakeServer::start(LOOPBACK, Behavior::openai("m"));
    let e =
        s.ws.save_endpoint(input(
            Some(&e.id),
            &format!("http://127.0.0.1:{}", full.port()),
            "m",
            KeyChange::Keep,
        ))
        .unwrap();
    assert_eq!(e.structured_output, None, "a new URL forgets the flag");
    let r = s.ws.test_endpoint(&e.id).unwrap();
    assert_eq!(r.structured_output, Some(true));
    assert!(!r.summary().contains("Structured output not supported"));
    assert_eq!(s.ws.endpoint(&e.id).unwrap().structured_output, Some(true));
}

/// US-09.7 AC1, NFR-SEC-04: a new project allows no non-local endpoint. The gate refuses
/// before any request, and nothing is recorded. A this-machine endpoint needs no permission.
#[test]
fn us_09_7_ac1_new_project_allows_no_non_local() {
    let s = setup();
    let server = FakeServer::start(LOOPBACK, Behavior::openai("m"));
    // The names point to addresses on the local network and the internet; the server would
    // answer if anything were sent to it.
    s.resolver.then("llm.lan.test", &["192.168.1.20"]);
    s.resolver.then("api.cloud.test", &["93.184.215.14"]);
    let lan =
        s.ws.save_endpoint(input(
            None,
            "http://llm.lan.test:8080",
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    let cloud =
        s.ws.save_endpoint(input(None, "https://api.cloud.test", "m", set_key()))
            .unwrap();
    let local =
        s.ws.save_endpoint(input(
            None,
            &format!("http://127.0.0.1:{}", server.port()),
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    let view = s.ws.project_endpoints(&s.project).unwrap();
    let access = |id: &str| view.iter().find(|p| p.id == id).unwrap().access.clone();
    assert_eq!(access(&lan.id), Access::NotAllowed);
    assert_eq!(access(&cloud.id), Access::NotAllowed);
    assert_eq!(access(&local.id), Access::ThisMachine);
    for (id, class) in [
        (&lan.id, DestinationClass::LocalNetwork),
        (&cloud.id, DestinationClass::Internet),
    ] {
        for purpose in [
            Purpose::Extraction,
            Purpose::Embeddings,
            Purpose::SearchQuery,
        ] {
            let err =
                s.ws.model_client(&s.project, Destination::Endpoint(id.clone()), purpose)
                    .err()
                    .unwrap();
            assert!(err.is_blocked());
            assert!(
                matches!(&err, GateError::NotAllowed { class: c, .. } if *c == class),
                "{err:?}"
            );
        }
    }
    assert_eq!(server.count(), 0);
    assert!(s.project.egress_records().unwrap().is_empty());
    assert!(s.project.store().endpoint_permissions().unwrap().is_empty());
    // The this-machine endpoint works with no permission.
    let client =
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(local.id.clone()),
            Purpose::Extraction,
        )
        .unwrap();
    client.chat(&hello()).unwrap();
    assert_eq!(server.count(), 1);
}

#[test]
fn nfr_sec_04_new_project_allows_no_non_local_endpoint() {
    // Every non-local endpoint of the app, across a fresh project: none is allowed.
    let s = setup();
    s.resolver.then("a.test", &["10.1.2.3"]);
    s.resolver.then("b.test", &["fd00::5"]);
    s.resolver.then("c.test", &["100.64.0.9"]);
    s.resolver.then("d.test", &["192.168.0.4", "8.8.8.8"]);
    for h in ["a.test", "b.test", "c.test", "d.test"] {
        s.ws.save_endpoint(input(
            None,
            &format!("http://{h}:1"),
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    }
    let (fresh, _) = s.ws.create("Fresh", &s.root).unwrap();
    let view = s.ws.project_endpoints(&fresh).unwrap();
    assert_eq!(view.len(), 4);
    assert!(
        view.iter().all(|p| p.access == Access::NotAllowed),
        "{view:?}"
    );
    for p in &view {
        assert!(
            s.ws.model_client(
                &fresh,
                Destination::Endpoint(p.id.clone()),
                Purpose::Extraction
            )
            .is_err()
        );
    }
    assert!(fresh.store().endpoint_permissions().unwrap().is_empty());
    assert!(fresh.egress_records().unwrap().is_empty());
}

/// US-09.7 AC2, INV-EG-02: allowing shows the exact host and class first, and stores the
/// permission only for that host and class; a move afterwards needs a new confirmation.
#[test]
fn us_09_7_ac2_confirm_needs_host_and_class() {
    let s = setup();
    s.resolver.then("llm.lan.test", &["192.168.1.20"]);
    let e =
        s.ws.save_endpoint(input(
            None,
            "http://llm.lan.test:8080",
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    let prompt = s.ws.prepare_allow(&e.id).unwrap();
    assert!(prompt.needs_confirmation);
    let d = &prompt.destination;
    assert_eq!(
        (d.host.as_str(), d.port, d.class, d.origin.as_str()),
        (
            "llm.lan.test",
            8080,
            DestinationClass::LocalNetwork,
            "http://llm.lan.test:8080"
        )
    );
    assert_eq!(d.addresses, vec!["192.168.1.20".parse::<IpAddr>().unwrap()]);
    // Preparing stores nothing.
    assert!(s.project.store().endpoint_permissions().unwrap().is_empty());

    // A class or origin other than the one shown is refused.
    let err =
        s.ws.confirm_allow(&s.project, &e.id, &d.origin, DestinationClass::Internet)
            .unwrap_err();
    assert_eq!(err.code, "destination_changed");
    let err =
        s.ws.confirm_allow(
            &s.project,
            &e.id,
            "http://other.test:8080",
            DestinationClass::LocalNetwork,
        )
        .unwrap_err();
    assert_eq!(err.code, "destination_changed");
    assert!(s.project.store().endpoint_permissions().unwrap().is_empty());

    s.ws.confirm_allow(&s.project, &e.id, &d.origin, d.class)
        .unwrap();
    let view = s.ws.project_endpoints(&s.project).unwrap();
    match &view[0].access {
        Access::Allowed {
            class,
            origin,
            confirmed_at,
        } => {
            assert_eq!(*class, DestinationClass::LocalNetwork);
            assert_eq!(origin, "http://llm.lan.test:8080");
            assert!(confirmed_at.ends_with('Z'), "{confirmed_at}");
        }
        a => panic!("{a:?}"),
    }
    // Other projects are not affected (INV-EG-01).
    let (other, _) = s.ws.create("Other", &s.root).unwrap();
    assert_eq!(
        s.ws.project_endpoints(&other).unwrap()[0].access,
        Access::NotAllowed
    );

    // The host moves to the internet: shown as needing reconfirmation, and the gate agrees.
    // (The script repeats its last answer; queue the new one and use up the old.)
    s.resolver.then("llm.lan.test", &["93.184.215.14"]);
    mk_egress::Resolve::resolve(s.resolver.as_ref(), "llm.lan.test", 8080).unwrap();
    let view = s.ws.project_endpoints(&s.project).unwrap();
    assert!(
        matches!(view[0].access, Access::NeedsReconfirmation { .. }),
        "{:?}",
        view[0].access
    );
    let err =
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(e.id.clone()),
            Purpose::Extraction,
        )
        .err()
        .unwrap();
    assert!(
        matches!(err, GateError::NeedsReconfirmation { .. }),
        "{err:?}"
    );
    let stored = s
        .project
        .store()
        .endpoint_permission(&e.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.state, "needs_reconfirmation");
    assert!(s.project.egress_records().unwrap().is_empty());
    // The old class cannot be confirmed again; the new one can.
    let err =
        s.ws.confirm_allow(&s.project, &e.id, &d.origin, DestinationClass::LocalNetwork)
            .unwrap_err();
    assert_eq!(err.code, "destination_changed");
    s.ws.confirm_allow(&s.project, &e.id, &d.origin, DestinationClass::Internet)
        .unwrap();
    assert!(matches!(
        s.ws.project_endpoints(&s.project).unwrap()[0].access,
        Access::Allowed {
            class: DestinationClass::Internet,
            ..
        }
    ));
    // Editing the endpoint to another host needs a new confirmation too.
    s.resolver.then("llm2.lan.test", &["93.184.215.14"]);
    s.ws.save_endpoint(input(
        Some(&e.id),
        "http://llm2.lan.test:8080",
        "m",
        KeyChange::Keep,
    ))
    .unwrap();
    let err =
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(e.id.clone()),
            Purpose::Extraction,
        )
        .err()
        .unwrap();
    assert!(
        matches!(err, GateError::NeedsReconfirmation { .. }),
        "{err:?}"
    );
    // Revoke: back to Not allowed.
    s.ws.revoke_endpoint(&s.project, &e.id).unwrap();
    assert_eq!(
        s.ws.project_endpoints(&s.project).unwrap()[0].access,
        Access::NotAllowed
    );
}

/// US-09.8 AC2, INV-EG-05: each destination that got project data is listed with its class,
/// purpose and first and last dates, written before the send.
#[test]
fn us_09_8_ac2_records_with_dates() {
    let s = setup();
    let local = FakeServer::start(LOOPBACK, Behavior::openai("m"));
    let e =
        s.ws.save_endpoint(input(
            None,
            &format!("http://127.0.0.1:{}", local.port()),
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    let client =
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(e.id.clone()),
            Purpose::Extraction,
        )
        .unwrap();
    client.chat(&hello()).unwrap();
    client.chat(&hello()).unwrap();
    let emb =
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(e.id.clone()),
            Purpose::Embeddings,
        )
        .unwrap();
    emb.embeddings(&["a".to_string(), "b".to_string()]).unwrap();
    // A call that fails after the record still leaves the record: it was written first.
    let dead =
        s.ws.save_endpoint(input(
            None,
            &format!("http://localhost:{}", dead_port()),
            "m",
            KeyChange::Remove,
        ))
        .unwrap();
    s.resolver.then("localhost", &["127.0.0.1"]);
    let c =
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(dead.id.clone()),
            Purpose::SearchQuery,
        )
        .unwrap();
    assert_eq!(c.chat(&hello()).unwrap_err().code(), "cannot_reach");

    let mut lan_host = None;
    if let Some(ip) = private_lan_ip() {
        let lan = FakeServer::start(ip, Behavior::openai("m"));
        s.resolver.then_ips("box.lan.test", vec![ip]);
        let e =
            s.ws.save_endpoint(input(
                None,
                &format!("http://box.lan.test:{}", lan.port()),
                "m",
                KeyChange::Remove,
            ))
            .unwrap();
        let p = s.ws.prepare_allow(&e.id).unwrap();
        s.ws.confirm_allow(
            &s.project,
            &e.id,
            &p.destination.origin,
            p.destination.class,
        )
        .unwrap();
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(e.id.clone()),
            Purpose::Extraction,
        )
        .unwrap()
        .chat(&hello())
        .unwrap();
        assert_eq!(lan.count(), 1);
        lan_host = Some("box.lan.test");
    } else {
        eprintln!("no private LAN address here: the local-network record is not checked");
    }

    let records = s.project.egress_records().unwrap();
    let find = |host: &str, purpose: Purpose| {
        records
            .iter()
            .find(|r| r.host == host && r.purpose == purpose)
            .unwrap_or_else(|| panic!("{host} {purpose:?} in {records:?}"))
    };
    let r = find("127.0.0.1", Purpose::Extraction);
    assert_eq!(r.class, DestinationClass::ThisMachine);
    assert!(r.first_sent_at <= r.last_sent_at);
    assert!(r.first_sent_at.ends_with('Z'));
    assert_eq!(
        find("127.0.0.1", Purpose::Embeddings).class,
        DestinationClass::ThisMachine
    );
    assert_eq!(
        find("localhost", Purpose::SearchQuery).class,
        DestinationClass::ThisMachine
    );
    if let Some(h) = lan_host {
        assert_eq!(
            find(h, Purpose::Extraction).class,
            DestinationClass::LocalNetwork
        );
    }
    assert_eq!(records.len(), 3 + usize::from(lan_host.is_some()));
    // Records stay after a rebuild of the derived tables.
    s.project.store().rebuild_all().unwrap();
    assert_eq!(s.project.egress_records().unwrap(), records);
}

/// NFR-SEC-07: a known key goes through add, test and a gate call; no file under the app data
/// or the project (WAL included) holds it, nor does anything the app shows.
#[test]
fn nfr_sec_07_test_key_never_on_disk() {
    let s = setup();
    let server = FakeServer::start(
        LOOPBACK,
        Behavior::OpenAi {
            model: "m".into(),
            structured: true,
            embeddings: true,
            key: Some(KEY.into()),
        },
    );
    let e =
        s.ws.save_endpoint(input(
            None,
            &format!("http://127.0.0.1:{}", server.port()),
            "m",
            set_key(),
        ))
        .unwrap();
    let report = s.ws.test_endpoint(&e.id).unwrap();
    assert!(report.ok(), "{}", report.summary());
    let client =
        s.ws.model_client(
            &s.project,
            Destination::Endpoint(e.id.clone()),
            Purpose::Extraction,
        )
        .unwrap();
    client.chat(&hello()).unwrap();
    client.embeddings(&["x".to_string()]).unwrap();
    // The key was sent each time (so it went through the code paths under test).
    assert_eq!(server.count(), 5);
    assert!(
        server
            .seen()
            .iter()
            .all(|q| q.header("authorization") == Some(&format!("Bearer {KEY}")))
    );
    // A refused key: the error words do not echo it.
    let wrong = FakeServer::start(
        LOOPBACK,
        Behavior::Fixed {
            status: 500,
            body: format!("bad key {KEY}"),
        },
    );
    let e2 =
        s.ws.save_endpoint(input(
            None,
            &format!("http://127.0.0.1:{}", wrong.port()),
            "m",
            set_key(),
        ))
        .unwrap();
    let r2 = s.ws.test_endpoint(&e2.id).unwrap();
    let shown = format!(
        "{report:?}{r2:?}{:?}{:?}{:?}",
        s.ws.endpoints().unwrap(),
        s.ws.project_endpoints(&s.project).unwrap(),
        s.project.egress_records().unwrap()
    );
    assert!(!shown.contains(KEY), "{shown}");
    assert!(!r2.summary().contains(KEY), "{}", r2.summary());

    // Every file while the databases are open (their -wal and -shm files exist then), and
    // again after closing.
    let wal = |dir: &Path| {
        std::fs::read_dir(dir)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with("-wal"))
    };
    assert!(wal(&s.root.join("app")) && wal(&s.root.join("Case")));
    let open_hits = files_containing(&s.root, KEY);
    assert!(open_hits.is_empty(), "{open_hits:?}");
    drop(client);
    let Setup {
        _tmp: tmp,
        root,
        ws,
        project,
        ..
    } = s;
    drop(project);
    drop(ws);
    let closed_hits = files_containing(&root, KEY);
    assert!(closed_hits.is_empty(), "{closed_hits:?}");
    assert!(file_count(&root) >= 2);
    drop(tmp);
}

/// NFR-SEC-07 and ADR-0012 with the real OS secret store (not in CI: it writes to the user's
/// keychain, and removes the entry again). Run once as is, and once with
/// `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent MK_EXPECT_SESSION_ONLY=1` for a machine
/// without a Secret Service:
///
/// ```sh
/// cargo test -p mk-core --test m5a_egress nfr_sec_07_real_keychain -- --ignored --nocapture
/// ```
#[test]
#[ignore = "uses the OS keychain"]
fn nfr_sec_07_real_keychain() {
    let session_only = std::env::var_os("MK_EXPECT_SESSION_ONLY").is_some();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let ws = Workspace::open_with(&root.join("app"), EgressConfig::default()).unwrap();
    let status = ws.secret_store_status();
    eprintln!("secret store: {:?}", status.kind);
    if session_only {
        assert_eq!(status.kind, SecretStoreKind::SessionOnly);
        assert!(status.notice.is_some());
    } else {
        assert_eq!(status.kind, SecretStoreKind::Keyring);
    }
    let (project, _) = ws.create("Case", &root).unwrap();
    let server = FakeServer::start(
        LOOPBACK,
        Behavior::OpenAi {
            model: "m".into(),
            structured: true,
            embeddings: true,
            key: Some(KEY.into()),
        },
    );
    let e = ws
        .save_endpoint(input(
            None,
            &format!("http://127.0.0.1:{}", server.port()),
            "m",
            set_key(),
        ))
        .unwrap();
    eprintln!("account: endpoint:{}", e.id);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_eq!(e.key, KeyState::Stored);
        assert!(ws.test_endpoint(&e.id).unwrap().ok());
        ws.model_client(
            &project,
            Destination::Endpoint(e.id.clone()),
            Purpose::Extraction,
        )
        .unwrap()
        .chat(&hello())
        .unwrap();
        assert!(
            server
                .seen()
                .iter()
                .all(|q| q.header("authorization") == Some(&format!("Bearer {KEY}")))
        );
        let hits = files_containing(&root, KEY);
        assert!(hits.is_empty(), "{hits:?}");
        // A new session: the keychain still has the key; a session store does not.
        let again = Workspace::open_with(&root.join("app"), EgressConfig::default()).unwrap();
        let state = again.endpoint(&e.id).unwrap().key;
        assert_eq!(
            state,
            if session_only {
                KeyState::Missing
            } else {
                KeyState::Stored
            }
        );
    }));
    ws.delete_endpoint(&e.id).unwrap();
    if !session_only {
        // The entry is gone from the keychain too.
        let keychain = mk_egress::KeyringStore::probe(mk_egress::SERVICE).unwrap();
        assert!(
            keychain
                .get(&format!("endpoint:{}", e.id))
                .unwrap()
                .is_none()
        );
    }
    drop(project);
    drop(ws);
    assert!(files_containing(&root, KEY).is_empty());
    if let Err(p) = result {
        std::panic::resume_unwind(p);
    }
}
