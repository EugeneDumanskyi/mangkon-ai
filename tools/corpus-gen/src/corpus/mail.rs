//! Email: single EML files and two MBOX mailboxes. Covers plain, multipart/alternative,
//! HTML-only, quoted-printable, base64, a Windows-1252 part, RFC 2047 subjects and PDF
//! attachments (child documents, INV-CO-05).

use mk_bench::{DocPart, FileKind, RelationType};

use super::{invoice_bwl, stem};
use crate::fonts::Afm;
use crate::formats::{AttachmentOut, Charset, Cte, Msg, Part, email, mbox};
use crate::layout::Laid;
use crate::pdf::{self, PdfOpts};
use crate::rich::Rich;
use crate::story::Story;
use crate::truth::{Collector, FileMeta};

struct Head<'a> {
    from: &'a str,
    to: &'a str,
    cc: Option<&'a str>,
    date: &'a str,
    subject: &'a str,
    /// Message-ID local part; the domain is always `localhost`.
    id: &'a str,
}

fn headers(s: &Story, h: &Head) -> Vec<(&'static str, Rich)> {
    let mut v = vec![("From", s.t(h.from)), ("To", s.t(h.to))];
    if let Some(cc) = h.cc {
        v.push(("Cc", s.t(cc)));
    }
    v.push(("Date", Rich::plain_text(h.date)));
    v.push(("Subject", s.t(h.subject)));
    v.push((
        "Message-ID",
        Rich::plain_text(&format!("<{}@localhost>", h.id)),
    ));
    v
}

fn text(s: &Story, subtype: &'static str, charset: Charset, cte: Cte, body: &str) -> Part {
    Part::Text {
        subtype,
        charset,
        cte,
        rich: s.t(body),
    }
}

/// A PDF attachment and the layout it was drawn from (for its ground truth).
struct Att {
    filename: String,
    laid: Laid,
}

impl Att {
    fn part(&self) -> Part {
        Part::Attachment {
            content_type: "application/pdf",
            filename: self.filename.clone(),
            bytes: self.bytes(),
        }
    }
    fn bytes(&self) -> Vec<u8> {
        pdf::render(&self.laid, PdfOpts::new(stem(&self.filename)))
    }
}

fn bwl_invoice_att(s: &Story, n: usize) -> Att {
    Att {
        filename: format!("invoice-bwl-2024-{n:03}.pdf"),
        laid: invoice_bwl(s, &Afm, n),
    }
}

/// Records the attachments of a message as child documents of `parent`.
fn children(c: &mut Collector, file: &str, parent: &str, outs: &[AttachmentOut], atts: &[Att]) {
    for a in outs {
        let att = atts
            .iter()
            .find(|x| x.filename == a.filename)
            .expect("attachment layout");
        let id = format!("{parent}-att{}", a.part.replace('.', "-"));
        let part = DocPart::Attachment {
            mime_part: a.part.clone(),
            filename: a.filename.clone(),
        };
        c.doc(
            &id,
            file,
            part,
            FileKind::PdfText,
            Some(parent),
            Some(mk_bench::sha256_hex(&a.bytes)),
        );
        c.ingest(&id, pdf::truth(&att.laid, &[]));
    }
}

fn eml(c: &mut Collector, path: &str, msg: Msg, atts: &[Att], purpose: &str) {
    let id = format!("d-{}", stem(path));
    let out = email(&msg, stem(path), "\r\n");
    c.write_file(path, &out.bytes, FileMeta::new(FileKind::Eml, purpose));
    c.whole(&id, path, FileKind::Eml);
    c.ingest(&id, out.placed);
    children(c, path, &id, &out.attachments, atts);
}

/// One MBOX message: envelope date (asctime), message, attachments.
struct Boxed {
    envelope: &'static str,
    msg: Msg,
    atts: Vec<Att>,
}

fn mbox_file(c: &mut Collector, path: &str, messages: Vec<Boxed>, purpose: &str) {
    let base = format!("d-{}", stem(path));
    let mut raw = Vec::new();
    let mut outs = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let out = email(&m.msg, &format!("{}-{i}", stem(path)), "\n");
        raw.push((m.envelope.to_string(), out.bytes.clone()));
        outs.push(out);
    }
    c.write_file(path, &mbox(&raw), FileMeta::new(FileKind::Mbox, purpose));
    for (i, (out, m)) in outs.into_iter().zip(&messages).enumerate() {
        let id = format!("{base}-m{i}");
        c.doc(
            &id,
            path,
            DocPart::MboxMessage { index: i as u32 },
            FileKind::Mbox,
            None,
            None,
        );
        c.ingest(&id, out.placed);
        children(c, path, &id, &out.attachments, &m.atts);
    }
}

pub fn build(c: &mut Collector) {
    let s = c.story;
    use Charset::{Utf8, Windows1252};
    use Cte::{Base64, QuotedPrintable as Qp, SevenBit};

    eml(
        c,
        "synthetic/email/2024-01-29-irene-to-tomas-formation.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:irene} <{i:em_irene}>",
                    to: "{e:tomas} <{i:em_tomas}>",
                    cc: None,
                    date: "Mon, 29 Jan 2024 10:02:00 +0000",
                    subject: "New company",
                    id: "20240129100200.irene",
                },
            ),
            body: Part::Multipart {
                subtype: "alternative",
                parts: vec![
                    text(
                        s,
                        "plain",
                        Utf8,
                        Qp,
                        "Dear {e:tomas|Mr Reinholt},\n\nas discussed, please form a new private company for logistics services. The name should be {e:bwl}. I will be the sole director; the shares go to {e:saltmarsh}.\n\nPlease use your office as the formation agent so that no other name appears on the register.\n\nKind regards,\n{e:irene|#rel_irene}\n\nP.S. {e:daniel|#rel_daniel} is my brother-in-law. He must not appear on any paper.",
                    ),
                    text(
                        s,
                        "html",
                        Utf8,
                        Qp,
                        "<p>Dear {e:tomas|Mr Reinholt},</p>\n<p>as discussed, please form a new private company for logistics services. The name should be <b>{e:bwl}</b>. I will be the sole director; the shares go to {e:saltmarsh}.</p>\n<p>Please use your office as the formation agent so that no other name appears on the register.</p>\n<p>Kind regards,<br>{e:irene}</p>",
                    ),
                ],
            },
        },
        &[],
        "Irene asks Kestrel to form Brightwater; names her brother-in-law (multipart/alternative, QP)",
    );
    c.relation(
        RelationType::RelatedTo,
        "d-2024-01-29-irene-to-tomas-formation",
        "rel_irene",
        "rel_daniel",
    );

    eml(
        c,
        "synthetic/email/2024-05-21-daniel-to-irene-tender.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{i:em_daniel_priv}",
                    to: "{e:irene|Irene Vasquez} <{i:em_irene}>",
                    cc: None,
                    date: "Tue, 21 May 2024 21:47:00 +0100",
                    subject: "the notice is out",
                    id: "20240521214700.d",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                SevenBit,
                "{e:irene},\n\nthe notice for {i:tnd_031} went up yesterday. Bid on the logistics lot and keep the price just above 1.28m. Use the office line {i:ph_bwl_office|intl} on the forms, never my number.\n\nDelete this.\nD.",
            ),
        },
        &[],
        "Daniel's private address tips off Brightwater about the tender (plain 7bit)",
    );

    eml(
        c,
        "synthetic/email/2024-06-25-award-notice.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:daniel} <{i:em_daniel}>",
                    to: "{i:em_irene}",
                    cc: Some("{i:em_gregor}"),
                    date: "Tue, 25 Jun 2024 16:05:00 +0100",
                    subject: "Tender {i:tnd_031} – result",
                    id: "20240625160500.award",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                Base64,
                "Dear bidders,\n\nthe evaluation of tender {i:tnd_031} is complete. The contract is awarded to {e:bwl|Brightwater Logistics Limited}. {e:northgate|Northgate Marine Supplies} may ask for a debrief within ten days.\n\nRegards,\n{e:daniel|D. Okonkwo-Hart}\nDirector of Procurement, {e:vpa}\n{i:ph_vpa|intl}",
            ),
        },
        &[],
        "Award notice (RFC 2047 subject, base64 UTF-8 body)",
    );

    eml(
        c,
        "synthetic/email/2024-07-10-contract-signed.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:daniel|Mr Okonkwo-Hart} <{i:em_daniel|mixed}>",
                    to: "{i:em_irene|mixed}",
                    cc: None,
                    date: "Wed, 10 Jul 2024 12:30:00 +0100",
                    subject: "Framework FA-2024-117 signed",
                    id: "20240710123000.fa",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                SevenBit,
                "{e:irene|Ms Vasquez-Lindqvist},\n\nthe framework agreement FA-2024-117 is signed by both sides. Please send invoices to {i:em_ap} and quote the contract number.\n\n{e:daniel|D. Okonkwo-Hart}",
            ),
        },
        &[],
        "Mixed-case email addresses (same keys as the lower-case forms)",
    );

    let att = vec![bwl_invoice_att(s, 1)];
    eml(
        c,
        "synthetic/email/2024-08-02-bwl-invoice-001.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:bwl|Brightwater Logistics} Accounts <{i:em_bwl_accounts}>",
                    to: "{i:em_ap}",
                    cc: None,
                    date: "Fri, 2 Aug 2024 08:15:00 +0100",
                    subject: "Invoice BWL-2024-001",
                    id: "20240802081500.bwl001",
                },
            ),
            body: Part::Multipart {
                subtype: "mixed",
                parts: vec![
                    text(
                        s,
                        "plain",
                        Utf8,
                        SevenBit,
                        "Please find attached invoice BWL-2024-001 under framework FA-2024-117.\n\n{e:bwl} Accounts\n{i:ph_bwl_office|national}",
                    ),
                    att[0].part(),
                ],
            },
        },
        &att,
        "Invoice email with the PDF attached (child document, same bytes as the PDF file)",
    );

    eml(
        c,
        "synthetic/email/2024-11-05-irene-to-tomas-transfer.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:irene|Irene Vasquez} <{i:em_irene}>",
                    to: "{i:em_tomas}",
                    cc: None,
                    date: "Tue, 5 Nov 2024 18:20:00 +0000",
                    subject: "Re: fees",
                    id: "20241105182000.irene",
                },
            ),
            body: text(
                s,
                "plain",
                Windows1252,
                Qp,
                "{e:tomas|T. Reinholt},\n\nthe Port paid invoice 4 today. Please raise your fee note for €212,000 to {e:bwl|Brightwater} and I will pay it to {i:iban_kestrel} this week.\n\nWe can meet at the café on Canal Street on Thursday.\n\n{e:irene}",
            ),
        },
        &[],
        "Windows-1252 quoted-printable part (euro sign, accented letters)",
    );

    eml(
        c,
        "synthetic/email/2024-12-03-tomas-to-corvin.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:tomas} <{i:em_tomas}>",
                    to: "{e:corvin} <{i:em_corvin}>",
                    cc: None,
                    date: "Tue, 3 Dec 2024 09:40:00 +0000",
                    subject: "Retainer Q4",
                    id: "20241203094000.tomas",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                Qp,
                "{e:corvin|Mr Hale},\n\n{e:kestrel|#kp_payer} will pay the Q4 consultancy retainer to {e:saltmarsh|Saltmarsh Holdings|#kp_payee} in January, to {i:iban_saltmarsh|compact}. Please send the usual consultancy invoice first; the text can stay as last quarter.\n\nBest,\n{e:tomas|T. Reinholt}\n{i:ph_kestrel|national}",
            ),
        },
        &[],
        "Kestrel announces the onward payment to Saltmarsh",
    );
    c.relation(
        RelationType::Paid,
        "d-2024-12-03-tomas-to-corvin",
        "kp_payer",
        "kp_payee",
    );

    eml(
        c,
        "synthetic/email/2025-01-20-daniel-wallets.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{i:em_daniel_priv}",
                    to: "{i:em_corvin}",
                    cc: None,
                    date: "Mon, 20 Jan 2025 22:11:00 +0000",
                    subject: "addresses",
                    id: "20250120221100.d",
                },
            ),
            body: text(
                s,
                "html",
                Utf8,
                Qp,
                "<html><body>\n<p>For the marina, as agreed. Bitcoin only to this one:</p>\n<p><code>{i:btc_daniel}</code></p>\n<p>USDT on Ethereum to <code>{i:evm_daniel|eip55}</code></p>\n<p>Nothing to my bank account, please.</p>\n<p>D.</p>\n</body></html>",
            ),
        },
        &[],
        "HTML-only email from the private address with two crypto addresses",
    );

    eml(
        c,
        "synthetic/email/2025-01-30-samuel-to-priya.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:samuel} <{i:em_samuel}>",
                    to: "{e:priya} <{i:em_priya}>",
                    cc: None,
                    date: "Thu, 30 Jan 2025 16:02:00 +0000",
                    subject: "Payments to {e:bwl|Brightwater} account",
                    id: "20250130160200.samuel",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                SevenBit,
                "Dear {e:priya|P. Ramanathan-Cole},\n\nfollowing my call: can you confirm that the six payments to {i:iban_bwl} were made under a valid contract? We see most of each amount leave the account within days.\n\n{e:samuel|S. Achterberg}\nCompliance, {e:meridian}\n{i:ph_meridian|intl}",
            ),
        },
        &[],
        "The bank asks the clerk about the payments",
    );

    eml(
        c,
        "synthetic/email/2025-02-21-corvin-to-daniel.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:corvin|C. Hale} <{i:em_corvin}>",
                    to: "{i:em_daniel_priv}",
                    cc: None,
                    date: "Fri, 21 Feb 2025 11:30:00 +0000",
                    subject: "done",
                    id: "20250221113000.corvin",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                SevenBit,
                "Sent from {i:btc_saltmarsh} to your address this morning. The USDT went out from {i:tron_saltmarsh}.\n\nC.",
            ),
        },
        &[],
        "Saltmarsh pays Daniel in crypto (Bitcoin bech32 and TRON addresses)",
    );

    eml(
        c,
        "synthetic/email/2025-03-27-priya-report.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:priya} <{i:em_priya}>",
                    to: "Internal Audit <{i:em_audit}>",
                    cc: None,
                    date: "Thu, 27 Mar 2025 17:55:00 +0000",
                    subject: "{e:bwl|Brightwater} invoices – phone number",
                    id: "20250327175500.priya",
                },
            ),
            body: Part::Multipart {
                subtype: "alternative",
                parts: vec![
                    text(
                        s,
                        "plain",
                        Utf8,
                        Qp,
                        "Dear {e:rosalind|Ms Achebe-Frost},\n\nfrom invoice 3 on, the accounts phone number on the invoices of {e:bwl|Brightwater} is {i:ph_daniel_mob|intl}. Our staff list gives this number as the mobile of {e:daniel|Mr Okonkwo-Hart}.\n\nOn the paper copy of invoice 5 the IBAN reads {n:neg_iban_typo}, which looks like a typing error.\n\nAlso, an expense claim in the same file shows the test card number {n:neg_card_visa}; I think it is a placeholder.\n\n{e:priya}",
                    ),
                    text(
                        s,
                        "html",
                        Utf8,
                        Qp,
                        "<p>Dear {e:rosalind|Ms Achebe-Frost},</p><p>from invoice 3 on, the accounts phone number on the invoices of {e:bwl|Brightwater} is {i:ph_daniel_mob|intl}. Our staff list gives this number as the mobile of {e:daniel|Mr Okonkwo-Hart}.</p><p>{e:priya}</p>",
                    ),
                ],
            },
        },
        &[],
        "The clerk's report: the key clue, a bad-checksum IBAN and a test card number (negatives)",
    );

    eml(
        c,
        "synthetic/email/2025-04-02-rosalind-to-samuel.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:rosalind} <{i:em_audit}>",
                    to: "{e:samuel} <{i:em_samuel}>",
                    cc: None,
                    date: "Wed, 2 Apr 2025 08:45:00 +0100",
                    subject: "Audit request: account records",
                    id: "20250402084500.audit",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                SevenBit,
                "Dear {e:samuel|S. Achterberg},\n\nunder our audit powers, please send the statements of the accounts of {e:bwl} and {e:kestrel} for August 2024 to February 2025. Please use this address only, not a personal {n:neg_stop_gmail} or {n:neg_stop_outlook} account.\n\n{e:rosalind|R. Achebe-Frost}\nChief Internal Auditor, {e:vpa}",
            ),
        },
        &[],
        "Audit request; mentions stop-list webmail domains (negatives)",
    );

    eml(
        c,
        "synthetic/email/2025-04-15-gregor-complaint.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:gregor} <{i:em_gregor}>",
                    to: "{i:em_daniel}",
                    cc: Some("{i:em_audit}"),
                    date: "Tue, 15 Apr 2025 10:10:00 -0400",
                    subject: "Tender {i:tnd_007}",
                    id: "20250415101000.gregor",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                SevenBit,
                "Dear {e:daniel|Mr Okonkwo-Hart},\n\nfor the second time our lower bid has lost to {e:bwl|Brightwater Logistics}. We ask for the evaluation scores of both bids for {i:tnd_007}.\n\n{e:gregor|G. Almqvist}\n{e:northgate}\n{i:ph_gregor|national}",
            ),
        },
        &[],
        "The losing bidder complains (background, own identifiers)",
    );

    eml(
        c,
        "synthetic/email/2025-04-28-lena-press-query.eml",
        Msg {
            headers: headers(
                s,
                &Head {
                    from: "{e:lena} <{i:em_lena}>",
                    to: "{i:em_audit}",
                    cc: None,
                    date: "Mon, 28 Apr 2025 14:25:00 +0100",
                    subject: "Press query: port logistics contracts",
                    id: "20250428142500.lena",
                },
            ),
            body: text(
                s,
                "plain",
                Utf8,
                Qp,
                "Hello,\n\nI am a reporter at {e:harbourpost}. We plan to publish a story on Friday about the contracts of the Port Authority with {e:bwl} and the role of {e:kestrel|Kestrel Advisory}. Can the authority comment on why {e:northgate|Northgate Marine} lost with a lower bid?\n\n{e:lena|L. Hartwig}\n{i:ph_lena|intl}",
            ),
        },
        &[],
        "Press query before the article",
    );

    mbox_file(
        c,
        "synthetic/mbox/irene-sent-2024.mbox",
        vec![
            Boxed {
                envelope: "Mon Feb 12 15:00:00 2024",
                msg: Msg {
                    headers: headers(
                        s,
                        &Head {
                            from: "{e:irene} <{i:em_irene}>",
                            to: "{i:em_tomas}",
                            cc: None,
                            date: "Mon, 12 Feb 2024 15:00:00 +0000",
                            subject: "Re: New company",
                            id: "20240212150000.irene",
                        },
                    ),
                    body: text(
                        s,
                        "plain",
                        Utf8,
                        SevenBit,
                        "{e:tomas|T. Reinholt},\n\nthank you, the certificate arrived. From now on please write to the company address {i:em_bwl_accounts} for invoices.\n\nFrom next month the website will be {i:url_bwl}.\n\n{e:irene}",
                    ),
                },
                atts: vec![],
            },
            Boxed {
                envelope: "Thu Jun 20 19:12:00 2024",
                msg: Msg {
                    headers: headers(
                        s,
                        &Head {
                            from: "{e:irene} <{i:em_irene}>",
                            to: "{i:em_daniel_priv}",
                            cc: None,
                            date: "Thu, 20 Jun 2024 19:12:00 +0100",
                            subject: "bid",
                            id: "20240620191200.irene",
                        },
                    ),
                    body: text(
                        s,
                        "plain",
                        Utf8,
                        Qp,
                        "The bid went in today at £1,284,500. The office number is on the form.\n\nI.\n",
                    ),
                },
                atts: vec![],
            },
            Boxed {
                envelope: "Mon Dec  2 09:05:00 2024",
                msg: Msg {
                    headers: headers(
                        s,
                        &Head {
                            from: "{e:bwl} <{i:em_bwl_accounts}>",
                            to: "{i:em_ap}",
                            cc: None,
                            date: "Mon, 2 Dec 2024 09:05:00 +0000",
                            subject: "Invoice BWL-2024-005",
                            id: "20241202090500.bwl005",
                        },
                    ),
                    body: text(
                        s,
                        "plain",
                        Utf8,
                        SevenBit,
                        "Invoice BWL-2024-005 follows by post.\nFrom our side all work for phase 5 is done.\n\nAccounts, {e:bwl|Brightwater}\n{i:ph_daniel_mob|national}",
                    ),
                },
                atts: vec![],
            },
        ],
        "Sent folder of Brightwater (MBOX, mboxrd quoting of a body line that starts with \"From \")",
    );

    let att6 = vec![bwl_invoice_att(s, 6)];
    mbox_file(
        c,
        "synthetic/mbox/vpa-payables-2025.mbox",
        vec![
            Boxed {
                envelope: "Mon Jan  6 08:30:00 2025",
                msg: Msg {
                    headers: headers(
                        s,
                        &Head {
                            from: "{e:bwl|Brightwater Logistics} Accounts <{i:em_bwl_accounts}>",
                            to: "{i:em_ap}",
                            cc: None,
                            date: "Mon, 6 Jan 2025 08:30:00 +0000",
                            subject: "Invoice BWL-2024-006",
                            id: "20250106083000.bwl006",
                        },
                    ),
                    body: Part::Multipart {
                        subtype: "mixed",
                        parts: vec![
                            text(
                                s,
                                "plain",
                                Utf8,
                                SevenBit,
                                "Please find attached invoice BWL-2024-006 for tender {i:tnd_007}.\n\nAccounts, {e:bwl|Brightwater}",
                            ),
                            att6[0].part(),
                        ],
                    },
                },
                atts: att6,
            },
            Boxed {
                envelope: "Tue Jan 14 11:00:00 2025",
                msg: Msg {
                    headers: headers(
                        s,
                        &Head {
                            from: "{e:northgate|Northgate Marine Supplies} <{i:em_gregor}>",
                            to: "{i:em_ap}",
                            cc: None,
                            date: "Tue, 14 Jan 2025 11:00:00 -0500",
                            subject: "Statement of account",
                            id: "20250114110000.northgate",
                        },
                    ),
                    body: text(
                        s,
                        "plain",
                        Utf8,
                        SevenBit,
                        "Our statement for 2024 shows three invoices, all paid. Thank you.\nPayments to {i:iban_gregor} as before.\n\n{e:gregor|G. Almqvist}",
                    ),
                },
                atts: vec![],
            },
            Boxed {
                envelope: "Fri Mar 28 09:00:00 2025",
                msg: Msg {
                    headers: headers(
                        s,
                        &Head {
                            from: "{e:priya} <{i:em_priya}>",
                            to: "{i:em_ap}",
                            cc: None,
                            date: "Fri, 28 Mar 2025 09:00:00 +0000",
                            subject: "Fwd: {e:bwl|Brightwater} invoices – phone number",
                            id: "20250328090000.priya",
                        },
                    ),
                    body: text(
                        s,
                        "plain",
                        Utf8,
                        Qp,
                        "For the file. I sent this to internal audit yesterday.\n\n> from invoice 3 on, the accounts phone number on the invoices\n> is {i:ph_daniel_mob|intl}.\n\n{e:priya|P. Ramanathan-Cole}",
                    ),
                },
                atts: vec![],
            },
        ],
        "Accounts-payable inbox (MBOX) with a PDF attachment inside a message",
    );
}
