//! `cargo xtask check-layers` (ADR-0002 rule 3, ADR-0009, M2 acceptance criterion 6).
//!
//! A second check next to `cargo deny check bans`: it walks the whole resolved graph, so it also
//! catches a banned crate pulled in transitively, which `wrappers` in `deny.toml` does not.
//!
//! - Only `mangkon-desktop` may reach a Tauri crate.
//! - An HTTP client crate may be reached only through `mk-egress` or `mk-models`.

use std::collections::{BTreeSet, HashMap, VecDeque};

use serde::Deserialize;

/// The one package allowed to reach Tauri.
pub const TAURI_HOST: &str = "mangkon-desktop";
/// The only packages allowed to reach an HTTP client (ADR-0009).
pub const HTTP_WRAPPERS: &[&str] = &["mk-egress", "mk-models"];
/// HTTP client crates. Kept in step with `[bans]` in `deny.toml`.
pub const HTTP_CLIENTS: &[&str] = &[
    "reqwest",
    "ureq",
    "hyper",
    "hyper-util",
    "isahc",
    "attohttpc",
    "curl",
    "surf",
];
/// Desktop targets the app ships for. Mobile targets are not built (Tauri pulls `reqwest` there).
pub const TARGETS: &[&str] = &[
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
];

pub fn is_tauri(name: &str) -> bool {
    name == "tauri" || name.starts_with("tauri-")
}

#[derive(Deserialize)]
pub struct Metadata {
    pub packages: Vec<Package>,
    pub workspace_members: Vec<String>,
    pub resolve: Resolve,
}

#[derive(Deserialize)]
pub struct Package {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
}

#[derive(Deserialize)]
pub struct Resolve {
    pub nodes: Vec<Node>,
}

#[derive(Deserialize)]
pub struct Node {
    pub id: String,
    pub deps: Vec<Dep>,
}

#[derive(Deserialize)]
pub struct Dep {
    pub pkg: String,
    pub dep_kinds: Vec<DepKind>,
}

#[derive(Deserialize)]
pub struct DepKind {
    /// `null` for normal, `"dev"` or `"build"`.
    pub kind: Option<String>,
}

/// Dependency ids by package id.
type Edges<'a> = HashMap<&'a str, Vec<&'a str>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kinds {
    /// Normal and build dependencies: what a package needs to build and run.
    Build,
    /// Normal only: what ends up in the shipped binary.
    Runtime,
}

impl Metadata {
    pub fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.packages
            .iter()
            .find(|p| p.id == id)
            .map_or(id, |p| p.name.as_str())
    }

    /// Edges by package id, filtered by kind. A workspace member's dev-dependencies count only
    /// when it is the start of a walk, as Cargo builds them only for that package's own tests.
    /// Returns (normal, dev) edges.
    fn edges(&self, kinds: Kinds) -> (Edges<'_>, Edges<'_>) {
        let mut normal = HashMap::new();
        let mut dev = HashMap::new();
        for node in &self.resolve.nodes {
            let (mut n, mut d) = (Vec::new(), Vec::new());
            for dep in &node.deps {
                let has = |k: Option<&str>| dep.dep_kinds.iter().any(|x| x.kind.as_deref() == k);
                let runtime = has(None);
                let build = kinds == Kinds::Build && has(Some("build"));
                if runtime || build {
                    n.push(dep.pkg.as_str());
                } else if has(Some("dev")) {
                    d.push(dep.pkg.as_str());
                }
            }
            normal.insert(node.id.as_str(), n);
            dev.insert(node.id.as_str(), d);
        }
        (normal, dev)
    }

    /// Ids reachable from `start`, not walking into packages for which `stop` is true. Each with
    /// the path that reaches it first.
    pub fn reach<'a>(
        &'a self,
        start: &'a str,
        kinds: Kinds,
        with_dev: bool,
        stop: impl Fn(&str) -> bool,
    ) -> Vec<(&'a str, Vec<&'a str>)> {
        let (normal, dev) = self.edges(kinds);
        let mut parent: HashMap<&str, &str> = HashMap::new();
        let mut queue = VecDeque::from([start]);
        let mut found = Vec::new();
        while let Some(id) = queue.pop_front() {
            let mut next: Vec<&str> = normal.get(id).cloned().unwrap_or_default();
            if with_dev && id == start {
                next.extend(dev.get(id).cloned().unwrap_or_default());
            }
            for dep in next {
                if dep == start || parent.contains_key(dep) {
                    continue;
                }
                parent.insert(dep, id);
                let mut path = vec![dep];
                let mut at = dep;
                while let Some(&p) = parent.get(at) {
                    path.push(p);
                    at = p;
                }
                path.reverse();
                found.push((dep, path));
                if !stop(self.name(dep)) {
                    queue.push_back(dep);
                }
            }
        }
        found
    }
}

/// Every rule break in `meta`, as `"a -> b -> c: reason"` lines.
pub fn violations(meta: &Metadata) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for member in &meta.workspace_members {
        let name = meta.name(member);
        let show = |path: &[&str]| {
            path.iter()
                .map(|id| meta.name(id))
                .collect::<Vec<_>>()
                .join(" -> ")
        };
        if name != TAURI_HOST {
            // Stop at the first Tauri crate on each path: one line per way in.
            for (dep, path) in meta.reach(member, Kinds::Build, true, is_tauri) {
                if is_tauri(meta.name(dep)) {
                    out.insert(format!(
                        "{}: only {TAURI_HOST} may depend on Tauri",
                        show(&path)
                    ));
                }
            }
        }
        if !HTTP_WRAPPERS.contains(&name) {
            let stop = |n: &str| HTTP_WRAPPERS.contains(&n);
            for (dep, path) in meta.reach(member, Kinds::Build, true, stop) {
                if HTTP_CLIENTS.contains(&meta.name(dep)) {
                    out.insert(format!(
                        "{}: HTTP clients only through {} (ADR-0009)",
                        show(&path),
                        HTTP_WRAPPERS.join(" or ")
                    ));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny resolved graph: `(name, [(dep, kind)])`, kind `""`, `"dev"` or `"build"`.
    fn meta(members: &[&str], graph: &[(&str, &[(&str, &str)])]) -> Metadata {
        let json = serde_json::json!({
            "packages": graph.iter().map(|(n, _)| serde_json::json!({"id": n, "name": n})).collect::<Vec<_>>(),
            "workspace_members": members,
            "resolve": {"nodes": graph.iter().map(|(n, deps)| serde_json::json!({
                "id": n,
                "deps": deps.iter().map(|(d, k)| serde_json::json!({
                    "pkg": d,
                    "dep_kinds": [{"kind": if k.is_empty() { None } else { Some(*k) }}],
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>()},
        });
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn m2_ac6_clean_graph_passes() {
        let m = meta(
            &["mangkon-desktop", "mk-core", "mk-models", "mk-kernel"],
            &[
                (
                    "mangkon-desktop",
                    &[("tauri", ""), ("mk-core", ""), ("tauri-build", "build")],
                ),
                ("mk-core", &[("mk-models", ""), ("mk-kernel", "")]),
                ("mk-models", &[("reqwest", "")]),
                ("mk-kernel", &[]),
                ("tauri", &[("tauri-runtime", "")]),
                ("tauri-runtime", &[]),
                ("tauri-build", &[]),
                ("reqwest", &[("hyper", "")]),
                ("hyper", &[]),
            ],
        );
        assert_eq!(violations(&m), BTreeSet::new());
    }

    #[test]
    fn m2_ac6_tauri_in_mk_core_fails() {
        let m = meta(
            &["mangkon-desktop", "mk-core"],
            &[
                ("mangkon-desktop", &[("mk-core", "")]),
                ("mk-core", &[("tauri", "")]),
                ("tauri", &[]),
            ],
        );
        let v = violations(&m);
        assert!(
            v.contains("mk-core -> tauri: only mangkon-desktop may depend on Tauri"),
            "{v:?}"
        );
        // The desktop reaches Tauri through mk-core; that is allowed for the desktop itself.
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn m2_ac6_transitive_http_client_fails() {
        let m = meta(
            &["mk-kernel", "mk-bench"],
            &[
                ("mk-kernel", &[("some-sdk", "")]),
                ("some-sdk", &[("ureq", "")]),
                ("ureq", &[]),
                ("mk-bench", &[("reqwest", "dev")]),
                ("reqwest", &[]),
            ],
        );
        let v = violations(&m);
        assert!(
            v.iter()
                .any(|l| l.starts_with("mk-kernel -> some-sdk -> ureq:")),
            "{v:?}"
        );
        assert!(
            v.iter().any(|l| l.starts_with("mk-bench -> reqwest:")),
            "{v:?}"
        );
    }

    #[test]
    fn m2_ac6_dev_dependency_of_a_dependency_is_not_followed() {
        let m = meta(
            &["mk-core", "mk-parsers"],
            &[
                ("mk-core", &[("mk-parsers", "")]),
                ("mk-parsers", &[("tauri", "dev")]),
                ("tauri", &[]),
            ],
        );
        let v = violations(&m);
        assert_eq!(v.len(), 1, "{v:?}");
        assert!(v.iter().all(|l| l.starts_with("mk-parsers -> tauri")));
    }
}
