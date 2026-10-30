//! The Luhn check digit (ISO/IEC 7812-1), used to tell a card-like number from other long digit
//! runs: the search lists mask them (NFR-SEC-14) and M4b flags them (US-04.6).

/// Card numbers have 13 to 19 digits (ISO/IEC 7812-1).
pub const CARD_DIGITS: std::ops::RangeInclusive<usize> = 13..=19;

/// Whether `digits` (ASCII `0`–`9` only) pass the Luhn check. False for an empty slice or any
/// other byte.
pub fn is_valid(digits: &[u8]) -> bool {
    if digits.is_empty() {
        return false;
    }
    let mut sum = 0u32;
    for (i, b) in digits.iter().rev().enumerate() {
        if !b.is_ascii_digit() {
            return false;
        }
        let mut d = u32::from(b - b'0');
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum.is_multiple_of(10)
}

/// Whether `digits` look like a card number: 13–19 digits passing Luhn.
pub fn is_card_like(digits: &[u8]) -> bool {
    CARD_DIGITS.contains(&digits.len()) && is_valid(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_test_cards_pass() {
        for n in ["4111111111111111", "5555555555554444", "378282246310005"] {
            assert!(is_card_like(n.as_bytes()), "{n}");
        }
    }

    #[test]
    fn wrong_check_digit_length_or_bytes_fail() {
        assert!(!is_valid(b"4111111111111112"));
        assert!(!is_card_like(b"0"));
        assert!(!is_card_like(b"00000000000000000000"));
        assert!(!is_valid(b"4111 1111"));
        assert!(!is_valid(b""));
    }
}
