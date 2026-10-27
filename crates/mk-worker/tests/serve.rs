//! The worker loop in process, over pipes (ADR-0007): hello and protocol version, chunks that
//! reassemble to the parse, `file_changed`, panics and unknown types fail only their item.

use std::io::{PipeReader, PipeWriter};
use std::path::Path;

use mk_kernel::ContentHash;
use mk_worker::frame::{FrameReader, FrameWriter};
use mk_worker::{
    Chunk, FromWorker, Limits, PROTOCOL, ProcessContent, ServeOptions, Settings, ToWorker,
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
        settings: Settings { test_faults },
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
    let bad = tmp.path().join("latin1.txt");
    std::fs::write(&bad, b"caf\xe9\n").unwrap();
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
    // Not UTF-8, a panic in the parser step, an unknown type, a missing file: each fails alone.
    w.to.send(&request(&bad, "txt", b"caf\xe9\n", false))
        .unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, plain_reason }]
        if code == "unknown_text_encoding" && plain_reason == "Unknown text encoding")
    );
    w.to.send(&request(&panics, "txt", b"mk-test-fault: panic\n", true))
        .unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, .. }]
        if code == "parser_crashed")
    );
    w.to.send(&request(&good, "pdf", csv.as_bytes(), false))
        .unwrap();
    assert!(
        matches!(&replies(&mut w)[..], [FromWorker::Failed { code, .. }]
        if code == "unsupported_type")
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
