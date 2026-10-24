//! ADR-0014 build check: the statically linked Tesseract reads one synthetic line.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use mk_parsers::TesseractOcr;

static NOTO_SANS: &[u8] = include_bytes!("../../../tools/corpus-gen/fonts/NotoSans-Regular.ttf");

/// Black text on white, 8-bit grey, drawn at `px` pixels per em.
fn render_line(text: &str, px: f32) -> (Vec<u8>, u32, u32) {
    let font = FontRef::try_from_slice(NOTO_SANS).unwrap();
    let scaled = font.as_scaled(PxScale::from(px));
    let margin = px;
    let advance: f32 = text
        .chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum();
    let (w, h) = (
        (advance + 2.0 * margin) as u32,
        (scaled.height() + 2.0 * margin) as u32,
    );
    let mut pixels = vec![255u8; (w * h) as usize];
    let mut x = margin;
    let baseline = margin + scaled.ascent();
    for c in text.chars() {
        let glyph = scaled.scaled_glyph(c).clone();
        let id = glyph.id;
        let glyph = ab_glyph::Glyph {
            position: point(x, baseline),
            ..glyph
        };
        if let Some(outline) = font.outline_glyph(glyph) {
            let b = outline.px_bounds();
            outline.draw(|gx, gy, cov| {
                let (px_x, px_y) = (b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64);
                if (0..w as i64).contains(&px_x) && (0..h as i64).contains(&px_y) {
                    let p = &mut pixels[(px_y as u32 * w + px_x as u32) as usize];
                    *p = (*p as f32 * (1.0 - cov.clamp(0.0, 1.0))) as u8;
                }
            });
        }
        x += scaled.h_advance(id);
    }
    (pixels, w, h)
}

#[test]
fn adr_0014_tesseract_reads_synthetic_line() {
    let line = "Invoice 4471 paid to Harbor Freight";
    let (pixels, w, h) = render_line(line, 48.0);
    let mut ocr = TesseractOcr::english().expect("pinned eng.traineddata loads");
    let page = ocr.recognize_grey(&pixels, w, h, 300).unwrap();

    assert_eq!(
        page.text.trim(),
        line,
        "engine {}",
        TesseractOcr::engine_version()
    );
    let words: Vec<&str> = page.words.iter().map(|w| w.text.as_str()).collect();
    assert_eq!(words, line.split(' ').collect::<Vec<_>>());
    for word in &page.words {
        assert!((0.0..=1.0).contains(&word.confidence), "{word:?}");
        assert!(word.x0 < word.x1 && word.y0 < word.y1 && word.x1 <= w && word.y1 <= h);
    }
}
