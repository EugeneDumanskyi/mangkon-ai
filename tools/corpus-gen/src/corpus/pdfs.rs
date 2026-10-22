//! Text PDFs: invoices, reports, minutes, contract, registry extracts, the 500-page ledger.

use mk_bench::RelationType;

use super::{a4, plain, text_pdf};
use crate::fonts::{Afm, Font, Metrics};
use crate::layout::{Flow, Laid};
use crate::story::Story;
use crate::truth::Collector;

pub struct Invoice<'a> {
    pub issuer: &'a str,
    pub issuer_lines: Vec<&'a str>,
    pub to_lines: Vec<&'a str>,
    pub fields: Vec<(&'a str, &'a str)>,
    pub items: Vec<(&'a str, &'a str)>,
    pub total: &'a str,
    pub pay: Vec<&'a str>,
    pub footer: &'a str,
}

pub fn invoice(s: &Story, m: &dyn Metrics, inv: &Invoice) -> Laid {
    let mut f = Flow::new(m, a4(), 56.0);
    let right = f.width - f.right;
    f.para(Font::SansBold, 16.0, &s.t(inv.issuer));
    let title_base = f.top + 18.0;
    f.line_right(right, title_base, Font::SansBold, 20.0, &plain("INVOICE"));
    for l in &inv.issuer_lines {
        f.para(Font::Sans, 9.0, &s.t(l));
    }
    f.gap(14.0);
    f.para(Font::SansBold, 9.0, &plain("Bill to"));
    let block_top = f.y;
    for l in &inv.to_lines {
        f.para(Font::Sans, 9.5, &s.t(l));
    }
    let after_to = f.y;
    f.y = block_top;
    for (k, v) in &inv.fields {
        f.row(&[340.0, 430.0], Font::Sans, 9.5, &[plain(k), s.t(v)]);
    }
    f.y = f.y.max(after_to) + 16.0;
    f.hrule();
    f.row(
        &[56.0, 400.0],
        Font::SansBold,
        9.5,
        &[plain("Description"), plain("")],
    );
    f.line_right(
        right,
        f.y - 9.5 * 0.3,
        Font::SansBold,
        9.5,
        &plain("Amount"),
    );
    f.hrule();
    for (d, a) in &inv.items {
        let base = f.y + 9.5;
        f.para_in(56.0, 360.0, Font::Sans, 9.5, &s.t(d));
        f.line_right(right, base, Font::Sans, 9.5, &plain(a));
        f.gap(2.0);
    }
    f.hrule();
    let base = f.y + 10.0;
    f.line_at(340.0, base, Font::SansBold, 10.0, &plain("Total due"));
    f.line_right(right, base, Font::SansBold, 10.0, &plain(inv.total));
    f.gap(30.0);
    f.para(Font::SansBold, 9.5, &plain("Payment details"));
    for p in &inv.pay {
        f.para(Font::Sans, 9.5, &s.t(p));
    }
    // The footer sits in the bottom margin.
    f.bottom = 24.0;
    f.y = f.height - 48.0;
    f.para_in(56.0, right - 56.0, Font::Sans, 7.5, &s.t(inv.footer));
    f.laid
}

const BWL_FOOTER: &str = "{e:bwl} · Registered with the {e:registry} under no. VC-0418822 · Registered office: 14 Quayside Row, Velmora VL2 4QR";

/// Brightwater invoice `n` (1–6). Used for the PDF files, the scanned copy and the email attachment.
pub fn invoice_bwl(s: &Story, m: &dyn Metrics, n: usize) -> Laid {
    const DATES: [&str; 6] = [
        "1 August 2024",
        "2 September 2024",
        "1 October 2024",
        "4 November 2024",
        "2 December 2024",
        "6 January 2025",
    ];
    const AMOUNTS: [&str; 6] = [
        "£148,500.00",
        "£212,750.00",
        "£187,300.00",
        "£256,900.00",
        "£231,400.00",
        "£247,650.00",
    ];
    // From invoice 3 on, the contact mobile is Daniel's private number: the key clue.
    let phone = if n >= 3 {
        "Accounts: {i:ph_daniel_mob|national}"
    } else {
        "Accounts: {i:ph_bwl_office|national}"
    };
    let number = format!("BWL-2024-{n:03}");
    let tender = if n == 6 { "{i:tnd_007}" } else { "{i:tnd_031}" };
    let item1 = format!("Dredging logistics coordination, phase {n} (tender {tender})");
    let items = vec![
        (item1.as_str(), AMOUNTS[n - 1]),
        ("Vessel scheduling and berth liaison", "included"),
        ("Site supervision and reporting", "included"),
    ];
    invoice(
        s,
        m,
        &Invoice {
            issuer: "{e:bwl|BRIGHTWATER LOGISTICS LTD}",
            issuer_lines: vec![
                "14 Quayside Row, Velmora VL2 4QR",
                phone,
                "Email: {i:em_bwl_accounts}",
                "Web: {i:url_bwl}",
            ],
            to_lines: vec![
                "{e:vpa}",
                "Accounts Payable, Harbour House",
                "1 Pier Approach, Velmora VL1 1AA",
            ],
            fields: vec![
                ("Invoice no.", &number),
                ("Date", DATES[n - 1]),
                ("Terms", "14 days"),
                ("Contract", "Framework FA-2024-117"),
            ],
            items,
            total: AMOUNTS[n - 1],
            pay: vec![
                "Bank: {e:meridian}",
                "Account name: {e:bwl|Brightwater Logistics Limited}",
                "IBAN: {i:iban_bwl}",
            ],
            footer: BWL_FOOTER,
        },
    )
}

fn kestrel_invoice(s: &Story, n: usize) -> Laid {
    const NUMBERS: [&str; 3] = ["KA-1186", "KA-1203", "KA-1219"];
    const DATES: [&str; 3] = ["29 November 2024", "10 January 2025", "14 February 2025"];
    const AMOUNTS: [&str; 3] = ["€212,000.00", "€268,500.00", "€195,000.00"];
    invoice(
        s,
        &Afm,
        &Invoice {
            issuer: "{e:kestrel}",
            issuer_lines: vec![
                "Chartered accountants and corporate services",
                "Suite 9, 220 Canal Street, Velmora VL3 8BD",
                "Tel. {i:ph_kestrel} · {i:em_tomas}",
                "{i:url_kestrel}",
            ],
            to_lines: vec![
                "{e:bwl}",
                "Attn: {e:irene|Ms Vasquez-Lindqvist}",
                "14 Quayside Row, Velmora VL2 4QR",
            ],
            fields: vec![
                ("Invoice no.", NUMBERS[n]),
                ("Date", DATES[n]),
                ("Terms", "On receipt"),
            ],
            items: vec![
                ("Strategic advisory services, port sector", AMOUNTS[n]),
                ("Corporate secretarial services", "included"),
            ],
            total: AMOUNTS[n],
            pay: vec![
                "Please pay to: {e:kestrel|Kestrel Advisory}",
                "IBAN: {i:iban_kestrel}",
            ],
            footer: "{e:kestrel|KESTREL ADVISORY LLP} · Managing partner: {e:tomas}",
        },
    )
}

fn northgate_invoice(s: &Story, n: usize) -> Laid {
    const DATES: [&str; 3] = ["12 March 2024", "18 June 2024", "9 October 2024"];
    const ITEMS: [(&str, &str); 3] = [
        ("Mooring ropes, 64 mm, 12 coils", "$18,420.00"),
        ("Fender units, cylindrical, 4 pcs", "$27,960.00"),
        ("Bollard paint and anti-corrosion kit", "$6,385.50"),
    ];
    let number = format!("NMS-24-{:04}", 311 + n * 17);
    invoice(
        s,
        &Afm,
        &Invoice {
            issuer: "{e:northgate}",
            issuer_lines: vec![
                "Ship chandlers since 1987",
                "Pier 6, Velmora VL1 9TT",
                "Sales: {i:ph_gregor} · {i:em_gregor}",
            ],
            to_lines: vec![
                "{e:vpa}",
                "Accounts Payable",
                "1 Pier Approach, Velmora VL1 1AA",
            ],
            fields: vec![
                ("Invoice no.", &number),
                ("Date", DATES[n]),
                ("Terms", "30 days"),
            ],
            items: vec![ITEMS[n]],
            total: ITEMS[n].1,
            pay: vec!["IBAN: {i:iban_gregor|compact}"],
            footer: "{e:northgate|NORTHGATE MARINE SUPPLIES INC.} · Director: {e:gregor}",
        },
    )
}

pub fn build(c: &mut Collector) {
    let s = c.story;
    for n in 1..=6 {
        let path = format!("synthetic/pdf/invoice-bwl-2024-{n:03}.pdf");
        let purpose = if n >= 3 {
            "Brightwater invoice to the Port Authority; contact mobile is Daniel's private number"
        } else {
            "Brightwater invoice to the Port Authority (table layout)"
        };
        text_pdf(
            c,
            &path,
            &format!("d-invoice-bwl-{n:03}"),
            &invoice_bwl(s, &Afm, n),
            purpose,
        );
    }
    for n in 0..3 {
        let path = format!(
            "synthetic/pdf/invoice-kestrel-{}.pdf",
            ["2024-11", "2025-01", "2025-02"][n]
        );
        text_pdf(
            c,
            &path,
            &format!("d-invoice-kestrel-{}", n + 1),
            &kestrel_invoice(s, n),
            "Kestrel invoice to Brightwater: the money moves on",
        );
    }
    for n in 0..3 {
        let path = format!("synthetic/pdf/invoice-northgate-{}.pdf", n + 1);
        text_pdf(
            c,
            &path,
            &format!("d-invoice-northgate-{}", n + 1),
            &northgate_invoice(s, n),
            "Honest supplier invoice (background, own identifiers)",
        );
    }
    audit_report(c);
    article(c);
    contract(c);
    evaluation_memo(c);
    board_minutes(c);
    payment_instruction(c);
    registry_extracts(c);
    ledger(c);
}

/// Lays two columns of paragraphs side by side from the current `y`.
fn two_columns(f: &mut Flow, s: &Story, left: &[&str], right: &[&str], size: f64) {
    let top = f.y;
    let (w, gap) = ((f.width - f.left - f.right - 18.0) / 2.0, 18.0);
    for p in left {
        f.para_in(f.left, w, Font::Serif, size, &s.t(p));
        f.gap(size * 0.5);
    }
    let left_end = f.y;
    f.y = top;
    let x2 = f.left + w + gap;
    for p in right {
        f.para_in(x2, w, Font::Serif, size, &s.t(p));
        f.gap(size * 0.5);
    }
    f.y = f.y.max(left_end);
}

/// Adds a header and a footer line to every page. Headers come last in reading order, as
/// they are drawn after the body (a common real-world pattern).
fn header_footer(f: &mut Flow, s: &Story, header: &str, footer: &dyn Fn(usize, usize) -> String) {
    let n = f.laid.pages.len();
    let (left, right) = (f.left, f.width - f.right);
    for i in 0..n {
        f.line_on(i, left, 34.0, Font::Sans, 8.0, &s.t(header));
        let foot = plain(&footer(i + 1, n));
        let w = f.text_width(Font::Sans, 8.0, &foot.plain());
        f.line_on(i, right - w, f.height - 30.0, Font::Sans, 8.0, &foot);
    }
}

fn audit_report(c: &mut Collector) {
    let s = c.story;
    let doc = "d-audit-report";
    let mut f = Flow::new(&Afm, a4(), 56.0);
    f.top = 60.0;
    f.y = 60.0;
    f.para(
        Font::SansBold,
        18.0,
        &plain("Internal Audit Report IA-2025-04"),
    );
    f.para(
        Font::Sans,
        11.0,
        &s.t("Procurement of logistics services, tenders {i:tnd_031} and {i:tnd_007}"),
    );
    f.para(
        Font::Sans,
        9.0,
        &s.t("Prepared by {e:rosalind}, Chief Internal Auditor · 10 April 2025"),
    );
    f.gap(12.0);
    f.hrule();
    f.gap(6.0);
    two_columns(
        &mut f,
        s,
        &[
            "1. Summary. Between August 2024 and January 2025 {e:vpa|#vpa_paid} paid {e:bwl|#bwl_paid} £1,284,500 under framework FA-2024-117. We found that the award process did not follow the procurement rules and that the supplier has links to a member of staff.",
            "2. Scope. We reviewed the tender file, the evaluation memo, six invoices, the bank records supplied by {e:meridian|Meridian Trust}, and a report made to this office by {e:priya|#priya_rep}.",
            "3. Findings. On 28 March 2025 {e:priya|P. Ramanathan-Cole|#priya_ev} reported that the accounts phone number on invoices 3 to 6, {i:ph_daniel_mob|intl}, is listed in staff records as the private mobile of the Director of Procurement.",
        ],
        &[
            "Records of the {e:registry} show that {e:bwl|Brightwater} was incorporated on 12 February 2024, five weeks before the tender was planned, with {e:kestrel} as formation agent. Its sole director is {e:irene}.",
            "The evaluation memo scored {e:bwl|Brightwater Logistics} above {e:northgate|Northgate Marine} on 'local presence', a criterion that was not in the published tender.",
            "4. Recommendation. Suspend payments under FA-2024-117, refer the matter to the police, and place {e:daniel|D. Okonkwo-Hart} on leave pending investigation.",
        ],
        9.5,
    );
    f.new_page();
    f.para(
        Font::SansBold,
        11.0,
        &plain("Annex A. Payments under FA-2024-117"),
    );
    f.gap(4.0);
    f.row(
        &[56.0, 150.0, 300.0, 440.0],
        Font::SansBold,
        9.0,
        &[
            plain("Date"),
            plain("Invoice"),
            plain("Paid to"),
            plain("Amount"),
        ],
    );
    f.hrule();
    let rows = [
        ("14 Aug 2024", "BWL-2024-001", "£148,500.00"),
        ("16 Sep 2024", "BWL-2024-002", "£212,750.00"),
        ("15 Oct 2024", "BWL-2024-003", "£187,300.00"),
        ("18 Nov 2024", "BWL-2024-004", "£256,900.00"),
        ("16 Dec 2024", "BWL-2024-005", "£231,400.00"),
        ("20 Jan 2025", "BWL-2024-006", "£247,650.00"),
    ];
    for (d, inv, amt) in rows {
        f.row(
            &[56.0, 150.0, 300.0, 440.0],
            Font::Sans,
            9.0,
            &[
                plain(d),
                plain(inv),
                s.t("{i:iban_bwl|compact}"),
                plain(amt),
            ],
        );
    }
    f.gap(10.0);
    f.para(Font::Sans, 9.5, &s.t("All six payments left the {e:vpa|VPA} operating account {i:iban_vpa} and were credited to the account above, held at {e:meridian}."));
    f.new_page();
    f.para(
        Font::SansBold,
        11.0,
        &plain("Annex B. Persons and companies"),
    );
    f.gap(4.0);
    for p in [
        "{e:irene|#irene_dir} is the sole director of {e:bwl|#bwl_dir}.",
        "{e:saltmarsh|#salt_own} owns all shares of {e:bwl|Brightwater Logistics Ltd|#bwl_own}.",
        "{e:tomas|#tomas_dir} is the managing partner of {e:kestrel|#kestrel_dir}.",
        "{e:daniel|#dan_works} is Director of Procurement at {e:vpa|#vpa_works}.",
    ] {
        f.para(Font::Sans, 9.5, &s.t(p));
        f.gap(3.0);
    }
    header_footer(
        &mut f,
        s,
        "{e:vpa|VPA} Internal Audit · Confidential",
        &|i, n| format!("IA-2025-04 · page {i} of {n}"),
    );
    text_pdf(
        c,
        "synthetic/pdf/audit-report-2025-04.pdf",
        doc,
        &f.laid,
        "Two-column report with repeated headers and footers; relations and events",
    );
    c.relation(RelationType::Paid, doc, "vpa_paid", "bwl_paid");
    c.relation(RelationType::DirectorOf, doc, "irene_dir", "bwl_dir");
    c.relation(RelationType::Owns, doc, "salt_own", "bwl_own");
    c.relation(RelationType::DirectorOf, doc, "tomas_dir", "kestrel_dir");
    c.relation(RelationType::WorksFor, doc, "dan_works", "vpa_works");
    c.event(
        "ev-report",
        "2025-03-28",
        "Priya reports the phone number match to Internal Audit",
        doc,
        &["priya_ev"],
        "On 28 March 2025 P. Ramanathan-Cole reported that the accounts phone number on invoices 3 to 6,",
    );
}

fn article(c: &mut Collector) {
    let s = c.story;
    let mut f = Flow::new(&Afm, a4(), 50.0);
    f.para(
        Font::SansBold,
        9.0,
        &s.t("{e:harbourpost} · Friday 2 May 2025"),
    );
    f.gap(4.0);
    f.para(
        Font::SansBold,
        22.0,
        &plain("Port paid £1.3m to firm linked to its own procurement chief"),
    );
    f.para(Font::Serif, 10.0, &s.t("By {e:lena} · {i:url_article}"));
    f.gap(8.0);
    f.hrule();
    two_columns(
        &mut f,
        s,
        &[
            "{e:vpa|Port of Velmora} paid more than £1.3 million to a company whose director is related by marriage to the official who ran the tender, documents seen by the {e:harbourpost|Harbour Post} show.",
            "{e:bwl|Brightwater Logistics} won tender {i:tnd_031} in July 2024 although it had no staff, no vessels and had existed for less than five months. Its director, {e:irene|Irene Vasquez}, is the sister-in-law of procurement director {e:daniel|Daniel Okonkwo Hart}.",
            "Company records list the shareholder as {e:saltmarsh|Saltmarsh Holdings}, which shares a registered agent with {e:kestrel|Kestrel}.",
        ],
        &[
            "\"Our members load the ships. We never saw anyone from that firm on the quay,\" said {e:gregory}, a spokesman for the {e:union}.",
            "{e:gregor}, whose company {e:northgate|Northgate Marine Supplies} lost the tender, said he had raised concerns in June 2024 and received no reply.",
            "The {e:vpa} said it had started an internal review. {e:daniel|Mr Okonkwo-Hart} did not respond to requests for comment. {e:bwl|Brightwater Logistics Ltd} could not be reached at {i:ph_bwl_office|intl}.",
            "No relation: {e:bwl_gmbh}, a Hamburg firm with a similar name, told the {e:harbourpost|Harbour Post} it has no link to the Velmora company.",
        ],
        9.5,
    );
    text_pdf(
        c,
        "synthetic/pdf/harbour-post-article.pdf",
        "d-article-pdf",
        &f.laid,
        "Two-column news article; lookalike person and company names",
    );
}

fn contract(c: &mut Collector) {
    let s = c.story;
    let doc = "d-contract";
    let mut f = Flow::new(&Afm, a4(), 64.0);
    f.para(
        Font::SansBold,
        16.0,
        &plain("Framework Agreement FA-2024-117"),
    );
    f.para(
        Font::Sans,
        10.0,
        &s.t("Dredging logistics coordination · tender {i:tnd_031}"),
    );
    f.gap(14.0);
    for p in [
        "This agreement is made on 10 July 2024 between {e:vpa|#vpa_c} (the Authority) and {e:bwl|Brightwater Logistics Limited|#bwl_c} (the Contractor), registered office 14 Quayside Row, Velmora.",
        "1. Term. The agreement runs for 24 months from the date above and may be extended once by 12 months.",
        "2. Services. The Contractor shall coordinate dredging logistics, vessel scheduling and berth liaison as set out in Schedule 1.",
        "3. Payment. The Authority shall pay monthly invoices within 14 days to the Contractor's account {i:iban_bwl} at {e:meridian}.",
        "4. Notices. Notices to the Contractor go to {e:irene|I. Vasquez-Lindqvist}, {i:em_irene}. Notices to the Authority go to the Director of Procurement, {i:em_daniel}.",
        "5. Conflicts of interest. Each party confirms that no member of its staff has a personal interest in this agreement.",
    ] {
        f.para(Font::Serif, 10.5, &s.t(p));
        f.gap(5.0);
    }
    f.new_page();
    f.para(Font::SansBold, 11.0, &plain("Signatures"));
    f.gap(30.0);
    f.para(
        Font::Serif,
        10.5,
        &s.t("For the Authority: {e:daniel}, Director of Procurement"),
    );
    f.gap(30.0);
    f.para(
        Font::Serif,
        10.5,
        &s.t("For the Contractor: {e:irene}, Director"),
    );
    // Schedule 1 is a landscape table on a page with /Rotate 90.
    f.width = 842.0;
    f.height = 595.0;
    f.new_page();
    f.page().rotate = 90;
    f.para(
        Font::SansBold,
        12.0,
        &plain("Schedule 1. Payment schedule (landscape)"),
    );
    f.gap(6.0);
    let cols = [64.0, 180.0, 420.0, 640.0];
    f.row(
        &cols,
        Font::SansBold,
        9.5,
        &[
            plain("Month"),
            plain("Milestone"),
            plain("Payee account"),
            plain("Cap (GBP)"),
        ],
    );
    f.hrule();
    for (mth, ms, cap) in [
        ("Aug 2024", "Mobilisation", "150,000"),
        ("Sep 2024", "Survey vessels on site", "215,000"),
        ("Oct 2024", "Channel phase 1", "190,000"),
        ("Nov 2024", "Channel phase 2", "260,000"),
        ("Dec 2024", "Berth 4 approach", "235,000"),
        ("Jan 2025", "Close-out report", "250,000"),
    ] {
        f.row(
            &cols,
            Font::Sans,
            9.5,
            &[plain(mth), plain(ms), s.t("{i:iban_bwl|hard}"), plain(cap)],
        );
    }
    f.gap(8.0);
    f.para(Font::Sans, 9.0, &s.t("Contractor contact for this schedule: {e:irene|Irene}, {i:ph_bwl_office|national|hard}."));
    text_pdf(
        c,
        "synthetic/pdf/framework-contract-bwl.pdf",
        doc,
        &f.laid,
        "Contract; page 3 is a landscape schedule on a /Rotate 90 page",
    );
    c.event(
        "ev-contract",
        "2024-07-10",
        "Framework agreement signed between the Port Authority and Brightwater",
        doc,
        &["vpa_c", "bwl_c"],
        "This agreement is made on 10 July 2024 between Velmora Port Authority (the Authority) and Brightwater Logistics Limited (the Contractor),",
    );
}

fn evaluation_memo(c: &mut Collector) {
    let s = c.story;
    let mut f = Flow::new(&Afm, a4(), 60.0);
    f.para(Font::SansBold, 14.0, &plain("Tender evaluation memo"));
    f.para(
        Font::Sans,
        10.0,
        &s.t("Tender {i:tnd_031} · Dredging logistics coordination · 26 June 2024"),
    );
    f.para(
        Font::Sans,
        10.0,
        &s.t(
            "From: {e:daniel|D. Okonkwo-Hart}, Director of Procurement · To: Board of the {e:vpa}",
        ),
    );
    f.gap(10.0);
    f.hrule();
    let cols = [60.0, 260.0, 340.0, 420.0, 490.0];
    f.row(
        &cols,
        Font::SansBold,
        9.5,
        &[
            plain("Bidder"),
            plain("Price"),
            plain("Quality"),
            plain("Local"),
            plain("Total"),
        ],
    );
    f.hrule();
    f.row(
        &cols,
        Font::Sans,
        9.5,
        &[
            s.t("{e:bwl|Brightwater Logistics Ltd}"),
            plain("31"),
            plain("38"),
            plain("20"),
            plain("89"),
        ],
    );
    f.row(
        &cols,
        Font::Sans,
        9.5,
        &[
            s.t("{e:northgate|Northgate Marine Supplies Inc.}"),
            plain("40"),
            plain("36"),
            plain("5"),
            plain("81"),
        ],
    );
    f.gap(10.0);
    for p in [
        "The 'Local' criterion rewards bidders with an office in Velmora. It was added after the tender was published.",
        "Recommendation: award the framework to {e:bwl|Brightwater}. Contact for the bidder: {e:irene}, {i:em_irene|mixed}.",
        "Signed {e:daniel|Mr Okonkwo-Hart}.",
    ] {
        f.para(Font::Sans, 10.0, &s.t(p));
        f.gap(4.0);
    }
    f.page().shuffle = true;
    text_pdf(
        c,
        "synthetic/pdf/tender-evaluation-memo.pdf",
        "d-eval-memo",
        &f.laid,
        "Content stream written out of reading order",
    );
}

fn board_minutes(c: &mut Collector) {
    let s = c.story;
    let doc = "d-board-minutes";
    let mut f = Flow::new(&Afm, a4(), 60.0);
    f.top = 64.0;
    f.y = 64.0;
    f.para(
        Font::SansBold,
        14.0,
        &plain("Minutes of the Board meeting, 3 July 2024"),
    );
    f.gap(8.0);
    let items = [
        "Present: the Chair, four board members, the Chief Executive, {e:rosalind} (Chief Internal Auditor, observer) and {e:daniel} (Director of Procurement, for item 4).",
        "1. Apologies. None.",
        "2. Minutes of the previous meeting were approved.",
        "3. Harbour dues for 2025 were approved without change.",
        "4. Tender {i:tnd_031}. {e:daniel|Mr Okonkwo-Hart} presented the evaluation memo and recommended the award to {e:bwl|Brightwater Logistics}.",
        "A board member asked why a company formed in 2024 scored highest on local presence. {e:daniel|Mr Okonkwo-Hart} said the firm's director had long experience in the port.",
        "{e:rosalind|Ms Achebe-Frost} asked that the conflict-of-interest declarations be filed with the tender papers. The Chair agreed.",
        "Decision: on 3 July 2024 the Board awarded tender VPA-TND-2024-031 to Brightwater Logistics Ltd.",
        "5. Any other business. A complaint from {e:northgate} about the tender timetable was noted.",
        "6. Next meeting: 4 September 2024.",
    ];
    for (i, p) in items.iter().enumerate() {
        let p = if i == 7 {
            "Decision: on 3 July 2024 the {e:vpa|#vpa_award} Board awarded tender {i:tnd_031} to {e:bwl|#bwl_award}."
        } else {
            p
        };
        f.para(Font::Serif, 11.0, &s.t(p));
        f.gap(8.0);
        // Spread the minutes over four pages, so the headers repeat.
        if i == 2 || i == 5 || i == 7 {
            f.new_page();
        }
    }
    header_footer(
        &mut f,
        s,
        "{e:vpa|VELMORA PORT AUTHORITY} · Board minutes · 3 July 2024",
        &|i, n| format!("Page {i} of {n} · Confidential"),
    );
    text_pdf(
        c,
        "synthetic/pdf/board-minutes-2024-07.pdf",
        doc,
        &f.laid,
        "Four pages with repeated header/footer carrying an org name",
    );
    c.event(
        "ev-award",
        "2024-07-03",
        "Board awards tender VPA-TND-2024-031 to Brightwater",
        doc,
        &["vpa_award", "bwl_award"],
        "Decision: on 3 July 2024 the Velmora Port Authority Board awarded tender VPA-TND-2024-031 to Brightwater Logistics Ltd.",
    );
}

fn payment_instruction(c: &mut Collector) {
    let s = c.story;
    let mut f = Flow::new(&Afm, a4(), 72.0);
    f.para(
        Font::SansBold,
        15.0,
        &s.t("{e:bwl|BRIGHTWATER LOGISTICS LTD}"),
    );
    f.para(Font::Sans, 9.0, &plain("14 Quayside Row, Velmora VL2 4QR"));
    f.gap(20.0);
    f.para(Font::Sans, 10.5, &s.t("To: Accounts Payable, {e:vpa}"));
    f.para(Font::Sans, 10.5, &plain("Date: 22 July 2024"));
    f.para(
        Font::Sans,
        10.5,
        &plain("Subject: Standing payment instruction"),
    );
    f.gap(12.0);
    // The column is narrow on purpose: the IBAN and the email wrap across lines.
    let w = 300.0;
    for p in [
        "Dear colleagues, please remit all payments under the framework agreement to our account at {e:meridian}, IBAN {i:iban_bwl|split=14}, quoting the invoice number.",
        "Remittance advice should be sent to {i:em_bwl_accounts|split=21} and not to individual staff.",
        "Kind regards,",
        "{e:irene|I. Vasquez-Lindqvist}, Director",
    ] {
        f.para_in(72.0, w, Font::Sans, 10.5, &s.t(p));
        f.gap(6.0);
    }
    text_pdf(
        c,
        "synthetic/pdf/payment-instruction-bwl.pdf",
        "d-payment-instruction",
        &f.laid,
        "Identifiers split across lines (hard)",
    );
}

fn registry_extracts(c: &mut Collector) {
    let s = c.story;
    let doc = "d-registry-bwl";
    let mut f = Flow::new(&Afm, a4(), 60.0);
    f.para(Font::SansBold, 13.0, &s.t("{e:registry}"));
    f.para(
        Font::Sans,
        10.0,
        &plain("Certified extract · company no. VC-0418822 · printed 3 March 2025"),
    );
    f.gap(10.0);
    f.hrule();
    let cols = [60.0, 210.0];
    for (k, v) in [
        ("Name", "{e:bwl|BRIGHTWATER LOGISTICS LTD}"),
        ("Status", "Active"),
        ("Incorporated", "12 February 2024"),
        ("Registered office", "14 Quayside Row, Velmora VL2 4QR"),
        ("Director", "{e:irene|VASQUEZ-LINDQVIST, Irene}"),
        (
            "Shareholder",
            "{e:saltmarsh|SALTMARSH HOLDINGS S.A.} (100%)",
        ),
        ("Formation agent", "{e:kestrel|KESTREL ADVISORY LLP}"),
        ("Registry page", "{i:url_registry}"),
    ] {
        f.row(&cols, Font::Sans, 10.0, &[plain(k), s.t(v)]);
    }
    f.gap(10.0);
    f.para(Font::Sans, 10.0, &s.t("{e:bwl|#bwl_inc} was incorporated on 12 February 2024 with {e:kestrel|Kestrel Advisory|#kestrel_inc} as formation agent."));
    text_pdf(
        c,
        "synthetic/pdf/registry-extract-bwl.pdf",
        doc,
        &f.laid,
        "Registry extract (key-value table)",
    );
    c.event(
        "ev-incorporated",
        "2024-02-12",
        "Brightwater Logistics Ltd incorporated",
        doc,
        &["bwl_inc", "kestrel_inc"],
        "Brightwater Logistics Ltd was incorporated on 12 February 2024 with Kestrel Advisory as formation agent.",
    );

    let doc = "d-registry-saltmarsh";
    let mut f = Flow::new(&Afm, a4(), 60.0);
    f.para(
        Font::SansBold,
        13.0,
        &plain("Register of Companies · Extract"),
    );
    f.para(
        Font::Sans,
        10.0,
        &plain("Company C 98113 · printed 11 April 2025"),
    );
    f.gap(10.0);
    f.hrule();
    for (k, v) in [
        ("Name", "{e:saltmarsh}"),
        ("Registered office", "Level 3, Triq il-Port, Valletta"),
        ("Director", "{e:corvin|#corvin_dir}"),
        ("Shareholder", "{e:corvin|Mr Hale} (nominee, 1 share)"),
        ("Contact", "{i:ph_saltmarsh} · {i:em_corvin}"),
        ("Web", "{i:url_saltmarsh}"),
    ] {
        f.row(&cols, Font::Sans, 10.0, &[plain(k), s.t(v)]);
    }
    f.gap(10.0);
    f.para(Font::Sans, 10.0, &s.t("{e:corvin|C. Hale|#corvin_own} holds the issued share of {e:saltmarsh|Saltmarsh Holdings S.A.|#salt_owned} as nominee."));
    text_pdf(
        c,
        "synthetic/pdf/registry-extract-saltmarsh.pdf",
        doc,
        &f.laid,
        "Offshore registry extract; nominee shareholder",
    );
    c.relation(RelationType::Owns, doc, "corvin_own", "salt_owned");
}

/// The 500-page vendor ledger (render timing for M0b). Story rows sit on a few known pages.
fn ledger(c: &mut Collector) {
    let s = c.story;
    let mut f = Flow::new(&Afm, a4(), 48.0);
    let cols = [48.0, 120.0, 330.0, 470.0];
    let costs = [
        "Fuel, tug fleet",
        "Electricity, terminal 2",
        "Payroll batch",
        "Crane spare parts",
        "Water rates",
        "Pilotage fees",
        "IT licences",
        "Cleaning contract",
        "Security patrols",
        "Office supplies",
    ];
    let bwl_pages = [37, 81, 126, 172, 219, 266];
    let northgate_pages = [12, 140, 301];
    let mut n = 0u32;
    for page in 0..500usize {
        if page > 0 {
            f.new_page();
        }
        f.para(
            Font::SansBold,
            9.0,
            &plain(&format!(
                "Vendor ledger 2024-2025 · page {} of 500",
                page + 1
            )),
        );
        f.row(
            &cols,
            Font::SansBold,
            8.5,
            &[
                plain("Date"),
                plain("Cost line"),
                plain("Reference"),
                plain("Amount"),
            ],
        );
        for row in 0..14usize {
            n += 1;
            let day = 1 + (n % 28);
            let month = 1 + (n / 28) % 12;
            let amount = format!("{}.{:02}", 400 + (n * 7919) % 90000, n % 100);
            let cell = if row == 5 && bwl_pages.contains(&page) {
                s.t("{e:bwl|BRIGHTWATER LOGISTICS LTD}")
            } else if row == 5 && northgate_pages.contains(&page) {
                s.t("{e:northgate|NORTHGATE MARINE SUPPLIES INC.}")
            } else {
                plain(costs[(n as usize) % costs.len()])
            };
            let reference = if row == 5 && bwl_pages.contains(&page) {
                s.t("{i:tnd_031}")
            } else {
                plain(&format!("AP-{:06}", 100000 + n))
            };
            f.row(
                &cols,
                Font::Sans,
                8.5,
                &[
                    plain(&format!("{day:02}/{month:02}/2024")),
                    cell,
                    reference,
                    plain(&amount),
                ],
            );
        }
    }
    text_pdf(
        c,
        "synthetic/pdf/vendor-ledger-2024-2025.pdf",
        "d-ledger",
        &f.laid,
        "500-page ledger for render timing (M0b); story rows on known pages",
    );
}
