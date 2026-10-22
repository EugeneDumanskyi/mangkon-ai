//! Collects files, documents and placed mentions into the ground truth.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use mk_bench::keys::name_key;
use mk_bench::*;
use mk_kernel::ContentHash;

use crate::rich::{MarkSpec, Target};
use crate::story::{NegDef, Story, domain_of};

/// Plain text of one segment with the char ranges covered by marks. Used to prove that no
/// story name or identifier was printed without a mark.
pub struct Segment {
    pub text: String,
    pub marked: Vec<(usize, usize)>,
}

/// What a writer returns for one document.
#[derive(Default)]
pub struct Placed {
    pub mentions: Vec<(MarkSpec, Location)>,
    pub segments: Vec<Segment>,
    pub pages: Vec<PageText>,
}

impl Placed {
    pub fn extend(&mut self, other: Placed) {
        self.mentions.extend(other.mentions);
        self.segments.extend(other.segments);
        self.pages.extend(other.pages);
    }
}

pub struct FileMeta<'a> {
    pub kind: FileKind,
    pub purpose: &'a str,
    pub pages: Option<u32>,
    pub encoding: Option<&'a str>,
    pub scan_tier: Option<ScanTier>,
    pub dpi: Option<u32>,
}

impl<'a> FileMeta<'a> {
    pub fn new(kind: FileKind, purpose: &'a str) -> Self {
        FileMeta {
            kind,
            purpose,
            pages: None,
            encoding: None,
            scan_tier: None,
            dpi: None,
        }
    }
    pub fn pages(mut self, n: usize) -> Self {
        self.pages = Some(n as u32);
        self
    }
    pub fn encoding(mut self, e: &'a str) -> Self {
        self.encoding = Some(e);
        self
    }
    pub fn scan(mut self, tier: ScanTier, dpi: u32) -> Self {
        self.scan_tier = Some(tier);
        self.dpi = Some(dpi);
        self
    }
}

struct PendingEvent {
    id: String,
    date: String,
    description: String,
    document: String,
    tags: Vec<String>,
    quote: String,
}

pub struct Collector<'s> {
    pub story: &'s Story,
    root: PathBuf,
    files: Vec<FileEntry>,
    documents: Vec<Document>,
    mentions: Vec<Mention>,
    negatives: Vec<Negative>,
    page_text: Vec<PageText>,
    tags: BTreeMap<(String, String), String>,
    relations: Vec<(RelationType, String, String, String)>,
    events: Vec<PendingEvent>,
    segments: Vec<(String, Segment)>,
    neg_count: BTreeMap<String, u32>,
}

impl<'s> Collector<'s> {
    pub fn new(story: &'s Story, root: &Path) -> Self {
        Collector {
            story,
            root: root.to_path_buf(),
            files: Vec::new(),
            documents: Vec::new(),
            mentions: Vec::new(),
            negatives: Vec::new(),
            page_text: Vec::new(),
            tags: BTreeMap::new(),
            relations: Vec::new(),
            events: Vec::new(),
            segments: Vec::new(),
            neg_count: BTreeMap::new(),
        }
    }

    /// Writes a synthetic file under the corpus root and records it.
    pub fn write_file(&mut self, path: &str, bytes: &[u8], meta: FileMeta) {
        let full = self.root.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).expect("create dir");
        std::fs::write(&full, bytes).unwrap_or_else(|e| panic!("write {}: {e}", full.display()));
        self.files.push(FileEntry {
            path: path.to_string(),
            sha256: ContentHash::of(bytes).to_hex(),
            bytes: bytes.len() as u64,
            origin: Origin::Synthetic,
            kind: meta.kind,
            pages: meta.pages,
            encoding: meta.encoding.map(str::to_string),
            scan_tier: meta.scan_tier,
            dpi: meta.dpi,
            annotation: Annotation::Full,
            purpose: meta.purpose.to_string(),
        });
    }

    /// Records a public file (already on disk, described by the manifest).
    pub fn public_file(&mut self, entry: FileEntry) {
        self.files.push(entry);
    }

    pub fn doc(
        &mut self,
        id: &str,
        file: &str,
        part: DocPart,
        kind: FileKind,
        parent: Option<&str>,
        sha256: Option<String>,
    ) {
        self.documents.push(Document {
            id: id.to_string(),
            file: file.to_string(),
            part,
            kind,
            parent: parent.map(str::to_string),
            sha256,
        });
    }

    /// A whole-file document with the given id.
    pub fn whole(&mut self, id: &str, file: &str, kind: FileKind) {
        self.doc(id, file, DocPart::Whole, kind, None, None);
    }

    pub fn ingest(&mut self, doc: &str, placed: Placed) {
        for (spec, loc) in placed.mentions {
            self.mention(doc, &spec, loc);
        }
        for s in placed.segments {
            self.segments.push((doc.to_string(), s));
        }
        for mut p in placed.pages {
            p.document = doc.to_string();
            self.page_text.push(p);
        }
    }

    fn next_mention_id(&self) -> String {
        format!("m{:04}", self.mentions.len() + 1)
    }

    /// Records a page-level identifier mention of a public file (partial annotation).
    pub fn public_mention(&mut self, doc: &str, key: IdentifierKey, raw: &str, page: u32) {
        let id = self.next_mention_id();
        let derived = matches!(key.id_type, IdType::Email | IdType::Url).then(|| domain_of(&key));
        self.mentions.push(Mention {
            id: id.clone(),
            document: doc.to_string(),
            subject: Subject::Identifier { key },
            raw: raw.to_string(),
            location: Location::PdfPage { page },
            difficulty: Difficulty::Normal,
            derived_from: None,
            note: None,
        });
        if let Some(d) = derived {
            self.derived_domain(doc, &id, d, raw, Location::PdfPage { page });
        }
    }

    fn derived_domain(&mut self, doc: &str, src: &str, domain: String, raw: &str, loc: Location) {
        let id = self.next_mention_id();
        self.mentions.push(Mention {
            id,
            document: doc.to_string(),
            subject: Subject::Identifier {
                key: IdentifierKey {
                    id_type: IdType::Domain,
                    value: domain,
                    pattern: None,
                },
            },
            raw: raw.to_string(),
            location: loc,
            difficulty: Difficulty::Normal,
            derived_from: Some(src.to_string()),
            note: None,
        });
    }

    fn mention(&mut self, doc: &str, spec: &MarkSpec, loc: Location) {
        let difficulty = if spec.hard {
            Difficulty::Hard
        } else {
            Difficulty::Normal
        };
        match &spec.target {
            Target::Neg(nid) => {
                let n = self.neg_count.entry(nid.clone()).or_default();
                *n += 1;
                let id = format!("{nid}-{n}");
                let document = doc.to_string();
                let raw = spec.raw.clone();
                self.negatives.push(match &self.story.negs[nid.as_str()] {
                    NegDef::BadChecksum { claimed, .. } => Negative::BadChecksum {
                        id,
                        claimed_type: *claimed,
                        raw,
                        document,
                        location: loc,
                    },
                    NegDef::CardLike { .. } => {
                        let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
                        let last4 = digits[digits.len() - 4..].to_string();
                        Negative::CardLike {
                            id,
                            raw,
                            last4,
                            document,
                            location: loc,
                        }
                    }
                    NegDef::StopList { domain } => Negative::StopListDomain {
                        id,
                        domain: domain.clone(),
                        document,
                        location: loc,
                    },
                });
            }
            Target::Ident(iid) => {
                let key = self.story.ident(iid).key.clone();
                let id = self.next_mention_id();
                self.tag(doc, spec, &id);
                let derived =
                    matches!(key.id_type, IdType::Email | IdType::Url).then(|| domain_of(&key));
                self.mentions.push(Mention {
                    id: id.clone(),
                    document: doc.to_string(),
                    subject: Subject::Identifier { key },
                    raw: spec.raw.clone(),
                    location: loc.clone(),
                    difficulty,
                    derived_from: None,
                    note: spec.note.clone(),
                });
                if let Some(d) = derived {
                    self.derived_domain(doc, &id, d, &spec.raw, loc);
                }
            }
            Target::Entity(eid) => {
                let e = self.story.entity(eid);
                let id = self.next_mention_id();
                self.tag(doc, spec, &id);
                self.mentions.push(Mention {
                    id,
                    document: doc.to_string(),
                    subject: Subject::Entity {
                        entity: eid.clone(),
                        name_key: name_key(e.kind, &spec.raw),
                    },
                    raw: spec.raw.clone(),
                    location: loc,
                    difficulty,
                    derived_from: None,
                    note: spec.note.clone(),
                });
            }
        }
    }

    fn tag(&mut self, doc: &str, spec: &MarkSpec, id: &str) {
        if let Some(t) = &spec.tag {
            let prev = self
                .tags
                .insert((doc.to_string(), t.clone()), id.to_string());
            assert!(prev.is_none(), "tag #{t} used twice in {doc}");
        }
    }

    /// A Layer 3 relation between two tagged entity mentions of `doc`.
    pub fn relation(&mut self, rel: RelationType, doc: &str, subject_tag: &str, object_tag: &str) {
        self.relations.push((
            rel,
            doc.to_string(),
            subject_tag.to_string(),
            object_tag.to_string(),
        ));
    }

    /// A Layer 3 event. `quote` must be the exact text of the sentence in `doc`.
    pub fn event(
        &mut self,
        id: &str,
        date: &str,
        description: &str,
        doc: &str,
        tags: &[&str],
        quote: &str,
    ) {
        self.events.push(PendingEvent {
            id: id.to_string(),
            date: date.to_string(),
            description: description.to_string(),
            document: doc.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            quote: quote.to_string(),
        });
    }

    fn tagged(&self, doc: &str, tag: &str) -> String {
        self.tags
            .get(&(doc.to_string(), tag.to_string()))
            .unwrap_or_else(|| panic!("no tag #{tag} in {doc}"))
            .clone()
    }

    /// Finishes the ground truth. Fails if any story name or identifier was printed unmarked,
    /// or an event quote is not in its document.
    pub fn finish(
        self,
        generator: Generator,
        page_text_file: &str,
    ) -> Result<GroundTruth, Vec<String>> {
        let mut errors = self.unmarked();
        let entity_of = |mid: &str| -> String {
            match &self
                .mentions
                .iter()
                .find(|m| m.id == mid)
                .expect("mention")
                .subject
            {
                Subject::Entity { entity, .. } => entity.clone(),
                Subject::Identifier { .. } => {
                    panic!("relation evidence {mid} is not an entity mention")
                }
            }
        };
        let mut relations = Vec::new();
        for (i, (rel, doc, s, o)) in self.relations.iter().enumerate() {
            let (sm, om) = (self.tagged(doc, s), self.tagged(doc, o));
            relations.push(Relation {
                id: format!("r{:02}", i + 1),
                rel_type: *rel,
                subject: entity_of(&sm),
                object: entity_of(&om),
                document: doc.clone(),
                evidence: vec![sm, om],
            });
        }
        let mut events = Vec::new();
        for e in &self.events {
            // Wrapped lines join with `\n`; compare with whitespace collapsed.
            let squash = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
            let quote = squash(&e.quote);
            let in_doc = self
                .segments
                .iter()
                .any(|(d, s)| *d == e.document && squash(&s.text).contains(&quote));
            if !in_doc {
                errors.push(format!("event {}: quote not found in {}", e.id, e.document));
            }
            events.push(Event {
                id: e.id.clone(),
                date: e.date.clone(),
                description: e.description.clone(),
                document: e.document.clone(),
                evidence: e.tags.iter().map(|t| self.tagged(&e.document, t)).collect(),
                quote: e.quote.clone(),
            });
        }

        let mut by_subject: BTreeMap<LinkSubject, (BTreeSet<String>, Vec<String>)> =
            BTreeMap::new();
        let mut variants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for m in &self.mentions {
            let s = match &m.subject {
                Subject::Identifier { key } => LinkSubject::Identifier { key: key.clone() },
                Subject::Entity { entity, .. } => {
                    variants
                        .entry(entity.clone())
                        .or_default()
                        .insert(m.raw.clone());
                    LinkSubject::Entity {
                        entity: entity.clone(),
                    }
                }
            };
            let e = by_subject.entry(s).or_default();
            e.0.insert(m.document.clone());
            e.1.push(m.id.clone());
        }
        let links = by_subject
            .into_iter()
            .filter(|(_, (docs, _))| docs.len() >= 2)
            .map(|(subject, (docs, mentions))| Link {
                subject,
                documents: docs.into_iter().collect(),
                mentions,
            })
            .collect();
        let merges = self
            .story
            .entities
            .iter()
            .filter_map(|e| {
                let v = variants.get(e.id)?;
                (v.len() >= 2).then(|| Merge {
                    entity: e.id.to_string(),
                    variants: v.iter().cloned().collect(),
                })
            })
            .collect();

        let mut negatives = self.negatives;
        for (id, a, b, note) in &self.story.distinct {
            negatives.push(Negative::DistinctEntities {
                id: id.to_string(),
                a: a.to_string(),
                b: b.to_string(),
                note: note.to_string(),
            });
        }
        let entities = self
            .story
            .entities
            .iter()
            .map(|e| Entity {
                id: e.id.to_string(),
                kind: e.kind,
                canonical: e.canonical.to_string(),
                aliases: e.aliases.iter().map(|a| a.to_string()).collect(),
                role: e.role.to_string(),
            })
            .collect();

        if !errors.is_empty() {
            return Err(errors);
        }
        Ok(GroundTruth {
            schema_version: SCHEMA_VERSION,
            generator,
            page_text_file: page_text_file.to_string(),
            files: self.files,
            documents: self.documents,
            entities,
            mentions: self.mentions,
            negatives,
            links,
            merges,
            relations,
            events,
            page_text: self.page_text,
        })
    }

    fn unmarked(&self) -> Vec<String> {
        let forms = self.story.surface_forms();
        let mut errors = Vec::new();
        for (doc, seg) in &self.segments {
            let chars: Vec<char> = seg.text.chars().collect();
            let text: String = chars.iter().collect();
            // Byte offset → char offset.
            let mut char_at = vec![0usize; text.len() + 1];
            let mut ci = 0;
            for (bi, c) in text.char_indices() {
                for slot in &mut char_at[bi..bi + c.len_utf8()] {
                    *slot = ci;
                }
                ci += 1;
            }
            char_at[text.len()] = ci;
            for (form, owner) in &forms {
                let flen = form.chars().count();
                let first_alnum = form.chars().next().is_some_and(char::is_alphanumeric);
                let last_alnum = form.chars().last().is_some_and(char::is_alphanumeric);
                for (bi, _) in text.match_indices(form.as_str()) {
                    let s = char_at[bi];
                    let e = s + flen;
                    if first_alnum && s > 0 && chars[s - 1].is_alphanumeric() {
                        continue;
                    }
                    if last_alnum && e < chars.len() && chars[e].is_alphanumeric() {
                        continue;
                    }
                    if !seg.marked.iter().any(|&(ms, me)| ms <= s && e <= me) {
                        errors.push(format!("{doc}: unmarked {form:?} ({owner}) at char {s}"));
                    }
                }
            }
        }
        errors
    }
}
