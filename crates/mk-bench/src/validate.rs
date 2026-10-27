//! `GroundTruth::validate`: referential integrity and self-consistency of the ground truth.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use crate::checks;
use crate::model::*;

const BOX_TOLERANCE: f64 = 0.01;

impl GroundTruth {
    /// Checks the ground truth. With `root` (the corpus directory), also checks every file's
    /// size and SHA-256 on disk. Returns every problem found, not just the first.
    pub fn validate(&self, root: Option<&Path>) -> Result<(), Vec<String>> {
        let mut v = Validator {
            gt: self,
            errors: Vec::new(),
        };
        v.run(root);
        if v.errors.is_empty() {
            Ok(())
        } else {
            Err(v.errors)
        }
    }
}

struct Validator<'a> {
    gt: &'a GroundTruth,
    errors: Vec<String>,
}

macro_rules! fail {
    ($v:expr, $($arg:tt)*) => { $v.errors.push(format!($($arg)*)) };
}

impl<'a> Validator<'a> {
    fn run(&mut self, root: Option<&Path>) {
        let gt = self.gt;
        if gt.schema_version != SCHEMA_VERSION {
            fail!(
                self,
                "schema_version {} (expected {SCHEMA_VERSION})",
                gt.schema_version
            );
        }
        self.unique("file", gt.files.iter().map(|f| f.path.as_str()));
        self.unique("document", gt.documents.iter().map(|d| d.id.as_str()));
        self.unique("entity", gt.entities.iter().map(|e| e.id.as_str()));
        self.unique("mention", gt.mentions.iter().map(|m| m.id.as_str()));
        self.unique("negative", gt.negatives.iter().map(Negative::id));
        self.unique("relation", gt.relations.iter().map(|r| r.id.as_str()));
        self.unique("event", gt.events.iter().map(|e| e.id.as_str()));

        self.documents();
        self.entities();
        let pages = self.page_text();
        self.mentions(&pages);
        self.negatives(&pages);
        self.links();
        self.merges();
        self.relations_and_events();
        if let Some(root) = root {
            self.files_on_disk(root);
        }
    }

    fn unique<'s>(&mut self, what: &str, ids: impl Iterator<Item = &'s str>) {
        let mut seen = HashSet::new();
        for id in ids {
            if id.is_empty() {
                fail!(self, "{what}: empty id");
            }
            if !seen.insert(id) {
                fail!(self, "{what} {id}: duplicate id");
            }
        }
    }

    fn file(&self, path: &str) -> Option<&'a FileEntry> {
        self.gt.files.iter().find(|f| f.path == path)
    }

    fn document(&self, id: &str) -> Option<&'a Document> {
        self.gt.documents.iter().find(|d| d.id == id)
    }

    fn entity(&self, id: &str) -> Option<&'a Entity> {
        self.gt.entities.iter().find(|e| e.id == id)
    }

    fn documents(&mut self) {
        let gt = self.gt;
        let mut files_with_docs = HashSet::new();
        for d in &gt.documents {
            if self.file(&d.file).is_none() {
                fail!(self, "document {}: unknown file {}", d.id, d.file);
            }
            files_with_docs.insert(d.file.as_str());
            match (&d.part, &d.parent) {
                (DocPart::Attachment { .. }, None) => {
                    fail!(self, "document {}: attachment without parent", d.id)
                }
                (DocPart::Attachment { .. }, Some(p)) => match self.document(p) {
                    None => fail!(self, "document {}: unknown parent {p}", d.id),
                    Some(parent) if parent.file != d.file => {
                        fail!(self, "document {}: parent {p} is in another file", d.id)
                    }
                    Some(_) => {}
                },
                (_, Some(_)) => fail!(self, "document {}: only attachments have a parent", d.id),
                (_, None) => {}
            }
            // INV-CO-05: no cycles. Walk up at most `documents.len()` steps.
            let mut cur = d;
            for _ in 0..=gt.documents.len() {
                match cur.parent.as_deref().and_then(|p| self.document(p)) {
                    Some(p) if p.id == d.id => {
                        fail!(self, "document {}: parent cycle", d.id);
                        break;
                    }
                    Some(p) => cur = p,
                    None => break,
                }
            }
        }
        for f in &gt.files {
            if !files_with_docs.contains(f.path.as_str()) {
                fail!(self, "file {}: no document", f.path);
            }
        }
    }

    fn entities(&mut self) {
        for e in &self.gt.entities {
            if e.aliases.iter().any(|a| a == &e.canonical) {
                fail!(self, "entity {}: alias repeats the canonical name", e.id);
            }
        }
    }

    /// Checks page text and returns (document, page) → (width, height, unit).
    fn page_text(&mut self) -> HashMap<(String, u32), (f64, f64, Unit)> {
        let mut pages = HashMap::new();
        for p in &self.gt.page_text {
            let key = (p.document.clone(), p.page);
            if self.document(&p.document).is_none() {
                fail!(
                    self,
                    "page_text {}#{}: unknown document",
                    p.document,
                    p.page
                );
            }
            if p.page == 0 {
                fail!(self, "page_text {}: page numbers are 1-based", p.document);
            }
            if pages.insert(key, (p.width, p.height, p.unit)).is_some() {
                fail!(self, "page_text {}#{}: duplicate page", p.document, p.page);
            }
            let from_words: Vec<&str> = p.words.iter().map(|w| w.t.as_str()).collect();
            let from_text: Vec<&str> = p.text.split_whitespace().collect();
            if from_words != from_text {
                fail!(
                    self,
                    "page_text {}#{}: words do not match text",
                    p.document,
                    p.page
                );
            }
            for w in &p.words {
                if !box_ok(&w.b, p.width, p.height) {
                    fail!(
                        self,
                        "page_text {}#{}: word {:?} box {:?} outside page",
                        p.document,
                        p.page,
                        w.t,
                        w.b.0
                    );
                }
            }
        }
        pages
    }

    fn location(
        &mut self,
        what: &str,
        doc: &Document,
        loc: &Location,
        raw: &str,
        pages: &HashMap<(String, u32), (f64, f64, Unit)>,
    ) {
        use FileKind::*;
        let ok_kind = match loc {
            Location::PdfText { .. } => matches!(doc.kind, PdfText | PdfMixed | PdfOcrLayer),
            Location::Scan { .. } => matches!(doc.kind, Image | PdfScan | PdfMixed),
            Location::PdfPage { .. } => {
                matches!(doc.kind, PdfText | PdfScan | PdfMixed | PdfOcrLayer)
            }
            Location::TextLine { .. } => matches!(doc.kind, Txt | Md),
            Location::DocxParagraph { .. } => doc.kind == Docx,
            Location::CsvCell { .. } => doc.kind == Csv,
            Location::JsonValue { .. } => doc.kind == Json,
            Location::EmailHeader { .. } | Location::MimePart { .. } => {
                matches!(doc.kind, Eml | Mbox)
            }
            Location::HtmlSource { .. } => doc.kind == Html,
        };
        if !ok_kind {
            fail!(
                self,
                "{what}: location {} does not fit document kind {:?} (INV-CO-03)",
                loc.kind_name(),
                doc.kind
            );
        }
        let annotation = self.file(&doc.file).map(|f| f.annotation);
        if matches!(loc, Location::PdfPage { .. }) && annotation == Some(Annotation::Full) {
            fail!(self, "{what}: page-only location in a fully annotated file");
        }
        if let Some((start, end)) = loc.offsets() {
            if start >= end {
                fail!(self, "{what}: empty span {start}..{end}");
            } else if (end - start) as usize != raw.chars().count() {
                fail!(
                    self,
                    "{what}: span {start}..{end} does not match raw length {}",
                    raw.chars().count()
                );
            }
        }
        let (page, boxes, unit) = match loc {
            Location::PdfText { page, boxes } => (*page, boxes, Unit::Pt),
            Location::Scan { page, boxes, .. } => (*page, boxes, Unit::Px),
            _ => return,
        };
        if boxes.is_empty() {
            fail!(self, "{what}: no boxes");
        }
        match pages.get(&(doc.id.clone(), page)) {
            None => fail!(self, "{what}: no page_text for page {page}"),
            Some(&(w, h, u)) => {
                if u != unit {
                    fail!(self, "{what}: box unit does not match page unit");
                }
                for b in boxes {
                    if !box_ok(b, w, h) {
                        fail!(self, "{what}: box {:?} outside page {w}x{h}", b.0);
                    }
                }
            }
        }
    }

    fn mentions(&mut self, pages: &HashMap<(String, u32), (f64, f64, Unit)>) {
        let gt = self.gt;
        let by_id: HashMap<&str, &Mention> =
            gt.mentions.iter().map(|m| (m.id.as_str(), m)).collect();
        for m in &gt.mentions {
            let what = format!("mention {}", m.id);
            let Some(doc) = self.document(&m.document) else {
                fail!(self, "{what}: unknown document {}", m.document);
                continue;
            };
            self.location(&what, doc, &m.location, &m.raw, pages);
            match &m.subject {
                Subject::Entity { entity, name_key } => match self.entity(entity) {
                    None => fail!(self, "{what}: unknown entity {entity}"),
                    Some(e) => {
                        if *name_key != crate::keys::name_key(e.kind, &m.raw) {
                            fail!(
                                self,
                                "{what}: name_key {name_key} is not the key of {:?}",
                                m.raw
                            );
                        }
                        if m.raw != e.canonical && !e.aliases.contains(&m.raw) {
                            fail!(
                                self,
                                "{what}: {:?} is neither canonical nor an alias of {entity}",
                                m.raw
                            );
                        }
                    }
                },
                Subject::Identifier { key } => {
                    if let Err(e) = identifier_consistent(key, &m.raw, m.derived_from.is_some()) {
                        fail!(self, "{what}: {e}");
                    }
                }
            }
            if let Some(src) = &m.derived_from {
                match by_id.get(src.as_str()) {
                    None => fail!(self, "{what}: derived_from unknown mention {src}"),
                    Some(s) if s.document != m.document || s.location != m.location => {
                        fail!(self, "{what}: derived_from {src} is at another place")
                    }
                    Some(_) => {}
                }
            }
        }
    }

    fn negatives(&mut self, pages: &HashMap<(String, u32), (f64, f64, Unit)>) {
        for n in &self.gt.negatives {
            let what = format!("negative {}", n.id());
            match n {
                Negative::BadChecksum {
                    claimed_type,
                    raw,
                    document,
                    location,
                    ..
                } => {
                    if let Some(doc) = self.document(document) {
                        self.location(&what, doc, location, raw, pages);
                    } else {
                        fail!(self, "{what}: unknown document {document}");
                    }
                    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
                    let passes = match claimed_type {
                        IdType::Iban => checks::iban_valid(&compact.to_ascii_uppercase()),
                        IdType::Btc => checks::btc_valid(&compact),
                        IdType::Evm => checks::evm_valid(&compact),
                        IdType::Sol => checks::sol_valid(&compact),
                        IdType::Tron => checks::tron_valid(&compact),
                        other => {
                            fail!(self, "{what}: {other:?} has no checksum");
                            false
                        }
                    };
                    if passes {
                        fail!(
                            self,
                            "{what}: {raw:?} passes its checksum but is listed as bad"
                        );
                    }
                }
                Negative::CardLike {
                    raw,
                    last4,
                    document,
                    location,
                    ..
                } => {
                    if let Some(doc) = self.document(document) {
                        self.location(&what, doc, location, raw, pages);
                    } else {
                        fail!(self, "{what}: unknown document {document}");
                    }
                    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
                    if !checks::luhn_valid(&digits) {
                        fail!(self, "{what}: {raw:?} does not pass Luhn");
                    }
                    if !digits.ends_with(last4.as_str()) || last4.len() != 4 {
                        fail!(self, "{what}: last4 {last4} does not match");
                    }
                }
                Negative::StopListDomain {
                    domain,
                    document,
                    location,
                    ..
                } => {
                    if let Some(doc) = self.document(document) {
                        self.location(&what, doc, location, domain, pages);
                    } else {
                        fail!(self, "{what}: unknown document {document}");
                    }
                }
                Negative::DistinctEntities { a, b, .. } => {
                    if a == b || self.entity(a).is_none() || self.entity(b).is_none() {
                        fail!(self, "{what}: needs two different known entities");
                    }
                }
            }
        }
    }

    fn links(&mut self) {
        let gt = self.gt;
        // Expected links, recomputed from the mentions (INV-LK-01).
        let mut by_subject: BTreeMap<LinkSubject, (BTreeSet<&str>, BTreeSet<&str>)> =
            BTreeMap::new();
        for m in &gt.mentions {
            let s = match &m.subject {
                Subject::Identifier { key } => LinkSubject::Identifier { key: key.clone() },
                Subject::Entity { entity, .. } => LinkSubject::Entity {
                    entity: entity.clone(),
                },
            };
            let e = by_subject.entry(s).or_default();
            e.0.insert(&m.document);
            e.1.insert(&m.id);
        }
        let mut listed = BTreeSet::new();
        for l in &gt.links {
            let what = format!("link {:?}", l.subject);
            if !listed.insert(l.subject.clone()) {
                fail!(self, "{what}: duplicate");
            }
            let distinct: BTreeSet<&str> = l.documents.iter().map(String::as_str).collect();
            if distinct.len() < 2 {
                fail!(self, "{what}: fewer than 2 documents (INV-LK-02)");
            }
            match by_subject.get(&l.subject) {
                None => fail!(self, "{what}: no mentions"),
                Some((docs, mentions)) => {
                    if *docs != distinct {
                        fail!(
                            self,
                            "{what}: documents differ from the mentions' documents"
                        );
                    }
                    let lm: BTreeSet<&str> = l.mentions.iter().map(String::as_str).collect();
                    if *mentions != lm {
                        fail!(
                            self,
                            "{what}: mention list differs from the subject's mentions"
                        );
                    }
                }
            }
        }
        for (s, (docs, _)) in &by_subject {
            if docs.len() >= 2 && !listed.contains(s) {
                fail!(
                    self,
                    "subject {s:?} is in {} documents but has no link",
                    docs.len()
                );
            }
        }
    }

    fn merges(&mut self) {
        for mg in &self.gt.merges {
            if self.entity(&mg.entity).is_none() {
                fail!(self, "merge {}: unknown entity", mg.entity);
            }
            for v in &mg.variants {
                let seen = self.gt.mentions.iter().any(|m| {
                    m.raw == *v && matches!(&m.subject, Subject::Entity { entity, .. } if *entity == mg.entity)
                });
                if !seen {
                    fail!(self, "merge {}: variant {v:?} has no mention", mg.entity);
                }
            }
        }
    }

    fn relations_and_events(&mut self) {
        let gt = self.gt;
        let by_id: HashMap<&str, &Mention> =
            gt.mentions.iter().map(|m| (m.id.as_str(), m)).collect();
        let entity_of = |id: &str| match by_id.get(id).map(|m| &m.subject) {
            Some(Subject::Entity { entity, .. }) => Some(entity.clone()),
            _ => None,
        };
        for r in &gt.relations {
            let what = format!("relation {}", r.id);
            if self.document(&r.document).is_none() {
                fail!(self, "{what}: unknown document");
            }
            if r.evidence.len() != 2 {
                fail!(
                    self,
                    "{what}: evidence must be [subject mention, object mention]"
                );
                continue;
            }
            if entity_of(&r.evidence[0]).as_deref() != Some(r.subject.as_str())
                || entity_of(&r.evidence[1]).as_deref() != Some(r.object.as_str())
            {
                fail!(
                    self,
                    "{what}: evidence mentions are not the subject and object"
                );
            }
            for e in &r.evidence {
                if by_id
                    .get(e.as_str())
                    .is_some_and(|m| m.document != r.document)
                {
                    fail!(self, "{what}: evidence {e} is in another document");
                }
            }
        }
        for ev in &gt.events {
            let what = format!("event {}", ev.id);
            if !date_ok(&ev.date) {
                fail!(
                    self,
                    "{what}: date {:?} is not ISO 8601 (full or partial) or \"unknown\" (INV-AI-07)",
                    ev.date
                );
            }
            if self.document(&ev.document).is_none() {
                fail!(self, "{what}: unknown document");
            }
            if ev.quote.trim().is_empty() {
                fail!(self, "{what}: empty quote");
            }
            for e in &ev.evidence {
                match by_id.get(e.as_str()) {
                    None => fail!(self, "{what}: unknown evidence mention {e}"),
                    Some(m) if m.document != ev.document => {
                        fail!(self, "{what}: evidence {e} is in another document")
                    }
                    Some(_) => {}
                }
            }
        }
    }

    fn files_on_disk(&mut self, root: &Path) {
        for f in &self.gt.files {
            let path = root.join(&f.path);
            match std::fs::read(&path) {
                Err(e) => fail!(self, "file {}: cannot read: {e}", f.path),
                Ok(bytes) => {
                    if bytes.len() as u64 != f.bytes {
                        fail!(
                            self,
                            "file {}: {} bytes on disk, {} listed",
                            f.path,
                            bytes.len(),
                            f.bytes
                        );
                    }
                    let h = crate::sha256_hex(&bytes);
                    if h != f.sha256 {
                        fail!(
                            self,
                            "file {}: sha256 {h} on disk, {} listed",
                            f.path,
                            f.sha256
                        );
                    }
                }
            }
        }
    }
}

fn box_ok(b: &BBox, w: f64, h: f64) -> bool {
    let t = BOX_TOLERANCE;
    b.x0() < b.x1()
        && b.y0() < b.y1()
        && b.x0() >= -t
        && b.y0() >= -t
        && b.x1() <= w + t
        && b.y1() <= h + t
}

fn date_ok(d: &str) -> bool {
    if d == "unknown" {
        return true;
    }
    let parts: Vec<&str> = d.split('-').collect();
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|c| c.is_ascii_digit());
    match parts.as_slice() {
        [y] => digits(y, 4),
        [y, m] => digits(y, 4) && digits(m, 2) && (1..=12).contains(&m.parse::<u8>().unwrap_or(0)),
        [y, m, day] => {
            digits(y, 4)
                && digits(m, 2)
                && digits(day, 2)
                && (1..=12).contains(&m.parse::<u8>().unwrap_or(0))
                && (1..=31).contains(&day.parse::<u8>().unwrap_or(0))
        }
        _ => false,
    }
}

/// The key must pass its checksum, and the raw text must normalize to the key for the types
/// with a simple rule. Phone numbers, URLs and custom patterns only need a non-empty value.
fn identifier_consistent(key: &IdentifierKey, raw: &str, derived: bool) -> Result<(), String> {
    let v = key.value.as_str();
    let compact: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
    let ok = match key.id_type {
        IdType::Iban => checks::iban_valid(v) && compact.to_ascii_uppercase() == v,
        IdType::Btc => checks::btc_valid(v) && (compact == v || compact.to_ascii_lowercase() == v),
        IdType::Evm => checks::evm_valid(&compact) && compact.to_ascii_lowercase() == v,
        IdType::Sol => checks::sol_valid(v) && compact == v,
        IdType::Tron => checks::tron_valid(v) && compact == v,
        IdType::Email => raw.to_lowercase() == v && v.contains('@'),
        IdType::Domain => {
            // A derived domain shares its source's location and raw text (see `Mention::derived_from`).
            !v.is_empty()
                && if derived {
                    raw.to_lowercase().contains(v)
                } else {
                    raw.to_lowercase().trim_start_matches("www.") == v
                }
        }
        IdType::Phone => {
            v.starts_with('+') && v[1..].bytes().all(|c| c.is_ascii_digit()) && v.len() >= 8
        }
        IdType::Url => v.starts_with("http"),
        IdType::Custom => key.pattern.is_some() && !v.is_empty(),
    };
    if !ok {
        return Err(format!(
            "{:?} {:?} does not check out against raw {raw:?}",
            key.id_type, v
        ));
    }
    if key.pattern.is_some() && key.id_type != IdType::Custom {
        return Err("pattern is only for custom identifiers".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        for ok in ["2025", "2025-03", "2025-03-28", "unknown"] {
            assert!(date_ok(ok), "{ok}");
        }
        for bad in ["25-03-28", "2025-13", "2025-03-32", "March 2025", ""] {
            assert!(!date_ok(bad), "{bad}");
        }
    }
}
