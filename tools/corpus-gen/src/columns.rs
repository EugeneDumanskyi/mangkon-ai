//! The column types of TD-1's CSV and JSON files, labelled by hand (NFR-QA-13, generator v6).
//! The mapping suggester never sees these: its header words stay generic. JSON columns are the
//! keys of string and number values; true/false and null values are not read as text.

use mk_bench::{ColumnTruth, GroundTruth};

const LABELS: &[(&str, &[(&str, &str)])] = &[
    (
        "synthetic/csv/kestrel-fee-ledger-2024.csv",
        &[
            ("Datum", "date"),
            ("Client", "org"),
            ("Omschrijving", "ignore"),
            ("Bedrag €", "ignore"),
        ],
    ),
    (
        "synthetic/csv/meridian-statement-bwl-2024.csv",
        &[
            ("Date", "date"),
            ("Description", "ignore"),
            ("Counterparty", "org"),
            ("Counterparty account", "iban"),
            ("Debit", "ignore"),
            ("Credit", "ignore"),
            ("Balance", "ignore"),
        ],
    ),
    (
        "synthetic/csv/vpa-expense-claims-2025.csv",
        &[
            ("claim_id", "ignore"),
            ("employee", "person"),
            ("date", "date"),
            ("description", "ignore"),
            ("amount_gbp", "ignore"),
            // Card-like numbers are never a column type (NFR-SEC-14).
            ("card_number", "ignore"),
        ],
    ),
    (
        "synthetic/csv/vpa-supplier-payments-2024.csv",
        &[
            ("payment_date", "date"),
            ("supplier", "org"),
            ("invoice", "ignore"),
            ("amount_gbp", "ignore"),
            ("supplier_iban", "iban"),
            ("approved_by", "person"),
        ],
    ),
    (
        "synthetic/json/coralline-account-export.json",
        &[
            ("/exchange", "org"),
            ("/account/holder", "org"),
            ("/account/contact", "email"),
            ("/account/tier", "ignore"),
            ("/transactions/*/id", "ignore"),
            ("/transactions/*/timestamp", "date"),
            ("/transactions/*/type", "ignore"),
            ("/transactions/*/asset", "ignore"),
            ("/transactions/*/network", "ignore"),
            ("/transactions/*/amount", "ignore"),
            ("/transactions/*/address", "crypto"),
        ],
    ),
    (
        "synthetic/json/registry-export-bwl.json",
        &[
            ("/company_number", "ignore"),
            ("/name", "org"),
            ("/incorporated", "date"),
            ("/status", "ignore"),
            ("/registered_office", "ignore"),
            ("/presenter", "org"),
            ("/officers/*/name", "person"),
            ("/officers/*/role", "ignore"),
            ("/officers/*/appointed", "date"),
            ("/shareholders/*/name", "org"),
            ("/shareholders/*/shares", "ignore"),
            ("/shareholders/*/percent", "ignore"),
            ("/contact/email", "email"),
            ("/contact/phone", "phone"),
            ("/contact/website", "url_domain"),
            ("/record_url", "url_domain"),
        ],
    ),
    (
        "synthetic/json/registry-export-saltmarsh.json",
        &[
            ("/company_number", "ignore"),
            ("/name", "org"),
            ("/incorporated", "date"),
            ("/registered_agent", "org"),
            ("/officers/*/name", "person"),
            ("/officers/*/role", "ignore"),
            ("/shareholders/*/name", "person"),
            ("/shareholders/*/percent", "ignore"),
            ("/contact/email", "email"),
            ("/contact/phone", "phone"),
            ("/contact/website", "url_domain"),
        ],
    ),
];

/// Puts the labels on the documents of those files; every labelled file must be there.
pub fn label(gt: &mut GroundTruth) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    for (file, columns) in LABELS {
        match gt.documents.iter_mut().find(|d| d.file == *file) {
            Some(d) => {
                d.columns = columns
                    .iter()
                    .map(|(k, t)| ColumnTruth {
                        column_key: (*k).to_string(),
                        column_type: (*t).to_string(),
                    })
                    .collect();
            }
            None => errors.push(format!("no document for {file}")),
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
