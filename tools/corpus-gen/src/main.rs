//! Generates the Mangkon AI test corpus (TD-1 synthetic slice, TD-3) and its ground truth.
//! The corpus is not committed; `corpus.sha256` is. See `docs/07-test-corpus.md`.
//!
//! ```text
//! corpus-gen fetch    [--out testdata]   download the public slice and check its hashes
//! corpus-gen generate [--out testdata]   write the corpus and update corpus.sha256
//! corpus-gen check    [--out testdata]   regenerate into a temp dir and compare with corpus.sha256
//! corpus-gen td2 [--copies 99] [--out testdata]   write TD-2 (scale corpus) from TD-1
//! ```

mod afm;
mod corpus;
mod fonts;
mod formats;
mod ids;
mod layout;
mod pdf;
mod public;
mod raster;
mod rich;
mod story;
mod td2;
mod td3;
mod truth;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use mk_bench::Generator;

/// Bump when any output byte changes for the same seed.
pub const GENERATOR_VERSION: u32 = 1;
pub const SEED: u64 = 20261022;

/// SHA-256 of every generated file, committed so that drift shows up without committing data.
const HASH_LIST: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/corpus.sha256");

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut out = PathBuf::from("testdata");
    let mut cmd = None;
    let mut copies = td2::DEFAULT_COPIES;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => match it.next() {
                Some(p) => out = PathBuf::from(p),
                None => return usage(),
            },
            "--copies" => match it.next().and_then(|n| n.parse().ok()) {
                Some(n) => copies = n,
                None => return usage(),
            },
            "fetch" | "generate" | "check" | "td2" if cmd.is_none() => cmd = Some(a.clone()),
            _ => return usage(),
        }
    }
    let result = match cmd.as_deref() {
        Some("fetch") => public::fetch(&out.join("td1"))
            .map(|n| println!("fetched {n} files; public slice complete")),
        Some("generate") => generate(&out).and_then(|n| {
            std::fs::write(HASH_LIST, hash_list(&owned(&out)?))
                .map_err(|e| vec![format!("{HASH_LIST}: {e}")])?;
            println!("wrote {n} files under {}", out.display());
            Ok(())
        }),
        Some("check") => check(&out),
        Some("td2") => td2::build(
            &out,
            copies,
            SEED,
            &format!("corpus-gen {GENERATOR_VERSION}"),
        )
        .map(|m| {
            println!(
                "wrote {} files ({} bytes, ~{} segments) under {}",
                m.files,
                m.bytes,
                m.segment_estimate,
                out.join("td2").display()
            )
        }),
        _ => return usage(),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(errors) => {
            for e in &errors {
                eprintln!("error: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: corpus-gen (fetch | generate | check | td2 [--copies N]) [--out DIR]");
    ExitCode::from(2)
}

fn generator() -> Generator {
    Generator {
        name: "corpus-gen".into(),
        version: GENERATOR_VERSION,
        seed: SEED,
    }
}

/// Writes everything the generator owns under `out`. The public slice (`td1/public/`) is
/// read, not written; `fetch` puts it there. Returns the number of files written.
fn generate(out: &Path) -> Result<usize, Vec<String>> {
    for dir in ["td1/synthetic", "td3"] {
        let d = out.join(dir);
        if d.exists() {
            std::fs::remove_dir_all(&d).map_err(|e| vec![format!("{}: {e}", d.display())])?;
        }
    }
    let story = story::Story::new(SEED);
    let noto = fonts::Noto::load();
    let mut c = truth::Collector::new(&story, &out.join("td1"));
    corpus::build(&mut c, &noto, SEED);
    public::build(&mut c, &out.join("td1"))?;
    let gt = c.finish(generator(), "page-text.jsonl")?;
    gt.validate(Some(&out.join("td1"))).map_err(|e| {
        e.into_iter()
            .map(|e| format!("validate: {e}"))
            .collect::<Vec<_>>()
    })?;
    let (main, pages) = mk_bench::to_files(&gt);
    let write = |p: PathBuf, s: &str| {
        std::fs::write(&p, s).map_err(|e| vec![format!("{}: {e}", p.display())])
    };
    write(out.join("td1/ground-truth.json"), &main)?;
    write(out.join("td1/page-text.jsonl"), &pages)?;

    let td3_count = td3::build(&out.join("td3"), generator(), &noto)?;
    let synthetic = gt
        .files
        .iter()
        .filter(|f| f.origin == mk_bench::Origin::Synthetic)
        .count();
    Ok(synthetic + 2 + td3_count)
}

/// The paths `generate` owns, relative to `out`.
fn owned(out: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, Vec<String>> {
    let mut files = BTreeMap::new();
    for dir in ["td1/synthetic", "td3"] {
        collect(out, &out.join(dir), &mut files)?;
    }
    for f in ["td1/ground-truth.json", "td1/page-text.jsonl"] {
        let p = out.join(f);
        let bytes = std::fs::read(&p).map_err(|e| vec![format!("{}: {e}", p.display())])?;
        files.insert(PathBuf::from(f), bytes);
    }
    Ok(files)
}

fn collect(
    base: &Path,
    dir: &Path,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), Vec<String>> {
    let err = |e: std::io::Error| vec![format!("{}: {e}", dir.display())];
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir).map_err(err)? {
        let path = entry.map_err(err)?.path();
        if path.is_dir() {
            collect(base, &path, files)?;
        } else {
            let bytes = std::fs::read(&path).map_err(err)?;
            files.insert(path.strip_prefix(base).unwrap().to_path_buf(), bytes);
        }
    }
    Ok(())
}

/// `sha256sum` format, sorted by path, so `sha256sum -c` works from inside `testdata/`.
fn hash_list(files: &BTreeMap<PathBuf, Vec<u8>>) -> String {
    let mut out = String::new();
    for (p, bytes) in files {
        let p = p.to_string_lossy().replace('\\', "/");
        out.push_str(&format!("{}  {p}\n", mk_bench::sha256_hex(bytes)));
    }
    out
}

/// Regenerates into a temp dir (with the local public slice copied in) and compares every
/// file's hash with the committed `corpus.sha256`.
fn check(out: &Path) -> Result<(), Vec<String>> {
    let want = std::fs::read_to_string(HASH_LIST).map_err(|e| vec![format!("{HASH_LIST}: {e}")])?;
    let tmp = std::env::temp_dir().join(format!("corpus-gen-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let public = out.join("td1/public");
    copy_dir(&public, &tmp.join("td1/public"))
        .map_err(|e| vec![format!("copy public slice: {e}")])?;
    let result = generate(&tmp).and_then(|_| {
        let got = hash_list(&owned(&tmp)?);
        let parse = |s: &str| -> BTreeMap<String, String> {
            s.lines()
                .filter_map(|l| l.split_once("  "))
                .map(|(h, p)| (p.to_string(), h.to_string()))
                .collect()
        };
        let (want, got) = (parse(&want), parse(&got));
        let mut errors = Vec::new();
        for (p, h) in &got {
            match want.get(p) {
                None => errors.push(format!("{p} is not in corpus.sha256")),
                Some(w) if w != h => errors.push(format!("{p} differs from corpus.sha256")),
                Some(_) => {}
            }
        }
        for p in want.keys().filter(|p| !got.contains_key(*p)) {
            errors.push(format!("{p} is in corpus.sha256 but not generated"));
        }
        if errors.is_empty() {
            println!("ok: {} generated files match corpus.sha256", got.len());
            Ok(())
        } else {
            Err(errors)
        }
    });
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    if !from.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(from)? {
        let path = entry?.path();
        let dest = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &dest)?;
        } else {
            std::fs::copy(&path, &dest)?;
        }
    }
    Ok(())
}
