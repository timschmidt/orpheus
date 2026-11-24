//! Mount + directory iteration (+ some writes) on mutated FAT12/16/32 and
//! exFAT images. The input selects a base image and patches bytes in its
//! first 16 MiB (see `yoton_fs::host::fuzz_one`).
//!
//! Base images come from `cargo test -p yoton-fs --test fuzz_smoke`, which
//! writes them to `target/tmp/yoton-fs/fuzz-seeds/`; override the directory
//! with `YOTON_FS_SEEDS`.
#![no_main]

use std::sync::LazyLock;

use libfuzzer_sys::fuzz_target;

static BASES: LazyLock<Vec<Vec<u8>>> = LazyLock::new(|| {
    let dir = std::env::var("YOTON_FS_SEEDS").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../target/tmp/yoton-fs/fuzz-seeds").to_string()
    });
    let mut v = Vec::new();
    for name in ["fat12", "fat16", "fat32", "exfat"] {
        let p = format!("{dir}/{name}.img");
        match std::fs::read(&p) {
            Ok(b) => v.push(b),
            Err(e) => panic!("seed {p}: {e} (run `cargo test -p yoton-fs --test fuzz_smoke` first)"),
        }
    }
    v
});

fuzz_target!(|data: &[u8]| {
    let refs: Vec<&[u8]> = BASES.iter().map(Vec::as_slice).collect();
    yoton_fs::host::fuzz_one(&refs, data);
});
