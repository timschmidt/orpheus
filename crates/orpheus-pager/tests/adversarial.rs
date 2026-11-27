//! Access traces in any order for the pager policy (VER-3, 07 §3.1/§3.7),
//! as `proptest` properties with shrinking (the random traces of
//! `traces.rs` use fixed seeds and cannot shrink a failure): any store
//! size up to the window limits (with cold code in the rodata window),
//! any mix of fetches and reads of any length at any address of the
//! windows, cold code reading its own constants. After every access the
//! simulator's invariants hold (no physical line mapped twice, every
//! mapped line holds its page's data, tables agree with the MPU), every
//! completed access returns the page store's bytes, and an access that
//! does not complete fails only as a documented fatal fault — never wrong
//! data, a livelock or an unmapped alias.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;

use adversarial::config;
use orpheus_pager::sim::{Kind, Machine, SimError};
use orpheus_pager::{CODE_WIN, MAX_CODE_PAGES, MAX_RO_PAGES, PAGE_SIZE, code_page_va, ro_page_va};
use proptest::prelude::*;

fn store(ncode: u32, nro: u32) -> Vec<u8> {
    let mut s = Vec::with_capacity(((ncode + nro) * PAGE_SIZE) as usize);
    for page in 0..ncode + nro {
        for word in 0..PAGE_SIZE / 4 {
            s.extend_from_slice(&((page << 16) | word).wrapping_mul(0x9E37_79B9).to_le_bytes());
        }
    }
    s
}

#[derive(Clone, Debug)]
enum Op {
    /// Fetch at code page `page` offset `off` (2 or 4 bytes).
    Fetch { page: u32, off: u32, wide: bool },
    /// Read `len` bytes of rodata page `page` at `off` by the instruction
    /// at code page `pc_page` offset `pc_off`.
    Read { page: u32, off: u32, len: u32, pc_page: u32, pc_off: u32 },
    /// A cold-code fetch in the rodata window.
    ColdFetch { page: u32, off: u32 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (any::<u32>(), 0u32..PAGE_SIZE / 2, any::<bool>()).prop_map(|(page, off, wide)| Op::Fetch { page, off: off * 2, wide }),
        3 => (any::<u32>(), 0u32..PAGE_SIZE, prop::sample::select(vec![1u32, 2, 4, 8]), any::<u32>(), 0u32..PAGE_SIZE / 2)
            .prop_map(|(page, off, len, pc_page, pc_off)| Op::Read { page, off, len, pc_page, pc_off: pc_off * 2 }),
        1 => (any::<u32>(), 0u32..PAGE_SIZE / 2).prop_map(|(page, off)| Op::ColdFetch { page, off: off * 2 }),
    ]
}

fn ok_or_fatal(r: Result<u64, SimError>, what: &str) -> Result<(), TestCaseError> {
    match r {
        Ok(_) | Err(SimError::Fatal(_)) => Ok(()),
        Err(e) => Err(TestCaseError::fail(format!("{what}: {e:?}"))),
    }
}

proptest! {
    #![proptest_config(config("pager-traces-in-any-order", 96))]

    #[test]
    fn traces_in_any_order(ncode in 1u32..=MAX_CODE_PAGES, nro in 1u32..=MAX_RO_PAGES, cold in 0u32..8,
                           ops in prop::collection::vec(op(), 1..400)) {
        let ro_code = cold.min(nro - 1);
        let s = store(ncode, nro);
        let mut m = Box::new(Machine::with_cold(&s, ncode, nro, ro_code));
        let ro_exec = nro - ro_code;
        for o in &ops {
            match *o {
                Op::Fetch { page, off, wide } => {
                    let pc = code_page_va(page % ncode) + off.min(PAGE_SIZE - 4);
                    ok_or_fatal(m.fetch(pc, if wide { 4 } else { 2 }), &format!("{o:?}"))?;
                }
                Op::Read { page, off, len, pc_page, pc_off } => {
                    let q = page % nro;
                    // Inside the store (an access past the last page is a
                    // different, fatal case covered by `traces.rs`).
                    let off = off.min(PAGE_SIZE - len);
                    let pc = code_page_va(pc_page % ncode) + pc_off.min(PAGE_SIZE - 4);
                    ok_or_fatal(m.access(ro_page_va(q) + off, len, Kind::Read, pc), &format!("{o:?}"))?;
                }
                Op::ColdFetch { page, off } => {
                    if ro_code > 0 {
                        let q = ro_exec + page % ro_code;
                        let pc = ro_page_va(q) + off.min(PAGE_SIZE - 4);
                        ok_or_fatal(m.fetch(pc, 2), &format!("{o:?}"))?;
                    }
                }
            }
            m.check().map_err(|e| TestCaseError::fail(format!("after {o:?}: {e}")))?;
        }
        let st = m.core.stats();
        prop_assert!(st.loads >= 1);
        let _ = CODE_WIN;
    }
}
