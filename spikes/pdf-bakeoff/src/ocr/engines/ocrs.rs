//! Candidate A: `ocrs` (pure Rust, rten inference). Two models: word detection and line
//! recognition. Word boxes come from the recognised characters; there is no confidence.

use ocrs::{ImageSource, OcrEngineParams, TextItem};
use rten::Model;

use crate::engine::GrayImage;
use crate::fetch::ocr_dir;
use crate::ocr::engine::{OcrEngine, OcrError, OcrErrorKind, OcrLine, OcrWord};

pub struct Ocrs {
    engine: ocrs::OcrEngine,
}

impl Ocrs {
    pub fn new() -> Result<Self, String> {
        let dir = ocr_dir("ocrs");
        let load = |f: &str| {
            Model::load_file(dir.join(f))
                .map_err(|e| format!("{f}: {e} (run `pdf-bakeoff fetch-ocr`)"))
        };
        let engine = ocrs::OcrEngine::new(OcrEngineParams {
            detection_model: Some(load("text-detection.rten")?),
            recognition_model: Some(load("text-recognition.rten")?),
            ..Default::default()
        })
        .map_err(|e| e.to_string())?;
        Ok(Self { engine })
    }
}

impl OcrEngine for Ocrs {
    fn recognize(&self, image: &GrayImage, _langs: &[&str]) -> Result<Vec<OcrLine>, OcrError> {
        let other = |e: &dyn std::fmt::Display| OcrError::new(OcrErrorKind::Other, e.to_string());
        let src = ImageSource::from_bytes(&image.pixels, (image.width, image.height))
            .map_err(|e| OcrError::new(OcrErrorKind::Unsupported, e.to_string()))?;
        let input = self.engine.prepare_input(src).map_err(|e| other(&e))?;
        let words = self.engine.detect_words(&input).map_err(|e| other(&e))?;
        let lines = self.engine.find_text_lines(&input, &words);
        let text = self
            .engine
            .recognize_text(&input, &lines)
            .map_err(|e| other(&e))?;
        Ok(text
            .into_iter()
            .flatten()
            .map(|line| OcrLine {
                words: line
                    .words()
                    .filter_map(|w| {
                        let r = w.bounding_rect();
                        Some(OcrWord {
                            text: w.to_string(),
                            rect: mk_kernel::Rect::new(
                                r.left() as f32,
                                r.top() as f32,
                                r.right() as f32,
                                r.bottom() as f32,
                            )?,
                            confidence: None,
                        })
                    })
                    .collect(),
            })
            .collect())
    }
}
