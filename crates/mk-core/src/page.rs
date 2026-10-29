//! The page view (ADR-0011, US-08.1 AC1–AC2, US-08.3): a PDF or image page rendered by a worker,
//! cached in the project, with the hit's boxes as fractions of the page.
//!
//! The image is a picture of the stored content: the render worker checks the file's hash, and
//! a PDF highlight is placed only when the page's text, laid out again, is the text that was
//! stored (INV-CO-04). Otherwise the view falls back to the stored text with a note.

use std::fs::File;
use std::io::{self, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mk_kernel::{ContentHash, Locator, Rect};
use mk_parsers::Format;
use mk_parsers::layout::to_display;
use mk_parsers::pdf::engine::PageInfo;
use mk_store::{DocumentInfo, WindowSegment};
use mk_worker::frame::{FrameReader, FrameWriter};
use mk_worker::{PageImage, PagePoints, RenderPage};

use crate::project::{Project, ProjectError};
use crate::workers::{RenderError, Stopped, WorkerProcess};

/// Pixels per PDF point of a page view image: 144 DPI. ASSUMPTION (W3b-1 checks the time).
pub const PAGE_SCALE: f32 = 2.0;

/// The page cache holds at most this many bytes; the least recently used pages go first
/// (ADR-0011 ASSUMPTION).
pub const PAGE_CACHE_LIMIT: u64 = 500 << 20;

/// Whether the page image is shown, and if not, why the stored text is shown instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageStatus {
    /// The page image, with the hit's boxes.
    Shown,
    /// The original file is gone (US-08.5).
    OriginalMissing,
    /// The original file changed since it was read, or its page no longer matches the stored
    /// text (ADR-0011).
    Changed,
    /// The page could not be shown for another reason, given in the note.
    TextOnly,
}

/// A box on the page as fractions of the shown page (0 to 1, top-left origin), so the UI can
/// place it at any zoom.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PageBox {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl PageBox {
    fn of(r: &Rect, width: f32, height: f32) -> Self {
        PageBox {
            x: r.x0() / width,
            y: r.y0() / height,
            width: r.width() / width,
            height: r.height() / height,
        }
    }
}

/// One page as the page view shows it.
#[derive(Clone, PartialEq, Debug)]
pub struct PageView {
    pub document_id: i64,
    /// 1-based.
    pub page: u32,
    /// Pages in the file; the stored text's last page when the file could not be opened.
    pub pages: u32,
    pub status: PageStatus,
    /// The image's size in pixels, when shown.
    pub width: u32,
    pub height: u32,
    /// The hit's boxes (one per line for text, one per word for OCR), when shown.
    pub boxes: Vec<PageBox>,
    /// Why the image or the highlight is not shown.
    pub note: Option<String>,
}

/// Rendered pages under `cache/pages/<content hash>/<page>@<scale>.{png,geom}`. The `.geom`
/// file holds the rest of the worker's answer (size, page count, PDF geometry).
pub(crate) struct PageCache {
    dir: PathBuf,
    limit: u64,
}

impl PageCache {
    pub(crate) fn new(dir: PathBuf, limit: u64) -> Self {
        PageCache { dir, limit }
    }

    fn paths(&self, hash: &ContentHash, page: u32, scale: f32) -> (PathBuf, PathBuf) {
        let dir = self.dir.join(hash.to_hex());
        (
            dir.join(format!("{page}@{scale}.png")),
            dir.join(format!("{page}@{scale}.geom")),
        )
    }

    /// The page without its PNG, if cached. Marks it used.
    fn meta(&self, hash: &ContentHash, page: u32, scale: f32) -> Option<PageImage> {
        let (png, geom) = self.paths(hash, page, scale);
        if !png.is_file() {
            return None;
        }
        let meta: PageImage = FrameReader::new(BufReader::new(File::open(&geom).ok()?))
            .recv()
            .ok()??;
        if let Ok(f) = File::options().write(true).open(&png) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some(meta)
    }

    fn png(&self, hash: &ContentHash, page: u32, scale: f32) -> Option<Vec<u8>> {
        std::fs::read(self.paths(hash, page, scale).0).ok()
    }

    /// Stores a page, then trims the cache to its limit. Each file is written next to its
    /// name and renamed into place; the PNG goes last, since it marks the entry present.
    fn put(&self, hash: &ContentHash, page: u32, scale: f32, image: &PageImage) -> io::Result<()> {
        let (png, geom) = self.paths(hash, page, scale);
        if let Some(dir) = png.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let meta = PageImage {
            png: Vec::new(),
            ..image.clone()
        };
        let partial = geom.with_extension("geom.partial");
        let mut w = FrameWriter::new(BufWriter::new(File::create(&partial)?));
        w.send(&meta)?;
        w.into_inner()
            .into_inner()
            .map_err(|e| e.into_error())?
            .flush()?;
        std::fs::rename(&partial, &geom)?;
        let partial = png.with_extension("png.partial");
        std::fs::write(&partial, &image.png)?;
        std::fs::rename(&partial, &png)?;
        self.trim()
    }

    /// Removes the least recently used pages until the cache is under its limit.
    fn trim(&self) -> io::Result<()> {
        let mut pages: Vec<(SystemTime, u64, PathBuf)> = Vec::new();
        let mut total = 0u64;
        for dir in std::fs::read_dir(&self.dir)?.flatten() {
            for f in std::fs::read_dir(dir.path())?.flatten() {
                let Ok(m) = f.metadata() else { continue };
                total += m.len();
                if f.path().extension().is_some_and(|e| e == "png") {
                    let geom = f.path().with_extension("geom");
                    let size = m.len() + std::fs::metadata(&geom).map_or(0, |g| g.len());
                    let used = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    pages.push((used, size, f.path()));
                }
            }
        }
        pages.sort_by_key(|p| p.0);
        for (_, size, png) in pages {
            if total <= self.limit {
                break;
            }
            let _ = std::fs::remove_file(&png);
            let _ = std::fs::remove_file(png.with_extension("geom"));
            total = total.saturating_sub(size);
        }
        Ok(())
    }
}

/// A file that can be rendered: a PDF or an image of its own, not an attachment.
fn renderable(document: &DocumentInfo) -> bool {
    document.parent.is_none()
        && Format::from_name(&document.file_type).is_some_and(|f| f.is_paged())
}

/// The hit's boxes on a rendered page, or why they cannot be placed.
fn boxes(
    image: &PageImage,
    segment: &WindowSegment,
    locator: &Locator,
) -> Result<Vec<PageBox>, String> {
    match locator {
        Locator::OcrText {
            boxes, image: size, ..
        } => {
            let (w, h) = (size.width as f32, size.height as f32);
            Ok(boxes.iter().map(|r| PageBox::of(r, w, h)).collect())
        }
        Locator::PdfText { span, .. } => {
            let (
                Some(geometry),
                Some(PagePoints {
                    width,
                    height,
                    rotate,
                }),
            ) = (&image.geometry, image.points)
            else {
                return Err("This page has no text layer to place the highlight on".into());
            };
            let info = PageInfo {
                width,
                height,
                rotate,
            };
            let (w, h) = info.display_size();
            let rects = geometry
                .highlight(&segment.text, *span)
                .map_err(|e| format!("The highlight could not be placed: {e}"))?;
            Ok(rects
                .iter()
                .map(|r| PageBox::of(&to_display(r, &info), w, h))
                .collect())
        }
        _ => Ok(Vec::new()),
    }
}

impl Project {
    /// The rendered page `page` of a PDF or image document, through the page cache; `None` if
    /// the document is gone or has no pages (US-08.1 AC1–AC2). With `mention`, the mention's
    /// boxes are placed when it is on this page.
    pub fn page_view(
        &self,
        document: i64,
        page: u32,
        mention: Option<i64>,
    ) -> Result<Option<PageView>, ProjectError> {
        let Some(doc) = self.store().document_info(document)? else {
            return Ok(None);
        };
        if !Format::from_name(&doc.kind).is_some_and(|f| f.is_paged()) || page == 0 {
            return Ok(None);
        }
        let stored_pages = self.store().last_text_page(document)?;
        let text_only = |status, note: &str| PageView {
            document_id: document,
            page,
            pages: stored_pages.max(page),
            status,
            width: 0,
            height: 0,
            boxes: Vec::new(),
            note: Some(note.to_string()),
        };
        if !renderable(&doc) {
            return Ok(Some(text_only(
                PageStatus::TextOnly,
                "Pages of attachments are shown as text",
            )));
        }
        let Some(path) = doc.path.clone() else {
            return Ok(Some(text_only(
                PageStatus::OriginalMissing,
                "Original file missing: showing the stored text",
            )));
        };
        let image = match self.rendered(&doc, &path, page)? {
            Ok(image) => image,
            Err(f) => {
                let status = match f.code.as_str() {
                    "file_missing" => PageStatus::OriginalMissing,
                    "file_changed" => PageStatus::Changed,
                    _ => PageStatus::TextOnly,
                };
                let note = match status {
                    PageStatus::OriginalMissing => {
                        "Original file missing: showing the stored text".to_string()
                    }
                    PageStatus::Changed => {
                        "The file changed since it was read: showing the stored text".to_string()
                    }
                    _ => format!("{}: showing the stored text", f.reason),
                };
                return Ok(Some(text_only(status, &note)));
            }
        };
        let mut view = PageView {
            document_id: document,
            page,
            pages: image.pages,
            status: PageStatus::Shown,
            width: image.width,
            height: image.height,
            boxes: Vec::new(),
            note: None,
        };
        let hit = match mention {
            Some(m) => self.store().mention(m)?.filter(|o| {
                o.document_id == document && o.locator.page().is_some_and(|p| p.get() == page)
            }),
            None => None,
        };
        if let Some(hit) = hit
            && let Some(segment) = self.store().page_segment(document, page)?
        {
            match boxes(&image, &segment, &hit.locator) {
                Ok(b) => view.boxes = b,
                Err(note) => {
                    view.status = PageStatus::Changed;
                    view.note = Some(format!("{note}: showing the stored text"));
                }
            }
        }
        Ok(Some(view))
    }

    /// The PNG of a page [`Project::page_view`] has shown, from the page cache (rendered again
    /// if it was trimmed). `None` if it cannot be shown.
    pub fn page_image(&self, document: i64, page: u32) -> Result<Option<Vec<u8>>, ProjectError> {
        let Some(doc) = self.store().document_info(document)? else {
            return Ok(None);
        };
        if !renderable(&doc) {
            return Ok(None);
        }
        if let Some(png) = self.page_cache().png(&doc.hash, page, PAGE_SCALE) {
            return Ok(Some(png));
        }
        let Some(path) = doc.path.clone() else {
            return Ok(None);
        };
        Ok(self
            .rendered(&doc, &path, page)?
            .ok()
            .and_then(|_| self.page_cache().png(&doc.hash, page, PAGE_SCALE)))
    }

    /// The page from the cache, or rendered by the render worker and cached. The image comes
    /// back without its PNG (read it with [`PageCache::png`]).
    fn rendered(
        &self,
        doc: &DocumentInfo,
        path: &str,
        page: u32,
    ) -> Result<Result<PageImage, crate::stage_a::Failure>, ProjectError> {
        let cache = self.page_cache();
        if let Some(meta) = cache.meta(&doc.hash, page, PAGE_SCALE) {
            return Ok(Ok(meta));
        }
        let req = RenderPage {
            path: path.to_string(),
            content_hash: doc.hash,
            file_type: doc.file_type.clone(),
            page,
            scale: PAGE_SCALE,
        };
        let cfg = self.worker_config();
        let mut slot = self.renderer().lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(WorkerProcess::spawn(cfg)?);
        }
        let worker = slot.as_mut().expect("started above");
        match worker.render(cfg, req) {
            Ok(image) => {
                drop(slot);
                if let Err(e) = cache.put(&doc.hash, page, PAGE_SCALE, &image) {
                    return Ok(Err(crate::stage_a::Failure::new(
                        "internal_error",
                        format!("The page could not be cached: {e}"),
                    )));
                }
                Ok(Ok(PageImage {
                    png: Vec::new(),
                    ..image
                }))
            }
            Err(RenderError::Failed(f)) => Ok(Err(f)),
            Err(RenderError::Stopped(why)) => {
                *slot = None;
                let reason = match why {
                    Stopped::Deadline | Stopped::Silent => "The page took too long to show",
                    Stopped::Memory => "The page needed too much memory to show",
                    Stopped::Crashed => "The page could not be shown: the reader stopped",
                };
                Ok(Err(crate::stage_a::Failure::new("render_failed", reason)))
            }
        }
    }

    fn page_cache(&self) -> PageCache {
        PageCache::new(page_cache_dir(&self.layout().cache()), PAGE_CACHE_LIMIT)
    }
}

/// `cache/pages/` of a project cache.
pub fn page_cache_dir(cache: &Path) -> PathBuf {
    cache.join("pages")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(png: &[u8]) -> PageImage {
        PageImage {
            png: png.to_vec(),
            width: 10,
            height: 20,
            pages: 3,
            points: None,
            geometry: None,
        }
    }

    /// ADR-0011: the cache gives back what was put, and drops the least recently used page
    /// when it is over its limit.
    #[test]
    fn page_cache_round_trip_and_lru() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PageCache::new(dir.path().join("pages"), 2_500);
        let h = ContentHash::of(b"x");
        cache.put(&h, 1, 2.0, &image(&[1; 1000])).unwrap();
        assert_eq!(cache.meta(&h, 1, 2.0), Some(image(&[])));
        assert_eq!(cache.png(&h, 1, 2.0), Some(vec![1; 1000]));
        // Page 1 is older than page 2; page 1 is then used, so page 2 goes first.
        let old = SystemTime::now() - std::time::Duration::from_secs(60);
        let (png1, _) = cache.paths(&h, 1, 2.0);
        File::options()
            .write(true)
            .open(&png1)
            .unwrap()
            .set_modified(old)
            .unwrap();
        cache.put(&h, 2, 2.0, &image(&[2; 1000])).unwrap();
        let (png2, _) = cache.paths(&h, 2, 2.0);
        File::options()
            .write(true)
            .open(&png2)
            .unwrap()
            .set_modified(old - std::time::Duration::from_secs(60))
            .unwrap();
        assert!(cache.meta(&h, 1, 2.0).is_some());
        cache.put(&h, 3, 2.0, &image(&[3; 1000])).unwrap();
        assert!(cache.png(&h, 2, 2.0).is_none(), "least recently used goes");
        assert!(cache.png(&h, 1, 2.0).is_some());
        assert!(cache.png(&h, 3, 2.0).is_some());
    }

    #[test]
    fn page_box_is_a_fraction() {
        let r = Rect::new(10.0, 20.0, 30.0, 60.0).unwrap();
        assert_eq!(
            PageBox::of(&r, 100.0, 200.0),
            PageBox {
                x: 0.1,
                y: 0.1,
                width: 0.2,
                height: 0.2
            }
        );
    }
}
