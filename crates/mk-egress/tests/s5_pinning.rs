//! Spike S5 (K8, ADR-0009 follow-ups): the client is pinned to the checked address and keeps
//! SNI and certificate checks for the host name; rebinding, multi-address hosts and a revoke
//! during a running job block or reconfirm as specified (INV-EG-03, domain §7.6).

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use mk_egress::testkit::{
    Behavior, FakeServer, MemoryEgressStore, MemoryEndpoints, ScriptedResolver, TestCert,
    private_lan_ip,
};
use mk_egress::{
    BaseUrl, ChatMessage, ChatRequest, Destination, DestinationClass, EndpointConfig, ErrorCode,
    Gate, GateError, Permission, Purpose, ReconfirmReason, SessionStore, TlsRoots,
};

const MODEL: &str = "test-model";

fn endpoint(id: &str, url: &str) -> EndpointConfig {
    EndpointConfig {
        id: id.into(),
        base_url: BaseUrl::parse(url).unwrap(),
        model: MODEL.into(),
        secret_ref: None,
    }
}

fn gate(resolver: Arc<ScriptedResolver>, endpoints: Vec<EndpointConfig>, roots: TlsRoots) -> Gate {
    Gate::new(
        resolver,
        Arc::new(MemoryEndpoints::with(endpoints)),
        Arc::new(SessionStore::new()),
        roots,
    )
    .with_timeout(Duration::from_secs(10))
}

fn hello() -> ChatRequest {
    ChatRequest {
        messages: vec![ChatMessage::user("hello")],
        ..ChatRequest::default()
    }
}

fn allowed(url: &str, class: DestinationClass) -> Permission {
    Permission::Allowed {
        origin: BaseUrl::parse(url).unwrap().origin().clone(),
        class,
        at_ms: 1,
    }
}

const LOOPBACK: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

/// S5: the request to `https://api.example.test:<port>` goes to the checked address
/// (127.0.0.1), the server sees SNI `api.example.test`, and the certificate is checked
/// against that name. `.test` never resolves in DNS, so a lookup by the client would fail.
#[test]
fn s5_pinned_tls_keeps_sni_and_cert() {
    let cert = TestCert::new(&["api.example.test"]);
    let server = FakeServer::start_tls(LOOPBACK, Behavior::openai(MODEL), &cert);
    let url = format!("https://api.example.test:{}", server.port());
    let resolver = Arc::new(ScriptedResolver::new());
    resolver.then("api.example.test", &["127.0.0.1"]);
    let g = gate(
        resolver.clone(),
        vec![endpoint("e1", &url)],
        TlsRoots::Only(vec![cert.ca_der.clone()]),
    );
    let store = MemoryEgressStore::default();
    let client = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .expect("loopback needs no permission");
    let reply = client.chat(&hello()).expect("pinned TLS call");
    assert_eq!(reply.content, "Hello");
    let seen = server.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].sni.as_deref(), Some("api.example.test"));
    assert_eq!(seen[0].path, "/v1/chat/completions");
    assert_eq!(
        seen[0].header("host"),
        Some(format!("api.example.test:{}", server.port()).as_str())
    );
    // Only the gate's resolver was asked: once for the client, once for the call.
    assert_eq!(resolver.calls(), 2);
    assert_eq!(
        store.records(),
        vec![(
            "api.example.test".into(),
            DestinationClass::ThisMachine,
            Purpose::Extraction
        )]
    );
}

/// S5: a certificate for another name is refused, in plain words.
#[test]
fn s5_pinned_tls_wrong_name_fails() {
    let cert = TestCert::new(&["other.test"]);
    let server = FakeServer::start_tls(LOOPBACK, Behavior::openai(MODEL), &cert);
    let url = format!("https://api.example.test:{}", server.port());
    let resolver = Arc::new(ScriptedResolver::new());
    resolver.then("api.example.test", &["127.0.0.1"]);
    let g = gate(
        resolver,
        vec![endpoint("e1", &url)],
        TlsRoots::Only(vec![cert.ca_der.clone()]),
    );
    let store = MemoryEgressStore::default();
    let client = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .unwrap();
    let err = client.chat(&hello()).unwrap_err();
    let GateError::Failed(e) = &err else {
        panic!("{err:?}")
    };
    assert_eq!(e.code, ErrorCode::BadCertificate, "{e:?}");
    assert_eq!(
        e.message,
        "The certificate of api.example.test is not valid"
    );
    // No request reached the HTTP layer.
    assert_eq!(server.count(), 0);
}

/// S5: a CA the client does not trust is refused as well.
#[test]
fn s5_untrusted_ca_fails() {
    let cert = TestCert::new(&["api.example.test"]);
    let other_ca = TestCert::new(&["api.example.test"]);
    let server = FakeServer::start_tls(LOOPBACK, Behavior::openai(MODEL), &cert);
    let url = format!("https://api.example.test:{}", server.port());
    let resolver = Arc::new(ScriptedResolver::new());
    resolver.then("api.example.test", &["127.0.0.1"]);
    let g = gate(
        resolver,
        vec![endpoint("e1", &url)],
        TlsRoots::Only(vec![other_ca.ca_der]),
    );
    let store = MemoryEgressStore::default();
    let client = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .unwrap();
    let err = client.chat(&hello()).unwrap_err();
    assert_eq!(err.code(), "bad_certificate", "{err:?}");
    assert_eq!(server.count(), 0);
}

/// S5: the send uses the addresses of its own check, not a second lookup. The resolver says
/// 127.0.0.1 for the check and would say an Internet address after; the request still reaches
/// the local server, and each call resolves exactly once.
#[test]
fn s5_send_uses_checked_address_not_a_second_lookup() {
    let server = FakeServer::start(LOOPBACK, Behavior::openai(MODEL));
    let url = format!("http://model.test:{}", server.port());
    let resolver = Arc::new(ScriptedResolver::new());
    // Client check, call 1 check, then the Internet answer for any later lookup.
    resolver
        .then("model.test", &["127.0.0.1"])
        .then("model.test", &["127.0.0.1"])
        .then("model.test", &["93.184.215.14"]);
    let g = gate(
        resolver.clone(),
        vec![endpoint("e1", &url)],
        TlsRoots::Platform,
    );
    let store = MemoryEgressStore::default();
    let client = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Embeddings,
        )
        .unwrap();
    client.chat(&hello()).expect("sent to the checked address");
    assert_eq!(server.count(), 1);
    assert_eq!(resolver.calls(), 2);
    // The next call sees the Internet answer and is blocked before anything is sent.
    let err = client.chat(&hello()).unwrap_err();
    assert!(
        matches!(
            err,
            GateError::NotAllowed {
                class: DestinationClass::Internet,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(server.count(), 1);
    assert_eq!(resolver.calls(), 3);
}

/// S5 (ADR-0009 follow-up: rebinding): a host confirmed on the local network that now
/// resolves to the Internet needs reconfirmation. Nothing is sent or recorded, and the stored
/// permission moves to "needs reconfirmation" (domain §7.6).
#[test]
fn s5_rebinding_lan_to_internet_needs_reconfirmation() {
    // Serve on this machine's private address when it has one, so call 1 is a real send to a
    // Local network address. Without one, call 1 is the client's check only.
    let lan = private_lan_ip();
    let server = lan.map(|ip| FakeServer::start(ip, Behavior::openai(MODEL)));
    let port = server.as_ref().map_or(8080, FakeServer::port);
    let url = format!("http://lan-model.test:{port}");
    let first = lan.unwrap_or_else(|| "192.168.1.20".parse().unwrap());
    let resolver = Arc::new(ScriptedResolver::new());
    resolver
        .then_ips("lan-model.test", vec![first])
        .then_ips("lan-model.test", vec![first])
        .then("lan-model.test", &["93.184.215.14"]);
    let g = gate(resolver, vec![endpoint("e1", &url)], TlsRoots::Platform);
    let store = MemoryEgressStore::default();
    store.set("e1", allowed(&url, DestinationClass::LocalNetwork));

    let client = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .expect("allowed on the local network");
    let mut recorded = Vec::new();
    if let Some(server) = &server {
        client
            .chat(&hello())
            .expect("call 1 goes to the local network address");
        assert_eq!(server.count(), 1);
        recorded.push((
            "lan-model.test".to_string(),
            DestinationClass::LocalNetwork,
            Purpose::Extraction,
        ));
    } else {
        eprintln!("no private address on this machine: call 1 is the client's check only");
        // Use up the second local-network answer, as call 1 would.
        g.client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .unwrap();
    }
    assert_eq!(store.records(), recorded);

    let err = client.chat(&hello()).unwrap_err();
    assert_eq!(
        err,
        GateError::NeedsReconfirmation {
            endpoint_id: "e1".into(),
            reason: ReconfirmReason::ClassChanged {
                confirmed: DestinationClass::LocalNetwork,
                now: DestinationClass::Internet,
            },
        }
    );
    assert!(err.is_blocked());
    assert_eq!(
        store.records(),
        recorded,
        "nothing recorded for the refused call"
    );
    if let Some(server) = &server {
        assert_eq!(server.count(), 1, "nothing sent for the refused call");
    }
    assert!(matches!(
        store.get("e1"),
        Permission::NeedsReconfirmation { .. }
    ));
    // It stays blocked until the user confirms again, whatever DNS says next.
    let err = client.chat(&hello()).unwrap_err();
    assert!(
        matches!(err, GateError::NeedsReconfirmation { .. }),
        "{err:?}"
    );
}

/// S5 (multi-address hosts, INV-EG-03): one private and one public address → Internet.
#[test]
fn s5_multi_address_most_exposed_wins() {
    let url = "http://multi.test:8080";
    let resolver = Arc::new(ScriptedResolver::new());
    resolver.then("multi.test", &["10.0.0.5", "8.8.8.8"]);
    let g = gate(resolver, vec![endpoint("e1", url)], TlsRoots::Platform);
    let store = MemoryEgressStore::default();

    // Not allowed: refused with the Internet class.
    let err = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .err()
        .unwrap();
    assert_eq!(
        err,
        GateError::NotAllowed {
            endpoint_id: "e1".into(),
            host: "multi.test".into(),
            class: DestinationClass::Internet,
        }
    );
    // Confirmed as Local network: the Internet address needs a new confirmation.
    store.set("e1", allowed(url, DestinationClass::LocalNetwork));
    let err = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .err()
        .unwrap();
    assert!(
        matches!(
            err,
            GateError::NeedsReconfirmation {
                reason: ReconfirmReason::ClassChanged {
                    now: DestinationClass::Internet,
                    ..
                },
                ..
            }
        ),
        "{err:?}"
    );
    // Confirmed as Internet: allowed (nothing is sent here).
    store.set("e1", allowed(url, DestinationClass::Internet));
    assert!(
        g.client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction
        )
        .is_ok()
    );
    assert!(store.records().is_empty());
}

/// S5 (revoked permission during a running job): a job thread calls the gate in a loop; after
/// the revoke the next call is Blocked and the server gets no request after it.
#[test]
fn s5_revoked_during_running_job_blocks_next_call() {
    let lan = private_lan_ip();
    let Some(lan) = lan else {
        // Without a private address the job cannot make real local-network sends; the check
        // itself is covered by the gate-level case below.
        eprintln!("no private address on this machine: gate-level check only");
        revoked_gate_level();
        return;
    };
    let server = FakeServer::start(lan, Behavior::openai(MODEL));
    let url = format!("http://{}:{}", lan, server.port());
    let resolver = Arc::new(ScriptedResolver::new());
    let g = gate(resolver, vec![endpoint("e1", &url)], TlsRoots::Platform);
    let store = MemoryEgressStore::default();
    store.set("e1", allowed(&url, DestinationClass::LocalNetwork));

    let revoked = AtomicBool::new(false);
    let sent_before_revoke = std::sync::Mutex::new(0usize);
    std::thread::scope(|s| {
        let job = s.spawn(|| {
            let client = g
                .client(
                    &store,
                    Destination::Endpoint("e1".into()),
                    Purpose::Extraction,
                )
                .expect("allowed");
            let mut ok = 0usize;
            loop {
                match client.chat(&hello()) {
                    Ok(_) => ok += 1,
                    Err(e) => return (ok, e),
                }
                if ok > 1_000 {
                    panic!("never blocked");
                }
            }
        });
        // Let the job make some calls, then revoke (the row is deleted: NotAllowed).
        while server.count() < 5 {
            std::thread::sleep(Duration::from_millis(2));
        }
        store.permissions.lock().unwrap().remove("e1");
        *sent_before_revoke.lock().unwrap() = server.count();
        revoked.store(true, Ordering::SeqCst);
        let (ok, err) = job.join().unwrap();
        assert!(matches!(err, GateError::NotAllowed { .. }), "{err:?}");
        assert!(err.is_blocked());
        // A call already past its check when the revoke landed may still finish; none after.
        let before = *sent_before_revoke.lock().unwrap();
        assert!(
            server.count() <= before + 1,
            "{} after {before}",
            server.count()
        );
        assert_eq!(server.count(), ok);
    });
    assert!(revoked.load(Ordering::SeqCst));
}

fn revoked_gate_level() {
    let url = "http://192.168.1.20:8080";
    let g = gate(
        Arc::new(ScriptedResolver::new()),
        vec![endpoint("e1", url)],
        TlsRoots::Platform,
    );
    let store = MemoryEgressStore::default();
    store.set("e1", allowed(url, DestinationClass::LocalNetwork));
    g.client(
        &store,
        Destination::Endpoint("e1".into()),
        Purpose::Extraction,
    )
    .expect("allowed");
    store.permissions.lock().unwrap().remove("e1");
    let err = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .err()
        .unwrap();
    assert!(matches!(err, GateError::NotAllowed { .. }), "{err:?}");
}

/// S5: a redirect is an error, never followed, so the request cannot leave the checked host.
#[test]
fn s5_redirect_not_followed() {
    let elsewhere = FakeServer::start(LOOPBACK, Behavior::openai(MODEL));
    let server = FakeServer::start(
        LOOPBACK,
        Behavior::Redirect(format!(
            "http://127.0.0.1:{}/v1/chat/completions",
            elsewhere.port()
        )),
    );
    let url = format!("http://127.0.0.1:{}", server.port());
    let g = gate(
        Arc::new(ScriptedResolver::new()),
        vec![endpoint("e1", &url)],
        TlsRoots::Platform,
    );
    let store = MemoryEgressStore::default();
    let client = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .unwrap();
    let err = client.chat(&hello()).unwrap_err();
    assert_eq!(err.code(), "redirect", "{err:?}");
    assert_eq!(server.count(), 1);
    assert_eq!(elsewhere.count(), 0);
}

/// The local runtime is always on 127.0.0.1 and needs no permission (INV-MM-05); its sends are
/// recorded as This machine (INV-EG-05).
#[test]
fn local_runtime_needs_no_permission_and_is_recorded() {
    let server = FakeServer::start(LOOPBACK, Behavior::openai(MODEL));
    let g = gate(
        Arc::new(ScriptedResolver::new()),
        Vec::new(),
        TlsRoots::Platform,
    );
    let store = MemoryEgressStore::default();
    let client = g
        .client(
            &store,
            Destination::LocalRuntime {
                port: server.port(),
                model: MODEL.into(),
            },
            Purpose::SearchQuery,
        )
        .unwrap();
    let e = client.embeddings(&["a".into(), "b".into()]).unwrap();
    assert_eq!(e.vectors.len(), 2);
    assert_eq!(
        store.records(),
        vec![(
            "127.0.0.1".into(),
            DestinationClass::ThisMachine,
            Purpose::SearchQuery
        )]
    );
}

/// Domain §7.6: an endpoint deleted (or not on this machine) moves an allowed permission to
/// "needs reconfirmation"; edited to another origin, the same.
#[test]
fn endpoint_missing_or_edited_needs_reconfirmation() {
    let url = "http://192.168.1.20:8080";
    let endpoints = Arc::new(MemoryEndpoints::with(vec![endpoint("e1", url)]));
    let g = Gate::new(
        Arc::new(ScriptedResolver::new()),
        endpoints.clone(),
        Arc::new(SessionStore::new()),
        TlsRoots::Platform,
    );
    let store = MemoryEgressStore::default();
    store.set("e1", allowed(url, DestinationClass::LocalNetwork));
    endpoints
        .map
        .lock()
        .unwrap()
        .insert("e1".into(), endpoint("e1", "http://192.168.1.21:8080"));
    let err = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .err()
        .unwrap();
    assert!(
        matches!(
            err,
            GateError::NeedsReconfirmation {
                reason: ReconfirmReason::EndpointEdited,
                ..
            }
        ),
        "{err:?}"
    );
    store.set("e1", allowed(url, DestinationClass::LocalNetwork));
    endpoints.map.lock().unwrap().clear();
    let err = g
        .client(
            &store,
            Destination::Endpoint("e1".into()),
            Purpose::Extraction,
        )
        .err()
        .unwrap();
    assert!(
        matches!(
            err,
            GateError::NeedsReconfirmation {
                reason: ReconfirmReason::EndpointNotFound,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(matches!(
        store.get("e1"),
        Permission::NeedsReconfirmation { .. }
    ));
}
