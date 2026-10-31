//! Dates in table cells (US-05.1 AC1 "date"): ISO 8601 / RFC 3339 and the day-first forms
//! exports use. ASSUMPTION (M4c): `dd.mm.yyyy`, `dd-mm-yyyy` and `dd/mm/yyyy` are day first;
//! month-first `mm/dd/yyyy` is not read.

/// Whether `value` (trimmed) is a date: `yyyy-mm-dd`, an RFC 3339 date-time
/// (`2025-02-19T10:15:00Z`), or `dd.mm.yyyy`, `dd-mm-yyyy`, `dd/mm/yyyy` (day and month may have
/// one digit). The day must exist in its month.
pub fn is_date(value: &str) -> bool {
    let v = value.trim();
    if let Some((date, time)) = v.split_once(['T', 't', ' ']) {
        return iso_date(date) && rfc3339_time(time);
    }
    iso_date(v) || day_first(v)
}

fn digits(s: &str, min: usize, max: usize) -> Option<u32> {
    (s.len() >= min && s.len() <= max && s.bytes().all(|b| b.is_ascii_digit()))
        .then(|| s.parse().ok())
        .flatten()
}

fn valid(y: u32, m: u32, d: u32) -> bool {
    let leap = (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400);
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d) && (1000..=9999).contains(&y)
}

fn iso_date(s: &str) -> bool {
    let p: Vec<&str> = s.split('-').collect();
    match p.as_slice() {
        [y, m, d] => match (digits(y, 4, 4), digits(m, 2, 2), digits(d, 2, 2)) {
            (Some(y), Some(m), Some(d)) => valid(y, m, d),
            _ => false,
        },
        _ => false,
    }
}

fn day_first(s: &str) -> bool {
    for sep in ['.', '-', '/'] {
        let p: Vec<&str> = s.split(sep).collect();
        if let [d, m, y] = p.as_slice()
            && let (Some(d), Some(m), Some(y)) = (digits(d, 1, 2), digits(m, 1, 2), digits(y, 4, 4))
        {
            return valid(y, m, d);
        }
    }
    false
}

/// `HH:MM:SS` with optional fraction, then `Z` or `±HH:MM` (or nothing).
fn rfc3339_time(s: &str) -> bool {
    let (clock, zone) = match s.find(['Z', 'z', '+', '-']) {
        Some(i) => s.split_at(i),
        None => (s, ""),
    };
    let clock = clock.split_once('.').map_or(clock, |(c, frac)| {
        if !frac.is_empty() && frac.bytes().all(|b| b.is_ascii_digit()) {
            c
        } else {
            "x"
        }
    });
    let p: Vec<&str> = clock.split(':').collect();
    let clock_ok = match p.as_slice() {
        [h, m, sec] => matches!(
            (digits(h, 2, 2), digits(m, 2, 2), digits(sec, 2, 2)),
            (Some(h), Some(m), Some(s)) if h < 24 && m < 60 && s <= 60
        ),
        [h, m] => {
            matches!((digits(h, 2, 2), digits(m, 2, 2)), (Some(h), Some(m)) if h < 24 && m < 60)
        }
        _ => false,
    };
    let zone_ok = match zone {
        "" | "Z" | "z" => true,
        z => {
            let (h, m) = z[1..].split_once(':').unwrap_or((&z[1..], ""));
            matches!((digits(h, 2, 2), digits(m, 2, 2)), (Some(h), Some(m)) if h < 24 && m < 60)
        }
    };
    clock_ok && zone_ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read() {
        for d in [
            "2024-02-12",
            "2025-02-19T10:15:00Z",
            "2025-02-19T10:15:00.250+01:00",
            "12.03.2024",
            "15-08-2024",
            "1/2/2024",
            "29.02.2024",
        ] {
            assert!(is_date(d), "{d}");
        }
        for d in [
            "2024-13-01",
            "31.04.2024",
            "29.02.2023",
            "BWL-2024-001",
            "14 650,00",
            "2024",
            "12.03.24",
            "2025-02-19T25:00:00Z",
            "",
        ] {
            assert!(!is_date(d), "{d}");
        }
    }
}
