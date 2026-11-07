//! Meaning search (M6b, US-06.2, ADR-0010): the derived index file, and a query from typed text
//! to results.
//!
//! - The index file is `index/vectors-<model>.bin`: one sign bit per dimension per vector, ids
//!   beside them. It is built to a temp file and renamed. A header that does not match the active
//!   run and generation, a wrong length or a wrong checksum means "rebuild" (ADR-0010).
//! - While in use it is a plain buffer (`unsafe_code` is forbidden here, so no memory map), loaded
//!   on the first query and dropped after 10 idle minutes (NFR-RES-01).
//! - A query: readiness, the query's embedding through the gate as [`Purpose::SearchQuery`]
//!   (INV-SR-03), Hamming scan, float rescore over the run's own vectors (INV-SR-02), top 20 joined
//!   to their documents and locators, card-like numbers masked (NFR-SEC-14).
//!
//! No query text and no snippet text is logged.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use mk_egress::Purpose;
use mk_kernel::Span;
use mk_models::Role;
use mk_search::SNIPPET_CHARS;
use mk_search::meaning::{
    IndexFile, IndexHeader, RESCORE_POOL, TOP_K, decode_f16, normalize, push_sign_bits_f16,
    rescore_top, scan_top_k, sign_bits,
};
use mk_store::{AiRunRow, StoreError};

use crate::ai::AiError;
use crate::egress::refusal_text;
use crate::embeddings::MeaningState;
use crate::project::{Project, ProjectError};
use crate::search::{SearchHit, SegmentHit};
use crate::workspace::Workspace;

/// How long an unused index stays in memory (ADR-0010).
pub const MEANING_IDLE: Duration = Duration::from_secs(600);

/// Rows read from `project.db` at a time while building the index.
const BUILD_PAGE: usize = 4096;
/// A build that keeps finding the vectors changed under it gives up until the next look.
const BUILD_TRIES: u32 = 3;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

struct Loaded {
    index: Arc<IndexFile>,
    last_used: Instant,
}

/// A project's meaning index while in use.
pub(crate) struct MeaningCell {
    loaded: Mutex<Option<Loaded>>,
    building: AtomicBool,
    idle: Mutex<Duration>,
    /// A thread is watching for the idle time.
    watching: AtomicBool,
}

impl Default for MeaningCell {
    fn default() -> Self {
        MeaningCell {
            loaded: Mutex::new(None),
            building: AtomicBool::new(false),
            idle: Mutex::new(MEANING_IDLE),
            watching: AtomicBool::new(false),
        }
    }
}

impl MeaningCell {
    /// Drops the index once it has been idle long enough, then ends. It holds the cell weakly, so
    /// a closed project ends it too.
    fn watch(cell: &Arc<MeaningCell>) {
        if cell.watching.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak: Weak<MeaningCell> = Arc::downgrade(cell);
        let spawned = std::thread::Builder::new()
            .name("mk-meaning-idle".into())
            .spawn(move || {
                loop {
                    let Some(cell) = weak.upgrade() else {
                        return;
                    };
                    let idle = *lock(&cell.idle);
                    let wait = (idle / 4).clamp(Duration::from_millis(5), Duration::from_secs(15));
                    {
                        let mut loaded = lock(&cell.loaded);
                        let used = loaded.as_ref().map(|l| l.last_used);
                        if used.is_none_or(|at| at.elapsed() >= idle) {
                            *loaded = None;
                            cell.watching.store(false, Ordering::SeqCst);
                            return;
                        }
                    }
                    drop(cell);
                    std::thread::sleep(wait);
                }
            });
        if spawned.is_err() {
            cell.watching.store(false, Ordering::SeqCst);
        }
    }
}

/// One meaning result (US-06.2 AC1): where it is, and how similar.
#[derive(Clone, PartialEq, Debug)]
pub struct MeaningHit {
    /// Its `hit` is the window, where the view opens.
    pub hit: SearchHit,
    /// Cosine similarity to the query.
    pub score: f32,
}

/// Where a query's time went.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MeaningTimings {
    pub embed: Duration,
    /// Loading the index from its file, on a cold query.
    pub load: Duration,
    pub scan: Duration,
    pub vectors: Duration,
    pub rescore: Duration,
    pub rows: Duration,
}

/// What a query found, or why it could not run.
#[derive(Clone, PartialEq, Debug)]
pub enum MeaningOutcome {
    Results {
        hits: Vec<MeaningHit>,
        timings: MeaningTimings,
    },
    /// Why not, as the Meaning tab says it (US-06.2 AC2).
    NotReady(String),
    /// The index is missing or behind: [`Project::prepare_meaning_index`] makes it.
    Preparing,
}

fn store_error(e: StoreError) -> AiError {
    AiError {
        code: "store".into(),
        message: e.to_string(),
        blocked: false,
    }
}

/// `index/vectors-<model>.bin`. Only letters, digits, `-`, `_` and `.` of the identity are kept; a
/// clash between two models only makes the header differ, which means a rebuild.
fn index_file_name(run: &AiRunRow) -> String {
    let name: String = run
        .identity
        .as_str()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("vectors-{name}.bin")
}

/// The header at the start of a file, if it can be read and its length fits.
fn file_header(path: &Path) -> Option<IndexHeader> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut head = vec![0u8; IndexFile::header_bytes()];
    file.read_exact(&mut head).ok()?;
    let header = IndexFile::read_header(&head).ok()?;
    (header.file_bytes() == Some(file.metadata().ok()?.len())).then_some(header)
}

impl Project {
    fn meaning_index_path(&self, run: &AiRunRow) -> PathBuf {
        self.layout().index().join(index_file_name(run))
    }

    /// How long an unused index stays in memory (default [`MEANING_IDLE`]).
    pub fn set_meaning_idle(&self, idle: Duration) {
        *lock(&self.meaning().idle) = idle;
    }

    /// Whether the index is in memory now.
    pub fn meaning_index_in_memory(&self) -> bool {
        lock(&self.meaning().loaded).is_some()
    }

    /// Drops the index from memory (the project closes).
    pub(crate) fn release_meaning_index(&self) {
        *lock(&self.meaning().loaded) = None;
    }

    /// Whether a query would find its index ready: in memory or in a file that matches the active
    /// run and generation.
    pub fn meaning_index_current(&self) -> Result<bool, ProjectError> {
        let Some(run) = self.store().active_embedding_run()? else {
            return Ok(false);
        };
        let generation = self.store().embedding_generation()?;
        if let Some(l) = lock(&self.meaning().loaded).as_ref()
            && (l.index.header.run, l.index.header.generation) == (run.id, generation)
        {
            return Ok(true);
        }
        Ok(file_header(&self.meaning_index_path(&run))
            .is_some_and(|h| (h.run, h.generation) == (run.id, generation)))
    }

    /// True while the index is being built ("Preparing meaning search…").
    pub fn meaning_index_building(&self) -> bool {
        self.meaning().building.load(Ordering::SeqCst)
    }

    /// Makes the index file current, off the writer: reads the active run's vectors page by page,
    /// writes a temp file and renames it. Returns whether the file is current afterwards. A build
    /// already going on makes this return at once with false.
    pub fn prepare_meaning_index(&self) -> Result<bool, ProjectError> {
        if self.meaning().building.swap(true, Ordering::SeqCst) {
            return Ok(false);
        }
        let result = self.build_index();
        self.meaning().building.store(false, Ordering::SeqCst);
        result
    }

    fn build_index(&self) -> Result<bool, ProjectError> {
        for _ in 0..BUILD_TRIES {
            let Some(run) = self.store().active_embedding_run()? else {
                return Ok(false);
            };
            let generation = self.store().embedding_generation()?;
            if file_header(&self.meaning_index_path(&run))
                .is_some_and(|h| (h.run, h.generation) == (run.id, generation))
            {
                return Ok(true);
            }
            // A run with no vector yet has nothing to index.
            if run.dims.is_none() {
                return Ok(false);
            }
            let Some(index) = self.read_index(&run, generation)? else {
                return Ok(false);
            };
            if self.store().embedding_generation()? != generation {
                continue;
            }
            self.write_index(&run, &index)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// The run's codes, or `None` when the project started closing meanwhile.
    fn read_index(
        &self,
        run: &AiRunRow,
        generation: u64,
    ) -> Result<Option<IndexFile>, ProjectError> {
        let dims = run.dims.unwrap_or(0);
        let mut ids = Vec::new();
        let mut codes = Vec::new();
        let mut after = 0;
        loop {
            if self.embedding_lane().is_stopped() {
                return Ok(None);
            }
            let page = self
                .store()
                .embedding_rows_page(run.id, after, BUILD_PAGE)?;
            let Some((last, _)) = page.last() else {
                break;
            };
            after = *last;
            for (id, vector) in &page {
                ids.push(*id);
                push_sign_bits_f16(vector, &mut codes);
            }
            if page.len() < BUILD_PAGE {
                break;
            }
        }
        Ok(Some(IndexFile {
            header: IndexHeader {
                dims,
                run: run.id,
                generation,
                count: ids.len() as u64,
            },
            ids,
            codes,
        }))
    }

    fn write_index(&self, run: &AiRunRow, index: &IndexFile) -> Result<(), ProjectError> {
        let dir = self.layout().index();
        let io = |path: &Path, e| ProjectError::Io(path.to_path_buf(), e);
        std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
        let path = self.meaning_index_path(run);
        let temp = path.with_extension("tmp");
        {
            let mut file = std::fs::File::create(&temp).map_err(|e| io(&temp, e))?;
            file.write_all(&index.encode()).map_err(|e| io(&temp, e))?;
            file.sync_all().map_err(|e| io(&temp, e))?;
        }
        std::fs::rename(&temp, &path).map_err(|e| io(&path, e))?;
        // Another model's index is of no more use.
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("vectors-") && e.path() != path {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
        Ok(())
    }

    /// The index for this run and generation: in memory, or loaded from a good file. `None`: it
    /// must be built. A damaged file is removed.
    fn meaning_index(
        &self,
        run: &AiRunRow,
        generation: u64,
    ) -> Result<Option<(Arc<IndexFile>, Duration)>, ProjectError> {
        let cell = self.meaning();
        {
            let mut loaded = lock(&cell.loaded);
            if let Some(l) = loaded.as_mut()
                && (l.index.header.run, l.index.header.generation) == (run.id, generation)
            {
                l.last_used = Instant::now();
                return Ok(Some((l.index.clone(), Duration::ZERO)));
            }
            *loaded = None;
        }
        let path = self.meaning_index_path(run);
        let Some(header) = file_header(&path) else {
            let _ = std::fs::remove_file(&path);
            return Ok(None);
        };
        if (header.run, header.generation) != (run.id, generation) {
            return Ok(None);
        }
        let started = Instant::now();
        let bytes = std::fs::read(&path).map_err(|e| ProjectError::Io(path.clone(), e))?;
        let index = match IndexFile::decode(&bytes) {
            Ok(i) => Arc::new(i),
            Err(_) => {
                let _ = std::fs::remove_file(&path);
                return Ok(None);
            }
        };
        drop(bytes);
        *lock(&cell.loaded) = Some(Loaded {
            index: index.clone(),
            last_used: Instant::now(),
        });
        MeaningCell::watch(cell);
        Ok(Some((index, started.elapsed())))
    }

    /// The load a query does once the status has said ready, without that look: a query can get
    /// past the look just before the vectors change, which a test can only reach this way.
    #[cfg(feature = "testkit")]
    pub fn meaning_index_loads(&self) -> Result<bool, ProjectError> {
        let Some(run) = self.store().active_embedding_run()? else {
            return Ok(false);
        };
        let generation = self.store().embedding_generation()?;
        Ok(self.meaning_index(&run, generation)?.is_some())
    }
}

impl Workspace {
    /// The prefixes the chosen embedding model wants on queries and on stored text, as its model
    /// card says. None for an endpoint: what stands behind it is not known (ASSUMPTION).
    pub(crate) fn embedding_prefixes(&self) -> (String, String) {
        let Ok(view) = self.ai_choice(Role::Embeddings) else {
            return Default::default();
        };
        let Some(crate::ai::AiChoice::Local(id)) = view.choice else {
            return Default::default();
        };
        self.models
            .embedding_of(&id)
            .map(|e| (e.query_prefix, e.document_prefix))
            .unwrap_or_default()
    }

    /// One meaning query (US-06.2 AC1): the top 20 most similar windows of the active run.
    pub fn meaning_search(
        &self,
        project: &Project,
        query: &str,
    ) -> Result<MeaningOutcome, AiError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(AiError {
                code: "empty_query".into(),
                message: "Type something to search for".into(),
                blocked: false,
            });
        }
        let project_error = |e: ProjectError| AiError {
            code: "index".into(),
            message: e.to_string(),
            blocked: false,
        };
        let view = self.meaning_view(project).map_err(project_error)?;
        match view.state {
            MeaningState::Ready => {}
            MeaningState::Preparing => return Ok(MeaningOutcome::Preparing),
            _ => {
                return Ok(MeaningOutcome::NotReady(view.reason.unwrap_or_default()));
            }
        }
        let store = project.store();
        let Some(run) = store.active_embedding_run().map_err(store_error)? else {
            return Ok(MeaningOutcome::NotReady(
                "Embeddings have not run yet".into(),
            ));
        };
        let generation = store.embedding_generation().map_err(store_error)?;
        let Some((index, load)) = project
            .meaning_index(&run, generation)
            .map_err(project_error)?
        else {
            return Ok(MeaningOutcome::Preparing);
        };
        let mut timings = MeaningTimings {
            load,
            ..MeaningTimings::default()
        };

        let t = Instant::now();
        let mut client = self.ai_client_for(project, Role::Embeddings, Purpose::SearchQuery)?;
        client.pin_to_run(&run.identity);
        let (prefix, _) = self.embedding_prefixes();
        let reply = client
            .client
            .embeddings(&[format!("{prefix}{query}")])
            .map_err(|e| AiError {
                code: e.code().into(),
                message: refusal_text(&e),
                blocked: e.is_blocked(),
            })?;
        client.succeeded();
        drop(client);
        let mut vector = reply.vectors.into_iter().next().unwrap_or_default();
        normalize(&mut vector);
        timings.embed = t.elapsed();
        let dims = index.header.dims as usize;
        if vector.len() != dims {
            return Err(AiError {
                code: "dims".into(),
                message: format!(
                    "The model sent a vector of {} dimensions, not {dims}",
                    vector.len()
                ),
                blocked: false,
            });
        }

        let t = Instant::now();
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        let rows = scan_top_k(&index.codes, &sign_bits(&vector), RESCORE_POOL, threads);
        let ids: Vec<i64> = rows.iter().map(|&r| index.ids[r as usize]).collect();
        timings.scan = t.elapsed();

        let t = Instant::now();
        let stored = store
            .load_embedding_vectors(run.id, &ids)
            .map_err(store_error)?;
        timings.vectors = t.elapsed();

        let t = Instant::now();
        let vectors: Vec<(i64, Vec<f32>)> = stored
            .into_iter()
            .filter_map(|(id, bytes)| Some((id, decode_f16(&bytes, dims)?)))
            .collect();
        let top = rescore_top(&vector, &vectors, TOP_K);
        timings.rescore = t.elapsed();

        let t = Instant::now();
        let ids: Vec<i64> = top.iter().map(|t| t.0).collect();
        let found = store.embedding_search_rows(&ids).map_err(store_error)?;
        let hits = found
            .into_iter()
            .filter_map(|(id, (start, end), row)| {
                let score = top.iter().find(|t| t.0 == id)?.1;
                let len = row.text.chars().count();
                let hit = Span::new(start, end, len).ok().map(|span| SegmentHit {
                    segment_id: row.segment_id,
                    span,
                });
                let shown = mk_search::mask_card_like(&row.text);
                Some(MeaningHit {
                    hit: SearchHit {
                        // The whole window matched, not some words: nothing is marked.
                        snippet: mk_search::snippet(&shown, &[(start, start)], SNIPPET_CHARS),
                        segment_id: row.segment_id,
                        document_id: row.document_id,
                        file_type: row.file_type,
                        path: row.path,
                        missing: row.missing,
                        locator: row.locator,
                        hit,
                    },
                    score,
                })
            })
            .collect();
        timings.rows = t.elapsed();
        Ok(MeaningOutcome::Results { hits, timings })
    }
}
