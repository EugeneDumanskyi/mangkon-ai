//! Crypto addresses (US-04.5): Bitcoin, EVM, TRON and Solana, each kept only when its checksum
//! or decoding holds (INV-ID-02).

use std::sync::LazyLock;

use mk_kernel::IdentifierKind;
use regex::Regex;
use sha3::{Digest, Keccak256};

use crate::{ByteHit, Claims};

/// The Base58 alphabet (no `0`, `O`, `I`, `l`).
const B58: &str = "[1-9A-HJ-NP-Za-km-z]";

static EVM: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b0x[0-9a-fA-F]{40}\b").expect("evm pattern compiles"));
/// Legacy (P2PKH `1…`) and P2SH (`3…`) addresses.
static BTC_B58: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r"\b[13]{B58}{{25,34}}\b")).expect("btc pattern compiles")
});
/// Segwit: `bc1` and the bech32 alphabet, one case.
static BTC_BECH32: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:bc1[02-9ac-hj-np-z]{11,71}|BC1[02-9AC-HJ-NP-Z]{11,71})\b")
        .expect("bech32 pattern compiles")
});
static TRON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\bT{B58}{{33}}\b")).expect("tron pattern compiles"));
static SOL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"\b{B58}{{32,44}}\b")).expect("sol pattern compiles"));
/// Words that make a Solana address likely (US-04.5 AC3).
static SOL_CUE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:wallet|sol|solana|address)\b").expect("cue pattern compiles")
});

/// Confidence of a Solana address without a cue word nearby (US-04.5 AC3, ASSUMPTION).
pub const SOL_UNCUED_CONFIDENCE: f32 = 0.7;
/// How far, in characters, a cue word may be from a Solana address.
const SOL_CUE_CHARS: usize = 50;

/// Base58Check with a 21-byte payload whose first byte is in `versions`.
fn base58check(s: &str, versions: &[u8]) -> bool {
    bs58::decode(s)
        .with_check(None)
        .into_vec()
        .is_ok_and(|v| v.len() == 21 && versions.contains(&v[0]))
}

fn btc_bech32(s: &str) -> bool {
    bech32::segwit::decode(s).is_ok_and(|(hrp, _, _)| hrp == bech32::hrp::BC)
}

/// EIP-55: all lower or all upper case passes; mixed case must match the checksum case.
fn evm_checksum_ok(hex: &str) -> bool {
    let lower = hex.to_ascii_lowercase();
    let mixed =
        hex.bytes().any(|c| c.is_ascii_lowercase()) && hex.bytes().any(|c| c.is_ascii_uppercase());
    if !mixed {
        return true;
    }
    let hash = Keccak256::digest(lower.as_bytes());
    lower
        .bytes()
        .zip(hex.bytes())
        .enumerate()
        .all(|(i, (l, c))| {
            let nibble = (hash[i / 2] >> if i % 2 == 0 { 4 } else { 0 }) & 0x0f;
            let want = if l.is_ascii_alphabetic() && nibble >= 8 {
                l.to_ascii_uppercase()
            } else {
                l
            };
            c == want
        })
}

fn sol_valid(s: &str) -> bool {
    bs58::decode(s).into_vec().is_ok_and(|v| v.len() == 32)
}

/// Whether a cue word lies within [`SOL_CUE_CHARS`] characters of `start..end`.
fn sol_cued(text: &str, start: usize, end: usize) -> bool {
    let from = text[..start]
        .char_indices()
        .rev()
        .nth(SOL_CUE_CHARS - 1)
        .map_or(0, |(i, _)| i);
    let to = text[end..]
        .char_indices()
        .nth(SOL_CUE_CHARS)
        .map_or(text.len(), |(i, _)| end + i);
    SOL_CUE.is_match(&text[from..to])
}

/// Every valid address not inside an earlier hit. EVM is stored lower case (US-04.5 AC2),
/// bech32 lower case, Base58 forms as written. Solana is tried last, on what no other type took,
/// and gets [`SOL_UNCUED_CONFIDENCE`] without a cue word nearby (AC3).
pub(crate) fn find(text: &str, claims: &Claims) -> Vec<ByteHit> {
    let mut out: Vec<ByteHit> = Vec::new();
    let free = |out: &[ByteHit], s: usize, e: usize| {
        !claims.overlaps(s, e) && !out.iter().any(|h| s < h.end && h.start < e)
    };
    for m in EVM.find_iter(text) {
        if evm_checksum_ok(&m.as_str()[2..]) && free(&out, m.start(), m.end()) {
            let value = m.as_str().to_ascii_lowercase();
            out.push(ByteHit::new(m.start(), m.end(), IdentifierKind::Evm, value));
        }
    }
    for m in BTC_BECH32.find_iter(text) {
        if btc_bech32(m.as_str()) && free(&out, m.start(), m.end()) {
            let value = m.as_str().to_ascii_lowercase();
            out.push(ByteHit::new(m.start(), m.end(), IdentifierKind::Btc, value));
        }
    }
    for m in BTC_B58.find_iter(text) {
        if base58check(m.as_str(), &[0x00, 0x05]) && free(&out, m.start(), m.end()) {
            out.push(ByteHit::new(
                m.start(),
                m.end(),
                IdentifierKind::Btc,
                m.as_str(),
            ));
        }
    }
    for m in TRON.find_iter(text) {
        if base58check(m.as_str(), &[0x41]) && free(&out, m.start(), m.end()) {
            out.push(ByteHit::new(
                m.start(),
                m.end(),
                IdentifierKind::Tron,
                m.as_str(),
            ));
        }
    }
    for m in SOL.find_iter(text) {
        if sol_valid(m.as_str()) && free(&out, m.start(), m.end()) {
            let mut hit = ByteHit::new(m.start(), m.end(), IdentifierKind::Sol, m.as_str());
            if !sol_cued(text, m.start(), m.end()) {
                hit.confidence = SOL_UNCUED_CONFIDENCE;
            }
            out.push(hit);
        }
    }
    out.sort_by_key(|h| h.start);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::keys;
    use crate::{Rules, find_all};

    const BTC: &str = "1GqupJhz8eLskYZzTj9aWudk1E8vM34zSV";
    const BECH32: &str = "bc1qs95x428wtu729ffv6cfm4um8yxjvl5lgaf9xsc";
    const EVM_MIXED: &str = "0xf948A998b22fF8229C16Bd28B0AF3006c012a87e";
    const TRON_ADDR: &str = "TH2fjNgATy9Hi1e4NtJHzukziwPxX2Vb9r";
    const SOL_ADDR: &str = "H34gx4D9ADcYkon9B9uTUpsxPUkKmXTL6Y82nyGg8srm";

    /// US-04.5 AC1: Base58Check or bech32 must hold.
    #[test]
    fn us_04_5_ac1_bitcoin_checksum() {
        assert_eq!(
            keys(&format!("{BTC} and {BECH32}"), &Rules::default()),
            vec![
                (format!("btc:{BTC}"), BTC.to_string()),
                (format!("btc:{BECH32}"), BECH32.to_string()),
            ]
        );
        assert_eq!(
            keys(&BECH32.to_ascii_uppercase(), &Rules::default())[0].0,
            format!("btc:{BECH32}")
        );
        assert_eq!(
            keys("1GqupJhz8eLskYZzTj9aWudk1E8vM34zS1", &Rules::default()),
            vec![]
        );
        assert_eq!(
            keys(
                "bc1qs95x428wtu729ffv6cfm4um8yxjvl5lgaf9xsq",
                &Rules::default()
            ),
            vec![]
        );
        // The genesis address is P2PKH; a P2SH address passes too.
        assert_eq!(
            keys("3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy", &Rules::default()).len(),
            1
        );
    }

    /// US-04.5 AC2: stored lower case; mixed case must pass EIP-55.
    #[test]
    fn us_04_5_ac2_evm_eip55() {
        let lower = EVM_MIXED.to_ascii_lowercase();
        assert_eq!(
            keys(EVM_MIXED, &Rules::default()),
            vec![(format!("evm:{lower}"), EVM_MIXED.to_string())]
        );
        assert_eq!(keys(&lower, &Rules::default()).len(), 1);
        assert_eq!(
            keys(
                "0xf948A998b22fF8229C16Bd28B0AF3006c012a87E",
                &Rules::default()
            ),
            vec![]
        );
        assert_eq!(
            keys(
                "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
                &Rules::default()
            )
            .len(),
            1
        );
        assert_eq!(keys(&format!("{lower}0"), &Rules::default()), vec![]);
    }

    /// US-04.5 AC3: 32 bytes, 0.7 confidence without a cue word within 50 characters.
    #[test]
    fn us_04_5_ac3_solana_confidence() {
        let bare = find_all(&format!("ref {SOL_ADDR}"), &Rules::default());
        assert_eq!(bare.hits.len(), 1);
        assert_eq!(bare.hits[0].key.to_string(), format!("sol:{SOL_ADDR}"));
        assert_eq!(bare.hits[0].confidence, SOL_UNCUED_CONFIDENCE);
        for text in [
            format!("Solana wallet: {SOL_ADDR}"),
            format!("{SOL_ADDR} (SOL)"),
            format!("address{}{SOL_ADDR}", " ".repeat(40)),
        ] {
            assert_eq!(
                find_all(&text, &Rules::default()).hits[0].confidence,
                1.0,
                "{text}"
            );
        }
        let far = format!("wallet{}{SOL_ADDR}", " ".repeat(60));
        assert_eq!(
            find_all(&far, &Rules::default()).hits[0].confidence,
            SOL_UNCUED_CONFIDENCE
        );
        // 44 Base58 characters that do not decode to 32 bytes.
        assert_eq!(
            keys(
                "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
                &Rules::default()
            ),
            vec![]
        );
    }

    /// US-04.5 AC4: TRON Base58Check, version 0x41; never also a Solana address.
    #[test]
    fn us_04_5_ac4_tron_checksum() {
        assert_eq!(
            keys(&format!("TRC20 {TRON_ADDR}"), &Rules::default()),
            vec![(format!("tron:{TRON_ADDR}"), TRON_ADDR.to_string())]
        );
        let mut broken = TRON_ADDR.to_string();
        broken.replace_range(33.., "s");
        assert!(
            keys(&broken, &Rules::default())
                .iter()
                .all(|(k, _)| !k.starts_with("tron:"))
        );
    }
}
