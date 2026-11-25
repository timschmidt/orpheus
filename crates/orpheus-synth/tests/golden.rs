//! Golden renders: the FNV-1a hash of every scenario's `i16` samples
//! (`orpheus_synth::demo::SCENARIOS`: each melodic preset playing a phrase,
//! each drum voice, the kit, glide, stealing, each effect, LFO sync,
//! parameter sweeps, a four-second demo at 48 and 44.1 kHz) against
//! `tests/goldens.txt`.
//!
//! The same table is checked by the `render` example in a release build
//! (`cargo run --release -p orpheus-synth --example render -- --check`):
//! the same samples at another optimisation level. The player checks it in
//! SYN-3. After an intended change of the sound:
//! `ORPHEUS_WRITE_GOLDENS=1 cargo test -p orpheus-synth --test golden`, and
//! listen to the WAVs from `cargo run --release -p orpheus-synth --example
//! render -- <dir>`. (The WAVs are not committed: 33 s of stereo is 6 MB.)

use orpheus_synth::demo::{SCENARIOS, hash_i16};

const FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens.txt");

#[test]
fn golden_renders() {
    let mut table = String::new();
    for sc in &SCENARIOS {
        let mut out = vec![0i16; 2 * sc.frames()];
        sc.render_i16(&mut out, 256);
        let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak > 1000, "{}: silent (peak {peak})", sc.name);
        table.push_str(&format!("{} {:016x} {}\n", sc.name, hash_i16(&out), sc.frames()));
    }
    if std::env::var_os("ORPHEUS_WRITE_GOLDENS").is_some() {
        std::fs::write(FILE, &table).unwrap();
        return;
    }
    let want =
        std::fs::read_to_string(FILE).expect("tests/goldens.txt missing: run with ORPHEUS_WRITE_GOLDENS=1");
    let mut bad = Vec::new();
    for (got, want) in table.lines().zip(want.lines()) {
        if got != want {
            bad.push(format!("  got  {got}\n  want {want}"));
        }
    }
    assert_eq!(table.lines().count(), want.lines().count(), "scenario list changed");
    assert!(bad.is_empty(), "golden renders differ:\n{}", bad.join("\n"));
}
