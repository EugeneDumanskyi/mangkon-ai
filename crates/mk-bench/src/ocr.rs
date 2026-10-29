//! OCR character accuracy (NFR-QA-07): 1 − character error rate against the reference page
//! text, whitespace-normalised, reading-order neutral. Ported from spike S0 (M0c,
//! `08-engine-bakeoff.md` §13), where the pass mark was measured.

/// Characters of `s` with every run of whitespace as one space, ends trimmed.
pub fn norm(s: &str) -> Vec<char> {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .collect()
}

/// Edit distance (insert, delete, substitute; each costs 1).
pub fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != cb) as usize)
                .min(prev[j + 1] + 1)
                .min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The OCR lines, each moved to the reference line it matches (edit distance as a share of the
/// longer line). Pairs are taken cheapest first, one OCR line per reference line, so repeated
/// lines each find their own place; OCR lines left over (a reference line split in two) join
/// their best match. Ties keep OCR order. Only the OCR side is permuted, whole lines at a time,
/// so the result is still a real edit distance, without counting reading order.
pub fn reordered(got: &str, reference: &str) -> String {
    let lines =
        |s: &str| -> Vec<Vec<char>> { s.lines().map(norm).filter(|l| !l.is_empty()).collect() };
    let (ocr, refs) = (lines(got), lines(reference));
    let cost = |a: &[char], b: &[char]| levenshtein(a, b) as f64 / a.len().max(b.len()) as f64;
    let mut pairs: Vec<(f64, usize, usize)> = (0..ocr.len())
        .flat_map(|i| (0..refs.len()).map(move |j| (i, j)))
        .map(|(i, j)| (cost(&ocr[i], &refs[j]), i, j))
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then((a.1, a.2).cmp(&(b.1, b.2))));
    let mut place: Vec<Option<usize>> = vec![None; ocr.len()];
    let mut taken = vec![false; refs.len()];
    for &(_, i, j) in &pairs {
        if place[i].is_none() && !taken[j] {
            (place[i], taken[j]) = (Some(j), true);
        }
    }
    for &(_, i, j) in &pairs {
        place[i].get_or_insert(j);
    }
    let mut order: Vec<(usize, usize)> = place
        .iter()
        .enumerate()
        .map(|(i, j)| (j.unwrap_or(0), i))
        .collect();
    order.sort();
    order
        .into_iter()
        .map(|(_, i)| ocr[i].iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Edits and reference characters of one page. A page that could not be OCR'd is `None`: every
/// reference character counts as wrong.
pub fn page_errors(got: Option<&str>, reference: &str) -> (usize, usize) {
    let r = norm(reference);
    let edits = match got {
        Some(g) => levenshtein(&norm(&reordered(g, reference)), &r),
        None => r.len(),
    };
    (edits, r.len())
}

/// 1 − CER over pages of `(edits, reference characters)`.
pub fn accuracy(pages: impl IntoIterator<Item = (usize, usize)>) -> f64 {
    let (e, n) = pages
        .into_iter()
        .fold((0, 0), |(e, n), (pe, pn)| (e + pe, n + pn));
    1.0 - e as f64 / n.max(1) as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &str) -> Vec<char> {
        s.chars().collect()
    }

    #[test]
    fn levenshtein_basics() {
        assert_eq!(levenshtein(&c("kitten"), &c("sitting")), 3);
        assert_eq!(levenshtein(&c(""), &c("abc")), 3);
    }

    #[test]
    fn reading_order_does_not_count() {
        let reference = "Invoice 4471\nTotal 12.00\nPaid";
        assert_eq!(
            page_errors(Some("Paid\nInvoice  4471\nTotal 12.00"), reference).0,
            0
        );
        assert_eq!(
            page_errors(Some("Invoice 4471\nTota1 12.00\nPaid"), reference).0,
            1
        );
        let (e, n) = page_errors(None, reference);
        assert_eq!(e, n);
        assert!((accuracy([(1, 100), (0, 100)]) - 0.995).abs() < 1e-9);
    }
}
