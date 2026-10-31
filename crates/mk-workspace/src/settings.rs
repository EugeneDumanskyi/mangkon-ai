//! Project settings (US-01.6, INV-WS-05/06/07).
//!
//! M3c: OCR languages. M4b: the phone region (identifier extraction) and the domain stop list
//! (connections). M4c: custom patterns (US-04.7). Model endpoints come in M5.

use std::fmt;

/// The OCR language of a new project (INV-WS-05).
pub const DEFAULT_OCR_LANGUAGE: &str = "eng";

/// The standard domain stop list of a new project (INV-WS-05, US-04.3 AC3): big free-mail and
/// platform domains, whose shared domain says nothing about a link. Registrable domains, sorted.
/// ASSUMPTION (M4b): these 50.
pub const DEFAULT_STOP_LIST: &[&str] = &[
    "126.com",
    "163.com",
    "amazon.com",
    "aol.com",
    "apple.com",
    "bing.com",
    "dropbox.com",
    "facebook.com",
    "fastmail.com",
    "github.com",
    "gmail.com",
    "gmx.com",
    "gmx.de",
    "gmx.net",
    "google.com",
    "googlemail.com",
    "hotmail.co.uk",
    "hotmail.com",
    "icloud.com",
    "instagram.com",
    "linkedin.com",
    "live.com",
    "mac.com",
    "mail.com",
    "mail.ru",
    "me.com",
    "microsoft.com",
    "msn.com",
    "naver.com",
    "office.com",
    "outlook.com",
    "pm.me",
    "proton.me",
    "protonmail.com",
    "qq.com",
    "rambler.ru",
    "t.me",
    "tiktok.com",
    "tutanota.com",
    "twitter.com",
    "web.de",
    "whatsapp.com",
    "x.com",
    "yahoo.co.uk",
    "yahoo.com",
    "yandex.com",
    "yandex.ru",
    "youtube.com",
    "zoho.com",
    "zoom.us",
];

/// A project's one settings set (INV-WS-05).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProjectSettings {
    /// ISO 3166-1 alpha-2 region for phone numbers without a country code, such as `DE`;
    /// `None` reads only numbers with one.
    pub phone_region: Option<String>,
    /// Tesseract language codes, at least one (INV-WS-06).
    pub ocr_languages: Vec<String>,
    /// Lowercase domains, sorted, no duplicates.
    pub domain_stop_list: Vec<String>,
    /// Custom patterns in the order they were added (US-04.7); names unique ignoring case
    /// (INV-ID-07).
    pub custom_patterns: Vec<PatternSetting>,
}

/// A custom pattern as the project stores it: its name and expression (US-04.7).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PatternSetting {
    pub name: String,
    pub expression: String,
}

impl PatternSetting {
    pub fn new(name: impl Into<String>, expression: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            expression: expression.into(),
        }
    }
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            phone_region: None,
            ocr_languages: vec![DEFAULT_OCR_LANGUAGE.to_string()],
            domain_stop_list: DEFAULT_STOP_LIST.iter().map(|d| d.to_string()).collect(),
            custom_patterns: Vec::new(),
        }
    }
}

/// Why settings were refused, in plain words.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SettingsError {
    NoOcrLanguage,
    BadOcrLanguage(String),
    BadPhoneRegion(String),
    BadDomain(String),
    /// A pattern that does not compile, with its name and the plain reason (US-04.7 AC2).
    BadPattern(String, String),
    /// Another pattern has this name, ignoring case (INV-ID-07).
    DuplicatePattern(String),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::NoOcrLanguage => f.write_str("Pick at least one OCR language"),
            SettingsError::BadOcrLanguage(c) => write!(f, "Not an OCR language code: {c}"),
            SettingsError::BadPhoneRegion(r) => {
                write!(f, "Not a region code: {r} (use two letters, such as DE)")
            }
            SettingsError::BadDomain(d) => write!(f, "Not a domain: {d}"),
            SettingsError::BadPattern(_, reason) => f.write_str(reason),
            SettingsError::DuplicatePattern(n) => {
                write!(f, "A pattern named \"{n}\" already exists")
            }
        }
    }
}

impl std::error::Error for SettingsError {}

/// What a settings change needs re-done (INV-WS-07). Empty when nothing already read changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Reprocess {
    /// Stage A again for contents with OCR'd pages and for contents that failed for a missing
    /// OCR language.
    pub ocr: bool,
    /// Identifiers found again from the stored text of every content (the phone region
    /// changed, US-04.2 AC2). No file is read again.
    pub identifiers: bool,
    /// The custom patterns changed: only their step runs again, on the stored text (US-04.7
    /// AC3, INV-ID-08). It starts at once, without asking.
    pub custom: bool,
}

impl Reprocess {
    pub fn is_empty(&self) -> bool {
        !self.ocr && !self.identifiers && !self.custom
    }
}

/// `eng`, `chi_sim`, `Latin`: what Tesseract names its language files (the same rule as
/// `mk_parsers::ocr::valid_language`).
fn valid_language(code: &str) -> bool {
    !code.is_empty()
        && code.len() <= 32
        && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn valid_domain(d: &str) -> bool {
    let labels: Vec<&str> = d.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
}

impl ProjectSettings {
    /// The settings checked and in their stored form: languages in order without duplicates,
    /// the region uppercase, domains lowercase, sorted and without duplicates.
    pub fn normalized(self) -> Result<Self, SettingsError> {
        let mut ocr_languages: Vec<String> = Vec::new();
        for code in self.ocr_languages {
            let code = code.trim().to_string();
            if !valid_language(&code) {
                return Err(SettingsError::BadOcrLanguage(code));
            }
            if !ocr_languages.contains(&code) {
                ocr_languages.push(code);
            }
        }
        if ocr_languages.is_empty() {
            return Err(SettingsError::NoOcrLanguage);
        }
        let phone_region = match self.phone_region.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(r) if r.len() == 2 && r.bytes().all(|b| b.is_ascii_alphabetic()) => {
                Some(r.to_ascii_uppercase())
            }
            Some(r) => return Err(SettingsError::BadPhoneRegion(r.to_string())),
        };
        let mut domain_stop_list = Vec::new();
        for d in self.domain_stop_list {
            let d = d.trim().trim_start_matches('@').to_lowercase();
            if d.is_empty() {
                continue;
            }
            if !valid_domain(&d) {
                return Err(SettingsError::BadDomain(d));
            }
            domain_stop_list.push(d);
        }
        domain_stop_list.sort();
        domain_stop_list.dedup();
        let mut custom_patterns: Vec<PatternSetting> = Vec::new();
        for p in self.custom_patterns {
            let name = p.name.trim().to_string();
            if let Err(e) = mk_identifiers::CustomPattern::compile(&name, &p.expression) {
                return Err(SettingsError::BadPattern(name, e.to_string()));
            }
            if custom_patterns
                .iter()
                .any(|q| q.name.to_lowercase() == name.to_lowercase())
            {
                return Err(SettingsError::DuplicatePattern(name));
            }
            custom_patterns.push(PatternSetting {
                name,
                expression: p.expression,
            });
        }
        Ok(Self {
            phone_region,
            ocr_languages,
            domain_stop_list,
            custom_patterns,
        })
    }

    /// The OCR languages as Tesseract takes them, `+`-joined.
    pub fn ocr_languages_joined(&self) -> String {
        self.ocr_languages.join("+")
    }

    /// What changing from `old` to `new` needs re-done (INV-WS-07). A new OCR language set
    /// re-reads the files that were OCR'd or failed for a missing language; a new phone region
    /// finds identifiers again in the stored text; new custom patterns run only their own step
    /// again (INV-ID-08). The stop list needs nothing re-done: it applies to connections when
    /// they are derived (INV-LK-03), right away.
    pub fn affected_by(old: &ProjectSettings, new: &ProjectSettings) -> Reprocess {
        Reprocess {
            ocr: old.ocr_languages != new.ocr_languages,
            identifiers: old.phone_region != new.phone_region,
            custom: old.custom_patterns != new.custom_patterns,
        }
    }

    /// The custom patterns compiled, for the finders. The settings were checked
    /// ([`ProjectSettings::normalized`]); a pattern that no longer compiles is left out.
    pub fn compiled_patterns(&self) -> Vec<mk_identifiers::CustomPattern> {
        self.custom_patterns
            .iter()
            .filter_map(|p| mk_identifiers::CustomPattern::compile(&p.name, &p.expression).ok())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// INV-WS-05, US-01.6 AC2: no phone region, English OCR, the standard stop list.
    #[test]
    fn inv_ws_05_defaults() {
        let d = ProjectSettings::default();
        assert_eq!(d.phone_region, None);
        assert_eq!(d.ocr_languages, vec!["eng"]);
        assert!(d.domain_stop_list.contains(&"gmail.com".to_string()));
        assert_eq!(
            d.clone().normalized().unwrap(),
            d,
            "defaults are in stored form"
        );
    }

    /// INV-WS-06: at least one OCR language.
    #[test]
    fn inv_ws_06_at_least_one_ocr_language() {
        let s = ProjectSettings {
            ocr_languages: vec![],
            ..ProjectSettings::default()
        };
        assert_eq!(s.normalized(), Err(SettingsError::NoOcrLanguage));
        let s = ProjectSettings {
            ocr_languages: vec!["eng".into(), "de-u".into()],
            ..ProjectSettings::default()
        };
        assert_eq!(
            s.normalized(),
            Err(SettingsError::BadOcrLanguage("de-u".into()))
        );
    }

    #[test]
    fn normalized_form() {
        let s = ProjectSettings {
            phone_region: Some(" de ".into()),
            ocr_languages: vec!["eng".into(), "deu".into(), "eng".into()],
            domain_stop_list: vec!["@Gmail.com".into(), "".into(), "gmail.com".into()],
            custom_patterns: vec![],
        }
        .normalized()
        .unwrap();
        assert_eq!(s.phone_region.as_deref(), Some("DE"));
        assert_eq!(s.ocr_languages_joined(), "eng+deu");
        assert_eq!(s.domain_stop_list, vec!["gmail.com"]);
        let bad = |r: &str| {
            ProjectSettings {
                phone_region: Some(r.into()),
                ..ProjectSettings::default()
            }
            .normalized()
        };
        assert!(bad("DEU").is_err());
        assert!(bad("").unwrap().phone_region.is_none());
        let domain = |d: &str| {
            ProjectSettings {
                domain_stop_list: vec![d.into()],
                ..ProjectSettings::default()
            }
            .normalized()
        };
        assert!(domain("localhost").is_err());
        assert!(domain("a b.com").is_err());
        assert!(domain("münchen.de").is_ok());
    }

    /// INV-WS-07: OCR languages re-read OCR'd files, the region re-runs identifiers, the stop
    /// list re-runs nothing.
    #[test]
    fn inv_ws_07_only_dependent_steps() {
        let old = ProjectSettings::default();
        let stop = ProjectSettings {
            domain_stop_list: vec![],
            ..old.clone()
        };
        assert!(ProjectSettings::affected_by(&old, &stop).is_empty());
        let region = ProjectSettings {
            phone_region: Some("DE".into()),
            ..old.clone()
        };
        assert_eq!(
            ProjectSettings::affected_by(&old, &region),
            Reprocess {
                ocr: false,
                identifiers: true,
                custom: false,
            }
        );
        let ocr = ProjectSettings {
            ocr_languages: vec!["eng".into(), "deu".into()],
            ..old.clone()
        };
        assert_eq!(
            ProjectSettings::affected_by(&old, &ocr),
            Reprocess {
                ocr: true,
                identifiers: false,
                custom: false,
            }
        );
    }

    /// INV-ID-07: names unique ignoring case and trimmed; the expression must compile.
    /// INV-ID-08: a pattern change re-runs only the custom-pattern step.
    #[test]
    fn inv_id_07_unique_name_ignoring_case() {
        let with = |ps: Vec<PatternSetting>| {
            ProjectSettings {
                custom_patterns: ps,
                ..ProjectSettings::default()
            }
            .normalized()
        };
        let ok = with(vec![
            PatternSetting::new(" tender_ref ", r"VPA-TND-\d{4}-\d{3}"),
            PatternSetting::new("case", r"C-\d+"),
        ])
        .unwrap();
        assert_eq!(ok.custom_patterns[0].name, "tender_ref");
        assert_eq!(ok.compiled_patterns().len(), 2);
        assert_eq!(
            with(vec![
                PatternSetting::new("Case", "a"),
                PatternSetting::new("case", "b")
            ]),
            Err(SettingsError::DuplicatePattern("case".into()))
        );
        let bad = with(vec![PatternSetting::new("x", "a(?=b)")]).unwrap_err();
        assert!(matches!(bad, SettingsError::BadPattern(..)));
        assert!(bad.to_string().contains("not supported"), "{bad}");
        let old = ProjectSettings::default();
        assert_eq!(
            ProjectSettings::affected_by(&old, &ok),
            Reprocess {
                ocr: false,
                identifiers: false,
                custom: true,
            }
        );
    }

    /// US-04.3 AC3: about 50 domains, in stored form (sorted, lower case, valid).
    #[test]
    fn us_04_3_ac3_default_stop_list() {
        let d = ProjectSettings::default();
        assert!((45..=60).contains(&d.domain_stop_list.len()));
        for domain in ["gmail.com", "outlook.com", "yahoo.com", "google.com"] {
            assert!(d.domain_stop_list.contains(&domain.to_string()), "{domain}");
        }
    }
}
