//! Adversarial Sequencer project files (VER-3, 07 §3.7) through the fuzz
//! body (`fuzz/src/lib.rs`: no panic; a file that loads saves within
//! `MAX_FILE`, loads back to the same project and saves the same bytes;
//! the project plays and exports within its bounds). Every seed swept
//! with the CRC made right (option bit 0 clear) and as it is (bit 0 set):
//! cut at every byte of its head, damaged, 16/32-bit fields at their
//! edges; and any record bytes with a right CRC (proptest).

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
mod bodies;

use adversarial::{config, seeds, sweep};
use proptest::prelude::*;

#[test]
fn projects_swept() {
    // The large seeds (songs, 17–25 KB) have the same header and records
    // as the small ones; they get the proptest below.
    for (name, s) in seeds("seq_load").into_iter().filter(|(_, s)| s.len() < 4096) {
        eprintln!("{name}");
        for opt in [0u8, 1] {
            let mut d = s.clone();
            d[0] = opt | (s[0] & 0xF0);
            sweep(&d, 1, 64, bodies::seq_load);
        }
    }
}

proptest! {
    #![proptest_config(config("seq-records", 64))]

    /// A real project's bytes with any of them overwritten, CRC right.
    #[test]
    fn any_project_bytes(which in any::<usize>(), opt in any::<u8>(),
                         patches in prop::collection::vec((any::<u16>(), any::<u8>()), 1..24)) {
        let all = seeds("seq_load");
        let (_, s) = &all[which % all.len()];
        let mut d = s.clone();
        d[0] = opt & !1;
        let n = d.len().saturating_sub(5).max(1);
        for (at, v) in patches {
            let i = 1 + usize::from(at) % n;
            if i < d.len() {
                d[i] = v;
            }
        }
        bodies::seq_load(&d);
    }
}
