//! Scanned pages: draw a laid-out page with the Noto fonts, then degrade it per tier.
//! Word and mention boxes are the tight glyph bounds, moved with the skew.
//! All maths is plain f32/f64 plus `libm` trig, with no platform libm and no SIMD paths.

use ab_glyph::{Font as _, PxScale, point};
use mk_bench::{BBox, Location, PageText, ScanTier, Unit, Word};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};

use crate::fonts::Noto;
use crate::layout::{Laid, Page, char_xs, page_text, word_ranges};
use crate::pdf::round2;
use crate::truth::{Placed, Segment};

pub struct Tier {
    pub tier: ScanTier,
    pub dpi: u32,
    pub skew_deg: f64,
}

impl Tier {
    pub fn clean() -> Self {
        Tier {
            tier: ScanTier::Clean,
            dpi: 300,
            skew_deg: 0.0,
        }
    }
    /// `skew` within ±1.5°.
    pub fn office(skew: f64) -> Self {
        assert!(skew.abs() <= 1.5);
        Tier {
            tier: ScanTier::Office,
            dpi: 300,
            skew_deg: skew,
        }
    }
    pub fn poor(skew: f64) -> Self {
        Tier {
            tier: ScanTier::Poor,
            dpi: 200,
            skew_deg: skew,
        }
    }
}

pub struct Scanned {
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub pixels: Vec<u8>,
    /// Mentions use `Location::Scan` with the given page number.
    pub placed: Placed,
}

const INK: f32 = 28.0;

#[derive(Clone, Copy)]
struct Rect {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
}

impl Rect {
    fn union(a: Option<Rect>, b: Rect) -> Rect {
        match a {
            None => b,
            Some(a) => Rect {
                x0: a.x0.min(b.x0),
                y0: a.y0.min(b.y0),
                x1: a.x1.max(b.x1),
                y1: a.y1.max(b.y1),
            },
        }
    }
}

/// Renders one laid-out page (`laid.pages[page_index]`) as a scan.
pub fn scan(
    laid: &Laid,
    page_index: usize,
    noto: &Noto,
    tier: &Tier,
    seed: u64,
    page_no: u32,
) -> Scanned {
    let page: &Page = &laid.pages[page_index];
    let s = tier.dpi as f32 / 72.0;
    let w = (page.width as f32 * s).round() as u32;
    let h = (page.height as f32 * s).round() as u32;
    let mut cov = vec![0f32; (w * h) as usize];

    // Tight box per char of each line.
    let mut char_boxes: Vec<Vec<Option<Rect>>> = Vec::new();
    for l in &page.lines {
        let f = noto.font(l.font);
        let upem = f.units_per_em().expect("upem");
        let em_px = l.size as f32 * s;
        let scale = PxScale::from(em_px * f.height_unscaled() / upem);
        let xs = char_xs(noto, l);
        let mut boxes = Vec::new();
        for (i, c) in l.text.chars().enumerate() {
            let g = f
                .glyph_id(c)
                .with_scale_and_position(scale, point(xs[i] as f32 * s, l.y as f32 * s));
            let Some(og) = f.outline_glyph(g) else {
                boxes.push(None);
                continue;
            };
            let b = og.px_bounds();
            og.draw(|gx, gy, c| {
                let x = b.min.x as i64 + gx as i64;
                let y = b.min.y as i64 + gy as i64;
                if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
                    let p = &mut cov[(y as u32 * w + x as u32) as usize];
                    *p = (*p + c).min(1.0);
                }
            });
            boxes.push(Some(Rect {
                x0: b.min.x,
                y0: b.min.y,
                x1: b.max.x,
                y1: b.max.y,
            }));
        }
        char_boxes.push(boxes);
    }
    for r in &page.rules {
        draw_rule(
            &mut cov,
            w,
            h,
            [
                r[0] as f32 * s,
                r[1] as f32 * s,
                r[2] as f32 * s,
                r[3] as f32 * s,
            ],
            1.0 * s / 4.0,
        );
    }
    let mut px: Vec<u8> = cov
        .iter()
        .map(|c| (255.0 - c * (255.0 - INK)).round() as u8)
        .collect();

    let mut rng = ChaCha20Rng::seed_from_u64(seed);
    let theta = tier.skew_deg.to_radians();
    if tier.skew_deg != 0.0 {
        px = rotate(&px, w, h, theta);
    }
    match tier.tier {
        ScanTier::Clean => {}
        ScanTier::Office => {
            px = blur(&px, w, h);
            noise(&mut px, &mut rng, 6.0);
        }
        ScanTier::Poor => {
            contrast(&mut px, 72.0, 214.0);
            speckle(&mut px, w, h, &mut rng, 1.0 / 2500.0);
            px = blur(&px, w, h);
            noise(&mut px, &mut rng, 9.0);
        }
    }

    let fwd = |r: Rect| -> BBox {
        if theta == 0.0 {
            return BBox([r.x0, r.y0, r.x1, r.y1].map(|v| round2(v.clamp(0.0, f32::MAX) as f64)));
        }
        let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
        let (c, sn) = (libm::cos(theta), libm::sin(theta));
        let corners = [(r.x0, r.y0), (r.x1, r.y0), (r.x0, r.y1), (r.x1, r.y1)].map(|(x, y)| {
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            (cx + dx * c + dy * sn, cy - dx * sn + dy * c)
        });
        let xs = corners.map(|p| p.0);
        let ys = corners.map(|p| p.1);
        let min = |a: [f64; 4]| a.iter().copied().fold(f64::MAX, f64::min);
        let max = |a: [f64; 4]| a.iter().copied().fold(f64::MIN, f64::max);
        BBox([
            round2(min(xs).max(0.0)),
            round2(min(ys).max(0.0)),
            round2(max(xs).min(w as f64)),
            round2(max(ys).min(h as f64)),
        ])
    };
    let clamp = |r: Rect| Rect {
        x0: r.x0.max(0.0),
        y0: r.y0.max(0.0),
        x1: r.x1.min(w as f32),
        y1: r.y1.min(h as f32),
    };

    let (text, offs) = page_text(page);
    let mut words = Vec::new();
    let mut mark_boxes: Vec<Vec<BBox>> = vec![Vec::new(); laid.marks.len()];
    let mut ranges: Vec<Option<(usize, usize)>> = vec![None; laid.marks.len()];
    let span_box = |li: usize, s: usize, e: usize| -> Option<Rect> {
        char_boxes[li][s..e]
            .iter()
            .flatten()
            .fold(None, |acc, r| Some(Rect::union(acc, *r)))
    };
    for (li, l) in page.lines.iter().enumerate() {
        let chars: Vec<char> = l.text.chars().collect();
        for (s0, e0) in word_ranges(&l.text) {
            if let Some(r) = span_box(li, s0, e0) {
                words.push(Word {
                    t: chars[s0..e0].iter().collect(),
                    b: fwd(clamp(r)),
                });
            }
        }
        for lm in &l.marks {
            if let Some(r) = span_box(li, lm.start, lm.end) {
                mark_boxes[lm.mark].push(fwd(clamp(r)));
            }
            let (s0, e0) = (offs[li] + lm.start, offs[li] + lm.end);
            let r = ranges[lm.mark].get_or_insert((s0, e0));
            r.0 = r.0.min(s0);
            r.1 = r.1.max(e0);
        }
    }
    let mut placed = Placed::default();
    for (i, boxes) in mark_boxes.into_iter().enumerate() {
        if !boxes.is_empty() {
            placed.mentions.push((
                laid.marks[i].clone(),
                Location::Scan {
                    page: page_no,
                    dpi: tier.dpi,
                    boxes,
                },
            ));
        }
    }
    placed.segments.push(Segment {
        text: text.clone(),
        marked: ranges.into_iter().flatten().collect(),
    });
    placed.pages.push(PageText {
        document: String::new(),
        page: page_no,
        unit: Unit::Px,
        width: w as f64,
        height: h as f64,
        dpi: Some(tier.dpi),
        rotate: 0,
        skew_deg: tier.skew_deg,
        text,
        words,
    });
    Scanned {
        width: w,
        height: h,
        dpi: tier.dpi,
        pixels: px,
        placed,
    }
}

fn draw_rule(cov: &mut [f32], w: u32, h: u32, r: [f32; 4], half: f32) {
    let (x0, x1) = (r[0].min(r[2]) - half, r[0].max(r[2]) + half);
    let (y0, y1) = (r[1].min(r[3]) - half, r[1].max(r[3]) + half);
    for y in (y0.max(0.0) as u32)..(y1.min(h as f32) as u32) {
        for x in (x0.max(0.0) as u32)..(x1.min(w as f32) as u32) {
            cov[(y * w + x) as usize] = 0.8;
        }
    }
}

/// Rotates counter-clockwise by `theta` around the centre, bilinear, white outside.
fn rotate(src: &[u8], w: u32, h: u32, theta: f64) -> Vec<u8> {
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let (c, s) = (libm::cos(theta), libm::sin(theta));
    let mut out = vec![255u8; src.len()];
    let get = |x: i64, y: i64| -> f64 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            255.0
        } else {
            src[(y as u32 * w + x as u32) as usize] as f64
        }
    };
    for oy in 0..h {
        for ox in 0..w {
            let (dx, dy) = (ox as f64 + 0.5 - cx, oy as f64 + 0.5 - cy);
            let sx = cx + dx * c - dy * s - 0.5;
            let sy = cy + dx * s + dy * c - 0.5;
            let (fx, fy) = (libm::floor(sx), libm::floor(sy));
            let (ax, ay) = (sx - fx, sy - fy);
            let (ix, iy) = (fx as i64, fy as i64);
            let v = get(ix, iy) * (1.0 - ax) * (1.0 - ay)
                + get(ix + 1, iy) * ax * (1.0 - ay)
                + get(ix, iy + 1) * (1.0 - ax) * ay
                + get(ix + 1, iy + 1) * ax * ay;
            out[(oy * w + ox) as usize] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// 3×3 weighted blur (1 2 1 / 2 4 2 / 1 2 1), integer maths.
fn blur(src: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (w, h) = (w as i64, h as i64);
    let at = |x: i64, y: i64| src[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize] as u32;
    let mut out = vec![0u8; src.len()];
    for y in 0..h {
        for x in 0..w {
            let v = at(x - 1, y - 1)
                + 2 * at(x, y - 1)
                + at(x + 1, y - 1)
                + 2 * at(x - 1, y)
                + 4 * at(x, y)
                + 2 * at(x + 1, y)
                + at(x - 1, y + 1)
                + 2 * at(x, y + 1)
                + at(x + 1, y + 1);
            out[(y * w + x) as usize] = ((v + 8) / 16) as u8;
        }
    }
    out
}

/// Approximately Gaussian noise (sum of four uniforms), standard deviation `sigma`.
fn noise(px: &mut [u8], rng: &mut ChaCha20Rng, sigma: f32) {
    // Sum of 4 uniforms on [-0.5, 0.5] has variance 1/3.
    let k = sigma * 3f32.sqrt();
    for p in px.iter_mut() {
        let r = rng.next_u32();
        let u: f32 = (0..4)
            .map(|i| ((r >> (i * 8)) & 0xff) as f32 / 255.0 - 0.5)
            .sum();
        *p = (*p as f32 + u * k).round().clamp(0.0, 255.0) as u8;
    }
}

fn contrast(px: &mut [u8], lo: f32, hi: f32) {
    for p in px.iter_mut() {
        *p = (lo + (*p as f32 / 255.0) * (hi - lo)).round() as u8;
    }
}

/// Dark specks (2×2) and white drop-outs at `density` per pixel.
fn speckle(px: &mut [u8], w: u32, h: u32, rng: &mut ChaCha20Rng, density: f64) {
    let n = (w as f64 * h as f64 * density) as u32;
    for i in 0..n {
        let x = rng.next_u32() % (w - 1);
        let y = rng.next_u32() % (h - 1);
        let v = if i % 3 == 0 { 250 } else { 40 };
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            px[((y + dy) * w + x + dx) as usize] = v;
        }
    }
}

pub fn png_gray(w: u32, h: u32, px: &[u8], dpi: u32) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Grayscale);
        e.set_depth(png::BitDepth::Eight);
        e.set_compression(png::Compression::High);
        let ppm = (dpi as f64 / 0.0254).round() as u32;
        e.set_pixel_dims(Some(png::PixelDimensions {
            xppu: ppm,
            yppu: ppm,
            unit: png::Unit::Meter,
        }));
        let mut wr = e.write_header().expect("png header");
        wr.write_image_data(px).expect("png data");
    }
    out
}

pub fn jpeg_gray(w: u32, h: u32, px: &[u8], dpi: u32, quality: u8) -> Vec<u8> {
    let mut out = Vec::new();
    let mut e = jpeg_encoder::Encoder::new(&mut out, quality);
    e.set_density(jpeg_encoder::PixelDensity {
        density: (dpi as u16, dpi as u16),
        unit: jpeg_encoder::PixelDensityUnit::Inches,
    });
    e.encode(px, w as u16, h as u16, jpeg_encoder::ColorType::Luma)
        .expect("jpeg");
    out
}

/// A multi-page TIFF: one LZW grey image per page, each `(width, height, pixels)`.
pub fn tiff_gray_pages(pages: &[(u32, u32, &[u8])], dpi: u32) -> Vec<u8> {
    use tiff::encoder::{Compression, Rational, TiffEncoder, colortype};
    use tiff::tags::ResolutionUnit;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut buf)
            .expect("tiff")
            .with_compression(Compression::Lzw);
        for (w, h, px) in pages {
            let mut img = enc
                .new_image::<colortype::Gray8>(*w, *h)
                .expect("tiff image");
            img.resolution(ResolutionUnit::Inch, Rational { n: dpi, d: 1 });
            img.write_data(px).expect("tiff data");
        }
    }
    buf.into_inner()
}

/// A lossless WEBP of grey pixels (WEBP keeps no resolution).
pub fn webp_gray(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    image_webp::WebPEncoder::new(&mut out)
        .encode(px, w, h, image_webp::ColorType::L8)
        .expect("webp");
    out
}

pub fn tiff_gray(w: u32, h: u32, px: &[u8], dpi: u32) -> Vec<u8> {
    use tiff::encoder::{Compression, Rational, TiffEncoder, colortype};
    use tiff::tags::ResolutionUnit;
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut enc = TiffEncoder::new(&mut buf)
            .expect("tiff")
            .with_compression(Compression::Lzw);
        let mut img = enc.new_image::<colortype::Gray8>(w, h).expect("tiff image");
        img.resolution(ResolutionUnit::Inch, Rational { n: dpi, d: 1 });
        img.write_data(px).expect("tiff data");
    }
    buf.into_inner()
}
