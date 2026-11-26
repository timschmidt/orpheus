//! Fuzz-target body of `orpheus-seq` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`seq_load`] — arbitrary bytes as a project file. The CRC would refuse
//! nearly every mutation, so the first byte chooses: bit 0 set, the rest
//! is taken as a file as it is; clear, the rest is a file whose last four
//! bytes are replaced by the right CRC (so the record decoder is reached).
//! Oracles: no panic; a file that loads saves into at most `MAX_FILE`
//! bytes, and that save loads back into the same project and saves the
//! same bytes; the loaded project plays (a dry run of 4 s at 8 kHz and a
//! short render): hits in frame order, notes ≤ 127, velocities 1–127,
//! nothing dropped, every sample finite.

#![forbid(unsafe_code)]

use orpheus_seq::file::{MAX_FILE, crc32};
use orpheus_seq::{Project, Sequencer, load_into, save};

/// Bytes the body looks at (larger inputs are cut).
pub const MAX_INPUT: usize = 64 * 1024;

/// The target body.
pub fn seq_load(data: &[u8]) {
    let Some((&opt, rest)) = data.split_first() else { return };
    let rest = &rest[..rest.len().min(MAX_INPUT)];
    let mut file = rest.to_vec();
    if opt & 1 == 0 && file.len() >= 4 {
        let n = file.len() - 4;
        let c = crc32(0, &file[..n]);
        file[n..].copy_from_slice(&c.to_le_bytes());
    }
    let _ = orpheus_seq::file::peek_name(&file);
    let mut p = Box::new(Project::NEW);
    if load_into(&mut p, &file).is_err() {
        return;
    }
    let mut buf = vec![0u8; MAX_FILE];
    let n = save(&p, &mut buf).expect("a loaded project fits MAX_FILE");
    let mut q = Box::new(Project::NEW);
    load_into(&mut q, &buf[..n]).expect("a saved project loads");
    assert!(p.same_as(&q), "load → save → load keeps the project");
    let mut buf2 = vec![0u8; MAX_FILE];
    let n2 = save(&q, &mut buf2).expect("fits");
    assert_eq!(buf[..n], buf2[..n2], "and saves the same bytes");
    play(&p, opt);
}

fn play(p: &Project, opt: u8) {
    let mut s = Sequencer::new(8_000);
    s.play();
    let mut last = 0;
    let mut n = 0u32;
    s.run_dry(p, 4 * 8_000, |h| {
        assert!(h.frame >= last, "hits in order");
        last = h.frame;
        assert!(h.note <= 127 && (1..=127).contains(&h.velocity));
        n += 1;
    });
    assert_eq!(s.dropped(), 0);
    if opt & 2 != 0 {
        let mut e = Box::new(orpheus_synth::Engine::new(8_000, 1));
        let mut s = Sequencer::new(8_000);
        s.play();
        let mut out = vec![0f32; 2 * 997];
        for _ in 0..4 {
            s.render_f32(p, &mut e, &mut out);
            assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        }
    }
}
