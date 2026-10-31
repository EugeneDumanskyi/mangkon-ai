//! Suggestions (US-05.1 AC2). Identifier types and dates come from the values, person and
//! organization from the header words. Header words are generic, not those of any one file.
//!
//! A header word that can name either ("name", "holder", "shareholder", "agent" …) leaves it to
//! the values: half or more with a legal suffix ("Ltd", "LLP", "S.A." …) make an organization,
//! else a person (ASSUMPTION M4c, refining AC2). For a JSON key, the key above it may decide
//! first (`/officers/*/name` is a person).

use mk_identifiers::{IdentifierHit, Rules, find_all};
use mk_kernel::{IdentifierKind, NameKey};

use crate::{ColumnType, SUGGEST_SHARE, is_date};

/// A suggested type and the share of the sample that supports it (0–1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Suggestion {
    pub column_type: ColumnType,
    pub share: f32,
}

impl Suggestion {
    fn ignore() -> Self {
        Suggestion {
            column_type: ColumnType::Ignore,
            share: 0.0,
        }
    }
}

const PERSON_WORDS: &[&str] = &[
    "person",
    "people",
    "employee",
    "employees",
    "staff",
    "officer",
    "officers",
    "director",
    "directors",
    "approver",
    "approved",
    "signatory",
    "signatories",
    "signer",
    "author",
    "authors",
    "surname",
    "forename",
    "firstname",
    "lastname",
    "fullname",
    "passenger",
    "patient",
    "witness",
    "applicant",
    "manager",
    "individual",
];

const ORG_WORDS: &[&str] = &[
    "company",
    "companies",
    "organisation",
    "organization",
    "org",
    "employer",
    "supplier",
    "suppliers",
    "vendor",
    "vendors",
    "client",
    "clients",
    "customer",
    "customers",
    "counterparty",
    "bank",
    "exchange",
    "merchant",
    "firm",
    "business",
    "agency",
    "institution",
    "issuer",
    "contractor",
    "provider",
    "manufacturer",
    "carrier",
    "insurer",
    "lender",
    "registrar",
    "corporation",
];

/// Words that name either; the values (or the key above) decide.
const EITHER_WORDS: &[&str] = &[
    "name",
    "holder",
    "holders",
    "shareholder",
    "shareholders",
    "owner",
    "owners",
    "agent",
    "presenter",
    "beneficiary",
    "payee",
    "payer",
    "party",
    "member",
    "members",
    "recipient",
    "sender",
];

/// Words before "name" that make it a person's name.
const PERSON_NAME_PARTS: &[&str] = &["full", "first", "last", "given", "family", "middle", "sur"];

/// Trailing words that say nothing ("approved_by" reads as "approved").
const FILLERS: &[&str] = &["by", "of", "to", "from"];

/// The lower-case words of a header: split at anything not a letter or digit and at
/// lower-to-upper case changes (`CompanyName` → `company`, `name`).
pub fn header_words(header: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut prev_lower = false;
    for c in header.chars() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum HeaderKind {
    Person,
    Org,
    Either,
    Neither,
}

fn word_kind(words: &[String]) -> HeaderKind {
    let mut words: Vec<&str> = words.iter().map(String::as_str).collect();
    while words.len() > 1 && FILLERS.contains(words.last().unwrap()) {
        words.pop();
    }
    let Some(&last) = words.last() else {
        return HeaderKind::Neither;
    };
    let is = |list: &[&str], w: &str| list.contains(&w);
    if is(PERSON_WORDS, last) {
        return HeaderKind::Person;
    }
    if is(ORG_WORDS, last) {
        return HeaderKind::Org;
    }
    if !is(EITHER_WORDS, last) {
        return HeaderKind::Neither;
    }
    // "supplier_name", "full_name", "account_holder": the words before decide, if they can.
    for w in words[..words.len() - 1].iter().rev() {
        if last == "name" && is(PERSON_NAME_PARTS, w) || is(PERSON_WORDS, w) {
            return HeaderKind::Person;
        }
        if is(ORG_WORDS, w) {
            return HeaderKind::Org;
        }
    }
    HeaderKind::Either
}

/// The column type a value is valid for as a whole, through the built-in finders: one hit that
/// covers the whole trimmed value. A card-like number is `None` (never an identifier, INV-ID-06).
fn value_type(value: &str, rules: &Rules) -> Option<ColumnType> {
    let found = find_all(value, rules);
    let len = value.chars().count();
    if !found.cards.is_empty() {
        return None;
    }
    let whole = |h: &&IdentifierHit| h.span.start() == 0 && h.span.end() == len;
    let hit = found.hits.iter().find(whole)?;
    Some(match hit.key.kind() {
        IdentifierKind::Email => ColumnType::Email,
        IdentifierKind::Phone | IdentifierKind::PhoneUnnormalized => ColumnType::Phone,
        IdentifierKind::Url | IdentifierKind::Domain => ColumnType::UrlDomain,
        IdentifierKind::Iban => ColumnType::Iban,
        IdentifierKind::Btc | IdentifierKind::Evm | IdentifierKind::Sol | IdentifierKind::Tron => {
            ColumnType::Crypto
        }
        IdentifierKind::Custom => return None,
    })
}

/// Suggests a type for one column from its header and a sample of its non-empty values
/// (US-05.1 AC2). `parent` is the key above a JSON key (`officers` for `/officers/*/name`),
/// `None` for a CSV column. Identifier types are read with the project's `rules` (its phone
/// region).
pub fn suggest(header: &str, parent: Option<&str>, sample: &[&str], rules: &Rules) -> Suggestion {
    let values: Vec<&str> = sample
        .iter()
        .map(|v| v.trim())
        .filter(|v| !v.is_empty())
        .collect();
    if values.is_empty() {
        return Suggestion::ignore();
    }
    let n = values.len() as f32;
    let share = |count: usize| count as f32 / n;

    let mut counts = [0usize; ColumnType::ALL.len()];
    for v in &values {
        if let Some(t) = value_type(v, rules) {
            counts[t as usize] += 1;
        }
    }
    if let Some((i, &c)) = counts
        .iter()
        .enumerate()
        .max_by_key(|&(i, c)| (*c, std::cmp::Reverse(i)))
        && share(c) >= SUGGEST_SHARE
    {
        return Suggestion {
            column_type: ColumnType::ALL[i],
            share: share(c),
        };
    }
    let dates = values.iter().filter(|v| is_date(v)).count();
    if share(dates) >= SUGGEST_SHARE {
        return Suggestion {
            column_type: ColumnType::Date,
            share: share(dates),
        };
    }

    // Names have letters; a column of codes or amounts under a name-like header is not one.
    let wordy = values
        .iter()
        .filter(|v| v.chars().any(char::is_alphabetic) && value_type(v, rules).is_none())
        .count();
    if share(wordy) < SUGGEST_SHARE {
        return Suggestion::ignore();
    }
    let mut kind = word_kind(&header_words(header));
    if kind == HeaderKind::Either
        && let Some(p) = parent
    {
        match word_kind(&header_words(p)) {
            HeaderKind::Person => kind = HeaderKind::Person,
            HeaderKind::Org => kind = HeaderKind::Org,
            _ => {}
        }
    }
    let orgs = share(
        values
            .iter()
            .filter(|v| NameKey::has_legal_suffix(v))
            .count(),
    );
    let (column_type, share) = match kind {
        HeaderKind::Person => (ColumnType::PersonName, share(wordy)),
        HeaderKind::Org => (ColumnType::OrgName, share(wordy)),
        HeaderKind::Either if orgs >= 0.5 => (ColumnType::OrgName, orgs),
        HeaderKind::Either => (ColumnType::PersonName, 1.0 - orgs),
        HeaderKind::Neither => return Suggestion::ignore(),
    };
    Suggestion { column_type, share }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(header: &str, parent: Option<&str>, sample: &[&str]) -> ColumnType {
        suggest(header, parent, sample, &Rules::default()).column_type
    }

    #[test]
    fn words_of_headers() {
        assert_eq!(header_words("CompanyName"), vec!["company", "name"]);
        assert_eq!(header_words("approved_by"), vec!["approved", "by"]);
        assert_eq!(
            header_words("Counterparty account"),
            vec!["counterparty", "account"]
        );
        assert_eq!(header_words("Bedrag €"), vec!["bedrag"]);
        assert_eq!(header_words("IBAN"), vec!["iban"]);
    }

    /// US-05.1 AC2: identifier types and dates by an 80% share of the values.
    #[test]
    fn us_05_1_ac2_identifier_columns_by_share() {
        let mails = [
            "a@x.example",
            "b@y.example",
            "c@z.example",
            "d@w.example",
            "oops",
        ];
        assert_eq!(t("contact", None, &mails), ColumnType::Email);
        let s = suggest("contact", None, &mails, &Rules::default());
        assert!((s.share - 0.8).abs() < 1e-6);
        let mostly_not = ["a@x.example", "b@y.example", "x", "y", "z"];
        assert_eq!(t("contact", None, &mostly_not), ColumnType::Ignore);
        assert_eq!(
            t(
                "acct",
                None,
                &["GB33 QNMS 4011 2255 4433 22", "GB86QMTB60991248213377"]
            ),
            ColumnType::Iban
        );
        assert_eq!(
            t(
                "wallet",
                None,
                &[
                    "bc1qs95x428wtu729ffv6cfm4um8yxjvl5lgaf9xsc",
                    "TH2fjNgATy9Hi1e4NtJHzukziwPxX2Vb9r"
                ]
            ),
            ColumnType::Crypto
        );
        assert_eq!(
            t("tel", None, &["+442079460318", "+61491570156"]),
            ColumnType::Phone
        );
        assert_eq!(
            t(
                "site",
                None,
                &["https://www.a.example/x", "https://b.example"]
            ),
            ColumnType::UrlDomain
        );
        assert_eq!(
            t("Datum", None, &["15-08-2024", "16-09-2024"]),
            ColumnType::Date
        );
        assert_eq!(t("when", None, &["2025-02-19T10:15:00Z"]), ColumnType::Date);
        // Empty cells are not in the sample; a column with none is ignored.
        assert_eq!(t("x", None, &["", " "]), ColumnType::Ignore);
        // A card-like column is never an identifier column.
        assert_eq!(
            t(
                "card_number",
                None,
                &["5555555555554444", "4111111111111111"]
            ),
            ColumnType::Ignore
        );
    }

    /// US-05.1 AC2: person and organization from header words, on headers of any file.
    #[test]
    fn us_05_1_ac2_names_from_header_words() {
        let people = ["Ann Lee", "Bo Chen"];
        for h in [
            "employee",
            "full_name",
            "FirstName",
            "approved_by",
            "Director",
            "signatory",
        ] {
            assert_eq!(t(h, None, &people), ColumnType::PersonName, "{h}");
        }
        let orgs = ["Acme", "Globex"];
        for h in [
            "Company",
            "CompanyName",
            "employer",
            "supplier",
            "Vendor Name",
            "Client",
        ] {
            assert_eq!(t(h, None, &orgs), ColumnType::OrgName, "{h}");
        }
        for h in [
            "description",
            "status",
            "role",
            "amount_gbp",
            "claim_id",
            "notes",
        ] {
            assert_eq!(t(h, None, &people), ColumnType::Ignore, "{h}");
        }
        // Codes and amounts under a name-like header are not names.
        assert_eq!(t("customer", None, &["10021", "10022"]), ColumnType::Ignore);
    }

    /// ASSUMPTION (M4c): a header that names either is decided by the key above, then by the
    /// values' legal suffixes.
    #[test]
    fn either_words_decided_by_parent_or_values() {
        assert_eq!(
            t("name", None, &["BRIGHTWATER LOGISTICS LTD"]),
            ColumnType::OrgName
        );
        assert_eq!(t("name", None, &["Corvin Hale"]), ColumnType::PersonName);
        assert_eq!(
            t("name", Some("officers"), &["Acme Ltd"]),
            ColumnType::PersonName
        );
        assert_eq!(
            t("name", Some("shareholders"), &["C. Hale"]),
            ColumnType::PersonName
        );
        assert_eq!(
            t("holder", Some("account"), &["SALTMARSH HOLDINGS S.A."]),
            ColumnType::OrgName
        );
        assert_eq!(
            t("registered_agent", None, &["KESTREL ADVISORY LLP"]),
            ColumnType::OrgName
        );
        assert_eq!(t("supplier_name", None, &["Acme"]), ColumnType::OrgName);
    }
}
