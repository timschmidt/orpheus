//! CRCs used by the formats: CRC-8 (poly 0x07, FLAC frame header) and
//! CRC-16 (poly 0x8005, FLAC frame footer; MPEG audio uses the same
//! polynomial with initial value 0xFFFF).

const fn table8() -> [u8; 256] {
    let mut t = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = i as u8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

const fn table16() -> [u16; 256] {
    let mut t = [0u16; 256];
    let mut i = 0;
    while i < 256 {
        let mut c = (i as u16) << 8;
        let mut k = 0;
        while k < 8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
}

static T8: [u8; 256] = table8();
static T16: [u16; 256] = table16();

/// Continue a CRC-8 over `data`.
#[must_use]
pub fn crc8(mut crc: u8, data: &[u8]) -> u8 {
    for &b in data {
        crc = T8[usize::from(crc ^ b)];
    }
    crc
}

/// Continue a CRC-16 over `data`.
#[must_use]
pub fn crc16(mut crc: u16, data: &[u8]) -> u16 {
    for &b in data {
        crc = (crc << 8) ^ T16[usize::from((crc >> 8) as u8 ^ b)];
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_values() {
        // CRC-8/SMBUS and CRC-16/UMTS (BUYPASS) check values for "123456789".
        assert_eq!(crc8(0, b"123456789"), 0xF4);
        assert_eq!(crc16(0, b"123456789"), 0xFEE8);
    }
}
