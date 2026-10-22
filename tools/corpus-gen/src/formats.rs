//! Writers for the text-based formats: TXT/MD, CSV, JSON, DOCX, HTML, EML/MBOX.
//! Each returns the file bytes (or text) and the placed mentions in the format's own terms.

use std::io::Write as _;

use base64::Engine as _;
use mk_bench::Location;

use crate::rich::{MarkSpec, Rich};
use crate::truth::{Placed, Segment};

#[derive(Clone, Copy, Debug)]
pub enum Enc {
    Utf8,
    Utf8Bom,
    Utf16LeBom,
    Windows1252,
}

impl Enc {
    pub fn name(self) -> &'static str {
        match self {
            Enc::Utf8 => "utf-8",
            Enc::Utf8Bom => "utf-8-bom",
            Enc::Utf16LeBom => "utf-16le-bom",
            Enc::Windows1252 => "windows-1252",
        }
    }

    pub fn encode(self, text: &str) -> Vec<u8> {
        match self {
            Enc::Utf8 => text.as_bytes().to_vec(),
            Enc::Utf8Bom => [&[0xEF, 0xBB, 0xBF][..], text.as_bytes()].concat(),
            Enc::Utf16LeBom => {
                let mut out = vec![0xFF, 0xFE];
                for u in text.encode_utf16() {
                    out.extend_from_slice(&u.to_le_bytes());
                }
                out
            }
            Enc::Windows1252 => {
                let (b, _, bad) = encoding_rs::WINDOWS_1252.encode(text);
                assert!(!bad, "text not representable in Windows-1252");
                b.into_owned()
            }
        }
    }
}

fn segment_of(rich: &Rich, base: usize, marked: &mut Vec<(usize, usize)>) {
    for (_, s, e) in rich.marks() {
        marked.push((base + s, base + e));
    }
}

/// TXT/MD. Returns the text (with `\n`) and `TextLine` locations.
pub fn text_doc(rich: &Rich) -> (String, Placed) {
    let mut placed = Placed::default();
    let mut text = String::new();
    let mut marked = Vec::new();
    for (li, line) in rich.lines().iter().enumerate() {
        if li > 0 {
            text.push('\n');
        }
        let base = text.chars().count();
        for (m, s, e) in line.marks() {
            assert!(m.split_at.is_none(), "split marks are for PDFs and DOCX");
            placed.mentions.push((
                m.clone(),
                Location::TextLine {
                    line: li as u32 + 1,
                    start: s as u32,
                    end: e as u32,
                },
            ));
            marked.push((base + s, base + e));
        }
        text.push_str(&line.plain());
    }
    placed.segments.push(Segment {
        text: text.clone(),
        marked,
    });
    (text, placed)
}

pub fn with_crlf(text: &str) -> String {
    text.replace('\n', "\r\n")
}

/// CSV with RFC 4180 quoting. `rows[0]` is the header row.
pub fn csv(rows: &[Vec<Rich>], delim: char) -> (String, Placed) {
    let mut out = String::new();
    let mut placed = Placed::default();
    let mut seg = String::new();
    let mut marked = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        for (ci, cell) in row.iter().enumerate() {
            if ci > 0 {
                out.push(delim);
                seg.push(delim);
            }
            let v = cell.plain();
            if v.contains(delim) || v.contains('"') || v.contains('\n') {
                out.push('"');
                out.push_str(&v.replace('"', "\"\""));
                out.push('"');
            } else {
                out.push_str(&v);
            }
            let base = seg.chars().count();
            for (m, s, e) in cell.marks() {
                placed.mentions.push((
                    m.clone(),
                    Location::CsvCell {
                        row: ri as u32,
                        column: ci as u32,
                        start: s as u32,
                        end: e as u32,
                    },
                ));
            }
            segment_of(cell, base, &mut marked);
            seg.push_str(&v);
        }
        out.push_str("\r\n");
        seg.push('\n');
    }
    placed.segments.push(Segment { text: seg, marked });
    (out, placed)
}

/// JSON value with rich string leaves.
pub enum J {
    Obj(Vec<(&'static str, J)>),
    Arr(Vec<J>),
    Str(Rich),
    Num(String),
    Bool(bool),
    Null,
}

pub fn json(root: &J) -> (String, Placed) {
    struct St {
        out: String,
        placed: Placed,
        seg: String,
        marked: Vec<(usize, usize)>,
    }
    fn esc_ptr(k: &str) -> String {
        k.replace('~', "~0").replace('/', "~1")
    }
    fn go(v: &J, ptr: &str, indent: usize, st: &mut St) {
        let pad = "  ".repeat(indent + 1);
        let end_pad = "  ".repeat(indent);
        match v {
            J::Obj(kv) => {
                if kv.is_empty() {
                    st.out.push_str("{}");
                    return;
                }
                st.out.push_str("{\n");
                for (i, (k, v)) in kv.iter().enumerate() {
                    st.out
                        .push_str(&format!("{pad}{}: ", serde_json::to_string(k).unwrap()));
                    go(v, &format!("{ptr}/{}", esc_ptr(k)), indent + 1, st);
                    st.out.push_str(if i + 1 < kv.len() { ",\n" } else { "\n" });
                }
                st.out.push_str(&format!("{end_pad}}}"));
            }
            J::Arr(items) => {
                if items.is_empty() {
                    st.out.push_str("[]");
                    return;
                }
                st.out.push_str("[\n");
                for (i, v) in items.iter().enumerate() {
                    st.out.push_str(&pad);
                    go(v, &format!("{ptr}/{i}"), indent + 1, st);
                    st.out
                        .push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
                }
                st.out.push_str(&format!("{end_pad}]"));
            }
            J::Str(r) => {
                let plain = r.plain();
                st.out.push_str(&serde_json::to_string(&plain).unwrap());
                let base = st.seg.chars().count();
                for (m, s, e) in r.marks() {
                    st.placed.mentions.push((
                        m.clone(),
                        Location::JsonValue {
                            pointer: ptr.to_string(),
                            start: s as u32,
                            end: e as u32,
                        },
                    ));
                    st.marked.push((base + s, base + e));
                }
                st.seg.push_str(&plain);
                st.seg.push('\n');
            }
            J::Num(n) => st.out.push_str(n),
            J::Bool(b) => st.out.push_str(if *b { "true" } else { "false" }),
            J::Null => st.out.push_str("null"),
        }
    }
    let mut st = St {
        out: String::new(),
        placed: Placed::default(),
        seg: String::new(),
        marked: Vec::new(),
    };
    go(root, "", 0, &mut st);
    st.out.push('\n');
    st.placed.segments.push(Segment {
        text: st.seg,
        marked: st.marked,
    });
    (st.out, st.placed)
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A DOCX body block.
pub enum DBlock {
    Heading(Rich),
    Para(Rich),
    /// Rows of cells; each cell is one paragraph.
    Table(Vec<Vec<Rich>>),
}

/// DOCX: a minimal WordprocessingML package. A mark with `split_at` is written as two runs
/// with different formatting (a common real-world break inside one word).
pub fn docx(blocks: &[DBlock]) -> (Vec<u8>, Placed) {
    let mut placed = Placed::default();
    let mut seg = String::new();
    let mut marked = Vec::new();
    let mut body = String::new();
    let mut para_index = 0u32;
    let mut para = |rich: &Rich, ppr: &str, body: &mut String, placed: &mut Placed| {
        body.push_str(&format!("<w:p>{ppr}"));
        let base = seg.chars().count();
        for run in &rich.runs {
            let pieces: Vec<(String, bool)> = match &run.mark {
                Some(MarkSpec {
                    split_at: Some(n), ..
                }) => {
                    let c: Vec<char> = run.text.chars().collect();
                    vec![
                        (c[..*n].iter().collect(), false),
                        (c[*n..].iter().collect(), true),
                    ]
                }
                _ => vec![(run.text.clone(), false)],
            };
            for (t, bold) in pieces {
                let rpr = if bold || ppr.contains("Heading") {
                    "<w:rPr><w:b/></w:rPr>"
                } else {
                    ""
                };
                body.push_str(&format!(
                    "<w:r>{rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                    xml_escape(&t)
                ));
            }
        }
        for (m, s, e) in rich.marks() {
            placed.mentions.push((
                m.clone(),
                Location::DocxParagraph {
                    paragraph: para_index,
                    start: s as u32,
                    end: e as u32,
                },
            ));
            marked.push((base + s, base + e));
        }
        seg.push_str(&rich.plain());
        seg.push('\n');
        body.push_str("</w:p>");
        para_index += 1;
    };
    for b in blocks {
        match b {
            DBlock::Heading(r) => para(
                r,
                "<w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr>",
                &mut body,
                &mut placed,
            ),
            DBlock::Para(r) => para(r, "", &mut body, &mut placed),
            DBlock::Table(rows) => {
                body.push_str("<w:tbl><w:tblPr><w:tblBorders><w:top w:val=\"single\" w:sz=\"4\"/><w:bottom w:val=\"single\" w:sz=\"4\"/><w:insideH w:val=\"single\" w:sz=\"4\"/></w:tblBorders></w:tblPr>");
                for row in rows {
                    body.push_str("<w:tr>");
                    for cell in row {
                        body.push_str("<w:tc>");
                        para(cell, "", &mut body, &mut placed);
                        body.push_str("</w:tc>");
                    }
                    body.push_str("</w:tr>");
                }
                body.push_str("</w:tbl>");
            }
        }
    }
    drop(para);
    placed.segments.push(Segment { text: seg, marked });

    let document = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}<w:sectPr><w:pgSz w:w=\"11906\" w:h=\"16838\"/><w:pgMar w:top=\"1440\" w:right=\"1440\" w:bottom=\"1440\" w:left=\"1440\"/></w:sectPr></w:body></w:document>"
    );
    let files: [(&str, String); 4] = [
        (
            "[Content_Types].xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\"><Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/><Default Extension=\"xml\" ContentType=\"application/xml\"/><Override PartName=\"/word/document.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml\"/><Override PartName=\"/word/styles.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml\"/></Types>".into(),
        ),
        (
            "_rels/.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"word/document.xml\"/></Relationships>".into(),
        ),
        (
            "word/_rels/document.xml.rels",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"><Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles\" Target=\"styles.xml\"/></Relationships>".into(),
        ),
        (
            "word/styles.xml",
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:style w:type=\"paragraph\" w:default=\"1\" w:styleId=\"Normal\"><w:name w:val=\"Normal\"/><w:rPr><w:sz w:val=\"22\"/></w:rPr></w:style><w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:basedOn w:val=\"Normal\"/><w:rPr><w:b/><w:sz w:val=\"32\"/></w:rPr></w:style></w:styles>".into(),
        ),
    ];
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let time = zip::DateTime::from_date_and_time(2025, 4, 10, 9, 0, 0).expect("valid date");
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(time)
        .unix_permissions(0o644);
    for (name, content) in files
        .iter()
        .map(|(n, c)| (*n, c.as_str()))
        .chain([("word/document.xml", document.as_str())])
    {
        zip.start_file(name, opts).expect("zip entry");
        zip.write_all(content.as_bytes()).expect("zip write");
    }
    let bytes = zip.finish().expect("zip finish").into_inner();
    (bytes, placed)
}

/// HTML source builder. Text is escaped; mark offsets are in the source.
#[derive(Default)]
pub struct Html {
    pub src: String,
    placed: Placed,
    marked: Vec<(usize, usize)>,
}

impl Html {
    /// Markup with no story text in it.
    pub fn raw(&mut self, s: &str) -> &mut Self {
        self.src.push_str(s);
        self
    }

    pub fn text(&mut self, rich: &Rich) -> &mut Self {
        for run in &rich.runs {
            let base = self.src.chars().count();
            match &run.mark {
                Some(m) => {
                    assert!(
                        !run.text.contains(['&', '<', '>']),
                        "mark text needs escaping"
                    );
                    let n = run.text.chars().count();
                    self.placed.mentions.push((
                        m.clone(),
                        Location::HtmlSource {
                            start: base as u32,
                            end: (base + n) as u32,
                        },
                    ));
                    self.marked.push((base, base + n));
                    self.src.push_str(&run.text);
                }
                None => self.src.push_str(&xml_escape(&run.text)),
            }
        }
        self
    }

    /// `<tag>text</tag>`.
    pub fn el(&mut self, tag: &str, rich: &Rich) -> &mut Self {
        let name = tag.split(' ').next().unwrap().to_string();
        self.raw(&format!("<{tag}>"))
            .text(rich)
            .raw(&format!("</{name}>\n"))
    }

    pub fn finish(mut self) -> (String, Placed) {
        self.placed.segments.push(Segment {
            text: self.src.clone(),
            marked: self.marked,
        });
        (self.src, self.placed)
    }
}

// ---------------------------------------------------------------- email

#[derive(Clone, Copy)]
pub enum Cte {
    SevenBit,
    QuotedPrintable,
    Base64,
}

#[derive(Clone, Copy)]
pub enum Charset {
    Utf8,
    Windows1252,
}

pub enum Part {
    Text {
        subtype: &'static str,
        charset: Charset,
        cte: Cte,
        rich: Rich,
    },
    Multipart {
        subtype: &'static str,
        parts: Vec<Part>,
    },
    Attachment {
        content_type: &'static str,
        filename: String,
        bytes: Vec<u8>,
    },
}

pub struct Msg {
    /// Header name and value. Values with non-ASCII text are written as RFC 2047 encoded words.
    pub headers: Vec<(&'static str, Rich)>,
    pub body: Part,
}

pub struct AttachmentOut {
    pub part: String,
    pub filename: String,
    pub bytes: Vec<u8>,
}

pub struct EmailOut {
    pub bytes: Vec<u8>,
    pub placed: Placed,
    pub attachments: Vec<AttachmentOut>,
}

fn qp_encode(bytes: &[u8], eol: &str) -> String {
    let mut out = String::new();
    let mut line_len = 0;
    let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    for (li, line) in lines.iter().enumerate() {
        for (i, &b) in line.iter().enumerate() {
            let last = i + 1 == line.len();
            let enc = match b {
                b'=' => format!("={b:02X}"),
                b' ' | b'\t' if last => format!("={b:02X}"),
                33..=126 | b' ' | b'\t' => (b as char).to_string(),
                _ => format!("={b:02X}"),
            };
            if line_len + enc.len() > 75 {
                out.push('=');
                out.push_str(eol);
                line_len = 0;
            }
            out.push_str(&enc);
            line_len += enc.len();
        }
        if li + 1 < lines.len() {
            out.push_str(eol);
            line_len = 0;
        }
    }
    out
}

fn b64_lines(bytes: &[u8], eol: &str) -> String {
    let s = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut out = String::new();
    for chunk in s.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push_str(eol);
    }
    out
}

fn header_value(v: &str) -> String {
    if v.is_ascii() {
        v.to_string()
    } else {
        format!(
            "=?UTF-8?B?{}?=",
            base64::engine::general_purpose::STANDARD.encode(v.as_bytes())
        )
    }
}

/// Renders a message. `eol` is `\r\n` for EML and `\n` inside MBOX. Boundaries come from `seed`.
pub fn email(msg: &Msg, seed: &str, eol: &str) -> EmailOut {
    let mut placed = Placed::default();
    let mut attachments = Vec::new();
    let mut out = String::new();
    for (name, value) in &msg.headers {
        out.push_str(&format!("{name}: {}{eol}", header_value(&value.plain())));
        let mut marked = Vec::new();
        for (m, s, e) in value.marks() {
            placed.mentions.push((
                m.clone(),
                Location::EmailHeader {
                    header: name.to_string(),
                    start: s as u32,
                    end: e as u32,
                },
            ));
            marked.push((s, e));
        }
        placed.segments.push(Segment {
            text: value.plain(),
            marked,
        });
    }
    out.push_str(&format!("MIME-Version: 1.0{eol}"));
    let root_num = if matches!(msg.body, Part::Multipart { .. }) {
        String::new()
    } else {
        "1".to_string()
    };
    part(
        &msg.body,
        &root_num,
        seed,
        eol,
        &mut out,
        &mut placed,
        &mut attachments,
    );
    EmailOut {
        bytes: out.into_bytes(),
        placed,
        attachments,
    }
}

fn part(
    p: &Part,
    num: &str,
    seed: &str,
    eol: &str,
    out: &mut String,
    placed: &mut Placed,
    atts: &mut Vec<AttachmentOut>,
) {
    match p {
        Part::Text {
            subtype,
            charset,
            cte,
            rich,
        } => {
            let (cs, bytes) = match charset {
                Charset::Utf8 => ("utf-8", rich.plain().into_bytes()),
                Charset::Windows1252 => ("windows-1252", Enc::Windows1252.encode(&rich.plain())),
            };
            let (cte_name, body) = match cte {
                Cte::SevenBit => {
                    assert!(bytes.is_ascii(), "7bit part with non-ASCII text");
                    (
                        "7bit",
                        String::from_utf8(bytes).unwrap().replace('\n', eol) + eol,
                    )
                }
                Cte::QuotedPrintable => ("quoted-printable", qp_encode(&bytes, eol) + eol),
                Cte::Base64 => {
                    let crlf = String::from_utf8_lossy(&bytes).replace('\n', "\r\n");
                    let raw = match charset {
                        Charset::Utf8 => crlf.into_bytes(),
                        Charset::Windows1252 => {
                            Enc::Windows1252.encode(&rich.plain().replace('\n', "\r\n"))
                        }
                    };
                    ("base64", b64_lines(&raw, eol))
                }
            };
            out.push_str(&format!("Content-Type: text/{subtype}; charset={cs}{eol}Content-Transfer-Encoding: {cte_name}{eol}{eol}"));
            out.push_str(&body);
            let mut marked = Vec::new();
            for (m, s, e) in rich.marks() {
                placed.mentions.push((
                    m.clone(),
                    Location::MimePart {
                        part: num.to_string(),
                        start: s as u32,
                        end: e as u32,
                    },
                ));
                marked.push((s, e));
            }
            placed.segments.push(Segment {
                text: rich.plain(),
                marked,
            });
        }
        Part::Multipart { subtype, parts } => {
            let boundary = format!(
                "=_{}_{}",
                seed,
                if num.is_empty() {
                    "0".to_string()
                } else {
                    num.replace('.', "_")
                }
            );
            out.push_str(&format!(
                "Content-Type: multipart/{subtype}; boundary=\"{boundary}\"{eol}{eol}"
            ));
            out.push_str(&format!(
                "This is a multi-part message in MIME format.{eol}"
            ));
            for (i, child) in parts.iter().enumerate() {
                let n = if num.is_empty() {
                    format!("{}", i + 1)
                } else {
                    format!("{num}.{}", i + 1)
                };
                out.push_str(&format!("{eol}--{boundary}{eol}"));
                part(child, &n, seed, eol, out, placed, atts);
            }
            out.push_str(&format!("{eol}--{boundary}--{eol}"));
        }
        Part::Attachment {
            content_type,
            filename,
            bytes,
        } => {
            out.push_str(&format!(
                "Content-Type: {content_type}; name=\"{filename}\"{eol}Content-Disposition: attachment; filename=\"{filename}\"{eol}Content-Transfer-Encoding: base64{eol}{eol}"
            ));
            out.push_str(&b64_lines(bytes, eol));
            atts.push(AttachmentOut {
                part: num.to_string(),
                filename: filename.clone(),
                bytes: bytes.clone(),
            });
        }
    }
}

/// mboxrd: `From ` envelope line, `>`-quoting of body lines that start with `>*From `.
pub fn mbox(messages: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (envelope_date, bytes) in messages {
        out.extend_from_slice(format!("From - {envelope_date}\n").as_bytes());
        let text = String::from_utf8(bytes.clone()).expect("ascii message");
        for line in text.split_inclusive('\n') {
            if line.trim_start_matches('>').starts_with("From ") {
                out.push(b'>');
            }
            out.extend_from_slice(line.as_bytes());
        }
        out.push(b'\n');
    }
    out
}
