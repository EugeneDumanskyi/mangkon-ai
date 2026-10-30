//! Project names and the app's project registry (domain §5.2, INV-WS-01, US-01.1, US-01.2).

use std::fmt;

/// Longest project name, in characters.
pub const MAX_NAME: usize = 100;

/// Why a project name was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NameError {
    Empty,
    TooLong,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NameError::Empty => f.write_str("Enter a name for the project"),
            NameError::TooLong => write!(f, "A project name has at most {MAX_NAME} characters"),
        }
    }
}

impl std::error::Error for NameError {}

/// A checked project name: trimmed, 1–100 characters (US-01.1 AC2).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProjectName(String);

impl ProjectName {
    pub fn parse(name: &str) -> Result<Self, NameError> {
        let name = name.trim();
        match name.chars().count() {
            0 => Err(NameError::Empty),
            n if n > MAX_NAME => Err(NameError::TooLong),
            _ => Ok(Self(name.to_string())),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The name as the registry compares it: names are unique ignoring case (INV-WS-01).
    pub fn key(&self) -> String {
        name_key(&self.0)
    }

    /// The project's folder name under the chosen location (`<location>/<name>`, ASSUMPTION
    /// M3c): characters no OS allows in a file name (`/\\:*?"<>|`) and control characters become
    /// `_`; a leading dot (a hidden folder) and trailing dots and spaces (refused on Windows)
    /// too.
    pub fn folder_name(&self) -> String {
        let mut out: String = self
            .0
            .chars()
            .map(|c| {
                if c.is_control() || "/\\:*?\"<>|".contains(c) {
                    '_'
                } else {
                    c
                }
            })
            .collect();
        if out.starts_with('.') {
            out.replace_range(..1, "_");
        }
        let kept = out.trim_end_matches(['.', ' ']).len();
        if kept < out.len() {
            out.truncate(kept);
            out.push('_');
        }
        out
    }
}

impl fmt::Display for ProjectName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The case-folded form of a name, as stored in `registry_entry.name_key`.
pub fn name_key(name: &str) -> String {
    name.trim().to_lowercase()
}

/// One project the app knows (domain §5.2 `REGISTRY_ENTRY`).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RegistryEntry {
    pub id: i64,
    pub name: String,
    /// The project folder, absolute.
    pub path: String,
    /// Milliseconds since the Unix epoch, UTC.
    pub last_opened_at: i64,
}

/// Whether `name` clashes with an entry other than the one at `except_path` (INV-WS-01).
pub fn name_taken(
    entries: &[RegistryEntry],
    name: &ProjectName,
    except_path: Option<&str>,
) -> bool {
    let key = name.key();
    entries
        .iter()
        .any(|e| name_key(&e.name) == key && Some(e.path.as_str()) != except_path)
}

/// The recent list order: last opened first, then by name (US-01.2 AC1).
pub fn newest_first(entries: &mut [RegistryEntry]) {
    entries.sort_by(|a, b| {
        b.last_opened_at
            .cmp(&a.last_opened_at)
            .then_with(|| name_key(&a.name).cmp(&name_key(&b.name)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64, name: &str, path: &str, at: i64) -> RegistryEntry {
        RegistryEntry {
            id,
            name: name.into(),
            path: path.into(),
            last_opened_at: at,
        }
    }

    /// US-01.1 AC2: a name is required, trimmed, and at most 100 characters.
    #[test]
    fn us_01_1_ac2_name_rules() {
        assert_eq!(ProjectName::parse("  "), Err(NameError::Empty));
        assert_eq!(ProjectName::parse(" Acme ").unwrap().as_str(), "Acme");
        assert!(ProjectName::parse(&"x".repeat(100)).is_ok());
        assert_eq!(
            ProjectName::parse(&"é".repeat(101)),
            Err(NameError::TooLong)
        );
    }

    /// INV-WS-01: names clash ignoring case; an entry does not clash with itself.
    #[test]
    fn inv_ws_01_names_unique_ignoring_case() {
        let entries = [entry(1, "Acme", "/p/Acme", 1)];
        let acme = ProjectName::parse("ACME").unwrap();
        assert!(name_taken(&entries, &acme, None));
        assert!(!name_taken(&entries, &acme, Some("/p/Acme")));
        assert!(!name_taken(
            &entries,
            &ProjectName::parse("Acme 2").unwrap(),
            None
        ));
        assert_eq!(ProjectName::parse("Ärger").unwrap().key(), "ärger");
    }

    #[test]
    fn folder_names_are_safe_on_every_os() {
        let f = |n: &str| ProjectName::parse(n).unwrap().folder_name();
        assert_eq!(f("Acme"), "Acme");
        assert_eq!(f("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(f(".hidden"), "_hidden");
        assert_eq!(f("tab\there"), "tab_here");
        assert_eq!(f("ends."), "ends_");
        assert_eq!(f("ends.. ."), "ends_");
        assert_eq!(f("Case 2026"), "Case 2026");
    }

    /// US-01.2 AC1: newest first.
    #[test]
    fn us_01_2_ac1_newest_first() {
        let mut e = vec![
            entry(1, "b", "/b", 10),
            entry(2, "a", "/a", 30),
            entry(3, "C", "/c", 10),
        ];
        newest_first(&mut e);
        let ids: Vec<i64> = e.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![2, 1, 3]);
    }
}
