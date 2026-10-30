//! Card-like numbers in lists (NFR-SEC-14): all digits but the last four become `*`.
//!
//! A card-like number is a run of 13–19 ASCII digits, single spaces or dashes allowed between
//! them, that passes Luhn (`mk_identifiers::luhn`). Separators are kept, so
//! `4111 1111 1111 1111` shows as `**** **** **** 1111`. The text keeps its length in
//! characters, so hit spans in it stay valid (INV-CO-04). The stored text itself is never
//! changed (INV-ID-06): this is for display in lists only.

use std::borrow::Cow;

use mk_identifiers::luhn;

/// `text` with its card-like numbers masked; borrowed when there are none.
pub fn mask_card_like(text: &str) -> Cow<'_, str> {
    let chars: Vec<char> = text.chars().collect();
    let mut masked: Option<Vec<char>> = None;
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // A maximal run: digits, with one space or dash between two digits.
        let mut digits = vec![i];
        let mut j = i + 1;
        while j < chars.len() {
            if chars[j].is_ascii_digit() {
                digits.push(j);
                j += 1;
            } else if matches!(chars[j], ' ' | '-')
                && chars.get(j + 1).is_some_and(char::is_ascii_digit)
            {
                j += 1;
            } else {
                break;
            }
        }
        let bytes: Vec<u8> = digits.iter().map(|&k| chars[k] as u8).collect();
        if luhn::is_card_like(&bytes) {
            let out = masked.get_or_insert_with(|| chars.clone());
            for &k in &digits[..digits.len() - 4] {
                out[k] = '*';
            }
        }
        i = j;
    }
    match masked {
        Some(out) => Cow::Owned(out.into_iter().collect()),
        None => Cow::Borrowed(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// NFR-SEC-14: test cards in the forms TD-1 writes them.
    #[test]
    fn nfr_sec_14_masks_card_like_numbers() {
        let cases = [
            ("38.50\t5555555555554444", "38.50\t************4444"),
            ("card 4111 1111 1111 1111.", "card **** **** **** 1111."),
            ("4111-1111-1111-1111", "****-****-****-1111"),
            ("amex 378282246310005", "amex ***********0005"),
        ];
        for (text, want) in cases {
            assert_eq!(mask_card_like(text), want, "{text}");
        }
    }

    #[test]
    fn other_numbers_stay() {
        for text in [
            "4111111111111112",         // fails Luhn
            "411111111111",             // 12 digits
            "41111111111111111111",     // 20 digits
            "+44 20 7946 0958",         // a phone number
            "EC-25-0041 on 2025-01-09", // ids and dates
            "4111  1111 1111 1111",     // two spaces break the run
        ] {
            assert!(matches!(mask_card_like(text), Cow::Borrowed(_)), "{text}");
        }
    }

    proptest! {
        /// INV-CO-04: masking keeps the length in characters, and only ever writes `*`.
        #[test]
        fn masking_keeps_length(text in "[0-9 \\-a-zé]{0,60}") {
            let out = mask_card_like(&text);
            prop_assert_eq!(out.chars().count(), text.chars().count());
            for (a, b) in text.chars().zip(out.chars()) {
                prop_assert!(a == b || (a.is_ascii_digit() && b == '*'));
            }
        }
    }
}
