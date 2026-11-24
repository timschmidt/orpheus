//! Demand-pager policy for the ATJ2157 (docs/plan/02-architecture.md §3).
//!
//! The SoC has 320 KiB of RAM and a SPI NOR that is not memory-mapped. Code
//! and constants that do not fit the resident image live in a NOR page store
//! and are fetched on demand, one 1 KiB *line* per MemManage fault:
//!
//! * SRAM2 and SRAM3 (32 KiB each) ignore address bits above bit 14, so each
//!   appears 32 times in its 1 MiB window. `xtask pagelink` gives every 8 KiB
//!   page of paged code or rodata its own alias ([`code_page_va`],
//!   [`ro_page_va`]); the alias decides the physical 8 KiB *slot*.
//! * MPU region 0 denies both windows (except `0x300000–0x33FFFF`, where
//!   SRAM3 is plain RAM). Pool regions 1–6 each map one page with only its
//!   loaded lines enabled (sub-region disable bits). Region 7 is the stack
//!   guard and not the pager's.
//! * A physical line keeps the data of the last page loaded into it even
//!   when that page's region is reused, so a page coming back gets its intact
//!   lines without a NOR read.
//!
//! This crate is the policy only: [`PagerCore`] turns a fault address into
//! an [`Action`] that the resident MemManage handler carries out (MPU writes,
//! one NOR read). It is allocation-free, panic-free by construction (all
//! table indices are masked or checked), has no recursion and no
//! formatting, so it can run inside the fault handler; the same code runs on
//! the host against a simulated MPU ([`sim`], feature `sim`).
//!
//! The algorithm follows the pager proven on this hardware by the Rockbox
//! port (`pager-atj2157.c`): region allocation by least-recent fault, never
//! evicting the region that holds the faulting PC, line reuse across region
//! reuse, and the *straddle* rule (an access that crosses into the next line
//! reports its first address; when that line is already mapped the fault
//! belongs to the next line). Deliberate differences, each covered by tests:
//!
//! * six pool regions instead of seven (region 7 is the stack guard);
//! * a straddle out of the last line of a page continues into line 0 of the
//!   next page when that page is VA-contiguous (code: always; rodata: the
//!   second page of a 16 KiB group), instead of being fatal: pagelink lets
//!   code sections cross page boundaries;
//! * rodata addresses in the upper 16 KiB of an alias block (physically the
//!   SRAM3 RAM half) are fatal instead of being mapped onto the wrong slot;
//! * a load that would take the physical line holding the faulting
//!   instruction is fatal ([`Fatal::LineConflict`]) instead of livelocking;
//! * pool regions are read-only Normal memory (writes into a paged area
//!   fault; unaligned reads are architecturally defined), see [`mpu`].

#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "sim")]
pub mod sim;

/// Page size: one MPU pool region.
pub const PAGE_SIZE: u32 = 0x2000;
/// Line size: one MPU sub-region, the unit of a NOR read.
pub const LINE_SIZE: u32 = 0x400;
/// Lines per page.
pub const LINES_PER_PAGE: u32 = 8;

/// Code window (SRAM2 aliases). Code page `p` is at `CODE_WIN + p * 8 KiB`.
pub const CODE_WIN: u32 = 0x0020_0000;
pub const CODE_WIN_END: u32 = 0x0030_0000;
/// Rodata window (SRAM3 aliases above the plain-RAM first 256 KiB).
pub const RO_WIN: u32 = 0x0034_0000;
pub const RO_WIN_END: u32 = 0x0040_0000;
/// Physical slots: SRAM2 holds four code slots, SRAM3's low half two rodata
/// slots.
pub const CODE_SLOTS: u32 = 4;
pub const RO_SLOTS: u32 = 2;
/// Base of the physical slots in the CPU address space (the first alias).
pub const SRAM2_BASE: u32 = 0x0020_0000;
pub const SRAM3_BASE: u32 = 0x0030_0000;
/// Largest page counts the windows can address.
pub const MAX_CODE_PAGES: u32 = 128;
pub const MAX_RO_PAGES: u32 = 48;
/// A rodata group: two VA-contiguous pages at the start of a 32 KiB alias
/// block. Code pages are VA-contiguous over the whole code window.
pub const RO_GROUP_SIZE: u32 = 0x4000;
pub const ALIAS_BLOCK: u32 = 0x8000;

/// MPU pool regions used by the pager (`FIRST_POOL_REGION ..`).
pub const POOL_REGIONS: usize = 6;
pub const FIRST_POOL_REGION: u8 = 1;
/// Physical slots tracked (four code, two rodata).
const SLOTS: usize = (CODE_SLOTS + RO_SLOTS) as usize;
const LINES: usize = SLOTS * LINES_PER_PAGE as usize;

/// Virtual address of code page `p` (`p < MAX_CODE_PAGES`).
#[must_use]
pub const fn code_page_va(p: u32) -> u32 {
    CODE_WIN + (p >> 2) * ALIAS_BLOCK + (p & 3) * PAGE_SIZE
}

/// Virtual address of rodata page `q` (`q < MAX_RO_PAGES`).
#[must_use]
pub const fn ro_page_va(q: u32) -> u32 {
    RO_WIN + (q >> 1) * ALIAS_BLOCK + (q & 1) * PAGE_SIZE
}

/// A page as the pager names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Page {
    Code(u16),
    Rodata(u16),
}

impl Page {
    /// The page containing `addr`, if `addr` lies in a page alias (whether
    /// or not the page exists in the current store).
    #[must_use]
    pub const fn of(addr: u32) -> Option<Self> {
        if addr >= CODE_WIN && addr < CODE_WIN_END {
            Some(Self::Code(((addr - CODE_WIN) / PAGE_SIZE) as u16))
        } else if addr >= RO_WIN && addr < RO_WIN_END {
            let off = addr - RO_WIN;
            if off & RO_GROUP_SIZE != 0 {
                // Upper half of an alias block: SRAM3's RAM half, not a page.
                return None;
            }
            Some(Self::Rodata((((off >> 15) << 1) | ((off >> 13) & 1)) as u16))
        } else {
            None
        }
    }

    /// Virtual address of the page.
    #[must_use]
    pub const fn va(self) -> u32 {
        match self {
            Self::Code(p) => code_page_va(p as u32),
            Self::Rodata(q) => ro_page_va(q as u32),
        }
    }

    /// Index of the page in the page store, given the number of code pages
    /// (code pages first, then rodata pages).
    #[must_use]
    pub const fn store_index(self, ncode: u32) -> u32 {
        match self {
            Self::Code(p) => p as u32,
            Self::Rodata(q) => ncode + q as u32,
        }
    }

    /// Physical slot index: 0–3 code (SRAM2), 4–5 rodata (SRAM3).
    #[must_use]
    pub const fn slot(self) -> u32 {
        match self {
            Self::Code(p) => (p as u32) & 3,
            Self::Rodata(q) => CODE_SLOTS + ((q as u32) & 1),
        }
    }

    /// CPU address of the start of the page's physical slot.
    #[must_use]
    pub const fn phys(self) -> u32 {
        match self {
            Self::Code(p) => SRAM2_BASE + ((p as u32) & 3) * PAGE_SIZE,
            Self::Rodata(q) => SRAM3_BASE + ((q as u32) & 1) * PAGE_SIZE,
        }
    }

    const fn id(self) -> u16 {
        match self {
            Self::Code(p) => p,
            Self::Rodata(q) => RO_TAG | q,
        }
    }

    const fn from_id(id: u16) -> Self {
        if id & RO_TAG != 0 { Self::Rodata(id & !RO_TAG) } else { Self::Code(id) }
    }
}

/// MPU register words (ARMv7-M PMSA).
pub mod mpu {
    /// `MPU_CTRL`: enable, default map for privileged accesses.
    pub const CTRL_ON: u32 = 0b101;
    pub const CTRL_OFF: u32 = 0;
    /// `CFSR.MMARVALID`: `MMFAR` holds the data address of the fault.
    pub const CFSR_MMARVALID: u32 = 1 << 7;
    /// Access permission: no access.
    pub const AP_NONE: u32 = 0b000;
    /// Access permission: read-only, privileged and unprivileged.
    pub const AP_RO: u32 = 0b110;
    /// Memory attributes of the pool regions: Normal, write-through
    /// (TEX=000, C=1, B=0), the same type the default memory map gives this
    /// address range. The Rockbox port used TEX/C/B = 0 (strongly ordered),
    /// where unaligned accesses are not architecturally defined.
    pub const POOL_ATTR: u32 = 1 << 17;

    /// Region 0: deny the code and rodata windows (2 MiB at `0x200000`),
    /// sub-region 4 (`0x300000–0x33FFFF`, SRAM3 as RAM) disabled so it falls
    /// through to the default map.
    pub const DENY_REGION: u8 = 0;
    pub const DENY_RBAR: u32 = super::CODE_WIN;
    pub const DENY_RASR: u32 = rasr(21, AP_NONE, 0, true, 0x10);

    /// A `RASR` word: region of `2^size_log2` bytes.
    #[must_use]
    pub const fn rasr(size_log2: u32, ap: u32, attr: u32, xn: bool, srd: u8) -> u32 {
        ((xn as u32) << 28) | (ap << 24) | attr | ((srd as u32) << 8) | ((size_log2 - 1) << 1) | 1
    }

    /// `RASR` of a pool region mapping one page with the given sub-region
    /// disable mask (bit set = line not mapped).
    #[must_use]
    pub const fn pool_rasr(xn: bool, srd: u8) -> u32 {
        rasr(13, AP_RO, POOL_ATTR, xn, srd)
    }

    /// The region set-up before the first fault: `(region, RBAR, RASR)` for
    /// regions 0–6 (pool regions disabled). Write with the MPU off, then
    /// `CTRL_ON`, and enable MemManage (`SHCSR.MEMFAULTENA`).
    pub const INITIAL: [(u8, u32, u32); 1 + super::POOL_REGIONS] =
        [(0, DENY_RBAR, DENY_RASR), (1, 0, 0), (2, 0, 0), (3, 0, 0), (4, 0, 0), (5, 0, 0), (6, 0, 0)];

    /// The address a MemManage fault is about: `MMFAR` for data accesses,
    /// the stacked PC for instruction fetches.
    #[must_use]
    pub const fn fault_address(cfsr: u32, mmfar: u32, stacked_pc: u32) -> u32 {
        if cfsr & CFSR_MMARVALID != 0 { mmfar } else { stacked_pc }
    }
}

/// One MPU pool-region write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionMap {
    /// MPU region number (`FIRST_POOL_REGION ..`).
    pub region: u8,
    /// Region base (`RBAR` without the VALID bit): the page's VA.
    pub base: u32,
    /// Sub-region disable mask: bit `k` set = line `k` not mapped.
    pub srd: u8,
    /// Execute-never (rodata pages).
    pub xn: bool,
}

impl RegionMap {
    #[must_use]
    pub const fn rbar(&self) -> u32 {
        self.base
    }

    #[must_use]
    pub const fn rasr(&self) -> u32 {
        mpu::pool_rasr(self.xn, self.srd)
    }
}

/// Why a fault is not a paging fault (the handler records it and goes to
/// the crash path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fatal {
    /// `init` has not run or the store has no pages.
    Uninitialised,
    /// The address is not in a page alias.
    NotPaged,
    /// The address is in a page alias beyond the page store.
    NoSuchPage,
    /// The line (and, for a straddling access, the next line) is mapped:
    /// the fault is a permission fault (write, execute-never) or a bug.
    NotAPagingFault,
    /// The line to load physically holds the instruction that faulted; the
    /// access could never complete.
    LineConflict,
}

/// What the MemManage handler must do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// The data is in place: write `map` (MPU off around the write) and
    /// return.
    Remap(RegionMap),
    /// Disable the MPU; write `unmap` if present (the line is taken from
    /// that region's page); read [`LINE_SIZE`] bytes at `nor_offset` from the
    /// start of the page store into `phys` (a CPU address inside the denied
    /// window, hence MPU off; DMA uses the SRAM2/3 DMA alias); write `map`;
    /// enable the MPU; return. If the read fails, call
    /// [`PagerCore::abort_load`] before taking the crash path.
    Load {
        map: RegionMap,
        nor_offset: u32,
        phys: u32,
        unmap: Option<RegionMap>,
    },
    Fatal(Fatal),
}

/// Fault statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Faults whose address lay in the code / rodata window.
    pub code_faults: u32,
    pub ro_faults: u32,
    /// NOR line reads requested.
    pub loads: u32,
    /// Faults served by re-mapping data already in place.
    pub remaps: u32,
    /// Faults resolved as straddling accesses (moved to the next line).
    pub straddles: u32,
    /// Pool regions taken from another page.
    pub evictions: u32,
    pub fatal: u32,
}

const NO_PAGE: u16 = 0xFFFF;
const RO_TAG: u16 = 0x100;

/// The pager's tables: pool regions, physical line owners, counters.
/// About 200 bytes; lives in resident RAM.
#[derive(Clone, Debug)]
pub struct PagerCore {
    ncode: u16,
    nro: u16,
    /// Per pool region: page id (`NO_PAGE` = free), VA, LRU stamp, mapped
    /// lines, physical slot.
    reg_page: [u16; POOL_REGIONS],
    reg_va: [u32; POOL_REGIONS],
    reg_lru: [u32; POOL_REGIONS],
    reg_mask: [u8; POOL_REGIONS],
    reg_slot: [u8; POOL_REGIONS],
    /// Per physical line (slot * 8 + line): the page whose data it holds.
    line_page: [u16; LINES],
    clock: u32,
    stats: Stats,
}

impl Default for PagerCore {
    fn default() -> Self {
        Self::new()
    }
}

/// The page a fault resolves to.
#[derive(Clone, Copy)]
struct Target {
    id: u16,
    va: u32,
    slot: u8,
    xn: bool,
    store: u32,
    phys: u32,
    /// The next page is VA-contiguous and exists (straddle target).
    has_next: bool,
}

impl PagerCore {
    /// Empty tables; every fault is fatal until [`init`](Self::init).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            ncode: 0,
            nro: 0,
            reg_page: [NO_PAGE; POOL_REGIONS],
            reg_va: [0; POOL_REGIONS],
            reg_lru: [0; POOL_REGIONS],
            reg_mask: [0; POOL_REGIONS],
            reg_slot: [0; POOL_REGIONS],
            line_page: [NO_PAGE; LINES],
            clock: 0,
            stats: Stats {
                code_faults: 0,
                ro_faults: 0,
                loads: 0,
                remaps: 0,
                straddles: 0,
                evictions: 0,
                fatal: 0,
            },
        }
    }

    /// Reset the tables for a page store of `ncode` code pages followed by
    /// `nro` rodata pages (counts beyond the window limits are clamped).
    /// The MPU must be in its [`mpu::INITIAL`] state.
    pub fn init(&mut self, ncode: u32, nro: u32) {
        *self = Self::new();
        self.ncode = ncode.min(MAX_CODE_PAGES) as u16;
        self.nro = nro.min(MAX_RO_PAGES) as u16;
    }

    #[must_use]
    pub const fn stats(&self) -> Stats {
        self.stats
    }

    #[must_use]
    pub const fn pages(&self) -> (u32, u32) {
        (self.ncode as u32, self.nro as u32)
    }

    /// The page and mapped-line mask of pool region `i` (`0..POOL_REGIONS`,
    /// MPU region `FIRST_POOL_REGION + i`).
    #[must_use]
    pub fn region(&self, i: usize) -> Option<(Page, u8)> {
        let id = *self.reg_page.get(i)?;
        if id == NO_PAGE {
            None
        } else {
            Some((Page::from_id(id), self.reg_mask.get(i).copied().unwrap_or(0)))
        }
    }

    /// The page whose data physical line `slot * 8 + line` holds.
    #[must_use]
    pub fn line_owner(&self, slot: usize, line: usize) -> Option<Page> {
        let id = *self.line_page.get(slot * LINES_PER_PAGE as usize + line)?;
        if id == NO_PAGE { None } else { Some(Page::from_id(id)) }
    }

    fn target(&self, addr: u32) -> Result<Target, Fatal> {
        if self.ncode == 0 && self.nro == 0 {
            return Err(Fatal::Uninitialised);
        }
        let page = Page::of(addr).ok_or(Fatal::NotPaged)?;
        let (exists, has_next) = match page {
            Page::Code(p) => (p < self.ncode, p + 1 < self.ncode),
            Page::Rodata(q) => (q < self.nro, q & 1 == 0 && q + 1 < self.nro),
        };
        if !exists {
            return Err(Fatal::NoSuchPage);
        }
        Ok(Target {
            id: page.id(),
            va: page.va(),
            slot: page.slot() as u8,
            xn: matches!(page, Page::Rodata(_)),
            store: page.store_index(u32::from(self.ncode)),
            phys: page.phys(),
            has_next,
        })
    }

    fn map(&self, r: usize) -> RegionMap {
        let r = r % POOL_REGIONS;
        RegionMap {
            region: FIRST_POOL_REGION + r as u8,
            base: self.reg_va[r],
            srd: !self.reg_mask[r],
            xn: self.reg_page[r] & RO_TAG != 0 && self.reg_page[r] != NO_PAGE,
        }
    }

    fn touch(&mut self, r: usize) {
        if self.clock == u32::MAX {
            // Renumber by rank so the order survives the wrap.
            let old = self.reg_lru;
            for (stamp, mine) in self.reg_lru.iter_mut().zip(old) {
                *stamp = old.iter().filter(|&&o| o < mine).count() as u32 + 1;
            }
            self.clock = POOL_REGIONS as u32 + 1;
        }
        self.clock += 1;
        self.reg_lru[r % POOL_REGIONS] = self.clock;
    }

    /// Find the region mapping `t`, or take one: a free region, else the
    /// least recently faulted one that does not map the faulting code.
    fn region_for(&mut self, t: &Target, pc: u32) -> (usize, bool) {
        if let Some(r) = self.reg_page.iter().position(|&p| p == t.id) {
            return (r, false);
        }
        let pc_page = match Page::of(pc) {
            Some(p @ Page::Code(_)) => p.id(),
            _ => NO_PAGE,
        };
        let r = if let Some(free) = self.reg_page.iter().position(|&p| p == NO_PAGE) {
            free
        } else {
            let mut best = u32::MAX;
            let mut r = 0;
            for i in 0..POOL_REGIONS {
                if self.reg_page[i] != pc_page && self.reg_lru[i] < best {
                    best = self.reg_lru[i];
                    r = i;
                }
            }
            self.stats.evictions += 1;
            r
        };
        self.reg_page[r] = t.id;
        self.reg_va[r] = t.va;
        self.reg_slot[r] = t.slot;
        // Lines still holding this page's data come back as they are.
        let mut mask = 0u8;
        for k in 0..LINES_PER_PAGE as usize {
            if self.line_page[(usize::from(t.slot) * 8 + k) % LINES] == t.id {
                mask |= 1 << k;
            }
        }
        self.reg_mask[r] = mask;
        (r, true)
    }

    fn fatal(&mut self, f: Fatal) -> Action {
        self.stats.fatal += 1;
        Action::Fatal(f)
    }

    /// Resolve a MemManage fault at `addr` ([`mpu::fault_address`]) taken
    /// with the stacked PC `pc`.
    pub fn fault(&mut self, addr: u32, pc: u32) -> Action {
        let mut addr = addr;
        // Set once a straddle moved on to the next page.
        let mut hopped = false;
        loop {
            let t = match self.target(addr) {
                Ok(t) => t,
                Err(f) => return self.fatal(f),
            };
            if !hopped {
                if t.xn {
                    self.stats.ro_faults += 1;
                } else {
                    self.stats.code_faults += 1;
                }
            }
            let mut sub = (addr >> 10) & 7;
            let (r, fresh) = self.region_for(&t, pc);
            if self.reg_mask[r] & (1 << sub) != 0 {
                if !fresh {
                    // The line is mapped, so the access straddles into the
                    // next line and reported its start.
                    if hopped {
                        return self.fatal(Fatal::NotAPagingFault);
                    }
                    if sub == 7 {
                        // Into line 0 of the next page, if VA-contiguous.
                        if !t.has_next {
                            return self.fatal(Fatal::NotAPagingFault);
                        }
                        self.stats.straddles += 1;
                        addr = t.va + PAGE_SIZE;
                        hopped = true;
                        continue;
                    }
                    if self.reg_mask[r] & (2 << sub) != 0 {
                        return self.fatal(Fatal::NotAPagingFault);
                    }
                    self.stats.straddles += 1;
                    sub += 1;
                    if self.line_page[(usize::from(t.slot) * 8 + sub as usize) % LINES] == t.id {
                        self.reg_mask[r] |= 1 << sub;
                    }
                }
                if self.reg_mask[r] & (1 << sub) != 0 {
                    self.stats.remaps += 1;
                    self.touch(r);
                    return Action::Remap(self.map(r));
                }
            }
            return self.load(r, &t, sub, pc);
        }
    }

    fn load(&mut self, r: usize, t: &Target, sub: u32, pc: u32) -> Action {
        let line = (usize::from(t.slot) * 8 + sub as usize) % LINES;
        let prev = self.line_page[line];
        if prev != NO_PAGE
            && let Some(pc_page) = Page::of(pc)
            && pc_page.id() == prev
            && (pc_page.slot() * 8 + ((pc >> 10) & 7)) as usize == line
        {
            return self.fatal(Fatal::LineConflict);
        }
        // Take the line from the page that has it; unmap it there.
        let mut unmap = None;
        if prev != NO_PAGE {
            for o in 0..POOL_REGIONS {
                if o != r && self.reg_page[o] == prev && self.reg_mask[o] & (1 << sub) != 0 {
                    self.reg_mask[o] &= !(1 << sub);
                    unmap = Some(self.map(o));
                }
            }
        }
        self.line_page[line] = t.id;
        self.reg_mask[r] |= 1 << sub;
        self.touch(r);
        self.stats.loads += 1;
        Action::Load {
            map: self.map(r),
            nor_offset: t.store * PAGE_SIZE + sub * LINE_SIZE,
            phys: t.phys + sub * LINE_SIZE,
            unmap,
        }
    }

    /// Undo the table update of a [`Action::Load`] whose NOR read failed:
    /// the line holds no page's data and is not mapped. Write the returned
    /// region (if any) to the MPU.
    pub fn abort_load(&mut self, phys: u32) -> Option<RegionMap> {
        let slot = match phys {
            a if (SRAM2_BASE..SRAM2_BASE + CODE_SLOTS * PAGE_SIZE).contains(&a) => {
                (a - SRAM2_BASE) / PAGE_SIZE
            }
            a if (SRAM3_BASE..SRAM3_BASE + RO_SLOTS * PAGE_SIZE).contains(&a) => {
                CODE_SLOTS + (a - SRAM3_BASE) / PAGE_SIZE
            }
            _ => return None,
        };
        let sub = (phys >> 10) & 7;
        let line = (slot * 8 + sub) as usize % LINES;
        let id = self.line_page[line];
        self.line_page[line] = NO_PAGE;
        let r = self.reg_page.iter().position(|&p| p == id && id != NO_PAGE)?;
        self.reg_mask[r] &= !(1 << sub);
        Some(self.map(r))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn va_formulas() {
        // Code pages are VA-contiguous over the whole window.
        for p in 0..MAX_CODE_PAGES {
            assert_eq!(code_page_va(p), CODE_WIN + p * PAGE_SIZE);
            assert_eq!(Page::of(code_page_va(p) + 0x1FFF), Some(Page::Code(p as u16)));
            assert_eq!(Page::Code(p as u16).slot(), p & 3);
        }
        assert_eq!(code_page_va(MAX_CODE_PAGES), CODE_WIN_END);
        for q in 0..MAX_RO_PAGES {
            let va = ro_page_va(q);
            assert_eq!(Page::of(va), Some(Page::Rodata(q as u16)));
            assert_eq!((va >> 13) & 3, q & 1, "slot follows the alias bits");
        }
        assert_eq!(ro_page_va(MAX_RO_PAGES - 1) + PAGE_SIZE, RO_WIN_END - RO_GROUP_SIZE);
        assert_eq!(Page::of(RO_WIN + RO_GROUP_SIZE), None, "SRAM3 RAM half");
        assert_eq!(Page::of(0x30_0000), None);
        assert_eq!(Page::of(0x1F_FFFF), None);
    }

    #[test]
    fn rasr_words() {
        // Deny region: 2 MiB, no access, XN, sub-region 4 disabled.
        assert_eq!(mpu::DENY_RASR, 0x1000_1029);
        let m = RegionMap { region: 1, base: 0x20_2000, srd: 0xFE, xn: false };
        assert_eq!(m.rasr(), 0x0602_FE19);
        assert_eq!(mpu::fault_address(0x82, 0x34_0010, 0x20_0000), 0x34_0010);
        assert_eq!(mpu::fault_address(0x01, 0x34_0010, 0x20_0000), 0x20_0000);
    }

    #[test]
    fn lru_wrap_keeps_order() {
        let mut c = PagerCore::new();
        c.init(32, 0);
        for p in 0..POOL_REGIONS as u32 {
            assert!(matches!(c.fault(code_page_va(p), 0), Action::Load { .. }));
        }
        c.clock = u32::MAX - 2;
        // Touch regions 3, 4, 5 again, crossing the wrap.
        for p in 3..6 {
            assert!(matches!(c.fault(code_page_va(p) + LINE_SIZE, 0), Action::Load { .. }));
        }
        // The next eviction takes page 0's region (oldest), then page 1's.
        let Action::Load { map, .. } = c.fault(code_page_va(20), 0) else { panic!() };
        assert_eq!(map.region, FIRST_POOL_REGION);
        let Action::Load { map, .. } = c.fault(code_page_va(21), 0) else { panic!() };
        assert_eq!(map.region, FIRST_POOL_REGION + 1);
    }
}
