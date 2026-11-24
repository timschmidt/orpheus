//! Pager policy against a simulated MPU (docs/plan/07-tooling-testing.md
//! §3.1): structured and random access traces; after every handled fault
//! the simulator checks that no physical line is mapped twice and that
//! every mapped line holds its page's data, and every completed access is
//! compared with the page store.

use orpheus_pager::sim::{Kind, Machine, SimError};
use orpheus_pager::{
    Action, CODE_WIN, Fatal, LINE_SIZE, PAGE_SIZE, Page, PagerCore, RO_WIN, Stats, code_page_va, ro_page_va,
};

/// A page store whose every word names its page and position.
fn store(ncode: u32, nro: u32) -> Vec<u8> {
    let mut s = Vec::with_capacity(((ncode + nro) * PAGE_SIZE) as usize);
    for page in 0..ncode + nro {
        for word in 0..PAGE_SIZE / 4 {
            s.extend_from_slice(&((page << 16) | word).wrapping_mul(0x9E37_79B9).to_le_bytes());
        }
    }
    s
}

fn machine(s: &[u8], ncode: u32, nro: u32) -> Box<Machine<'_>> {
    Box::new(Machine::new(s, ncode, nro))
}

/// Fetch, then check the invariants.
fn fetch(m: &mut Machine<'_>, pc: u32, len: u32) {
    m.fetch(pc, len).unwrap_or_else(|e| panic!("fetch {pc:#x}: {e:?}"));
    m.check().unwrap_or_else(|e| panic!("after fetch {pc:#x}: {e}"));
}

fn read(m: &mut Machine<'_>, addr: u32, len: u32, pc: u32) {
    m.access(addr, len, Kind::Read, pc).unwrap_or_else(|e| panic!("read {addr:#x} pc {pc:#x}: {e:?}"));
    m.check().unwrap_or_else(|e| panic!("after read {addr:#x}: {e}"));
}

/// Small deterministic generator (xorshift32).
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

#[test]
fn sequential_sweeps_load_each_line_once() {
    let s = store(12, 6);
    let mut m = machine(&s, 12, 6);
    for pc in (code_page_va(0)..code_page_va(12)).step_by(2) {
        fetch(&mut m, pc, 2);
    }
    for q in 0..6 {
        for a in (ro_page_va(q)..ro_page_va(q) + PAGE_SIZE).step_by(4) {
            read(&mut m, a, 4, CODE_WIN);
        }
    }
    let st = m.core.stats();
    assert_eq!(st.loads, 12 * 8 + 6 * 8);
    assert_eq!(st.fatal, 0);
    assert_eq!(m.nor_reads, st.loads);
}

#[test]
fn loop_that_fits_runs_fault_free_after_warm_up() {
    let s = store(8, 0);
    let mut m = machine(&s, 8, 0);
    // 24 lines: pages 0-2, three different slots.
    for _ in 0..50 {
        for pc in (CODE_WIN..CODE_WIN + 24 * LINE_SIZE).step_by(64) {
            fetch(&mut m, pc, 4);
        }
    }
    assert_eq!(m.core.stats().loads, 24);
}

#[test]
fn loop_larger_than_the_cache_thrashes_predictably() {
    let s = store(8, 0);
    let mut m = machine(&s, 8, 0);
    // 40 lines: pages 0-4; pages 0 and 4 share slot 0 line for line.
    let iters = 20;
    for _ in 0..iters {
        for pc in (CODE_WIN..CODE_WIN + 40 * LINE_SIZE).step_by(64) {
            fetch(&mut m, pc, 2);
        }
    }
    // First pass: 40 loads. Later passes: page 0 and page 4 take slot 0 from
    // each other, 16 loads per pass.
    assert_eq!(m.core.stats().loads, 40 + (iters - 1) * 16);
    assert_eq!(m.core.stats().fatal, 0);
}

#[test]
fn pages_sharing_a_slot_coexist_on_disjoint_lines() {
    let s = store(8, 0);
    let mut m = machine(&s, 8, 0);
    let a = code_page_va(0) + LINE_SIZE; // slot 0, line 1
    let b = code_page_va(4) + 5 * LINE_SIZE; // slot 0, line 5
    for _ in 0..1000 {
        fetch(&mut m, a, 2);
        fetch(&mut m, b, 2);
    }
    assert_eq!(m.core.stats().loads, 2);
    assert_eq!(m.faults, 2);

    // Same line: every switch takes the physical line from the other page.
    let s = store(8, 0);
    let mut m = machine(&s, 8, 0);
    let a = code_page_va(1) + 3 * LINE_SIZE;
    let b = code_page_va(5) + 3 * LINE_SIZE;
    for _ in 0..100 {
        fetch(&mut m, a, 2);
        fetch(&mut m, b, 2);
    }
    assert_eq!(m.core.stats().loads, 200);
}

#[test]
fn reused_region_gets_its_intact_lines_back_without_a_read() {
    let s = store(16, 0);
    let mut m = machine(&s, 16, 0);
    // Page 0, lines 0-2.
    for l in 0..3 {
        fetch(&mut m, CODE_WIN + l * LINE_SIZE, 2);
    }
    // Six other pages in slots 1-3 evict page 0's region but not its lines.
    for p in [1, 2, 3, 5, 6, 7] {
        fetch(&mut m, code_page_va(p) + 6 * LINE_SIZE, 2);
    }
    assert!((0..6).all(|r| m.core.region(r).map(|x| x.0) != Some(Page::Code(0))));
    let loads = m.core.stats().loads;
    // Coming back: one remap brings lines 0-2 back at once.
    for l in 0..3 {
        fetch(&mut m, CODE_WIN + l * LINE_SIZE, 2);
    }
    assert_eq!(m.core.stats().loads, loads);
    assert_eq!(m.core.stats().remaps, 1);
}

#[test]
fn straddling_instruction_at_a_line_end() {
    let s = store(4, 0);
    let mut m = machine(&s, 4, 0);
    let pc = code_page_va(1) + LINE_SIZE - 2;
    fetch(&mut m, pc, 4);
    let st = m.core.stats();
    assert_eq!((st.loads, st.straddles, m.faults), (2, 1, 2));
    // The next line is already there now.
    fetch(&mut m, pc + 2, 2);
    assert_eq!(m.faults, 2);
}

#[test]
fn straddling_data_access_at_a_line_end() {
    let s = store(2, 2);
    let mut m = machine(&s, 2, 2);
    fetch(&mut m, CODE_WIN, 2);
    // An 8-byte read (LDRD/LDM) across the line boundary, line 0 not loaded.
    read(&mut m, ro_page_va(0) + 3 * LINE_SIZE - 4, 8, CODE_WIN);
    let st = m.core.stats();
    assert_eq!((st.loads, st.straddles), (3, 1));
    // Line 0 already loaded, line 1 not: one fault, resolved by the rule.
    read(&mut m, ro_page_va(0) + 6 * LINE_SIZE - 2, 4, CODE_WIN);
    read(&mut m, ro_page_va(0) + 5 * LINE_SIZE + 8, 4, CODE_WIN);
    read(&mut m, ro_page_va(0) + 6 * LINE_SIZE - 2, 4, CODE_WIN);
}

#[test]
fn straddling_into_the_next_code_page() {
    let s = store(8, 0);
    let mut m = machine(&s, 8, 0);
    // Page 3 ends one alias block; page 4 starts the next, VA-contiguous.
    for p in [1, 3] {
        let pc = code_page_va(p + 1) - 2;
        assert_eq!(pc + 2, code_page_va(p + 1));
        fetch(&mut m, pc, 4);
    }
    let st = m.core.stats();
    assert_eq!((st.loads, st.straddles), (4, 2));
}

#[test]
fn straddling_rodata_inside_and_out_of_a_group() {
    let s = store(1, 4);
    let mut m = machine(&s, 1, 4);
    fetch(&mut m, CODE_WIN, 2);
    // Page 0 → page 1: one 16 KiB group.
    read(&mut m, ro_page_va(1) - 2, 4, CODE_WIN);
    // Page 1 → beyond the group (SRAM3 RAM half): not a paging fault.
    let r = m.access(ro_page_va(1) + PAGE_SIZE - 2, 4, Kind::Read, CODE_WIN);
    assert_eq!(r, Err(SimError::Fatal(Fatal::NotAPagingFault)));
    // Last code page: nothing follows.
    let r = m.fetch(code_page_va(1) - 2, 4);
    assert_eq!(r, Err(SimError::Fatal(Fatal::NotAPagingFault)));
}

#[test]
fn second_fault_on_mapped_lines_is_not_a_paging_fault() {
    let mut c = PagerCore::new();
    c.init(4, 0);
    assert!(matches!(c.fault(CODE_WIN, CODE_WIN), Action::Load { .. }));
    assert!(matches!(c.fault(CODE_WIN + LINE_SIZE, CODE_WIN), Action::Load { .. }));
    // Line 0 and line 1 mapped: a write to line 0, say.
    assert_eq!(c.fault(CODE_WIN + 8, CODE_WIN), Action::Fatal(Fatal::NotAPagingFault));
}

#[test]
fn the_page_holding_the_faulting_pc_is_never_evicted() {
    let s = store(16, 8);
    let mut m = machine(&s, 16, 8);
    let pc = code_page_va(0) + 0x100;
    fetch(&mut m, pc, 2);
    // Page 0 is now the least recently faulted region; each read below
    // needs a new region.
    for (i, q) in (0..8).enumerate() {
        read(&mut m, ro_page_va(q) + 0x40, 4, pc);
        let p = 2 + i as u32 * 3 % 13;
        if p & 3 != 0 {
            read(&mut m, code_page_va(p) + 0x700, 4, pc);
        }
        assert!(
            (0..6).any(|r| m.core.region(r).map(|x| x.0) == Some(Page::Code(0))),
            "page 0 evicted while executing from it"
        );
    }
    let faults = m.faults;
    fetch(&mut m, pc + 2, 2);
    assert_eq!(m.faults, faults);
    assert!(m.core.stats().evictions > 0);
}

#[test]
fn data_on_the_same_physical_line_as_the_pc_is_fatal_not_a_livelock() {
    let s = store(8, 0);
    let mut m = machine(&s, 8, 0);
    let pc = code_page_va(0) + 3 * LINE_SIZE + 0x10;
    fetch(&mut m, pc, 2);
    let r = m.access(code_page_va(4) + 3 * LINE_SIZE + 0x20, 4, Kind::Read, pc);
    assert_eq!(r, Err(SimError::Fatal(Fatal::LineConflict)));
}

#[test]
fn faults_outside_the_store_are_fatal() {
    let mut c = PagerCore::new();
    assert_eq!(c.fault(CODE_WIN, 0), Action::Fatal(Fatal::Uninitialised));
    c.init(4, 2);
    assert_eq!(c.fault(code_page_va(4), 0), Action::Fatal(Fatal::NoSuchPage));
    assert_eq!(c.fault(ro_page_va(2), 0), Action::Fatal(Fatal::NoSuchPage));
    assert_eq!(c.fault(0x30_0000, 0), Action::Fatal(Fatal::NotPaged));
    assert_eq!(c.fault(RO_WIN + 0x4000, 0), Action::Fatal(Fatal::NotPaged));
    assert_eq!(c.fault(0x10_0000, 0), Action::Fatal(Fatal::NotPaged));
    assert_eq!(c.stats().fatal, 5, "init() resets the counters");
}

#[test]
fn load_actions_address_the_store_and_the_slot() {
    let mut c = PagerCore::new();
    c.init(10, 4);
    let Action::Load { map, nor_offset, phys, unmap } = c.fault(code_page_va(6) + 5 * LINE_SIZE + 4, 0)
    else {
        panic!()
    };
    assert_eq!((nor_offset, phys, unmap), (6 * PAGE_SIZE + 5 * LINE_SIZE, 0x20_4000 + 5 * LINE_SIZE, None));
    assert_eq!((map.region, map.base, map.srd, map.xn), (1, code_page_va(6), !0x20, false));
    let Action::Load { map, nor_offset, phys, .. } = c.fault(ro_page_va(3) + 7 * LINE_SIZE, 0) else {
        panic!()
    };
    assert_eq!((nor_offset, phys), ((10 + 3) * PAGE_SIZE + 7 * LINE_SIZE, 0x30_2000 + 7 * LINE_SIZE));
    assert_eq!((map.region, map.base, map.srd, map.xn), (2, ro_page_va(3), !0x80, true));
    // Taking slot 2 line 5 from page 6 unmaps it there.
    let Action::Load { unmap, .. } = c.fault(code_page_va(2) + 5 * LINE_SIZE, 0) else { panic!() };
    let u = unmap.expect("line taken from page 6");
    assert_eq!((u.region, u.base, u.srd), (1, code_page_va(6), 0xFF));
}

#[test]
fn aborted_load_leaves_the_line_empty() {
    let mut c = PagerCore::new();
    c.init(4, 0);
    let Action::Load { phys, .. } = c.fault(CODE_WIN + 2 * LINE_SIZE, 0) else { panic!() };
    let m = c.abort_load(phys).expect("region");
    assert_eq!(m.srd, 0xFF);
    assert_eq!(c.line_owner(0, 2), None);
    assert!(matches!(c.fault(CODE_WIN + 2 * LINE_SIZE, 0), Action::Load { .. }));
}

/// A program-like trace: a PC that mostly runs straight on, calls and
/// returns between functions spread over the code pages, literal-pool reads
/// near the PC, rodata reads with some locality, unaligned and straddling
/// accesses included.
fn program_trace(m: &mut Machine<'_>, seed: u32, steps: u32, ncode: u32, nro: u32) {
    let mut rng = Rng(seed);
    let code_end = code_page_va(ncode);
    let mut pc = CODE_WIN;
    let mut stack: Vec<u32> = Vec::new();
    let mut ro_cursor = RO_WIN;
    for _ in 0..steps {
        let len = if rng.below(3) == 0 { 4 } else { 2 };
        if pc + len > code_end {
            pc = CODE_WIN;
        }
        fetch(m, pc, len);
        match rng.below(100) {
            0..=4 if stack.len() < 16 => {
                stack.push(pc + len);
                pc = CODE_WIN + 2 * rng.below((code_end - CODE_WIN) / 2 - 2);
                continue;
            }
            5..=9 if !stack.is_empty() => {
                pc = stack.pop().unwrap_or(CODE_WIN);
                continue;
            }
            10..=17 => {
                // Literal pool: same page as the PC, ahead of it.
                let a = (pc + 4 + 4 * rng.below(64)) & !3;
                if Page::of(a) == Page::of(pc) {
                    read(m, a, 4, pc);
                }
            }
            18..=30 if nro > 0 => {
                if rng.below(8) == 0 {
                    let q = rng.below(nro);
                    ro_cursor = ro_page_va(q) + rng.below(PAGE_SIZE);
                }
                let n = [1, 2, 4, 8][rng.below(4) as usize];
                let page = Page::of(ro_cursor).expect("rodata page");
                let end = page.va() + PAGE_SIZE;
                // Stay inside the page's group (a real object never leaves it).
                let a = ro_cursor.min(end - n);
                read(m, a, n, pc);
                ro_cursor = a + n;
                if ro_cursor >= end {
                    ro_cursor = page.va();
                }
            }
            _ => {}
        }
        pc += len;
    }
}

#[test]
fn random_program_traces_keep_the_invariants() {
    for seed in [1, 0xC0FFEE, 0xDEAD_BEEF, 12345] {
        let (ncode, nro) = (40, 12);
        let s = store(ncode, nro);
        let mut m = machine(&s, ncode, nro);
        program_trace(&mut m, seed, 60_000, ncode, nro);
        assert_eq!(m.core.stats().fatal, 0);
    }
}

#[test]
fn random_scattered_accesses_keep_the_invariants() {
    let (ncode, nro) = (24, 8);
    let s = store(ncode, nro);
    let mut m = machine(&s, ncode, nro);
    let mut rng = Rng(77);
    for _ in 0..50_000 {
        let pc = CODE_WIN + 2 * rng.below(ncode * PAGE_SIZE / 2 - 2);
        fetch(&mut m, pc, 2 + 2 * rng.below(2));
        let q = rng.below(nro);
        let n = [1, 2, 4, 8][rng.below(4) as usize];
        let a = ro_page_va(q & !1) + rng.below(2 * PAGE_SIZE - n);
        if q & !1 < nro - 1 || a + n <= ro_page_va(q & !1) + PAGE_SIZE {
            read(&mut m, a, n, pc);
        }
    }
    assert_eq!(m.core.stats().fatal, 0);
}

/// Fault counts of a fixed reference trace. A change here is a change of
/// policy: explain it in the commit.
#[test]
fn reference_trace_fault_counts() {
    let (ncode, nro) = (32, 10);
    let s = store(ncode, nro);
    let mut m = machine(&s, ncode, nro);
    program_trace(&mut m, 0x5EED, 100_000, ncode, nro);
    let got = m.core.stats();
    let want = Stats {
        code_faults: 6654,
        ro_faults: 2035,
        loads: 6685,
        remaps: 2004,
        straddles: 43,
        evictions: 7329,
        fatal: 0,
    };
    assert_eq!(got, want, "faults {}", m.faults);
    assert_eq!(m.faults, 8689);
}
