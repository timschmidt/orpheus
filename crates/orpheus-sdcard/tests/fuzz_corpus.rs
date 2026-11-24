//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-sdcard --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/sd_registers/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

#[test]
fn sd_registers_corpus() {
    replay::replay("sd_registers", bodies::sd_registers);
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

#[test]
fn seeds() {
    let s: Vec<(String, Vec<u8>)> = [
        // CSD v2 of a 32 GB SDHC card, CID of a SanDisk card (the unit
        // tests' registers), a CSD v1 (2 GB), a CSD v3, a reserved
        // structure, R1/OCR words.
        ("csd-v2", "400E00325B590000EDC77F800A400000"),
        ("cid", "035344534E3634478012345678015300"),
        ("csd-v1", "002E00325B5A83A9FFFFFF800A800000"),
        ("csd-v3", "800E00325B590001FFFF7F800A400000"),
        ("csd-reserved", "C00E00325B590000EDC77F800A400000"),
        ("r1-ocr", "00000900000009000000B00080FF80C0"),
    ]
    .iter()
    .map(|(n, h)| ((*n).to_string(), hex(h)))
    .collect();
    for (_, d) in &s {
        bodies::sd_registers(d);
    }
    replay::write_seeds("sd_registers", &s);
}
