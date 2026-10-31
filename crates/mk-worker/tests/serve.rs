//! The worker loop in process, over pipes (ADR-0007): hello and protocol version, chunks that
//! reassemble to the parse, `file_changed`, panics and unknown types fail only their item; the
//! page view's `RenderPage` (ADR-0011) and the OCR languages a request names (US-02.2 AC4).

use std::io::{PipeReader, PipeWriter};
use std::path::Path;

use mk_kernel::ContentHash;
use mk_worker::frame::{FrameReader, FrameWriter};
use mk_worker::{
    Chunk, FromWorker, Limits, PROTOCOL, ProcessContent, RenderPage, ServeOptions, Settings,
    ToWorker,
};

struct Worker {
    to: FrameWriter<PipeWriter>,
    from: FrameReader<PipeReader>,
    thread: std::thread::JoinHandle<std::io::Result<()>>,
}

fn start() -> (Worker, FromWorker) {
    let (req_r, req_w) = std::io::pipe().unwrap();
    let (rep_r, rep_w) = std::io::pipe().unwrap();
    let thread =
        std::thread::spawn(move || mk_worker::serve(req_r, rep_w, ServeOptions::default()));
    let mut w = Worker {
        to: FrameWriter::new(req_w),
        from: FrameReader::new(rep_r),
        thread,
    };
    let hello = w.from.recv().unwrap().unwrap();
    (w, hello)
}

fn request(path: &Path, file_type: &str, bytes: &[u8], test_faults: bool) -> ToWorker {
    ToWorker::Process(ProcessContent {
        path: path.to_str().unwrap().into(),
        content_hash: ContentHash::of(bytes),
        file_type: file_type.into(),
        limits: Limits { chunk_segments: 3 },
        settings: Settings {
            test_faults,
            ..Settings::default()
        },
    })
}

/// Replies up to and including `Done` or `Failed`, heartbeats left out.
fn replies(w: &mut Worker) -> Vec<FromWorker> {
    let mut out = Vec::new();
    loop {
        let msg: FromWorker = w.from.recv().unwrap().unwrap();
        match msg {
            FromWorker::Heartbeat { .. } => continue,
            FromWorker::Done { .. } | FromWorker::Failed { .. } => {
                out.push(msg);
                return out;
            }
            _ => out.push(msg),
        }
    }
}

#[test]
fn hello_names_the_protocol() {
    let (mut w, hello) = start();
    match hello {
        FromWorker::Hello {
            protocol,
            app_version,
            pid,
        } => {
            assert_eq!(protocol, PROTOCOL);
            assert_eq!(app_version, env!("CARGO_PKG_VERSION"));
            assert_eq!(pid, std::process::id());
        }
        other => panic!("{other:?}"),
    }
    w.to.send(&ToWorker::Shutdown).unwrap();
    w.thread.join().unwrap().unwrap();
}

/// Chunks reassemble to exactly what the parser and the identifier step give in process.
#[test]
fn chunks_match_the_parse_and_items_fail_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let mut csv = String::from("name;email\n");
    for i in 0..10 {
        csv.push_str(&format!("p{i};p{i}@x.example\n"));
    }
    csv.push_str("ragged\n");
    let good = tmp.path().join("people.csv");
    std::fs::write(&good, &csv).unwrap();
    let bad = tmp.path().join("binary.txt");
    std::fs::write(&bad, b"\x89PNG\r\n\x1a\n\0\0").unwrap();
    let cut = tmp.path().join("cut.json");
    std::fs::write(&cut, br#"["a@x.example", "#).unwrap();
    let panics = tmp.path().join("panic.txt");
    std::fs::write(&panics, "mk-test-fault: panic\n").unwrap();

    let (mut w, _) = start();
    w.to.send(&request(&good, "csv", csv.as_bytes(), false))
        .unwrap();
    let got = replies(&mut w);
    let parsed = mk_parsers::parse(mk_parsers::Format::Csv, csv.as_bytes()).unwrap();
    let mut want = Vec::new();
    mk_worker::extract::chunks::<()>(
        parsed,
        3,
        &mk_identifiers::Rules::default(),
        |c| {
            want.push(FromWorker::Chunk(c));
            Ok(())
        },
        |_| {},
    )
    .unwrap();
    want.push(FromWorker::Done {
        warnings: vec!["1 row has the wrong number of columns".into()],
    });
    assert_eq!(got, want);
    assert!(
        got.iter()
            .filter(|m| matches!(m, FromWorker::Chunk(Chunk::Segments(_))))
            .count()
            >= 4
    );

    // A file that changed since it was hashed.
    let mut changed = request(&good, "csv", b"other bytes", false);
    w.to.send(&changed).unwrap();
    assert_eq!(
        replies(&mut w),
        vec![FromWorker::Failed {
            code: "file_changed".into(),
            plain_reason: "File changed during processing".into()
        }]
    );
    // Binary, a panic in the parser step, an unknown type, a missing file: each fails alone.
    w.to.send(&request(&bad, "txt", b"\x89PNG\r\n\x1a\n\0\0", false))
        .unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, plain_reason }]
        if code == "unknown_text_encoding" && plain_reason == "Unknown text encoding")
    );
    // A failure after chunks were sent: the core drops what it staged.
    let json: &[u8] = br#"["a@x.example", "#;
    w.to.send(&request(&cut, "json", json, false)).unwrap();
    let got = replies(&mut w);
    assert!(matches!(
        got.first(),
        Some(FromWorker::Chunk(Chunk::Document { .. }))
    ));
    assert!(
        matches!(got.last(), Some(FromWorker::Failed { code, .. }) if code == "invalid_json"),
        "{got:?}"
    );
    w.to.send(&request(&panics, "txt", b"mk-test-fault: panic\n", true))
        .unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, .. }]
        if code == "parser_crashed")
    );
    w.to.send(&request(&good, "xlsx", csv.as_bytes(), false))
        .unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, .. }]
        if code == "unsupported_type")
    );
    // Since M3b a PDF is read by its bytes: CSV text is "Not a PDF".
    w.to.send(&request(&good, "pdf", csv.as_bytes(), false))
        .unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, .. }]
        if code == "not_a_pdf")
    );
    if let ToWorker::Process(p) = &mut changed {
        p.path = tmp.path().join("gone.txt").to_str().unwrap().into();
    }
    w.to.send(&changed).unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, .. }]
        if code == "file_missing")
    );

    // Without test faults the marker is plain text.
    w.to.send(&request(&panics, "txt", b"mk-test-fault: panic\n", false))
        .unwrap();
    assert!(matches!(
        replies(&mut w).last(),
        Some(FromWorker::Done { .. })
    ));

    // Still serving after all that.
    w.to.send(&request(&good, "csv", csv.as_bytes(), false))
        .unwrap();
    assert_eq!(replies(&mut w), want);
    drop(w.to);
    w.thread.join().unwrap().unwrap();
}

/// A small grey PNG: a white page with a dark bar.
fn png(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = vec![255u8; (width * height) as usize];
    for y in height / 3..height / 2 {
        for x in width / 4..width * 3 / 4 {
            pixels[(y * width + x) as usize] = 0;
        }
    }
    mk_parsers::image::GrayImage {
        width,
        height,
        pixels,
    }
    .to_png()
    .unwrap()
}

/// ADR-0011: an image page comes back as itself; a page past the end and a changed file fail
/// with a plain reason, and the worker goes on.
#[test]
fn render_page_of_an_image() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scan.png");
    let bytes = png(120, 80);
    std::fs::write(&path, &bytes).unwrap();
    let (mut w, _) = start();
    let ask = |page: u32, hash: ContentHash| {
        ToWorker::RenderPage(RenderPage {
            path: path.to_str().unwrap().into(),
            content_hash: hash,
            file_type: "image".into(),
            page,
            scale: 2.0,
        })
    };
    w.to.send(&ask(1, ContentHash::of(&bytes))).unwrap();
    match w.from.recv::<FromWorker>().unwrap().unwrap() {
        FromWorker::PageImage(p) => {
            assert_eq!((p.width, p.height, p.pages), (120, 80, 1));
            assert!(p.points.is_none() && p.geometry.is_none());
            let back = image_size(&p.png);
            assert_eq!(back, (120, 80));
        }
        other => panic!("{other:?}"),
    }
    for (page, hash, code) in [
        (2, ContentHash::of(&bytes), "no_such_page"),
        (1, ContentHash::of(b"other"), "file_changed"),
    ] {
        w.to.send(&ask(page, hash)).unwrap();
        match w.from.recv::<FromWorker>().unwrap().unwrap() {
            FromWorker::Failed { code: c, .. } => assert_eq!(c, code),
            other => panic!("{other:?}"),
        }
    }
    w.to.send(&ToWorker::Shutdown).unwrap();
    w.thread.join().unwrap().unwrap();
}

/// The width and height in a PNG's header.
fn image_size(png: &[u8]) -> (u32, u32) {
    let n = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
    (n(16), n(20))
}

/// US-02.2 AC4: the worker OCRs with the languages the request names, from the folder it names;
/// a missing language or no folder fails the file with a plain reason.
#[test]
fn us_02_2_ac4_ocr_languages_reach_the_worker() {
    let dir = tempfile::tempdir().unwrap();
    let tessdata = dir.path().join("tessdata");
    mk_parsers::ocr::install_english(&tessdata).unwrap();
    let path = dir.path().join("scan.png");
    let bytes = png(200, 100);
    std::fs::write(&path, &bytes).unwrap();
    let (mut w, _) = start();
    let ask = |languages: &str, tessdata: Option<&Path>| {
        ToWorker::Process(ProcessContent {
            path: path.to_str().unwrap().into(),
            content_hash: ContentHash::of(&bytes),
            file_type: "image".into(),
            limits: Limits::default(),
            settings: Settings {
                test_faults: false,
                ocr_languages: languages.into(),
                tessdata: tessdata.map(|t| t.to_str().unwrap().into()),
                ..Settings::default()
            },
        })
    };
    let last = |w: &mut Worker| replies(w).pop().unwrap();
    w.to.send(&ask("eng", Some(&tessdata))).unwrap();
    assert!(matches!(last(&mut w), FromWorker::Done { .. }));
    for (langs, folder, reason) in [
        (
            "eng+deu",
            Some(tessdata.as_path()),
            "OCR language not installed: deu",
        ),
        ("eng", None, "OCR is not available for this file"),
    ] {
        w.to.send(&ask(langs, folder)).unwrap();
        match last(&mut w) {
            FromWorker::Failed { plain_reason, .. } => assert_eq!(plain_reason, reason),
            other => panic!("{other:?}"),
        }
    }
    w.to.send(&ToWorker::Shutdown).unwrap();
    w.thread.join().unwrap().unwrap();
}
