//! TD-3 v1: broken and edge-case PDF and image files, with `expected.json`
//! (`docs/07-test-corpus.md` §6). Every file must finish within the per-file limits
//! (NFR-REL-06/07) and never stop the batch (NFR-REL-01).

use std::io::Write as _;
use std::path::Path;

use flate2::Compression;
use flate2::write::ZlibEncoder;
use mk_bench::Generator;
use mk_bench::td3::{Outcome, Td3Category as Cat, Td3File, Td3Manifest};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};

use crate::corpus::file_seed;
use crate::fonts::{Afm, Font, Noto};
use crate::layout::{A4, Flow, Laid};
use crate::pdf::{self, Encryption, Obj, PdfOpts, Trailer, Writer, add_fonts, file_id, flate};
use crate::raster::{self, Tier};
use crate::rich::Rich;

struct Out {
    files: Vec<Td3File>,
    dir: std::path::PathBuf,
}

impl Out {
    fn add(
        &mut self,
        path: &str,
        bytes: &[u8],
        category: Cat,
        expected: Outcome,
        reason: &str,
    ) -> Result<(), Vec<String>> {
        let full = self.dir.join(path);
        std::fs::write(&full, bytes).map_err(|e| vec![format!("{}: {e}", full.display())])?;
        self.files.push(Td3File {
            path: path.to_string(),
            sha256: mk_bench::sha256_hex(bytes),
            bytes: bytes.len() as u64,
            category,
            expected,
            reason: reason.to_string(),
        });
        Ok(())
    }
}

/// A short text document laid out with the AFM metrics (for PDFs).
fn text_laid(lines: &[&str], pages: usize) -> Laid {
    let mut f = Flow::new(&Afm, A4, 72.0);
    for p in 0..pages {
        if p > 0 {
            f.new_page();
        }
        for l in lines {
            f.para(
                Font::Sans,
                12.0,
                &Rich::plain_text(&l.replace("{n}", &(p + 1).to_string())),
            );
        }
    }
    f.laid
}

fn text_pdf(title: &str, lines: &[&str], pages: usize) -> Vec<u8> {
    pdf::render(&text_laid(lines, pages), PdfOpts::new(title))
}

/// A small scanned page with a few words, at 150 DPI.
fn small_scan(noto: &Noto, seed: u64, text: &str) -> raster::Scanned {
    let mut f = Flow::new(noto, (300.0, 200.0), 24.0);
    f.para(Font::Sans, 14.0, &Rich::plain_text(text));
    let tier = Tier {
        dpi: 150,
        ..Tier::clean()
    };
    raster::scan(&f.laid, 0, noto, &tier, seed, 1)
}

/// A PDF made of hand-built objects; object 1 must be the catalog.
fn custom_pdf(w: Writer, title: &str) -> Vec<u8> {
    w.serialize(&Trailer {
        root: 1,
        info: None,
        id: file_id(title),
        encrypt: None,
    })
    .bytes
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend_from_slice(&h.finalize().to_be_bytes());
}

fn png_raw(width: u32, height: u32, bit_depth: u8, idat: &[u8]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.extend_from_slice(&[bit_depth, 0, 0, 0, 0]);
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", idat);
    png_chunk(&mut out, b"IEND", &[]);
    out
}

/// Zlib stream of `n` copies of `byte`, written in chunks (no large buffer).
fn zlib_repeat(byte: u8, n: u64, level: u32) -> Vec<u8> {
    let mut e = ZlibEncoder::new(Vec::new(), Compression::new(level));
    let chunk = vec![byte; 1 << 20];
    let mut left = n;
    while left > 0 {
        let k = left.min(chunk.len() as u64) as usize;
        e.write_all(&chunk[..k]).expect("in-memory write");
        left -= k as u64;
    }
    e.finish().expect("in-memory write")
}

pub fn build(dir: &Path, generator: Generator, noto: &Noto) -> Result<usize, Vec<String>> {
    std::fs::create_dir_all(dir).map_err(|e| vec![format!("{}: {e}", dir.display())])?;
    let mut o = Out {
        files: Vec::new(),
        dir: dir.to_path_buf(),
    };
    let seed = generator.seed;
    use Outcome::{Fail, Success};

    // Empty.
    o.add("empty.pdf", &[], Cat::Empty, Fail, "Zero bytes.")?;
    o.add("empty.png", &[], Cat::Empty, Fail, "Zero bytes.")?;

    // Blank.
    let mut blank = Laid::default();
    for _ in 0..3 {
        blank.pages.push(crate::layout::Page {
            width: A4.0,
            height: A4.1,
            ..Default::default()
        });
    }
    o.add(
        "blank-pages.pdf",
        &pdf::render(&blank, PdfOpts::new("blank-pages")),
        Cat::Blank,
        Success,
        "Three valid pages with no content. Processed with zero text.",
    )?;

    // Encrypted.
    let enc = |user: &str| Encryption {
        user_password: user.as_bytes().to_vec(),
        owner_password: b"td3-owner".to_vec(),
        permissions: -3904,
    };
    let laid = text_laid(&["This file is encrypted. Page {n}."], 1);
    let user = pdf::render(
        &laid,
        PdfOpts {
            encrypt: Some(enc("velmora")),
            ..PdfOpts::new("encrypted-user-password")
        },
    );
    o.add("encrypted-user-password.pdf", &user, Cat::Encrypted, Fail, "RC4-128 (R3) with a user password (\"velmora\", for manual checks). Cannot be opened without it.")?;
    let owner = pdf::render(
        &laid,
        PdfOpts {
            encrypt: Some(enc("")),
            ..PdfOpts::new("encrypted-owner-only")
        },
    );
    o.add("encrypted-owner-only.pdf", &owner, Cat::Encrypted, Success, "RC4-128 (R3) with an owner password only and no permissions set. Opens with the empty user password; text is read.")?;

    // Corrupt.
    let good = text_pdf(
        "corrupt-xref",
        &["The xref offsets of this file are wrong. Page {n}."],
        2,
    );
    o.add("corrupt-xref-offsets.pdf", &shift_xref(&good, 7), Cat::Corrupt, Success, "Every xref offset is 7 bytes off. The objects are intact, so readers rebuild the xref by scanning. ASSUMPTION: rebuilding is expected.")?;
    let mut rng = ChaCha20Rng::seed_from_u64(file_seed(seed, "td3/garbage"));
    let mut garbage = b"%PDF-1.4\n".to_vec();
    let mut tail = vec![0u8; 16 * 1024];
    rng.fill_bytes(&mut tail);
    garbage.extend_from_slice(&tail);
    o.add(
        "garbage-after-header.pdf",
        &garbage,
        Cat::Corrupt,
        Fail,
        "A PDF header followed by 16 KiB of random bytes. No objects.",
    )?;

    // Truncated.
    let full = text_pdf("truncated", &["This file is cut short. Page {n}."], 2);
    let cut = find(&full, b"stream\n").expect("a stream") + 20;
    o.add(
        "truncated.pdf",
        &full[..cut],
        Cat::Truncated,
        Fail,
        "Cut inside the first content stream: no page objects, no xref, no trailer.",
    )?;
    let sc = small_scan(
        noto,
        file_seed(seed, "td3/truncated.png"),
        "Truncated image test",
    );
    let png = raster::png_gray(sc.width, sc.height, &sc.pixels, sc.dpi);
    o.add("truncated.png", &png[..png.len() / 2], Cat::Truncated, Fail, "PNG cut in half: the image data ends early and IEND is missing. ASSUMPTION: no partial decode.")?;
    let jpg = raster::jpeg_gray(sc.width, sc.height, &sc.pixels, sc.dpi, 85);
    o.add(
        "truncated.jpg",
        &jpg[..jpg.len() / 2],
        Cat::Truncated,
        Fail,
        "JPEG cut in half: no end-of-image marker. ASSUMPTION: no partial decode.",
    )?;

    // Huge.
    let huge = text_pdf(
        "pages-2000",
        &["Page {n} of 2000. Capacity test file."],
        2000,
    );
    o.add(
        "pages-2000.pdf",
        &huge,
        Cat::Huge,
        Success,
        "2,000 text pages (NFR-CAP-04). Processed within the per-file limits.",
    )?;

    // Wrong extension.
    let small = text_pdf("pdf-named-png", &["A PDF file with a .png name."], 1);
    o.add("pdf-named.png", &small, Cat::WrongExtension, Success, "A valid PDF named .png. Detected by its magic bytes and read as a PDF. ASSUMPTION: content sniffing wins over the extension.")?;
    let sc = small_scan(
        noto,
        file_seed(seed, "td3/jpeg-named.pdf"),
        "A JPEG file with a PDF name",
    );
    o.add("jpeg-named.pdf", &raster::jpeg_gray(sc.width, sc.height, &sc.pixels, sc.dpi, 85), Cat::WrongExtension, Success, "A valid JPEG named .pdf. Detected by its magic bytes and OCRed as an image. ASSUMPTION: content sniffing wins over the extension.")?;
    o.add("text-named.pdf", b"This is a plain text file with a .pdf name.\nIt has no PDF header.\n", Cat::WrongExtension, Fail, "Plain text named .pdf. No magic bytes, so the extension decides and the PDF parser rejects it. ASSUMPTION.")?;

    // Memory bombs (NFR-REL-07: one file may not use more than 1 GB).
    o.add("bomb-double-flate.pdf", &double_flate_bomb(), Cat::MemoryBomb, Fail, "Content stream with /Filter [/FlateDecode /FlateDecode] that expands to 2 GiB of spaces. Over the memory limit.")?;
    o.add("bomb-image-dimensions.pdf", &huge_image_pdf(), Cat::MemoryBomb, Fail, "An image-only page whose image claims 100,000 x 100,000 pixels (10 GB as 8-bit gray). The declared size is over the memory limit, so the image is refused before decoding. Some renderers draw it scaled down; the expected outcome is the app's guard, not theirs.")?;
    let idat = zlib_repeat(0, 1000, 9);
    o.add("png-lying-ihdr.png", &png_raw(100_000, 100_000, 8, &idat), Cat::MemoryBomb, Fail, "IHDR claims 100,000 x 100,000 pixels; the image data holds 1,000 bytes. Rejected before allocating.")?;
    o.add("png-decompression-bomb.png", &png_bomb(), Cat::MemoryBomb, Fail, "A real 1-bit 60,000 x 60,000 PNG of zeros: ~0.5 MB on disk, 3.6 GB as 8-bit pixels. Over the memory limit.")?;

    // Endless loops.
    o.add("loop-page-tree.pdf", &page_tree_cycle(), Cat::EndlessLoop, Fail, "Two /Pages nodes that list each other as kids. No leaf pages; a reader must detect the cycle.")?;
    let base = text_pdf(
        "loop-xref-prev",
        &["The xref chain of this file points to itself."],
        1,
    );
    o.add("loop-xref-prev.pdf", &xref_prev_loop(&base), Cat::EndlessLoop, Success, "An incremental-update xref whose /Prev points to itself. The objects are intact; a reader must stop following the chain.")?;
    o.add("loop-form-xobject.pdf", &form_cycle(), Cat::EndlessLoop, Success, "A Form XObject that draws itself. The page text sits outside the form; a reader must cap the recursion.")?;

    o.files.sort_by(|a, b| a.path.cmp(&b.path));
    let n = o.files.len();
    let manifest = Td3Manifest {
        schema_version: 1,
        generator,
        files: o.files,
    };
    let mut json = serde_json::to_string_pretty(&manifest).expect("manifest serializes");
    json.push('\n');
    std::fs::write(dir.join("expected.json"), json)
        .map_err(|e| vec![format!("expected.json: {e}")])?;
    Ok(n + 1)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn rfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).rposition(|w| w == needle)
}

/// Adds `by` to every in-use xref offset (keeps the 10-digit width).
fn shift_xref(pdf: &[u8], by: usize) -> Vec<u8> {
    let start = rfind(pdf, b"\nxref\n").expect("xref") + 1;
    let end = find(&pdf[start..], b"trailer").expect("trailer") + start;
    let table = std::str::from_utf8(&pdf[start..end]).expect("ascii xref");
    let shifted: String = table
        .split_inclusive('\n')
        .map(|l| match l.strip_suffix(" 00000 n \n") {
            Some(off) => format!(
                "{:010} 00000 n \n",
                off.parse::<usize>().expect("offset") + by
            ),
            None => l.to_string(),
        })
        .collect();
    [&pdf[..start], shifted.as_bytes(), &pdf[end..]].concat()
}

/// Appends an incremental-update section whose trailer `/Prev` is its own xref offset.
fn xref_prev_loop(pdf: &[u8]) -> Vec<u8> {
    let trailer = std::str::from_utf8(&pdf[rfind(pdf, b"trailer").expect("trailer")..])
        .expect("ascii trailer");
    let size: String = trailer
        .split("/Size ")
        .nth(1)
        .expect("/Size")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let id: String = trailer
        .split("/ID ")
        .nth(1)
        .expect("/ID")
        .split(']')
        .next()
        .unwrap()
        .to_string()
        + "]";
    let mut out = pdf.to_vec();
    let x = out.len();
    write!(out, "xref\n0 1\n0000000000 65535 f \ntrailer\n<< /Size {size} /Root 1 0 R /Prev {x} /ID {id} >>\nstartxref\n{x}\n%%EOF\n").unwrap();
    out
}

fn double_flate_bomb() -> Vec<u8> {
    let once = zlib_repeat(b' ', 2 << 30, 9);
    let twice = flate(&once);
    let mut w = Writer::default();
    let catalog = w.reserve();
    let pages = w.reserve();
    let cs = w.add(Obj::Stream {
        dict: "/Filter [/FlateDecode /FlateDecode]".into(),
        data: twice,
    });
    let page = w.raw(format!("<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 595 842] /Resources << >> /Contents {cs} 0 R >>"));
    w.set(
        pages,
        Obj::Raw(format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>").into_bytes()),
    );
    w.set(
        catalog,
        Obj::Raw(format!("<< /Type /Catalog /Pages {pages} 0 R >>").into_bytes()),
    );
    custom_pdf(w, "bomb-double-flate")
}

fn huge_image_pdf() -> Vec<u8> {
    let mut w = Writer::default();
    let catalog = w.reserve();
    let pages = w.reserve();
    let img = w.add(Obj::Stream {
        dict: "/Type /XObject /Subtype /Image /Width 100000 /Height 100000 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode".into(),
        data: zlib_repeat(0xFF, 4096, 9),
    });
    let cs = w.add(Obj::Stream {
        dict: "/Filter /FlateDecode".into(),
        data: flate(b"q 595 0 0 842 0 0 cm /Im1 Do Q\n"),
    });
    let page = w.raw(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 595 842] /Resources << /XObject << /Im1 {img} 0 R >> >> /Contents {cs} 0 R >>"
    ));
    w.set(
        pages,
        Obj::Raw(format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>").into_bytes()),
    );
    w.set(
        catalog,
        Obj::Raw(format!("<< /Type /Catalog /Pages {pages} 0 R >>").into_bytes()),
    );
    custom_pdf(w, "bomb-image-dimensions")
}

/// 1-bit gray, 60,000 × 60,000, all zero: each scanline is a filter byte plus 7,500 bytes.
fn png_bomb() -> Vec<u8> {
    const W: u32 = 60_000;
    let line = (W / 8) as u64 + 1;
    png_raw(W, W, 1, &zlib_repeat(0, line * W as u64, 9))
}

fn page_tree_cycle() -> Vec<u8> {
    let mut w = Writer::default();
    let catalog = w.reserve();
    let a = w.reserve();
    let b = w.reserve();
    w.set(
        a,
        Obj::Raw(format!("<< /Type /Pages /Kids [{b} 0 R] /Count 1 >>").into_bytes()),
    );
    w.set(
        b,
        Obj::Raw(
            format!("<< /Type /Pages /Parent {a} 0 R /Kids [{a} 0 R] /Count 1 >>").into_bytes(),
        ),
    );
    w.set(
        catalog,
        Obj::Raw(format!("<< /Type /Catalog /Pages {a} 0 R >>").into_bytes()),
    );
    custom_pdf(w, "loop-page-tree")
}

fn form_cycle() -> Vec<u8> {
    let mut w = Writer::default();
    let catalog = w.reserve();
    let pages = w.reserve();
    let fonts = add_fonts(&mut w);
    let form = w.reserve();
    w.set(
        form,
        Obj::Stream {
            dict: format!("/Type /XObject /Subtype /Form /BBox [0 0 595 842] /Resources << /XObject << /Fm1 {form} 0 R >> >> /Filter /FlateDecode"),
            data: flate(b"q 0.9 0 0 0.9 10 10 cm /Fm1 Do Q\n"),
        },
    );
    let content =
        b"BT /F1 12 Tf 72 760 Td (A form on this page draws itself.) Tj ET\n/Fm1 Do\n".to_vec();
    let cs = w.add(Obj::Stream {
        dict: "/Filter /FlateDecode".into(),
        data: flate(&content),
    });
    let page = w.raw(format!(
        "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 595 842] /Resources << {fonts} /XObject << /Fm1 {form} 0 R >> >> /Contents {cs} 0 R >>"
    ));
    w.set(
        pages,
        Obj::Raw(format!("<< /Type /Pages /Kids [{page} 0 R] /Count 1 >>").into_bytes()),
    );
    w.set(
        catalog,
        Obj::Raw(format!("<< /Type /Catalog /Pages {pages} 0 R >>").into_bytes()),
    );
    custom_pdf(w, "loop-form-xobject")
}
