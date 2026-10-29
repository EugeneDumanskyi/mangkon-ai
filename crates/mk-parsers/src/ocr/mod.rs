//! The `OcrEngine` port and its Tesseract adapter (ADR-0014), the OCR layout, and the OCR
//! language files (ADR-0018).

pub mod layout;
mod tesseract;

use std::fmt;
use std::path::{Path, PathBuf};

use mk_kernel::Rect;

use crate::image::GrayImage;

pub use tesseract::TesseractOcr;

/// Why an image or page could not be OCR'd. `kind` decides the plain reason the product shows
/// (NFR-USE-05), `message` is for logs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OcrError {
    pub kind: OcrErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OcrErrorKind {
    /// The image is over the image guard; refused before decoding.
    TooLarge,
    /// Not a readable image: no known format, bad header, truncated data.
    Corrupt,
    /// A readable image the engine cannot take.
    Unsupported,
    /// A language file is missing or unreadable; `message` is the language code.
    Language,
    Other,
}

impl OcrError {
    pub fn new(kind: OcrErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

impl fmt::Display for OcrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for OcrError {}

/// One recognised word: its text, its box in pixels of the input image (top-left origin, y
/// down), and the engine's confidence in `[0, 1]` if it gives one.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub rect: Rect,
    pub confidence: Option<f32>,
}

/// One text line as the engine found it, words left to right. Line order is not trusted: the
/// shared layout orders lines itself.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OcrLine {
    pub words: Vec<OcrWord>,
}

/// An OCR engine with its language data loaded. Loaded once and kept, since workers are
/// long-lived (ADR-0007); `recognize` is called once per page image.
pub trait OcrEngine {
    /// `dpi` is the resolution of the image.
    fn recognize(&mut self, image: &GrayImage, dpi: u32) -> Result<Vec<OcrLine>, OcrError>;
}

/// The built-in OCR language (ADR-0018).
pub const ENGLISH: &str = "eng";

/// The language codes of a `+`-joined list (`eng+deu`), in order, without blanks.
pub fn languages(list: &str) -> Vec<&str> {
    list.split('+')
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect()
}

/// Whether `code` can name a language file: ASCII letters, digits and `_` (Tesseract's
/// `chi_sim`, `deu_latf`), 1 to 32 of them. Anything else could reach outside the folder.
pub fn valid_language(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 32
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The language codes with a `.traineddata` file in `tessdata`, sorted.
pub fn installed_languages(tessdata: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(tessdata)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let code = name.strip_suffix(".traineddata")?;
            valid_language(code).then(|| code.to_string())
        })
        .collect();
    out.sort();
    out
}

/// Writes the built-in English data into `tessdata` when it is missing or differs from the
/// pinned file (ADR-0018). The file is written next to its final name and renamed into place,
/// so a reader never sees half of it.
pub fn install_english(tessdata: &Path) -> std::io::Result<PathBuf> {
    let path = tessdata.join(format!("{ENGLISH}.traineddata"));
    let data = mk_tesseract_sys::ENG_TRAINEDDATA;
    let same = std::fs::metadata(&path).is_ok_and(|m| m.len() == data.len() as u64)
        && std::fs::read(&path).is_ok_and(|b| b == data);
    if !same {
        std::fs::create_dir_all(tessdata)?;
        let partial = tessdata.join(format!(
            "{ENGLISH}.traineddata.{}.partial",
            std::process::id()
        ));
        std::fs::write(&partial, data)?;
        std::fs::rename(&partial, &path)?;
    }
    Ok(path)
}

/// The first language of `list` with no file in `tessdata`, or one that is not a valid code.
pub fn missing_language<'a>(tessdata: &Path, list: &'a str) -> Option<&'a str> {
    languages(list)
        .into_iter()
        .find(|l| !valid_language(l) || !tessdata.join(format!("{l}.traineddata")).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_lists_and_codes() {
        assert_eq!(languages("eng+deu"), vec!["eng", "deu"]);
        assert_eq!(languages(" eng + "), vec!["eng"]);
        assert!(valid_language("chi_sim"));
        assert!(!valid_language("../eng"));
        assert!(!valid_language(""));
    }

    /// ADR-0018: English is written once and listed; other languages are what the user put
    /// there; a missing one is named.
    #[test]
    fn adr_0018_english_built_in_others_installed() {
        let dir = tempfile::tempdir().unwrap();
        let tessdata = dir.path().join("tessdata");
        assert_eq!(missing_language(&tessdata, "eng"), Some("eng"));
        let path = install_english(&tessdata).unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        install_english(&tessdata).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            written
        );
        std::fs::write(&path, b"damaged").unwrap();
        install_english(&tessdata).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            mk_tesseract_sys::ENG_TRAINEDDATA
        );
        std::fs::write(tessdata.join("deu.traineddata"), b"x").unwrap();
        std::fs::write(tessdata.join("notes.txt"), b"x").unwrap();
        assert_eq!(installed_languages(&tessdata), vec!["deu", "eng"]);
        assert_eq!(missing_language(&tessdata, "eng+deu"), None);
        assert_eq!(missing_language(&tessdata, "eng+fra"), Some("fra"));
    }
}
