//! Candidate B: Tesseract 5 through `tesseract-rs`, whose build script compiles Tesseract and
//! Leptonica from source (cmake, C++) and links them statically. LSTM engine, `tessdata_fast`
//! English. Words, boxes and confidences come from Tesseract's TSV output.

use std::collections::BTreeMap;

use tesseract_rs::TesseractAPI;

use crate::engine::GrayImage;
use crate::fetch::ocr_dir;
use crate::ocr::engine::{LANGS, OcrEngine, OcrError, OcrErrorKind, OcrLine, OcrWord};

/// Tesseract has no resolution for raw pixels; tell it the corpus resolution. ASSUMPTION.
const SOURCE_DPI: i32 = 300;

pub struct Tesseract {
    api: TesseractAPI,
    langs: String,
}

impl Tesseract {
    pub fn new() -> Result<Self, String> {
        let api = TesseractAPI::new();
        let langs = LANGS.join("+");
        api.init(ocr_dir("tesseract"), &langs)
            .map_err(|e| format!("tesseract init: {e} (run `pdf-bakeoff fetch-ocr`)"))?;
        Ok(Self { api, langs })
    }
}

impl OcrEngine for Tesseract {
    fn recognize(&self, image: &GrayImage, langs: &[&str]) -> Result<Vec<OcrLine>, OcrError> {
        if langs.join("+") != self.langs {
            return Err(OcrError::new(
                OcrErrorKind::Model,
                format!("loaded {}, asked for {langs:?}", self.langs),
            ));
        }
        let other =
            |e: tesseract_rs::TesseractError| OcrError::new(OcrErrorKind::Other, e.to_string());
        let (w, h) = (image.width as i32, image.height as i32);
        self.api
            .set_image(&image.pixels, w, h, 1, w)
            .map_err(other)?;
        self.api.set_source_resolution(SOURCE_DPI).map_err(other)?;
        self.api.recognize().map_err(other)?;
        let tsv = self.api.get_tsv_text(0).map_err(other)?;
        Ok(parse_tsv(&tsv))
    }
}

/// Tesseract TSV → lines of words. Columns: level, page, block, par, line, word, left, top,
/// width, height, conf (0–100, −1 for non-words), text. Level 5 rows are words.
pub fn parse_tsv(tsv: &str) -> Vec<OcrLine> {
    let mut lines: BTreeMap<(u32, u32, u32, u32), OcrLine> = BTreeMap::new();
    for row in tsv.lines() {
        let c: Vec<&str> = row.split('\t').collect();
        if c.len() < 12 || c[0] != "5" {
            continue;
        }
        let n = |i: usize| c[i].parse::<f32>().unwrap_or(0.0);
        let key = (n(1) as u32, n(2) as u32, n(3) as u32, n(4) as u32);
        let text = c[11..].join("\t");
        if text.trim().is_empty() {
            continue;
        }
        let Some(rect) = mk_kernel::Rect::new(n(6), n(7), n(6) + n(8), n(7) + n(9)) else {
            continue;
        };
        let conf = n(10);
        lines.entry(key).or_default().words.push(OcrWord {
            text,
            rect,
            confidence: (conf >= 0.0).then(|| (conf / 100.0).clamp(0.0, 1.0)),
        });
    }
    lines.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_words_grouped_by_line() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   4\t1\t1\t1\t1\t0\t10\t10\t100\t20\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t10\t10\t40\t20\t96.5\tIBAN:\n\
                   5\t1\t1\t1\t1\t2\t60\t10\t50\t20\t88\tGB86\n\
                   5\t1\t1\t1\t2\t1\t10\t40\t30\t20\t91\tTotal\n";
        let lines = parse_tsv(tsv);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].words.len(), 2);
        assert_eq!(lines[0].words[1].text, "GB86");
        assert_eq!(lines[0].words[1].confidence, Some(0.88));
        assert_eq!(lines[1].words[0].rect.x1(), 40.0);
    }
}
