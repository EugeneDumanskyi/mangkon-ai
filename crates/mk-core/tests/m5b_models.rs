//! M5b criteria 1 and 2 (E09) on [`Workspace`]: the hardware verdict and what it allows
//! (US-09.1, US-09.2), the catalog view with Recommended (US-09.3) and downloads with the disk
//! check, pause, resume, cancel, checksum and delete (US-09.4, NFR-RES-05, NFR-REL-08). Fakes
//! only: a fixed hardware profile, a fake Hugging Face, a resolver that knows only test hosts.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mk_core::{
    ConnectionQuery, DownloadState, EgressConfig, Gpu, GpuVendor, HardwareProbe, HardwareProfile,
    ModelRole, ModelsConfig, SearchFilter, WorkerConfig, Workspace,
};
use mk_models::download::{file_path, part_path};
use mk_models::hardware::{OVERRIDE_WARNING, TOO_WEAK};
use mk_models::state::CORRUPTED;
use mk_models::testkit::{
    CDN_HOST, FakeHf, NamedResolver, ORIGIN_HOST, test_bytes, test_catalog, test_model,
};
use mk_models::{Catalog, GB, Model, Tier};

const MIB: usize = 1024 * 1024;

/// A machine whose free disk space the test sets, and that counts how often it is read.
struct TestProbe {
    profile: Mutex<HardwareProfile>,
    free: AtomicU64,
    probes: AtomicUsize,
}

impl HardwareProbe for TestProbe {
    fn probe(&self, _models_dir: &Path) -> HardwareProfile {
        self.probes.fetch_add(1, Ordering::SeqCst);
        let mut p = self.profile.lock().unwrap().clone();
        p.free_disk_bytes = Some(self.free.load(Ordering::SeqCst));
        p
    }

    fn free_disk(&self, _models_dir: &Path) -> Option<u64> {
        Some(self.free.load(Ordering::SeqCst))
    }
}

fn profile(vram: u64, ram: u64) -> HardwareProfile {
    HardwareProfile {
        os_family: "linux".into(),
        os: "Linux test".into(),
        cpu: "Test CPU (8 threads)".into(),
        ram_bytes: ram,
        gpus: vec![Gpu {
            vendor: GpuVendor::Nvidia,
            name: "NVIDIA Test".into(),
            vram_bytes: Some(vram),
            driver: Some("580.10".into()),
        }],
        unified_bytes: None,
        apple_silicon: false,
        free_disk_bytes: None,
        disk_path: "/models".into(),
    }
}

fn strong() -> HardwareProfile {
    profile(8_550_000_000, 64 * GB)
}

fn weak() -> HardwareProfile {
    profile(6 * GB, 8 * GB)
}

struct Setup {
    tmp: tempfile::TempDir,
    probe: Arc<TestProbe>,
    server: FakeHf,
    resolver: Arc<NamedResolver>,
    catalog: Catalog,
    emb: Model,
    ext: Model,
    bodies: Vec<Vec<u8>>,
}

fn setup(machine: HardwareProfile) -> Setup {
    let emb_body = test_bytes(3 * MIB, 1);
    let ext_body = test_bytes(2 * MIB, 2);
    let emb = test_model(
        "emb",
        ModelRole::Embeddings,
        std::slice::from_ref(&emb_body),
    );
    let ext = test_model(
        "ext",
        ModelRole::Extraction,
        std::slice::from_ref(&ext_body),
    );
    let server = FakeHf::serving(&[
        (&emb, std::slice::from_ref(&emb_body)),
        (&ext, std::slice::from_ref(&ext_body)),
    ]);
    server.faults(|f| f.redirect_to = Some(CDN_HOST.into()));
    Setup {
        tmp: tempfile::tempdir().unwrap(),
        probe: Arc::new(TestProbe {
            profile: Mutex::new(machine),
            free: AtomicU64::new(100 * GB),
            probes: AtomicUsize::new(0),
        }),
        resolver: NamedResolver::new(&[ORIGIN_HOST, CDN_HOST]),
        catalog: test_catalog(vec![emb.clone(), ext.clone()]),
        server,
        emb,
        ext,
        bodies: vec![emb_body, ext_body],
    }
}

impl Setup {
    fn app_dir(&self) -> PathBuf {
        self.tmp.path().join("app")
    }

    fn models_dir(&self) -> PathBuf {
        self.tmp.path().join("models")
    }

    fn open(&self) -> Workspace {
        self.open_catalog(self.catalog.clone())
    }

    fn open_catalog(&self, catalog: Catalog) -> Workspace {
        Workspace::open_with_models(
            &self.app_dir(),
            EgressConfig::default(),
            ModelsConfig {
                models_dir: Some(self.models_dir()),
                catalog,
                probe: self.probe.clone(),
                download: self.server.config(self.resolver.clone()),
            },
        )
        .unwrap()
    }
}

fn state(ws: &Workspace, id: &str) -> DownloadState {
    view(ws, id).state
}

fn view(ws: &Workspace, id: &str) -> mk_core::ModelView {
    ws.model_catalog()
        .unwrap()
        .into_iter()
        .find(|m| m.id == id)
        .unwrap()
}

fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(20);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn len(p: &Path) -> Option<u64> {
    std::fs::metadata(p).ok().map(|m| m.len())
}

// ---- US-09.1, US-09.2 ----

#[test]
fn us_09_1_verdict_on_the_workspace_is_cached_until_recheck() {
    let s = setup(strong());
    let ws = s.open();
    let h = ws.hardware().unwrap();
    assert!(h.verdict.meets, "{:?}", h.verdict);
    assert!(h.local_allowed);
    assert_eq!(h.too_weak, None);
    ws.hardware().unwrap();
    ws.model_catalog().unwrap();
    assert_eq!(
        s.probe.probes.load(Ordering::SeqCst),
        1,
        "one probe per session"
    );
    *s.probe.profile.lock().unwrap() = weak();
    assert!(ws.hardware().unwrap().verdict.meets, "cached");
    let h = ws.recheck_hardware().unwrap();
    assert!(!h.verdict.meets);
    assert_eq!(s.probe.probes.load(Ordering::SeqCst), 2);
}

#[test]
fn us_09_2_ac1_no_local_download_below_minimum() {
    let s = setup(weak());
    let ws = s.open();
    let h = ws.hardware().unwrap();
    assert_eq!(h.verdict.label, "Below minimum");
    assert!(!h.local_allowed);
    assert_eq!(h.too_weak, Some(TOO_WEAK));
    assert_eq!(
        TOO_WEAK,
        "This machine is too weak for local AI. Connect an external model endpoint."
    );
    let e = ws.start_download("emb").unwrap_err();
    assert_eq!(e.code, "below_minimum");
    assert_eq!(e.message, TOO_WEAK);
    assert_eq!(state(&ws, "emb"), DownloadState::Available);
    assert!(s.server.seen().is_empty(), "nothing was requested");
    assert!(s.resolver.asked().is_empty(), "no host was even looked up");
    assert!(!s.models_dir().exists());
}

#[test]
fn us_09_2_ac3_override_enables() {
    let s = setup(weak());
    let ws = s.open();
    let h = ws.set_local_override(true).unwrap();
    assert!(h.local_override && h.local_allowed);
    assert_eq!(h.verdict.label, "Below minimum", "the verdict stays honest");
    assert_eq!(h.override_warning, OVERRIDE_WARNING);
    assert_eq!(OVERRIDE_WARNING, "Not supported. It may be very slow.");
    // The minimum tier's models are recommended once local models are allowed.
    assert!(view(&ws, "emb").recommended);
    ws.start_download("emb").unwrap();
    ws.wait_download();
    assert_eq!(state(&ws, "emb"), DownloadState::Installed);
    drop(ws);
    // The override is kept in app.db.
    let ws = s.open();
    assert!(ws.hardware().unwrap().local_override);
    let h = ws.set_local_override(false).unwrap();
    assert!(!h.local_allowed);
}

/// US-09.2 AC2: below the minimum, Layer 1 works fully: identifiers, CSV mapping, keyword
/// search, connections and the source view do not need a model.
#[test]
fn us_09_2_ac2_layer_1_works_below_minimum() {
    let s = setup(weak());
    let ws = s.open();
    assert!(!ws.hardware().unwrap().local_allowed);
    let src = s.tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(
        src.join("a.txt"),
        "Invoice ACME-1 paid to DE89370400440532013000 by ops@example.org\n",
    )
    .unwrap();
    std::fs::write(src.join("b.csv"), "name,iban\nZoe,DE89370400440532013000\n").unwrap();
    let (mut project, _) = ws.create("Case", s.tmp.path()).unwrap();
    *project.worker_config_mut() = WorkerConfig::with_program(env!("CARGO_BIN_EXE_mk-test-worker"));
    project.add_source_root(&src).unwrap();
    project.ingest().unwrap();
    let hits = project
        .search("invoice", &SearchFilter::default(), None, 50)
        .unwrap()
        .hits;
    assert_eq!(hits.len(), 1);
    let page = project
        .connections_page(&ConnectionQuery::default(), None, 50)
        .unwrap();
    assert!(
        page.rows
            .iter()
            .any(|r| r.row.subject_key.contains("DE89370400440532013000")),
        "the IBAN shared by both files is a connection"
    );
    let ctx = project.segment_context(hits[0].segment_id, None).unwrap();
    assert!(ctx.is_some(), "the source view opens at the hit");
    assert!(s.server.seen().is_empty());
}

// ---- US-09.3 ----

#[test]
fn us_09_3_ac1_recommended_per_tier() {
    let s = setup(strong());
    let big_emb = test_model("big-emb", ModelRole::Embeddings, &[test_bytes(1000, 9)]);
    let mut catalog = test_catalog(vec![s.emb.clone(), s.ext.clone(), big_emb.clone()]);
    catalog.tiers.push(Tier {
        id: "large-ram".into(),
        name: "Large RAM".into(),
        min_vram_bytes: 8 * GB,
        min_ram_bytes: 48 * GB,
        min_unified_bytes: Some(64 * GB),
        recommended_extraction: Some("ext".into()),
        recommended_embeddings: "big-emb".into(),
    });
    // 64 GB RAM: the large tier.
    let ws = s.open_catalog(catalog.clone());
    let rec: Vec<String> = ws
        .model_catalog()
        .unwrap()
        .into_iter()
        .filter(|m| m.recommended)
        .map(|m| m.id)
        .collect();
    assert_eq!(rec, ["ext", "big-emb"]);
    drop(ws);
    // 16 GB RAM: the minimum tier.
    *s.probe.profile.lock().unwrap() = profile(8 * GB, 16 * GB);
    let ws = s.open_catalog(catalog);
    let rec: Vec<String> = ws
        .model_catalog()
        .unwrap()
        .into_iter()
        .filter(|m| m.recommended)
        .map(|m| m.id)
        .collect();
    assert_eq!(rec, ["emb", "ext"]);
    // License before Download (US-09.3 AC3, NFR-LEG-03).
    let v = view(&ws, "emb");
    assert_eq!(v.license_spdx, "Apache-2.0");
    assert!(v.license_url.starts_with("https://"));
    assert_eq!(v.bytes, 3 * MIB as u64);
}

#[test]
fn us_09_3_ac2_default_workspace_uses_the_shipped_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = Workspace::open(tmp.path()).unwrap();
    assert_eq!(ws.catalog(), &Catalog::shipped());
    assert_eq!(ws.models_dir(), tmp.path().join("models"));
}

// ---- US-09.4 ----

#[test]
fn us_09_4_ac1_disk_check_blocks() {
    let s = setup(strong());
    let ws = s.open();
    // Less than the download + 2 GB (INV-MM-02).
    s.probe.free.store(GB, Ordering::SeqCst);
    let e = ws.start_download("emb").unwrap_err();
    assert_eq!(e.code, "disk_full");
    assert_eq!(
        e.message,
        "Not enough free disk space: this download needs 2 GB free, 1 GB is free."
    );
    assert_eq!(state(&ws, "emb"), DownloadState::Available);
    assert!(s.server.seen().is_empty());
    // The row says so after the free space was read.
    let need = view(&ws, "emb").disk_needed.expect("won't fit");
    assert_eq!(need, 2 * GB + 3 * MIB as u64);
}

/// NFR-RES-05: the free space is read again at the start, not taken from the cached check.
/// Below the 30 GB of US-09.1 AC2 a download that fits still starts, without the override
/// (decided 2026-11-04): the 30 GB is a note in the hardware check.
#[test]
fn nfr_res_05_disk_check_before_download() {
    let s = setup(strong());
    let ws = s.open();
    assert!(ws.hardware().unwrap().verdict.meets);
    s.probe.free.store(GB, Ordering::SeqCst);
    let e = ws.start_download("emb").unwrap_err();
    assert_eq!(e.code, "disk_full");
    assert!(e.message.contains("1 GB is free"), "{}", e.message);
    s.probe.free.store(3 * GB, Ordering::SeqCst);
    let h = ws.recheck_hardware().unwrap();
    assert!(h.verdict.meets && h.local_allowed, "{h:?}");
    assert!(
        h.verdict
            .notes
            .iter()
            .any(|n| n.starts_with("Free disk 3 GB")),
        "{h:?}"
    );
    assert_eq!(view(&ws, "emb").disk_needed, None);
    ws.start_download("emb").unwrap();
    ws.wait_download();
    assert_eq!(state(&ws, "emb"), DownloadState::Installed);
    assert!(!ws.hardware().unwrap().local_override);
}

#[test]
fn us_09_4_ac2_progress_pause_cancel() {
    let s = setup(strong());
    s.server
        .faults(|f| f.throttle = Some(Duration::from_millis(20)));
    let ws = s.open();
    let notices = Arc::new(AtomicUsize::new(0));
    let n = notices.clone();
    ws.on_models_changed(move || {
        n.fetch_add(1, Ordering::SeqCst);
    });
    ws.start_download("emb").unwrap();
    assert_eq!(ws.downloading().as_deref(), Some("emb"));
    // Only one at a time.
    assert_eq!(ws.start_download("ext").unwrap_err().code, "busy");
    wait_until("progress", || {
        let v = view(&ws, "emb");
        v.bytes_done > 256 * 1024 && v.speed_bps > 0
    });
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Downloading);
    assert!(v.eta_s.is_some());
    ws.pause_download("emb").unwrap();
    ws.wait_download();
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Paused);
    let part = part_path(&s.models_dir(), &s.emb, &s.emb.files[0]);
    let kept = len(&part).unwrap();
    assert!(
        kept > 0 && kept < s.emb.bytes(),
        "partial file kept: {kept}"
    );
    assert_eq!(v.bytes_done, kept);
    assert!(
        notices.load(Ordering::SeqCst) >= 2,
        "start and pause notified"
    );

    // Cancel while paused deletes the partial files.
    ws.cancel_download("emb").unwrap();
    assert_eq!(state(&ws, "emb"), DownloadState::Available);
    assert!(!part.exists());

    // Cancel while running.
    ws.start_download("emb").unwrap();
    wait_until("bytes", || view(&ws, "emb").bytes_done > 0);
    ws.cancel_download("emb").unwrap();
    ws.wait_download();
    assert_eq!(state(&ws, "emb"), DownloadState::Available);
    assert!(!s.models_dir().join("emb").exists());
    assert!(ws.pause_download("emb").is_err());
}

#[test]
fn us_09_4_ac3_resume_after_drop() {
    let s = setup(strong());
    s.server.faults(|f| f.drop_after = Some(MIB as u64));
    let ws = s.open();
    ws.start_download("emb").unwrap();
    ws.wait_download();
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Interrupted);
    let e = v.error.unwrap();
    assert_eq!(e.code, "network");
    let part = part_path(&s.models_dir(), &s.emb, &s.emb.files[0]);
    let kept = len(&part).unwrap();
    assert!(kept >= MIB as u64 / 2, "kept {kept}");
    ws.resume_download("emb").unwrap();
    ws.wait_download();
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Installed);
    assert_eq!(v.error, None);
    let fin = file_path(&s.models_dir(), &s.emb, &s.emb.files[0]);
    assert_eq!(std::fs::read(fin).unwrap(), s.bodies[0]);
    // The resume asked for the rest only.
    let ranges: Vec<_> = s
        .server
        .seen()
        .into_iter()
        .filter_map(|x| x.range)
        .collect();
    assert!(
        ranges.iter().any(|r| r == &format!("bytes={kept}-")),
        "{ranges:?}"
    );
}

/// Domain §7.7 "app quit": a download running when the app closes is Interrupted after the
/// restart, its partial file kept, and it resumes from there.
#[test]
fn us_09_4_ac3_resume_after_restart() {
    let s = setup(strong());
    s.server
        .faults(|f| f.throttle = Some(Duration::from_millis(20)));
    let ws = s.open();
    ws.start_download("emb").unwrap();
    wait_until("bytes", || view(&ws, "emb").bytes_done > 256 * 1024);
    drop(ws);
    let part = part_path(&s.models_dir(), &s.emb, &s.emb.files[0]);
    let kept = len(&part).unwrap();
    assert!(kept > 0);
    s.server.faults(|f| f.throttle = None);
    let ws = s.open();
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Interrupted);
    assert_eq!(v.bytes_done, kept);
    ws.resume_download("emb").unwrap();
    ws.wait_download();
    assert_eq!(state(&ws, "emb"), DownloadState::Installed);
}

#[test]
fn us_09_4_ac4_checksum_mismatch_deletes() {
    let s = setup(strong());
    s.server.faults(|f| f.corrupt = true);
    let ws = s.open();
    ws.start_download("emb").unwrap();
    ws.wait_download();
    let v = view(&ws, "emb");
    assert_eq!(
        v.state,
        DownloadState::Available,
        "INV-MM-01: never Installed"
    );
    let e = v.error.unwrap();
    assert_eq!(e.code, "corrupted");
    assert_eq!(e.message, CORRUPTED);
    assert_eq!(CORRUPTED, "Download corrupted. Please retry.");
    let fin = file_path(&s.models_dir(), &s.emb, &s.emb.files[0]);
    assert!(!fin.exists());
    assert!(!part_path(&s.models_dir(), &s.emb, &s.emb.files[0]).exists());
    // Retry with good bytes: the error goes.
    s.server.faults(|f| f.corrupt = false);
    ws.start_download("emb").unwrap();
    ws.wait_download();
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Installed);
    assert_eq!(v.error, None);
}

/// NFR-SEC-11: a partial file changed on disk while paused is caught by the final check.
#[test]
fn nfr_sec_11_tampered_partial_is_rejected() {
    let s = setup(strong());
    s.server
        .faults(|f| f.throttle = Some(Duration::from_millis(20)));
    let ws = s.open();
    ws.start_download("emb").unwrap();
    wait_until("bytes", || view(&ws, "emb").bytes_done > 256 * 1024);
    ws.pause_download("emb").unwrap();
    ws.wait_download();
    let part = part_path(&s.models_dir(), &s.emb, &s.emb.files[0]);
    let mut bytes = std::fs::read(&part).unwrap();
    bytes[10] ^= 0xff;
    std::fs::write(&part, bytes).unwrap();
    s.server.faults(|f| f.throttle = None);
    ws.resume_download("emb").unwrap();
    ws.wait_download();
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Available);
    assert_eq!(v.error.unwrap().code, "corrupted");
    assert!(!part.exists());
}

#[test]
fn us_09_4_ac5_delete_frees_space() {
    let s = setup(strong());
    let ws = s.open();
    ws.start_download("ext").unwrap();
    ws.wait_download();
    assert_eq!(state(&ws, "ext"), DownloadState::Installed);
    assert_eq!(
        ws.delete_model("emb").map(|_| ()),
        Ok(()),
        "deleting a model that is not there is fine"
    );
    s.probe
        .free
        .store(100 * GB + s.ext.bytes(), Ordering::SeqCst);
    let free = ws.delete_model("ext").unwrap();
    assert_eq!(free, Some(100 * GB + s.ext.bytes()));
    assert_eq!(
        ws.hardware().unwrap().profile.free_disk_bytes,
        Some(100 * GB + s.ext.bytes()),
        "the shown free space is read again"
    );
    assert_eq!(state(&ws, "ext"), DownloadState::Available);
    assert!(!s.models_dir().join("ext").exists());
}

/// An installed model whose file is gone on disk shows as Available again.
#[test]
fn installed_with_missing_file_is_available() {
    let s = setup(strong());
    let ws = s.open();
    ws.start_download("ext").unwrap();
    ws.wait_download();
    assert_eq!(ws.start_download("ext").unwrap_err().code, "invalid_state");
    std::fs::remove_file(file_path(&s.models_dir(), &s.ext, &s.ext.files[0])).unwrap();
    assert_eq!(state(&ws, "ext"), DownloadState::Available);
    ws.start_download("ext").unwrap();
    ws.wait_download();
    assert_eq!(state(&ws, "ext"), DownloadState::Installed);
}

/// NFR-SEC-02: a redirect to a host outside the catalog is refused before it is looked up.
#[test]
fn nfr_sec_02_redirect_outside_catalog_refused() {
    let s = setup(strong());
    s.server
        .faults(|f| f.redirect_to = Some("evil.example".into()));
    let ws = s.open();
    ws.start_download("emb").unwrap();
    ws.wait_download();
    let v = view(&ws, "emb");
    assert_eq!(v.state, DownloadState::Interrupted);
    assert_eq!(v.error.unwrap().code, "download_host_not_allowed");
    assert!(
        !s.resolver.asked().iter().any(|h| h == "evil.example"),
        "{:?}",
        s.resolver.asked()
    );
}

#[test]
fn unknown_model_is_refused() {
    let s = setup(strong());
    let ws = s.open();
    assert_eq!(
        ws.start_download("nope").unwrap_err().code,
        "model_not_found"
    );
}
