//! TD-1 segments for extraction, each with its gold name keys (`docs/09-model-check.md` §3.1).
//!
//! - Every page in `page-text.jsonl` (text PDF, scan, scanned PDF, mixed, OCR layer) is one
//!   segment, except the 500-page ledger: only its pages with gold mentions, plus 20 seeded
//!   filler pages as negatives. The text is the reference text, so OCR noise is not measured.
//! - TXT/MD notes, decoded by their ground-truth encoding, split at line ends at ≤ 2,000 chars.
//! - EML and MBOX messages: From/To/Cc/Subject lines plus the text body (HTML-only bodies as
//!   text, tags stripped by `mail-parser`).
//! - Left out: CSV/JSON (column mapping, Layer 1), DOCX and HTML.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use mk_bench::{DocPart, EntityKind, FileKind, GroundTruth, Location, Subject, Unit};
use serde::{Deserialize, Serialize};

pub const LEDGER_DOC: &str = "d-ledger";
pub const LEDGER_FILLER: usize = 20;
pub const NOTE_CHUNK_CHARS: usize = 2000;
const EMAIL_HEADERS: [&str; 4] = ["From", "To", "Cc", "Subject"];

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    TextPdf,
    Scan,
    OcrLayer,
    Notes,
    Email,
}

impl Source {
    pub const ALL: [Source; 5] = [
        Source::TextPdf,
        Source::Scan,
        Source::OcrLayer,
        Source::Notes,
        Source::Email,
    ];
    pub fn name(self) -> &'static str {
        match self {
            Source::TextPdf => "text PDF",
            Source::Scan => "scan",
            Source::OcrLayer => "OCR layer",
            Source::Notes => "notes",
            Source::Email => "email",
        }
    }
}

/// One gold name mention inside a segment.
#[derive(Serialize, Clone, Debug)]
pub struct Gold {
    pub mention: String,
    pub kind: EntityKind,
    pub name_key: String,
    pub raw: String,
}

#[derive(Serialize, Clone, Debug)]
pub struct Segment {
    pub id: String,
    pub document: String,
    pub source: Source,
    pub text: String,
    pub gold: Vec<Gold>,
}

impl Segment {
    pub fn gold_keys(&self) -> BTreeSet<String> {
        self.gold.iter().map(|g| g.name_key.clone()).collect()
    }
}

#[derive(Serialize, Default, Debug)]
pub struct Coverage {
    /// Synthetic name mentions in the ground truth.
    pub name_mentions: usize,
    /// Of those, mentions inside a built segment and visible in its text.
    pub covered: usize,
    /// Mentions in a built segment whose raw text is not in the segment text (left out of gold).
    pub not_visible: Vec<String>,
    /// Mentions not covered, by document kind (CSV, JSON, DOCX, HTML are left out on purpose).
    pub not_covered: BTreeMap<String, usize>,
}

pub struct Segments {
    pub list: Vec<Segment>,
    pub coverage: Coverage,
}

/// splitmix64: a tiny seeded generator, so sampling needs no extra crate.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }
    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Fisher–Yates; returns the first `n` of a shuffled copy.
    pub fn sample<T: Clone>(&mut self, items: &[T], n: usize) -> Vec<T> {
        let mut v = items.to_vec();
        for i in (1..v.len()).rev() {
            let j = (self.next() % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
        v.truncate(n);
        v
    }
}

/// All whitespace removed: the visibility check for gold mentions (a name split across lines
/// is printed with a line break, but its `raw` has none).
pub fn compact(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Decodes a TXT/MD file by its ground-truth encoding.
pub fn decode(bytes: &[u8], encoding: &str) -> Result<String, String> {
    let enc = match encoding {
        "utf-8" | "utf-8-bom" => encoding_rs::UTF_8,
        "utf-16le-bom" => encoding_rs::UTF_16LE,
        "windows-1252" => encoding_rs::WINDOWS_1252,
        other => return Err(format!("unknown encoding {other:?}")),
    };
    let (text, had_errors) = enc.decode_with_bom_removal(bytes);
    if had_errors {
        return Err(format!("{encoding}: malformed bytes"));
    }
    Ok(text.into_owned())
}

/// Splits decoded text into chunks of whole lines, each ≤ `max` chars where a line allows.
/// Returns (first line, last line, text), lines 1-based.
pub fn chunk_lines(text: &str, max: usize) -> Vec<(u32, u32, String)> {
    let mut out = Vec::new();
    // (first line, text, chars)
    let mut cur: Option<(u32, String, usize)> = None;
    let mut last = 0;
    for (i, line) in text.lines().enumerate() {
        last = i as u32 + 1;
        let len = line.chars().count();
        match &mut cur {
            Some((_, t, c)) if *c + 1 + len <= max => {
                t.push('\n');
                t.push_str(line);
                *c += 1 + len;
            }
            _ => {
                if let Some((a, t, _)) = cur.take() {
                    out.push((a, last - 1, t));
                }
                cur = Some((last, line.to_string(), len));
            }
        }
    }
    if let Some((a, t, _)) = cur {
        out.push((a, last, t));
    }
    out
}

fn kind_of(name_key: &str) -> EntityKind {
    if name_key.starts_with("org:") {
        EntityKind::Org
    } else {
        EntityKind::Person
    }
}

fn addresses(a: Option<&mail_parser::Address>) -> String {
    a.map(|a| {
        a.iter()
            .map(|x| match (x.name(), x.address()) {
                (Some(n), Some(e)) => format!("{n} <{e}>"),
                (Some(n), None) => n.to_string(),
                (None, Some(e)) => e.to_string(),
                (None, None) => String::new(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_default()
}

/// Header lines and text body of one message, as the segment text.
pub fn message_text(raw: &[u8]) -> Result<String, String> {
    let msg = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or("message does not parse")?;
    let mut out = String::new();
    for (name, value) in [
        ("From", addresses(msg.from())),
        ("To", addresses(msg.to())),
        ("Cc", addresses(msg.cc())),
        ("Subject", msg.subject().unwrap_or("").to_string()),
    ] {
        if !value.is_empty() {
            out.push_str(&format!("{name}: {value}\n"));
        }
    }
    out.push('\n');
    for i in 0..msg.text_body_count() {
        if let Some(t) = msg.body_text(i) {
            out.push_str(t.trim_end());
            out.push('\n');
        }
    }
    Ok(out.replace("\r\n", "\n"))
}

fn mbox_message(path: &Path, index: u32) -> Result<Vec<u8>, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    mail_parser::mailbox::mbox::MessageIterator::new(std::io::BufReader::new(f))
        .nth(index as usize)
        .ok_or_else(|| format!("{}: no message {index}", path.display()))?
        .map(|m| m.unwrap_contents())
        .map_err(|_| format!("{}: message {index} unreadable", path.display()))
}

impl Segments {
    pub fn build(root: &Path, gt: &GroundTruth) -> Result<Self, String> {
        let doc_kind: HashMap<&str, FileKind> = gt
            .documents
            .iter()
            .map(|d| (d.id.as_str(), d.kind))
            .collect();
        // Name mentions by document.
        let mut by_doc: HashMap<&str, Vec<&mk_bench::Mention>> = HashMap::new();
        for m in &gt.mentions {
            if matches!(m.subject, Subject::Entity { .. }) {
                by_doc.entry(m.document.as_str()).or_default().push(m);
            }
        }
        let gold_of = |m: &mk_bench::Mention| {
            let Subject::Entity { name_key, .. } = &m.subject else {
                unreachable!()
            };
            Gold {
                mention: m.id.clone(),
                kind: kind_of(name_key),
                name_key: name_key.clone(),
                raw: m.raw.clone(),
            }
        };

        let mut list = Vec::new();

        // Pages. The ledger keeps its gold pages and a seeded filler sample.
        let page_gold = |doc: &str, page: u32| -> Vec<Gold> {
            by_doc
                .get(doc)
                .into_iter()
                .flatten()
                .filter(|m| match m.location {
                    Location::PdfText { page: p, .. } | Location::Scan { page: p, .. } => p == page,
                    _ => false,
                })
                .map(|m| gold_of(m))
                .collect()
        };
        let ledger_gold: Vec<u32> = gt
            .page_text
            .iter()
            .filter(|p| p.document == LEDGER_DOC && !page_gold(LEDGER_DOC, p.page).is_empty())
            .map(|p| p.page)
            .collect();
        let ledger_rest: Vec<u32> = gt
            .page_text
            .iter()
            .filter(|p| p.document == LEDGER_DOC && !ledger_gold.contains(&p.page))
            .map(|p| p.page)
            .collect();
        let filler: BTreeSet<u32> = Rng::new(1)
            .sample(&ledger_rest, LEDGER_FILLER)
            .into_iter()
            .collect();
        for p in &gt.page_text {
            if p.document == LEDGER_DOC
                && !ledger_gold.contains(&p.page)
                && !filler.contains(&p.page)
            {
                continue;
            }
            let source = match doc_kind.get(p.document.as_str()) {
                Some(FileKind::PdfOcrLayer) => Source::OcrLayer,
                Some(FileKind::Image | FileKind::PdfScan) => Source::Scan,
                _ if p.unit == Unit::Px => Source::Scan,
                _ => Source::TextPdf,
            };
            list.push(Segment {
                id: format!("{}#p{}", p.document, p.page),
                document: p.document.clone(),
                source,
                text: p.text.clone(),
                gold: page_gold(&p.document, p.page),
            });
        }

        // Notes and email, from the files on disk.
        let file_of: HashMap<&str, &mk_bench::FileEntry> =
            gt.files.iter().map(|f| (f.path.as_str(), f)).collect();
        for d in &gt.documents {
            let file = file_of
                .get(d.file.as_str())
                .ok_or_else(|| format!("no file {}", d.file))?;
            if file.origin != mk_bench::Origin::Synthetic || d.parent.is_some() {
                continue;
            }
            let path = root.join(&d.file);
            match d.kind {
                FileKind::Txt | FileKind::Md => {
                    let bytes =
                        std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                    let text = decode(&bytes, file.encoding.as_deref().unwrap_or("utf-8"))?;
                    for (a, b, chunk) in chunk_lines(&text, NOTE_CHUNK_CHARS) {
                        let gold = by_doc
                            .get(d.id.as_str())
                            .into_iter()
                            .flatten()
                            .filter(|m| matches!(m.location, Location::TextLine { line, .. } if (a..=b).contains(&line)))
                            .map(|m| gold_of(m))
                            .collect();
                        list.push(Segment {
                            id: format!("{}#l{a}-{b}", d.id),
                            document: d.id.clone(),
                            source: Source::Notes,
                            text: chunk,
                            gold,
                        });
                    }
                }
                FileKind::Eml | FileKind::Mbox => {
                    let raw = match &d.part {
                        DocPart::MboxMessage { index } => mbox_message(&path, *index)?,
                        _ => {
                            std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
                        }
                    };
                    let gold = by_doc
                        .get(d.id.as_str())
                        .into_iter()
                        .flatten()
                        .filter(|m| match &m.location {
                            Location::EmailHeader { header, .. } => {
                                EMAIL_HEADERS.contains(&header.as_str())
                            }
                            Location::MimePart { .. } => true,
                            _ => false,
                        })
                        .map(|m| gold_of(m))
                        .collect();
                    list.push(Segment {
                        id: d.id.clone(),
                        document: d.id.clone(),
                        source: Source::Email,
                        text: message_text(&raw)?,
                        gold,
                    });
                }
                _ => {}
            }
        }

        // Visibility: a gold mention whose text is not in its segment is reported, not scored.
        let mut coverage = Coverage::default();
        for s in &mut list {
            let text = compact(&s.text);
            s.gold.retain(|g| {
                let seen = text.contains(&compact(&g.raw));
                if !seen {
                    coverage
                        .not_visible
                        .push(format!("{} {} {:?}", s.id, g.mention, g.raw));
                }
                seen
            });
        }
        let covered: BTreeSet<&str> = list
            .iter()
            .flat_map(|s| s.gold.iter().map(|g| g.mention.as_str()))
            .collect();
        coverage.covered = covered.len();
        let synthetic_docs: BTreeSet<&str> = gt
            .documents
            .iter()
            .filter(|d| {
                file_of
                    .get(d.file.as_str())
                    .is_some_and(|f| f.origin == mk_bench::Origin::Synthetic)
            })
            .map(|d| d.id.as_str())
            .collect();
        for (doc, ms) in &by_doc {
            if !synthetic_docs.contains(doc) {
                continue;
            }
            coverage.name_mentions += ms.len();
            for _ in ms.iter().filter(|m| !covered.contains(m.id.as_str())) {
                let kind = serde_json::to_value(doc_kind[doc]).expect("serializes");
                *coverage
                    .not_covered
                    .entry(kind.as_str().unwrap_or("?").to_string())
                    .or_default() += 1;
            }
        }
        Ok(Segments { list, coverage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_lines_keeps_whole_lines() {
        let text = "aaaa\nbbbb\ncccc\n";
        assert_eq!(
            chunk_lines(text, 9),
            vec![(1, 2, "aaaa\nbbbb".to_string()), (3, 3, "cccc".to_string())]
        );
        assert_eq!(
            chunk_lines(text, 100),
            vec![(1, 3, "aaaa\nbbbb\ncccc".to_string())]
        );
        // A line longer than the limit stays whole.
        assert_eq!(chunk_lines("x\nyyyyyy\nz", 3).len(), 3);
        assert_eq!(
            chunk_lines("\r\nx\r\n", 100),
            vec![(1, 2, "\nx".to_string())]
        );
    }

    #[test]
    fn decode_by_encoding() {
        assert_eq!(decode(b"\xEF\xBB\xBFhi", "utf-8-bom").unwrap(), "hi");
        assert_eq!(decode(b"\xFF\xFEh\0i\0", "utf-16le-bom").unwrap(), "hi");
        assert_eq!(decode(b"\xA3 5 \x80", "windows-1252").unwrap(), "£ 5 €");
        assert!(decode(b"\xFF", "utf-8").is_err());
    }

    #[test]
    fn message_text_has_headers_and_body() {
        let raw = b"From: Gregor Almqvist <g@n.example>\r\nTo: d@v.example\r\nSubject: =?utf-8?q?Tender_=E2=80=93_007?=\r\nContent-Type: text/html\r\n\r\n<p>Dear <b>Mr Okonkwo-Hart</b>,</p>\r\n";
        let t = message_text(raw).unwrap();
        assert!(
            t.starts_with(
                "From: Gregor Almqvist <g@n.example>\nTo: d@v.example\nSubject: Tender – 007\n\n"
            ),
            "{t}"
        );
        assert!(t.contains("Mr Okonkwo-Hart"), "{t}");
        assert!(!t.contains("<b>"), "{t}");
    }

    #[test]
    fn rng_is_seeded() {
        let v: Vec<u32> = (0..100).collect();
        assert_eq!(Rng::new(1).sample(&v, 5), Rng::new(1).sample(&v, 5));
        assert_ne!(Rng::new(1).sample(&v, 5), Rng::new(2).sample(&v, 5));
    }
}
