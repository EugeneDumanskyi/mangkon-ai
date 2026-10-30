//! Project settings (US-01.6, INV-WS-05/06/07).
//!
//! M3c: OCR languages. M4b: the phone region (identifier extraction) and the domain stop list
//! (connections). Custom patterns and model endpoints come in M4c/M5.

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
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            phone_region: None,
            ocr_languages: vec![DEFAULT_OCR_LANGUAGE.to_string()],
            domain_stop_list: DEFAULT_STOP_LIST.iter().map(|d| d.to_string()).collect(),
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
}

impl Reprocess {
    pub fn is_empty(&self) -> bool {
        !self.ocr && !self.identifiers
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
        Ok(Self {
            phone_region,
            ocr_languages,
            domain_stop_list,
        })
    }

    /// The OCR languages as Tesseract takes them, `+`-joined.
    pub fn ocr_languages_joined(&self) -> String {
        self.ocr_languages.join("+")
    }

    /// What changing from `old` to `new` needs re-done (INV-WS-07). A new OCR language set
    /// re-reads the files that were OCR'd or failed for a missing language; a new phone region
    /// finds identifiers again in the stored text. The stop list needs nothing re-done: it
    /// applies to connections when they are derived (INV-LK-03), right away.
    pub fn affected_by(old: &ProjectSettings, new: &ProjectSettings) -> Reprocess {
        Reprocess {
            ocr: old.ocr_languages != new.ocr_languages,
            identifiers: old.phone_region != new.phone_region,
        }
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
                identifiers: true
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
                identifiers: false
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
