//! The committed JSON Schemas (`schema/`) match the Rust types. US-14.3 AC1: documented format.
//! After a model change, run with `MK_BLESS=1` to rewrite them.

use std::path::PathBuf;

#[test]
fn us_14_3_ac1_schemas_up_to_date() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schema");
    let (gt, pages, td3) = mk_bench::schemas();
    let bless = std::env::var_os("MK_BLESS").is_some();
    for (name, want) in [
        ("ground-truth.schema.json", gt),
        ("page-text.schema.json", pages),
        ("td3-expected.schema.json", td3),
    ] {
        let path = dir.join(name);
        if bless {
            std::fs::write(&path, &want).expect("write schema");
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            have == want,
            "schema/{name} is out of date; run `MK_BLESS=1 cargo test -p mk-bench`"
        );
    }
}
