//! NFR-MNT-09: the catalog is a versioned JSON file with a schema, validated in CI.
//! After a type change, run with `MK_BLESS=1` to rewrite `schema/catalog.schema.json`.

use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn nfr_mnt_09_schema_up_to_date() {
    let path = crate_dir().join("schema/catalog.schema.json");
    let want = mk_models::schema();
    if std::env::var_os("MK_BLESS").is_some() {
        std::fs::write(&path, &want).expect("write schema");
    }
    let have = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        have == want,
        "schema/catalog.schema.json is out of date; run `MK_BLESS=1 cargo test -p mk-models`"
    );
}

#[test]
fn nfr_mnt_09_catalog_valid() {
    let c = mk_models::load(&crate_dir().join("catalog/catalog.json")).expect("catalog loads");
    if let Err(errors) = c.validate() {
        panic!("catalog is invalid:\n{}", errors.join("\n"));
    }
    assert!(
        c.models
            .iter()
            .any(|m| m.role == mk_models::Role::Extraction)
    );
    assert!(
        c.models
            .iter()
            .any(|m| m.role == mk_models::Role::Embeddings)
    );
    assert!(!c.tiers.is_empty());
}
