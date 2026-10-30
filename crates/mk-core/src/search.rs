//! Keyword search (M4a, US-06.1): the query parsed by `mk-search`, run on the FTS5 index in
//! `mk-store`, each result cut to a snippet with card-like numbers masked (NFR-SEC-14); and the
//! source view opened at a result (US-06.1 AC4, US-08.1).
//!
//! No query text and no snippet text is logged.

use mk_kernel::{Locator, Span};
use mk_search::{Run, SNIPPET_CHARS};
use mk_store::{SearchCursor, SearchFilter};

use crate::project::{Project, ProjectError};
use crate::view::{Original, SourceContext};

/// Results per page (NFR-PERF-30: the first 50).
pub const SEARCH_PAGE: usize = 50;

/// A place in a segment's stored text: a search hit (INV-CO-04).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SegmentHit {
    pub segment_id: i64,
    pub span: Span,
}

impl SegmentHit {
    /// A hit from outside (the UI): `start..end` in characters. The span is checked against
    /// the segment's text where it is used; `None` if it is empty or reversed.
    pub fn new(segment_id: i64, start: usize, end: usize) -> Option<SegmentHit> {
        Some(SegmentHit {
            segment_id,
            span: Span::new(start, end, end).ok()?,
        })
    }
}

/// One search result (US-06.1 AC1): the document, where in it, and a snippet with the hit.
#[derive(Clone, PartialEq, Debug)]
pub struct SearchHit {
    pub segment_id: i64,
    pub document_id: i64,
    pub file_type: String,
    /// The file's path: the first present one, else the one last seen.
    pub path: Option<String>,
    /// No path of the file is present (domain D4: shown with a "Missing" badge).
    pub missing: bool,
    /// The segment's locator.
    pub locator: Locator,
    /// Card-like numbers masked (NFR-SEC-14).
    pub snippet: Vec<Run>,
    /// The first hit in the stored text, where the view opens.
    pub hit: Option<SegmentHit>,
}

/// One page of results and the cursor of the page after it.
#[derive(Clone, PartialEq, Debug)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    /// `None` on the last page.
    pub next: Option<SearchCursor>,
}

impl Project {
    /// One page of keyword results for `query` as typed, best first (US-06.1 AC1–AC3). A query
    /// with nothing to search for is [`ProjectError::Invalid`] with a plain reason.
    pub fn search(
        &self,
        query: &str,
        filter: &SearchFilter,
        after: Option<&SearchCursor>,
        limit: usize,
    ) -> Result<SearchPage, ProjectError> {
        let query = mk_search::parse(query).map_err(|e| ProjectError::Invalid(e.to_string()))?;
        let limit = limit.max(1);
        let rows = self.store().search_page(
            &query.to_fts5(),
            |texts| query.highlight_fts5(texts),
            filter,
            after,
            limit,
        )?;
        let next = (rows.len() == limit)
            .then(|| rows.last().map(|r| r.cursor()))
            .flatten();
        let hits = rows
            .into_iter()
            .map(|r| {
                let len = r.text.chars().count();
                let hit = r.hits.first().and_then(|&(s, e)| {
                    Some(SegmentHit {
                        segment_id: r.segment_id,
                        span: Span::new(s, e, len).ok()?,
                    })
                });
                let shown = mk_search::mask_card_like(&r.text);
                SearchHit {
                    snippet: mk_search::snippet(&shown, &r.hits, SNIPPET_CHARS),
                    segment_id: r.segment_id,
                    document_id: r.document_id,
                    file_type: r.file_type,
                    path: r.path,
                    missing: r.missing,
                    locator: r.locator,
                    hit,
                }
            })
            .collect();
        Ok(SearchPage { hits, next })
    }

    /// The source around a place in a segment, marked (US-06.1 AC4): the same view as at a
    /// mention, for every locator kind. A span that does not fit the segment's text opens the
    /// segment unmarked. `None` if the segment is gone.
    pub fn segment_context(
        &self,
        segment: i64,
        span: Option<(usize, usize)>,
    ) -> Result<Option<SourceContext>, ProjectError> {
        let Some(window) = self.store().segment_window(segment, 0, 0)? else {
            return Ok(None);
        };
        let Some(seg) = window.segments.into_iter().find(|s| s.id == segment) else {
            return Ok(None);
        };
        let Some(document) = self.store().document_info(window.document_id)? else {
            return Ok(None);
        };
        let len = seg.text.chars().count();
        let span = span.and_then(|(s, e)| Span::new(s, e, len).ok());
        let hit = span.map(|span| SegmentHit {
            segment_id: segment,
            span,
        });
        let Some(body) = self.body(
            &document,
            hit.map(|h| (segment, h.span)),
            Some(&seg.locator),
        )?
        else {
            return Ok(None);
        };
        Ok(Some(SourceContext {
            occurrence: None,
            position: None,
            hit,
            original: Original::of(&document),
            parent: self.parent_link(&document)?,
            document,
            body,
        }))
    }
}
