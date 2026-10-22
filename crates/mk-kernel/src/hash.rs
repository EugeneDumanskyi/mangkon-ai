use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

/// Fingerprint of a file's bytes: SHA-256. Same bytes → same hash (domain §3.3).
///
/// Displays as 64 lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Hashes a complete byte slice.
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// Starts an incremental hash, for content that arrives in chunks.
    pub fn hasher() -> ContentHasher {
        ContentHasher(Sha256::new())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        self.to_string()
    }
}

/// Incremental form of [`ContentHash::of`]. Feeding the same bytes in any chunking gives the same hash.
pub struct ContentHasher(Sha256);

impl ContentHasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finish(self) -> ContentHash {
        ContentHash(self.0.finalize().into())
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ContentHash({self})")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseHashError;

impl fmt::Display for ParseHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("expected 64 lowercase hex characters")
    }
}

impl std::error::Error for ParseHashError {}

impl FromStr for ContentHash {
    type Err = ParseHashError;

    /// Accepts only the canonical form that `Display` produces.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.as_bytes();
        if s.len() != 64 {
            return Err(ParseHashError);
        }
        let nibble = |c: u8| match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(ParseHashError),
        };
        let mut out = [0u8; 32];
        for (i, pair) in s.as_chunks::<2>().0.iter().enumerate() {
            out[i] = nibble(pair[0])? << 4 | nibble(pair[1])?;
        }
        Ok(Self(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn known_vector_empty_input() {
        assert_eq!(
            ContentHash::of(b"").to_string(),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn known_vector_abc() {
        assert_eq!(
            ContentHash::of(b"abc").to_string(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn rejects_non_canonical_hex() {
        let upper = "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855";
        assert!(upper.parse::<ContentHash>().is_err());
        assert!("abc".parse::<ContentHash>().is_err());
    }

    proptest! {
        #[test]
        fn same_bytes_same_hash(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            prop_assert_eq!(ContentHash::of(&bytes), ContentHash::of(&bytes.clone()));
        }

        #[test]
        fn chunking_does_not_change_hash(bytes in proptest::collection::vec(any::<u8>(), 0..512), cut in 0usize..512) {
            let cut = cut.min(bytes.len());
            let mut h = ContentHash::hasher();
            h.update(&bytes[..cut]);
            h.update(&bytes[cut..]);
            prop_assert_eq!(h.finish(), ContentHash::of(&bytes));
        }

        #[test]
        fn display_parse_round_trip(raw in any::<[u8; 32]>()) {
            let h = ContentHash::from_bytes(raw);
            prop_assert_eq!(h.to_string().parse::<ContentHash>().unwrap(), h);
        }
    }
}
