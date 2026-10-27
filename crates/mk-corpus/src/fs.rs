//! File system side of re-scan: walk source roots and hash files. The only I/O in this crate;
//! the diff itself is pure ([`crate::rescan`]). Reads only, never writes (INV-CO-01).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use mk_kernel::ContentHash;

use crate::rescan::{FileMeta, RootId, WalkedEntry};

/// Which files have a parser, by extension (US-01.3 AC3).
#[derive(Clone, Debug)]
pub struct FileTypes {
    extensions: Vec<String>,
}

impl FileTypes {
    pub fn new<S: AsRef<str>>(extensions: impl IntoIterator<Item = S>) -> Self {
        Self {
            extensions: extensions
                .into_iter()
                .map(|e| e.as_ref().to_ascii_lowercase())
                .collect(),
        }
    }

    pub fn supports(&self, path: &Path) -> bool {
        path.extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| self.extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
    }
}

/// A file that is not stored, and why (INV-CO-11: reported, not stored).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Skipped {
    pub rel_path: String,
    pub reason: &'static str,
}

/// Result of walking one source root.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Walk {
    pub entries: Vec<WalkedEntry>,
    /// Unsupported types and names that cannot be stored. Hidden and system files are left out
    /// silently.
    pub skipped: Vec<Skipped>,
    /// Folders or files that could not be read, with the error.
    pub errors: Vec<(String, String)>,
}

/// Hidden and system files never become file references (INV-CO-11): dot-files and
/// dot-folders, `Thumbs.db`, `desktop.ini`.
pub fn is_hidden_or_system(name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("thumbs.db")
        || name.eq_ignore_ascii_case("desktop.ini")
}

/// The file a stored reference points to: the root itself for a single-file root (empty
/// `rel_path`), otherwise the root joined with the `/`-separated relative path.
pub fn resolve(root: &Path, rel_path: &str) -> PathBuf {
    if rel_path.is_empty() {
        return root.to_path_buf();
    }
    rel_path
        .split('/')
        .fold(root.to_path_buf(), |p, c| p.join(c))
}

/// Size and modified time of a file.
pub fn meta(md: &std::fs::Metadata) -> FileMeta {
    let mtime_ns = md
        .modified()
        .ok()
        .map(|t| match t.duration_since(UNIX_EPOCH) {
            Ok(d) => i64::try_from(d.as_nanos()).unwrap_or(i64::MAX),
            Err(e) => -i64::try_from(e.duration().as_nanos()).unwrap_or(i64::MAX),
        })
        .unwrap_or(0);
    FileMeta {
        size: md.len(),
        mtime_ns,
    }
}

/// Walks a source root: a folder (recursively) or a single file. Symbolic links are not
/// followed.
pub fn walk(root_id: RootId, root: &Path, types: &FileTypes) -> Walk {
    let mut out = Walk::default();
    let md = match std::fs::metadata(root) {
        Ok(md) => md,
        Err(e) => {
            out.errors.push((String::new(), e.to_string()));
            return out;
        }
    };
    if md.is_file() {
        if types.supports(root) {
            out.entries.push(WalkedEntry {
                root: root_id,
                rel_path: String::new(),
                meta: meta(&md),
            });
        } else {
            out.skipped.push(Skipped {
                rel_path: String::new(),
                reason: "unsupported file type",
            });
        }
        return out;
    }
    let it = walkdir::WalkDir::new(root)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_hidden_or_system(&e.file_name().to_string_lossy()));
    for entry in it {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                let at = e
                    .path()
                    .and_then(|p| p.strip_prefix(root).ok())
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.errors.push((at, e.to_string()));
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(root)
            .expect("walk stays under its root");
        let Some(rel_path) = rel_path_of(rel) else {
            out.skipped.push(Skipped {
                rel_path: rel.to_string_lossy().into_owned(),
                reason: "file name is not valid Unicode",
            });
            continue;
        };
        if !types.supports(rel) {
            out.skipped.push(Skipped {
                rel_path,
                reason: "unsupported file type",
            });
            continue;
        }
        match entry.metadata() {
            Ok(md) => out.entries.push(WalkedEntry {
                root: root_id,
                rel_path,
                meta: meta(&md),
            }),
            Err(e) => out.errors.push((rel_path, e.to_string())),
        }
    }
    out
}

/// `/`-separated relative path, the same on every OS (NFR-COMP-07).
fn rel_path_of(rel: &Path) -> Option<String> {
    let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
    Some(parts?.join("/"))
}

/// BLAKE3 of a file, read in 1 MiB chunks.
pub fn hash_file(path: &Path) -> std::io::Result<ContentHash> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = ContentHash::hasher();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hasher.finish());
        }
        hasher.update(&buf[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn types() -> FileTypes {
        FileTypes::new(["txt", "md", "csv"])
    }

    /// US-01.3 AC3, INV-CO-11: supported files are walked with `/` paths; unsupported ones are
    /// reported; hidden and system files and folders are left out silently.
    #[test]
    fn inv_co_11_walk_skips_hidden_and_reports_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        std::fs::create_dir_all(p.join("sub/deeper")).unwrap();
        std::fs::create_dir_all(p.join(".git")).unwrap();
        for f in [
            "a.txt",
            "B.CSV",
            "sub/notes.md",
            "sub/deeper/scan.pdf",
            ".hidden.txt",
            ".git/config.txt",
            "Thumbs.db",
            "sub/desktop.ini",
        ] {
            std::fs::write(p.join(f), b"x").unwrap();
        }
        let w = walk(7, p, &types());
        let rels: Vec<&str> = w.entries.iter().map(|e| e.rel_path.as_str()).collect();
        assert_eq!(rels, vec!["B.CSV", "a.txt", "sub/notes.md"]);
        assert!(w.entries.iter().all(|e| e.root == 7 && e.meta.size == 1));
        assert_eq!(
            w.skipped,
            vec![Skipped {
                rel_path: "sub/deeper/scan.pdf".into(),
                reason: "unsupported file type"
            }]
        );
        assert!(w.errors.is_empty());
    }

    #[test]
    fn single_file_root_and_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("one.txt");
        std::fs::write(&f, b"hello").unwrap();
        let w = walk(1, &f, &types());
        assert_eq!(w.entries.len(), 1);
        assert_eq!(w.entries[0].rel_path, "");
        assert_eq!(resolve(&f, ""), f);
        assert_eq!(
            resolve(dir.path(), "sub/deeper/x.txt"),
            dir.path().join("sub").join("deeper").join("x.txt")
        );
        let gone = walk(1, &dir.path().join("nope"), &types());
        assert_eq!(gone.errors.len(), 1);
    }

    #[test]
    fn hash_file_matches_hash_of_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("big.txt");
        let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&f, &bytes).unwrap();
        assert_eq!(hash_file(&f).unwrap(), ContentHash::of(&bytes));
    }
}
