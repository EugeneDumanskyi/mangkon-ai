//! Scanned documents: image files (PNG, JPEG, TIFF), image-only PDFs, mixed text + scan PDFs
//! and scans with an invisible OCR text layer. Scans are laid out with the Noto metrics, so
//! the raster and its boxes come from the same layout.

use mk_bench::{BBox, FileKind, Location, RelationType, Unit};

use super::{a4, file_seed, invoice_bwl, plain, stem};
use crate::fonts::{Afm, Font, Noto};
use crate::layout::{Flow, HiddenWord, ImageData, Laid, Page, PageImage};
use crate::pdf::{self, PdfOpts, round2};
use crate::raster::{self, Scanned, Tier};
use crate::story::Story;
use crate::truth::{Collector, FileMeta, Placed};

#[derive(Clone, Copy)]
enum Img {
    Png,
    Jpeg(u8),
    Tiff,
}

/// Office-tier JPEG quality (`ScanTier::Office`).
const OFFICE_Q: u8 = 75;
const POOR_Q: u8 = 55;

pub fn build(c: &mut Collector, noto: &Noto, seed: u64) {
    let s = c.story;
    let n = noto;
    let img = |c: &mut Collector, path: &str, laid: Laid, tier: Tier, fmt: Img, purpose: &str| {
        let id = format!("d-{}", stem(path));
        image_file(c, noto, seed, path, &id, &laid, tier, fmt, purpose);
    };

    img(
        c,
        "synthetic/scan/invoice-bwl-003-clean.png",
        invoice_bwl(s, n, 3),
        Tier::clean(),
        Img::Png,
        "Scanned copy of Brightwater invoice 3 (clean tier, OCR accuracy reference)",
    );
    img(
        c,
        "synthetic/scan/invoice-bwl-004-office.jpg",
        invoice_bwl(s, n, 4),
        Tier::office(0.9),
        Img::Jpeg(OFFICE_Q),
        "Scanned copy of Brightwater invoice 4 (office tier)",
    );
    img(
        c,
        "synthetic/scan/invoice-bwl-005-poor.tif",
        invoice_bwl(s, n, 5),
        Tier::poor(-3.0),
        Img::Tiff,
        "Scanned copy of Brightwater invoice 5 (poor tier)",
    );
    img(
        c,
        "synthetic/scan/card-daniel-private.png",
        card(
            s,
            n,
            &[
                "{e:daniel}",
                "Private",
                "Mobile {i:ph_daniel_mob|intl}",
                "{i:em_daniel_priv}",
            ],
        ),
        Tier::clean(),
        Img::Png,
        "Business card: Daniel's private mobile and email",
    );
    img(
        c,
        "synthetic/scan/card-tomas-reinholt.jpg",
        card(
            s,
            n,
            &[
                "{e:tomas}",
                "Managing Partner, {e:kestrel}",
                "{i:ph_kestrel|intl}",
                "{i:em_tomas}",
                "{i:url_kestrel}",
            ],
        ),
        Tier::office(0.6),
        Img::Jpeg(OFFICE_Q),
        "Business card of the Kestrel partner",
    );
    img(
        c,
        "synthetic/scan/card-corvin-hale.tif",
        card(
            s,
            n,
            &[
                "{e:corvin}",
                "Director, {e:saltmarsh}",
                "{i:ph_saltmarsh|intl}",
                "{i:em_corvin}",
                "SOL {i:sol_corvin|hard}",
            ],
        ),
        Tier::poor(3.0),
        Img::Tiff,
        "Business card of the nominee director, with a Solana address in small type",
    );
    img(
        c,
        "synthetic/scan/note-wallets.png",
        wallet_note(s, n),
        Tier::poor(-2.5),
        Img::Png,
        "Sticky note with two crypto addresses (poor tier, hard)",
    );
    let otc = otc_confirmation(s, n);
    img(
        c,
        "synthetic/scan/coralline-otc-confirmation.jpg",
        otc,
        Tier::office(-1.2),
        Img::Jpeg(OFFICE_Q),
        "OTC trade confirmation: Saltmarsh buys Bitcoin and USDT",
    );
    c.event(
        "ev-otc",
        "2025-02",
        "Saltmarsh buys Bitcoin and USDT from the Coralline Exchange OTC desk",
        "d-coralline-otc-confirmation",
        &["ex_client", "ex_seller"],
        "Trade settled in February 2025 against a euro payment from the client account.",
    );
    img(
        c,
        "synthetic/scan/appointment-letter-corvin.jpg",
        appointment_letter(s, n),
        Tier::poor(2.5),
        Img::Jpeg(POOR_Q),
        "Letter appointing the nominee director (poor tier)",
    );
    c.relation(
        RelationType::DirectorOf,
        "d-appointment-letter-corvin",
        "app_person",
        "app_org",
    );
    img(
        c,
        "synthetic/scan/northgate-bid-cover.tif",
        northgate_bid(s, n),
        Tier::clean(),
        Img::Tiff,
        "Cover sheet of the honest bid that lost",
    );
    c.relation(
        RelationType::DirectorOf,
        "d-northgate-bid-cover",
        "bid_person",
        "bid_org",
    );
    img(
        c,
        "synthetic/scan/certificate-incorporation-bwl.jpg",
        incorporation(s, n),
        Tier::office(1.1),
        Img::Jpeg(OFFICE_Q),
        "Certificate of incorporation of Brightwater, formed by Kestrel",
    );
    img(
        c,
        "synthetic/scan/visitor-log-2024-06.tif",
        visitor_log(s, n),
        Tier::poor(-2.8),
        Img::Tiff,
        "Visitor log: Brightwater and Kestrel visit Daniel before the award",
    );
    img(
        c,
        "synthetic/scan/remittance-advice-vpa-001.tif",
        remittance(s, n),
        Tier::clean(),
        Img::Tiff,
        "Port Authority remittance advice for the first Brightwater payment",
    );
    c.relation(
        RelationType::Paid,
        "d-remittance-advice-vpa-001",
        "ra_payer",
        "ra_payee",
    );
    img(
        c,
        "synthetic/scan/transfer-kestrel-saltmarsh.png",
        transfer(s, n),
        Tier::poor(3.0),
        Img::Png,
        "Payment confirmation: Kestrel pays Saltmarsh",
    );
    c.relation(
        RelationType::Paid,
        "d-transfer-kestrel-saltmarsh",
        "tc_payer",
        "tc_payee",
    );
    c.event(
        "ev-kestrel-saltmarsh",
        "2025-01-15",
        "Kestrel Advisory transfers EUR 240,000 to Saltmarsh Holdings",
        "d-transfer-kestrel-saltmarsh",
        &["tc_payer", "tc_payee"],
        "On 15 January 2025 the ordering customer sent EUR 240,000.00 to the beneficiary.",
    );
    img(
        c,
        "synthetic/scan/phone-message-slip.jpg",
        message_slip(s, n),
        Tier::office(0.4),
        Img::Jpeg(OFFICE_Q),
        "Phone message slip: the bank's compliance officer calls the clerk",
    );

    // Image-only PDFs.
    let tender = tender_notice(s, n);
    scan_pdf(
        c,
        noto,
        seed,
        "synthetic/scan-pdf/tender-notice-031.pdf",
        "d-tender-notice-031",
        &tender,
        &[Tier::clean()],
        None,
        "Scanned contract notice for tender 031 (image-only PDF)",
    );
    c.event(
        "ev-tender",
        "2024-05-20",
        "Port Authority publishes tender VPA-TND-2024-031",
        "d-tender-notice-031",
        &["tn_vpa", "tn_ref"],
        "Published 20 May 2024. Bids are due by 21 June 2024.",
    );
    let agreement = service_agreement(s, n);
    scan_pdf(
        c,
        noto,
        seed,
        "synthetic/scan-pdf/service-agreement-kestrel-saltmarsh.pdf",
        "d-service-agreement-kestrel-saltmarsh",
        &agreement,
        &[Tier::office(0.7), Tier::office(-0.5)],
        Some(OFFICE_Q),
        "Scanned two-page agreement: Kestrel pays Saltmarsh a retainer (JPEG pages)",
    );
    c.relation(
        RelationType::WorksFor,
        "d-service-agreement-kestrel-saltmarsh",
        "sa_tomas",
        "sa_firm",
    );
    c.relation(
        RelationType::DirectorOf,
        "d-service-agreement-kestrel-saltmarsh",
        "sa_corvin",
        "sa_consultant",
    );
    scan_pdf(
        c,
        noto,
        seed,
        "synthetic/scan-pdf/invoice-bwl-006-poor.pdf",
        "d-scan-invoice-bwl-006",
        &invoice_bwl(s, n, 6),
        &[Tier::poor(3.0)],
        None,
        "Scanned copy of Brightwater invoice 6 (poor tier, image-only PDF)",
    );
    scan_pdf(
        c,
        noto,
        seed,
        "synthetic/scan-pdf/meridian-alert-memo.pdf",
        "d-meridian-alert-memo",
        &alert_memo(s, n),
        &[Tier::office(-1.0)],
        Some(OFFICE_Q),
        "Bank compliance memo on the Brightwater account (scanned)",
    );
    c.relation(
        RelationType::Paid,
        "d-meridian-alert-memo",
        "am_payer",
        "am_payee",
    );

    // Mixed: text page(s) followed by scanned page(s).
    let (text, scanned) = audit_annex(s, n);
    mixed_pdf(
        c,
        noto,
        seed,
        "synthetic/mixed/audit-annex-b.pdf",
        "d-audit-annex-b",
        &text,
        &scanned,
        Tier::office(0.8),
        Some(OFFICE_Q),
        "Audit annex: typed cover page plus a scanned bank statement",
    );
    let (text, scanned) = tender_bids(s, n);
    mixed_pdf(
        c,
        noto,
        seed,
        "synthetic/mixed/tender-bids-031.pdf",
        "d-tender-bids-031",
        &text,
        &scanned,
        Tier::clean(),
        None,
        "Bid summary (text) plus the scanned Brightwater bid form",
    );
    c.relation(
        RelationType::DirectorOf,
        "d-tender-bids-031",
        "bf_irene",
        "bf_bwl",
    );

    // Scans with an invisible OCR text layer.
    ocr_pdf(
        c,
        noto,
        seed,
        "synthetic/ocr-layer/annual-return-saltmarsh.pdf",
        "d-annual-return-saltmarsh",
        &annual_return(s, n),
        Tier::clean(),
        None,
        "Registry annual return with an OCR text layer: Corvin owns Saltmarsh",
    );
    c.relation(
        RelationType::Owns,
        "d-annual-return-saltmarsh",
        "ar_owner",
        "ar_company",
    );
    ocr_pdf(
        c,
        noto,
        seed,
        "synthetic/ocr-layer/letter-priya-to-audit.pdf",
        "d-letter-priya-to-audit",
        &priya_letter(s, n),
        Tier::office(0.6),
        Some(OFFICE_Q),
        "The clerk's report to internal audit, scanned with an OCR text layer",
    );
    c.relation(
        RelationType::SentTo,
        "d-letter-priya-to-audit",
        "pl_priya",
        "pl_rosalind",
    );
}

fn encode(sc: &Scanned, fmt: Img) -> Vec<u8> {
    match fmt {
        Img::Png => raster::png_gray(sc.width, sc.height, &sc.pixels, sc.dpi),
        Img::Jpeg(q) => raster::jpeg_gray(sc.width, sc.height, &sc.pixels, sc.dpi, q),
        Img::Tiff => raster::tiff_gray(sc.width, sc.height, &sc.pixels, sc.dpi),
    }
}

#[allow(clippy::too_many_arguments)]
fn image_file(
    c: &mut Collector,
    noto: &Noto,
    seed: u64,
    path: &str,
    id: &str,
    laid: &Laid,
    tier: Tier,
    fmt: Img,
    purpose: &str,
) {
    assert_eq!(laid.pages.len(), 1, "{path}: image files hold one page");
    let sc = raster::scan(laid, 0, noto, &tier, file_seed(seed, path), 1);
    let bytes = encode(&sc, fmt);
    c.write_file(
        path,
        &bytes,
        FileMeta::new(FileKind::Image, purpose)
            .pages(1)
            .scan(tier.tier, tier.dpi),
    );
    c.whole(id, path, FileKind::Image);
    c.ingest(id, sc.placed);
}

/// A PDF page that shows one scanned image over the whole page.
fn image_page(width: f64, height: f64, sc: &Scanned, jpeg: Option<u8>) -> Page {
    let data = match jpeg {
        Some(q) => ImageData::Jpeg(raster::jpeg_gray(
            sc.width, sc.height, &sc.pixels, sc.dpi, q,
        )),
        None => ImageData::Gray(sc.pixels.clone()),
    };
    Page {
        width,
        height,
        images: vec![PageImage {
            width_px: sc.width,
            height_px: sc.height,
            data,
            rect: [0.0, 0.0, width, height],
        }],
        ..Page::default()
    }
}

/// Scans every page of `laid` (`tiers[i]`, or the last tier for later pages).
fn scan_pages(
    noto: &Noto,
    seed: u64,
    path: &str,
    laid: &Laid,
    tiers: &[Tier],
    first_page: u32,
) -> Vec<Scanned> {
    (0..laid.pages.len())
        .map(|i| {
            let tier = &tiers[i.min(tiers.len() - 1)];
            let page_no = first_page + i as u32;
            raster::scan(
                laid,
                i,
                noto,
                tier,
                file_seed(seed, &format!("{path}#{page_no}")),
                page_no,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn scan_pdf(
    c: &mut Collector,
    noto: &Noto,
    seed: u64,
    path: &str,
    id: &str,
    laid: &Laid,
    tiers: &[Tier],
    jpeg: Option<u8>,
    purpose: &str,
) {
    let mut out = Laid::default();
    let mut placed = Placed::default();
    for (i, sc) in scan_pages(noto, seed, path, laid, tiers, 1)
        .into_iter()
        .enumerate()
    {
        out.pages.push(image_page(
            laid.pages[i].width,
            laid.pages[i].height,
            &sc,
            jpeg,
        ));
        placed.extend(sc.placed);
    }
    let bytes = pdf::render(&out, PdfOpts::new(stem(path)));
    let t = &tiers[0];
    c.write_file(
        path,
        &bytes,
        FileMeta::new(FileKind::PdfScan, purpose)
            .pages(out.pages.len())
            .scan(t.tier, t.dpi),
    );
    c.whole(id, path, FileKind::PdfScan);
    c.ingest(id, placed);
}

#[allow(clippy::too_many_arguments)]
fn mixed_pdf(
    c: &mut Collector,
    noto: &Noto,
    seed: u64,
    path: &str,
    id: &str,
    text: &Laid,
    scanned: &Laid,
    tier: Tier,
    jpeg: Option<u8>,
    purpose: &str,
) {
    let mut out = Laid {
        pages: text.pages.clone(),
        marks: Vec::new(),
    };
    let mut placed = pdf::truth(text, &[]);
    let first = text.pages.len() as u32 + 1;
    for (i, sc) in scan_pages(
        noto,
        seed,
        path,
        scanned,
        std::slice::from_ref(&tier),
        first,
    )
    .into_iter()
    .enumerate()
    {
        out.pages.push(image_page(
            scanned.pages[i].width,
            scanned.pages[i].height,
            &sc,
            jpeg,
        ));
        placed.extend(sc.placed);
    }
    let bytes = pdf::render(&out, PdfOpts::new(stem(path)));
    c.write_file(
        path,
        &bytes,
        FileMeta::new(FileKind::PdfMixed, purpose)
            .pages(out.pages.len())
            .scan(tier.tier, tier.dpi),
    );
    c.whole(id, path, FileKind::PdfMixed);
    c.ingest(id, placed);
}

/// A scan with an invisible text layer: one hidden word per scanned word, scaled to its box.
/// Truth is the text layer in PDF points (what a text extractor sees).
#[allow(clippy::too_many_arguments)]
fn ocr_pdf(
    c: &mut Collector,
    noto: &Noto,
    seed: u64,
    path: &str,
    id: &str,
    laid: &Laid,
    tier: Tier,
    jpeg: Option<u8>,
    purpose: &str,
) {
    let mut out = Laid::default();
    let mut placed = Placed::default();
    for (i, sc) in scan_pages(noto, seed, path, laid, std::slice::from_ref(&tier), 1)
        .into_iter()
        .enumerate()
    {
        let k = 72.0 / sc.dpi as f64;
        let to_pt = |b: &BBox| BBox(b.0.map(|v| round2(v * k)));
        let mut page = image_page(laid.pages[i].width, laid.pages[i].height, &sc, jpeg);
        let mut p = sc.placed;
        for pt in &mut p.pages {
            for w in &mut pt.words {
                w.b = to_pt(&w.b);
                page.hidden.push(HiddenWord {
                    text: w.t.clone(),
                    rect: w.b.0,
                });
            }
            pt.unit = Unit::Pt;
            pt.width = page.width;
            pt.height = page.height;
            pt.dpi = None;
        }
        for (_, loc) in &mut p.mentions {
            if let Location::Scan { page, boxes, .. } = loc {
                *loc = Location::PdfText {
                    page: *page,
                    boxes: boxes.iter().map(to_pt).collect(),
                };
            }
        }
        out.pages.push(page);
        placed.extend(p);
    }
    let bytes = pdf::render(&out, PdfOpts::new(stem(path)));
    c.write_file(
        path,
        &bytes,
        FileMeta::new(FileKind::PdfOcrLayer, purpose)
            .pages(out.pages.len())
            .scan(tier.tier, tier.dpi),
    );
    c.whole(id, path, FileKind::PdfOcrLayer);
    c.ingest(id, placed);
}

/// A 3.5 × 2 inch business card.
fn card(s: &Story, m: &Noto, lines: &[&str]) -> Laid {
    let mut f = Flow::new(m, (252.0, 144.0), 16.0);
    f.para(Font::SansBold, 11.0, &s.t(lines[0]));
    f.gap(4.0);
    for l in &lines[1..] {
        f.para(Font::Sans, 7.0, &s.t(l));
    }
    f.laid
}

fn wallet_note(s: &Story, m: &Noto) -> Laid {
    let mut f = Flow::new(m, (216.0, 216.0), 14.0);
    f.para(Font::SansBold, 10.0, &plain("for the marina"));
    f.gap(8.0);
    f.para(Font::Sans, 8.0, &plain("BTC"));
    f.para(Font::Mono, 7.0, &s.t("{i:btc_daniel|hard}"));
    f.gap(6.0);
    f.para(Font::Sans, 8.0, &plain("ETH / USDT"));
    f.para(Font::Mono, 7.0, &s.t("{i:evm_daniel|eip55|hard}"));
    f.gap(8.0);
    f.para(
        Font::Sans,
        8.0,
        &plain("send after each payment, not before"),
    );
    f.laid
}

fn form(s: &Story, m: &Noto, title: &[&str], rows: &[(&str, &str)], body: &[&str]) -> Laid {
    let mut f = Flow::new(m, a4(), 64.0);
    f.para(Font::SansBold, 15.0, &s.t(title[0]));
    for t in &title[1..] {
        f.para(Font::Sans, 10.0, &s.t(t));
    }
    f.gap(8.0);
    f.hrule();
    f.gap(8.0);
    for (k, v) in rows {
        f.row(&[64.0, 210.0], Font::Sans, 10.0, &[plain(k), s.t(v)]);
    }
    f.gap(12.0);
    for b in body {
        f.para(Font::Serif, 10.5, &s.t(b));
        f.gap(6.0);
    }
    f.laid
}

fn otc_confirmation(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &["{e:exchange}", "OTC Desk · Trade confirmation TC-25-0219"],
        &[
            ("Trade date", "19 February 2025"),
            ("Client", "{e:saltmarsh|SALTMARSH HOLDINGS S.A.|#ex_client}"),
            ("Signatory", "{e:corvin|C. Hale}"),
            ("Bought", "2.40000000 BTC for EUR 196,320.00"),
            ("Delivered to", "{i:btc_saltmarsh}"),
            ("Bought", "25,000.00 USDT (TRC-20) for EUR 23,150.00"),
            ("Delivered to", "{i:tron_saltmarsh}"),
        ],
        &[
            "Trade settled in February 2025 against a euro payment from the client account.",
            "{e:exchange|Coralline|#ex_seller} acted as principal. Please check the addresses above; transfers on chain cannot be reversed.",
        ],
    )
}

fn appointment_letter(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &[
            "{e:kestrel|KESTREL ADVISORY LLP}",
            "Suite 9, 220 Canal Street, Velmora VL3 8BD",
        ],
        &[("Date", "1 March 2024"), ("Our ref.", "KA/NOM/0042")],
        &[
            "Dear {e:corvin|Mr Hale},",
            "We confirm your appointment as nominee director {e:corvin|#app_person} of {e:saltmarsh|Saltmarsh Holdings|#app_org} with effect from 1 March 2024. You will act on the written instructions of this firm and will receive a fixed fee each quarter.",
            "Please return the signed consent form and keep this letter with the company papers.",
            "Yours sincerely,",
            "{e:tomas|T. Reinholt}",
        ],
    )
}

fn northgate_bid(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &[
            "{e:northgate|NORTHGATE MARINE SUPPLIES INC.}",
            "Bid for tender {i:tnd_031}",
        ],
        &[
            ("Bidder", "{e:northgate|Northgate Marine Supplies|#bid_org}"),
            ("Contact", "{e:gregor|#bid_person}, Director"),
            ("Telephone", "{i:ph_gregor|intl}"),
            ("Email", "{i:em_gregor}"),
            ("Bank", "IBAN {i:iban_gregor}"),
            ("Total price", "£1,118,000.00"),
        ],
        &[
            "We confirm that this bid stays valid for 90 days from the closing date and that we have no conflict of interest with any officer of the contracting authority.",
        ],
    )
}

fn incorporation(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &["{e:registry}", "Certificate of Incorporation"],
        &[
            ("Company name", "{e:bwl|BRIGHTWATER LOGISTICS LTD}"),
            ("Company number", "VC-0418822"),
            ("Incorporated on", "12 February 2024"),
            ("Company type", "Private limited company"),
            ("Formation agent", "{e:kestrel}"),
            ("Presented by", "{e:tomas|T. Reinholt}"),
            ("Online record", "{i:url_registry}"),
        ],
        &[
            "The Registrar certifies that the company named above is incorporated under the Companies Act and that it is a limited company.",
        ],
    )
}

fn visitor_log(s: &Story, m: &Noto) -> Laid {
    let mut f = Flow::new(m, a4(), 56.0);
    f.para(
        Font::SansBold,
        14.0,
        &plain("Harbour House · Visitor log · June 2024"),
    );
    f.gap(8.0);
    let cols = [56.0, 130.0, 270.0, 400.0];
    f.row(
        &cols,
        Font::SansBold,
        10.0,
        &[
            plain("Date"),
            plain("Visitor"),
            plain("Company"),
            plain("Host"),
        ],
    );
    f.hrule();
    for (d, v, co, h) in [
        (
            "03/06/2024",
            "{e:irene|Irene Vasquez}",
            "{e:bwl|Brightwater}",
            "{e:daniel|D. Okonkwo-Hart}",
        ),
        (
            "11/06/2024",
            "{e:tomas|T. Reinholt}",
            "{e:kestrel|Kestrel}",
            "{e:daniel|D. Okonkwo-Hart}",
        ),
        (
            "18/06/2024",
            "{e:gregor|G. Almqvist}",
            "{e:northgate|Northgate Marine}",
            "Procurement desk",
        ),
        (
            "24/06/2024",
            "{e:irene|Irene Vasquez}",
            "{e:bwl|Brightwater}",
            "{e:daniel|D. Okonkwo-Hart}",
        ),
        ("27/06/2024", "Courier", "Parcel delivery", "Reception"),
    ] {
        f.row(
            &cols,
            Font::Sans,
            10.0,
            &[plain(d), s.t(v), s.t(co), s.t(h)],
        );
        f.gap(6.0);
    }
    f.laid
}

fn remittance(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &[
            "{e:vpa|VELMORA PORT AUTHORITY}",
            "Remittance advice RA-2024-0811",
        ],
        &[
            ("Payer", "{e:vpa|#ra_payer}"),
            ("From account", "{i:iban_vpa}"),
            ("Payee", "{e:bwl|#ra_payee}"),
            ("To account", "{i:iban_bwl|compact}"),
            ("Invoice", "BWL-2024-001"),
            ("Amount", "£148,500.00"),
            ("Payment date", "13 August 2024"),
        ],
        &["Questions about this payment: {i:em_ap} or {i:ph_vpa|national}."],
    )
}

fn transfer(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &["Payment confirmation", "SEPA credit transfer · executed"],
        &[
            (
                "Ordering customer",
                "{e:kestrel|KESTREL ADVISORY LLP|#tc_payer}",
            ),
            ("Debit account", "{i:iban_kestrel}"),
            (
                "Beneficiary",
                "{e:saltmarsh|SALTMARSH HOLDINGS S.A.|#tc_payee}",
            ),
            ("Credit account", "{i:iban_saltmarsh}"),
            ("Value date", "15 January 2025"),
            ("Amount", "EUR 240,000.00"),
            ("Reference", "CONSULTANCY RETAINER Q4"),
        ],
        &["On 15 January 2025 the ordering customer sent EUR 240,000.00 to the beneficiary."],
    )
}

fn message_slip(s: &Story, m: &Noto) -> Laid {
    let mut f = Flow::new(m, (297.0, 210.0), 18.0);
    f.para(Font::SansBold, 12.0, &plain("While you were out"));
    f.gap(6.0);
    for l in [
        "For: {e:priya}",
        "From: {e:samuel}, {e:meridian|Meridian Trust}",
        "Tel. {i:ph_meridian|national}",
        "Message: please call back about the transfers from {e:bwl|Brightwater} to {e:kestrel|Kestrel}.",
        "Taken: 30 January 2025, 15:40",
    ] {
        f.para(Font::Sans, 9.0, &s.t(l));
        f.gap(2.0);
    }
    f.laid
}

fn tender_notice(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &[
            "{e:vpa|VELMORA PORT AUTHORITY|#tn_vpa}",
            "Contract notice {i:tnd_031|#tn_ref}",
        ],
        &[
            ("Title", "Dredging logistics coordination"),
            ("Type", "Framework agreement, 24 months"),
            ("Estimated value", "£1,300,000.00"),
            ("Procedure", "Open"),
        ],
        &[
            "Published 20 May 2024. Bids are due by 21 June 2024.",
            "Contact: {e:daniel|D. Okonkwo-Hart}, Director of Procurement, {i:em_daniel}, telephone {i:ph_vpa|intl}.",
            "Bidders must declare any family or business relationship with staff of the contracting authority.",
        ],
    )
}

fn service_agreement(s: &Story, m: &Noto) -> Laid {
    let mut f = Flow::new(m, a4(), 64.0);
    f.para(Font::SansBold, 15.0, &plain("CONSULTANCY AGREEMENT"));
    f.gap(10.0);
    for p in [
        "This agreement is made on 4 March 2024 between {e:kestrel|#sa_firm1} (\"the Firm\") and {e:saltmarsh|#sa_consultant1} (\"the Consultant\").",
        "1. Services. The Consultant shall provide market research and introductions in the port sector as the Firm requests from time to time.",
        "2. Fees. The Firm shall pay the Consultant a retainer each quarter, in an amount the parties agree in writing, to the account {i:iban_saltmarsh}.",
        "3. Confidentiality. Neither party shall disclose this agreement or the identity of the Firm's clients.",
        "4. Term. This agreement runs for two years and renews unless one party gives three months' notice.",
    ] {
        f.para(Font::Serif, 11.0, &s.t(p));
        f.gap(8.0);
    }
    f.new_page();
    for p in [
        "5. Law. This agreement is governed by the laws of Velmora.",
        "Signed for {e:kestrel|Kestrel Advisory|#sa_firm}: {e:tomas|#sa_tomas}, Managing Partner.",
        "Signed for {e:saltmarsh|Saltmarsh Holdings|#sa_consultant}: {e:corvin|#sa_corvin}, Director.",
        "Notices to the Consultant: {i:em_corvin}, {i:ph_saltmarsh|intl}, {i:url_saltmarsh}.",
    ] {
        f.para(Font::Serif, 11.0, &s.t(p));
        f.gap(8.0);
    }
    f.laid
}

fn alert_memo(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &[
            "{e:meridian|MERIDIAN TRUST BANK}",
            "Financial Crime Compliance · Internal memo",
        ],
        &[
            ("Date", "3 February 2025"),
            ("From", "{e:samuel}"),
            (
                "Customer",
                "{e:bwl|Brightwater Logistics Limited|#am_payer}",
            ),
            ("Account", "{i:iban_bwl}"),
        ],
        &[
            "Since August 2024 the account received six payments from {e:vpa} and sent most of each amount within days to {e:kestrel|Kestrel Advisory|#am_payee} at {i:iban_kestrel}.",
            "The pattern does not fit the customer's stated business. Recommendation: file a suspicious activity report and inform {e:vpa|VPA} internal audit at {i:em_audit}.",
        ],
    )
}

fn audit_annex(s: &Story, m: &Noto) -> (Laid, Laid) {
    let mut f = Flow::new(&Afm, a4(), 64.0);
    f.para(
        Font::SansBold,
        16.0,
        &plain("Annex B to Internal Audit Report IA-2025-04"),
    );
    f.gap(10.0);
    f.para(Font::Serif, 11.0, &s.t("Bank statement extract supplied by {e:meridian} under the audit request of 2 April 2025. The next page is a scan of the paper copy received."));
    f.gap(8.0);
    f.para(
        Font::Serif,
        11.0,
        &s.t("Prepared by {e:rosalind|R. Achebe-Frost}."),
    );
    let text = f.laid;

    let mut f = Flow::new(m, a4(), 56.0);
    f.para(
        Font::SansBold,
        14.0,
        &s.t("{e:meridian|MERIDIAN TRUST BANK} · Statement extract"),
    );
    f.para(
        Font::Sans,
        10.0,
        &s.t("Account holder: {e:bwl|BRIGHTWATER LOGISTICS LTD}"),
    );
    f.para(Font::Sans, 10.0, &s.t("IBAN: {i:iban_bwl}"));
    f.gap(10.0);
    let cols = [56.0, 130.0, 400.0];
    f.row(
        &cols,
        Font::SansBold,
        10.0,
        &[plain("Date"), plain("Details"), plain("Amount (GBP)")],
    );
    f.hrule();
    for (d, t, a) in [
        (
            "13/08/2024",
            "Credit {e:vpa|VELMORA PORT AUTHORITY}",
            "148,500.00",
        ),
        (
            "15/08/2024",
            "Debit {e:kestrel|KESTREL ADVISORY LLP}",
            "-141,000.00",
        ),
        (
            "12/09/2024",
            "Credit {e:vpa|VELMORA PORT AUTHORITY}",
            "212,750.00",
        ),
        (
            "16/09/2024",
            "Debit {e:kestrel|KESTREL ADVISORY LLP}",
            "-202,000.00",
        ),
        ("30/09/2024", "Account fee", "-25.00"),
    ] {
        f.row(&cols, Font::Sans, 10.0, &[plain(d), s.t(t), plain(a)]);
    }
    (text, f.laid)
}

fn tender_bids(s: &Story, m: &Noto) -> (Laid, Laid) {
    let mut f = Flow::new(&Afm, a4(), 64.0);
    f.para(
        Font::SansBold,
        16.0,
        &s.t("Tender {i:tnd_031} · Bids received"),
    );
    f.gap(10.0);
    let cols = [64.0, 360.0];
    f.row(
        &cols,
        Font::SansBold,
        10.5,
        &[plain("Bidder"), plain("Price")],
    );
    f.hrule();
    f.row(
        &cols,
        Font::Sans,
        10.5,
        &[s.t("{e:bwl}"), plain("£1,284,500.00")],
    );
    f.row(
        &cols,
        Font::Sans,
        10.5,
        &[s.t("{e:northgate}"), plain("£1,118,000.00")],
    );
    f.gap(12.0);
    f.para(Font::Serif, 11.0, &s.t("Bids opened by {e:daniel} on 24 June 2024. The bid form of {e:bwl|Brightwater} follows as a scan."));
    let text = f.laid;

    let scanned = form(
        s,
        m,
        &["Bid form · tender {i:tnd_031}"],
        &[
            ("Bidder", "{e:bwl|Brightwater Logistics Limited|#bf_bwl}"),
            ("Signed by", "{e:irene|#bf_irene}, Director"),
            ("Telephone", "{i:ph_bwl_office|intl}"),
            ("Email", "{i:em_irene|mixed}"),
            ("Price", "£1,284,500.00"),
        ],
        &[
            "We declare that no director of the bidder has a family or business relationship with staff of the contracting authority.",
        ],
    );
    (text, scanned)
}

fn annual_return(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &["{e:registry}", "Annual return 2024"],
        &[
            (
                "Company",
                "{e:saltmarsh|SALTMARSH HOLDINGS S.A.|#ar_company}",
            ),
            ("Registered agent", "{e:kestrel}"),
            ("Director", "{e:corvin|C. Hale}"),
            ("Shareholder", "{e:corvin|#ar_owner}, 100 shares (100%)"),
            ("Email", "{i:em_corvin}"),
            ("Website", "{i:url_saltmarsh}"),
        ],
        &["The company states that the information above is correct at the date of this return."],
    )
}

fn priya_letter(s: &Story, m: &Noto) -> Laid {
    form(
        s,
        m,
        &["Report to Internal Audit"],
        &[
            ("From", "{e:priya|#pl_priya}, Accounts Payable"),
            ("To", "{e:rosalind|Ms Achebe-Frost|#pl_rosalind}"),
            ("Date", "27 March 2025"),
        ],
        &[
            "I am writing about the invoices from {e:bwl|Brightwater}. From invoice BWL-2024-003 on, the accounts phone number is {i:ph_daniel_mob|national}, which our staff list gives as the mobile of {e:daniel|Mr Okonkwo-Hart}.",
            "I have kept copies of all six invoices. Please reply to {i:em_priya}.",
        ],
    )
}
