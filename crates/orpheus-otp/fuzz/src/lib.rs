//! Fuzz-target bodies of `orpheus-otp` (VER-4), shared by the cargo-fuzz
//! targets in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! * [`otp_uri`] — `otpauth://` URIs and base32 text as they come from a file on
//!   the card. Oracles: no panic; a parsed entry satisfies the entry
//!   invariants; it survives a round trip through a canonical URI written
//!   from it; base32 encode/decode round-trips.
//! * [`otp_vault`] — the vault sector as read from NOR, then a sequence of
//!   operations. Oracles: no panic; `load` classifies erased / damaged /
//!   locked consistently with the header bytes; the NOR model's program
//!   alignment; after any save, a fresh mount and unlock returns exactly the
//!   entries that were saved; wrong keys always fail and arm the back-off.

#![forbid(unsafe_code)]

use embassy_futures::block_on;
use orpheus_otp::entry::{MAX_ACCOUNT, MAX_ISSUER, MAX_PERIOD, MAX_SECRET};
use orpheus_otp::vault::{self, SECTOR};
use orpheus_otp::{Algorithm, Entry, Kind, SecretKind, Status, Unlocked, Vault, VaultError, base32, uri};

/// Characters a URI component may carry unescaped.
fn unreserved(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~' | b'@')
}

fn pct(s: &str, out: &mut String) {
    for &c in s.as_bytes() {
        if unreserved(c) {
            out.push(char::from(c));
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
}

/// A canonical `otpauth://` URI for `e` (every field explicit).
pub fn to_uri(e: &Entry) -> String {
    let mut u = String::from("otpauth://");
    u.push_str(match e.kind {
        Kind::Totp { .. } => "totp/",
        Kind::Hotp { .. } => "hotp/",
    });
    // The parser splits the decoded label at its first ':' and trims the
    // account after it; an account with a ':' therefore needs an (empty)
    // prefix, and came from one, so it has no leading space. The issuer
    // parameter takes precedence over the label's.
    if e.account.as_str().contains(':') {
        u.push(':');
    }
    pct(e.account.as_str(), &mut u);
    let mut b32 = [0u8; 128];
    let n = base32::encode(e.secret(), &mut b32).expect("a 64-byte secret fits 128 characters");
    u.push_str("?secret=");
    u.push_str(core::str::from_utf8(&b32[..n]).expect("base32 is ASCII"));
    u.push_str("&issuer=");
    pct(e.issuer.as_str(), &mut u);
    u.push_str("&algorithm=");
    u.push_str(e.alg.name());
    u.push_str(&format!("&digits={}", e.digits));
    match e.kind {
        Kind::Totp { period } => u.push_str(&format!("&period={period}")),
        Kind::Hotp { counter } => u.push_str(&format!("&counter={counter}")),
    }
    u
}

/// The invariants every entry has, however it was made.
pub fn check_entry(e: &Entry) {
    assert!(!e.secret().is_empty() && e.secret().len() <= MAX_SECRET, "secret length {}", e.secret().len());
    assert!((6..=8).contains(&e.digits), "digits {}", e.digits);
    if let Kind::Totp { period } = e.kind {
        assert!((1..=MAX_PERIOD).contains(&period), "period {period}");
    }
    assert!(e.issuer.as_str().len() <= MAX_ISSUER);
    assert!(e.account.as_str().len() <= MAX_ACCOUNT);
    for t in [0, 59, 1_111_111_109, 1_700_000_000, u64::MAX] {
        let c = e.code(t);
        assert_eq!(c.as_bytes().len(), usize::from(e.digits));
        assert!(c.as_bytes().iter().all(u8::is_ascii_digit));
    }
}

/// `otpauth://` parsing and base32 (see the crate docs).
pub fn otp_uri(data: &[u8]) {
    // base32: whatever bytes, encode then decode is the identity.
    let raw = &data[..data.len().min(MAX_SECRET)];
    let mut text = [0u8; 128];
    let n = base32::encode(raw, &mut text).expect("fits");
    let mut back = [0u8; MAX_SECRET];
    let m = base32::decode(&text[..n], &mut back).expect("own encoding decodes");
    assert_eq!(&back[..m], raw);
    // base32: whatever text decodes, re-encoding gives the same bytes.
    let mut out = [0u8; MAX_SECRET];
    if let Ok(k) = base32::decode(data, &mut out) {
        let n = base32::encode(&out[..k], &mut text).expect("fits");
        let mut again = [0u8; MAX_SECRET];
        let m = base32::decode(&text[..n], &mut again).expect("own encoding decodes");
        assert_eq!(&again[..m], &out[..k]);
    }

    let Ok(s) = core::str::from_utf8(data) else { return };
    let Ok(e) = uri::parse(s) else { return };
    check_entry(&e);
    let canon = to_uri(&e);
    match uri::parse(&canon) {
        Ok(e2) => assert_eq!(e2, e, "canonical URI {canon:?} parses differently"),
        Err(err) => panic!("canonical URI {canon:?} of a parsed entry is rejected: {err:?}"),
    }
}

/// The vault sector in RAM with NOR semantics (program clears bits, erase
/// sets `0xFF`; both checked for alignment).
pub struct Nor(pub Vec<u8>);

impl vault::Storage for &mut Nor {
    type Error = ();
    async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), ()> {
        let o = off as usize;
        let src = self.0.get(o..o + buf.len()).expect("vault reads stay inside the sector");
        buf.copy_from_slice(src);
        Ok(())
    }
    async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
        assert!(off % 4 == 0 && data.len() % 4 == 0, "unaligned program {off} +{}", data.len());
        let o = off as usize;
        let dst = self.0.get_mut(o..o + data.len()).expect("vault writes stay inside the sector");
        for (d, s) in dst.iter_mut().zip(data) {
            *d &= *s;
        }
        Ok(())
    }
    async fn erase(&mut self) -> Result<(), ()> {
        self.0.fill(0xFF);
        Ok(())
    }
}

/// Deterministic "entropy".
pub struct Rng(pub u64);

impl orpheus_otp::Entropy for Rng {
    fn fill(&mut self, buf: &mut [u8]) {
        for b in buf {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            *b = (self.0 >> 56) as u8;
        }
    }
}

pub const DEVICE: &[u8] = b"fuzz-device-id-0";
pub const PIN: &[u8] = b"1234";
/// Vaults with more iterations than this are not unlocked by the target
/// (each unlock runs the whole key derivation).
pub const MAX_FUZZ_ITERATIONS: u32 = 2_000;

/// Byte reader over the fuzz input.
pub struct Ops<'a>(pub &'a [u8]);

impl Ops<'_> {
    pub fn byte(&mut self) -> Option<u8> {
        let (&b, rest) = self.0.split_first()?;
        self.0 = rest;
        Some(b)
    }
    pub fn take(&mut self, n: usize) -> &[u8] {
        let n = n.min(self.0.len());
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        a
    }
    pub fn u64(&mut self) -> u64 {
        let b = self.take(8);
        let mut w = [0u8; 8];
        w[..b.len()].copy_from_slice(b);
        u64::from_le_bytes(w)
    }
}

fn text(b: &[u8]) -> String {
    const SET: &[u8] = b"abcXYZ09 .:@-_%+";
    let mut s: String = b.iter().map(|&c| char::from(SET[usize::from(c) % SET.len()])).collect();
    if b.first().is_some_and(|&c| c & 0x80 != 0) {
        s.push('é');
    }
    s
}

/// An entry made from fuzz bytes (`None` if they make no valid entry).
pub fn entry_from(ops: &mut Ops<'_>) -> Option<Entry> {
    let sel = ops.byte()?;
    let alg = [Algorithm::Sha1, Algorithm::Sha256, Algorithm::Sha512][usize::from(sel % 3)];
    let digits = 6 + (sel >> 2) % 3;
    let kind = if sel & 0x20 != 0 {
        Kind::Hotp { counter: ops.u64() }
    } else {
        Kind::Totp { period: u32::from(ops.byte()?) * 15 + 1 }
    };
    let sl = 1 + usize::from(ops.byte()?) % MAX_SECRET;
    let secret = ops.take(sl).to_vec();
    let il = usize::from(ops.byte()? % 40);
    let issuer = text(ops.take(il));
    let al = usize::from(ops.byte()? % 70);
    let account = text(ops.take(al));
    Entry::new(kind, alg, digits, &secret, &issuer, &account).ok()
}

fn entries(u: &Unlocked) -> Vec<Entry> {
    (0..u.len()).map(|i| u.entry(i).expect("every counted entry decodes")).collect()
}

/// Mount, check the header classification, unlock with `secret`.
fn mount(nor: &mut Nor, now: u64) -> (Vault<&mut Nor>, Result<Status, VaultError<()>>) {
    let hdr: Vec<u8> = nor.0[..80].to_vec();
    let mut v = Vault::new(nor);
    let st = block_on(v.load(now));
    match st {
        Ok(Status::Absent) => {
            assert!(hdr.iter().all(|&b| b == 0xFF), "erased header reported for {hdr:02X?}");
        }
        Ok(Status::Locked { failures, .. }) => {
            assert_eq!(&hdr[..4], b"YVLT");
            assert_eq!(&hdr[76..80], &[0; 4], "a vault without its commit word is not damaged");
            assert_eq!(v.failures(), failures);
            let wait = u64::from(orpheus_otp::backoff::delay_s(failures)) * 1000;
            assert_eq!(v.wait_ms(now), wait);
        }
        Ok(Status::Damaged) | Err(VaultError::UnsupportedVersion) => {}
        Err(e) => panic!("load: {e:?}"),
    }
    (v, st)
}

/// The vault sector and operations on it (see the crate docs). Input:
/// `[n][n operation bytes][sector image…]` (the image is padded with
/// `0xFF` to 4 KiB).
pub fn otp_vault(data: &[u8]) {
    let Some((&n, rest)) = data.split_first() else { return };
    let (ops, img) = rest.split_at(usize::from(n).min(rest.len()));
    let mut nor = Nor(vec![0xFF; SECTOR]);
    let k = img.len().min(SECTOR);
    nor.0[..k].copy_from_slice(&img[..k]);
    let mut ops = Ops(ops);
    let mut rng = Rng(0x5EED);
    let mut now = 1_000u64;
    let mut secret: Vec<u8> = PIN.to_vec();

    // A wrong key on a hostile or real vault fails and arms the back-off.
    let (mut v, st) = mount(&mut nor, now);
    let mut unlocked = None;
    if let Ok(Status::Locked { failures, .. }) = st
        && v.iterations() <= MAX_FUZZ_ITERATIONS
    {
        now += v.wait_ms(now);
        match block_on(v.unlock(b"wrong", DEVICE, now)) {
            Err(VaultError::WrongKey) => {
                assert_eq!(v.failures(), failures.saturating_add(1));
                assert!(v.wait_ms(now) > 0, "no back-off after a wrong key");
            }
            Err(VaultError::Corrupt) => {}
            Ok(_) => panic!("the wrong PIN unlocked the vault"),
            Err(e) => panic!("unlock with a wrong key: {e:?}"),
        }
        now += v.wait_ms(now);
        match block_on(v.unlock(PIN, DEVICE, now)) {
            Ok(u) => unlocked = Some(u),
            Err(VaultError::WrongKey | VaultError::Corrupt) => {}
            Err(e) => panic!("unlock: {e:?}"),
        }
    }
    let mut u = match unlocked {
        Some(u) => u,
        None => {
            if ops.0.is_empty() {
                return;
            }
            block_on(v.create(PIN, SecretKind::Pin, DEVICE, orpheus_otp::kdf::MIN_ITERATIONS, &mut rng))
                .expect("create")
        }
    };
    let mut model = entries(&u);
    for e in &model {
        check_entry(e);
    }

    let mut steps = 0;
    while let Some(op) = ops.byte() {
        steps += 1;
        if steps > 24 {
            break;
        }
        match op % 8 {
            0 | 1 => {
                if let Some(e) = entry_from(&mut ops) {
                    match u.push::<()>(&e) {
                        Ok(()) => model.push(e),
                        Err(VaultError::Full | VaultError::TooManyEntries) => {}
                        Err(err) => panic!("push: {err:?}"),
                    }
                }
            }
            2 => {
                let i = usize::from(ops.byte().unwrap_or(0));
                let ok = u.remove::<()>(i).is_ok();
                assert_eq!(ok, i < model.len());
                if ok {
                    model.remove(i);
                }
            }
            3 => {
                let i = usize::from(ops.byte().unwrap_or(0));
                let c = ops.u64();
                let hotp = model.get(i).is_some_and(|e| matches!(e.kind, Kind::Hotp { .. }));
                assert_eq!(u.set_counter::<()>(i, c).is_ok(), hotp);
                if hotp {
                    model[i].kind = Kind::Hotp { counter: c };
                }
            }
            4 => {
                let i = usize::from(ops.byte().unwrap_or(0));
                let r = block_on(v.hotp_next(&mut u, i, &mut rng));
                match model.get_mut(i) {
                    Some(e) if matches!(e.kind, Kind::Hotp { .. }) => {
                        let code = r.expect("HOTP step");
                        assert_eq!(code, e.code(0), "the code of the counter before the step");
                        if let Kind::Hotp { counter } = &mut e.kind {
                            *counter = counter.wrapping_add(1);
                        }
                    }
                    _ => assert!(matches!(r, Err(VaultError::BadIndex))),
                }
            }
            5 => {
                block_on(v.save(&mut u, &mut rng)).expect("save");
            }
            6 => {
                let s = ops.take(6).to_vec();
                let kind = if s.len() > 4 { SecretKind::Passphrase } else { SecretKind::Pin };
                block_on(v.change_secret(
                    &mut u,
                    &s,
                    kind,
                    DEVICE,
                    orpheus_otp::kdf::MIN_ITERATIONS,
                    &mut rng,
                ))
                .expect("change secret");
                secret = s;
            }
            _ => {
                // Remount and unlock: exactly what was saved comes back.
                block_on(v.save(&mut u, &mut rng)).expect("save");
                drop(u);
                let (mut v2, st) = mount(&mut nor, now);
                assert!(matches!(st, Ok(Status::Locked { failures: 0, .. })), "{st:?}");
                let u2 = block_on(v2.unlock(&secret, DEVICE, now)).expect("unlock what was just saved");
                assert_eq!(entries(&u2), model, "entries after remount");
                v = v2;
                u = u2;
            }
        }
        assert_eq!(u.len(), model.len());
        assert!(u.used() <= vault::PAYLOAD_MAX);
        for (i, e) in model.iter().enumerate() {
            assert_eq!(u.entry(i).as_ref(), Some(e), "entry {i}");
        }
        assert!(u.entry(model.len()).is_none());
    }
}
