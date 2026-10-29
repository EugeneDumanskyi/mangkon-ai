use std::fmt;
use std::io::{Read, Seek};

use mk_kernel::{Column, Locator, OcrGeometry, Span};

use crate::paged::Engines;

/// File formats with a parser. M2b: TXT, MD and CSV. M3a: JSON, HTML, DOCX, EML and MBOX, and
/// the UTF-16 (BOM) and Windows-1252 encodings for the text formats. M3b: PDF and images.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Format {
    Text,
    Markdown,
    Csv,
    Json,
    Html,
    Docx,
    Eml,
    Mbox,
    Pdf,
    /// PNG, JPEG, TIFF (every page) and WEBP, read with OCR (US-02.2 AC2).
    Image,
}

const MB: u64 = 1 << 20;

impl Format {
    pub const ALL: [Format; 10] = [
        Format::Text,
        Format::Markdown,
        Format::Csv,
        Format::Json,
        Format::Html,
        Format::Docx,
        Format::Eml,
        Format::Mbox,
        Format::Pdf,
        Format::Image,
    ];

    /// Stable name, stored as the content's file type and the document kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Markdown => "md",
            Format::Csv => "csv",
            Format::Json => "json",
            Format::Html => "html",
            Format::Docx => "docx",
            Format::Eml => "eml",
            Format::Mbox => "mbox",
            Format::Pdf => "pdf",
            Format::Image => "image",
        }
    }

    /// File name extensions read as this format (lower case).
    pub fn extensions(&self) -> &'static [&'static str] {
        match self {
            Format::Html => &["html", "htm"],
            Format::Text => &["txt"],
            Format::Markdown => &["md"],
            Format::Csv => &["csv"],
            Format::Json => &["json"],
            Format::Docx => &["docx"],
            Format::Eml => &["eml"],
            Format::Mbox => &["mbox"],
            Format::Pdf => &["pdf"],
            Format::Image => &["png", "jpg", "jpeg", "tif", "tiff", "webp"],
        }
    }

    /// The format for a file name extension, ignoring case.
    pub fn from_extension(ext: &str) -> Option<Format> {
        Self::ALL
            .into_iter()
            .find(|f| f.extensions().iter().any(|e| e.eq_ignore_ascii_case(ext)))
    }

    /// The format for a stored file type (`as_str`).
    pub fn from_name(name: &str) -> Option<Format> {
        Self::ALL.into_iter().find(|f| f.as_str() == name)
    }

    /// The largest file read in this format (NFR-CAP-05/06); `None` for no limit of its own.
    /// A larger file fails with "File too large (limit: …)" before a worker starts
    /// (NFR-CAP-07).
    pub fn size_limit(&self) -> Option<u64> {
        match self {
            Format::Csv => Some(500 * MB),
            Format::Mbox => Some(2048 * MB),
            _ => None,
        }
    }

    /// Whether the parser streams the file, so its memory does not grow with the file size.
    pub fn is_streamed(&self) -> bool {
        !matches!(self, Format::Eml | Format::Pdf | Format::Image)
    }

    /// Whether the content has pages, each with its own deadline (ADR-0017) and a page view
    /// (ADR-0011).
    pub fn is_paged(&self) -> bool {
        matches!(self, Format::Pdf | Format::Image)
    }

    /// Estimated Stage A memory for a file of `size` bytes, in bytes: a worker's base, what the
    /// parser holds at once, and the core's staging buffer before it spills to disk. The memory
    /// budget uses it (gap A2, ADR-0007).
    ///
    /// M2c measured ≈ 8 × size (TXT) and ≈ 10 × size (CSV) with the whole file and result in
    /// memory (`docs/10-walking-skeleton.md` §S2a). Since M3a every format but EML streams and
    /// the core spills a result over 32 MB, so the need stops growing past a window.
    /// ASSUMPTION: 4 × a 64 MB window (one line, row or value at a time; one DOCX entry is
    /// capped at 256 MB but read as a stream), 4 × a 256 MB window for MBOX (one message at a
    /// time), 4 × size for EML. Checked in watch items W3a-1/2 (`docs/11-ingestion.md`).
    ///
    /// PDF and images (M3b) hold the whole file, and a page at a time is rendered and OCR'd.
    /// ASSUMPTION: 2 × size + a 256 MB render/OCR window (a 300 DPI A4 page is 8.7 MB of grey
    /// pixels; Tesseract's working set is ~100 MB). Checked on TD-1 and TD-3
    /// (`docs/11-ingestion.md` §7).
    pub fn memory_estimate(&self, size: u64) -> u64 {
        const BASE: u64 = 40 * MB;
        const STAGING: u64 = 32 * MB;
        if self.is_paged() {
            return BASE
                .saturating_add(STAGING)
                .saturating_add(size.saturating_mul(2))
                .saturating_add(256 * MB);
        }
        let window = match self {
            Format::Mbox => size.min(256 * MB),
            Format::Eml => size,
            _ => size.min(64 * MB),
        };
        BASE.saturating_add(STAGING)
            .saturating_add(window.saturating_mul(4))
    }
}

/// Row state of a segment: CSV rows with the wrong number of cells are kept, marked
/// (INV-CO-10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RowState {
    Normal,
    Malformed,
}

/// One segment: its locator (INV-CO-03), its stored text, for CSV rows where each cell starts in
/// that text, and for OCR'd pages the word geometry.
#[derive(Clone, PartialEq, Debug)]
pub struct ParsedSegment {
    pub locator: Locator,
    pub text: String,
    pub row_state: RowState,
    /// CSV rows only: the character offset of each cell in `text`. Cells are joined by one
    /// `\t`, so cell `i` ends one character before cell `i + 1` starts; the last ends at the
    /// end of `text`. Cell values may hold tabs themselves, so the table, not the tabs, is the
    /// truth.
    pub cell_starts: Vec<u32>,
    /// OCR'd pages only: each word's span, box and confidence (US-02.2 AC3). Mention locators
    /// and confidences come from it.
    pub ocr: Option<OcrGeometry>,
}

impl ParsedSegment {
    /// A segment whose locator covers all of `text`: `place` is the locator kind with any span.
    /// `None` for empty text (a span is never empty).
    pub(crate) fn whole(place: impl FnOnce(Span) -> Locator, text: String) -> Option<Self> {
        let len = text.chars().count();
        let span = Span::new(0, len, len).ok()?;
        Some(ParsedSegment {
            locator: place(span),
            text,
            row_state: RowState::Normal,
            cell_starts: Vec::new(),
            ocr: None,
        })
    }

    /// The OCR confidence of `span`: the lowest of the words it touches (INV-ID-03). `None`
    /// for a segment that was not OCR'd.
    pub fn ocr_confidence(&self, span: Span) -> Option<f32> {
        crate::paged::ocr_confidence(self.ocr.as_ref(), &self.text, span)
    }

    /// The cell `span` lies in, as `(index, cell span start, cell span end)`. `None` for a
    /// segment without cells or a span that crosses a cell edge.
    pub fn cell_of(&self, span: Span) -> Option<(usize, usize, usize)> {
        if self.cell_starts.is_empty() {
            return None;
        }
        let i = self
            .cell_starts
            .partition_point(|s| *s as usize <= span.start())
            .checked_sub(1)?;
        let start = self.cell_starts[i] as usize;
        let end = match self.cell_starts.get(i + 1) {
            Some(next) => *next as usize - 1,
            None => self.text.chars().count(),
        };
        (span.end() <= end).then_some((i, start, end))
    }
}

/// The locator of a mention found at `span` of `segment`'s text (INV-CO-03/04): the segment's
/// place with the mention's span, for CSV the row + column, and for OCR text the boxes of the
/// words the span touches. `columns` are the document's
/// CSV column names. `None` if the span is past the text, a CSV span crosses a cell edge, or
/// the segment is a `Message-ID` header, which is not scanned for identifiers (ASSUMPTION: an
/// id, not an address).
pub fn mention_locator(
    segment: &ParsedSegment,
    columns: &[Option<String>],
    span: Span,
) -> Option<Locator> {
    if span.end() > segment.text.chars().count() {
        return None;
    }
    match &segment.locator {
        Locator::CsvRow { row } => {
            let (i, _, _) = segment.cell_of(span)?;
            let column = match columns.get(i) {
                Some(Some(name)) => Column::Name(name.clone()),
                _ => Column::Index(i as u32),
            };
            Some(Locator::CsvCell {
                row: *row,
                column,
                span,
            })
        }
        Locator::EmailHeader { name, .. } if name.eq_ignore_ascii_case("Message-ID") => None,
        Locator::OcrText { page, .. } => segment
            .ocr
            .as_ref()?
            .locator(*page, &segment.text, span)
            .ok(),
        other => other.with_span(span),
    }
}

/// One document of a content. TXT/MD/CSV/JSON/HTML/DOCX/EML/PDF/image files give one each; an MBOX
/// gives one per message, and an email attachment gives a child document (INV-CO-05).
#[derive(Clone, PartialEq, Debug)]
pub struct ParsedDocument {
    pub kind: Format,
    /// Where the document sits in its content: `"whole"`, `"message 3"` (0-based, MBOX), or
    /// `"part 2: name.txt"` (an attachment, by IMAP section number).
    pub position: String,
    /// Index of the parent document in the same content (attachments), started before this.
    pub parent: Option<u32>,
    /// CSV only: the header name of each column when the file has a header and the name is
    /// unique and non-empty, `None` otherwise. Empty for other formats.
    pub columns: Vec<Option<String>>,
    pub segments: Vec<ParsedSegment>,
}

impl ParsedDocument {
    /// See [`mention_locator`]. `None` also if the segment does not exist.
    pub fn mention_locator(&self, segment: usize, span: Span) -> Option<Locator> {
        mention_locator(self.segments.get(segment)?, &self.columns, span)
    }
}

/// Everything a parser returns for one content (US-02.3 – US-02.6).
#[derive(Clone, PartialEq, Debug, Default)]
pub struct ParsedContent {
    pub documents: Vec<ParsedDocument>,
    /// Plain-English notes that do not fail the file, such as malformed rows (US-02.4 AC3).
    pub warnings: Vec<String>,
}

impl ParsedContent {
    pub fn segment_count(&self) -> usize {
        self.documents.iter().map(|d| d.segments.len()).sum()
    }
}

/// Why a file could not be parsed: a stable code and a plain-English message (NFR-USE-05,
/// INV-CO-09).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ParseError {
    pub code: &'static str,
    pub message: String,
}

impl ParseError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// Bytes in none of the read encodings (US-02.3 AC3, NFR-COMP-06).
    pub fn unknown_encoding() -> Self {
        Self::new("unknown_text_encoding", "Unknown text encoding")
    }

    pub(crate) fn io(e: std::io::Error) -> Self {
        Self::new(
            "file_unreadable",
            format!("The file could not be read: {e}"),
        )
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// What a parser reads: a file, or bytes in memory. Parsers rewind it when they need a second
/// pass (encoding detection, the CSV sample).
pub trait Source: Read + Seek {}

impl<T: Read + Seek> Source for T {}

/// The start of a document, given to [`Sink::document`].
#[derive(Clone, PartialEq, Debug)]
pub struct DocumentStart {
    pub kind: Format,
    pub position: String,
    /// Index returned by an earlier [`Sink::document`] call.
    pub parent: Option<u32>,
    pub columns: Vec<Option<String>>,
}

/// Where a streaming parser puts what it reads, in order (ADR-0013 rule 1, §4 #16): a parser
/// never holds a whole file's result. Segments belong to the last document started. A
/// document's own segments all come before any child document starts.
///
/// An error from the sink (the output is gone) stops the parse and is returned as is.
pub trait Sink {
    /// Starts a document and returns its index (0, 1, 2, … in call order).
    fn document(&mut self, start: DocumentStart) -> Result<u32, ParseError>;
    fn segment(&mut self, segment: ParsedSegment) -> Result<(), ParseError>;
    fn warning(&mut self, warning: String);
    /// Units of work finished, for heartbeats. MBOX counts messages (ADR-0016), PDF and images
    /// pages (ADR-0017); the other formats do not call it.
    fn progress(&mut self, _units: u64) {}
    /// A page's OCR (render or decode, then recognise) starts (`true`) or ends (`false`); it has
    /// its own deadline (ADR-0017). Only PDF and images call it.
    fn ocr(&mut self, _active: bool) {}
}

/// A [`Sink`] that keeps everything: the result of [`parse`].
#[derive(Default)]
pub struct Collector {
    pub content: ParsedContent,
}

impl Sink for Collector {
    fn document(&mut self, start: DocumentStart) -> Result<u32, ParseError> {
        let index = self.content.documents.len() as u32;
        self.content.documents.push(ParsedDocument {
            kind: start.kind,
            position: start.position,
            parent: start.parent,
            columns: start.columns,
            segments: Vec::new(),
        });
        Ok(index)
    }

    fn segment(&mut self, segment: ParsedSegment) -> Result<(), ParseError> {
        self.content
            .documents
            .last_mut()
            .ok_or_else(|| ParseError::new("internal_error", "A segment came before its document"))?
            .segments
            .push(segment);
        Ok(())
    }

    fn warning(&mut self, warning: String) {
        self.content.warnings.push(warning);
    }
}

impl Collector {
    /// Sends what was collected on to `sink`: documents with no parent get `parent`, the others
    /// keep theirs, renumbered.
    pub(crate) fn replay(self, sink: &mut dyn Sink, parent: Option<u32>) -> Result<(), ParseError> {
        let mut index = Vec::with_capacity(self.content.documents.len());
        for doc in self.content.documents {
            let start = DocumentStart {
                kind: doc.kind,
                position: doc.position,
                parent: match doc.parent {
                    None => parent,
                    Some(p) => index.get(p as usize).copied(),
                },
                columns: doc.columns,
            };
            index.push(sink.document(start)?);
            for seg in doc.segments {
                sink.segment(seg)?;
            }
        }
        for w in self.content.warnings {
            sink.warning(w);
        }
        Ok(())
    }
}

/// Where a document goes: its position, its parent, and how many attachments deep it is.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Place {
    pub position: String,
    pub parent: Option<u32>,
    pub depth: u32,
}

impl Place {
    pub(crate) fn whole() -> Self {
        Place {
            position: "whole".into(),
            parent: None,
            depth: 0,
        }
    }

    pub(crate) fn start(&self, kind: Format, columns: Vec<Option<String>>) -> DocumentStart {
        DocumentStart {
            kind,
            position: self.position.clone(),
            parent: self.parent,
            columns,
        }
    }
}

/// Parses one content as a stream from its start, putting documents, segments and warnings into
/// `sink`. A content of zero bytes succeeds with one empty document whatever its format (none for
/// MBOX, whose documents are messages): the store keeps one content per hash, so every empty
/// file must share one outcome (TD-3 `empty.*`). PDFs and images use `engines`.
pub fn parse_stream(
    format: Format,
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    let len = input
        .seek(std::io::SeekFrom::End(0))
        .map_err(ParseError::io)?;
    input
        .seek(std::io::SeekFrom::Start(0))
        .map_err(ParseError::io)?;
    let place = Place::whole();
    if len == 0 {
        if format != Format::Mbox {
            sink.document(place.start(format, Vec::new()))?;
        }
        return Ok(());
    }
    parse_at(format, input, sink, &place, engines)
}

pub(crate) fn parse_at(
    format: Format,
    input: &mut dyn Source,
    sink: &mut dyn Sink,
    place: &Place,
    engines: &mut Engines,
) -> Result<(), ParseError> {
    match format {
        Format::Text | Format::Markdown => crate::txt::parse(format, input, sink, place),
        Format::Csv => crate::csv::parse(input, sink, place),
        Format::Json => crate::json::parse(input, sink, place),
        Format::Html => crate::html::parse(input, sink, place),
        Format::Docx => crate::docx::parse(input, sink, place),
        Format::Eml => crate::email::parse_eml(input, sink, place, engines),
        Format::Mbox => crate::email::parse_mbox(input, sink, place, engines),
        Format::Pdf | Format::Image => crate::paged::parse(format, input, sink, place, engines),
    }
}

/// Parses one content's bytes in the given format and keeps the whole result, without OCR. For
/// tests and in-process benchmarks; workers stream with [`parse_stream`].
pub fn parse(format: Format, bytes: &[u8]) -> Result<ParsedContent, ParseError> {
    parse_with(format, bytes, &mut Engines::default())
}

/// [`parse`] with the given engines.
pub fn parse_with(
    format: Format,
    bytes: &[u8],
    engines: &mut Engines,
) -> Result<ParsedContent, ParseError> {
    let mut sink = Collector::default();
    parse_stream(format, &mut std::io::Cursor::new(bytes), &mut sink, engines)?;
    Ok(sink.content)
}
