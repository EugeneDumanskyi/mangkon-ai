//! Emails (US-02.6): EML files and MBOX mailboxes (ADR-0013: `mail-parser`, plus a streaming
//! MBOX splitter). Emails and HTML are shown as text only (gap A8).

use std::io::{self, BufRead, BufReader, Cursor};

use mail_parser::parsers::MessageStream;
use mail_parser::{HeaderValue, Message, MessageParser, MessagePart, MimeHeaders, PartType};
use mk_kernel::Locator;

use crate::content::{Collector, Format, ParseError, ParsedSegment, Place, Sink, Source};
use crate::paged::Engines;

/// The headers stored as fields, one segment each (US-02.6 AC2), in this spelling.
pub const HEADERS: [&str; 8] = [
    "From",
    "To",
    "Cc",
    "Bcc",
    "Reply-To",
    "Date",
    "Subject",
    "Message-ID",
];

/// The largest MBOX message read; a larger one is skipped with a warning. ASSUMPTION: 256 MB.
pub const MAX_MESSAGE: usize = 256 << 20;

/// How many attachments deep child documents go (an attachment of an attachment is 2).
/// ASSUMPTION: 8.
pub const MAX_DEPTH: u32 = 8;

fn not_an_email() -> ParseError {
    ParseError::new("not_an_email", "Not an email")
}

/// One EML file: one document (US-02.6). A file with no known header fails with "Not an
/// email".
pub(crate) fn parse_eml(
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes).map_err(ParseError::io)?;
    let message = parse_message(&bytes).ok_or_else(not_an_email)?;
    emit(&message, Format::Eml, place, sink, engines)
}

/// One MBOX file, split as a stream: a message starts at each line beginning with `From `, and
/// mboxrd `>From ` lines lose one `>`. Each message is parsed alone into document `message N`
/// (0-based, file order, US-02.6 AC1); [`Sink::progress`] counts finished messages, which
/// restarts the deadline (ADR-0016). A message over [`MAX_MESSAGE`] or with no known header is
/// skipped with a warning. A file whose first non-blank line is not a `From ` line fails; an
/// empty file has no messages.
pub(crate) fn parse_mbox(
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let mut reader = BufReader::with_capacity(1 << 16, input);
    let mut line = Vec::new();
    let mut message: Option<Vec<u8>> = None;
    let mut over = false;
    let mut index = 0u64;
    loop {
        line.clear();
        let (n, cut) =
            read_line_capped(&mut reader, &mut line, MAX_MESSAGE).map_err(ParseError::io)?;
        if n == 0 {
            break;
        }
        if line.starts_with(b"From ") {
            if let Some(m) = message.take() {
                finish(&m, over, index, place, sink, engines)?;
                index += 1;
                sink.progress(index);
            }
            message = Some(Vec::new());
            over = false;
            continue;
        }
        let Some(m) = message.as_mut() else {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            return Err(ParseError::new(
                "not_a_mailbox",
                "Not a mailbox file: it does not start with a \"From \" line",
            ));
        };
        if over {
            continue;
        }
        let body = if line.iter().position(|b| *b != b'>').is_some_and(|i| i > 0)
            && line[line.iter().position(|b| *b != b'>').unwrap_or(0)..].starts_with(b"From ")
        {
            &line[1..]
        } else {
            &line[..]
        };
        if cut || m.len() + body.len() > MAX_MESSAGE {
            over = true;
            *m = Vec::new();
            continue;
        }
        m.extend_from_slice(body);
    }
    if let Some(m) = message {
        finish(&m, over, index, place, sink, engines)?;
        sink.progress(index + 1);
    }
    Ok(())
}

/// Reads one line (with its `\n`) into `buf`, keeping at most `cap` bytes of it. Returns the
/// bytes consumed and whether the line was cut.
fn read_line_capped(
    r: &mut impl BufRead,
    buf: &mut Vec<u8>,
    cap: usize,
) -> io::Result<(usize, bool)> {
    let (mut consumed, mut cut) = (0, false);
    loop {
        let available = match r.fill_buf() {
            Ok(b) => b,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if available.is_empty() {
            return Ok((consumed, cut));
        }
        let (take, done) = match available.iter().position(|b| *b == b'\n') {
            Some(i) => (i + 1, true),
            None => (available.len(), false),
        };
        let room = cap.saturating_sub(buf.len());
        if take > room {
            cut = true;
        }
        buf.extend_from_slice(&available[..take.min(room)]);
        r.consume(take);
        consumed += take;
        if done {
            return Ok((consumed, cut));
        }
    }
}

fn finish(
    bytes: &[u8],
    over: bool,
    index: u64,
    place: &Place,
    sink: &mut dyn Sink,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    if over {
        sink.warning(format!(
            "Message {index} is larger than 256 MB and was skipped"
        ));
        return Ok(());
    }
    let Some(message) = parse_message(bytes) else {
        sink.warning(format!("Message {index} is not an email and was skipped"));
        return Ok(());
    };
    let at = Place {
        position: format!("message {index}"),
        parent: place.parent,
        depth: place.depth,
    };
    emit(&message, Format::Mbox, &at, sink, engines)
}

/// A message with at least one header `mail-parser` knows (From, Date, Content-Type, …).
fn parse_message(bytes: &[u8]) -> Option<Message<'_>> {
    let message = MessageParser::new().parse(bytes)?;
    message
        .root_part()
        .headers
        .iter()
        .any(|h| !h.name.is_other())
        .then_some(message)
}

/// The header's value as text: unfolded, RFC 2047 words decoded, trimmed.
fn header_text(raw: &[u8]) -> String {
    let mut line = raw.to_vec();
    line.push(b'\n');
    match MessageStream::new(&line).parse_unstructured() {
        HeaderValue::Text(t) => t.trim().to_string(),
        _ => String::new(),
    }
}

/// `\r\n` and lone `\r` as `\n`.
fn unix_lines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// The body parts and attachments of a message, by IMAP section number.
#[derive(Default)]
struct Parts<'m, 'x> {
    text: Vec<(String, String)>,
    attachments: Vec<(String, &'m MessagePart<'x>)>,
}

fn section(parent: &str, i: usize) -> String {
    if parent.is_empty() {
        (i + 1).to_string()
    } else {
        format!("{parent}.{}", i + 1)
    }
}

fn is_attachment(part: &MessagePart<'_>) -> bool {
    part.content_disposition()
        .is_some_and(|d| d.is_attachment())
}

fn is_plain(part: &MessagePart<'_>) -> bool {
    matches!(part.body, PartType::Text(_)) && !is_attachment(part)
}

fn is_html(part: &MessagePart<'_>) -> bool {
    matches!(part.body, PartType::Html(_)) && !is_attachment(part)
}

/// Walks the MIME tree (US-02.6 AC3): text/plain parts are body text; an HTML part is used for
/// its visible text only when its `multipart/alternative` has no plain part (or it stands
/// alone); everything else, and parts marked as attachments, are attachments.
fn walk<'m, 'x>(message: &'m Message<'x>, id: u32, number: String, out: &mut Parts<'m, 'x>) {
    let Some(part) = message.parts.get(id as usize) else {
        return;
    };
    match &part.body {
        PartType::Multipart(children) => {
            let alternative = part
                .content_type()
                .and_then(|c| c.subtype())
                .is_some_and(|s| s.eq_ignore_ascii_case("alternative"));
            let child = |i: usize| message.parts.get(children[i] as usize);
            let pick = alternative
                .then(|| {
                    let find = |f: fn(&MessagePart<'_>) -> bool| {
                        (0..children.len()).find(|i| child(*i).is_some_and(f))
                    };
                    find(is_plain)
                        .or_else(|| find(is_html))
                        .or(children.len().checked_sub(1))
                })
                .flatten();
            for (i, c) in children.iter().enumerate() {
                if pick.is_none_or(|p| p == i) {
                    walk(message, *c, section(&number, i), out);
                }
            }
        }
        PartType::Text(t) if !is_attachment(part) => out.text.push((number, unix_lines(t))),
        PartType::Html(h) if !is_attachment(part) => {
            out.text.push((number, crate::html::visible_text(h)))
        }
        _ => out.attachments.push((number, part)),
    }
}

/// One message as a document: its headers, then its body parts, then its attachments as child
/// documents (US-02.6 AC2–AC4).
fn emit(
    message: &Message<'_>,
    kind: Format,
    place: &Place,
    sink: &mut dyn Sink,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let doc = sink.document(place.start(kind, Vec::new()))?;
    for header in &message.root_part().headers {
        let Some(name) = HEADERS
            .iter()
            .find(|n| n.eq_ignore_ascii_case(header.name.as_str()))
        else {
            continue;
        };
        let raw = message
            .raw_message
            .get(header.offset_start as usize..header.offset_end as usize)
            .unwrap_or_default();
        let name = name.to_string();
        if let Some(seg) =
            ParsedSegment::whole(|span| Locator::EmailHeader { name, span }, header_text(raw))
        {
            sink.segment(seg)?;
        }
    }
    let mut parts = Parts::default();
    let root = match message.parts.first().map(|p| &p.body) {
        Some(PartType::Multipart(_)) => String::new(),
        _ => "1".to_string(),
    };
    walk(message, 0, root, &mut parts);
    for (part, text) in parts.text {
        if text.trim().is_empty() {
            continue;
        }
        if let Some(seg) = ParsedSegment::whole(|span| Locator::MimePart { part, span }, text) {
            sink.segment(seg)?;
        }
    }
    for (number, part) in parts.attachments {
        attachment(part, &number, doc, place.depth, sink, engines)?;
    }
    Ok(())
}

/// The format of an attachment: by its file name, else by its content type.
fn attachment_format(part: &MessagePart<'_>) -> Option<Format> {
    let by_name = part
        .attachment_name()
        .and_then(|n| n.rsplit_once('.'))
        .and_then(|(_, ext)| Format::from_extension(ext));
    by_name.or_else(|| {
        let ct = part.content_type()?;
        let full = format!(
            "{}/{}",
            ct.ctype().to_ascii_lowercase(),
            ct.subtype().unwrap_or("").to_ascii_lowercase()
        );
        match full.as_str() {
            "text/plain" => Some(Format::Text),
            "text/markdown" => Some(Format::Markdown),
            "text/csv" => Some(Format::Csv),
            "application/json" => Some(Format::Json),
            "text/html" => Some(Format::Html),
            "message/rfc822" => Some(Format::Eml),
            "application/pdf" => Some(Format::Pdf),
            "image/png" | "image/jpeg" | "image/tiff" | "image/webp" => Some(Format::Image),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                Some(Format::Docx)
            }
            _ => None,
        }
    })
}

/// An image of a type that is not read (GIF, BMP: not in US-02.2 AC2).
fn is_other_image(part: &MessagePart<'_>) -> bool {
    let name = part.attachment_name().unwrap_or("").to_ascii_lowercase();
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e);
    let image = part
        .content_type()
        .is_some_and(|c| c.ctype().eq_ignore_ascii_case("image"));
    image || ["gif", "bmp"].contains(&ext)
}

/// An attachment of a supported type becomes a child document of `parent`, parsed by its own
/// parser: text formats, and since M3b PDFs and images, with OCR (US-02.6 AC4, INV-CO-05). It is
/// parsed into a buffer first, so one that fails leaves no partial document: it gets a warning
/// instead, as do unsupported types and attachments past [`MAX_DEPTH`].
fn attachment(
    part: &MessagePart<'_>,
    number: &str,
    parent: u32,
    depth: u32,
    sink: &mut dyn Sink,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let name = part
        .attachment_name()
        .map(str::to_string)
        .or_else(|| match &part.body {
            PartType::Message(m) => m.subject().map(str::to_string),
            _ => None,
        })
        .unwrap_or_else(|| format!("part {number}"));
    let Some(format) = attachment_format(part) else {
        sink.warning(if is_other_image(part) {
            format!("Attachment {name} was not read: this image type is not supported")
        } else {
            format!("Attachment {name} is not a supported file type")
        });
        return Ok(());
    };
    if depth + 1 > MAX_DEPTH {
        sink.warning(format!(
            "Attachment {name} is nested more than {MAX_DEPTH} levels deep and was not read"
        ));
        return Ok(());
    }
    let bytes: &[u8] = match &part.body {
        // An inline nested message shares the outer message's bytes; its root part says where
        // it lies in them.
        PartType::Message(m) => {
            let root = m.root_part();
            m.raw_message
                .get(root.offset_header as usize..root.offset_end as usize)
                .unwrap_or(&m.raw_message)
        }
        PartType::Text(t) | PartType::Html(t) => t.as_bytes(),
        PartType::Binary(b) | PartType::InlineBinary(b) => b,
        PartType::Multipart(_) => &[],
    };
    let place = Place {
        position: format!("part {number}: {name}"),
        parent: None,
        depth: depth + 1,
    };
    let mut buffer = Collector::default();
    let mut input = Cursor::new(bytes);
    match crate::content::parse_at(format, &mut input, &mut buffer, &place, engines) {
        Ok(()) => buffer.replay(sink, Some(parent)),
        Err(e) => {
            sink.warning(format!("Attachment {name} could not be read: {e}"));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{ParsedContent, ParsedDocument, parse};

    fn segs(doc: &ParsedDocument) -> Vec<(String, String)> {
        doc.segments
            .iter()
            .map(|s| match &s.locator {
                Locator::EmailHeader { name, .. } => (format!("header {name}"), s.text.clone()),
                Locator::MimePart { part, .. } => (format!("part {part}"), s.text.clone()),
                other => (format!("{other:?}"), s.text.clone()),
            })
            .collect()
    }

    const PLAIN: &str = "From: Irene Vasquez =?UTF-8?B?TGluZHF2aXN0?= <irene@bw.example>\r\n\
        To: tomas@k.example,\r\n  corvin@s.example\r\n\
        Subject: =?UTF-8?B?VGVuZGVyIOKAkyByZXN1bHQ=?=\r\n\
        Message-ID: <1@mail.example>\r\n\
        X-Other: skipped\r\n\
        \r\n\
        Hello,\r\nwrite to accounts@bw.example.\r\n";

    /// US-02.6 AC2: the eight headers as fields, unfolded and decoded; addresses in them are
    /// found, Message-ID is not scanned.
    #[test]
    fn us_02_6_ac2_headers_fields() {
        let parsed = parse(Format::Eml, PLAIN.as_bytes()).unwrap();
        assert_eq!(parsed.documents.len(), 1);
        let doc = &parsed.documents[0];
        assert_eq!((doc.kind, doc.position.as_str()), (Format::Eml, "whole"));
        assert_eq!(
            segs(doc),
            vec![
                (
                    "header From".into(),
                    "Irene Vasquez Lindqvist <irene@bw.example>".into()
                ),
                (
                    "header To".into(),
                    "tomas@k.example, corvin@s.example".into()
                ),
                ("header Subject".into(), "Tender – result".into()),
                ("header Message-ID".into(), "<1@mail.example>".into()),
                (
                    "part 1".into(),
                    "Hello,\nwrite to accounts@bw.example.\n".into()
                ),
            ]
        );
        let span = mk_kernel::Span::new(1, 5, 16).unwrap();
        assert_eq!(
            doc.mention_locator(3, span),
            None,
            "Message-ID is not scanned"
        );
        assert!(doc.mention_locator(0, span).is_some());
    }

    fn multipart(parts: &[&str]) -> String {
        let mut m = String::from(
            "From: a@x.example\r\nSubject: s\r\nContent-Type: multipart/mixed; boundary=\"B\"\r\n\r\npre\r\n",
        );
        for p in parts {
            m.push_str("--B\r\n");
            m.push_str(p);
            m.push_str("\r\n");
        }
        m.push_str("--B--\r\n");
        m
    }

    /// US-02.6 AC3: text/plain is preferred in an alternative; HTML alone gives its visible
    /// text; the locator has the IMAP part number.
    #[test]
    fn us_02_6_ac3_plain_preferred_html_fallback_part_number() {
        let alt = "Content-Type: multipart/alternative; boundary=\"A\"\r\n\r\n\
            --A\r\nContent-Type: text/plain\r\n\r\nplain body\r\n\
            --A\r\nContent-Type: text/html\r\n\r\n<p>html body</p>\r\n--A--";
        let html = "Content-Type: text/html; charset=utf-8\r\n\r\n<p>Only <b>html</b></p><p>b@y.example</p>";
        let parsed = parse(Format::Eml, multipart(&[alt, html]).as_bytes()).unwrap();
        let got = segs(&parsed.documents[0]);
        assert_eq!(
            &got[2..],
            &[
                ("part 1.1".to_string(), "plain body".to_string()),
                ("part 2".to_string(), "Only html\nb@y.example".to_string()),
            ]
        );
    }

    /// US-02.6 AC4, INV-CO-05: text attachments become child documents; PDFs, unsupported
    /// types and broken attachments give warnings.
    #[test]
    fn us_02_6_ac4_text_attachment_child_document() {
        let txt = "Content-Type: text/plain; name=\"notes.txt\"\r\n\
            Content-Disposition: attachment; filename=\"notes.txt\"\r\n\r\nline c@z.example";
        let csv = "Content-Type: text/csv\r\nContent-Disposition: attachment; filename=\"t.csv\"\r\n\
            Content-Transfer-Encoding: base64\r\n\r\nbmFtZSxlbWFpbApBbm5hLGFAeC5leGFtcGxlCg==";
        let pdf = "Content-Type: application/pdf; name=\"i.pdf\"\r\n\
            Content-Disposition: attachment; filename=\"i.pdf\"\r\n\r\n%PDF-1.4";
        let zip = "Content-Type: application/zip\r\nContent-Disposition: attachment; filename=\"a.zip\"\r\n\r\nPK";
        let bad = "Content-Type: application/json\r\nContent-Disposition: attachment; filename=\"b.json\"\r\n\r\n{nope";
        let parsed: ParsedContent = parse(
            Format::Eml,
            multipart(&[
                "Content-Type: text/plain\r\n\r\nbody",
                txt,
                csv,
                pdf,
                zip,
                bad,
            ])
            .as_bytes(),
        )
        .unwrap();
        let docs = &parsed.documents;
        assert_eq!(docs.len(), 3);
        assert_eq!(
            (docs[1].kind, docs[1].position.as_str(), docs[1].parent),
            (Format::Text, "part 2: notes.txt", Some(0))
        );
        assert_eq!(docs[1].segments[0].text, "line c@z.example");
        assert_eq!(
            (
                docs[2].kind,
                docs[2].parent,
                docs[2].segments[1].text.as_str()
            ),
            (Format::Csv, Some(0), "Anna\ta@x.example")
        );
        assert_eq!(
            parsed.warnings,
            vec![
                "Attachment i.pdf could not be read: Not a readable PDF".to_string(),
                "Attachment a.zip is not a supported file type".to_string(),
                "Attachment b.json could not be read: Invalid JSON at line 1, column 2".to_string(),
            ]
        );
    }

    /// Attached messages are child emails, down to [`MAX_DEPTH`].
    #[test]
    fn nested_messages_stop_at_max_depth() {
        let mut m = "From: d@x.example\r\nSubject: level 20\r\n\r\ninnermost".to_string();
        for level in (0..20).rev() {
            m = format!(
                "From: d@x.example\r\nSubject: level {level}\r\nContent-Type: multipart/mixed; boundary=\"L{level}\"\r\n\r\n\
                 --L{level}\r\nContent-Type: text/plain\r\n\r\nbody {level}\r\n\
                 --L{level}\r\nContent-Type: message/rfc822\r\n\r\n{m}\r\n--L{level}--\r\n"
            );
        }
        let parsed = parse(Format::Eml, m.as_bytes()).unwrap();
        assert_eq!(parsed.documents.len(), 9);
        for (i, d) in parsed.documents.iter().enumerate().skip(1) {
            assert_eq!((d.kind, d.parent), (Format::Eml, Some(i as u32 - 1)));
        }
        assert_eq!(
            parsed.warnings,
            vec!["Attachment level 9 is nested more than 8 levels deep and was not read"]
        );
    }

    /// US-02.6 AC1: N messages give N documents, `message 0` first; mboxrd quoting is undone;
    /// a message that is not an email is skipped with a warning.
    #[test]
    fn us_02_6_ac1_mbox_n_documents() {
        let mbox = "From - Mon Feb 12 15:00:00 2024\n\
            From: a@x.example\nSubject: one\n\nfirst\n>From the start\n>>From quoted\n\n\
            From - Tue Feb 13 15:00:00 2024\n\
            not an email at all\n\n\
            From - Wed Feb 14 15:00:00 2024\n\
            From: b@y.example\nSubject: three\n\nthird";
        let parsed = parse(Format::Mbox, mbox.as_bytes()).unwrap();
        let docs = &parsed.documents;
        assert_eq!(docs.len(), 2);
        assert_eq!(
            (docs[0].kind, docs[0].position.as_str()),
            (Format::Mbox, "message 0")
        );
        assert_eq!(docs[1].position, "message 2");
        assert_eq!(
            docs[0].segments.last().unwrap().text,
            "first\nFrom the start\n>From quoted\n\n"
        );
        assert_eq!(
            parsed.warnings,
            vec!["Message 1 is not an email and was skipped"]
        );
        assert_eq!(parse(Format::Mbox, b"").unwrap().documents.len(), 0);
        assert_eq!(
            parse(Format::Mbox, b"\n\nFrom: a@x.example\n\nno envelope")
                .unwrap_err()
                .code,
            "not_a_mailbox"
        );
    }

    #[test]
    fn not_an_email_fails() {
        for bytes in [
            &b" \n"[..],
            b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR",
            b"just some words\n",
        ] {
            assert_eq!(
                parse(Format::Eml, bytes).unwrap_err().message,
                "Not an email",
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn capped_lines() {
        let mut r = Cursor::new(b"abcdef\ngh".to_vec());
        let mut buf = Vec::new();
        assert_eq!(read_line_capped(&mut r, &mut buf, 3).unwrap(), (7, true));
        assert_eq!(buf, b"abc");
        buf.clear();
        assert_eq!(read_line_capped(&mut r, &mut buf, 3).unwrap(), (2, false));
        assert_eq!(buf, b"gh");
    }
}
