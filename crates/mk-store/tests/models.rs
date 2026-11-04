//! `app.db` v3 (M5b): model downloads, verified files, app settings.

use mk_store::{APP_MIGRATIONS, AppStore};

#[test]
fn domain_7_7_app_quit_leaves_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    {
        let app = AppStore::open(dir.path()).unwrap();
        app.set_model_download("a", "downloading", 10, 100, None)
            .unwrap();
        app.set_model_download("b", "verifying", 100, 100, None)
            .unwrap();
        app.set_model_download("c", "paused", 5, 100, None).unwrap();
        app.set_model_download("d", "installed", 100, 100, None)
            .unwrap();
        app.set_model_bytes("a", 42).unwrap();
    }
    let app = AppStore::open(dir.path()).unwrap();
    let rows: Vec<(String, String, i64)> = app
        .model_downloads()
        .unwrap()
        .into_iter()
        .map(|r| (r.model_id, r.state, r.bytes_done))
        .collect();
    assert_eq!(
        rows,
        vec![
            ("a".into(), "interrupted".into(), 42),
            ("b".into(), "interrupted".into(), 100),
            ("c".into(), "paused".into(), 5),
            ("d".into(), "installed".into(), 100),
        ]
    );
}

#[test]
fn available_forgets_the_row_and_verified_files() {
    let dir = tempfile::tempdir().unwrap();
    let app = AppStore::open(dir.path()).unwrap();
    app.set_model_download("m", "installed", 9, 9, None)
        .unwrap();
    app.model_file_verified("m", "x.gguf", 9).unwrap();
    assert_eq!(
        app.model_files_verified("m").unwrap(),
        vec![("x.gguf".to_string(), 9)]
    );
    app.set_model_download("m", "interrupted", 3, 9, Some(("network", "lost")))
        .unwrap();
    let row = &app.model_downloads().unwrap()[0];
    assert_eq!(row.error_code.as_deref(), Some("network"));
    app.set_model_download("m", "available", 0, 9, None)
        .unwrap();
    assert!(app.model_downloads().unwrap().is_empty());
    assert!(app.model_files_verified("m").unwrap().is_empty());
    // A state outside §7.7 is refused by the table.
    assert!(app.set_model_download("m", "nonsense", 0, 9, None).is_err());
}

#[test]
fn settings_and_upgrade_from_v2() {
    let dir = tempfile::tempdir().unwrap();
    AppStore::open_with(dir.path(), &APP_MIGRATIONS[..2]).unwrap();
    let app = AppStore::open(dir.path()).unwrap();
    assert!(dir.path().join("backups").is_dir(), "v2 is backed up first");
    assert_eq!(app.setting("local_models_override").unwrap(), None);
    app.set_setting("local_models_override", Some("1")).unwrap();
    assert_eq!(
        app.setting("local_models_override").unwrap().as_deref(),
        Some("1")
    );
    app.set_setting("local_models_override", None).unwrap();
    assert_eq!(app.setting("local_models_override").unwrap(), None);
}
