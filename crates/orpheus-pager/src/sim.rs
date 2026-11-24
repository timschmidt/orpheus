//! A simulated MPU, SRAM2/SRAM3 and NOR page store around [`PagerCore`]
//! (feature `sim`): host tests replay access traces through it, `xtask`
//! replays a real page store.
//!
//! The model follows ARMv7-M PMSA as the pager uses it: the highest-numbered
//! enabled region whose enabled sub-region contains the address decides;
//! with no match the default map applies (the MPU runs with PRIVDEFENA and
//! everything is privileged). SRAM2/3 decode address bits 0–14 only. Any
//! byte of an access that is not permitted faults the whole access, and the
//! fault reports the *start* of the access (`MMFAR` for data, the PC for a
//! fetch), which is the worst case the straddle rule must handle.
//!
//! Besides the pager's own tables the simulator keeps the ground truth of
//! what each physical line holds (which page and line was last read into
//! it), so [`Machine::check`] can prove the invariants independently of the
//! core.

use crate::{
    Action, CODE_SLOTS, CODE_WIN, CODE_WIN_END, Fatal, LINE_SIZE, LINES_PER_PAGE, PAGE_SIZE, Page, PagerCore,
    RO_SLOTS, RegionMap, SRAM2_BASE, SRAM3_BASE, mpu,
};

const SLOTS: usize = (CODE_SLOTS + RO_SLOTS) as usize;
const LINES: usize = SLOTS * LINES_PER_PAGE as usize;
/// Retries of one access before the simulator calls it a livelock.
const MAX_FAULTS_PER_ACCESS: u32 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Region {
    rbar: u32,
    rasr: u32,
}

impl Region {
    fn enabled(self) -> bool {
        self.rasr & 1 != 0
    }
    fn size(self) -> u32 {
        1u32 << (((self.rasr >> 1) & 0x1F) + 1)
    }
    fn covers(self, addr: u32) -> bool {
        if !self.enabled() {
            return false;
        }
        let size = self.size();
        let base = self.rbar & !(size - 1);
        if addr < base || addr - base >= size {
            return false;
        }
        let sub = (addr - base) / (size / 8);
        (self.rasr >> 8) & (1 << sub) == 0
    }
    fn ap(self) -> u32 {
        (self.rasr >> 24) & 7
    }
    fn xn(self) -> bool {
        self.rasr & (1 << 28) != 0
    }
}

/// Kind of access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Instruction fetch (the reported fault address is the PC).
    Fetch,
    /// Data read (the reported fault address is the start of the access).
    Read,
}

/// Why an access did not complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimError {
    Fatal(Fatal),
    /// The access still faulted after several handled faults.
    Livelock,
    /// The MPU let the access through but the bytes differ from the page
    /// store (a pager bug).
    WrongData {
        addr: u32,
    },
    /// The access touched SRAM2/3 outside any page alias and outside RAM.
    Unmapped {
        addr: u32,
    },
}

/// Ground truth of one physical line: the page and line last read into it.
type LineTruth = Option<(Page, u32)>;

/// Simulated machine. Large (64 KiB of SRAM): keep it boxed or static.
pub struct Machine<'a> {
    pub core: PagerCore,
    store: &'a [u8],
    ncode: u32,
    regions: [Region; 8],
    mpu_on: bool,
    sram2: [u8; 0x8000],
    sram3: [u8; 0x8000],
    truth: [LineTruth; LINES],
    /// Faults taken and NOR line reads done.
    pub faults: u32,
    pub nor_reads: u32,
    /// Accesses completed.
    pub accesses: u64,
}

impl<'a> Machine<'a> {
    /// A machine whose page store is `store` (`ncode` code pages, then
    /// `nro` rodata pages, 8 KiB each), with the MPU in its initial state.
    #[must_use]
    pub fn new(store: &'a [u8], ncode: u32, nro: u32) -> Self {
        let mut core = PagerCore::new();
        core.init(ncode, nro);
        let mut m = Self {
            core,
            store,
            ncode,
            regions: [Region::default(); 8],
            mpu_on: false,
            // Garbage, as after power-up.
            sram2: [0xA5; 0x8000],
            sram3: [0x5A; 0x8000],
            truth: [None; LINES],
            faults: 0,
            nor_reads: 0,
            accesses: 0,
        };
        for (n, rbar, rasr) in mpu::INITIAL {
            m.regions[usize::from(n)] = Region { rbar, rasr };
        }
        m.mpu_on = true;
        m
    }

    fn permitted(&self, addr: u32, kind: Kind) -> bool {
        if !self.mpu_on {
            return true;
        }
        match self.regions.iter().rev().find(|r| r.covers(addr)) {
            None => true,
            Some(r) => r.ap() != 0 && !(kind == Kind::Fetch && r.xn()),
        }
    }

    fn phys_byte(&self, addr: u32) -> Option<u8> {
        match addr {
            0x0020_0000..0x0030_0000 => Some(self.sram2[(addr & 0x7FFF) as usize]),
            0x0030_0000..0x0040_0000 => Some(self.sram3[(addr & 0x7FFF) as usize]),
            _ => None,
        }
    }

    /// The byte the page store holds for virtual address `addr`.
    #[must_use]
    pub fn expected(&self, addr: u32) -> Option<u8> {
        let page = Page::of(addr)?;
        let off = page.store_index(self.ncode) * PAGE_SIZE + (addr - page.va());
        self.store.get(off as usize).copied()
    }

    fn apply_region(&mut self, m: RegionMap) {
        if let Some(r) = self.regions.get_mut(usize::from(m.region)) {
            *r = Region { rbar: m.rbar(), rasr: m.rasr() };
        }
    }

    fn apply(&mut self, a: Action) -> Result<(), SimError> {
        match a {
            Action::Fatal(f) => Err(SimError::Fatal(f)),
            Action::Remap(m) => {
                self.mpu_on = false;
                self.apply_region(m);
                self.mpu_on = true;
                Ok(())
            }
            Action::Load { map, nor_offset, phys, unmap } => {
                self.mpu_on = false;
                if let Some(u) = unmap {
                    self.apply_region(u);
                }
                self.nor_reads += 1;
                let src = nor_offset as usize..(nor_offset + LINE_SIZE) as usize;
                let dst = (phys & 0x7FFF) as usize..((phys & 0x7FFF) + LINE_SIZE) as usize;
                let ram = if phys >= SRAM3_BASE { &mut self.sram3 } else { &mut self.sram2 };
                match self.store.get(src) {
                    Some(s) => ram[dst].copy_from_slice(s),
                    None => ram[dst].fill(0xFF),
                }
                // Ground truth: which page/line went where, derived from the
                // NOR offset (not from the core's tables).
                let store_page = nor_offset / PAGE_SIZE;
                let page = if store_page < self.ncode {
                    Page::Code(store_page as u16)
                } else {
                    Page::Rodata((store_page - self.ncode) as u16)
                };
                let slot = if phys >= SRAM3_BASE {
                    CODE_SLOTS + (phys - SRAM3_BASE) / PAGE_SIZE
                } else {
                    (phys - SRAM2_BASE) / PAGE_SIZE
                };
                let line = (nor_offset % PAGE_SIZE) / LINE_SIZE;
                if let Some(t) = self.truth.get_mut((slot * LINES_PER_PAGE + (phys >> 10) % 8) as usize) {
                    *t = Some((page, line));
                }
                self.apply_region(map);
                self.mpu_on = true;
                Ok(())
            }
        }
    }

    /// Perform an access of `len` bytes (1–8) at `addr` by the instruction
    /// at `pc`, taking and handling faults like the CPU and the MemManage
    /// handler would. Returns the bytes read (little-endian in a `u64`)
    /// after checking them against the page store.
    pub fn access(&mut self, addr: u32, len: u32, kind: Kind, pc: u32) -> Result<u64, SimError> {
        let len = len.clamp(1, 8);
        for _ in 0..=MAX_FAULTS_PER_ACCESS {
            let ok = (0..len).all(|i| self.permitted(addr + i, kind));
            if ok {
                let mut v = 0u64;
                for i in 0..len {
                    let a = addr + i;
                    let b = self.phys_byte(a).ok_or(SimError::Unmapped { addr: a })?;
                    if Page::of(a).is_some() {
                        if self.expected(a) != Some(b) {
                            return Err(SimError::WrongData { addr: a });
                        }
                    } else if (CODE_WIN..CODE_WIN_END).contains(&a) {
                        return Err(SimError::Unmapped { addr: a });
                    }
                    v |= u64::from(b) << (8 * i);
                }
                self.accesses += 1;
                return Ok(v);
            }
            self.faults += 1;
            let reported = if kind == Kind::Fetch { pc } else { addr };
            let a = self.core.fault(reported, pc);
            self.apply(a)?;
        }
        Err(SimError::Livelock)
    }

    /// Fetch a Thumb instruction of `len` (2 or 4) bytes at `pc`.
    pub fn fetch(&mut self, pc: u32, len: u32) -> Result<u64, SimError> {
        self.access(pc, len, Kind::Fetch, pc)
    }

    /// Check the invariants: every mapped line holds the data of the page
    /// its region maps (by ground truth), no physical line is mapped twice,
    /// and the pager's tables agree with the MPU.
    pub fn check(&self) -> Result<(), &'static str> {
        let mut mapped = [false; LINES];
        for (i, r) in self.regions.iter().enumerate().skip(1).take(crate::POOL_REGIONS) {
            if !r.enabled() {
                continue;
            }
            if r.size() != PAGE_SIZE {
                return Err("pool region is not one page");
            }
            let page = Page::of(r.rbar).ok_or("pool region outside the windows")?;
            if page.va() != r.rbar {
                return Err("pool region base is not a page VA");
            }
            match self.core.region(i - 1) {
                Some((p, _)) if p == page => {}
                _ => return Err("MPU region and pager table disagree"),
            }
            if r.xn() != matches!(page, Page::Rodata(_)) {
                return Err("XN does not match the page kind");
            }
            let slot = page.slot();
            for line in 0..LINES_PER_PAGE {
                if (r.rasr >> 8) & (1 << line) != 0 {
                    continue;
                }
                let idx = (slot * LINES_PER_PAGE + line) as usize;
                if mapped[idx] {
                    return Err("one physical line mapped by two regions");
                }
                mapped[idx] = true;
                if self.truth[idx] != Some((page, line)) {
                    return Err("mapped line does not hold that page's data");
                }
                if self.core.line_owner(slot as usize, line as usize) != Some(page) {
                    return Err("pager line table disagrees with the data");
                }
            }
        }
        Ok(())
    }
}
