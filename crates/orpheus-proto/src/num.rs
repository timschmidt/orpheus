//! Numbers in developer test commands (`settings test-set 0x030D 1`, …).

/// A decimal or `0x` hexadecimal number of at most six digits, optionally
/// negative. Hex digits may be upper or lower case (2026-11-26: the
/// firmware took only lower case, and `settings test-set 0x030D 1` from
/// the suite's `settings_brightness` journey got the usage text).
#[must_use]
pub fn parse(w: &[u8]) -> Option<i32> {
    let (neg, d) = match w.split_first() {
        Some((b'-', d)) => (true, d),
        _ => (false, w),
    };
    let (radix, d) = match d.strip_prefix(b"0x").or_else(|| d.strip_prefix(b"0X")) {
        Some(h) => (16, h),
        None => (10, d),
    };
    if d.is_empty() || d.len() > 6 {
        return None;
    }
    let mut v = 0i32;
    for &c in d {
        let x = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' if radix == 16 => c - b'a' + 10,
            b'A'..=b'F' if radix == 16 => c - b'A' + 10,
            _ => return None,
        };
        v = v * radix + i32::from(x);
    }
    Some(if neg { -v } else { v })
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// 2026-11-26: upper-case hex keys were refused by the firmware.
    #[test]
    fn regress_upper_case_hex_keys_parse() {
        assert_eq!(parse(b"0x030D"), Some(0x030D));
        assert_eq!(parse(b"0x030d"), Some(0x030D));
        assert_eq!(parse(b"0X1F"), Some(0x1F));
    }

    #[test]
    fn decimal_negative_and_refusals() {
        assert_eq!(parse(b"781"), Some(781));
        assert_eq!(parse(b"-5"), Some(-5));
        assert_eq!(parse(b"0x"), None);
        assert_eq!(parse(b""), None);
        assert_eq!(parse(b"12a"), None);
        assert_eq!(parse(b"0x1234567"), None);
        assert_eq!(parse(b"0xG1"), None);
    }
}
