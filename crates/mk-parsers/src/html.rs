//! HTML (US-02.3 AC1/AC2): the visible text, one segment per block, read with the `html5ever`
//! tokenizer alone (no tree). Nothing is fetched: this crate has no network code.

use std::cell::RefCell;
use std::io::Read;

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::states::RawKind;
use html5ever::tokenizer::{
    BufferQueue, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use mk_kernel::Locator;

use crate::content::{Format, ParseError, ParsedSegment, Place, Sink, Source};
use crate::encoding::{Decoded, detect};

/// Elements whose content is never shown. `head` is one too, except its `title`.
const HIDDEN: &[&str] = &["script", "style", "template", "noscript"];

/// Elements that start and end a block of text: a segment never crosses one.
const BLOCKS: &[&str] = &[
    "address",
    "article",
    "aside",
    "blockquote",
    "body",
    "br",
    "caption",
    "dd",
    "details",
    "dialog",
    "div",
    "dl",
    "dt",
    "fieldset",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "header",
    "hr",
    "html",
    "li",
    "main",
    "nav",
    "ol",
    "option",
    "p",
    "pre",
    "section",
    "summary",
    "table",
    "td",
    "th",
    "title",
    "tr",
    "ul",
];

/// Text in the encodings of NFR-COMP-06 (a `<meta charset>` is not read; ASSUMPTION). Skips
/// `script`, `style`, `template`, `noscript`, `head` (except `title`) and comments; splits
/// segments at block elements and `<br>`; collapses runs of whitespace to one space, except in
/// `pre`, where each line is a segment. Character references are decoded. `href` targets and
/// `alt` texts are not kept (ASSUMPTION; URLs come with M4's identifiers).
///
/// The locator of a segment is its offset in the document's extracted text (its segments
/// joined by `\n`) and a span in the segment (INV-CO-03).
pub(crate) fn parse(
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
) -> Result<(), ParseError> {
    let encoding = detect(input)?;
    sink.document(place.start(Format::Html, Vec::new()))?;
    let mut decoded = Decoded::new(input, encoding)?;
    let tokenizer = Tokenizer::new(Text::default(), TokenizerOpts::default());
    let queue = BufferQueue::default();
    let mut bytes = vec![0u8; 1 << 16];
    let mut pending: Vec<u8> = Vec::new();
    loop {
        let n = decoded.read(&mut bytes).map_err(ParseError::io)?;
        if n == 0 {
            break;
        }
        // The decoder gives UTF-8, but a read can end inside a character.
        pending.extend_from_slice(&bytes[..n]);
        let valid = match std::str::from_utf8(&pending) {
            Ok(s) => s.len(),
            Err(e) => e.valid_up_to(),
        };
        let chunk = std::str::from_utf8(&pending[..valid]).expect("checked");
        queue.push_back(StrTendril::from_slice(chunk));
        pending.drain(..valid);
        let _ = tokenizer.feed(&queue);
        drain(&tokenizer.sink, sink)?;
    }
    tokenizer.end();
    tokenizer.sink.flush();
    drain(&tokenizer.sink, sink)
}

fn drain(text: &Text, sink: &mut dyn Sink) -> Result<(), ParseError> {
    let ready = std::mem::take(&mut *text.ready.borrow_mut());
    for seg in ready {
        sink.segment(seg)?;
    }
    Ok(())
}

/// The visible text of an HTML string, segments joined by `\n` (an email's HTML-only body,
/// US-02.6 AC3).
pub(crate) fn visible_text(html: &str) -> String {
    let tokenizer = Tokenizer::new(Text::default(), TokenizerOpts::default());
    let queue = BufferQueue::default();
    queue.push_back(StrTendril::from_slice(html));
    let _ = tokenizer.feed(&queue);
    tokenizer.end();
    tokenizer.sink.flush();
    let ready = tokenizer.sink.ready.take();
    ready
        .into_iter()
        .map(|s| s.text)
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Default)]
struct State {
    /// Open hidden elements, innermost last.
    hidden: Vec<String>,
    in_head: bool,
    in_title: bool,
    pre: u32,
    current: String,
    space: bool,
    /// Characters of the extracted text before the current segment.
    offset: u64,
}

/// The tokenizer's sink: builds segments as tokens come.
#[derive(Default)]
struct Text {
    state: RefCell<State>,
    ready: RefCell<Vec<ParsedSegment>>,
}

impl Text {
    fn flush(&self) {
        let mut s = self.state.borrow_mut();
        let text = std::mem::take(&mut s.current);
        s.space = false;
        let len = text.chars().count() as u64;
        if len == 0 {
            return;
        }
        let offset = s.offset;
        s.offset += len + 1;
        if let Some(seg) = ParsedSegment::whole(|span| Locator::HtmlText { offset, span }, text) {
            self.ready.borrow_mut().push(seg);
        }
    }

    fn chars(&self, text: &str) {
        let mut s = self.state.borrow_mut();
        let visible = s.hidden.is_empty() && (!s.in_head || s.in_title);
        if !visible {
            return;
        }
        if s.pre > 0 {
            let mut lines = text.split('\n').peekable();
            while let Some(line) = lines.next() {
                s.current.push_str(line.strip_suffix('\r').unwrap_or(line));
                if lines.peek().is_some() {
                    drop(s);
                    self.flush();
                    s = self.state.borrow_mut();
                }
            }
            return;
        }
        for c in text.chars() {
            if c.is_whitespace() {
                s.space = !s.current.is_empty();
            } else {
                if s.space {
                    s.current.push(' ');
                    s.space = false;
                }
                s.current.push(c);
            }
        }
    }
}

impl TokenSink for Text {
    type Handle = ();

    fn process_token(&self, token: Token, _line: u64) -> TokenSinkResult<()> {
        match token {
            Token::CharacterTokens(t) => self.chars(&t),
            Token::TagToken(tag) => {
                let name: &str = &tag.name;
                if BLOCKS.contains(&name) {
                    self.flush();
                }
                let mut s = self.state.borrow_mut();
                match tag.kind {
                    TagKind::StartTag => {
                        match name {
                            "head" => s.in_head = true,
                            "body" => s.in_head = false,
                            "title" => s.in_title = true,
                            "pre" | "listing" | "textarea" => s.pre += 1,
                            _ => {}
                        }
                        if HIDDEN.contains(&name) && !tag.self_closing {
                            s.hidden.push(name.to_string());
                        }
                        // What the tree builder would tell the tokenizer, so script and style
                        // text is never read as markup.
                        return match name {
                            "script" => TokenSinkResult::RawData(RawKind::ScriptData),
                            "style" | "noscript" | "xmp" | "iframe" | "noembed" | "noframes" => {
                                TokenSinkResult::RawData(RawKind::Rawtext)
                            }
                            "title" | "textarea" => TokenSinkResult::RawData(RawKind::Rcdata),
                            "plaintext" => TokenSinkResult::Plaintext,
                            _ => TokenSinkResult::Continue,
                        };
                    }
                    TagKind::EndTag => {
                        match name {
                            "head" => s.in_head = false,
                            "title" => s.in_title = false,
                            "pre" | "listing" | "textarea" => s.pre = s.pre.saturating_sub(1),
                            _ => {}
                        }
                        if let Some(i) = s.hidden.iter().rposition(|h| h == name) {
                            s.hidden.truncate(i);
                        }
                    }
                }
            }
            Token::EOFToken => self.flush(),
            _ => {}
        }
        TokenSinkResult::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::parse;

    fn blocks(html: &str) -> Vec<(u64, String)> {
        let parsed = parse(Format::Html, html.as_bytes()).unwrap();
        parsed.documents[0]
            .segments
            .iter()
            .map(|s| match &s.locator {
                Locator::HtmlText { offset, .. } => (*offset, s.text.clone()),
                other => panic!("{other:?}"),
            })
            .collect()
    }

    /// US-02.3 AC1: visible text by blocks, whitespace collapsed, references decoded, offsets
    /// in the extracted text.
    #[test]
    fn us_02_3_ac1_visible_text_blocks() {
        let html = "<!doctype html><html><head><title>Team &amp; staff</title>\
            <meta charset=utf-8><style>p{color:red}</style></head><body>\n\
            <h1>Kestrel  <b>Advisory</b></h1><p>Mail <a href=\"mailto:t@k.example\">t@k.example</a>\n now</p>\
            <ul><li>one</li><li>two<br>three</li></ul><pre>a  b\nc</pre></body></html>";
        assert_eq!(
            blocks(html),
            vec![
                (0, "Team & staff".into()),
                (13, "Kestrel Advisory".into()),
                (30, "Mail t@k.example now".into()),
                (51, "one".into()),
                (55, "two".into()),
                (59, "three".into()),
                (65, "a  b".into()),
                (70, "c".into()),
            ]
        );
    }

    /// US-02.3 AC2: scripts, styles, comments and hidden markup give no text; markup inside a
    /// script is not read as tags.
    #[test]
    fn us_02_3_ac2_scripts_and_styles_ignored() {
        let html = "<p>before</p><script>var s = '</p><p>fake</p>'; if (a<b) {}</script>\
            <!-- <p>comment</p> --><noscript><img src=\"http://x.example/p.gif\"></noscript>\
            <template><p>tpl</p></template><p>after <img src=\"http://track.example/t.gif\"></p>";
        assert_eq!(
            blocks(html),
            vec![(0, "before".into()), (7, "after".into())]
        );
    }

    /// TD-3 `scripts-and-remote-images.html`: scripts in `head`, one with a `src`.
    #[test]
    fn us_02_3_ac2_head_scripts_ignored() {
        let html = "<!doctype html><html><head><title>Payments</title>\
            <link rel=\"stylesheet\" href=\"http://cdn.example/s.css\">\
            <script src=\"http://cdn.example/t.js\"></script>\
            <script>var who = \"hidden@script.example\"; document.write('<p>' + who + '</p>');</script>\
            <style>p { background: url(http://cdn.example/bg.png); }</style></head>\
            <body><p>Payments go to visible@html.example.</p></body></html>";
        assert_eq!(
            blocks(html),
            vec![
                (0, "Payments".into()),
                (9, "Payments go to visible@html.example.".into())
            ]
        );
    }

    #[test]
    fn email_html_body_text() {
        assert_eq!(
            visible_text("<p>Hi <b>Anna</b>,</p><p>see a@x.example</p>"),
            "Hi Anna,\nsee a@x.example"
        );
    }
}
