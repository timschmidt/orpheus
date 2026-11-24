//! `otpauth://` URI parsing (the Key Uri Format used by authenticator apps):
//!
//! ```text
//! otpauth://TYPE/LABEL?secret=BASE32[&issuer=..][&algorithm=SHA1|SHA256|SHA512]
//!           [&digits=6|7|8][&period=SECONDS][&counter=N]
//! LABEL = [issuer ":" [spaces]] account      (percent-encoded)
//! ```
//!
//! Unknown parameters (`image`, `color`, …) are ignored. An `issuer`
//! parameter takes precedence over the label prefix. HOTP requires
//! `counter`. Text longer than the entry's fields is cut at a character
//! boundary; a secret that does not fit is an error.

use zeroize::Zeroize;

use crate::base32;
use crate::entry::{Entry, EntryError, Kind};
use crate::otp::Algorithm;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UriError {
    BadScheme,
    BadType,
    /// Invalid `%XX` escape or text that is not UTF-8.
    BadEncoding,
    MissingSecret,
    BadSecret(base32::Error),
    BadAlgorithm,
    BadDigits,
    BadPeriod,
    MissingCounter,
    BadCounter,
    Entry(EntryError),
}

/// Longest decoded label or parameter value kept (longer text is cut).
const SCRATCH: usize = 160;

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Percent-decode `src` into `out`, dropping what does not fit. Returns
/// the decoded length.
fn pct_decode(src: &[u8], plus_is_space: bool, out: &mut [u8]) -> Result<usize, UriError> {
    let mut n = 0;
    let mut i = 0;
    while i < src.len() {
        let c = match src[i] {
            b'%' => {
                let (Some(&h), Some(&l)) = (src.get(i + 1), src.get(i + 2)) else {
                    return Err(UriError::BadEncoding);
                };
                let (Some(h), Some(l)) = (hex(h), hex(l)) else { return Err(UriError::BadEncoding) };
                i += 2;
                (h << 4) | l
            }
            b'+' if plus_is_space => b' ',
            c => c,
        };
        if n < out.len() {
            out[n] = c;
            n += 1;
        }
        i += 1;
    }
    Ok(n)
}

/// The longest valid UTF-8 prefix of `b`; an invalid sequence that is not
/// merely cut off at the end is an error.
fn utf8(b: &[u8]) -> Result<&str, UriError> {
    match core::str::from_utf8(b) {
        Ok(s) => Ok(s),
        Err(e) if e.error_len().is_none() => {
            core::str::from_utf8(&b[..e.valid_up_to()]).map_err(|_| UriError::BadEncoding)
        }
        Err(_) => Err(UriError::BadEncoding),
    }
}

fn parse_u64(v: &[u8]) -> Option<u64> {
    if v.is_empty() || v.len() > 20 {
        return None;
    }
    let mut x: u64 = 0;
    for &c in v {
        if !c.is_ascii_digit() {
            return None;
        }
        x = x.checked_mul(10)?.checked_add(u64::from(c - b'0'))?;
    }
    Some(x)
}

fn strip_prefix_ci<'a>(s: &'a [u8], p: &[u8]) -> Option<&'a [u8]> {
    if s.len() >= p.len() && s[..p.len()].eq_ignore_ascii_case(p) { Some(&s[p.len()..]) } else { None }
}

/// Parse one `otpauth://` URI (surrounding whitespace is ignored).
pub fn parse(uri: &str) -> Result<Entry, UriError> {
    let u = uri.trim().as_bytes();
    let rest = strip_prefix_ci(u, b"otpauth://").ok_or(UriError::BadScheme)?;
    let slash = rest.iter().position(|&c| c == b'/').ok_or(UriError::BadType)?;
    let ty = &rest[..slash];
    let hotp = if ty.eq_ignore_ascii_case(b"totp") {
        false
    } else if ty.eq_ignore_ascii_case(b"hotp") {
        true
    } else {
        return Err(UriError::BadType);
    };
    let rest = &rest[slash + 1..];
    let (label, query) = match rest.iter().position(|&c| c == b'?') {
        Some(q) => (&rest[..q], &rest[q + 1..]),
        None => (rest, &[][..]),
    };

    let mut label_buf = [0u8; SCRATCH];
    let ln = pct_decode(label, false, &mut label_buf)?;
    let label = utf8(&label_buf[..ln])?;
    let (label_issuer, account) = match label.find(':') {
        Some(i) => (&label[..i], label[i + 1..].trim_start()),
        None => ("", label),
    };

    let mut issuer_buf = [0u8; SCRATCH];
    let mut issuer_len: Option<usize> = None;
    let mut secret = [0u8; crate::entry::MAX_SECRET];
    let mut secret_len: Option<usize> = None;
    let mut alg = Algorithm::Sha1;
    let mut digits = 6u8;
    let mut period = 30u32;
    let mut counter: Option<u64> = None;

    let result = (|| {
        for pair in query.split(|&c| c == b'&') {
            if pair.is_empty() {
                continue;
            }
            let (k, v) = match pair.iter().position(|&c| c == b'=') {
                Some(e) => (&pair[..e], &pair[e + 1..]),
                None => (pair, &[][..]),
            };
            let mut vb = [0u8; SCRATCH];
            let vn = pct_decode(v, true, &mut vb)?;
            let v = &vb[..vn];
            if k.eq_ignore_ascii_case(b"secret") {
                let n = base32::decode(v, &mut secret).map_err(UriError::BadSecret)?;
                secret_len = Some(n);
                vb.zeroize();
            } else if k.eq_ignore_ascii_case(b"issuer") {
                utf8(v)?;
                issuer_buf[..vn].copy_from_slice(v);
                issuer_len = Some(vn);
            } else if k.eq_ignore_ascii_case(b"algorithm") {
                alg = if v.eq_ignore_ascii_case(b"SHA1") {
                    Algorithm::Sha1
                } else if v.eq_ignore_ascii_case(b"SHA256") {
                    Algorithm::Sha256
                } else if v.eq_ignore_ascii_case(b"SHA512") {
                    Algorithm::Sha512
                } else {
                    return Err(UriError::BadAlgorithm);
                };
            } else if k.eq_ignore_ascii_case(b"digits") {
                digits = match parse_u64(v) {
                    Some(d @ 6..=8) => d as u8,
                    _ => return Err(UriError::BadDigits),
                };
            } else if k.eq_ignore_ascii_case(b"period") {
                period = match parse_u64(v) {
                    Some(p @ 1..=3600) => p as u32,
                    _ => return Err(UriError::BadPeriod),
                };
            } else if k.eq_ignore_ascii_case(b"counter") {
                counter = Some(parse_u64(v).ok_or(UriError::BadCounter)?);
            }
        }
        let n = secret_len.ok_or(UriError::MissingSecret)?;
        if n == 0 {
            return Err(UriError::MissingSecret);
        }
        let kind = if hotp {
            Kind::Hotp { counter: counter.ok_or(UriError::MissingCounter)? }
        } else {
            Kind::Totp { period }
        };
        let issuer = match issuer_len {
            Some(n) => utf8(&issuer_buf[..n])?,
            None => label_issuer,
        };
        Entry::new(kind, alg, digits, &secret[..n], issuer, account).map_err(UriError::Entry)
    })();
    secret.zeroize();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_example() {
        let e =
            parse("otpauth://totp/Example:alice@google.com?secret=JBSWY3DPEHPK3PXP&issuer=Example").unwrap();
        assert_eq!(e.kind, Kind::Totp { period: 30 });
        assert_eq!(e.alg, Algorithm::Sha1);
        assert_eq!(e.digits, 6);
        assert_eq!(e.secret(), b"Hello!\xDE\xAD\xBE\xEF");
        assert_eq!(e.issuer.as_str(), "Example");
        assert_eq!(e.account.as_str(), "alice@google.com");
    }

    #[test]
    fn all_parameters_and_encodings() {
        let e = parse(
            "OTPAUTH://HOTP/ACME%20Co%3A%20john.doe%2Btag@email.com?SECRET=hxdmvjecjjwsrb3hwizr4ifugftmxboz%3D\
             &issuer=ACME+Co&algorithm=sha512&digits=8&counter=42&image=x",
        )
        .unwrap();
        assert_eq!(e.kind, Kind::Hotp { counter: 42 });
        assert_eq!(e.alg, Algorithm::Sha512);
        assert_eq!(e.digits, 8);
        assert_eq!(e.secret().len(), 20);
        assert_eq!(e.issuer.as_str(), "ACME Co");
        assert_eq!(e.account.as_str(), "john.doe+tag@email.com");

        let e = parse("otpauth://totp/Only%20account?secret=GEZDGNBV&period=60&algorithm=SHA256").unwrap();
        assert_eq!(e.kind, Kind::Totp { period: 60 });
        assert_eq!(e.alg, Algorithm::Sha256);
        assert_eq!(e.issuer.as_str(), "");
        assert_eq!(e.account.as_str(), "Only account");

        // The issuer parameter wins over the label prefix.
        let e = parse("otpauth://totp/Old:bob?issuer=New&secret=GEZDGNBV").unwrap();
        assert_eq!(e.issuer.as_str(), "New");
    }

    #[test]
    fn errors() {
        assert_eq!(parse("http://totp/x?secret=GEZDGNBV").unwrap_err(), UriError::BadScheme);
        assert_eq!(parse("otpauth://motp/x?secret=GEZDGNBV").unwrap_err(), UriError::BadType);
        assert_eq!(parse("otpauth://totp").unwrap_err(), UriError::BadType);
        assert_eq!(parse("otpauth://totp/x").unwrap_err(), UriError::MissingSecret);
        assert_eq!(parse("otpauth://totp/x?secret=").unwrap_err(), UriError::MissingSecret);
        assert_eq!(
            parse("otpauth://totp/x?secret=GEZ1").unwrap_err(),
            UriError::BadSecret(base32::Error::InvalidChar)
        );
        assert_eq!(parse("otpauth://totp/x?secret=GEZDGNBV&digits=5").unwrap_err(), UriError::BadDigits);
        assert_eq!(parse("otpauth://totp/x?secret=GEZDGNBV&period=0").unwrap_err(), UriError::BadPeriod);
        assert_eq!(
            parse("otpauth://totp/x?secret=GEZDGNBV&algorithm=MD5").unwrap_err(),
            UriError::BadAlgorithm
        );
        assert_eq!(parse("otpauth://hotp/x?secret=GEZDGNBV").unwrap_err(), UriError::MissingCounter);
        assert_eq!(parse("otpauth://hotp/x?secret=GEZDGNBV&counter=-1").unwrap_err(), UriError::BadCounter);
        assert_eq!(parse("otpauth://totp/a%2?secret=GEZDGNBV").unwrap_err(), UriError::BadEncoding);
        assert_eq!(parse("otpauth://totp/a%FF?secret=GEZDGNBV").unwrap_err(), UriError::BadEncoding);
        // 65 bytes of secret: 104 Base32 characters.
        let mut long = std::string::String::from("otpauth://totp/x?secret=");
        for _ in 0..104 {
            long.push('A');
        }
        assert_eq!(parse(&long).unwrap_err(), UriError::BadSecret(base32::Error::TooLong));
    }

    #[test]
    fn long_labels_are_cut_on_char_boundaries() {
        let mut uri = std::string::String::from("otpauth://totp/");
        for _ in 0..40 {
            uri.push_str("%C3%A9"); // é, two bytes
        }
        uri.push(':');
        for _ in 0..50 {
            uri.push_str("%E2%82%AC"); // €, three bytes
        }
        uri.push_str("?secret=GEZDGNBV");
        let e = parse(&uri).unwrap();
        assert_eq!(e.issuer.as_str().len(), 32);
        assert!(e.issuer.as_str().chars().all(|c| c == 'é'));
        // The label scratch (160 bytes) is cut first, then the field.
        assert!(e.account.as_str().len() <= 64);
        assert!(e.account.as_str().chars().all(|c| c == '€'));
    }

    /// Fuzz smoke: random and mutated URIs never panic, and whatever parses
    /// satisfies the entry invariants.
    #[test]
    fn fuzz_smoke() {
        let seeds: [&[u8]; 3] = [
            b"otpauth://totp/Example:alice@google.com?secret=JBSWY3DPEHPK3PXP&issuer=Example&period=60",
            b"otpauth://hotp/A%3Ab?secret=GEZDGNBV&counter=7&digits=8&algorithm=SHA256",
            b"otpauth://totp/x?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ%3D%3D&issuer=%E2%82%AC",
        ];
        let alphabet = b"%:?&=/+ AZaz27otpauhseciruglm09xF\xC3\xA9\x80";
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let mut ok = 0;
        for i in 0..20_000 {
            let mut buf = std::vec::Vec::from(seeds[i % seeds.len()]);
            let edits = 1 + next() % 6;
            for _ in 0..edits {
                let r = next();
                let pos = (r >> 8) as usize % (buf.len() + 1);
                let c = alphabet[(r >> 40) as usize % alphabet.len()];
                match r % 3 {
                    0 if pos < buf.len() => buf[pos] = c,
                    1 => buf.insert(pos, c),
                    _ if pos < buf.len() => {
                        buf.remove(pos);
                    }
                    _ => {}
                }
            }
            let Ok(s) = core::str::from_utf8(&buf) else { continue };
            if let Ok(e) = parse(s) {
                ok += 1;
                assert!(!e.secret().is_empty() && e.secret().len() <= crate::entry::MAX_SECRET);
                assert!((6..=8).contains(&e.digits));
                let _ = e.code(1_700_000_000);
            }
        }
        assert!(ok > 1000, "mutations too destructive: {ok}");
    }
}
