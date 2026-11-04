//! US-09.4 AC2–AC4, NFR-REL-08, NFR-SEC-02, NFR-SEC-11, NFR-RES-05 against a fake Hugging Face
//! (`testkit`): redirects to a second host, resume with `Range`, drops, wrong bytes, a full
//! disk, hosts outside the catalog.

use std::path::Path;
use std::sync::Arc;

use mk_models::Role;
use mk_models::download::{
    Control, DownloadConfig, Downloader, ErrorCode, Event, Outcome, bytes_on_disk, file_path,
    part_path, remaining_bytes,
};
use mk_models::state::{CORRUPTED, DownloadState};
use mk_models::testkit::{
    CDN_HOST, FakeHf, NamedResolver, ORIGIN_HOST, test_bytes, test_catalog, test_model,
};

const MIB: usize = 1024 * 1024;

struct Setup {
    model: mk_models::Model,
    bodies: Vec<Vec<u8>>,
    server: FakeHf,
    resolver: Arc<NamedResolver>,
    hosts: Vec<String>,
    dir: tempfile::TempDir,
}

fn setup(sizes: &[usize]) -> Setup {
    let bodies: Vec<Vec<u8>> = sizes
        .iter()
        .enumerate()
        .map(|(i, n)| test_bytes(*n, i as u8 + 1))
        .collect();
    let model = test_model("emb", Role::Embeddings, &bodies);
    let server = FakeHf::serving(&[(&model, &bodies)]);
    let hosts = test_catalog(vec![model.clone()]).download_hosts;
    Setup {
        model,
        bodies,
        server,
        resolver: NamedResolver::new(&[ORIGIN_HOST, CDN_HOST]),
        hosts,
        dir: tempfile::tempdir().expect("temp dir"),
    }
}

impl Setup {
    fn config(&self) -> DownloadConfig {
        self.server.config(self.resolver.clone())
    }

    fn downloader(&self) -> Downloader {
        Downloader::new(self.config(), self.hosts.clone())
    }

    fn run(
        &self,
        d: &Downloader,
        control: &Control,
        on: &mut dyn FnMut(Event),
    ) -> Result<Outcome, mk_models::download::DownloadError> {
        d.download(&self.model, self.dir.path(), &|_| false, control, on)
    }

    fn installed_ok(&self) {
        for (f, body) in self.model.files.iter().zip(&self.bodies) {
            let p = file_path(self.dir.path(), &self.model, f);
            assert_eq!(&std::fs::read(&p).expect("final file"), body, "{}", f.name);
            assert!(!part_path(self.dir.path(), &self.model, f).exists());
        }
    }
}

fn no_events(_: Event) {}

#[test]
fn downloads_through_the_cdn_redirect() {
    let s = setup(&[MIB + 123]);
    s.server.faults(|f| f.redirect_to = Some(CDN_HOST.into()));
    let mut events = Vec::new();
    let out = s.run(&s.downloader(), &Control::default(), &mut |e| {
        events.push(e)
    });
    assert_eq!(out, Ok(Outcome::Installed));
    s.installed_ok();
    let seen = s.server.seen();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!(seen[0].host, ORIGIN_HOST);
    assert_eq!(seen[1].host, CDN_HOST);
    assert!(seen[1].path.starts_with("/cdn/test/emb/resolve/"));
    assert!(events.contains(&Event::Verifying));
    assert!(events.contains(&Event::FileVerified("emb.gguf".into())));
    let Some(Event::Progress(last)) = events
        .iter()
        .rev()
        .find(|e| matches!(e, Event::Progress(_)))
    else {
        panic!("no progress")
    };
    assert_eq!(last.bytes_done, (MIB + 123) as u64);
    assert_eq!(last.total, (MIB + 123) as u64);
}

#[test]
fn nfr_sec_02_redirect_to_unlisted_host_is_refused_before_connecting() {
    let s = setup(&[1000]);
    s.server
        .faults(|f| f.redirect_to = Some("evil.example".into()));
    let err = s
        .run(&s.downloader(), &Control::default(), &mut no_events)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::HostNotAllowed);
    assert_eq!(err.code.as_str(), "download_host_not_allowed");
    assert!(err.message.contains("evil.example"), "{}", err.message);
    assert_eq!(err.state_after(), DownloadState::Interrupted);
    // The resolver was never asked for it, and the server saw only the first request.
    assert_eq!(s.resolver.asked(), vec![ORIGIN_HOST.to_string()]);
    assert_eq!(s.server.seen().len(), 1);
}

#[test]
fn nfr_sec_02_https_only_without_the_test_switch() {
    let s = setup(&[1000]);
    let cfg = DownloadConfig {
        allow_http: false,
        ..s.config()
    };
    let err = Downloader::new(cfg, s.hosts.clone())
        .download(
            &s.model,
            s.dir.path(),
            &|_| false,
            &Control::default(),
            &mut no_events,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::HostNotAllowed);
    assert!(s.resolver.asked().is_empty());
    assert!(s.server.seen().is_empty());
}

#[test]
fn nfr_sec_02_origin_outside_the_catalog_is_refused() {
    let s = setup(&[1000]);
    let err = Downloader::new(s.config(), vec!["huggingface.co".into()])
        .download(
            &s.model,
            s.dir.path(),
            &|_| false,
            &Control::default(),
            &mut no_events,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::HostNotAllowed);
    assert!(s.resolver.asked().is_empty());
}

#[test]
fn us_09_4_ac3_resume_after_drop() {
    let s = setup(&[3 * MIB + 7]);
    s.server.faults(|f| {
        f.redirect_to = Some(CDN_HOST.into());
        f.drop_after = Some(MIB as u64 + 5);
    });
    let d = s.downloader();
    let err = s.run(&d, &Control::default(), &mut no_events).unwrap_err();
    assert_eq!(err.code, ErrorCode::Network);
    assert_eq!(err.state_after(), DownloadState::Interrupted);
    let part = part_path(s.dir.path(), &s.model, &s.model.files[0]);
    let kept = std::fs::metadata(&part).expect("part kept").len();
    assert_eq!(kept, MIB as u64 + 5);
    assert_eq!(bytes_on_disk(s.dir.path(), &s.model), kept);
    assert_eq!(
        remaining_bytes(s.dir.path(), &s.model),
        s.model.bytes() - kept
    );

    // Resume (a new downloader, as after an app restart): asks the origin again, with Range.
    let mut first = None;
    let out = s.run(&s.downloader(), &Control::default(), &mut |e| {
        if let Event::Progress(p) = e {
            first.get_or_insert(p.bytes_done);
        }
    });
    assert_eq!(out, Ok(Outcome::Installed));
    s.installed_ok();
    assert_eq!(first, Some(kept), "progress starts at the kept bytes");
    let seen = s.server.seen();
    let resumed: Vec<_> = seen.iter().filter(|r| r.range.is_some()).collect();
    assert_eq!(resumed.len(), 2, "{seen:?}");
    assert_eq!(resumed[0].host, ORIGIN_HOST);
    assert_eq!(resumed[1].host, CDN_HOST);
    assert_eq!(
        resumed[1].range.as_deref(),
        Some(&*format!("bytes={kept}-"))
    );
}

#[test]
fn resume_restarts_when_the_server_ignores_range() {
    let s = setup(&[2 * MIB]);
    s.server.faults(|f| f.drop_after = Some(MIB as u64));
    assert!(
        s.run(&s.downloader(), &Control::default(), &mut no_events)
            .is_err()
    );
    s.server.faults(|f| f.ignore_range = true);
    assert_eq!(
        s.run(&s.downloader(), &Control::default(), &mut no_events),
        Ok(Outcome::Installed)
    );
    s.installed_ok();
}

#[test]
fn us_09_4_ac2_pause_and_cancel() {
    let s = setup(&[4 * MIB]);
    s.server
        .faults(|f| f.throttle = Some(std::time::Duration::from_millis(5)));
    let d = s.downloader();
    let control = Control::default();
    let mut progress = Vec::new();
    let out = s.run(&d, &control, &mut |e| {
        if let Event::Progress(p) = e {
            progress.push(p);
            if p.bytes_done > 0 {
                control.pause();
            }
        }
    });
    assert_eq!(out, Ok(Outcome::Paused));
    let part = part_path(s.dir.path(), &s.model, &s.model.files[0]);
    let kept = std::fs::metadata(&part).expect("part kept").len();
    assert!(kept > 0 && kept < 4 * MIB as u64, "{kept}");
    assert!(progress.iter().all(|p| p.total == 4 * MIB as u64));

    // Cancel while downloading deletes the partial file and the folder.
    let control = Control::default();
    let out = s.run(&d, &control, &mut |e| {
        if matches!(e, Event::Progress(p) if p.bytes_done > kept) {
            control.cancel();
        }
    });
    assert_eq!(out, Ok(Outcome::Cancelled));
    assert!(!s.dir.path().join("emb").exists());
}

#[test]
fn us_09_4_ac2_speed_and_time_left() {
    let s = setup(&[6 * MIB]);
    s.server
        .faults(|f| f.throttle = Some(std::time::Duration::from_millis(20)));
    let mut seen = Vec::new();
    let out = s.run(&s.downloader(), &Control::default(), &mut |e| {
        if let Event::Progress(p) = e {
            seen.push(p);
        }
    });
    assert_eq!(out, Ok(Outcome::Installed));
    let measured: Vec<_> = seen.iter().filter(|p| p.speed_bps > 0).collect();
    assert!(!measured.is_empty(), "{seen:?}");
    assert!(
        measured
            .iter()
            .any(|p| p.eta_s.is_some() && p.bytes_done < p.total)
    );
}

#[test]
fn us_09_4_ac4_checksum_mismatch_deletes() {
    let s = setup(&[MIB]);
    s.server.faults(|f| f.corrupt = true);
    let mut events = Vec::new();
    let err = s
        .run(&s.downloader(), &Control::default(), &mut |e| {
            events.push(e)
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Corrupted);
    assert_eq!(err.message, CORRUPTED);
    assert_eq!(err.state_after(), DownloadState::Available);
    assert!(!part_path(s.dir.path(), &s.model, &s.model.files[0]).exists());
    assert!(!file_path(s.dir.path(), &s.model, &s.model.files[0]).exists());
    assert!(!events.iter().any(|e| matches!(e, Event::FileVerified(_))));
}

#[test]
fn nfr_sec_11_part_tampered_while_paused_is_rejected() {
    let s = setup(&[2 * MIB]);
    s.server.faults(|f| f.drop_after = Some(MIB as u64));
    assert!(
        s.run(&s.downloader(), &Control::default(), &mut no_events)
            .is_err()
    );
    let part = part_path(s.dir.path(), &s.model, &s.model.files[0]);
    let mut bytes = std::fs::read(&part).expect("part");
    bytes[10] ^= 1;
    std::fs::write(&part, &bytes).expect("tamper");
    let err = s
        .run(&s.downloader(), &Control::default(), &mut no_events)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Corrupted);
    assert!(!part.exists());
    assert!(!file_path(s.dir.path(), &s.model, &s.model.files[0]).exists());
}

#[test]
fn nfr_res_05_disk_full_mid_download_is_an_interruption() {
    let s = setup(&[2 * MIB]);
    let cfg = DownloadConfig {
        disk_full_after: Some(MIB as u64 / 2),
        ..s.config()
    };
    let err = Downloader::new(cfg, s.hosts.clone())
        .download(
            &s.model,
            s.dir.path(),
            &|_| false,
            &Control::default(),
            &mut no_events,
        )
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::DiskFull);
    assert!(err.message.starts_with("Disk full"), "{}", err.message);
    assert_eq!(err.state_after(), DownloadState::Interrupted);
    // The bytes written are kept; with space again the download resumes.
    let part = part_path(s.dir.path(), &s.model, &s.model.files[0]);
    assert_eq!(
        std::fs::metadata(&part).expect("part").len(),
        MIB as u64 / 2
    );
    assert_eq!(
        s.run(&s.downloader(), &Control::default(), &mut no_events),
        Ok(Outcome::Installed)
    );
    s.installed_ok();
}

#[test]
fn split_model_skips_verified_files() {
    let s = setup(&[MIB, 300_000, 5]);
    let d = s.downloader();
    let mut verified = Vec::new();
    let out = d.download(
        &s.model,
        s.dir.path(),
        &|_| false,
        &Control::default(),
        &mut |e| {
            if let Event::FileVerified(n) = e {
                verified.push(n);
            }
        },
    );
    assert_eq!(out, Ok(Outcome::Installed));
    assert_eq!(verified.len(), 3);
    s.installed_ok();
    let before = s.server.seen().len();
    // Run again with all verified: nothing is fetched.
    let out = d.download(
        &s.model,
        s.dir.path(),
        &|_| true,
        &Control::default(),
        &mut no_events,
    );
    assert_eq!(out, Ok(Outcome::Installed));
    assert_eq!(s.server.seen().len(), before);
}

#[test]
fn final_file_not_recorded_is_checked_again() {
    let s = setup(&[MIB]);
    let fin = file_path(s.dir.path(), &s.model, &s.model.files[0]);
    std::fs::create_dir_all(fin.parent().unwrap_or(Path::new("."))).expect("dir");
    // Right size, wrong bytes, not recorded as verified: hashed, rejected, deleted.
    std::fs::write(&fin, vec![0u8; MIB]).expect("write");
    let err = s
        .run(&s.downloader(), &Control::default(), &mut no_events)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Corrupted);
    assert!(!fin.exists());
    assert_eq!(
        s.run(&s.downloader(), &Control::default(), &mut no_events),
        Ok(Outcome::Installed)
    );
}

#[test]
fn server_errors_are_interruptions() {
    let s = setup(&[1000]);
    s.server.faults(|f| f.status = Some(503));
    let err = s
        .run(&s.downloader(), &Control::default(), &mut no_events)
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Network);
    assert!(err.message.contains("HTTP 503"), "{}", err.message);
}

/// Step 2 of M5b: the real Hugging Face path (redirect to the CDN, Range on the signed URL).
/// Downloads nomic-embed-text (146 MB) into `$MK_REAL_DIR`, pausing at ~40 MB and resuming
/// with a new downloader. Run with `--ignored --nocapture`.
#[test]
#[ignore = "downloads 146 MB from huggingface.co"]
fn real_hf_nomic_pause_resume() {
    let dir = std::path::PathBuf::from(std::env::var("MK_REAL_DIR").expect("MK_REAL_DIR"));
    let catalog = mk_models::Catalog::shipped();
    let model = catalog
        .model("nomic-embed-text-v1.5-q8_0")
        .expect("nomic")
        .clone();
    let started = std::time::Instant::now();
    let control = Control::default();
    let out = Downloader::new(DownloadConfig::default(), catalog.download_hosts.clone()).download(
        &model,
        &dir,
        &|_| false,
        &control,
        &mut |e| {
            if let Event::Progress(p) = e
                && p.bytes_done > 40_000_000
            {
                control.pause();
            }
        },
    );
    assert_eq!(out, Ok(Outcome::Paused));
    let kept = bytes_on_disk(&dir, &model);
    eprintln!("paused at {kept} bytes after {:?}", started.elapsed());
    let mut last = None;
    let out = Downloader::new(DownloadConfig::default(), catalog.download_hosts.clone()).download(
        &model,
        &dir,
        &|_| false,
        &Control::default(),
        &mut |e| {
            if let Event::Progress(p) = e {
                last = Some(p);
            }
        },
    );
    eprintln!("resumed: {out:?} {last:?} total {:?}", started.elapsed());
    assert_eq!(out, Ok(Outcome::Installed));
}
