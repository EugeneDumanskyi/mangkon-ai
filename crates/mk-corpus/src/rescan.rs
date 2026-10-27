//! Re-scan diff (architecture §5.4, US-01.4): compare what is on disk with the stored file
//! references by size and modified time, hash only what is new or changed, then say what
//! happened to each path. Pure: the walk and the hashing are in [`crate::fs`].

use std::collections::{HashMap, HashSet};

use mk_kernel::ContentHash;

/// Id of a stored source root.
pub type RootId = i64;

/// What re-scan compares before hashing: size and modified time, nanoseconds since the Unix
/// epoch.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileMeta {
    pub size: u64,
    pub mtime_ns: i64,
}

/// A stored file reference (domain §5.1 `FILE_REFERENCE`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StoredRef {
    pub root: RootId,
    /// Inside its root, `/`-separated; empty for a single-file root.
    pub rel_path: String,
    pub meta: FileMeta,
    pub hash: ContentHash,
    pub present: bool,
}

/// A supported file found by the walk.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct WalkedEntry {
    pub root: RootId,
    pub rel_path: String,
    pub meta: FileMeta,
}

/// What a path needs before its outcome is known.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Planned {
    /// Present before and now, same size and time.
    Unchanged,
    /// New, or size or time changed, or a full re-scan: read and hash it.
    Hash,
    /// Was present, not found now.
    Missing,
    /// Was missing, found again with the same size and time.
    FoundAgain,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlanItem {
    pub root: RootId,
    pub rel_path: String,
    /// `None` for a path not found now.
    pub meta: Option<FileMeta>,
    /// The stored hash and presence, if the path was known.
    pub stored: Option<(ContentHash, bool)>,
    pub planned: Planned,
}

/// Every known or found path with what it needs. Paths that were already missing and are still
/// missing are left out.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct RescanPlan {
    pub items: Vec<PlanItem>,
}

/// What happened to one path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PathChange {
    /// A path not stored before (US-01.4 AC2).
    New {
        root: RootId,
        rel_path: String,
        meta: FileMeta,
        hash: ContentHash,
    },
    /// A known path now has other bytes: it points to a new content (US-01.4 AC3).
    Changed {
        root: RootId,
        rel_path: String,
        meta: FileMeta,
        old: ContentHash,
        new: ContentHash,
    },
    /// Same bytes, but size or time changed (for example a copy that kept the content). Only
    /// the stored size and time change; no work (INV-PR-08).
    Touched {
        root: RootId,
        rel_path: String,
        meta: FileMeta,
        hash: ContentHash,
    },
    /// The path is gone (US-01.4 AC4).
    Missing {
        root: RootId,
        rel_path: String,
        hash: ContentHash,
    },
    /// A missing path is back with the same bytes.
    FoundAgain {
        root: RootId,
        rel_path: String,
        meta: FileMeta,
        hash: ContentHash,
    },
}

impl PathChange {
    pub fn root(&self) -> RootId {
        match self {
            PathChange::New { root, .. }
            | PathChange::Changed { root, .. }
            | PathChange::Touched { root, .. }
            | PathChange::Missing { root, .. }
            | PathChange::FoundAgain { root, .. } => *root,
        }
    }

    pub fn rel_path(&self) -> &str {
        match self {
            PathChange::New { rel_path, .. }
            | PathChange::Changed { rel_path, .. }
            | PathChange::Touched { rel_path, .. }
            | PathChange::Missing { rel_path, .. }
            | PathChange::FoundAgain { rel_path, .. } => rel_path,
        }
    }
}

/// The outcome of a re-scan once hashes are known.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Resolved {
    pub changes: Vec<PathChange>,
    /// Paths with nothing to do.
    pub unchanged: usize,
}

/// Compares the walk with the stored references (architecture §5.4 steps 1–2). With `full`,
/// every found path is hashed, for when modified times lie.
///
/// `roots` are the roots that were walked: their stored paths that the walk did not find become
/// Missing. Stored paths of other roots are left out.
pub fn plan(
    stored: &[StoredRef],
    walked: &[WalkedEntry],
    roots: &[RootId],
    full: bool,
) -> RescanPlan {
    let by_path: HashMap<(RootId, &str), &StoredRef> = stored
        .iter()
        .map(|s| ((s.root, s.rel_path.as_str()), s))
        .collect();
    let mut seen: HashSet<(RootId, &str)> = HashSet::with_capacity(walked.len());
    let mut items = Vec::with_capacity(walked.len());
    for w in walked {
        seen.insert((w.root, w.rel_path.as_str()));
        let s = by_path.get(&(w.root, w.rel_path.as_str()));
        let planned = match s {
            None => Planned::Hash,
            Some(s) if full || s.meta != w.meta => Planned::Hash,
            Some(s) if s.present => Planned::Unchanged,
            Some(_) => Planned::FoundAgain,
        };
        items.push(PlanItem {
            root: w.root,
            rel_path: w.rel_path.clone(),
            meta: Some(w.meta),
            stored: s.map(|s| (s.hash, s.present)),
            planned,
        });
    }
    for s in stored {
        if s.present && roots.contains(&s.root) && !seen.contains(&(s.root, s.rel_path.as_str())) {
            items.push(PlanItem {
                root: s.root,
                rel_path: s.rel_path.clone(),
                meta: None,
                stored: Some((s.hash, true)),
                planned: Planned::Missing,
            });
        }
    }
    RescanPlan { items }
}

impl RescanPlan {
    /// Indexes of the items to hash, in order.
    pub fn to_hash(&self) -> impl Iterator<Item = (usize, &PlanItem)> {
        self.items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.planned == Planned::Hash)
    }

    /// Each path's outcome (architecture §5.4 step 3). `hashes` holds the hash of every item
    /// from [`to_hash`](Self::to_hash) that could be read, by index; an item missing from it
    /// (unreadable now) is left as it was.
    pub fn resolve(&self, hashes: &HashMap<usize, ContentHash>) -> Resolved {
        let mut out = Resolved::default();
        for (i, item) in self.items.iter().enumerate() {
            let (root, rel_path) = (item.root, item.rel_path.clone());
            let change = match (item.planned, item.stored, item.meta) {
                (Planned::Unchanged, ..) => None,
                (Planned::Missing, Some((hash, _)), _) => Some(PathChange::Missing {
                    root,
                    rel_path,
                    hash,
                }),
                (Planned::FoundAgain, Some((hash, _)), Some(meta)) => {
                    Some(PathChange::FoundAgain {
                        root,
                        rel_path,
                        meta,
                        hash,
                    })
                }
                (Planned::Hash, stored, Some(meta)) => match (hashes.get(&i), stored) {
                    (None, _) => None,
                    (Some(&hash), None) => Some(PathChange::New {
                        root,
                        rel_path,
                        meta,
                        hash,
                    }),
                    (Some(&new), Some((old, _))) if new != old => Some(PathChange::Changed {
                        root,
                        rel_path,
                        meta,
                        old,
                        new,
                    }),
                    (Some(&hash), Some((_, false))) => Some(PathChange::FoundAgain {
                        root,
                        rel_path,
                        meta,
                        hash,
                    }),
                    (Some(&hash), Some((_, true))) => Some(PathChange::Touched {
                        root,
                        rel_path,
                        meta,
                        hash,
                    }),
                },
                _ => None,
            };
            match change {
                Some(c) => out.changes.push(c),
                None => out.unchanged += 1,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: u8) -> ContentHash {
        ContentHash::from_bytes([b; 32])
    }

    fn meta(size: u64, mtime_ns: i64) -> FileMeta {
        FileMeta { size, mtime_ns }
    }

    fn stored(rel: &str, m: FileMeta, hash: ContentHash, present: bool) -> StoredRef {
        StoredRef {
            root: 1,
            rel_path: rel.into(),
            meta: m,
            hash,
            present,
        }
    }

    fn walked(rel: &str, m: FileMeta) -> WalkedEntry {
        WalkedEntry {
            root: 1,
            rel_path: rel.into(),
            meta: m,
        }
    }

    fn planned(p: &RescanPlan) -> Vec<(&str, Planned)> {
        p.items
            .iter()
            .map(|i| (i.rel_path.as_str(), i.planned))
            .collect()
    }

    /// NFR-PERF-33, INV-PR-08: nothing changed → nothing hashed, nothing to do.
    #[test]
    fn nfr_perf_33_unchanged_hashes_nothing() {
        let s = vec![
            stored("a.txt", meta(10, 1), h(1), true),
            stored("b.csv", meta(20, 2), h(2), true),
        ];
        let w = vec![walked("a.txt", meta(10, 1)), walked("b.csv", meta(20, 2))];
        let p = plan(&s, &w, &[1], false);
        assert_eq!(p.to_hash().count(), 0);
        let r = p.resolve(&HashMap::new());
        assert_eq!((r.changes.len(), r.unchanged), (0, 2));
    }

    /// Architecture §5.4 step 4: a full re-scan hashes everything; same bytes are still no work.
    #[test]
    fn full_rescan_hashes_everything() {
        let s = vec![stored("a.txt", meta(10, 1), h(1), true)];
        let p = plan(&s, &[walked("a.txt", meta(10, 1))], &[1], true);
        assert_eq!(p.to_hash().count(), 1);
        let r = p.resolve(&HashMap::from([(0, h(1))]));
        assert_eq!(
            r.changes,
            vec![PathChange::Touched {
                root: 1,
                rel_path: "a.txt".into(),
                meta: meta(10, 1),
                hash: h(1)
            }]
        );
    }

    /// US-01.4 AC2–AC4: new, changed, missing and found-again paths.
    #[test]
    fn us_01_4_new_changed_missing_found_again() {
        let s = vec![
            stored("same.txt", meta(1, 1), h(1), true),
            stored("edited.txt", meta(2, 2), h(2), true),
            stored("gone.txt", meta(3, 3), h(3), true),
            stored("back.txt", meta(4, 4), h(4), false),
            stored("still-gone.txt", meta(5, 5), h(5), false),
            stored("touched.txt", meta(6, 6), h(6), true),
        ];
        let w = vec![
            walked("same.txt", meta(1, 1)),
            walked("edited.txt", meta(2, 9)),
            walked("back.txt", meta(4, 4)),
            walked("touched.txt", meta(6, 7)),
            walked("new.csv", meta(8, 8)),
        ];
        let p = plan(&s, &w, &[1], false);
        assert_eq!(
            planned(&p),
            vec![
                ("same.txt", Planned::Unchanged),
                ("edited.txt", Planned::Hash),
                ("back.txt", Planned::FoundAgain),
                ("touched.txt", Planned::Hash),
                ("new.csv", Planned::Hash),
                ("gone.txt", Planned::Missing),
            ]
        );
        let hashes: HashMap<usize, ContentHash> = HashMap::from([(1, h(20)), (3, h(6)), (4, h(8))]);
        let r = p.resolve(&hashes);
        assert_eq!(r.unchanged, 1);
        assert_eq!(
            r.changes,
            vec![
                PathChange::Changed {
                    root: 1,
                    rel_path: "edited.txt".into(),
                    meta: meta(2, 9),
                    old: h(2),
                    new: h(20)
                },
                PathChange::FoundAgain {
                    root: 1,
                    rel_path: "back.txt".into(),
                    meta: meta(4, 4),
                    hash: h(4)
                },
                PathChange::Touched {
                    root: 1,
                    rel_path: "touched.txt".into(),
                    meta: meta(6, 7),
                    hash: h(6)
                },
                PathChange::New {
                    root: 1,
                    rel_path: "new.csv".into(),
                    meta: meta(8, 8),
                    hash: h(8)
                },
                PathChange::Missing {
                    root: 1,
                    rel_path: "gone.txt".into(),
                    hash: h(3)
                },
            ]
        );
    }

    /// A missing path found again with other bytes points to a new content.
    #[test]
    fn found_again_with_new_bytes_is_changed() {
        let s = vec![stored("x.txt", meta(1, 1), h(1), false)];
        let p = plan(&s, &[walked("x.txt", meta(2, 2))], &[1], false);
        let r = p.resolve(&HashMap::from([(0, h(2))]));
        assert!(
            matches!(&r.changes[..], [PathChange::Changed { old, new, .. }] if *old == h(1) && *new == h(2))
        );
    }

    /// INV-CO-02: two paths with the same bytes are two new paths with one hash; the store
    /// keeps one content for them.
    #[test]
    fn inv_co_02_duplicate_content_two_paths() {
        let w = vec![
            walked("a.txt", meta(5, 1)),
            walked("copy/a.txt", meta(5, 2)),
        ];
        let p = plan(&[], &w, &[1], false);
        let r = p.resolve(&HashMap::from([(0, h(7)), (1, h(7))]));
        let hashes: Vec<ContentHash> = r
            .changes
            .iter()
            .map(|c| match c {
                PathChange::New { hash, .. } => *hash,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(hashes, vec![h(7), h(7)]);
    }

    /// A path that could not be read now is left as it was.
    #[test]
    fn unreadable_path_is_left_alone() {
        let p = plan(&[], &[walked("locked.txt", meta(1, 1))], &[1], false);
        let r = p.resolve(&HashMap::new());
        assert_eq!((r.changes.len(), r.unchanged), (0, 1));
    }

    /// Only walked roots lose paths; a walked root with no files left loses all of them.
    #[test]
    fn only_walked_roots_lose_paths() {
        let mut other = stored("o.txt", meta(1, 1), h(1), true);
        other.root = 2;
        let p = plan(
            &[other.clone()],
            &[walked("a.txt", meta(1, 1))],
            &[1],
            false,
        );
        assert!(p.items.iter().all(|i| i.planned != Planned::Missing));
        let p = plan(&[other], &[], &[2], false);
        assert_eq!(planned(&p), vec![("o.txt", Planned::Missing)]);
    }
}
