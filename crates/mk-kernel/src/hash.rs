use std::fmt;
use std::str::FromStr;

/// Fingerprint of a file's bytes: BLAKE3. Same bytes → same hash (domain §3.3).
///
/// BLAKE3 because re-scan hashes every new or changed file and it is several times faster than
/// SHA-256 (architecture §5.4). Ground-truth files keep SHA-256 (`mk-bench`). Displays as 64
/// lowercase hex characters.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    /// Hashes a complete byte slice.
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    /// Starts an incremental hash, for content that arrives in chunks.
    pub fn hasher() -> ContentHasher {
        ContentHasher(Box::new(blake3::Hasher::new()))
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
pub struct ContentHasher(Box<blake3::Hasher>);

impl ContentHasher {
    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn finish(self) -> ContentHash {
        ContentHash(*self.0.finalize().as_bytes())
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
            "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262"
        );
    }

    #[test]
    fn known_vector_abc() {
        assert_eq!(
            ContentHash::of(b"abc").to_string(),
            "6437b3ac38465133ffb63b75273a8db548c558465d79db03fd359c6cd5bd9d85"
        );
    }

    #[test]
    fn rejects_non_canonical_hex() {
        let upper = "AF1349B9F5F9A1A6A0404DEA36DCC9499BCB25C9ADC112B7CC9A93CAE41F3262";
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
