//! Fake-but-valid identifier values (NFR-LEG-04). See `docs/07-test-corpus.md` §7.

use mk_bench::checks;
use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::Rng;

/// IBAN with correct check digits for `country` + `bban`.
pub fn iban(country: &str, bban: &str) -> String {
    let probe = format!("{country}00{bban}");
    let check = 98 - checks::iban_mod97(&probe);
    let out = format!("{country}{check:02}{bban}");
    assert!(checks::iban_valid(&out), "generated IBAN {out} is invalid");
    out
}

/// Groups of four, as printed on paper.
pub fn iban_spaced(iban: &str) -> String {
    let chars: Vec<char> = iban.chars().collect();
    chars
        .chunks(4)
        .map(|c| c.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Random bytes from the seeded generator. No private key exists for any address made from
/// these bytes, so nobody can control the "wallet".
pub fn bytes<const N: usize>(rng: &mut ChaCha20Rng) -> [u8; N] {
    let mut out = [0u8; N];
    rng.fill_bytes(&mut out);
    out
}

/// Bitcoin P2PKH (Base58Check, version 0x00).
pub fn btc_p2pkh(payload: [u8; 20]) -> String {
    let mut v = vec![0x00];
    v.extend_from_slice(&payload);
    bs58::encode(v).with_check().into_string()
}

/// Bitcoin P2WPKH (bech32, witness v0, 20-byte program).
pub fn btc_bech32(program: [u8; 20]) -> String {
    bech32::segwit::encode_v0(bech32::hrp::BC, &program).expect("valid program")
}

/// EVM address, lowercase.
pub fn evm(payload: [u8; 20]) -> String {
    let mut s = String::from("0x");
    for b in payload {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// TRON (Base58Check, version 0x41).
pub fn tron(payload: [u8; 20]) -> String {
    let mut v = vec![0x41];
    v.extend_from_slice(&payload);
    bs58::encode(v).with_check().into_string()
}

/// Solana: base58 of 32 bytes.
pub fn sol(key: [u8; 32]) -> String {
    bs58::encode(key).into_string()
}

/// Changes one character so the checksum fails. Tries positions from the end, so the
/// result keeps the original shape (alphabet, length).
pub fn corrupt(value: &str, alphabet: &str, valid: impl Fn(&str) -> bool) -> String {
    let chars: Vec<char> = value.chars().collect();
    for pos in (chars.len() / 2..chars.len()).rev() {
        for repl in alphabet.chars() {
            if repl == chars[pos] {
                continue;
            }
            let mut c = chars.clone();
            c[pos] = repl;
            let s: String = c.into_iter().collect();
            if !valid(&s) {
                return s;
            }
        }
    }
    panic!("cannot corrupt {value}");
}

pub const BASE58: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
