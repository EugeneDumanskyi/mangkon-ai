//! Keyword search over the FTS5 index of segment text (US-06.1, ADR-0010, schema v5).
//!
//! The query comes in as FTS5 syntax already made safe by `mk-search`. A page is found in two
//! steps: the ids and scores of the page's segments (bm25, keyset on `(score, id)`), then, for
//! those rows only, what the list shows and where the hits are in the stored text.

use mk_kernel::Locator;
use rusqlite::{Connection, OptionalExtension, params_from_iter, types::Value};

use crate::{StoreError, codec};

/// Filters of a search (US-06.1 AC3). Empty / `None` matches all.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct SearchFilter {
    /// Content file types, such as `pdf` and `csv`.
    pub file_types: Vec<String>,
    /// One document.
    pub document: Option<i64>,
}

impl SearchFilter {
    fn is_empty(&self) -> bool {
        self.file_types.is_empty() && self.document.is_none()
    }
}

/// Where a result list continues: the last row's score and segment id.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SearchCursor {
    pub score: f64,
    pub segment_id: i64,
}

/// One search result: a segment that matches (INV-SR-01: always a segment with its locator).
#[derive(Clone, PartialEq, Debug)]
pub struct SearchRow {
    pub segment_id: i64,
    pub document_id: i64,
    pub content_id: i64,
    pub file_type: String,
    /// The first present path of the content, else the path last seen; `None` if it has none.
    pub path: Option<String>,
    /// No path of the content is present (domain D4: still searchable, with a badge).
    pub missing: bool,
    /// The segment's locator; its span covers the whole text.
    pub locator: Locator,
    /// The segment's stored text (INV-CO-04).
    pub text: String,
    /// Character spans in `text` of the words that matched, in order.
    pub hits: Vec<(usize, usize)>,
    /// bm25: lower is better.
    pub score: f64,
}

impl SearchRow {
    pub fn cursor(&self) -> SearchCursor {
        SearchCursor {
            score: self.score,
            segment_id: self.segment_id,
        }
    }
}

/// Marks around a hit in `highlight()` output: private-use characters, never in a query.
const OPEN: char = '\u{E000}';
const CLOSE: char = '\u{E001}';

/// The ids and scores of one page of matches, best first.
fn page_ids(
    conn: &Connection,
    fts: &str,
    filter: &SearchFilter,
    after: Option<&SearchCursor>,
    limit: usize,
) -> Result<Vec<(i64, f64)>, StoreError> {
    let mut args: Vec<Value> = vec![
        fts.to_string().into(),
        after.map(|a| a.score).into(),
        after.map(|a| a.segment_id).into(),
        (limit as i64).into(),
    ];
    let sql = if filter.is_empty() {
        // Ranked in the index alone: no join before the top rows are known.
        "SELECT id, score FROM (
             SELECT rowid AS id, bm25(segment_fts) AS score
             FROM segment_fts WHERE segment_fts MATCH ?1)
         WHERE ?2 IS NULL OR (score, id) > (?2, ?3)
         ORDER BY score, id LIMIT ?4"
            .to_string()
    } else {
        let types = if filter.file_types.is_empty() {
            String::new()
        } else {
            let at = args.len() + 2;
            let marks: Vec<String> = (0..filter.file_types.len())
                .map(|i| format!("?{}", at + i))
                .collect();
            format!("AND c.file_type IN ({})", marks.join(", "))
        };
        args.push(filter.document.into());
        args.extend(filter.file_types.iter().cloned().map(Value::from));
        format!(
            "SELECT h.id, h.score FROM (
                 SELECT rowid AS id, bm25(segment_fts) AS score
                 FROM segment_fts WHERE segment_fts MATCH ?1) h
             JOIN segment s ON s.id = h.id
             JOIN document d ON d.id = s.document_id
             JOIN content c ON c.id = d.content_id
             WHERE (?2 IS NULL OR (h.score, h.id) > (?2, ?3))
               AND (?5 IS NULL OR s.document_id = ?5) {types}
             ORDER BY h.score, h.id LIMIT ?4"
        )
    };
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// `highlight()` output back to the plain text and the character spans of its hits. `None` if
/// the marks do not pair up or the text does not come back (the text holds a mark character).
fn hit_spans(marked: &str, text: &str) -> Option<Vec<(usize, usize)>> {
    let mut plain = String::with_capacity(marked.len());
    let mut spans = Vec::new();
    let mut open = None;
    let mut at = 0usize;
    for c in marked.chars() {
        match c {
            OPEN if open.is_none() => open = Some(at),
            CLOSE => spans.push((open.take()?, at)),
            OPEN => return None,
            c => {
                plain.push(c);
                at += 1;
            }
        }
    }
    (open.is_none() && plain == text).then_some(spans)
}

/// One page of results, best first (US-06.1 AC1), after `after` (keyset paging).
pub(crate) fn search_page(
    conn: &Connection,
    fts: &str,
    highlight_query: impl FnOnce(&[&str]) -> String,
    filter: &SearchFilter,
    after: Option<&SearchCursor>,
    limit: usize,
) -> Result<Vec<SearchRow>, StoreError> {
    let ids = page_ids(conn, fts, filter, after, limit)?;
    let mut row = conn.prepare_cached(
        "SELECT s.document_id, d.content_id, c.file_type, s.loc_kind, s.loc_pos, s.loc_ref,
                s.text, s.ocr, r.path, f.rel_path, COALESCE(f.present, 0)
         FROM segment s
         JOIN document d ON d.id = s.document_id
         JOIN content c ON c.id = d.content_id
         LEFT JOIN file_ref f ON f.id = (
             SELECT id FROM file_ref WHERE content_id = d.content_id
             ORDER BY present DESC, CASE WHEN present = 1 THEN id END, last_seen_at DESC, id DESC
             LIMIT 1)
         LEFT JOIN source_root r ON r.id = f.root_id
         WHERE s.id = ?1",
    )?;
    let mut highlight = conn.prepare_cached(
        "SELECT highlight(segment_fts, 0, char(57344), char(57345)) FROM segment_fts
         WHERE segment_fts MATCH ?1 AND rowid = ?2",
    )?;
    let mut out = Vec::with_capacity(ids.len());
    for (id, score) in ids {
        let Some(found) = row
            .query_row([id], |r| {
                let text: String = r.get(6)?;
                let ocr: Option<Vec<u8>> = r.get(7)?;
                let path = match (
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                ) {
                    (Some(root), Some(rel)) => Some(
                        mk_corpus::fs::resolve(root.as_ref(), &rel)
                            .to_string_lossy()
                            .into_owned(),
                    ),
                    _ => None,
                };
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    (r.get::<_, String>(3)?, r.get::<_, i64>(4)?, r.get(5)?),
                    text,
                    ocr,
                    path,
                    !r.get::<_, bool>(10)?,
                ))
            })
            .optional()?
            .map(
                |(document_id, content_id, file_type, loc, text, ocr, path, missing)| {
                    let (kind, pos, loc_ref) = loc;
                    let locator = codec::segment_from(&kind, pos, loc_ref, &text, ocr.as_deref())?;
                    Ok::<_, StoreError>(SearchRow {
                        segment_id: id,
                        document_id,
                        content_id,
                        file_type,
                        path,
                        missing,
                        locator,
                        text,
                        hits: Vec::new(),
                        score,
                    })
                },
            )
            .transpose()?
        else {
            continue;
        };
        out.push(found);
    }
    let texts: Vec<&str> = out.iter().map(|r| r.text.as_str()).collect();
    let marks = highlight_query(&texts);
    for found in &mut out {
        let marked: Option<String> = highlight
            .query_row(rusqlite::params![marks, found.segment_id], |r| r.get(0))
            .optional()?;
        found.hits = marked
            .and_then(|m| hit_spans(&m, &found.text))
            .unwrap_or_default();
    }
    Ok(out)
}

/// Refills the index from `segment` (FTS5 'rebuild').
pub(crate) fn rebuild_index(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO segment_fts (segment_fts) VALUES ('rebuild')",
        [],
    )?;
    Ok(())
}

/// FTS5 'integrity-check' against the segment text (rank 1 compares the index with the
/// content table). An error means the index and the segments differ.
pub(crate) fn check_index(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(
        "INSERT INTO segment_fts (segment_fts, rank) VALUES ('integrity-check', 1)",
        [],
    )?;
    Ok(())
}

/// Index rows (for tests: equals the segment count when the index is in step).
pub(crate) fn index_rows(conn: &Connection) -> Result<u64, StoreError> {
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM segment_fts_docsize", [], |r| r.get(0))?;
    Ok(n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_spans_count_characters() {
        let text = "Zoë paid Zoë";
        let marked = "\u{E000}Zoë\u{E001} paid \u{E000}Zoë\u{E001}";
        assert_eq!(hit_spans(marked, text), Some(vec![(0, 3), (9, 12)]));
        assert_eq!(hit_spans("\u{E000}a", "a"), None);
        assert_eq!(hit_spans("b", "a"), None);
    }
}
