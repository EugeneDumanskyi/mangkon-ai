//! Candidate C: PP-OCRv5 mobile (detection + English recognition) on ONNX Runtime, through
//! `oar-ocr`. `ort` links a prebuilt ONNX Runtime that its build script downloads. Confidence is
//! per line; word boxes are cut from the per-character boxes `oar-ocr` derives from the CTC
//! output.

use mk_kernel::Rect;
use oar_ocr::core::config::OrtSessionConfig;
use oar_ocr::oarocr::{OAROCR, OAROCRBuilder};
use oar_ocr::processors::BoundingBox;

use crate::engine::GrayImage;
use crate::fetch::ocr_dir;
use crate::ocr::engine::{OcrEngine, OcrError, OcrErrorKind, OcrLine, OcrWord};

pub struct PpOcr {
    ocr: OAROCR,
}

impl PpOcr {
    pub fn new() -> Result<Self, String> {
        let dir = ocr_dir("onnx");
        let ocr = OAROCRBuilder::new(
            dir.join("pp-ocrv5_mobile_det.onnx"),
            dir.join("en_pp-ocrv5_mobile_rec.onnx"),
            dir.join("ppocrv5_en_dict.txt"),
        )
        // One thread: the speed criterion is per core.
        .ort_session(
            OrtSessionConfig::new()
                .with_intra_threads(1)
                .with_inter_threads(1),
        )
        .return_word_box(true)
        .build()
        .map_err(|e| format!("oar-ocr: {e} (run `pdf-bakeoff fetch-ocr`)"))?;
        Ok(Self { ocr })
    }
}

fn rect(b: &BoundingBox) -> Option<mk_kernel::Rect> {
    rect_of(b.points.iter().map(|p| (p.x, p.y)))
}

impl OcrEngine for PpOcr {
    fn recognize(&self, image: &GrayImage, _langs: &[&str]) -> Result<Vec<OcrLine>, OcrError> {
        let rgb = image::RgbImage::from_fn(image.width, image.height, |x, y| {
            let v = image.get(x, y);
            image::Rgb([v, v, v])
        });
        let results = self
            .ocr
            .predict(vec![rgb])
            .map_err(|e| OcrError::new(OcrErrorKind::Other, e.to_string()))?;
        let mut lines = Vec::new();
        for region in results.iter().flat_map(|r| &r.text_regions) {
            let (Some(text), Some(line_rect)) =
                (region.text.as_deref(), rect(&region.bounding_box))
            else {
                continue;
            };
            let confidence = region.confidence.map(|c| c.clamp(0.0, 1.0));
            let chars: Vec<char> = text.chars().collect();
            let boxes: Vec<mk_kernel::Rect> = region
                .word_boxes
                .iter()
                .flatten()
                .filter_map(rect)
                .collect();
            let words = if boxes.len() == chars.len() {
                // Words are runs of non-spaces; a word's box is the union of its characters'.
                let mut words = Vec::new();
                let mut i = 0;
                while i < chars.len() {
                    if chars[i].is_whitespace() {
                        i += 1;
                        continue;
                    }
                    let start = i;
                    while i < chars.len() && !chars[i].is_whitespace() {
                        i += 1;
                    }
                    let r = boxes[start + 1..i]
                        .iter()
                        .fold(boxes[start], |u, b| u.union(b));
                    words.push(OcrWord {
                        text: chars[start..i].iter().collect(),
                        rect: r,
                        confidence,
                    });
                }
                words
            } else {
                // No usable character boxes: the shared layout splits the line box by
                // character count.
                vec![OcrWord {
                    text: text.to_string(),
                    rect: line_rect,
                    confidence,
                }]
            };
            lines.push(OcrLine { words });
        }
        Ok(lines)
    }
}

/// Box from corner points (any orientation), as a valid `Rect`.
fn rect_of(points: impl IntoIterator<Item = (f32, f32)>) -> Option<Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y) in points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    Rect::new(x0, y0, x1, y1)
}
