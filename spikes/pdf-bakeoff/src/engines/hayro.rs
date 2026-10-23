//! Candidate A: hayro (pure Rust). `hayro-interpret` drives a glyph-collecting `Device` for
//! text and boxes; `hayro` renders through the same interpreter.

use std::path::Path;

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::BfString;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, interpret_page,
};
use hayro::hayro_syntax::page::{Page, Rotation};
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf};
use hayro::kurbo::{Affine, BezPath, Point, Rect};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings};

use crate::engine::{
    GrayImage, PageInfo, PdfDoc, PdfEngine, PdfError, PdfErrorKind, RawChar, read,
};

/// Glyph box in glyph space (1000 units per em): advance width × [descender, ascender].
/// hayro gives no font metrics, so the vertical extent is a fixed em fraction. ASSUMPTION.
const DESCENT: f64 = -250.0;
const ASCENT: f64 = 800.0;

pub struct Hayro {
    settings: InterpreterSettings,
}

impl Hayro {
    pub fn new() -> Self {
        Self {
            settings: InterpreterSettings::default(),
        }
    }
}

impl PdfEngine for Hayro {
    fn open<'e>(&'e self, path: &Path) -> Result<Box<dyn PdfDoc + 'e>, PdfError> {
        let pdf = Pdf::new(read(path)?).map_err(|e| match e {
            LoadPdfError::Decryption(DecryptionError::PasswordProtected) => {
                PdfError::new(PdfErrorKind::Encrypted, format!("{e:?}"))
            }
            LoadPdfError::Decryption(_) => {
                PdfError::new(PdfErrorKind::Unsupported, format!("{e:?}"))
            }
            LoadPdfError::Invalid => PdfError::new(PdfErrorKind::Corrupt, "invalid PDF"),
        })?;
        Ok(Box::new(HayroDoc {
            pdf,
            settings: &self.settings,
        }))
    }
}

struct HayroDoc<'e> {
    pdf: Pdf,
    settings: &'e InterpreterSettings,
}

impl HayroDoc<'_> {
    fn page(&self, page: usize) -> Result<&Page<'_>, PdfError> {
        self.pdf
            .pages()
            .get(page)
            .ok_or_else(|| PdfError::new(PdfErrorKind::Other, format!("no page {page}")))
    }
}

impl PdfDoc for HayroDoc<'_> {
    fn page_count(&self) -> usize {
        self.pdf.pages().len()
    }

    fn page_info(&self, page: usize) -> Result<PageInfo, PdfError> {
        let p = self.page(page)?;
        let mb = p.media_box();
        Ok(PageInfo {
            width: (mb.x1 - mb.x0) as f32,
            height: (mb.y1 - mb.y0) as f32,
            rotate: match p.rotation() {
                Rotation::None => 0,
                Rotation::Horizontal => 90,
                Rotation::Flipped => 180,
                Rotation::FlippedHorizontal => 270,
            },
        })
    }

    fn chars(&self, page: usize) -> Result<Vec<RawChar>, PdfError> {
        let p = self.page(page)?;
        let mb = p.media_box();
        // PDF user space → unrotated MediaBox, top-left origin, y down.
        let to_truth = Affine::new([1.0, 0.0, 0.0, -1.0, -mb.x0, mb.y1]);
        let cache = InterpreterCache::new();
        let mut ctx = Context::new(
            to_truth,
            Rect::new(-1e6, -1e6, 1e6, 1e6),
            &cache,
            p.xref(),
            self.settings.clone(),
        );
        let mut dev = Collector::default();
        interpret_page(p, &mut ctx, &mut dev);
        Ok(dev.chars)
    }

    fn render(&self, page: usize, scale: f32) -> Result<GrayImage, PdfError> {
        let p = self.page(page)?;
        let pix = hayro::render(
            p,
            &RenderCache::new(),
            self.settings,
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );
        Ok(GrayImage::from_rgba(
            pix.width() as u32,
            pix.height() as u32,
            pix.data_as_u8_slice(),
            false,
        ))
    }

    fn largest_image(&self, page: usize) -> Result<Option<(u64, u64)>, PdfError> {
        let xobjects = &self.page(page)?.resources().x_objects;
        let mut largest: Option<(u64, u64)> = None;
        for key in xobjects.keys() {
            let Some(stream) = xobjects.get::<hayro::hayro_syntax::object::Stream>(&*key) else {
                continue;
            };
            let d = stream.dict();
            let image = d
                .get::<hayro::hayro_syntax::object::Name>(b"Subtype")
                .is_some_and(|n| n.as_str() == "Image");
            if !image {
                continue;
            }
            let dim = |k: &[u8]| d.get::<f64>(k).map_or(0, |v| v.max(0.0) as u64);
            let (w, h) = (dim(b"Width"), dim(b"Height"));
            if largest.is_none_or(|(lw, lh)| w.saturating_mul(h) > lw.saturating_mul(lh)) {
                largest = Some((w, h));
            }
        }
        Ok(largest)
    }
}

#[derive(Default)]
struct Collector {
    chars: Vec<RawChar>,
    /// Fill-and-stroke text draws the same run twice; this spots the second call.
    last_fill: Option<(usize, usize, Affine)>,
}

impl<'a> Device<'a> for Collector {
    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, mode: &DrawMode) {
        let glyphs = run.glyphs();
        let Some(first) = glyphs.first() else {
            return;
        };
        let key = (glyphs.as_ptr() as usize, glyphs.len(), first.transform());
        match mode {
            DrawMode::Stroke(_) if self.last_fill.take() == Some(key) => return,
            DrawMode::Fill(_) => self.last_fill = Some(key),
            _ => self.last_fill = None,
        }
        for g in glyphs {
            let adv = match &**g {
                Glyph::Outline(o) => o.advance_width().unwrap_or(500.0) as f64,
                Glyph::Type3(_) => 500.0,
            };
            let t = props.transform * g.transform();
            let corners =
                [(0.0, DESCENT), (adv, DESCENT), (adv, ASCENT), (0.0, ASCENT)].map(|(x, y)| {
                    let p = t * Point::new(x, y);
                    (p.x, p.y)
                });
            let Some(rect) = rect_of(&corners) else {
                continue;
            };
            match g.as_unicode() {
                Some(BfString::Char(ch)) => self.chars.push(RawChar { ch, rect }),
                Some(BfString::String(s)) => split(&mut self.chars, &s, rect),
                None => self.chars.push(RawChar {
                    ch: '\u{FFFD}',
                    rect,
                }),
            }
        }
    }

    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}
}

/// Box from a quad's corners (any orientation), clamped to a valid `Rect`.
fn rect_of(points: &[(f64, f64)]) -> Option<mk_kernel::Rect> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in points {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    mk_kernel::Rect::new(x0 as f32, y0 as f32, x1 as f32, y1 as f32)
}

/// A glyph that maps to several characters (a ligature): split its box evenly.
fn split(out: &mut Vec<RawChar>, s: &str, rect: mk_kernel::Rect) {
    let n = s.chars().count().max(1) as f32;
    let w = rect.width() / n;
    for (i, ch) in s.chars().enumerate() {
        let x0 = rect.x0() + w * i as f32;
        if let Some(r) = mk_kernel::Rect::new(x0, rect.y0(), x0 + w, rect.y1()) {
            out.push(RawChar { ch, rect: r });
        }
    }
}
