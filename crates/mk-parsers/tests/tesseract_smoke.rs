//! ADR-0014 build check: the statically linked Tesseract reads synthetic lines through the
//! `OcrEngine` port, with the built-in English (ADR-0018); and an image file goes through the
//! whole OCR path into an `OcrText` segment with word boxes and confidences (US-02.2 AC2/AC3).

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};
use mk_kernel::Locator;
use mk_parsers::image::GrayImage;
use mk_parsers::ocr::{OcrEngine, install_english};
use mk_parsers::{Engines, Format, OcrConfig, TesseractOcr};

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

/// Two lines, one under the other.
fn render_lines(lines: [&str; 2], px: f32) -> GrayImage {
    let (a, aw, ah) = render_line(lines[0], px);
    let (b, bw, bh) = render_line(lines[1], px);
    let width = aw.max(bw);
    let mut pixels = vec![255u8; (width * (ah + bh)) as usize];
    for (rows, w, top) in [(&a, aw, 0), (&b, bw, ah)] {
        for (y, row) in rows.chunks(w as usize).enumerate() {
            let at = ((top + y as u32) * width) as usize;
            pixels[at..at + w as usize].copy_from_slice(row);
        }
    }
    GrayImage {
        width,
        height: ah + bh,
        pixels,
    }
}

#[test]
fn adr_0014_tesseract_reads_synthetic_lines() {
    let lines = [
        "Invoice 4471 paid to Harbor Freight",
        "Contact ops@harbor.example",
    ];
    let image = render_lines(lines, 48.0);
    let dir = tempfile::tempdir().unwrap();
    install_english(dir.path()).unwrap();
    let mut ocr = TesseractOcr::new(dir.path(), "eng").expect("built-in eng.traineddata loads");
    let got = ocr.recognize(&image, 300).unwrap();

    let text: Vec<String> = got
        .iter()
        .map(|l| {
            l.words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    assert_eq!(text, lines, "engine {}", TesseractOcr::engine_version());
    for word in got.iter().flat_map(|l| &l.words) {
        assert!(
            word.confidence.is_some_and(|c| (0.0..=1.0).contains(&c)),
            "{word:?}"
        );
        let r = word.rect;
        assert!(r.x1() <= image.width as f32 && r.y1() <= image.height as f32);
    }
}

/// US-02.2 AC2/AC3: a PNG is OCR'd into one `OcrText` segment; the mention in it gets the boxes
/// of its words and the lowest word confidence (INV-ID-03).
#[test]
fn us_02_2_png_through_the_ocr_path() {
    let lines = [
        "Invoice 4471 paid to Harbor Freight",
        "Contact ops@harbor.example",
    ];
    let png = render_lines(lines, 48.0).to_png().unwrap();
    let dir = tempfile::tempdir().unwrap();
    install_english(dir.path()).unwrap();
    let mut engines = Engines::new(Some(OcrConfig {
        tessdata: dir.path().to_path_buf(),
        languages: "eng".into(),
    }));
    // The bytes decide: a PNG named as a PDF is still an image.
    let parsed = mk_parsers::parse_with(Format::Pdf, &png, &mut engines).unwrap();
    let doc = &parsed.documents[0];
    assert_eq!(doc.kind, Format::Image);
    assert_eq!(doc.segments.len(), 1);
    let seg = &doc.segments[0];
    assert_eq!(seg.text, lines.join("\n"));
    let at = seg.text.find("ops@").unwrap();
    let span =
        mk_kernel::Span::new(at, seg.text.chars().count(), seg.text.chars().count()).unwrap();
    let Some(Locator::OcrText { page, boxes, .. }) = doc.mention_locator(0, span) else {
        panic!("no OCR locator");
    };
    assert_eq!((page.get(), boxes.len()), (1, 1));
    let conf = seg.ocr_confidence(span).unwrap();
    assert!((0.0..=1.0).contains(&conf));
    // Without the language file the file fails with a plain reason.
    let mut missing = Engines::new(Some(OcrConfig {
        tessdata: dir.path().to_path_buf(),
        languages: "eng+deu".into(),
    }));
    let e = mk_parsers::parse_with(Format::Image, &png, &mut missing).unwrap_err();
    assert_eq!(e.to_string(), "OCR language not installed: deu");
}
