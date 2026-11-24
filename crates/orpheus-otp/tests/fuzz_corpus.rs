//! The fuzz targets' committed corpora and regression inputs, replayed on
//! stable through the same bodies the cargo-fuzz targets run (VER-4); and
//! the seed inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-otp --test
//! fuzz_corpus` rewrites `tests/fuzz-corpus/<target>/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use bodies::{DEVICE, Nor, Ops, PIN, Rng, entry_from, to_uri};
use embassy_futures::block_on;
use orpheus_otp::vault::SECTOR;
use orpheus_otp::{SecretKind, Vault};

#[test]
fn otp_uri_corpus() {
    replay::replay("otp_uri", bodies::otp_uri);
}

#[test]
fn otp_vault_corpus() {
    replay::replay("otp_vault", bodies::otp_vault);
}

fn uri_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = [
        "otpauth://totp/Example:alice@google.com?secret=JBSWY3DPEHPK3PXP&issuer=Example",
        "otpauth://totp/ACME%20Co:john.doe@email.com?secret=HXDMVJECJJWSRB3HWIZR4IFUGFTMXBOZ&issuer=ACME%20Co&algorithm=SHA1&digits=6&period=30",
        "otpauth://hotp/A%3Ab?secret=GEZDGNBV&counter=7&digits=8&algorithm=SHA256",
        "otpauth://totp/x?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ%3D%3D&issuer=%E2%82%AC",
        "otpauth://totp/%C3%A9t%C3%A9:Bob?secret=gezd-gnbv gy3t&algorithm=sha512&period=3600&image=x",
        " otpauth://TOTP/a?secret=MFRGG===&digits=7 ",
    ]
    .iter()
    .enumerate()
    .map(|(i, s)| (format!("doc-{i}"), s.as_bytes().to_vec()))
    .collect();
    // Canonical URIs of entries made from pseudo-random bytes.
    let mut r = Rng(7);
    let mut made = 0;
    for i in 0..64 {
        let mut b = [0u8; 160];
        orpheus_otp::Entropy::fill(&mut r, &mut b);
        if let Some(e) = entry_from(&mut Ops(&b)) {
            v.push((format!("canon-{i}"), to_uri(&e).into_bytes()));
            made += 1;
            if made == 8 {
                break;
            }
        }
    }
    v
}

/// A vault image made through the API, trailing erased bytes cut (the
/// target pads them back).
fn vault_image(entries: usize, attempts: u32) -> Vec<u8> {
    let mut nor = Nor(vec![0xFF; SECTOR]);
    let mut rng = Rng(entries as u64);
    {
        let mut v = Vault::new(&mut nor);
        let mut u =
            block_on(v.create(PIN, SecretKind::Pin, DEVICE, orpheus_otp::kdf::MIN_ITERATIONS, &mut rng))
                .unwrap();
        let mut b = [0u8; 4096];
        orpheus_otp::Entropy::fill(&mut rng, &mut b);
        let mut ops = Ops(&b);
        while u.len() < entries {
            if let Some(e) = entry_from(&mut ops) {
                u.push::<()>(&e).unwrap();
            }
        }
        block_on(v.save(&mut u, &mut rng)).unwrap();
        for _ in 0..attempts {
            block_on(v.load(0)).unwrap();
            let _ = block_on(v.unlock(b"bad", DEVICE, 1u64 << 40));
        }
    }
    let mut img = nor.0;
    while img.last() == Some(&0xFF) {
        img.pop();
    }
    img
}

fn vault_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    // [n][ops][image]
    let with = |ops: &[u8], img: &[u8]| {
        let mut d = vec![ops.len() as u8];
        d.extend_from_slice(ops);
        d.extend_from_slice(img);
        d
    };
    // Erased sector, operations create a vault and use it.
    let ops: &[u8] = &[0, 0x21, 1, 2, 3, 4, 5, 6, 7, 8, 9, 7, 4, 0, 5, 3, 0, 9, 9, 9, 9, 9, 9, 9, 9, 7];
    v.push(("erased-ops".to_string(), with(ops, &[])));
    for (n, att) in [(0, 0), (1, 0), (3, 2), (6, 9)] {
        let img = vault_image(n, att);
        v.push((format!("vault-{n}-{att}"), with(&[], &img)));
        v.push((format!("vault-{n}-{att}-ops"), with(&[2, 0, 5, 7, 1, 0x30, 9, 4, 0, 7], &img)));
    }
    // Torn write: header without its commit word.
    let mut torn = vault_image(2, 0);
    torn[76..80].fill(0xFF);
    v.push(("torn".to_string(), with(&[], &torn)));
    v
}

#[test]
fn seeds() {
    let u = uri_seeds();
    for (_, d) in &u {
        bodies::otp_uri(d);
    }
    let vs = vault_seeds();
    for (_, d) in &vs {
        bodies::otp_vault(d);
    }
    replay::write_seeds("otp_uri", &u);
    replay::write_seeds("otp_vault", &vs);
}
