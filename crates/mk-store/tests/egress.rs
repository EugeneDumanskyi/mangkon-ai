//! Schema v8 (BC-11) and app.db v2 (endpoints): permissions, egress records, endpoints.

use mk_store::{AppStore, EndpointRow, EndpointTestRow, Store};

fn project(dir: &std::path::Path) -> Store {
    Store::open(&dir.join("project.db"), &dir.join("backups"))
        .unwrap()
        .0
}

/// INV-EG-01, NFR-SEC-04: a new project has no permission at all.
#[test]
fn inv_eg_01_new_project_has_no_permission() {
    let dir = tempfile::tempdir().unwrap();
    let store = project(dir.path());
    assert!(store.endpoint_permissions().unwrap().is_empty());
    assert!(store.endpoint_permission("any").unwrap().is_none());
    assert!(store.egress_records().unwrap().is_empty());
}

/// Domain §7.6: allow → needs reconfirmation → allow again → revoke.
#[test]
fn permission_life() {
    let dir = tempfile::tempdir().unwrap();
    let store = project(dir.path());
    assert!(
        store
            .allow_endpoint("e1", "http://h:1", "this_machine")
            .is_err()
    );
    store
        .allow_endpoint("e1", "http://h:1", "local_network")
        .unwrap();
    let p = store.endpoint_permission("e1").unwrap().unwrap();
    assert_eq!(
        (p.state.as_str(), p.confirmed_class.as_str(), p.reason),
        ("allowed", "local_network", None)
    );
    assert!(
        store
            .permission_needs_reconfirmation("e1", "moved")
            .unwrap()
    );
    // The first reason stays.
    assert!(
        !store
            .permission_needs_reconfirmation("e1", "again")
            .unwrap()
    );
    let p = store.endpoint_permission("e1").unwrap().unwrap();
    assert_eq!(
        (p.state.as_str(), p.reason.as_deref()),
        ("needs_reconfirmation", Some("moved"))
    );
    store
        .allow_endpoint("e1", "http://h:1", "internet")
        .unwrap();
    let p = store.endpoint_permission("e1").unwrap().unwrap();
    assert_eq!(
        (p.state.as_str(), p.confirmed_class.as_str()),
        ("allowed", "internet")
    );
    assert!(store.revoke_endpoint("e1").unwrap());
    assert!(!store.revoke_endpoint("e1").unwrap());
    assert!(store.endpoint_permission("e1").unwrap().is_none());
    // Nothing for an unknown endpoint.
    assert!(!store.permission_needs_reconfirmation("e2", "x").unwrap());
}

/// INV-EG-05, US-09.8 AC2: one row per host, class and purpose, first date kept; a rebuild of
/// the derived tables leaves the egress tables alone.
#[test]
fn inv_eg_05_records_keep_first_and_survive_rebuild() {
    let dir = tempfile::tempdir().unwrap();
    let store = project(dir.path());
    store
        .record_egress("127.0.0.1", "this_machine", "extraction")
        .unwrap();
    store
        .record_egress("api.test", "internet", "embeddings")
        .unwrap();
    store
        .record_egress("127.0.0.1", "this_machine", "extraction")
        .unwrap();
    assert!(store.record_egress("x", "nowhere", "extraction").is_err());
    let rows = store.egress_records().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.first_sent_at <= r.last_sent_at));
    store
        .allow_endpoint("e1", "https://api.test:443", "internet")
        .unwrap();
    store.rebuild_all().unwrap();
    assert_eq!(store.egress_records().unwrap(), rows);
    assert!(store.endpoint_permission("e1").unwrap().is_some());
}

fn endpoint(id: &str) -> EndpointRow {
    EndpointRow {
        id: id.into(),
        name: "Box".into(),
        base_url: "http://192.168.1.20:8080/v1".into(),
        model_name: "m".into(),
        secret_ref: Some(format!("endpoint:{id}")),
        structured_output: None,
        created_at: 1,
        updated_at: 1,
    }
}

/// US-09.6 AC1/AC3 storage: no key column, tests trimmed, the flag forgotten on a new URL.
#[test]
fn app_db_endpoints() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppStore::open(dir.path()).unwrap();
    app.insert_endpoint(&endpoint("a1")).unwrap();
    assert!(app.insert_endpoint(&endpoint("a1")).is_err());
    assert_eq!(app.endpoints().unwrap(), vec![endpoint("a1")]);
    for i in 0..25 {
        app.record_endpoint_test(
            &EndpointTestRow {
                endpoint_id: "a1".into(),
                tested_at: i,
                ok: true,
                message: format!("t{i}"),
                response_ms: Some(5),
                reported_model: Some("m".into()),
            },
            Some(i % 2 == 0),
        )
        .unwrap();
    }
    assert_eq!(
        app.last_endpoint_test("a1").unwrap().unwrap().message,
        "t24"
    );
    assert_eq!(
        app.endpoint("a1").unwrap().unwrap().structured_output,
        Some(true)
    );
    let kept: i64 = rusqlite::Connection::open(app.path())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM endpoint_test", [], |r| r.get(0))
        .unwrap();
    assert_eq!(kept, 20);
    // Same URL and model: the flag stays. New model: forgotten.
    assert!(
        app.update_endpoint("a1", "Box 2", "http://192.168.1.20:8080/v1", "m", None)
            .unwrap()
    );
    assert_eq!(
        app.endpoint("a1").unwrap().unwrap().structured_output,
        Some(true)
    );
    app.update_endpoint("a1", "Box 2", "http://192.168.1.20:8080/v1", "m2", None)
        .unwrap();
    let e = app.endpoint("a1").unwrap().unwrap();
    assert_eq!(
        (e.structured_output, e.secret_ref, e.name.as_str()),
        (None, None, "Box 2")
    );
    // No column could hold a key.
    let cols: Vec<String> = rusqlite::Connection::open(app.path())
        .unwrap()
        .prepare("SELECT name FROM pragma_table_info('endpoint')")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        !cols.iter().any(|c| c.contains("key") && c != "secret_ref"),
        "{cols:?}"
    );
    assert!(app.delete_endpoint("a1").unwrap());
    assert!(app.last_endpoint_test("a1").unwrap().is_none());
    assert!(!app.delete_endpoint("a1").unwrap());
}
