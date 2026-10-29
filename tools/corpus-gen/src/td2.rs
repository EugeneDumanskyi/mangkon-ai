//! TD-2 (NFR §2, `docs/07-test-corpus.md` §TD-2): the scale corpus. `copies` copies of TD-1 under
//! `td2/c000/…`, each file made byte-unique per copy in a way its format allows, and every CSV
//! extended with generated rows so the corpus reaches about 1M segments. Deterministic: copy `k`
//! is the same whatever `copies` is. Not committed (`testdata/` is ignored).

use std::collections::BTreeMap;
use std::path::Path;

use mk_bench::FileKind;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{Rng, SeedableRng};
use serde::Serialize;

use crate::ids;

/// Default copy count: 99 copies × 101 TD-1 files ≈ 10,000 files. ASSUMPTION (NFR TD-2).
pub const DEFAULT_COPIES: usize = 99;
/// Generated rows added to each CSV in each copy. 4 CSVs × 99 copies × 2,500 ≈ 1M rows.
/// ASSUMPTION.
pub const CSV_ROWS: usize = 2_500;
/// Share of generated rows with a counterparty email.
const EMAIL_SHARE: f64 = 0.3;
/// Pool sizes. Keys are drawn with a Zipf-like weight, so a few repeat in every copy and most
/// are rare.
const EMAIL_POOL: usize = 5_000;
const IBAN_POOL: usize = 3_000;
const ZIPF_S: f64 = 1.1;

#[derive(Serialize, Default)]
pub struct Manifest {
    pub generator: String,
    pub seed: u64,
    pub copies: usize,
    pub files: usize,
    pub bytes: u64,
    /// Files per TD-1 kind, all copies.
    pub files_by_kind: BTreeMap<String, usize>,
    pub csv_rows_added: usize,
    pub csv_rows_with_email: usize,
    /// Segments the app would store: TXT/MD non-blank lines, CSV rows, PDF and image pages.
    /// EML, MBOX, JSON, HTML and DOCX are not counted (their segment rules come in M3).
    pub segment_estimate: usize,
}

/// Draws indexes `0..n` with weight `1 / (i + 1)^s`.
struct Zipf {
    cumulative: Vec<f64>,
}

impl Zipf {
    fn new(n: usize, s: f64) -> Self {
        let mut total = 0.0;
        let cumulative = (0..n)
            .map(|i| {
                total += 1.0 / ((i + 1) as f64).powf(s);
                total
            })
            .collect();
        Self { cumulative }
    }

    fn sample(&self, rng: &mut ChaCha20Rng) -> usize {
        let total = *self.cumulative.last().unwrap();
        let x = unit(rng) * total;
        self.cumulative
            .partition_point(|c| *c < x)
            .min(self.cumulative.len() - 1)
    }
}

fn unit(rng: &mut ChaCha20Rng) -> f64 {
    (rng.next_u64() >> 11) as f64 / (1u64 << 53) as f64
}

struct Pools {
    emails: Vec<String>,
    ibans: Vec<String>,
    email_zipf: Zipf,
    iban_zipf: Zipf,
}

impl Pools {
    fn new(seed: u64) -> Self {
        let mut rng = ChaCha20Rng::seed_from_u64(seed ^ 0x7d2_0001);
        const FIRST: [&str; 12] = [
            "anna", "ben", "chloe", "david", "elena", "farid", "greta", "hugo", "ines", "jonas",
            "kira", "liam",
        ];
        const LAST: [&str; 10] = [
            "berg", "costa", "dahl", "ellis", "falk", "grant", "holm", "ivers", "janssen", "kerr",
        ];
        const DOMAINS: [&str; 8] = [
            "harbourfreight.example",
            "northquay.example",
            "saltline.example",
            "copperbay.example",
            "westpier.example",
            "tidewater.example",
            "ironmoor.example",
            "galeport.example",
        ];
        let emails = (0..EMAIL_POOL)
            .map(|i| {
                let f = FIRST[(rng.next_u32() as usize) % FIRST.len()];
                let l = LAST[(rng.next_u32() as usize) % LAST.len()];
                let d = DOMAINS[(rng.next_u32() as usize) % DOMAINS.len()];
                format!("{f}.{l}{i}@{d}")
            })
            .collect();
        let ibans = (0..IBAN_POOL)
            .map(|_| {
                // Made-up bank codes start with `Q` (07-test-corpus.md §7, NFR-LEG-04).
                let bank: String = std::iter::once('Q')
                    .chain((0..3).map(|_| (b'A' + (rng.next_u32() % 26) as u8) as char))
                    .collect();
                let digits: String = (0..14)
                    .map(|_| (b'0' + (rng.next_u32() % 10) as u8) as char)
                    .collect();
                ids::iban("GB", &format!("{bank}{digits}"))
            })
            .collect();
        Self {
            emails,
            ibans,
            email_zipf: Zipf::new(EMAIL_POOL, ZIPF_S),
            iban_zipf: Zipf::new(IBAN_POOL, ZIPF_S),
        }
    }
}

/// Writes TD-2 under `out/td2` from `out/td1`. Returns the manifest, also written as
/// `td2/td2-manifest.json`.
pub fn build(
    out: &Path,
    copies: usize,
    seed: u64,
    generator: &str,
) -> Result<Manifest, Vec<String>> {
    let err = |p: &Path, e: std::io::Error| vec![format!("{}: {e}", p.display())];
    let td1 = out.join("td1");
    let gt = mk_bench::load(&td1.join("ground-truth.json")).map_err(|e| vec![e.to_string()])?;
    let td2 = out.join("td2");
    if td2.exists() {
        std::fs::remove_dir_all(&td2).map_err(|e| err(&td2, e))?;
    }
    let pools = Pools::new(seed);
    let mut m = Manifest {
        generator: generator.into(),
        seed,
        copies,
        ..Default::default()
    };
    let originals: Vec<(&mk_bench::FileEntry, Vec<u8>)> = gt
        .files
        .iter()
        .map(|f| {
            let p = td1.join(&f.path);
            std::fs::read(&p).map(|b| (f, b)).map_err(|e| err(&p, e))
        })
        .collect::<Result<_, _>>()?;
    for k in 0..copies {
        let mut rng = ChaCha20Rng::seed_from_u64(seed ^ (0x7d2_0000_0000 + k as u64));
        let dir = td2.join(format!("c{k:03}"));
        for (f, bytes) in &originals {
            let (data, segments) = vary(f, bytes, k, &pools, &mut rng, &mut m)
                .map_err(|e| vec![format!("{}: {e}", f.path)])?;
            let p = dir.join(&f.path);
            std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| err(&p, e))?;
            std::fs::write(&p, &data).map_err(|e| err(&p, e))?;
            m.files += 1;
            m.bytes += data.len() as u64;
            m.segment_estimate += segments;
            *m.files_by_kind.entry(kind_name(f.kind)).or_default() += 1;
        }
    }
    let json = serde_json::to_string_pretty(&m).expect("manifest serializes") + "\n";
    let p = td2.join("td2-manifest.json");
    std::fs::write(&p, json).map_err(|e| err(&p, e))?;
    Ok(m)
}

fn kind_name(kind: FileKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// `\r\n` if the text uses it, else `\n`.
fn line_end(bytes: &[u8]) -> &'static [u8] {
    if bytes.windows(2).any(|w| w == b"\r\n") {
        b"\r\n"
    } else {
        b"\n"
    }
}

/// The copy of one file, and its segment estimate.
fn vary(
    f: &mk_bench::FileEntry,
    bytes: &[u8],
    k: usize,
    pools: &Pools,
    rng: &mut ChaCha20Rng,
    m: &mut Manifest,
) -> Result<(Vec<u8>, usize), String> {
    let tag = format!("corpus copy {k:03}");
    let pages = f.pages.unwrap_or(1) as usize;
    Ok(match f.kind {
        FileKind::Txt | FileKind::Md => {
            let out = text_with_leading_line(bytes, f.encoding.as_deref(), &tag)?;
            let lines = count_lines(&out, f.encoding.as_deref());
            (out, lines)
        }
        FileKind::Csv => csv_scaled(bytes, k, pools, rng, m)?,
        FileKind::Eml => {
            let mut out = format!("X-Corpus-Copy: {k:03}").into_bytes();
            out.extend_from_slice(line_end(bytes));
            out.extend_from_slice(bytes);
            (out, 0)
        }
        FileKind::Mbox => {
            let first = bytes
                .iter()
                .position(|b| *b == b'\n')
                .ok_or("MBOX without a From_ line")?;
            let mut out = bytes[..=first].to_vec();
            out.extend_from_slice(format!("X-Corpus-Copy: {k:03}").as_bytes());
            out.extend_from_slice(line_end(bytes));
            out.extend_from_slice(&bytes[first + 1..]);
            (out, 0)
        }
        FileKind::Json => {
            let open = bytes
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .filter(|i| bytes[*i] == b'{')
                .ok_or("JSON top level is not an object")?;
            let mut out = bytes[..=open].to_vec();
            out.extend_from_slice(format!("\n  \"corpus_copy\": {k},").as_bytes());
            out.extend_from_slice(&bytes[open + 1..]);
            (out, 0)
        }
        FileKind::Html => {
            let mut out = bytes.to_vec();
            out.extend_from_slice(format!("<!-- {tag} -->\n").as_bytes());
            (out, 0)
        }
        FileKind::PdfText | FileKind::PdfScan | FileKind::PdfMixed | FileKind::PdfOcrLayer => {
            // After %%EOF: readers ignore it, and the next incremental update would follow it.
            let mut out = bytes.to_vec();
            out.extend_from_slice(format!("%copy {k:03}\n").as_bytes());
            (out, pages)
        }
        FileKind::Image => {
            let ext = Path::new(&f.path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let out = match ext.as_str() {
                "png" => png_with_text(bytes, &tag)?,
                "jpg" | "jpeg" => jpeg_with_comment(bytes, &tag)?,
                "tif" | "tiff" => {
                    // Bytes after the last IFD are never read.
                    let mut out = bytes.to_vec();
                    out.extend_from_slice(tag.as_bytes());
                    out
                }
                other => return Err(format!("unknown image type {other}")),
            };
            (out, pages)
        }
        FileKind::Docx => (zip_with_comment(bytes, &tag)?, 0),
    })
}

fn text_with_leading_line(
    bytes: &[u8],
    encoding: Option<&str>,
    tag: &str,
) -> Result<Vec<u8>, String> {
    let line = tag.to_string();
    match encoding {
        Some("utf-8") | Some("windows-1252") | None => {
            let mut out = line.into_bytes();
            out.extend_from_slice(line_end(bytes));
            out.extend_from_slice(bytes);
            Ok(out)
        }
        Some("utf-8-bom") => {
            let body = bytes
                .strip_prefix(b"\xef\xbb\xbf")
                .ok_or("missing UTF-8 BOM")?;
            let mut out = b"\xef\xbb\xbf".to_vec();
            out.extend_from_slice(line.as_bytes());
            out.extend_from_slice(line_end(body));
            out.extend_from_slice(body);
            Ok(out)
        }
        Some("utf-16le-bom") => {
            let body = bytes
                .strip_prefix(b"\xff\xfe")
                .ok_or("missing UTF-16LE BOM")?;
            let crlf = body.windows(4).any(|w| w == b"\r\0\n\0");
            let text = format!("{line}{}", if crlf { "\r\n" } else { "\n" });
            let mut out = b"\xff\xfe".to_vec();
            out.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
            out.extend_from_slice(body);
            Ok(out)
        }
        Some(other) => Err(format!("unknown text encoding {other}")),
    }
}

/// Non-blank lines (the TXT/MD segment rule, `mk-parsers`).
fn count_lines(bytes: &[u8], encoding: Option<&str>) -> usize {
    let text = match encoding {
        Some("utf-16le-bom") => {
            let units: Vec<u16> = bytes[2..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        }
        _ => String::from_utf8_lossy(bytes).into_owned(),
    };
    text.lines().filter(|l| !l.trim().is_empty()).count()
}

/// The CSV with a `counterparty_email` column and `CSV_ROWS` generated rows. Works on bytes:
/// every TD-1 CSV encoding is ASCII-compatible and the added text is ASCII.
fn csv_scaled(
    bytes: &[u8],
    k: usize,
    pools: &Pools,
    rng: &mut ChaCha20Rng,
    m: &mut Manifest,
) -> Result<(Vec<u8>, usize), String> {
    let eol = line_end(bytes);
    let (bom, body) = match bytes.strip_prefix(b"\xef\xbb\xbf") {
        Some(b) => (&b"\xef\xbb\xbf"[..], b),
        None => (&b""[..], bytes),
    };
    let lines: Vec<&[u8]> = body
        .split(|b| *b == b'\n')
        .map(|l| l.strip_suffix(b"\r").unwrap_or(l))
        .filter(|l| !l.is_empty())
        .collect();
    let header = *lines.first().ok_or("empty CSV")?;
    let delim = if header.iter().filter(|b| **b == b';').count()
        > header.iter().filter(|b| **b == b',').count()
    {
        b';'
    } else {
        b','
    };
    let names: Vec<String> = String::from_utf8_lossy(header)
        .split(delim as char)
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    let mut out = bom.to_vec();
    for (i, line) in lines.iter().enumerate() {
        if line.iter().filter(|b| **b == b'"').count() % 2 == 1 {
            return Err("quoted cell spans lines; cannot add a column".into());
        }
        out.extend_from_slice(line);
        out.push(delim);
        if i == 0 {
            out.extend_from_slice(b"counterparty_email");
        }
        out.extend_from_slice(eol);
    }
    let decimal = if delim == b';' { ',' } else { '.' };
    for row in 0..CSV_ROWS {
        let mut cells: Vec<String> = names
            .iter()
            .map(|n| {
                if n.contains("date") || n.contains("datum") {
                    format!(
                        "2025-{:02}-{:02}",
                        1 + rng.next_u32() % 12,
                        1 + rng.next_u32() % 28
                    )
                } else if n.contains("iban") || n.contains("account") {
                    pools.ibans[pools.iban_zipf.sample(rng)].clone()
                } else if ["amount", "bedrag", "debit", "credit", "balance"]
                    .iter()
                    .any(|a| n.contains(a))
                {
                    format!(
                        "{}{decimal}{:02}",
                        rng.next_u32() % 100_000,
                        rng.next_u32() % 100
                    )
                } else {
                    format!("Generated {k:03}-{row:05}")
                }
            })
            .collect();
        let email = if unit(rng) < EMAIL_SHARE {
            m.csv_rows_with_email += 1;
            pools.emails[pools.email_zipf.sample(rng)].clone()
        } else {
            String::new()
        };
        cells.push(email);
        out.extend_from_slice(cells.join(&(delim as char).to_string()).as_bytes());
        out.extend_from_slice(eol);
    }
    m.csv_rows_added += CSV_ROWS;
    Ok((out, lines.len() + CSV_ROWS))
}

/// One CSV of `rows` generated bank-statement rows plus a header, with the TD-2 row filler
/// (dates, IBANs, amounts, 30% counterparty emails, Zipf-spread keys), for spike S2a
/// (`docs/10-walking-skeleton.md` §S2a). Written as it is generated. Returns the bytes written
/// and the rows with an email.
pub fn csv_rows(path: &Path, rows: usize, seed: u64) -> Result<(u64, usize), Vec<String>> {
    use std::io::Write as _;
    let err = |e: std::io::Error| vec![format!("{}: {e}", path.display())];
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let file = std::fs::File::create(path).map_err(err)?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
    let pools = Pools::new(seed);
    let mut rng = ChaCha20Rng::seed_from_u64(seed ^ 0x52a_0001);
    let mut bytes = 0u64;
    let mut with_email = 0;
    let mut line = String::new();
    let header = "date,description,iban,amount,counterparty_email\n";
    out.write_all(header.as_bytes()).map_err(err)?;
    bytes += header.len() as u64;
    for row in 0..rows {
        let email = if unit(&mut rng) < EMAIL_SHARE {
            with_email += 1;
            pools.emails[pools.email_zipf.sample(&mut rng)].as_str()
        } else {
            ""
        };
        line.clear();
        line.push_str(&format!(
            "2025-{:02}-{:02},Generated row {row:07},{},{}.{:02},{email}\n",
            1 + rng.next_u32() % 12,
            1 + rng.next_u32() % 28,
            pools.ibans[pools.iban_zipf.sample(&mut rng)],
            rng.next_u32() % 100_000,
            rng.next_u32() % 100,
        ));
        out.write_all(line.as_bytes()).map_err(err)?;
        bytes += line.len() as u64;
    }
    out.flush().map_err(err)?;
    Ok((bytes, with_email))
}

/// A `tEXt` chunk before `IEND`.
fn png_with_text(bytes: &[u8], text: &str) -> Result<Vec<u8>, String> {
    let iend = bytes
        .windows(4)
        .rposition(|w| w == b"IEND")
        .and_then(|i| i.checked_sub(4))
        .ok_or("PNG without IEND")?;
    let mut data = b"Comment\0".to_vec();
    data.extend_from_slice(text.as_bytes());
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    let mut crc = crc32fast::Hasher::new();
    crc.update(b"tEXt");
    crc.update(&data);
    chunk.extend_from_slice(b"tEXt");
    chunk.extend_from_slice(&data);
    chunk.extend_from_slice(&crc.finalize().to_be_bytes());
    let mut out = bytes[..iend].to_vec();
    out.extend_from_slice(&chunk);
    out.extend_from_slice(&bytes[iend..]);
    Ok(out)
}

/// A `COM` segment right after SOI.
fn jpeg_with_comment(bytes: &[u8], text: &str) -> Result<Vec<u8>, String> {
    if !bytes.starts_with(&[0xFF, 0xD8]) {
        return Err("JPEG without SOI".into());
    }
    let len = (text.len() + 2) as u16;
    let mut out = vec![0xFF, 0xD8, 0xFF, 0xFE];
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(&bytes[2..]);
    Ok(out)
}

/// The ZIP archive comment (end of central directory record), replacing any comment.
fn zip_with_comment(bytes: &[u8], text: &str) -> Result<Vec<u8>, String> {
    let eocd = bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4B, 0x05, 0x06])
        .filter(|i| i + 22 <= bytes.len())
        .ok_or("ZIP without an end of central directory")?;
    let mut out = bytes[..eocd + 20].to_vec();
    out.extend_from_slice(&(text.len() as u16).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(out)
}

/// W3a-2 (M3a): an MBOX of at most `target` bytes (just under it) for the NFR-PERF-23 /
/// NFR-CAP-06 watch.
/// Messages of ~2-4 KB with addresses from the same pools as `csv_rows`; every 20th has a small
/// CSV attachment in base64. Returns (bytes, messages).
pub fn mbox_messages(path: &Path, target: u64, seed: u64) -> Result<(u64, u64), Vec<String>> {
    use base64::Engine as _;
    use std::io::Write as _;
    let err = |e: std::io::Error| vec![format!("{}: {e}", path.display())];
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let file = std::fs::File::create(path).map_err(err)?;
    let mut out = std::io::BufWriter::with_capacity(1 << 20, file);
    let pools = Pools::new(seed);
    let mut rng = ChaCha20Rng::seed_from_u64(seed ^ 0x3a2_0001);
    let mut bytes = 0u64;
    let mut n = 0u64;
    let mut msg = String::new();
    loop {
        let mut email = || pools.emails[pools.email_zipf.sample(&mut rng)].clone();
        let (from, to, cc) = (email(), email(), email());
        let (day, hour) = (1 + rng.next_u32() % 28, rng.next_u32() % 24);
        msg.clear();
        msg.push_str(&format!(
            "From {from} Mon Mar {day:02} {hour:02}:00:00 2025\n\
             From: {from}\nTo: {to}\nCc: {cc}\nSubject: Transfer batch {n}\n\
             Date: Mon, {day:02} Mar 2025 {hour:02}:00:00 +0000\n\
             Message-ID: <batch-{n}@mbox.example>\nMIME-Version: 1.0\n"
        ));
        let paragraphs = 3 + rng.next_u32() % 6;
        let mut body = String::new();
        for p in 0..paragraphs {
            body.push_str(&format!(
                "Paragraph {p} of batch {n}. The transfer to account {} was booked for {}.{:02}; \
                 questions go to {} before the cutoff. The ledger copy follows in the next \
                 message of this thread.\n\n",
                pools.ibans[pools.iban_zipf.sample(&mut rng)],
                rng.next_u32() % 100_000,
                rng.next_u32() % 100,
                pools.emails[pools.email_zipf.sample(&mut rng)],
            ));
        }
        if n.is_multiple_of(20) {
            let mut csv = String::from("date,amount,email\n");
            for r in 0..20 {
                csv.push_str(&format!(
                    "2025-03-{day:02},{r}.00,{}\n",
                    pools.emails[pools.email_zipf.sample(&mut rng)]
                ));
            }
            let b64 = base64::engine::general_purpose::STANDARD.encode(csv);
            msg.push_str(&format!(
                "Content-Type: multipart/mixed; boundary=\"b{n}\"\n\n--b{n}\n\
                 Content-Type: text/plain; charset=utf-8\n\n{body}--b{n}\n\
                 Content-Type: text/csv; name=\"batch-{n}.csv\"\n\
                 Content-Disposition: attachment; filename=\"batch-{n}.csv\"\n\
                 Content-Transfer-Encoding: base64\n\n"
            ));
            for line in b64.as_bytes().chunks(76) {
                msg.push_str(std::str::from_utf8(line).expect("ascii"));
                msg.push('\n');
            }
            msg.push_str(&format!("--b{n}--\n\n"));
        } else {
            msg.push_str("Content-Type: text/plain; charset=utf-8\n\n");
            msg.push_str(&body);
        }
        if bytes + msg.len() as u64 > target {
            break;
        }
        out.write_all(msg.as_bytes()).map_err(err)?;
        bytes += msg.len() as u64;
        n += 1;
    }
    out.flush().map_err(err)?;
    Ok((bytes, n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zipf_prefers_low_indexes() {
        let z = Zipf::new(1000, ZIPF_S);
        let mut rng = ChaCha20Rng::seed_from_u64(1);
        let draws: Vec<usize> = (0..10_000).map(|_| z.sample(&mut rng)).collect();
        let first = draws.iter().filter(|d| **d == 0).count();
        let tail = draws.iter().filter(|d| **d >= 500).count();
        assert!(first > 1000 && tail < 1000, "{first} {tail}");
        assert!(draws.iter().all(|d| *d < 1000));
    }

    #[test]
    fn png_text_chunk_keeps_iend_last() {
        let png = [
            &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..],
            &[0, 0, 0, 0],
            b"IEND",
            &[0xAE, 0x42, 0x60, 0x82],
        ]
        .concat();
        let out = png_with_text(&png, "x").unwrap();
        assert!(out.ends_with(&[0, 0, 0, 0, b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]));
        assert_eq!(&out[12..16], b"tEXt");
    }

    #[test]
    fn zip_comment_replaces_old_one() {
        let mut z = vec![1, 2, 3, 0x50, 0x4B, 0x05, 0x06];
        z.extend([0u8; 16]);
        z.extend([2, 0, b'o', b'k']);
        let out = zip_with_comment(&z, "abc").unwrap();
        assert_eq!(&out[out.len() - 5..], &[3, 0, b'a', b'b', b'c']);
        assert_eq!(out.len(), z.len() + 1);
    }

    #[test]
    fn utf16_leading_line() {
        let body = [0xFF, 0xFE, b'h', 0, b'i', 0, b'\r', 0, b'\n', 0];
        let out = text_with_leading_line(&body, Some("utf-16le-bom"), "c").unwrap();
        assert_eq!(
            out,
            [
                0xFF, 0xFE, b'c', 0, b'\r', 0, b'\n', 0, b'h', 0, b'i', 0, b'\r', 0, b'\n', 0
            ]
        );
    }
}
