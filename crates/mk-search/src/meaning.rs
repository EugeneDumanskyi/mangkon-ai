//! BC-10 Search, meaning part (M6b, US-06.2, ADR-0010): what is stored and scanned, as pure
//! functions.
//!
//! - [`plan_windows`] cuts a segment's stored text into windows of at most [`WINDOW_CHARS`]
//!   characters at whitespace, with no overlap. One vector per window; each is an exact character
//!   span of the text (INV-CO-04), so a result opens at its place.
//! - [`plan_items`] groups whole adjacent segments of one document into work items of up to
//!   [`ITEM_WINDOWS`] windows.
//! - [`normalize`], [`encode_f16`], [`decode_f16`], [`sign_bits`]: unit vectors, the stored half
//!   floats (the source of truth) and the 1-bit codes of the index, which come from the sign alone
//!   so the stored vector and the query give the same code.
//! - [`scan_top_k`] (Hamming distance, in parallel) and [`rescore_top`] (cosine over the pool).
//! - [`IndexFile`]: the derived index file. Its header names the run and generation it was built
//!   from, and a checksum covers the body: anything that does not match means "rebuild".

use half::f16;
use mk_kernel::ContentHash;

/// The most characters of one window (one vector).
pub const WINDOW_CHARS: usize = 500;

/// The most windows of one work item, unless a single segment has more (decided in M6b planning).
pub const ITEM_WINDOWS: usize = 32;

/// How many results a query shows (US-06.2 AC1).
pub const TOP_K: usize = 20;

/// The Hamming pool that is rescored: 50× oversampling. ADR-0010's 10× (200) reached recall@20
/// 0.836 on TD-2, this one 0.9875 (S6, `15-meaning-search.md`).
pub const RESCORE_POOL: usize = 1000;

/// The character spans of `text`'s windows, in order: at most [`WINDOW_CHARS`] characters each,
/// cut at whitespace, no overlap, no window that is only whitespace. A word longer than a window
/// is cut inside. Blank text gives none.
pub fn plan_windows(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    // The window being filled: its first character and the end of its last word.
    let mut window: Option<(usize, usize)> = None;
    let mut word_start: Option<usize> = None;
    let mut at = 0usize;
    let mut word = |start: usize, end: usize, out: &mut Vec<(usize, usize)>| {
        let mut start = start;
        // A word that cannot fit in any window: whole windows of it, then the rest goes on.
        if end - start > WINDOW_CHARS {
            if let Some(w) = window.take() {
                out.push(w);
            }
            while end - start > WINDOW_CHARS {
                out.push((start, start + WINDOW_CHARS));
                start += WINDOW_CHARS;
            }
        }
        match &mut window {
            Some((first, last)) if end - *first <= WINDOW_CHARS => *last = end,
            Some(w) => {
                out.push(*w);
                *w = (start, end);
            }
            None => window = Some((start, end)),
        }
    };
    for c in text.chars() {
        if c.is_whitespace() {
            if let Some(s) = word_start.take() {
                word(s, at, &mut out);
            }
        } else if word_start.is_none() {
            word_start = Some(at);
        }
        at += 1;
    }
    if let Some(s) = word_start {
        word(s, at, &mut out);
    }
    if let Some(w) = window {
        out.push(w);
    }
    out
}

/// Characters `start..end` of `text`.
pub fn char_slice(text: &str, start: usize, end: usize) -> &str {
    let mut bytes = text.char_indices().map(|(i, _)| i).chain([text.len()]);
    let Some(from) = bytes.nth(start) else {
        return "";
    };
    if end <= start {
        return "";
    }
    let to = bytes.nth(end - start - 1).unwrap_or(text.len());
    &text[from..to]
}

/// A planned work item: whole adjacent segments `first..=last` with `windows` windows in all.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ItemPlan {
    pub first: i64,
    pub last: i64,
    pub windows: usize,
}

/// Fills work items of at most [`ITEM_WINDOWS`] windows from one document's segments, pushed in
/// order. A segment is never split between items, so one long segment may exceed the limit alone.
/// A segment with no windows joins the item before it when it fits, and no item is only those.
#[derive(Debug, Default)]
pub struct ItemPlanner {
    current: Option<ItemPlan>,
    items: Vec<ItemPlan>,
}

impl ItemPlanner {
    pub fn new() -> ItemPlanner {
        ItemPlanner::default()
    }

    pub fn push(&mut self, segment: i64, windows: usize) {
        if let Some(item) = &mut self.current {
            if item.windows + windows <= ITEM_WINDOWS {
                item.last = segment;
                item.windows += windows;
                return;
            }
            self.items.push(*item);
            self.current = None;
        }
        if windows > 0 {
            self.current = Some(ItemPlan {
                first: segment,
                last: segment,
                windows,
            });
        }
    }

    pub fn finish(mut self) -> Vec<ItemPlan> {
        self.items.extend(self.current.take());
        self.items
    }
}

/// [`ItemPlanner`] over `(segment id, window count)` pairs.
pub fn plan_items(segments: impl IntoIterator<Item = (i64, usize)>) -> Vec<ItemPlan> {
    let mut planner = ItemPlanner::new();
    for (segment, windows) in segments {
        planner.push(segment, windows);
    }
    planner.finish()
}

/// Scales `v` to length 1; a zero vector stays as it is.
pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

/// A vector as stored: little-endian half floats (ADR-0010; M1 found no loss of recall).
pub fn encode_f16(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 2);
    for x in v {
        out.extend_from_slice(&f16::from_f32(*x).to_le_bytes());
    }
    out
}

/// A stored vector back to floats. `None` if `dims` does not fit its length.
pub fn decode_f16(bytes: &[u8], dims: usize) -> Option<Vec<f32>> {
    if bytes.len() != dims * 2 {
        return None;
    }
    Some(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f16::from_le_bytes(*b).to_f32())
            .collect(),
    )
}

/// Dot product. On unit vectors it is the cosine similarity.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 64-bit words of one code of `dims` bits.
pub fn code_words(dims: usize) -> usize {
    dims.div_ceil(64)
}

/// The code of a vector: one bit per dimension, set when negative.
pub fn sign_bits(v: &[f32]) -> Vec<u64> {
    let mut out = vec![0u64; code_words(v.len())];
    for (i, x) in v.iter().enumerate() {
        if x.is_sign_negative() {
            out[i / 64] |= 1 << (i % 64);
        }
    }
    out
}

/// The code of a stored vector, added to `out`, without decoding it: the sign is the high bit of
/// each half float, so this equals [`sign_bits`] of the vector it came from.
pub fn push_sign_bits_f16(bytes: &[u8], out: &mut Vec<u64>) {
    let dims = bytes.len() / 2;
    let at = out.len();
    out.resize(at + code_words(dims), 0);
    for (i, b) in bytes.as_chunks::<2>().0.iter().enumerate() {
        if b[1] & 0x80 != 0 {
            out[at + i / 64] |= 1 << (i % 64);
        }
    }
}

fn hamming(a: &[u64], b: &[u64]) -> u32 {
    a.iter().zip(b).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// The `k` smallest of `hits` by (distance, row), sorted.
fn smallest(mut hits: Vec<(u32, u32)>, k: usize) -> Vec<(u32, u32)> {
    if hits.len() > k {
        hits.select_nth_unstable(k);
        hits.truncate(k);
    }
    hits.sort_unstable();
    hits
}

/// The rows of the `k` codes nearest to `query` by Hamming distance, nearest first, ties by row.
/// `codes` holds `query.len()` words per row. Rows are scanned on `threads` threads.
pub fn scan_top_k(codes: &[u64], query: &[u64], k: usize, threads: usize) -> Vec<u32> {
    let words = query.len();
    if words == 0 || k == 0 {
        return Vec::new();
    }
    let rows = codes.len() / words;
    let threads = threads.clamp(1, rows.max(1));
    let per = rows.div_ceil(threads).max(1);
    let scan = |chunk: usize, part: &[u64]| {
        // At most 2k candidates at a time. Rows come in order, so once k are kept, a row at or past
        // the k-th distance can never enter: an earlier row wins the tie.
        let mut best: Vec<(u32, u32)> = Vec::with_capacity(2 * k);
        let mut limit = u32::MAX;
        for (i, code) in part.chunks_exact(words).enumerate() {
            let d = hamming(code, query);
            if d < limit {
                best.push((d, (chunk * per + i) as u32));
                if best.len() == 2 * k {
                    best = smallest(std::mem::take(&mut best), k);
                    limit = best.last().map_or(u32::MAX, |b| b.0);
                }
            }
        }
        smallest(best, k)
    };
    let found: Vec<Vec<(u32, u32)>> = std::thread::scope(|s| {
        let handles: Vec<_> = codes
            .chunks(per * words)
            .enumerate()
            .map(|(chunk, part)| {
                let scan = &scan;
                s.spawn(move || scan(chunk, part))
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("a scan thread does not panic"))
            .collect()
    });
    smallest(found.into_iter().flatten().collect(), k)
        .into_iter()
        .map(|(_, row)| row)
        .collect()
}

/// `(id, vector)` pairs to the `k` most similar to the unit vector `query`: `(id, similarity)`,
/// best first, ties by id.
pub fn rescore_top(query: &[f32], vectors: &[(i64, Vec<f32>)], k: usize) -> Vec<(i64, f32)> {
    let mut scored: Vec<(i64, f32)> = vectors.iter().map(|(id, v)| (*id, dot(query, v))).collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.truncate(k);
    scored
}

const MAGIC: &[u8; 8] = b"MKVECIDX";
const VERSION: u32 = 1;
/// magic + version + dims + run + generation + count + checksum.
const HEADER_BYTES: usize = 8 + 4 + 4 + 8 + 8 + 8 + 32;

/// What an index file is built from. A file whose header differs is rebuilt (ADR-0010).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IndexHeader {
    pub dims: u32,
    pub run: i64,
    /// The project's embedding generation when the vectors were read.
    pub generation: u64,
    pub count: u64,
}

impl IndexHeader {
    /// How long a whole file with this header is.
    pub fn file_bytes(&self) -> Option<u64> {
        let words = code_words(self.dims as usize) as u64;
        let row = 8u64.checked_add(words.checked_mul(8)?)?;
        row.checked_mul(self.count)?
            .checked_add(HEADER_BYTES as u64)
    }
}

/// The derived index in memory: rows in embedding id order, ids beside codes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IndexFile {
    pub header: IndexHeader,
    /// `segment_embedding.id` of each row.
    pub ids: Vec<i64>,
    /// [`code_words`]`(dims)` words per row.
    pub codes: Vec<u64>,
}

/// Why an index file cannot be used; each means "rebuild".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IndexFileError {
    Header,
    Length,
    Checksum,
}

impl IndexFile {
    pub fn body(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity((self.ids.len() + self.codes.len()) * 8);
        for id in &self.ids {
            body.extend_from_slice(&id.to_le_bytes());
        }
        for word in &self.codes {
            body.extend_from_slice(&word.to_le_bytes());
        }
        body
    }

    /// The whole file.
    pub fn encode(&self) -> Vec<u8> {
        let body = self.body();
        let mut out = encode_header(&self.header, &ContentHash::of(&body));
        out.extend_from_slice(&body);
        out
    }

    /// The header alone, from the start of a file (to compare it before reading the rest).
    pub fn read_header(bytes: &[u8]) -> Result<IndexHeader, IndexFileError> {
        if bytes.len() < HEADER_BYTES || &bytes[..8] != MAGIC {
            return Err(IndexFileError::Header);
        }
        let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
        let u64_at = |i: usize| u64::from_le_bytes(bytes[i..i + 8].try_into().expect("8 bytes"));
        if u32_at(8) != VERSION {
            return Err(IndexFileError::Header);
        }
        Ok(IndexHeader {
            dims: u32_at(12),
            run: u64_at(16) as i64,
            generation: u64_at(24),
            count: u64_at(32),
        })
    }

    /// How many bytes of a file the header takes.
    pub const fn header_bytes() -> usize {
        HEADER_BYTES
    }

    /// A whole file back, checked.
    pub fn decode(bytes: &[u8]) -> Result<IndexFile, IndexFileError> {
        let header = Self::read_header(bytes)?;
        if header.file_bytes() != Some(bytes.len() as u64) {
            return Err(IndexFileError::Length);
        }
        let stored = &bytes[40..HEADER_BYTES];
        let body = &bytes[HEADER_BYTES..];
        if ContentHash::of(body).as_bytes().as_slice() != stored {
            return Err(IndexFileError::Checksum);
        }
        let (ids, codes) = body.split_at(header.count as usize * 8);
        Ok(IndexFile {
            header,
            ids: ids
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| i64::from_le_bytes(*b))
                .collect(),
            codes: codes
                .as_chunks::<8>()
                .0
                .iter()
                .map(|b| u64::from_le_bytes(*b))
                .collect(),
        })
    }
}

fn encode_header(h: &IndexHeader, checksum: &ContentHash) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_BYTES);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&h.dims.to_le_bytes());
    out.extend_from_slice(&h.run.to_le_bytes());
    out.extend_from_slice(&h.generation.to_le_bytes());
    out.extend_from_slice(&h.count.to_le_bytes());
    out.extend_from_slice(checksum.as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn slices(text: &str) -> Vec<String> {
        plan_windows(text)
            .into_iter()
            .map(|(s, e)| char_slice(text, s, e).to_string())
            .collect()
    }

    #[test]
    fn blank_text_has_no_windows() {
        assert!(plan_windows("").is_empty());
        assert!(plan_windows("  \n\t ").is_empty());
    }

    #[test]
    fn a_short_row_is_kept() {
        assert_eq!(slices("  Ana  "), vec!["Ana"]);
    }

    #[test]
    fn cuts_at_whitespace_and_counts_characters() {
        let word = "é".repeat(300);
        let text = format!("{word} {word} {word}");
        let w = plan_windows(&text);
        assert_eq!(w, vec![(0, 300), (301, 601), (602, 902)]);
    }

    #[test]
    fn exactly_a_window_stays_one() {
        let text = format!("{} {}", "a".repeat(249), "b".repeat(250));
        assert_eq!(plan_windows(&text), vec![(0, 500)]);
    }

    #[test]
    fn a_long_word_is_cut_inside() {
        let text = format!("hi {} there", "x".repeat(1201));
        let w = plan_windows(&text);
        assert_eq!(w[0], (0, 2));
        assert_eq!(w[1], (3, 503));
        assert_eq!(w[2], (503, 1003));
        assert_eq!(w[3], (1003, 1204 + 6));
    }

    #[test]
    fn items_keep_segments_whole() {
        let items = plan_items([(1, 10), (2, 10), (3, 12), (4, 1), (5, 40), (6, 1)]);
        assert_eq!(
            items,
            vec![
                ItemPlan {
                    first: 1,
                    last: 3,
                    windows: 32
                },
                ItemPlan {
                    first: 4,
                    last: 4,
                    windows: 1
                },
                ItemPlan {
                    first: 5,
                    last: 5,
                    windows: 40
                },
                ItemPlan {
                    first: 6,
                    last: 6,
                    windows: 1
                },
            ]
        );
    }

    #[test]
    fn segments_without_windows_make_no_item() {
        assert!(plan_items([(1, 0), (2, 0)]).is_empty());
        assert_eq!(
            plan_items([(1, 0), (2, 3), (3, 0)]),
            vec![ItemPlan {
                first: 2,
                last: 3,
                windows: 3
            }]
        );
    }

    #[test]
    fn zero_vector_stays_zero() {
        let mut v = [0.0f32; 4];
        normalize(&mut v);
        assert_eq!(v, [0.0; 4]);
    }

    /// NFR-PERF-31 rests on this: the scan is exact on the codes.
    #[test]
    fn nfr_perf_31_scan_is_ordered_by_distance_then_row() {
        let codes = [0b1111u64, 0b0001, 0b0011, 0b0000, 0b0001];
        assert_eq!(scan_top_k(&codes, &[0], 3, 2), vec![3, 1, 4]);
        assert_eq!(scan_top_k(&codes, &[0], 9, 4), vec![3, 1, 4, 2, 0]);
        assert!(scan_top_k(&[], &[0], 3, 2).is_empty());
    }

    #[test]
    fn nfr_perf_31_rescore_puts_the_most_similar_first() {
        let q = [1.0, 0.0];
        let vectors = vec![
            (7, vec![0.0, 1.0]),
            (8, vec![1.0, 0.0]),
            (9, vec![0.6, 0.8]),
            (10, vec![0.6, 0.8]),
        ];
        let top = rescore_top(&q, &vectors, 3);
        assert_eq!(top.iter().map(|t| t.0).collect::<Vec<_>>(), vec![8, 9, 10]);
        assert!((top[0].1 - 1.0).abs() < 1e-6);
    }

    fn sample_index() -> IndexFile {
        IndexFile {
            header: IndexHeader {
                dims: 70,
                run: 5,
                generation: 9,
                count: 2,
            },
            ids: vec![3, 7],
            codes: vec![1, 2, 3, 4],
        }
    }

    #[test]
    fn index_file_round_trip() {
        let index = sample_index();
        let bytes = index.encode();
        assert_eq!(bytes.len() as u64, index.header.file_bytes().unwrap());
        assert_eq!(IndexFile::decode(&bytes).unwrap(), index);
        assert_eq!(IndexFile::read_header(&bytes).unwrap(), index.header);
    }

    #[test]
    fn a_damaged_index_file_is_refused() {
        let bytes = sample_index().encode();
        assert_eq!(
            IndexFile::decode(&bytes[..bytes.len() - 1]),
            Err(IndexFileError::Length)
        );
        assert_eq!(IndexFile::decode(&bytes[..10]), Err(IndexFileError::Header));
        let mut flipped = bytes.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert_eq!(IndexFile::decode(&flipped), Err(IndexFileError::Checksum));
        let mut other = bytes;
        other[0] = b'X';
        assert_eq!(IndexFile::decode(&other), Err(IndexFileError::Header));
    }

    proptest! {
        /// INV-CO-04: every window is an exact span; together they hold every word, in order.
        #[test]
        fn windows_are_exact_spans(text in "(\\PC{0,40}[ \\n\\t]{0,3}){0,60}") {
            let spans = plan_windows(&text);
            let mut last = 0;
            for &(s, e) in &spans {
                prop_assert!(s < e);
                prop_assert!(e - s <= WINDOW_CHARS);
                prop_assert!(s >= last);
                last = e;
                let w = char_slice(&text, s, e);
                prop_assert_eq!(w.chars().count(), e - s);
                prop_assert!(!w.starts_with(char::is_whitespace));
                prop_assert!(!w.ends_with(char::is_whitespace));
            }
            let joined: String = spans
                .iter()
                .map(|&(s, e)| char_slice(&text, s, e))
                .collect::<Vec<_>>()
                .join("");
            let words: String = text.split_whitespace().collect();
            let got: String = joined.split_whitespace().collect();
            prop_assert_eq!(got, words);
        }

        #[test]
        fn long_words_stay_exact(n in 0usize..2000, m in 0usize..700) {
            let text = format!("{} {}", "x".repeat(n), "y".repeat(m));
            let spans = plan_windows(&text);
            let got: String = spans.iter().map(|&(s, e)| char_slice(&text, s, e)).collect();
            let got: String = got.split_whitespace().collect();
            prop_assert_eq!(got, format!("{}{}", "x".repeat(n), "y".repeat(m)));
            prop_assert!(spans.iter().all(|&(s, e)| e - s <= WINDOW_CHARS));
        }

        /// Every window goes in exactly one item, and no item takes more than its limit unless it
        /// is one segment.
        #[test]
        fn items_cover_every_segment_once(counts in proptest::collection::vec(0usize..50, 0..60)) {
            let items = plan_items(counts.iter().enumerate().map(|(i, &c)| (i as i64, c)));
            let total: usize = items.iter().map(|i| i.windows).sum();
            prop_assert_eq!(total, counts.iter().sum::<usize>());
            let mut next = 0i64;
            for item in &items {
                prop_assert!(item.first >= next);
                prop_assert!(item.last >= item.first);
                next = item.last + 1;
                let inside: usize = counts[item.first as usize..=item.last as usize].iter().sum();
                prop_assert_eq!(inside, item.windows);
                prop_assert!(
                    item.windows <= ITEM_WINDOWS
                        || counts[item.first as usize..=item.last as usize]
                            .iter()
                            .filter(|&&c| c > 0)
                            .count()
                            == 1
                );
            }
        }

        /// The stored form loses at most half-float rounding, and keeps the sign.
        #[test]
        fn f16_round_trip(v in proptest::collection::vec(-1.0f32..1.0, 1..200)) {
            let bytes = encode_f16(&v);
            let back = decode_f16(&bytes, v.len()).unwrap();
            for (a, b) in v.iter().zip(&back) {
                prop_assert!((a - b).abs() < 1e-3);
                prop_assert_eq!(a.is_sign_negative(), b.is_sign_negative());
            }
            prop_assert_eq!(decode_f16(&bytes, v.len() + 1), None);
        }

        /// The code from the stored bytes equals the code of the float: query and index agree.
        #[test]
        fn stored_code_equals_float_code(v in proptest::collection::vec(-1.0f32..1.0, 1..200)) {
            let mut stored = Vec::new();
            push_sign_bits_f16(&encode_f16(&v), &mut stored);
            prop_assert_eq!(stored, sign_bits(&v));
        }

        /// NFR-PERF-31: the threaded scan equals a naive one.
        #[test]
        fn nfr_perf_31_scan_equals_naive(
            codes in proptest::collection::vec(any::<u64>(), 0..300),
            query in any::<u64>(),
            k in 1usize..40,
            threads in 1usize..8,
        ) {
            // A narrow spread of distances, so ties are common.
            let codes: Vec<u64> = codes.into_iter().map(|c| c & 0xff).collect();
            let query = [query & 0xff];
            let mut naive: Vec<(u32, u32)> = codes
                .iter()
                .enumerate()
                .map(|(i, c)| ((c ^ query[0]).count_ones(), i as u32))
                .collect();
            naive.sort_unstable();
            naive.truncate(k);
            let want: Vec<u32> = naive.into_iter().map(|n| n.1).collect();
            prop_assert_eq!(scan_top_k(&codes, &query, k, threads), want);
        }

        #[test]
        fn scan_handles_several_words_per_code(
            rows in proptest::collection::vec(any::<[u64; 3]>(), 0..80),
            query in any::<[u64; 3]>(),
        ) {
            let codes: Vec<u64> = rows.iter().flatten().copied().collect();
            let mut naive: Vec<(u32, u32)> = rows
                .iter()
                .enumerate()
                .map(|(i, r)| (hamming(r, &query), i as u32))
                .collect();
            naive.sort_unstable();
            naive.truncate(10);
            let want: Vec<u32> = naive.into_iter().map(|n| n.1).collect();
            prop_assert_eq!(scan_top_k(&codes, &query, 10, 3), want);
        }
    }
}
