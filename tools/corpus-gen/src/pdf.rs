//! Minimal PDF writer: numbered objects, classic xref table, Flate streams, standard-14 fonts
//! (WinAnsi), images, invisible text, and RC4-128 (R3) encryption. Written in-house so that
//! the TD-3 files can break any part of the structure on purpose.

use std::io::Write as _;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use md5::{Digest, Md5};
use mk_bench::{BBox, Location, PageText, Unit};

use crate::fonts::{Afm, Font, Metrics, winansi};
use crate::layout::{ImageData, Laid, Line, Page, char_xs, word_ranges};
use crate::truth::{Placed, Segment};

pub enum Obj {
    /// Any object body, written as is.
    Raw(Vec<u8>),
    /// A stream: dictionary entries (without `/Length`) and data, already filtered.
    Stream { dict: String, data: Vec<u8> },
}

#[derive(Default)]
pub struct Writer {
    objs: Vec<Option<Obj>>,
}

pub struct Encryption {
    pub user_password: Vec<u8>,
    pub owner_password: Vec<u8>,
    /// Permission flags (`/P`).
    pub permissions: i32,
}

pub struct Trailer {
    pub root: u32,
    pub info: Option<u32>,
    pub id: [u8; 16],
    pub encrypt: Option<Encryption>,
}

/// Result of serializing.
pub struct Serialized {
    pub bytes: Vec<u8>,
}

pub fn flate(data: &[u8]) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::best());
    e.write_all(data).expect("in-memory write");
    e.finish().expect("in-memory write")
}

impl Writer {
    pub fn reserve(&mut self) -> u32 {
        self.objs.push(None);
        self.objs.len() as u32
    }

    pub fn set(&mut self, id: u32, obj: Obj) {
        self.objs[id as usize - 1] = Some(obj);
    }

    pub fn add(&mut self, obj: Obj) -> u32 {
        let id = self.reserve();
        self.set(id, obj);
        id
    }

    pub fn raw(&mut self, body: impl Into<Vec<u8>>) -> u32 {
        self.add(Obj::Raw(body.into()))
    }

    pub fn serialize(&self, t: &Trailer) -> Serialized {
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
        let crypt = t.encrypt.as_ref().map(|e| Crypt::new(e, &t.id));
        let mut offsets = Vec::new();
        let mut encrypt_id = None;
        let total = self.objs.len() + usize::from(crypt.is_some());
        for (i, obj) in self.objs.iter().enumerate() {
            let num = i as u32 + 1;
            offsets.push(out.len());
            writeln!(out, "{num} 0 obj").unwrap();
            match obj
                .as_ref()
                .unwrap_or_else(|| panic!("object {num} reserved but never set"))
            {
                Obj::Raw(b) => out.extend_from_slice(b),
                Obj::Stream { dict, data } => {
                    let data = match &crypt {
                        Some(c) => c.apply(num, data),
                        None => data.clone(),
                    };
                    write!(out, "<< {dict} /Length {} >>\nstream\n", data.len()).unwrap();
                    out.extend_from_slice(&data);
                    out.extend_from_slice(b"\nendstream");
                }
            }
            out.extend_from_slice(b"\nendobj\n");
        }
        if let Some(c) = &crypt {
            let num = total as u32;
            encrypt_id = Some(num);
            offsets.push(out.len());
            write!(
                out,
                "{num} 0 obj\n<< /Filter /Standard /V 2 /R 3 /Length 128 /O <{}> /U <{}> /P {} >>\nendobj\n",
                hex(&c.o),
                hex(&c.u),
                c.p
            )
            .unwrap();
        }
        let xref_offset = out.len();
        write!(out, "xref\n0 {}\n0000000000 65535 f \n", total + 1).unwrap();
        for off in &offsets {
            writeln!(out, "{off:010} 00000 n ").unwrap();
        }
        write!(out, "trailer\n<< /Size {} /Root {} 0 R", total + 1, t.root).unwrap();
        if let Some(info) = t.info {
            write!(out, " /Info {info} 0 R").unwrap();
        }
        if let Some(e) = encrypt_id {
            write!(out, " /Encrypt {e} 0 R").unwrap();
        }
        write!(
            out,
            " /ID [<{0}> <{0}>] >>\nstartxref\n{xref_offset}\n%%EOF\n",
            hex(&t.id)
        )
        .unwrap();
        Serialized { bytes: out }
    }
}

pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

fn padded(pw: &[u8]) -> Vec<u8> {
    pw.iter()
        .copied()
        .chain(PAD.iter().copied())
        .take(32)
        .collect()
}

fn md5(parts: &[&[u8]]) -> [u8; 16] {
    let mut h = Md5::new();
    for p in parts {
        h.update(p);
    }
    h.finalize().into()
}

/// RC4 stream cipher (encryption and decryption are the same operation).
pub fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j: u8 = 0;
    for i in 0..256 {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|b| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            b ^ s[s[i as usize].wrapping_add(s[j as usize]) as usize]
        })
        .collect()
}

/// PDF standard security handler, revision 3, 128-bit key (PDF 1.7 §7.6.3, algorithms 2, 3, 5).
struct Crypt {
    key: [u8; 16],
    o: Vec<u8>,
    u: Vec<u8>,
    p: i32,
}

impl Crypt {
    fn new(e: &Encryption, id: &[u8; 16]) -> Self {
        // Algorithm 3: O.
        let mut h = md5(&[&padded(&e.owner_password)]);
        for _ in 0..50 {
            h = md5(&[&h]);
        }
        let mut o = rc4(&h, &padded(&e.user_password));
        for i in 1..=19u8 {
            let k: Vec<u8> = h.iter().map(|b| b ^ i).collect();
            o = rc4(&k, &o);
        }
        // Algorithm 2: file key.
        let p = e.permissions;
        let mut key = md5(&[&padded(&e.user_password), &o, &p.to_le_bytes(), id]);
        for _ in 0..50 {
            key = md5(&[&key]);
        }
        // Algorithm 5: U.
        let mut u = rc4(&key, &md5(&[&PAD, id]));
        for i in 1..=19u8 {
            let k: Vec<u8> = key.iter().map(|b| b ^ i).collect();
            u = rc4(&k, &u);
        }
        u.extend_from_slice(&[0u8; 16]);
        Crypt { key, o, u, p }
    }

    fn apply(&self, num: u32, data: &[u8]) -> Vec<u8> {
        let n = num.to_le_bytes();
        let k = md5(&[&self.key, &n[..3], &[0, 0]]);
        rc4(&k, data)
    }
}

/// Number formatting for content streams: up to 3 decimals, no trailing zeros.
pub fn num(v: f64) -> String {
    let s = format!("{v:.3}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.to_string() }
}

/// A PDF string literal in WinAnsi.
pub fn pdf_string(s: &str) -> String {
    let mut out = String::from("(");
    for c in s.chars() {
        let b = winansi(c);
        match b {
            b'(' | b')' | b'\\' => {
                out.push('\\');
                out.push(b as char);
            }
            0x20..=0x7E => out.push(b as char),
            _ => out.push_str(&format!("\\{b:03o}")),
        }
    }
    out.push(')');
    out
}

pub struct PdfOpts {
    pub title: String,
    pub encrypt: Option<Encryption>,
    pub id_seed: String,
}

impl PdfOpts {
    pub fn new(title: &str) -> Self {
        PdfOpts {
            title: title.into(),
            encrypt: None,
            id_seed: title.into(),
        }
    }
}

/// Unrotated MediaBox size of a page.
pub fn media_size(p: &Page) -> (f64, f64) {
    if p.rotate == 90 {
        (p.height, p.width)
    } else {
        (p.width, p.height)
    }
}

/// Display space (top-left, y down) → PDF user space (bottom-left, y up).
fn to_user(p: &Page, dx: f64, dy: f64) -> (f64, f64) {
    let (_, mh) = media_size(p);
    if p.rotate == 90 {
        (dy, dx)
    } else {
        (dx, mh - dy)
    }
}

/// Display-space box → ground-truth box (unrotated MediaBox, top-left origin, y down).
pub fn to_truth_box(p: &Page, b: [f64; 4]) -> BBox {
    let r = if p.rotate == 90 {
        [b[1], p.width - b[2], b[3], p.width - b[0]]
    } else {
        b
    };
    BBox(r.map(round2))
}

pub fn round2(v: f64) -> f64 {
    let r = (v * 100.0).round() / 100.0;
    if r == 0.0 { 0.0 } else { r }
}

fn text_op(p: &Page, l: &Line, hidden_scale: Option<f64>) -> String {
    let (fname, _) = l.font.pdf();
    let (ux, uy) = to_user(p, l.x, l.y);
    let m = if p.rotate == 90 {
        format!("0 1 -1 0 {} {}", num(ux), num(uy))
    } else {
        format!("1 0 0 1 {} {}", num(ux), num(uy))
    };
    let mut s = String::from("BT ");
    if let Some(tz) = hidden_scale {
        s.push_str(&format!("3 Tr {} Tz ", num(tz)));
    }
    s.push_str(&format!(
        "/{fname} {} Tf {m} Tm {} Tj ET\n",
        num(l.size),
        pdf_string(&l.text)
    ));
    s
}

fn content(p: &Page, images: &[String]) -> Vec<u8> {
    let mut s = String::new();
    for (img, name) in p.images.iter().zip(images) {
        let (x0, y1) = to_user(p, img.rect[0], img.rect[3]);
        let (w, h) = (img.rect[2] - img.rect[0], img.rect[3] - img.rect[1]);
        s.push_str(&format!(
            "q {} 0 0 {} {} {} cm /{name} Do Q\n",
            num(w),
            num(h),
            num(x0),
            num(y1)
        ));
    }
    if !p.rules.is_empty() {
        s.push_str("0.5 w 0.3 G\n");
        for r in &p.rules {
            let (a, b) = to_user(p, r[0], r[1]);
            let (c, d) = to_user(p, r[2], r[3]);
            s.push_str(&format!(
                "{} {} m {} {} l S\n",
                num(a),
                num(b),
                num(c),
                num(d)
            ));
        }
    }
    let n = p.lines.len();
    let order: Vec<usize> = if p.shuffle {
        // Second half first, then the first half backwards: never reading order.
        (n / 2..n).chain((0..n / 2).rev()).collect()
    } else {
        (0..n).collect()
    };
    for i in order {
        s.push_str(&text_op(p, &p.lines[i], None));
    }
    for w in &p.hidden {
        let height = w.rect[3] - w.rect[1];
        let size = height / (Afm.ascent(Font::Sans) - Afm.descent(Font::Sans));
        let natural = Afm.width(Font::Sans, &w.text) * size;
        let tz = 100.0 * (w.rect[2] - w.rect[0]) / natural;
        let l = Line {
            x: w.rect[0],
            y: w.rect[1] + Afm.ascent(Font::Sans) * size,
            font: Font::Sans,
            size,
            text: w.text.clone(),
            marks: Vec::new(),
        };
        s.push_str(&text_op(p, &l, Some(tz)));
    }
    s.into_bytes()
}

/// The ID is derived from a seed string, never from the clock.
pub fn file_id(seed: &str) -> [u8; 16] {
    md5(&[seed.as_bytes()])
}

/// Font dictionaries for the four standard fonts; returns the resource dictionary text.
pub fn add_fonts(w: &mut Writer) -> String {
    let mut res = String::from("/Font <<");
    for f in Font::ALL {
        let (name, base) = f.pdf();
        let id = w.raw(format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /{base} /Encoding /WinAnsiEncoding >>"
        ));
        res.push_str(&format!(" /{name} {id} 0 R"));
    }
    res.push_str(" >>");
    res
}

/// Writes a complete PDF from laid-out pages.
pub fn render(laid: &Laid, opts: PdfOpts) -> Vec<u8> {
    let mut w = Writer::default();
    let catalog = w.reserve();
    let pages_id = w.reserve();
    let fonts = add_fonts(&mut w);
    let mut kids = Vec::new();
    for p in &laid.pages {
        let mut xobjs = Vec::new();
        let mut names = Vec::new();
        for (i, img) in p.images.iter().enumerate() {
            let (filter, data) = match &img.data {
                ImageData::Gray(px) => ("/FlateDecode", flate(px)),
                ImageData::Jpeg(b) => ("/DCTDecode", b.clone()),
            };
            let id = w.add(Obj::Stream {
                dict: format!(
                    "/Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter {filter}",
                    img.width_px, img.height_px
                ),
                data,
            });
            let name = format!("Im{}", i + 1);
            xobjs.push(format!("/{name} {id} 0 R"));
            names.push(name);
        }
        let cs = w.add(Obj::Stream {
            dict: "/Filter /FlateDecode".into(),
            data: flate(&content(p, &names)),
        });
        let (mw, mh) = media_size(p);
        let mut res = format!("<< {fonts}");
        if !xobjs.is_empty() {
            res.push_str(&format!(" /XObject << {} >>", xobjs.join(" ")));
        }
        res.push_str(" >>");
        let rot = if p.rotate != 0 {
            format!(" /Rotate {}", p.rotate)
        } else {
            String::new()
        };
        kids.push(w.raw(format!(
            "<< /Type /Page /Parent {pages_id} 0 R /MediaBox [0 0 {} {}]{rot} /Resources {res} /Contents {cs} 0 R >>",
            num(mw),
            num(mh)
        )));
    }
    let kid_refs: Vec<String> = kids.iter().map(|k| format!("{k} 0 R")).collect();
    w.set(
        pages_id,
        Obj::Raw(
            format!(
                "<< /Type /Pages /Kids [{}] /Count {} >>",
                kid_refs.join(" "),
                kids.len()
            )
            .into_bytes(),
        ),
    );
    w.set(
        catalog,
        Obj::Raw(format!("<< /Type /Catalog /Pages {pages_id} 0 R >>").into_bytes()),
    );
    let info = if opts.encrypt.is_none() {
        Some(w.raw(format!(
            "<< /Title {} /Producer (corpus-gen) >>",
            pdf_string(&opts.title)
        )))
    } else {
        None
    };
    w.serialize(&Trailer {
        root: catalog,
        info,
        id: file_id(&opts.id_seed),
        encrypt: opts.encrypt,
    })
    .bytes
}

/// Ground truth for the text lines of every page (text layer, PDF points).
/// Pages listed in `skip` (scanned pages, handled by the raster module) are left out.
pub fn truth(laid: &Laid, skip: &[usize]) -> Placed {
    let m = &Afm;
    let mut placed = Placed::default();
    let mut mark_boxes: Vec<Vec<(usize, BBox)>> = vec![Vec::new(); laid.marks.len()];
    for (pi, page) in laid.pages.iter().enumerate() {
        if skip.contains(&pi) {
            continue;
        }
        let (text, offs) = crate::layout::page_text(page);
        let mut words = Vec::new();
        let mut ranges: Vec<Option<(usize, usize)>> = vec![None; laid.marks.len()];
        for (li, l) in page.lines.iter().enumerate() {
            let xs = char_xs(m, l);
            let top = l.y - m.ascent(l.font) * l.size;
            let bottom = l.y - m.descent(l.font) * l.size;
            let chars: Vec<char> = l.text.chars().collect();
            for (s, e) in word_ranges(&l.text) {
                words.push(mk_bench::Word {
                    t: chars[s..e].iter().collect(),
                    b: to_truth_box(page, [xs[s], top, xs[e], bottom]),
                });
            }
            for lm in &l.marks {
                mark_boxes[lm.mark].push((
                    pi,
                    to_truth_box(page, [xs[lm.start], top, xs[lm.end], bottom]),
                ));
                let (s, e) = (offs[li] + lm.start, offs[li] + lm.end);
                let r = ranges[lm.mark].get_or_insert((s, e));
                r.0 = r.0.min(s);
                r.1 = r.1.max(e);
            }
        }
        let (mw, mh) = media_size(page);
        placed.pages.push(PageText {
            document: String::new(),
            page: pi as u32 + 1,
            unit: Unit::Pt,
            width: mw,
            height: mh,
            dpi: None,
            rotate: page.rotate,
            skew_deg: 0.0,
            text: text.clone(),
            words,
        });
        placed.segments.push(Segment {
            text,
            marked: ranges.into_iter().flatten().collect(),
        });
    }
    for (i, boxes) in mark_boxes.into_iter().enumerate() {
        if boxes.is_empty() {
            continue;
        }
        let page = boxes[0].0;
        assert!(
            boxes.iter().all(|b| b.0 == page),
            "mark {:?} spans pages",
            laid.marks[i].raw
        );
        placed.mentions.push((
            laid.marks[i].clone(),
            Location::PdfText {
                page: page as u32 + 1,
                boxes: boxes.into_iter().map(|b| b.1).collect(),
            },
        ));
    }
    placed
}
