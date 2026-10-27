use std::fmt;

use mk_kernel::{Column, Locator, Span};

/// File formats with a parser. In M2b: TXT, MD and CSV, UTF-8 only.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Format {
    Text,
    Markdown,
    Csv,
}

impl Format {
    pub const ALL: [Format; 3] = [Format::Text, Format::Markdown, Format::Csv];

    /// Stable name, stored as the content's file type and the document kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            Format::Text => "txt",
            Format::Markdown => "md",
            Format::Csv => "csv",
        }
    }

    /// The format for a file name extension, ignoring case.
    pub fn from_extension(ext: &str) -> Option<Format> {
        Self::ALL
            .into_iter()
            .find(|f| f.as_str().eq_ignore_ascii_case(ext))
    }

    /// The format for a stored file type (`as_str`).
    pub fn from_name(name: &str) -> Option<Format> {
        Self::ALL.into_iter().find(|f| f.as_str() == name)
    }
}

/// Row state of a segment: CSV rows with the wrong number of cells are kept, marked
/// (INV-CO-10).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RowState {
    Normal,
    Malformed,
}

/// One segment: its locator (INV-CO-03), its stored text, and for CSV rows where each cell
/// starts in that text.
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
}

impl ParsedSegment {
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

/// One document of a content. TXT/MD/CSV files give one each; MBOX and attachments give more
/// (M3).
#[derive(Clone, PartialEq, Debug)]
pub struct ParsedDocument {
    pub kind: Format,
    /// Where the document sits in its content. `"whole"` for single-document files.
    pub position: String,
    /// CSV only: the header name of each column when the file has a header and the name is
    /// unique and non-empty, `None` otherwise. Empty for other formats.
    pub columns: Vec<Option<String>>,
    pub segments: Vec<ParsedSegment>,
}

impl ParsedDocument {
    /// The locator of a mention found at `span` of segment `segment`'s text (INV-CO-03/04):
    /// line + span for TXT/MD, row + column + span for CSV. `None` if the segment does not
    /// exist, the span is past its text, or a CSV span crosses a cell edge.
    pub fn mention_locator(&self, segment: usize, span: Span) -> Option<Locator> {
        let seg = self.segments.get(segment)?;
        if span.end() > seg.text.chars().count() {
            return None;
        }
        match &seg.locator {
            Locator::TextLine { line, .. } => Some(Locator::TextLine { line: *line, span }),
            Locator::CsvRow { row } => {
                let (i, _, _) = seg.cell_of(span)?;
                let column = match self.columns.get(i) {
                    Some(Some(name)) => Column::Name(name.clone()),
                    _ => Column::Index(i as u32),
                };
                Some(Locator::CsvCell {
                    row: *row,
                    column,
                    span,
                })
            }
            _ => None,
        }
    }
}

/// Everything a parser returns for one content (US-02.3, US-02.4).
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

    /// Bytes that are not UTF-8. Other encodings come in M3.
    pub fn unknown_encoding() -> Self {
        Self::new("unknown_text_encoding", "Unknown text encoding")
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ParseError {}

/// UTF-8 text with any byte-order mark removed, or `unknown_text_encoding`.
pub(crate) fn decode(bytes: &[u8]) -> Result<&str, ParseError> {
    let text = std::str::from_utf8(bytes).map_err(|_| ParseError::unknown_encoding())?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(text))
}

/// Parses one content's bytes in the given format.
pub fn parse(format: Format, bytes: &[u8]) -> Result<ParsedContent, ParseError> {
    match format {
        Format::Text | Format::Markdown => crate::txt::parse(format, bytes),
        Format::Csv => crate::csv::parse(bytes),
    }
}
