//! Checksum tests for identifier values in the ground truth (US-04.4, US-04.5, US-04.6).
//!
//! These exist so `validate()` can prove that every checksum-type value in the corpus
//! passes (and every bad-checksum negative fails). The production extractors are
//! `mk-identifiers` (M4); these functions are deliberately small and independent of them.

use sha3::{Digest, Keccak256};

/// IBAN: country code, two check digits, BBAN; mod-97 of the rearranged number is 1.
/// Expects the compact form (no spaces, uppercase). Checks the length for the countries
/// the corpus uses.
pub fn iban_valid(iban: &str) -> bool {
    let b = iban.as_bytes();
    if b.len() < 5
        || !b[..2].iter().all(u8::is_ascii_uppercase)
        || !b[2..4].iter().all(u8::is_ascii_digit)
    {
        return false;
    }
    let expected_len = match &iban[..2] {
        "GB" => 22,
        "NL" => 18,
        "MT" => 31,
        "DE" => 22,
        "LU" => 20,
        _ => return false,
    };
    if b.len() != expected_len
        || !b
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
    {
        return false;
    }
    iban_mod97(iban) == 1
}

/// Mod-97 remainder of an IBAN after moving the first four characters to the end
/// and turning letters into numbers (A = 10 … Z = 35).
pub fn iban_mod97(iban: &str) -> u32 {
    let rearranged = iban[4..].bytes().chain(iban[..4].bytes());
    let mut rem: u32 = 0;
    for c in rearranged {
        let v = if c.is_ascii_digit() {
            (c - b'0') as u32
        } else {
            (c - b'A') as u32 + 10
        };
        rem = if v >= 10 {
            (rem * 100 + v) % 97
        } else {
            (rem * 10 + v) % 97
        };
    }
    rem
}

/// Luhn check for 13–19 digit numbers (card-like, US-04.6). Spaces are not allowed.
pub fn luhn_valid(digits: &str) -> bool {
    if !(13..=19).contains(&digits.len()) || !digits.bytes().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, c)| {
            let d = (c - b'0') as u32;
            if i % 2 == 1 {
                let x = d * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Bitcoin mainnet: Base58Check P2PKH (version 0x00) or P2SH (0x05) with a 20-byte payload,
/// or a bech32/bech32m segwit address with HRP `bc`.
pub fn btc_valid(addr: &str) -> bool {
    if addr.starts_with('1') || addr.starts_with('3') {
        return match bs58::decode(addr).with_check(None).into_vec() {
            Ok(v) => v.len() == 21 && (v[0] == 0x00 || v[0] == 0x05),
            Err(_) => false,
        };
    }
    if addr.to_ascii_lowercase().starts_with("bc1") {
        return match bech32::segwit::decode(addr) {
            Ok((hrp, _, _)) => hrp == bech32::hrp::BC,
            Err(_) => false,
        };
    }
    false
}

/// EVM: `0x` + 40 hex. All-lowercase or all-uppercase is accepted; mixed case must match EIP-55.
pub fn evm_valid(addr: &str) -> bool {
    let Some(hex) = addr.strip_prefix("0x") else {
        return false;
    };
    if hex.len() != 40 || !hex.bytes().all(|c| c.is_ascii_hexdigit()) {
        return false;
    }
    let has_lower = hex.bytes().any(|c| c.is_ascii_lowercase());
    let has_upper = hex.bytes().any(|c| c.is_ascii_uppercase());
    if !(has_lower && has_upper) {
        return true;
    }
    eip55(&hex.to_ascii_lowercase()) == addr
}

/// EIP-55 mixed-case form of a lowercase 40-hex address (without `0x`), returned with `0x`.
pub fn eip55(lower_hex: &str) -> String {
    let hash = Keccak256::digest(lower_hex.as_bytes());
    let mut out = String::from("0x");
    for (i, c) in lower_hex.chars().enumerate() {
        let nibble = (hash[i / 2] >> if i % 2 == 0 { 4 } else { 0 }) & 0x0f;
        out.push(if c.is_ascii_alphabetic() && nibble >= 8 {
            c.to_ascii_uppercase()
        } else {
            c
        });
    }
    out
}

/// Solana: base58 that decodes to exactly 32 bytes (US-04.5 AC3).
pub fn sol_valid(addr: &str) -> bool {
    (32..=44).contains(&addr.len())
        && bs58::decode(addr)
            .into_vec()
            .map(|v| v.len() == 32)
            .unwrap_or(false)
}

/// TRON: 34-character `T…` Base58Check with version byte 0x41 and a 20-byte payload.
pub fn tron_valid(addr: &str) -> bool {
    addr.len() == 34
        && addr.starts_with('T')
        && bs58::decode(addr)
            .with_check(None)
            .into_vec()
            .map(|v| v.len() == 21 && v[0] == 0x41)
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iban_example_from_us_04_4() {
        assert!(iban_valid("DE89370400440532013000"));
        assert!(!iban_valid("DE89370400440532013001"));
    }

    #[test]
    fn luhn_public_test_cards() {
        assert!(luhn_valid("4111111111111111"));
        assert!(luhn_valid("5555555555554444"));
        assert!(!luhn_valid("4111111111111112"));
    }

    #[test]
    fn btc_known_addresses() {
        // Genesis block coinbase address (well known, public).
        assert!(btc_valid("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"));
        assert!(!btc_valid("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNb"));
    }

    #[test]
    fn btc_bech32_round_trip() {
        let addr = bech32::segwit::encode_v0(bech32::hrp::BC, &[7u8; 20]).unwrap();
        assert!(btc_valid(&addr));
        let mut broken = addr.into_bytes();
        let last = broken.len() - 1;
        broken[last] = if broken[last] == b'q' { b'p' } else { b'q' };
        assert!(!btc_valid(std::str::from_utf8(&broken).unwrap()));
    }

    #[test]
    fn eip55_spec_vector() {
        let a = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed";
        assert_eq!(eip55(&a[2..].to_ascii_lowercase()), a);
        assert!(evm_valid(a));
        assert!(!evm_valid("0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAeD"));
        assert!(evm_valid(&a.to_ascii_lowercase()));
    }

    #[test]
    fn sol_and_tron_shapes() {
        assert!(sol_valid("11111111111111111111111111111111"));
        assert!(!sol_valid("1111"));
        assert!(!tron_valid("T1111111111111111111111111111111111"));
    }
}
