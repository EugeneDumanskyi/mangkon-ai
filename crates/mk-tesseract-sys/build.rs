//! Builds Leptonica and Tesseract from the archives pinned in `native/sources.toml` and links them
//! statically (ADR-0014). Copies `eng.traineddata` next to the build output, where `src/lib.rs`
//! embeds it (ADR-0018).
//!
//! Fetching uses the system `curl` (a build prerequisite, NFR-MNT-01), so no HTTP crate enters
//! the dependency graph (ADR-0009). Every file is checked by SHA-256 before use; a mismatch fails
//! the build. Set `MK_NATIVE_SRC_DIR` to a folder with the files to build offline.

use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Sources {
    source: Vec<Source>,
}

#[derive(Deserialize)]
struct Source {
    name: String,
    url: String,
    file: String,
    sha256: String,
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let sources_path = manifest_dir.join("native/sources.toml");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", sources_path.display());
    println!("cargo:rerun-if-env-changed=MK_NATIVE_SRC_DIR");

    let sources: Sources = toml::from_str(&fs::read_to_string(&sources_path).unwrap())
        .unwrap_or_else(|e| fail(&format!("{}: {e}", sources_path.display())));
    let find = |name: &str| {
        let s = sources.source.iter().find(|s| s.name == name);
        s.unwrap_or_else(|| fail(&format!("{name} is missing from native/sources.toml")))
    };

    let leptonica_src = extract(&fetch(find("leptonica")), &out_dir.join("src/leptonica"));
    let tesseract_src = extract(&fetch(find("tesseract")), &out_dir.join("src/tesseract"));

    let leptonica = configure(&leptonica_src, &out_dir.join("leptonica"))
        .define("BUILD_PROG", "OFF")
        .define("SW_BUILD", "OFF")
        // We hand Tesseract raw grey pixels, so Leptonica needs no image codecs.
        .define("ENABLE_ZLIB", "OFF")
        .define("ENABLE_PNG", "OFF")
        .define("ENABLE_GIF", "OFF")
        .define("ENABLE_JPEG", "OFF")
        .define("ENABLE_TIFF", "OFF")
        .define("ENABLE_WEBP", "OFF")
        .define("ENABLE_OPENJPEG", "OFF")
        // Silence Leptonica's stderr messages. Tesseract asks it for a debug font on every page,
        // which fails without codecs and is harmless; Tesseract reports real errors itself.
        .cflag("-DMINIMUM_SEVERITY=L_SEVERITY_NONE")
        .build();

    let tesseract = configure(&tesseract_src, &out_dir.join("tesseract"))
        .define("CMAKE_PREFIX_PATH", &leptonica)
        .define("BUILD_TRAINING_TOOLS", "OFF")
        .define("BUILD_TESTS", "OFF")
        .define("SW_BUILD", "OFF")
        .define("OPENMP_BUILD", "OFF")
        .define("GRAPHICS_DISABLED", "ON")
        .define("DISABLE_ARCHIVE", "ON")
        .define("DISABLE_CURL", "ON")
        .define("DISABLE_TIFF", "ON")
        .define("INSTALL_CONFIGS", "OFF")
        // Portable binaries: no `-march=native`. SIMD paths are still picked at run time.
        .define("ENABLE_NATIVE", "OFF")
        .define("ENABLE_CCACHE", "OFF")
        .build();

    // Tesseract first: it needs Leptonica's symbols.
    link_static(&tesseract.join("lib"), "tesseract");
    link_static(&leptonica.join("lib"), "leptonica");
    if env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-lib=c++");
    } else if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo:rustc-link-lib=stdc++");
    }
    // MSVC links its C++ runtime by itself.

    let eng = find("tessdata_fast-eng");
    fs::copy(fetch(eng), out_dir.join("eng.traineddata")).unwrap();
    println!("cargo:rustc-env=MK_ENG_TRAINEDDATA_SHA256={}", eng.sha256);
}

/// A cmake build shared by both libraries: static, Release, position-independent.
fn configure(src: &Path, out: &Path) -> cmake::Config {
    let mut c = cmake::Config::new(src);
    c.out_dir(out)
        // Always Release: an MSVC Debug build would pick the debug C runtime, which Rust does not
        // link, and the OCR speed numbers in ADR-0014 are for optimised code.
        .profile("Release")
        .define("BUILD_SHARED_LIBS", "OFF")
        .define("CMAKE_POSITION_INDEPENDENT_CODE", "ON")
        .define("CMAKE_POLICY_VERSION_MINIMUM", "3.5");
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        c.define("CMAKE_MSVC_RUNTIME_LIBRARY", "MultiThreadedDLL");
    }
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        // Tesseract 5 uses std::filesystem, which needs macOS 10.15.
        c.define("CMAKE_OSX_DEPLOYMENT_TARGET", "10.15");
    }
    c
}

/// Links the one static library in `dir` whose name starts with `stem`. The file name varies by
/// platform (`libtesseract.a`, `tesseract55.lib`, `leptonica-1.87.0.lib`).
fn link_static(dir: &Path, stem: &str) {
    let found = fs::read_dir(dir).unwrap().flatten().find_map(|e| {
        let name = e.file_name().into_string().ok()?;
        let base = name
            .strip_suffix(".a")
            .or_else(|| name.strip_suffix(".lib"))?;
        let base = base.strip_prefix("lib").unwrap_or(base);
        base.starts_with(stem).then(|| base.to_string())
    });
    let lib = found.unwrap_or_else(|| fail(&format!("no {stem} library in {}", dir.display())));
    println!("cargo:rustc-link-search=native={}", dir.display());
    println!("cargo:rustc-link-lib=static={lib}");
}

/// The checked local copy of `s`: from `MK_NATIVE_SRC_DIR` if set, else from the download cache,
/// downloading it there first if needed.
fn fetch(s: &Source) -> PathBuf {
    if let Some(dir) = env::var_os("MK_NATIVE_SRC_DIR") {
        let path = PathBuf::from(dir).join(&s.file);
        if !path.is_file() {
            fail(&format!(
                "MK_NATIVE_SRC_DIR is set, but {} is not there. Download it from {}",
                path.display(),
                s.url
            ));
        }
        check(&path, s);
        return path;
    }

    let cache = cache_dir();
    let path = cache.join(&s.file);
    if path.is_file() && sha256(&path).is_ok_and(|h| h == s.sha256) {
        return path;
    }
    fs::create_dir_all(&cache).unwrap();
    // A per-process name, so parallel builds never see a half-written file.
    let partial = cache.join(format!("{}.{}.partial", s.file, std::process::id()));
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&partial)
        .arg(&s.url)
        .status();
    match status {
        Ok(st) if st.success() => {}
        Ok(st) => fail(&format!("curl could not download {} ({st})", s.url)),
        Err(e) => fail(&format!(
            "could not run curl ({e}). Install curl, or set MK_NATIVE_SRC_DIR to a folder \
             with the files in crates/mk-tesseract-sys/native/sources.toml"
        )),
    }
    check(&partial, s);
    fs::rename(&partial, &path).unwrap();
    path
}

fn check(path: &Path, s: &Source) {
    let got = sha256(path).unwrap_or_else(|e| fail(&format!("{}: {e}", path.display())));
    if got != s.sha256 {
        fail(&format!(
            "{} has the wrong SHA-256.\n  expected {}\n  got      {got}\n\
             The file may be damaged or changed upstream. Delete it and build again; if it \
             still fails, do not use it.",
            path.display(),
            s.sha256
        ));
    }
}

fn sha256(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `target/mk-native-cache/`: shared by every profile, so a source is downloaded once per target
/// directory. Cargo marks the target directory root with `CACHEDIR.TAG`.
fn cache_dir() -> PathBuf {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let target = out_dir
        .ancestors()
        .find(|p| p.join("CACHEDIR.TAG").is_file())
        .unwrap_or_else(|| out_dir.ancestors().nth(4).unwrap_or(&out_dir));
    target.join("mk-native-cache")
}

/// Extracts the zip at `archive` into `dest`, without its top-level folder. Skipped when `dest`
/// already holds this exact archive.
fn extract(archive: &Path, dest: &Path) -> PathBuf {
    let marker = dest.join(".mk-source-sha256");
    let hash = sha256(archive).unwrap();
    if fs::read_to_string(&marker).ok().as_deref() == Some(hash.as_str()) {
        return dest.to_path_buf();
    }
    if dest.exists() {
        fs::remove_dir_all(dest).unwrap();
    }
    let mut zip = zip::ZipArchive::new(fs::File::open(archive).unwrap())
        .unwrap_or_else(|e| fail(&format!("{}: {e}", archive.display())));
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        let Some(name) = entry.enclosed_name() else {
            fail(&format!("{}: unsafe path in archive", archive.display()));
        };
        let rel: PathBuf = name.components().skip(1).collect();
        if rel.as_os_str().is_empty() {
            continue;
        }
        let path = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&path).unwrap();
        } else {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            io::copy(&mut entry, &mut fs::File::create(&path).unwrap()).unwrap();
        }
    }
    fs::write(&marker, hash).unwrap();
    dest.to_path_buf()
}

fn fail(msg: &str) -> ! {
    eprintln!("error: mk-tesseract-sys: {msg}");
    std::process::exit(1);
}
