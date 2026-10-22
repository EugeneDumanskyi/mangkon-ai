//! Plain-text and Markdown notes, with UTF-16 and Windows-1252 copies (NFR-COMP-06) and
//! CRLF line ends.

use mk_bench::FileKind;

use super::stem;
use crate::formats::{Enc, text_doc, with_crlf};
use crate::truth::{Collector, FileMeta};

fn note(c: &mut Collector, path: &str, template: &str, enc: Enc, crlf: bool, purpose: &str) {
    let kind = if path.ends_with(".md") {
        FileKind::Md
    } else {
        FileKind::Txt
    };
    let (text, placed) = text_doc(&c.story.t(template));
    let text = if crlf { with_crlf(&text) } else { text };
    let id = format!("d-{}", stem(path));
    c.write_file(
        path,
        &enc.encode(&text),
        FileMeta::new(kind, purpose).encoding(enc.name()),
    );
    c.whole(&id, path, kind);
    c.ingest(&id, placed);
}

const PRIYA: &str = "# Supplier checks, October 2024

- {e:bwl|Brightwater} invoice 3: accounts phone changed from {i:ph_bwl_office|national} to {i:ph_daniel_mob|national}
- invoice 4: same new number, written as {i:ph_daniel_mob|e164} on the remittance query
- the new number is in the staff list next to {e:daniel|Mr Okonkwo-Hart}
- {e:northgate|Northgate Marine} invoices: no change, phone {i:ph_gregor|national}

## To do

1. Ask {e:samuel|S. Achterberg} at the bank what the account shows
2. Keep copies of all six invoices
3. Report to internal audit if the pattern continues
";

const PRIYA_TXT: &str = "Supplier checks (copy kept at home)

{e:bwl|Brightwater} invoice 3 onwards: accounts phone {i:ph_daniel_mob|national}.
That number is the mobile of {e:daniel|D. Okonkwo-Hart} in the staff list.
Bank contact: {e:samuel}, {i:em_samuel}.
";

const LENA: &str = "# Port contracts: research notes

## People

- {e:daniel}: director of procurement, {e:vpa|VPA}
- {e:irene}: sole director of {e:bwl|Brightwater}; sister-in-law of the above
- {e:tomas}: partner at {e:kestrel|Kestrel}; formed the company
- {e:corvin}: nominee at {e:saltmarsh|Saltmarsh}

## Leads

- A source says {e:daniel|Okonkwo-Hart|#mm_daniel} met {e:corvin|Mr Hale|#mm_corvin} at the marina. Date of the meeting not known.
- Same source gave a wallet as {n:neg_btc_typo}; it does not validate, probably mistyped.
- Website: {i:url_saltmarsh}
- Registry record: {i:url_registry}
";

const LENA_TXT: &str = "Port contracts - research notes (export)

People:
  {e:daniel} - director of procurement, {e:vpa|VPA}
  {e:irene} - sole director of {e:bwl|Brightwater}
  {e:tomas} - partner at {e:kestrel|Kestrel} · formed the company
  {e:corvin} - nominee at {e:saltmarsh|Saltmarsh}

Payments seen: £1,284,500 in, €240,000 out to {i:iban_saltmarsh}.
Contact for the story: {i:em_lena}, {i:ph_lena|national}.
";

const TIMELINE: &str = "INTERNAL AUDIT - CASE IA-2025-04 - TIMELINE

2024-02-12  {e:bwl} incorporated; agent {e:kestrel}
2024-05-20  {i:tnd_031} published; contact {e:daniel|D. Okonkwo-Hart}
2024-06-25  contract awarded to {e:bwl|Brightwater}
2024-08-13  first payment to {i:iban_bwl}
2025-01-15  {e:kestrel|Kestrel} pays {e:saltmarsh|Saltmarsh} EUR 240,000
2025-03-27  report from {e:priya|P. Ramanathan-Cole}

Open points:
- chat screenshot shows an address typed as {n:neg_evm_case}; checksum fails, do not rely on it
- right address per exchange export: {i:evm_daniel|lower}
";

const GREGOR: &str = "Call notes - 16 April 2025

Called {e:gregor|G. Almqvist} back on {i:ph_gregor|intl}. He is angry about {i:tnd_007}.
He is not related to {e:gregory}, the {e:union} representative quoted in the press.
Different spelling, different person.
";

const SAMUEL: &str = "# Case file MTB-FC-2025-019

**Customer:** {e:bwl|Brightwater Logistics Limited}
**Account:** {i:iban_bwl|compact}

| Date | Counterparty | Account | Amount |
|---|---|---|---|
| 2024-08-15 | {e:kestrel|Kestrel Advisory} | {i:iban_kestrel} | 141,000.00 |
| 2024-09-16 | {e:kestrel|Kestrel Advisory} | {i:iban_kestrel} | 202,000.00 |

Contact at the Port Authority: {i:em_priya}.
Our line: {i:ph_meridian|national}.
";

pub fn build(c: &mut Collector) {
    note(
        c,
        "synthetic/notes/priya-supplier-checks.md",
        PRIYA,
        Enc::Utf8,
        false,
        "Clerk's Markdown notes: one phone number in three formats",
    );
    note(
        c,
        "synthetic/notes/priya-supplier-checks-utf16.txt",
        PRIYA_TXT,
        Enc::Utf16LeBom,
        false,
        "Short copy of the clerk's notes in UTF-16LE with BOM",
    );
    note(
        c,
        "synthetic/notes/lena-research.md",
        LENA,
        Enc::Utf8,
        false,
        "Reporter's notes: undated meeting, bad-checksum Bitcoin address (negative)",
    );
    c.event(
        "ev-marina",
        "unknown",
        "Daniel meets the Saltmarsh nominee at the marina",
        "d-lena-research",
        &["mm_daniel", "mm_corvin"],
        "met Mr Hale at the marina. Date of the meeting not known.",
    );
    note(
        c,
        "synthetic/notes/lena-research-export-cp1252.txt",
        LENA_TXT,
        Enc::Windows1252,
        false,
        "Reporter's notes exported in Windows-1252 (pound and euro signs)",
    );
    note(
        c,
        "synthetic/notes/audit-timeline.txt",
        TIMELINE,
        Enc::Utf8,
        true,
        "Audit timeline with CRLF line ends; EVM address with a wrong checksum case (negative)",
    );
    note(
        c,
        "synthetic/notes/gregor-call-notes.txt",
        GREGOR,
        Enc::Utf8,
        false,
        "Lookalike names in one note: two different people",
    );
    note(
        c,
        "synthetic/notes/meridian-case-file.md",
        SAMUEL,
        Enc::Utf8Bom,
        false,
        "Bank case file in Markdown with a table (UTF-8 with BOM)",
    );
}
