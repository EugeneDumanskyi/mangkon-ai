//! ADR-0018: the embedded English data is the pinned file.

use sha2::{Digest, Sha256};

#[test]
fn adr_0018_embedded_english_matches_its_pin() {
    let got: String = Sha256::digest(mk_tesseract_sys::ENG_TRAINEDDATA)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert_eq!(got, mk_tesseract_sys::ENG_TRAINEDDATA_SHA256);
    assert!(mk_tesseract_sys::ENG_TRAINEDDATA.len() > 1 << 20);
}
