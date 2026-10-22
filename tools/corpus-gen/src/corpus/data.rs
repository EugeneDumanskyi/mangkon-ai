//! Structured exports: CSV bank statements and ledgers, JSON registry and exchange exports.

use mk_bench::FileKind;

use super::stem;
use crate::formats::{Enc, J, csv, json};
use crate::rich::Rich;
use crate::story::Story;
use crate::truth::{Collector, FileMeta};

fn csv_file(
    c: &mut Collector,
    path: &str,
    rows: &[Vec<&str>],
    delim: char,
    enc: Enc,
    purpose: &str,
) {
    let s = c.story;
    let rows: Vec<Vec<Rich>> = rows
        .iter()
        .map(|r| r.iter().map(|cell| s.t(cell)).collect())
        .collect();
    let (text, placed) = csv(&rows, delim);
    let id = format!("d-{}", stem(path));
    c.write_file(
        path,
        &enc.encode(&text),
        FileMeta::new(FileKind::Csv, purpose).encoding(enc.name()),
    );
    c.whole(&id, path, FileKind::Csv);
    c.ingest(&id, placed);
}

fn json_file(c: &mut Collector, path: &str, root: &J, purpose: &str) {
    let (text, placed) = json(root);
    let id = format!("d-{}", stem(path));
    c.write_file(
        path,
        text.as_bytes(),
        FileMeta::new(FileKind::Json, purpose).encoding(Enc::Utf8.name()),
    );
    c.whole(&id, path, FileKind::Json);
    c.ingest(&id, placed);
}

pub fn build(c: &mut Collector) {
    csv_file(
        c,
        "synthetic/csv/meridian-statement-bwl-2024.csv",
        &[
            vec![
                "Date",
                "Description",
                "Counterparty",
                "Counterparty account",
                "Debit",
                "Credit",
                "Balance",
            ],
            vec![
                "2024-08-13",
                "FPS CREDIT INV BWL-2024-001",
                "{e:vpa|VELMORA PORT AUTHORITY}",
                "{i:iban_vpa|compact}",
                "",
                "148500.00",
                "151210.40",
            ],
            vec![
                "2024-08-15",
                "SEPA DEBIT FEE NOTE",
                "{e:kestrel|KESTREL ADVISORY LLP}",
                "{i:iban_kestrel|compact}",
                "141000.00",
                "",
                "10210.40",
            ],
            vec![
                "2024-09-12",
                "FPS CREDIT INV BWL-2024-002",
                "{e:vpa|VELMORA PORT AUTHORITY}",
                "{i:iban_vpa|compact}",
                "",
                "212750.00",
                "222960.40",
            ],
            vec![
                "2024-09-16",
                "SEPA DEBIT FEE NOTE",
                "{e:kestrel|KESTREL ADVISORY LLP}",
                "{i:iban_kestrel|compact}",
                "202000.00",
                "",
                "20960.40",
            ],
            vec!["2024-09-30", "ACCOUNT FEE", "", "", "25.00", "", "20935.40"],
            vec![
                "2024-10-14",
                "FPS CREDIT INV BWL-2024-003",
                "{e:vpa|VELMORA PORT AUTHORITY}",
                "{i:iban_vpa|compact}",
                "",
                "187300.00",
                "208235.40",
            ],
            vec![
                "2024-10-17",
                "SEPA DEBIT FEE NOTE",
                "{e:kestrel|KESTREL ADVISORY LLP}",
                "{i:iban_kestrel|compact}",
                "178000.00",
                "",
                "30235.40",
            ],
            vec![
                "2024-11-05",
                "FPS CREDIT INV BWL-2024-004",
                "{e:vpa|VELMORA PORT AUTHORITY}",
                "{i:iban_vpa|compact}",
                "",
                "256900.00",
                "287135.40",
            ],
            vec![
                "2024-11-08",
                "SEPA DEBIT FEE NOTE",
                "{e:kestrel|KESTREL ADVISORY LLP}",
                "{i:iban_kestrel|compact}",
                "212000.00",
                "",
                "75135.40",
            ],
        ],
        ',',
        Enc::Utf8,
        "Bank statement of the Brightwater account (CSV, compact IBANs)",
    );
    csv_file(
        c,
        "synthetic/csv/vpa-supplier-payments-2024.csv",
        &[
            vec![
                "payment_date",
                "supplier",
                "invoice",
                "amount_gbp",
                "supplier_iban",
                "approved_by",
            ],
            vec![
                "12.03.2024",
                "{e:northgate|Northgate Marine Supplies}",
                "NMS-24-0312",
                "14 650,00",
                "{i:iban_gregor}",
                "{e:priya|P. Ramanathan-Cole}",
            ],
            vec![
                "18.06.2024",
                "{e:northgate|Northgate Marine Supplies}",
                "NMS-24-0618",
                "9 980,00",
                "{i:iban_gregor}",
                "{e:priya|P. Ramanathan-Cole}",
            ],
            vec![
                "13.08.2024",
                "{e:bwl}",
                "BWL-2024-001",
                "148 500,00",
                "{i:iban_bwl}",
                "{e:daniel|D. Okonkwo-Hart}",
            ],
            vec![
                "12.09.2024",
                "{e:bwl}",
                "BWL-2024-002",
                "212 750,00",
                "{i:iban_bwl}",
                "{e:daniel|D. Okonkwo-Hart}",
            ],
            vec![
                "14.10.2024",
                "{e:bwl}",
                "BWL-2024-003",
                "187 300,00",
                "{i:iban_bwl}",
                "{e:daniel|D. Okonkwo-Hart}",
            ],
            vec![
                "15.10.2024",
                "{e:northgate|Northgate Marine Supplies}",
                "NMS-24-1009",
                "22 310,00",
                "{i:iban_gregor}",
                "{e:priya|P. Ramanathan-Cole}",
            ],
            vec![
                "05.11.2024",
                "{e:bwl}",
                "BWL-2024-004",
                "256 900,00",
                "{i:iban_bwl}",
                "{e:daniel|D. Okonkwo-Hart}",
            ],
            vec![
                "09.12.2024",
                "{e:bwl}",
                "BWL-2024-005",
                "231 400,00",
                "{i:iban_bwl}",
                "{e:daniel|D. Okonkwo-Hart}",
            ],
        ],
        ';',
        Enc::Utf8Bom,
        "Port Authority payments export (semicolon, UTF-8 BOM, decimal commas, spaced IBANs)",
    );
    csv_file(
        c,
        "synthetic/csv/kestrel-fee-ledger-2024.csv",
        &[
            vec!["Datum", "Client", "Omschrijving", "Bedrag €"],
            vec![
                "15-08-2024",
                "{e:bwl|Brightwater Logistics Limited}",
                "Advies – haven",
                "141.000,00",
            ],
            vec![
                "16-09-2024",
                "{e:bwl|Brightwater Logistics Limited}",
                "Advies – haven",
                "202.000,00",
            ],
            vec![
                "17-10-2024",
                "{e:bwl|Brightwater Logistics Limited}",
                "Advies – haven",
                "178.000,00",
            ],
            vec![
                "08-11-2024",
                "{e:bwl|Brightwater Logistics Limited}",
                "Advies – haven",
                "212.000,00",
            ],
            vec![
                "15-01-2025",
                "{e:saltmarsh|Saltmarsh Holdings}",
                "Retainer Q4 – uitbetaling",
                "-240.000,00",
            ],
        ],
        ',',
        Enc::Windows1252,
        "Kestrel fee ledger in Windows-1252 (euro sign, en dash) with Dutch headers",
    );
    csv_file(
        c,
        "synthetic/csv/vpa-expense-claims-2025.csv",
        &[
            vec![
                "claim_id",
                "employee",
                "date",
                "description",
                "amount_gbp",
                "card_number",
            ],
            vec![
                "EC-25-0041",
                "{e:daniel}",
                "2025-01-09",
                "Taxi to the marina",
                "38.50",
                "{n:neg_card_mc}",
            ],
            vec![
                "EC-25-0042",
                "{e:priya}",
                "2025-01-14",
                "Training course",
                "120.00",
                "",
            ],
            vec![
                "EC-25-0047",
                "{e:daniel}",
                "2025-02-02",
                "Dinner, \"supplier meeting\"",
                "212.40",
                "{n:neg_card_mc}",
            ],
        ],
        ',',
        Enc::Utf8,
        "Expense claims with a test card number (flag only, never an identifier) and a quoted field",
    );

    let s = c.story;
    let t = |x: &str| J::Str(s.t(x));
    json_file(
        c,
        "synthetic/json/registry-export-bwl.json",
        &J::Obj(vec![
            ("company_number", t("VC-0418822")),
            ("name", t("{e:bwl|BRIGHTWATER LOGISTICS LTD}")),
            ("incorporated", t("2024-02-12")),
            ("status", t("active")),
            ("registered_office", t("14 Quayside Row, Velmora VL2 4QR")),
            ("presenter", t("{e:kestrel}")),
            (
                "officers",
                J::Arr(vec![J::Obj(vec![
                    ("name", t("{e:irene|VASQUEZ-LINDQVIST, Irene}")),
                    ("role", t("director")),
                    ("appointed", t("2024-02-12")),
                ])]),
            ),
            (
                "shareholders",
                J::Arr(vec![J::Obj(vec![
                    ("name", t("{e:saltmarsh|SALTMARSH HOLDINGS S.A.}")),
                    ("shares", J::Num("1000".into())),
                    ("percent", J::Num("100".into())),
                ])]),
            ),
            (
                "contact",
                J::Obj(vec![
                    ("email", t("{i:em_bwl_accounts}")),
                    ("phone", t("{i:ph_bwl_office|e164}")),
                    ("website", t("{i:url_bwl}")),
                ]),
            ),
            ("record_url", t("{i:url_registry}")),
            ("dormant", J::Bool(false)),
            ("previous_names", J::Arr(vec![])),
        ]),
        "Registry export: Irene directs Brightwater, Saltmarsh owns it (JSON, E.164 phone)",
    );
    json_file(
        c,
        "synthetic/json/registry-export-saltmarsh.json",
        &J::Obj(vec![
            ("company_number", t("VC-0377105")),
            ("name", t("{e:saltmarsh}")),
            ("incorporated", t("2023-11-02")),
            ("registered_agent", t("{e:kestrel|KESTREL ADVISORY LLP}")),
            (
                "officers",
                J::Arr(vec![J::Obj(vec![
                    ("name", t("{e:corvin}")),
                    ("role", t("director")),
                    ("nominee", J::Bool(true)),
                ])]),
            ),
            (
                "shareholders",
                J::Arr(vec![J::Obj(vec![
                    ("name", t("{e:corvin|C. Hale}")),
                    ("percent", J::Num("100".into())),
                ])]),
            ),
            (
                "contact",
                J::Obj(vec![
                    ("email", t("{i:em_corvin}")),
                    ("phone", t("{i:ph_saltmarsh|e164}")),
                    ("website", t("{i:url_saltmarsh}")),
                ]),
            ),
            ("notes", J::Null),
        ]),
        "Registry export for the offshore parent (nominee director)",
    );
    json_file(
        c,
        "synthetic/json/coralline-account-export.json",
        &coralline(s),
        "Exchange account export: Saltmarsh's crypto deposits and withdrawals",
    );
}

fn coralline(s: &Story) -> J {
    let t = |x: &str| J::Str(s.t(x));
    let tx = |id: &str, ts: &str, kind: &str, asset: &str, net: &str, amount: &str, addr: &str| {
        J::Obj(vec![
            ("id", t(id)),
            ("timestamp", t(ts)),
            ("type", t(kind)),
            ("asset", t(asset)),
            ("network", t(net)),
            ("amount", J::Num(amount.into())),
            ("address", t(addr)),
        ])
    };
    J::Obj(vec![
        ("exchange", t("{e:exchange}")),
        (
            "account",
            J::Obj(vec![
                ("holder", t("{e:saltmarsh|SALTMARSH HOLDINGS S.A.}")),
                ("contact", t("{i:em_corvin}")),
                ("tier", t("otc")),
            ]),
        ),
        (
            "transactions",
            J::Arr(vec![
                tx(
                    "TX-88120",
                    "2025-02-19T10:15:00Z",
                    "trade_settlement",
                    "BTC",
                    "bitcoin",
                    "2.40000000",
                    "{i:btc_saltmarsh}",
                ),
                tx(
                    "TX-88121",
                    "2025-02-19T10:16:00Z",
                    "trade_settlement",
                    "USDT",
                    "tron",
                    "25000.00",
                    "{i:tron_saltmarsh}",
                ),
                tx(
                    "TX-88157",
                    "2025-02-21T08:02:00Z",
                    "withdrawal",
                    "BTC",
                    "bitcoin",
                    "0.50000000",
                    "{i:btc_daniel}",
                ),
                tx(
                    "TX-88158",
                    "2025-02-21T08:05:00Z",
                    "withdrawal",
                    "USDT",
                    "ethereum",
                    "12000.00",
                    "{i:evm_daniel|lower}",
                ),
                tx(
                    "TX-88201",
                    "2025-03-03T14:40:00Z",
                    "withdrawal",
                    "SOL",
                    "solana",
                    "150.0",
                    "{i:sol_corvin}",
                ),
            ]),
        ),
    ])
}
