//! TD-1 synthetic documents. Each submodule writes one group of files; see
//! `docs/07-test-corpus.md` §4 for the file list.

mod data;
mod mail;
mod notes;
mod office;
mod pdfs;
mod scans;

use mk_bench::FileKind;
use sha2::{Digest, Sha256};

use crate::layout::{A4, Laid};
use crate::pdf::{self, PdfOpts};
use crate::rich::Rich;
use crate::truth::{Collector, FileMeta};

pub use pdfs::invoice_bwl;

pub fn build(c: &mut Collector, noto: &crate::fonts::Noto, seed: u64) {
    pdfs::build(c);
    scans::build(c, noto, seed);
    mail::build(c);
    data::build(c);
    office::build(c);
    notes::build(c);
}

/// Per-file seed: the corpus seed mixed with the file path.
pub fn file_seed(seed: u64, path: &str) -> u64 {
    let h = Sha256::digest([&seed.to_le_bytes()[..], path.as_bytes()].concat());
    u64::from_le_bytes(h[..8].try_into().unwrap())
}

pub fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap();
    name.split('.').next().unwrap()
}

/// Writes a text PDF and records its document and truth.
pub fn text_pdf(c: &mut Collector, path: &str, id: &str, laid: &Laid, purpose: &str) {
    let bytes = pdf::render(laid, PdfOpts::new(stem(path)));
    c.write_file(
        path,
        &bytes,
        FileMeta::new(FileKind::PdfText, purpose).pages(laid.pages.len()),
    );
    c.whole(id, path, FileKind::PdfText);
    c.ingest(id, pdf::truth(laid, &[]));
}

pub fn a4() -> (f64, f64) {
    A4
}

pub fn plain(s: &str) -> Rich {
    Rich::plain_text(s)
}
