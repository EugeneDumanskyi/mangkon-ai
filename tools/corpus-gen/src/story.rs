//! The story as data: cast, identifiers and negatives. Every file and the ground truth come
//! from here, so the ground truth is correct by construction. Narrative: `docs/07-test-corpus.md` §2.

use std::collections::BTreeMap;

use mk_bench::{EntityKind, IdType, IdentifierKey, checks};
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::SeedableRng;

use crate::ids;

#[derive(Clone, Copy)]
pub struct EntityDef {
    pub id: &'static str,
    pub kind: EntityKind,
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    pub role: &'static str,
}

pub struct IdentDef {
    pub key: IdentifierKey,
    /// Surface forms by name. The first one is the default.
    pub forms: Vec<(&'static str, String)>,
}

impl IdentDef {
    pub fn form(&self, name: Option<&str>) -> &str {
        match name {
            None => &self.forms[0].1,
            Some(n) => {
                &self
                    .forms
                    .iter()
                    .find(|(f, _)| *f == n)
                    .unwrap_or_else(|| panic!("no form {n} for {:?}", self.key))
                    .1
            }
        }
    }
}

pub enum NegDef {
    BadChecksum { claimed: IdType, raw: String },
    CardLike { raw: String },
    StopList { domain: String },
}

impl NegDef {
    pub fn raw(&self) -> &str {
        match self {
            NegDef::BadChecksum { raw, .. } | NegDef::CardLike { raw } => raw,
            NegDef::StopList { domain } => domain,
        }
    }
}

pub struct Story {
    pub entities: Vec<EntityDef>,
    pub idents: BTreeMap<&'static str, IdentDef>,
    pub negs: BTreeMap<&'static str, NegDef>,
    /// Pairs of entities with lookalike names that must not merge.
    pub distinct: Vec<(&'static str, &'static str, &'static str, &'static str)>,
}

use EntityKind::{Org, Person};

const ENTITIES: &[EntityDef] = &[
    EntityDef {
        id: "daniel",
        kind: Person,
        canonical: "Daniel Okonkwo-Hart",
        aliases: &[
            "D. Okonkwo-Hart",
            "Mr Okonkwo-Hart",
            "OKONKWO-HART, Daniel",
            "Daniel Okonkwo Hart",
            "Okonkwo-Hart",
        ],
        role: "Director of Procurement, Velmora Port Authority. Steers two tenders to Brightwater.",
    },
    EntityDef {
        id: "irene",
        kind: Person,
        canonical: "Irene Vasquez-Lindqvist",
        aliases: &[
            "I. Vasquez-Lindqvist",
            "Ms Vasquez-Lindqvist",
            "Irene Vasquez",
            "VASQUEZ-LINDQVIST, Irene",
            "Irene",
        ],
        role: "Sole director of Brightwater Logistics Ltd. Daniel's sister-in-law.",
    },
    EntityDef {
        id: "tomas",
        kind: Person,
        canonical: "Tomas Reinholt",
        aliases: &["T. Reinholt", "Mr Reinholt", "REINHOLT, Tomas"],
        role: "Accountant; managing partner of Kestrel Advisory LLP; company-formation agent.",
    },
    EntityDef {
        id: "corvin",
        kind: Person,
        canonical: "Corvin Hale",
        aliases: &["C. Hale", "Mr Hale"],
        role: "Nominee shareholder and director of Saltmarsh Holdings S.A.",
    },
    EntityDef {
        id: "priya",
        kind: Person,
        canonical: "Priya Ramanathan-Cole",
        aliases: &["P. Ramanathan-Cole", "Priya"],
        role: "Accounts-payable clerk at the Port Authority. Spots the pattern and reports it.",
    },
    EntityDef {
        id: "rosalind",
        kind: Person,
        canonical: "Rosalind Achebe-Frost",
        aliases: &["R. Achebe-Frost", "Ms Achebe-Frost"],
        role: "Chief Internal Auditor, Velmora Port Authority.",
    },
    EntityDef {
        id: "lena",
        kind: Person,
        canonical: "Lena Hartwig",
        aliases: &["L. Hartwig"],
        role: "Reporter at The Harbour Post.",
    },
    EntityDef {
        id: "samuel",
        kind: Person,
        canonical: "Samuel Achterberg",
        aliases: &["S. Achterberg"],
        role: "Compliance officer, Meridian Trust Bank.",
    },
    EntityDef {
        id: "gregor",
        kind: Person,
        canonical: "Gregor Almqvist",
        aliases: &["G. Almqvist", "Mr Almqvist"],
        role: "Director of Northgate Marine Supplies, the honest bidder who lost.",
    },
    EntityDef {
        id: "gregory",
        kind: Person,
        canonical: "Gregory Almquist",
        aliases: &[],
        role: "Dock workers' union representative quoted in the press. Lookalike of Gregor Almqvist; unrelated.",
    },
    EntityDef {
        id: "vpa",
        kind: Org,
        canonical: "Velmora Port Authority",
        aliases: &["VPA", "Port of Velmora", "VELMORA PORT AUTHORITY"],
        role: "Public body that runs the port and the tenders.",
    },
    EntityDef {
        id: "bwl",
        kind: Org,
        canonical: "Brightwater Logistics Ltd",
        aliases: &[
            "Brightwater Logistics Limited",
            "BRIGHTWATER LOGISTICS LTD",
            "Brightwater Logistics",
            "Brightwater",
        ],
        role: "Shell company that wins the tenders and invoices the Port Authority.",
    },
    EntityDef {
        id: "kestrel",
        kind: Org,
        canonical: "Kestrel Advisory LLP",
        aliases: &["Kestrel Advisory", "KESTREL ADVISORY LLP", "Kestrel"],
        role: "Advisory firm that formed Brightwater and moves money onward.",
    },
    EntityDef {
        id: "saltmarsh",
        kind: Org,
        canonical: "Saltmarsh Holdings S.A.",
        aliases: &["Saltmarsh Holdings", "SALTMARSH HOLDINGS S.A.", "Saltmarsh"],
        role: "Offshore parent of Brightwater; end point of the money.",
    },
    EntityDef {
        id: "northgate",
        kind: Org,
        canonical: "Northgate Marine Supplies Inc.",
        aliases: &[
            "Northgate Marine Supplies",
            "Northgate Marine",
            "NORTHGATE MARINE SUPPLIES INC.",
        ],
        role: "Honest supplier; background noise with its own identifiers.",
    },
    EntityDef {
        id: "meridian",
        kind: Org,
        canonical: "Meridian Trust Bank",
        aliases: &["Meridian Trust", "MERIDIAN TRUST BANK"],
        role: "Bank holding the Brightwater and Port Authority accounts.",
    },
    EntityDef {
        id: "harbourpost",
        kind: Org,
        canonical: "The Harbour Post",
        aliases: &["Harbour Post"],
        role: "Local newspaper.",
    },
    EntityDef {
        id: "registry",
        kind: Org,
        canonical: "Velmora Companies Registry",
        aliases: &["Companies Registry"],
        role: "Public company register that holds the incorporation records.",
    },
    EntityDef {
        id: "union",
        kind: Org,
        canonical: "Velmora Dock Workers Union",
        aliases: &["Dock Workers Union"],
        role: "Trade union; Gregory Almquist speaks for it.",
    },
    EntityDef {
        id: "exchange",
        kind: Org,
        canonical: "Coralline Exchange",
        aliases: &["Coralline"],
        role: "Crypto OTC desk that sells Bitcoin and USDT to Saltmarsh.",
    },
    EntityDef {
        id: "bwl_gmbh",
        kind: Org,
        canonical: "Brightwater Logistic Solutions GmbH",
        aliases: &[],
        role: "Unrelated firm with a lookalike name. Must not merge with Brightwater Logistics Ltd.",
    },
];

fn key(id_type: IdType, value: impl Into<String>) -> IdentifierKey {
    IdentifierKey {
        id_type,
        value: value.into(),
        pattern: None,
    }
}

impl Story {
    pub fn new(seed: u64) -> Self {
        let mut rng = ChaCha20Rng::seed_from_u64(seed);
        let mut idents = BTreeMap::new();
        let mut add = |id: &'static str, k: IdentifierKey, forms: Vec<(&'static str, String)>| {
            assert!(
                idents.insert(id, IdentDef { key: k, forms }).is_none(),
                "duplicate ident {id}"
            );
        };

        // Emails: RFC 2606 `.example` domains only.
        for (id, v) in [
            ("em_daniel", "daniel.okonkwo-hart@velmora-port.example"),
            ("em_daniel_priv", "d.oh.private@harbourmail.example"),
            ("em_irene", "irene@brightwater-logistics.example"),
            ("em_bwl_accounts", "accounts@brightwater-logistics.example"),
            ("em_tomas", "tomas.reinholt@kestrel-advisory.example"),
            ("em_corvin", "corvin.hale@saltmarsh-holdings.example"),
            ("em_priya", "priya.ramanathan-cole@velmora-port.example"),
            ("em_audit", "audit@velmora-port.example"),
            ("em_ap", "payables@velmora-port.example"),
            ("em_lena", "lena.hartwig@harbourpost.example"),
            ("em_samuel", "s.achterberg@meridian-trust.example"),
            ("em_gregor", "g.almqvist@northgate-marine.example"),
        ] {
            let mut forms = vec![("lower", v.to_string())];
            if id == "em_daniel" {
                forms.push(("mixed", "Daniel.Okonkwo-Hart@Velmora-Port.example".into()));
            }
            if id == "em_irene" {
                forms.push(("mixed", "Irene@Brightwater-Logistics.example".into()));
            }
            add(id, key(IdType::Email, v), forms);
        }

        // Phones: Ofcom drama ranges (020 7946 0xxx, 07700 900xxx), US 555-01xx, ACMA 0491 570 xxx.
        for (id, e164, intl, national) in [
            (
                "ph_daniel_mob",
                "+447700900461",
                "+44 7700 900461",
                "07700 900461",
            ),
            (
                "ph_bwl_office",
                "+442079460318",
                "+44 20 7946 0318",
                "020 7946 0318",
            ),
            (
                "ph_vpa",
                "+442079460102",
                "+44 20 7946 0102",
                "020 7946 0102",
            ),
            (
                "ph_meridian",
                "+442079460555",
                "+44 20 7946 0555",
                "020 7946 0555",
            ),
            (
                "ph_kestrel",
                "+12025550147",
                "+1 202-555-0147",
                "(202) 555-0147",
            ),
            (
                "ph_gregor",
                "+12125550183",
                "+1 212-555-0183",
                "(212) 555-0183",
            ),
            (
                "ph_saltmarsh",
                "+61491570156",
                "+61 491 570 156",
                "0491 570 156",
            ),
            (
                "ph_lena",
                "+447700900877",
                "+44 7700 900877",
                "07700 900877",
            ),
        ] {
            add(
                id,
                key(IdType::Phone, e164),
                vec![
                    ("intl", intl.into()),
                    ("national", national.into()),
                    ("e164", e164.into()),
                ],
            );
        }

        for (id, v) in [
            (
                "url_bwl",
                "https://www.brightwater-logistics.example/services",
            ),
            ("url_kestrel", "https://kestrel-advisory.example/team"),
            (
                "url_saltmarsh",
                "https://saltmarsh-holdings.example/contact",
            ),
            (
                "url_article",
                "https://harbourpost.example/2025/05/port-contracts",
            ),
            (
                "url_registry",
                "https://registry.velmora.example/companies/VC-0418822",
            ),
        ] {
            add(id, key(IdType::Url, v), vec![("plain", v.into())]);
        }
        for (id, v) in [
            ("dom_bwl", "brightwater-logistics.example"),
            ("dom_kestrel", "kestrel-advisory.example"),
            ("dom_saltmarsh", "saltmarsh-holdings.example"),
            ("dom_harbourmail", "harbourmail.example"),
        ] {
            add(id, key(IdType::Domain, v), vec![("plain", v.into())]);
        }

        // IBANs: valid check digits, made-up bank codes (Q…). ASSUMPTION: no real account.
        for (id, country, bban) in [
            ("iban_bwl", "GB", "QMTB60991248213377"),
            ("iban_vpa", "GB", "QMTB60991200102938"),
            ("iban_gregor", "GB", "QNMS40112255443322"),
            ("iban_kestrel", "NL", "QKES0417339821"),
            ("iban_saltmarsh", "MT", "QSAL12345000000000012345678"),
        ] {
            let v = ids::iban(country, bban);
            add(
                id,
                key(IdType::Iban, v.clone()),
                vec![("spaced", ids::iban_spaced(&v)), ("compact", v)],
            );
        }

        let btc_daniel = ids::btc_p2pkh(ids::bytes(&mut rng));
        let btc_saltmarsh = ids::btc_bech32(ids::bytes(&mut rng));
        let evm_daniel = ids::evm(ids::bytes(&mut rng));
        let tron_saltmarsh = ids::tron(ids::bytes(&mut rng));
        let sol_corvin = ids::sol(ids::bytes(&mut rng));
        let eip55 = checks::eip55(&evm_daniel[2..]);
        add(
            "btc_daniel",
            key(IdType::Btc, btc_daniel.clone()),
            vec![("plain", btc_daniel.clone())],
        );
        add(
            "btc_saltmarsh",
            key(IdType::Btc, btc_saltmarsh.clone()),
            vec![("plain", btc_saltmarsh.clone())],
        );
        add(
            "evm_daniel",
            key(IdType::Evm, evm_daniel.clone()),
            vec![("lower", evm_daniel), ("eip55", eip55.clone())],
        );
        add(
            "tron_saltmarsh",
            key(IdType::Tron, tron_saltmarsh.clone()),
            vec![("plain", tron_saltmarsh)],
        );
        add(
            "sol_corvin",
            key(IdType::Sol, sol_corvin.clone()),
            vec![("plain", sol_corvin)],
        );

        for (id, v) in [
            ("tnd_031", "VPA-TND-2024-031"),
            ("tnd_007", "VPA-TND-2025-007"),
        ] {
            let mut k = key(IdType::Custom, v);
            k.pattern = Some("tender_ref".into());
            add(id, k, vec![("plain", v.into())]);
        }

        let mut negs = BTreeMap::new();
        let bwl_iban = idents["iban_bwl"].key.value.clone();
        let bad_iban = ids::corrupt(&bwl_iban, "0123456789", checks::iban_valid);
        negs.insert(
            "neg_iban_typo",
            NegDef::BadChecksum {
                claimed: IdType::Iban,
                raw: ids::iban_spaced(&bad_iban),
            },
        );
        negs.insert(
            "neg_btc_typo",
            NegDef::BadChecksum {
                claimed: IdType::Btc,
                raw: ids::corrupt(&btc_daniel, ids::BASE58, checks::btc_valid),
            },
        );
        // EIP-55 with one letter's case flipped: mixed case, wrong checksum (US-04.5 AC2).
        let flipped = {
            let mut c: Vec<char> = eip55.chars().collect();
            let pos = (2..c.len())
                .rev()
                .find(|&i| c[i].is_ascii_alphabetic())
                .expect("a letter");
            c[pos] = if c[pos].is_ascii_uppercase() {
                c[pos].to_ascii_lowercase()
            } else {
                c[pos].to_ascii_uppercase()
            };
            c.into_iter().collect::<String>()
        };
        negs.insert(
            "neg_evm_case",
            NegDef::BadChecksum {
                claimed: IdType::Evm,
                raw: flipped,
            },
        );
        // Public test-card numbers only.
        negs.insert(
            "neg_card_visa",
            NegDef::CardLike {
                raw: "4111 1111 1111 1111".into(),
            },
        );
        negs.insert(
            "neg_card_mc",
            NegDef::CardLike {
                raw: "5555555555554444".into(),
            },
        );
        negs.insert(
            "neg_stop_gmail",
            NegDef::StopList {
                domain: "gmail.com".into(),
            },
        );
        negs.insert(
            "neg_stop_outlook",
            NegDef::StopList {
                domain: "outlook.com".into(),
            },
        );

        Story {
            entities: ENTITIES.to_vec(),
            idents,
            negs,
            distinct: vec![
                (
                    "neg_lookalike_gregor",
                    "gregor",
                    "gregory",
                    "Similar names, different people: supplier director vs union representative.",
                ),
                (
                    "neg_lookalike_bwl",
                    "bwl",
                    "bwl_gmbh",
                    "Similar names, different companies: the shell vs an unrelated German firm.",
                ),
            ],
        }
    }

    pub fn entity(&self, id: &str) -> &EntityDef {
        self.entities
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("unknown entity {id}"))
    }

    pub fn ident(&self, id: &str) -> &IdentDef {
        self.idents
            .get(id)
            .unwrap_or_else(|| panic!("unknown identifier {id}"))
    }

    /// Every surface form the corpus can print, with what it belongs to. Used to find
    /// unmarked occurrences in generated text.
    pub fn surface_forms(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for e in &self.entities {
            out.push((e.canonical.to_string(), format!("entity {}", e.id)));
            for a in e.aliases {
                out.push((a.to_string(), format!("entity {}", e.id)));
            }
        }
        for (id, d) in &self.idents {
            for (_, raw) in &d.forms {
                out.push((raw.clone(), format!("identifier {id}")));
            }
            if matches!(d.key.id_type, IdType::Email | IdType::Url) {
                // Domains are derived; their bare form must also be marked when printed alone.
                let domain = domain_of(&d.key);
                out.push((domain, format!("domain of {id}")));
            }
        }
        for (id, n) in &self.negs {
            out.push((n.raw().to_string(), format!("negative {id}")));
        }
        out
    }
}

/// The registrable domain of an email or URL key (`www.` removed, US-04.3).
/// Public suffixes with two labels that occur in the corpus. A stand-in for the Public Suffix
/// List that US-04.3 names (ASSUMPTION there); enough for the hosts in TD-1.
const TWO_LABEL_SUFFIXES: &[&str] = &["gc.ca", "co.uk", "org.uk", "gov.uk", "com.au"];

/// The derived domain of an email or URL key: the registrable domain of the host (US-04.3).
pub fn domain_of(k: &IdentifierKey) -> String {
    let v = &k.value;
    let host = match k.id_type {
        IdType::Email => v.rsplit('@').next().unwrap_or(v),
        IdType::Url => v
            .split("://")
            .nth(1)
            .unwrap_or(v)
            .split(['/', '?', '#', ':'])
            .next()
            .unwrap_or(""),
        _ => v.as_str(),
    }
    .to_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let n = labels.len();
    let keep = if n >= 3 && TWO_LABEL_SUFFIXES.contains(&labels[n - 2..].join(".").as_str()) {
        3
    } else {
        2
    };
    labels[n.saturating_sub(keep)..].join(".")
}
