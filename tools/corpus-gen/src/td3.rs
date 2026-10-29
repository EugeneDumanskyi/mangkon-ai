//! TD-3: broken and edge-case PDF and image files (v1), TXT and CSV files (v2, M2c), EML, MBOX,
//! DOCX, JSON and HTML files (v3, M3a), multi-page TIFF, WEBP, a PDF over the page limit and an
//! email with a scan (v4, M3b), with `expected.json` (`docs/07-test-corpus.md` §6). Every file
//! must finish within the per-file limits (NFR-REL-06/07) and never stop the batch (NFR-REL-01).

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
    scan_at(noto, seed, text, 150)
}

/// A small clean scan of a few words at `dpi`.
fn scan_at(noto: &Noto, seed: u64, text: &str, dpi: u32) -> raster::Scanned {
    let mut f = Flow::new(noto, (300.0, 200.0), 24.0);
    f.para(Font::Sans, 14.0, &Rich::plain_text(text));
    let tier = Tier {
        dpi,
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

    // Empty. v4: success, like every empty file. The store keeps one content per hash, so all
    // empty files share one outcome, and empty TXT, CSV, EML, MBOX and JSON files are read.
    o.add("empty.pdf", &[], Cat::Empty, Success, "Zero bytes. Processed with one empty document, like every empty file (one content per hash).")?;
    o.add("empty.png", &[], Cat::Empty, Success, "Zero bytes. Processed with one empty document, like every empty file (one content per hash).")?;

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
    o.add("encrypted-user-password.pdf", &user, Cat::Encrypted, Fail, "RC4-128 (R3) with a user password (\"velmora\", for manual checks). Cannot be opened without it: fails with \"PDF is password-protected\" (US-02.1 AC2).")?;
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
        "A PDF header followed by 16 KiB of random bytes. No objects: fails with \"Not a readable PDF\".",
    )?;

    // Truncated.
    let full = text_pdf("truncated", &["This file is cut short. Page {n}."], 2);
    let cut = find(&full, b"stream\n").expect("a stream") + 20;
    o.add(
        "truncated.pdf",
        &full[..cut],
        Cat::Truncated,
        Fail,
        "Cut inside the first content stream: no page objects, no xref, no trailer. Fails with \"Not a readable PDF\".",
    )?;
    let sc = small_scan(
        noto,
        file_seed(seed, "td3/truncated.png"),
        "Truncated image test",
    );
    let png = raster::png_gray(sc.width, sc.height, &sc.pixels, sc.dpi);
    o.add("truncated.png", &png[..png.len() / 2], Cat::Truncated, Fail, "PNG cut in half: the image data ends early and IEND is missing. Fails with \"Not a readable image\". ASSUMPTION: no partial decode.")?;
    let jpg = raster::jpeg_gray(sc.width, sc.height, &sc.pixels, sc.dpi, 85);
    o.add(
        "truncated.jpg",
        &jpg[..jpg.len() / 2],
        Cat::Truncated,
        Fail,
        "JPEG cut in half: no end-of-image marker. Fails with \"Not a readable image\". ASSUMPTION: no partial decode.",
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
    o.add("text-named.pdf", b"This is a plain text file with a .pdf name.\nIt has no PDF header.\n", Cat::WrongExtension, Fail, "Plain text named .pdf. No magic bytes, so the extension decides and the PDF parser rejects it with \"Not a PDF\". ASSUMPTION.")?;

    // Memory bombs (NFR-REL-07: one file may not use more than 1 GB).
    o.add("bomb-double-flate.pdf", &double_flate_bomb(), Cat::MemoryBomb, Fail, "Content stream with /Filter [/FlateDecode /FlateDecode] that expands to 2 GiB of spaces. Stopped over 1 GB: \"Needed too much memory\" (NFR-REL-07).")?;
    o.add("bomb-image-dimensions.pdf", &huge_image_pdf(), Cat::MemoryBomb, Fail, "An image-only page whose image claims 100,000 x 100,000 pixels (10 GB as 8-bit gray). The declared size is over the memory limit, so the image is refused before decoding. Some renderers draw it scaled down; the expected outcome is the app's guard, not theirs: \"Image too large (limit: 100 megapixels)\".")?;
    let idat = zlib_repeat(0, 1000, 9);
    o.add("png-lying-ihdr.png", &png_raw(100_000, 100_000, 8, &idat), Cat::MemoryBomb, Fail, "IHDR claims 100,000 x 100,000 pixels; the image data holds 1,000 bytes. Rejected before allocating: \"Image too large (limit: 100 megapixels)\".")?;
    o.add("png-decompression-bomb.png", &png_bomb(), Cat::MemoryBomb, Fail, "A real 1-bit 60,000 x 60,000 PNG of zeros: ~0.5 MB on disk, 3.6 GB as 8-bit pixels. Refused before decoding: \"Image too large (limit: 100 megapixels)\".")?;

    // Endless loops.
    o.add("loop-page-tree.pdf", &page_tree_cycle(), Cat::EndlessLoop, Fail, "Two /Pages nodes that list each other as kids. No leaf pages; a reader must detect the cycle. Fails with \"Not a readable PDF\".")?;
    let base = text_pdf(
        "loop-xref-prev",
        &["The xref chain of this file points to itself."],
        1,
    );
    o.add("loop-xref-prev.pdf", &xref_prev_loop(&base), Cat::EndlessLoop, Success, "An incremental-update xref whose /Prev points to itself. The objects are intact; a reader must stop following the chain.")?;
    o.add("loop-form-xobject.pdf", &form_cycle(), Cat::EndlessLoop, Success, "A Form XObject that draws itself. The page text sits outside the form; a reader must cap the recursion.")?;

    text_and_csv(&mut o, &png)?;
    text_formats(&mut o, noto, seed)?;
    pages_and_scans(&mut o, noto, seed)?;

    o.files.sort_by(|a, b| a.path.cmp(&b.path));
    // The store keeps one content per hash: files with the same bytes (other than the empty
    // ones) could not have different outcomes.
    let mut seen = std::collections::BTreeMap::new();
    for f in o.files.iter().filter(|f| f.bytes > 0) {
        if let Some(other) = seen.insert(&f.sha256, &f.path) {
            return Err(vec![format!("{} has the same bytes as {other}", f.path)]);
        }
    }
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

/// TD-3 v2 (M2c): TXT and CSV files, and the three fault files of the worker tests. `png` is a
/// valid PNG, reused under a text name.
fn text_and_csv(o: &mut Out, png: &[u8]) -> Result<(), Vec<String>> {
    use Outcome::{Fail, Success};
    o.add(
        "empty.txt",
        &[],
        Cat::Empty,
        Success,
        "Zero bytes. Processed with no segments.",
    )?;
    o.add(
        "empty.csv",
        &[],
        Cat::Empty,
        Success,
        "Zero bytes. Processed with no rows.",
    )?;
    o.add(
        "header-only.csv",
        b"date,description,amount,email\n",
        Cat::Blank,
        Success,
        "A header row and no data rows. Processed; the header is row 0.",
    )?;

    o.add("binary-named.txt", png, Cat::WrongExtension, Fail, "A PNG image named .txt. Not text: fails with \"Unknown text encoding\". ASSUMPTION: no content sniffing for text names.")?;
    let pdf = text_pdf("pdf-named-csv", &["A PDF file with a .csv name."], 1);
    o.add("pdf-named.csv", &pdf, Cat::WrongExtension, Fail, "A PDF with compressed streams named .csv. Not text: fails with \"Unknown text encoding\". ASSUMPTION: no content sniffing for text names.")?;

    let mut latin1 = b"Invoice for M\xfcller GmbH, contact anna.mueller@example.de\n".to_vec();
    latin1.extend_from_slice(b"Stra\xdfe 12, 10115 Berlin\n");
    o.add("invalid-utf8.txt", &latin1, Cat::Corrupt, Success, "Latin-1 bytes (0xFC, 0xDF) that are not UTF-8. Processed: decoded as Windows-1252 (NFR-COMP-06, M3a).")?;
    let mut cut = "Grüße aus Köln, max@example.org\nZweite Zeile ü"
        .as_bytes()
        .to_vec();
    cut.pop();
    o.add("truncated-utf8.txt", &cut, Cat::Truncated, Fail, "UTF-8 cut in the middle of the last character (half of ü). Fails with \"Unknown text encoding\". ASSUMPTION: no repair of a cut tail.")?;
    o.add(
        "nul-bytes.txt",
        b"before\0after nul@example.com\n\0\0\0\nlast line\n",
        Cat::Corrupt,
        Success,
        "Valid UTF-8 with NUL characters inside lines. Processed; NULs are kept as text.",
    )?;

    let mut line = String::with_capacity(16 << 20);
    let mut i = 0usize;
    while line.len() < (16 << 20) - 64 {
        line.push_str(&format!("cell {i} "));
        if i.is_multiple_of(50_000) {
            line.push_str(&format!("long{i}@line.example "));
        }
        i += 1;
    }
    line.push('\n');
    o.add("one-line-16mb.txt", line.as_bytes(), Cat::Huge, Success, "One 16 MB line with an email every 50,000 cells. Processed as one segment within the per-file limits.")?;
    let mut wide = (0..100_000)
        .map(|c| format!("c{c}"))
        .collect::<Vec<_>>()
        .join(",");
    wide.push('\n');
    wide.push_str(
        &(0..100_000)
            .map(|c| {
                if c == 99_999 {
                    "wide@cols.example".to_string()
                } else {
                    c.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join(","),
    );
    wide.push('\n');
    o.add("wide-row-100k-columns.csv", wide.as_bytes(), Cat::Huge, Success, "A header and one row of 100,000 columns; the last cell is an email. Processed within the per-file limits.")?;

    o.add("ragged-rows.csv", b"name,email,amount\nAnna,anna@ragged.example,10\nBen,ben@ragged.example\nCara,cara@ragged.example,30,extra\nDan,dan@ragged.example,40\n", Cat::Corrupt, Success, "Two of four data rows have the wrong number of cells. Processed; the rows are kept, marked malformed, with one warning (US-02.4 AC3).")?;
    o.add("unclosed-quote.csv", b"name,note,email\nAnna,\"fine\",anna@quote.example\nBen,\"never closed,ben@quote.example\nCara,after,cara@quote.example\n", Cat::Corrupt, Success, "A quote opened in row 2 is never closed, so the rest of the file is one cell. Processed with a malformed-row warning. ASSUMPTION: no repair of the quoting.")?;

    // Fault files: honoured only when the core enables test faults (tests); otherwise they are
    // plain text.
    o.add("fault-memory-bomb.txt", b"mk-test-fault: memory-bomb\nhidden@fault.example\n", Cat::MemoryBomb, Fail, "With test faults on, the worker allocates without end. Stopped over 1 GB: \"Needed too much memory\" (NFR-REL-07). Plain text otherwise.")?;
    o.add("fault-endless-loop.txt", b"mk-test-fault: endless-loop\nhidden@fault.example\n", Cat::EndlessLoop, Fail, "With test faults on, the worker spins and sends heartbeats. Stopped at the deadline: \"Took too long to read\" (NFR-REL-06). Plain text otherwise.")?;
    o.add("fault-crash.txt", b"mk-test-fault: crash\nhidden@fault.example\n", Cat::Crash, Fail, "With test faults on, the worker aborts. \"The file could not be read: the reader stopped unexpectedly\" (NFR-REL-01). Plain text otherwise.")?;
    Ok(())
}

/// TD-3 v3 (M3a): EML, MBOX, DOCX, JSON and HTML files. Over-limit CSV and MBOX files
/// (NFR-CAP-07) are not here: the tests make sparse ones. Each binary file named as text is its
/// own image: the store keeps one content per hash, so two files with the same bytes would share
/// one outcome.
fn text_formats(o: &mut Out, noto: &Noto, seed: u64) -> Result<(), Vec<String>> {
    use Outcome::{Fail, Success};
    let png = |name: &str, text: &str| {
        let sc = small_scan(noto, file_seed(seed, &format!("td3/{name}")), text);
        raster::png_gray(sc.width, sc.height, &sc.pixels, sc.dpi)
    };

    // EML.
    o.add(
        "empty.eml",
        &[],
        Cat::Empty,
        Success,
        "Zero bytes. Processed with one empty document, like every empty file (one content per hash).",
    )?;
    o.add("png-named.eml", &png("png-named.eml", "A PNG image with an email name"), Cat::WrongExtension, Fail, "A PNG image named .eml. No headers: fails with \"Not an email\". ASSUMPTION: no content sniffing for text names.")?;
    let b64 = base64_lines(
        "Ledger notes\nWire 4410 sent by cutoff@base64.example\nSecond page of the notes, which is cut\n",
    );
    let cut = &b64[..b64.len() * 2 / 3];
    let truncated = format!(
        "From: Anna Berg <anna@base64.example>\r\nTo: ledger@base64.example\r\nSubject: Notes\r\n\
         Date: Tue, 04 Mar 2025 09:12:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"b1\"\r\n\r\n--b1\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nThe notes are attached.\r\n--b1\r\n\
         Content-Type: text/plain; name=\"notes.txt\"\r\nContent-Disposition: attachment; filename=\"notes.txt\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{cut}"
    );
    o.add("truncated-base64.eml", truncated.as_bytes(), Cat::Truncated, Success, "A text attachment in base64 cut short, with no closing boundary. Processed: the attachment is a child document with the text that decodes. ASSUMPTION: no error for a cut body.")?;
    let unknown =
        b"From: Ben Ortiz <ben@charset.example>\r\nTo: anna@charset.example\r\nSubject: Rates\r\n\
        Date: Wed, 05 Mar 2025 10:00:00 +0000\r\nMIME-Version: 1.0\r\n\
        Content-Type: text/plain; charset=\"x-mangkon-unknown\"\r\n\r\n\
        The rates are in. Reply to rates@charset.example.\r\n";
    o.add("unknown-charset.eml", unknown, Cat::Corrupt, Success, "A text body in a charset nobody knows (x-mangkon-unknown). Processed: the ASCII text is read. ASSUMPTION: an unknown charset is read as UTF-8 with replacement characters.")?;
    o.add("nested-20-deep.eml", &nested_email(20), Cat::Huge, Success, "An email with an attached email, 20 levels deep. Processed: the outer email and 8 levels of attached emails become documents (9 in all); the rest is skipped with one warning. ASSUMPTION: 8 levels.")?;

    // MBOX.
    o.add(
        "empty.mbox",
        &[],
        Cat::Empty,
        Success,
        "Zero bytes. Processed with no messages, like every empty file (one content per hash).",
    )?;
    let single = b"From: Ben Ortiz <ben@single.example>\r\nTo: anna@single.example\r\n\
        Subject: One email\r\nDate: Wed, 05 Mar 2025 11:00:00 +0000\r\n\r\nJust one email.\r\n";
    o.add("no-from-line.mbox", single, Cat::WrongExtension, Fail, "A single email with no \"From \" separator line. Fails with \"Not a mailbox file: it does not start with a \"From \" line\". ASSUMPTION: no guessing.")?;
    let mut mbox = String::new();
    for (i, who) in ["cara", "dan", "eve"].iter().enumerate() {
        mbox.push_str(&format!(
            "From {who}@mbox.example Thu Mar 06 1{i}:00:00 2025\n\
             From: {who}@mbox.example\nTo: desk@mbox.example\nSubject: Message {i}\n\
             Date: Thu, 06 Mar 2025 1{i}:00:00 +0000\n\nBody of message {i}. \
             Contact {who}.desk@mbox.example for the next steps.\nSecond line.\n\n"
        ));
    }
    let cut = mbox.rfind("for the next steps").expect("third body");
    o.add("truncated-last.mbox", &mbox.as_bytes()[..cut], Cat::Truncated, Success, "Three messages; the last is cut inside its body. Processed: three documents, the last with the text up to the cut.")?;

    // DOCX.
    o.add(
        "not-a-zip.docx",
        b"This is a plain text file with a .docx name.\n",
        Cat::WrongExtension,
        Fail,
        "Plain text named .docx. Not a ZIP: fails with \"Word file is damaged\".",
    )?;
    let mut cfb = vec![0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
    cfb.resize(4096, 0);
    o.add("encrypted.docx", &cfb, Cat::Encrypted, Fail, "An OLE compound file header, as Word saves a password-protected document. Fails with \"Word file is password-protected\". ASSUMPTION: no decryption.")?;
    o.add(
        "no-document-xml.docx",
        &zip_of(&[("[Content_Types].xml", b"<Types/>")]),
        Cat::Corrupt,
        Fail,
        "A ZIP with no word/document.xml. Fails with \"Word file is damaged\".",
    )?;
    let good = docx_of(&format!(
        "{}<w:p><w:r><w:t>Cut short. Contact docx@cut.example.</w:t></w:r></w:p>{}",
        DOCX_HEAD, DOCX_TAIL
    ));
    o.add("truncated.docx", &good[..good.len() / 2], Cat::Truncated, Fail, "A valid DOCX cut in half: the ZIP central directory is missing. Fails with \"Word file is damaged\".")?;
    o.add("zip-bomb.docx", &docx_bomb(), Cat::MemoryBomb, Fail, "word/document.xml inflates to 300 MB of blank paragraphs (1 MiB of spaces each) from ~0.3 MB. Read until 256 MB: fails with \"Word file's text is too large to read (over 256 MB)\". ASSUMPTION: 256 MB.")?;

    // JSON.
    let invalid = b"{\n  \"name\": \"Anna Berg\",\n  \"email\": \"anna@json.example\",\n  \"amount\": 12,,\n  \"note\": \"after\"\n}\n";
    o.add(
        "invalid.json",
        invalid,
        Cat::Corrupt,
        Fail,
        "A double comma on line 4. Fails with \"Invalid JSON at line 4, column 16\" (US-02.5 AC2).",
    )?;
    let deep = format!("{}{}", "[".repeat(10_000), "]".repeat(10_000));
    o.add("nested-too-deep.json", deep.as_bytes(), Cat::MemoryBomb, Fail, "Arrays nested 10,000 deep. Fails with \"JSON is nested too deeply\" (over 128 levels) before the stack runs out.")?;
    o.add(
        "empty.json",
        &[],
        Cat::Empty,
        Success,
        "Zero bytes. Processed with one empty document, like every empty file (one content per hash).",
    )?;

    // HTML.
    let hostile = "<!doctype html><html><head><title>Payments</title>\
        <link rel=\"stylesheet\" href=\"http://cdn.tracker.example/s.css\">\
        <script src=\"http://cdn.tracker.example/t.js\"></script>\
        <script>var who = \"hidden@script.example\"; document.write('<p>' + who + '</p>');</script>\
        <style>p { background: url(http://cdn.tracker.example/bg.png); }</style></head>\
        <body><p>Payments go to visible@html.example.</p>\
        <img src=\"http://cdn.tracker.example/pixel.gif\" alt=\"\">\
        <iframe src=\"http://cdn.tracker.example/frame.html\"></iframe>\
        <p onclick=\"alert(1)\">Questions: help@html.example</p></body></html>\n";
    o.add("scripts-and-remote-images.html", hostile.as_bytes(), Cat::Corrupt, Success, "Scripts, a style sheet, a tracking pixel and an iframe, all remote. Processed: only the visible text is read (two addresses); the address in the script is not; nothing is fetched (US-02.3 AC2, NFR-SEC-09).")?;
    o.add(
        "binary-named.html",
        &png("binary-named.html", "A PNG image with an HTML name"),
        Cat::WrongExtension,
        Fail,
        "A PNG image named .html. Not text: fails with \"Unknown text encoding\".",
    )?;
    let unclosed = "<html><body><div><p>First <b>bold <i>open@tags.example\n<p>Second paragraph\
        <ul><li>one<li>two <span>three\n<table><tr><td>cell@tags.example<td>last";
    o.add(
        "unclosed-tags.html",
        unclosed.as_bytes(),
        Cat::Corrupt,
        Success,
        "Tags left open to the end of the file. Processed: the text is read by blocks.",
    )?;
    Ok(())
}

/// TD-3 v4 (M3b): a two-page TIFF, a WEBP, a PDF one page over the limit (NFR-CAP-04) and an
/// email with a scanned attachment (US-02.6 AC4). The scans are 300 DPI so OCR reads them.
fn pages_and_scans(o: &mut Out, noto: &Noto, seed: u64) -> Result<(), Vec<String>> {
    use Outcome::{Fail, Success};
    let a = scan_at(
        noto,
        file_seed(seed, "td3/scan-2-pages-1"),
        "Page one: first@tiff.example",
        300,
    );
    let b = scan_at(
        noto,
        file_seed(seed, "td3/scan-2-pages-2"),
        "Page two: second@tiff.example",
        300,
    );
    let tiff = raster::tiff_gray_pages(
        &[
            (a.width, a.height, &a.pixels),
            (b.width, b.height, &b.pixels),
        ],
        300,
    );
    o.add("scan-2-pages.tiff", &tiff, Cat::Format, Success, "A two-page TIFF, one address per page. Processed: each page is OCR'd into its own segment (US-02.2 AC2).")?;
    let w = scan_at(
        noto,
        file_seed(seed, "td3/scan.webp"),
        "Receipt for webp@scan.example",
        300,
    );
    o.add(
        "scan.webp",
        &raster::webp_gray(w.width, w.height, &w.pixels),
        Cat::Format,
        Success,
        "A lossless WEBP scan with one address. Processed: OCR'd like any image (US-02.2 AC2).",
    )?;
    let over = text_pdf(
        "pages-2001",
        &["Page {n} of 2001. One page over the limit."],
        2001,
    );
    o.add("pages-2001.pdf", &over, Cat::Huge, Fail, "2,001 text pages, one over the limit (NFR-CAP-04). Fails with \"File too large (limit: 2,000 pages)\" after the PDF is opened, before any page is read.")?;
    let s = scan_at(
        noto,
        file_seed(seed, "td3/email-with-scan"),
        "Signed copy: signed@attach.example",
        300,
    );
    let png = raster::png_gray(s.width, s.height, &s.pixels, s.dpi);
    let mut b64 = String::new();
    {
        use base64::Engine as _;
        let all = base64::engine::general_purpose::STANDARD.encode(&png);
        for line in all.as_bytes().chunks(76) {
            b64.push_str(std::str::from_utf8(line).expect("ascii"));
            b64.push_str("\r\n");
        }
    }
    let eml = format!(
        "From: Cara Diaz <cara@attach.example>\r\nTo: desk@attach.example\r\nSubject: Signed copy\r\n\
         Date: Mon, 10 Mar 2025 09:00:00 +0000\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"s1\"\r\n\r\n--s1\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\r\nThe signed copy is attached.\r\n--s1\r\n\
         Content-Type: image/png; name=\"signed.png\"\r\nContent-Disposition: attachment; filename=\"signed.png\"\r\n\
         Content-Transfer-Encoding: base64\r\n\r\n{b64}--s1--\r\n"
    );
    o.add("email-with-scan.eml", eml.as_bytes(), Cat::Format, Success, "An email with a scanned PNG attached. Processed: the scan is a child document, OCR'd, with its one address (US-02.6 AC4, US-02.2 AC2).")?;
    Ok(())
}

const DOCX_HEAD: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
    <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>";
const DOCX_TAIL: &str = "</w:body></w:document>";

/// A deflated ZIP of the given entries, with a fixed timestamp.
fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, data) in entries {
        zip.start_file(*name, zip_opts()).expect("in-memory zip");
        zip.write_all(data).expect("in-memory zip");
    }
    zip.finish().expect("in-memory zip").into_inner()
}

fn zip_opts() -> zip::write::SimpleFileOptions {
    zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9))
        .last_modified_time(zip::DateTime::default())
}

fn docx_of(document_xml: &str) -> Vec<u8> {
    zip_of(&[
        ("[Content_Types].xml", b"<Types/>"),
        ("word/document.xml", document_xml.as_bytes()),
    ])
}

/// `word/document.xml` of 300 MB: 300 paragraphs of 1 MiB of spaces each, written in chunks.
fn docx_bomb() -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    zip.start_file("[Content_Types].xml", zip_opts())
        .expect("in-memory zip");
    zip.write_all(b"<Types/>").expect("in-memory zip");
    zip.start_file("word/document.xml", zip_opts())
        .expect("in-memory zip");
    zip.write_all(DOCX_HEAD.as_bytes()).expect("in-memory zip");
    let spaces = vec![b' '; 1 << 20];
    for _ in 0..300 {
        zip.write_all(b"<w:p><w:r><w:t xml:space=\"preserve\">")
            .expect("in-memory zip");
        zip.write_all(&spaces).expect("in-memory zip");
        zip.write_all(b"</w:t></w:r></w:p>").expect("in-memory zip");
    }
    zip.write_all(DOCX_TAIL.as_bytes()).expect("in-memory zip");
    zip.finish().expect("in-memory zip").into_inner()
}

/// Base64 in 76-character lines with CRLF.
fn base64_lines(text: &str) -> String {
    use base64::Engine as _;
    let b = base64::engine::general_purpose::STANDARD.encode(text);
    b.as_bytes()
        .chunks(76)
        .map(|c| format!("{}\r\n", std::str::from_utf8(c).expect("ascii")))
        .collect()
}

/// An email whose attachment is an email, `depth` levels down; each level has one address.
fn nested_email(depth: u32) -> Vec<u8> {
    let mut inner = format!(
        "From: level{depth}@nested.example\r\nTo: desk@nested.example\r\nSubject: Level {depth}\r\n\
         Date: Fri, 07 Mar 2025 12:00:00 +0000\r\nContent-Type: text/plain\r\n\r\nThe innermost email.\r\n"
    );
    for level in (0..depth).rev() {
        inner = format!(
            "From: level{level}@nested.example\r\nTo: desk@nested.example\r\nSubject: Level {level}\r\n\
             Date: Fri, 07 Mar 2025 12:00:00 +0000\r\nMIME-Version: 1.0\r\n\
             Content-Type: multipart/mixed; boundary=\"n{level}\"\r\n\r\n--n{level}\r\n\
             Content-Type: text/plain\r\n\r\nLevel {level} forwards level {}.\r\n--n{level}\r\n\
             Content-Type: message/rfc822\r\nContent-Disposition: attachment; filename=\"level{}.eml\"\r\n\r\n\
             {inner}\r\n--n{level}--\r\n",
            level + 1,
            level + 1
        );
    }
    inner.into_bytes()
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
