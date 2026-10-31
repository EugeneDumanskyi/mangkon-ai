//! The ground-truth format (schema version 1). Field-by-field description: `docs/07-test-corpus.md` §5.
//! Machine-checkable form: `crates/mk-bench/schema/ground-truth.schema.json`, produced from these types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

/// The whole ground truth of one corpus (TD-1). Stored as `ground-truth.json`; the page
/// reference text is stored next to it as JSON Lines (`page_text_file`), one page per line.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GroundTruth {
    pub schema_version: u32,
    pub generator: Generator,
    /// Path of the page-text JSON Lines file, relative to the ground-truth file.
    pub page_text_file: String,
    pub files: Vec<FileEntry>,
    pub documents: Vec<Document>,
    pub entities: Vec<Entity>,
    pub mentions: Vec<Mention>,
    pub negatives: Vec<Negative>,
    pub links: Vec<Link>,
    pub merges: Vec<Merge>,
    pub relations: Vec<Relation>,
    pub events: Vec<Event>,
    /// Loaded from `page_text_file`; not part of `ground-truth.json` itself.
    #[serde(skip)]
    pub page_text: Vec<PageText>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Generator {
    pub name: String,
    /// Bumped whenever output bytes change on purpose.
    pub version: u32,
    pub seed: u64,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    Synthetic,
    Public,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// PDF with a real text layer.
    PdfText,
    /// PDF whose pages are images only.
    PdfScan,
    /// PDF with both text pages and image-only pages.
    PdfMixed,
    /// Scanned pages with an invisible text layer (render mode 3), as OCR tools write.
    PdfOcrLayer,
    /// PNG, JPEG or TIFF scan.
    Image,
    Eml,
    Mbox,
    Csv,
    Json,
    Docx,
    Html,
    Txt,
    Md,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum ScanTier {
    /// 300 DPI, straight, no noise. The NFR-QA-07 tier.
    Clean,
    /// Up to 1.5° skew, light noise, slight blur, JPEG q75.
    Office,
    /// 200 DPI, 3° skew, speckle, low contrast.
    Poor,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Annotation {
    /// Every mention in the file is listed, with exact locations. Precision and recall can be scored.
    Full,
    /// Listed mentions are true, but the list may be incomplete and has page-level locations only.
    /// Only recall can be scored.
    Partial,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FileEntry {
    /// Path relative to the corpus root (the directory holding `ground-truth.json`), `/`-separated.
    pub path: String,
    /// SHA-256 of the file bytes, lowercase hex.
    pub sha256: String,
    pub bytes: u64,
    pub origin: Origin,
    pub kind: FileKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pages: Option<u32>,
    /// Text encoding for text formats: `utf-8`, `utf-8-bom`, `utf-16le-bom`, `windows-1252`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encoding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scan_tier: Option<ScanTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    pub annotation: Annotation,
    /// What the file is for in the corpus (one line).
    pub purpose: String,
}

/// A document: the unit that mentions and connections count (domain BC-02). Most files are one
/// document; an MBOX holds one per message; an attachment is a child document (INV-CO-05).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Document {
    pub id: String,
    /// `FileEntry::path` of the file that holds this document.
    pub file: String,
    pub part: DocPart,
    /// The document kind, which decides the location kind of its mentions (INV-CO-03).
    pub kind: FileKind,
    /// Parent document id, for attachments. Exactly one parent, no cycles (INV-CO-05).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// SHA-256 of the decoded attachment bytes, for child documents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// CSV and JSON documents: every column with its correct type, labelled by hand
    /// (NFR-QA-13, BC-05). Generator v6.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<ColumnTruth>,
}

/// One column of a table document and its correct type.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColumnTruth {
    /// CSV: the header text; JSON: the pointer with array indices as `*` (`/officers/*/name`).
    pub column_key: String,
    /// `email`, `phone`, `url_domain`, `iban`, `crypto`, `person`, `org`, `date` or `ignore`.
    #[serde(rename = "type")]
    pub column_type: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DocPart {
    /// The whole file.
    Whole,
    /// Message `index` (0-based, file order) of an MBOX.
    MboxMessage { index: u32 },
    /// A MIME attachment of the parent message. `mime_part` is the IMAP-style section number.
    Attachment { mime_part: String, filename: String },
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Person,
    Org,
}

/// A person or organization of the story. `aliases` are the other surface forms the corpus uses;
/// all of them must end up in one entity (E11 merge truth).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    pub id: String,
    pub kind: EntityKind,
    pub canonical: String,
    pub aliases: Vec<String>,
    /// Role in the story (one line).
    pub role: String,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum IdType {
    Email,
    Phone,
    Url,
    Domain,
    Iban,
    Btc,
    Evm,
    Sol,
    Tron,
    /// A user-defined pattern (US-04.7); `IdentifierKey::pattern` names it.
    Custom,
}

/// Identifier type + normalized value: the identity of an identifier (domain §3.3).
#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(deny_unknown_fields)]
pub struct IdentifierKey {
    #[serde(rename = "type")]
    pub id_type: IdType,
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Subject {
    Identifier {
        key: IdentifierKey,
    },
    /// `name_key` is the NameKey of the mention's raw text (case, spaces, punctuation and legal
    /// suffixes normalized, US-11.1). Variants with different name keys are tied together only by
    /// `entity` (see `merges`).
    Entity {
        entity: String,
        name_key: String,
    },
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Normal,
    /// Split across lines or runs, rotated, out of reading order, or similar.
    Hard,
}

/// One true mention of an identifier or entity at one place in one document.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mention {
    pub id: String,
    pub document: String,
    pub subject: Subject,
    /// The text exactly as it appears in the document (line breaks inside it removed).
    pub raw: String,
    pub location: Location,
    pub difficulty: Difficulty,
    /// For domain identifiers that come out of an email address or URL (US-04.3 AC1/AC2):
    /// the mention they come from. Same location and raw text as that mention.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `[x0, y0, x1, y1]`, top-left origin, y down. Units depend on the location kind.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq)]
pub struct BBox(pub [f64; 4]);

impl BBox {
    pub fn x0(&self) -> f64 {
        self.0[0]
    }
    pub fn y0(&self) -> f64 {
        self.0[1]
    }
    pub fn x1(&self) -> f64 {
        self.0[2]
    }
    pub fn y1(&self) -> f64 {
        self.0[3]
    }
}

/// Where a mention is, in the format's own terms. Mirrors the INV-CO-03 locator kinds.
/// All `start`/`end` pairs are half-open offsets counted in Unicode scalar values (Rust `char`s).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Location {
    /// PDF text layer. `page` is 1-based. Boxes are in PDF points in the page's unrotated
    /// MediaBox space, top-left origin, y down; one box per line the mention covers.
    PdfText { page: u32, boxes: Vec<BBox> },
    /// Scanned page (image file, or image-only PDF page). Boxes in pixels at `dpi`, axis-aligned
    /// around the (possibly skewed) glyphs, top-left origin.
    Scan {
        page: u32,
        dpi: u32,
        boxes: Vec<BBox>,
    },
    /// Page only. Used by `partial` annotations of public files.
    PdfPage { page: u32 },
    /// TXT/MD: `line` is 1-based, after decoding; `start`/`end` within that line.
    TextLine { line: u32, start: u32, end: u32 },
    /// DOCX: `paragraph` is the 0-based index of `w:p` in body order; offsets within the
    /// concatenated `w:t` text of that paragraph.
    DocxParagraph {
        paragraph: u32,
        start: u32,
        end: u32,
    },
    /// CSV: `row` 0 is the header row; `column` is 0-based; offsets within the unquoted cell value.
    CsvCell {
        row: u32,
        column: u32,
        start: u32,
        end: u32,
    },
    /// JSON: RFC 6901 JSON Pointer to a string value; offsets within the decoded string.
    JsonValue {
        pointer: String,
        start: u32,
        end: u32,
    },
    /// Email header of the document's message, offsets within the decoded (RFC 2047) value.
    EmailHeader {
        header: String,
        start: u32,
        end: u32,
    },
    /// Email body part (IMAP-style section number), offsets within the decoded part text.
    MimePart { part: String, start: u32, end: u32 },
    /// HTML: offsets within the decoded HTML source (not the rendered text).
    HtmlSource { start: u32, end: u32 },
}

impl Location {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Location::PdfText { .. } => "pdf_text",
            Location::Scan { .. } => "scan",
            Location::PdfPage { .. } => "pdf_page",
            Location::TextLine { .. } => "text_line",
            Location::DocxParagraph { .. } => "docx_paragraph",
            Location::CsvCell { .. } => "csv_cell",
            Location::JsonValue { .. } => "json_value",
            Location::EmailHeader { .. } => "email_header",
            Location::MimePart { .. } => "mime_part",
            Location::HtmlSource { .. } => "html_source",
        }
    }

    /// The start/end offsets, for kinds that have them.
    pub fn offsets(&self) -> Option<(u32, u32)> {
        match *self {
            Location::TextLine { start, end, .. }
            | Location::DocxParagraph { start, end, .. }
            | Location::CsvCell { start, end, .. }
            | Location::JsonValue { start, end, .. }
            | Location::EmailHeader { start, end, .. }
            | Location::MimePart { start, end, .. }
            | Location::HtmlSource { start, end } => Some((start, end)),
            _ => None,
        }
    }
}

/// Things that must **not** become identifiers, connections or merges.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Negative {
    /// Looks like `claimed_type` but its checksum fails (US-04.4 AC2, US-04.5).
    BadChecksum {
        id: String,
        claimed_type: IdType,
        raw: String,
        document: String,
        location: Location,
    },
    /// Passes Luhn: flag the document only, never an identifier or connection (INV-ID-06, INV-LK-03).
    CardLike {
        id: String,
        raw: String,
        last4: String,
        document: String,
        location: Location,
    },
    /// A stop-list domain: may be stored but never becomes a connection (US-04.3 AC3, INV-LK-03).
    StopListDomain {
        id: String,
        domain: String,
        document: String,
        location: Location,
    },
    /// Two entities with similar names that must stay apart.
    DistinctEntities {
        id: String,
        a: String,
        b: String,
        note: String,
    },
}

impl Negative {
    pub fn id(&self) -> &str {
        match self {
            Negative::BadChecksum { id, .. }
            | Negative::CardLike { id, .. }
            | Negative::StopListDomain { id, .. }
            | Negative::DistinctEntities { id, .. } => id,
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LinkSubject {
    Identifier { key: IdentifierKey },
    Entity { entity: String },
}

/// A true connection: a subject with mentions in 2 or more distinct documents (INV-LK-01/02).
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub subject: LinkSubject,
    /// Distinct document ids, sorted.
    pub documents: Vec<String>,
    pub mentions: Vec<String>,
}

/// The raw name variants that must end up in one entity.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Merge {
    pub entity: String,
    pub variants: Vec<String>,
}

#[derive(
    Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
)]
#[serde(rename_all = "snake_case")]
pub enum RelationType {
    WorksFor,
    Owns,
    DirectorOf,
    Paid,
    SentTo,
    RelatedTo,
}

/// Layer 3 relation (INV-AI-06): subject and object are entity mentions in the same segment.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Relation {
    pub id: String,
    #[serde(rename = "type")]
    pub rel_type: RelationType,
    pub subject: String,
    pub object: String,
    pub document: String,
    /// Mention ids: the subject mention first, then the object mention.
    pub evidence: Vec<String>,
}

/// Layer 3 event (INV-AI-07): a full or partial ISO 8601 date, or `unknown`.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub id: String,
    pub date: String,
    pub description: String,
    pub document: String,
    /// Mention ids of the entities taking part.
    pub evidence: Vec<String>,
    /// Exact text of the sentence that states the event.
    pub quote: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// PDF points (1/72 inch).
    Pt,
    /// Pixels at `dpi`.
    Px,
}

/// Exact reference text of one synthetic PDF or scan page, with word boxes. Used for OCR
/// character accuracy (NFR-QA-07) and the M0b/M0c highlight round-trip.
#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PageText {
    pub document: String,
    /// 1-based.
    pub page: u32,
    pub unit: Unit,
    /// Unrotated page size in `unit`.
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dpi: Option<u32>,
    /// PDF `/Rotate` (0, 90, 180, 270).
    pub rotate: u32,
    /// Scan skew in degrees, counter-clockwise positive. Boxes already include it.
    pub skew_deg: f64,
    /// Reading-order text: lines joined with `\n`, words with one space.
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Serialize, Deserialize, JsonSchema, Debug, Clone, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Word {
    pub t: String,
    pub b: BBox,
}
