//! Small checksums used across the boundary.

/// FNV-1a, 32 bit. Names HIL tests in the mailbox and result block.
#[must_use]
pub const fn fnv1a(s: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    let mut i = 0;
    while i < s.len() {
        h ^= s[i] as u32;
        h = h.wrapping_mul(0x0100_0193);
        i += 1;
    }
    h
}

/// Byte sum, as the boot stub checks the resident image (cheap in 1 KiB of
/// stage-2 code).
#[must_use]
pub fn sum32(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |a, &b| a.wrapping_add(u32::from(b)))
}

/// CRC-32 (IEEE 802.3, reflected, as zlib). Bitwise: used for headers and
/// page stores on the host and in non-hot device paths.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    crc32_update(0xFFFF_FFFF, data) ^ 0xFFFF_FFFF
}

/// Continue a CRC-32 over more data; start with `0xFFFF_FFFF`, finish with
/// `^ 0xFFFF_FFFF`.
#[must_use]
pub fn crc32_update(mut crc: u32, data: &[u8]) -> u32 {
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vectors() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(fnv1a(b""), 0x811c_9dc5);
        assert_eq!(fnv1a(b"a"), 0xe40c_292c);
        assert_eq!(sum32(&[0xff, 0xff, 2]), 0x200);
    }
}
