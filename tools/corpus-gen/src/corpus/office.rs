//! DOCX documents and HTML pages.

use mk_bench::{FileKind, RelationType};

use super::stem;
use crate::formats::{DBlock, Enc, Html, docx};
use crate::story::Story;
use crate::truth::{Collector, FileMeta};

fn docx_file(c: &mut Collector, path: &str, blocks: &[DBlock], purpose: &str) {
    let (bytes, placed) = docx(blocks);
    let id = format!("d-{}", stem(path));
    c.write_file(path, &bytes, FileMeta::new(FileKind::Docx, purpose));
    c.whole(&id, path, FileKind::Docx);
    c.ingest(&id, placed);
}

fn html_file(c: &mut Collector, path: &str, h: Html, purpose: &str) {
    let (src, placed) = h.finish();
    let id = format!("d-{}", stem(path));
    c.write_file(
        path,
        src.as_bytes(),
        FileMeta::new(FileKind::Html, purpose).encoding(Enc::Utf8.name()),
    );
    c.whole(&id, path, FileKind::Html);
    c.ingest(&id, placed);
}

pub fn build(c: &mut Collector) {
    let s = c.story;
    let h = |x: &str| DBlock::Heading(s.t(x));
    let p = |x: &str| DBlock::Para(s.t(x));
    let row = |cells: &[&str]| cells.iter().map(|x| s.t(x)).collect::<Vec<_>>();

    docx_file(
        c,
        "synthetic/docx/tender-evaluation-scores-031.docx",
        &[
            h("Evaluation scores, tender {i:tnd_031}"),
            p(
                "Evaluation panel chaired by {e:daniel}. Scores out of 100; price weighs 30, quality 70.",
            ),
            DBlock::Table(vec![
                row(&["Bidder", "Price", "Quality", "Total"]),
                row(&["{e:bwl|split=5}", "26", "68", "94"]),
                row(&["{e:northgate|Northgate Marine Supplies}", "30", "41", "71"]),
            ]),
            p(
                "The quality scores were set by the chair alone. The other two panel members did not sign this sheet.",
            ),
        ],
        "Score sheet with a table; the winner's name is split across two runs (hard)",
    );
    docx_file(
        c,
        "synthetic/docx/kestrel-engagement-letter-bwl.docx",
        &[
            h("Letter of engagement"),
            p("Client: {e:bwl}. Date: 14 February 2024."),
            p(
                "This letter sets out the terms on which {e:kestrel|Kestrel Advisory|#el_kestrel} will act for the client. The engagement partner is {e:tomas|#el_tomas}.",
            ),
            p("Fees are payable on receipt of each fee note to IBAN {i:iban_kestrel|split=9}."),
            p("Questions: {i:em_tomas} or {i:ph_kestrel|intl}. Our team: {i:url_kestrel}."),
        ],
        "Engagement letter; the IBAN is split across two runs (hard)",
    );
    c.relation(
        RelationType::WorksFor,
        "d-kestrel-engagement-letter-bwl",
        "el_tomas",
        "el_kestrel",
    );
    docx_file(
        c,
        "synthetic/docx/audit-interview-priya.docx",
        &[
            h("Interview record"),
            p(
                "Interviewer: {e:rosalind}. Interviewee: {e:priya|#iv_priya}, accounts-payable clerk at {e:vpa|#iv_vpa}. Date: 31 March 2025.",
            ),
            p(
                "{e:priya|Priya} noticed in October 2024 that the accounts number on the {e:bwl|Brightwater} invoices had changed to {i:ph_daniel_mob|national}.",
            ),
            p(
                "She called the number once. The voicemail greeting gave the name {e:daniel|Daniel Okonkwo Hart}.",
            ),
            p("She reported the matter by email on 27 March 2025 to {i:em_audit}."),
        ],
        "Audit interview record (name variant without hyphen)",
    );
    c.relation(
        RelationType::WorksFor,
        "d-audit-interview-priya",
        "iv_priya",
        "iv_vpa",
    );
    docx_file(
        c,
        "synthetic/docx/northgate-debrief-request.docx",
        &[
            h("Request for debrief"),
            p("To: Procurement, {e:vpa|Velmora Port Authority}"),
            p("From: {e:gregor|Mr Almqvist}, {e:northgate|NORTHGATE MARINE SUPPLIES INC.}"),
            p(
                "We ask for a debrief on tender {i:tnd_031}. Our price was the lowest by £166,500. Please reply to {i:em_gregor} or call {i:ph_gregor|intl}.",
            ),
        ],
        "Debrief request from the losing bidder",
    );

    html_file(
        c,
        "synthetic/html/harbour-post-port-contracts.html",
        article(s),
        "Online news article: names, a lookalike person and a lookalike company",
    );
    c.event(
        "ev-article",
        "2025-05-02",
        "The Harbour Post publishes its story on the port contracts",
        "d-harbour-post-port-contracts",
        &["ar_paper", "ar_bwl"],
        "Published on 2 May 2025.",
    );
    html_file(
        c,
        "synthetic/html/brightwater-services.html",
        bwl_site(s),
        "Company website with contact details",
    );
    html_file(
        c,
        "synthetic/html/kestrel-team.html",
        kestrel_site(s),
        "Advisory firm team page",
    );
}

fn page_start(h: &mut Html, title: &str) {
    h.raw("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<title>")
        .raw(title)
        .raw("</title>\n<link rel=\"stylesheet\" href=\"/static/site.css\">\n</head>\n<body>\n");
}

fn page_end(h: &mut Html) {
    h.raw("</body>\n</html>\n");
}

fn article(s: &Story) -> Html {
    let mut h = Html::default();
    page_start(&mut h, "Port contracts went to a firm with family links");
    h.raw("<nav><a href=\"/\">Home</a> | <a href=\"/news\">News</a></nav>\n<article>\n");
    h.el(
        "h1",
        &s.t("Port contracts went to a firm with family links"),
    );
    h.el(
        "p class=\"byline\"",
        &s.t("By {e:lena}, {e:harbourpost|#ar_paper}"),
    );
    h.el("p class=\"date\"", &s.t("Published on 2 May 2025."));
    h.el("p", &s.t("Two logistics contracts of the {e:vpa} worth more than £1.2m went to {e:bwl|Brightwater Logistics|#ar_bwl}, a company formed weeks before the first tender by {e:kestrel}."));
    h.el("p", &s.t("The sole director of the company, {e:irene|Ms Vasquez-Lindqvist}, is the sister-in-law of the authority's director of procurement, {e:daniel}. He did not answer our questions."));
    h.el("p", &s.t("{e:gregory}, who speaks for the {e:union|Dock Workers Union}, said members were worried about the cost of the contracts."));
    h.el(
        "p",
        &s.t(
            "{e:bwl_gmbh}, a German firm with a similar name, told us it has no link to the port.",
        ),
    );
    h.el(
        "p class=\"permalink\"",
        &s.t("Permanent link: {i:url_article}"),
    );
    h.raw("</article>\n<footer><img src=\"/img/pixel.gif\" width=\"1\" height=\"1\" alt=\"\"></footer>\n");
    page_end(&mut h);
    h
}

fn bwl_site(s: &Story) -> Html {
    let mut h = Html::default();
    page_start(&mut h, "Services");
    h.raw("<header>");
    h.el("h1", &s.t("{e:bwl|Brightwater Logistics}"));
    h.raw("</header>\n<main>\n");
    h.el(
        "p",
        &s.t("Port logistics, dredging coordination & berth liaison since 2024."),
    );
    h.el("h2", &s.t("Contact"));
    h.raw("<ul>\n");
    h.el("li", &s.t("Email: {i:em_bwl_accounts}"));
    h.el("li", &s.t("Phone: {i:ph_bwl_office|intl}"));
    h.el("li", &s.t("Web: {i:url_bwl}"));
    h.raw("</ul>\n</main>\n");
    h.el(
        "footer",
        &s.t("© 2024 {e:bwl}. Registered office: 14 Quayside Row, Velmora."),
    );
    page_end(&mut h);
    h
}

fn kestrel_site(s: &Story) -> Html {
    let mut h = Html::default();
    page_start(&mut h, "Our team");
    h.el("h1", &s.t("{e:kestrel}"));
    h.el("h2", &s.t("Our team"));
    h.raw("<div class=\"card\">\n");
    h.el("h3", &s.t("{e:tomas}"));
    h.el(
        "p",
        &s.t("Managing partner. Company formation, nominee services and cross-border structures."),
    );
    h.el("p", &s.t("{i:em_tomas} · {i:ph_kestrel|intl}"));
    h.raw("</div>\n");
    h.el(
        "p",
        &s.t("Find us at {i:url_kestrel} or visit {i:dom_kestrel}."),
    );
    page_end(&mut h);
    h
}
