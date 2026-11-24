//! RFC 4648 Base32 (the alphabet of `otpauth://` secrets).

const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// A character outside the alphabet (after ignoring case, spaces,
    /// hyphens and trailing padding).
    InvalidChar,
    /// The decoded value does not fit the output buffer.
    TooLong,
    /// Padding in the middle, or a length no encoder produces.
    BadLength,
}

fn value(c: u8) -> Option<u8> {
    match c {
        b'A'..=b'Z' => Some(c - b'A'),
        b'a'..=b'z' => Some(c - b'a'),
        b'2'..=b'7' => Some(c - b'2' + 26),
        _ => None,
    }
}

/// Decode `text` into `out`; returns the number of bytes. Case is ignored;
/// spaces and hyphens (as in secrets printed in groups) are skipped; `=`
/// padding is accepted only at the end and is optional.
pub fn decode(text: &[u8], out: &mut [u8]) -> Result<usize, Error> {
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut n = 0usize;
    let mut chars = 0usize;
    let mut padding = false;
    for &c in text {
        if c == b' ' || c == b'-' {
            continue;
        }
        if c == b'=' {
            padding = true;
            continue;
        }
        if padding {
            return Err(Error::BadLength);
        }
        let v = value(c).ok_or(Error::InvalidChar)?;
        chars += 1;
        acc = (acc << 5) | u32::from(v);
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            if n == out.len() {
                return Err(Error::TooLong);
            }
            out[n] = (acc >> bits) as u8;
            n += 1;
            acc &= (1 << bits) - 1;
        }
    }
    // Valid unpadded lengths mod 8 are 0, 2, 4, 5, 7.
    if matches!(chars % 8, 1 | 3 | 6) {
        return Err(Error::BadLength);
    }
    Ok(n)
}

/// Encode `data` without padding into `out`; returns the number of
/// characters, or `None` if `out` is too small.
pub fn encode(data: &[u8], out: &mut [u8]) -> Option<usize> {
    let need = (data.len() * 8).div_ceil(5);
    if out.len() < need {
        return None;
    }
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    let mut n = 0;
    for &b in data {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out[n] = ALPHABET[((acc >> bits) & 31) as usize];
            n += 1;
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        out[n] = ALPHABET[((acc << (5 - bits)) & 31) as usize];
        n += 1;
    }
    Some(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors() {
        let v: [(&[u8], &[u8]); 7] = [
            (b"", b""),
            (b"f", b"MY======"),
            (b"fo", b"MZXQ===="),
            (b"foo", b"MZXW6==="),
            (b"foob", b"MZXW6YQ="),
            (b"fooba", b"MZXW6YTB"),
            (b"foobar", b"MZXW6YTBOI======"),
        ];
        for (plain, enc) in v {
            let mut out = [0u8; 16];
            let n = decode(enc, &mut out).unwrap();
            assert_eq!(&out[..n], plain);
            let mut e = [0u8; 16];
            let m = encode(plain, &mut e).unwrap();
            let unpadded: &[u8] = &enc[..enc.iter().position(|&c| c == b'=').unwrap_or(enc.len())];
            assert_eq!(&e[..m], unpadded);
        }
    }

    #[test]
    fn lenient_input_and_errors() {
        let mut out = [0u8; 32];
        let n = decode(b"gezd gnbv-gy3t qojq gezd gnbv gy3t qojq", &mut out).unwrap();
        assert_eq!(&out[..n], b"12345678901234567890");
        assert_eq!(decode(b"MZ1W", &mut out), Err(Error::InvalidChar));
        assert_eq!(decode(b"MY=A", &mut out), Err(Error::BadLength));
        assert_eq!(decode(b"M", &mut out), Err(Error::BadLength));
        assert_eq!(decode(b"MZXW6YTB", &mut [0u8; 4]), Err(Error::TooLong));
    }
}
