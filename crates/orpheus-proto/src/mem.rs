//! Physical memory map (docs/plan/02-architecture.md §1).

/// Mask ROM.
pub const ROM_BASE: u32 = 0x0000_0000;
pub const ROM_SIZE: u32 = 64 * 1024;

/// SRAM0: 224 KiB. The only block the USB DMA is known to reach.
pub const SRAM0_BASE: u32 = 0x0010_0000;
pub const SRAM0_SIZE: u32 = 224 * 1024;
pub const SRAM0_END: u32 = SRAM0_BASE + SRAM0_SIZE;

/// SRAM1: 32 KiB, holds the frame buffer.
pub const SRAM1_BASE: u32 = 0x0015_8000;
pub const SRAM1_SIZE: u32 = 32 * 1024;

/// SRAM2: 32 KiB (aliased every 32 KiB over 1 MiB). Code page cache.
pub const SRAM2_BASE: u32 = 0x0020_0000;
/// SRAM3: 32 KiB (aliased likewise). Rodata cache, PCM ring, log ring.
pub const SRAM3_BASE: u32 = 0x0030_0000;
pub const SRAM23_SIZE: u32 = 32 * 1024;
/// Peripheral-DMA view of SRAM2 / SRAM3.
pub const SRAM2_DMA: u32 = 0x1000_0000;
pub const SRAM3_DMA: u32 = 0x1800_0000;

/// SRAM3 above the rodata page cache: the PCM ring (12 KiB), the
/// profiler block (2 KiB, [`SRAM3_PROF`]), then a window of plain RAM for
/// state shared between tasks, ending at the card I/O counter in the last
/// word (02 §1 memory map).
pub const SRAM3_PCM_RING: u32 = 0x0030_4000;
pub const SRAM3_PCM_RING_SIZE: u32 = 12 * 1024;
/// The profiler's ring ([`crate::prof`], WP-T-8): the 2 KiB after the PCM
/// ring. Reserved for the persistent log ring at first
/// ([`crate::logring::BASE`]), which runs in SRAM0 instead
/// ([`crate::logring::ALT_BASE`]); nothing else ever used it (2026-11-25).
pub const SRAM3_PROF: u32 = SRAM3_PCM_RING + SRAM3_PCM_RING_SIZE;
pub const SRAM3_PROF_SIZE: u32 = 2048;
/// The shared-state window: the battery logger's state first, then the
/// step counter's, the radio's and the recorder's, each placed after the
/// one before with [`sram3_next`] (FM-1 fix, 2026-11-24: fixed addresses
/// chosen apart had put the step counter's state over the radio's).
pub const SRAM3_SHARED: u32 = 0x0030_7800;
/// The card I/O counter (`yoton_bsp::card::IO_ADDR`): the last word of
/// SRAM3, where the shared window ends.
pub const SRAM3_CARD_IO: u32 = SRAM3_BASE + SRAM23_SIZE - 4;

/// Where the next block of the shared window starts after one that ends at
/// `end` (8-byte aligned).
#[inline(always)]
#[must_use]
pub const fn sram3_next(end: usize) -> usize {
    (end + 7) & !7
}

/// Where a firmware or bring-up image is loaded and entered (Thumb: +1).
pub const IMAGE_BASE: u32 = SRAM0_BASE;
/// The `ymon` RAM monitor occupies `0x118000..0x11C000` while the host loads
/// an image; a RAM-loaded image must end below it.
pub const YMON_BASE: u32 = 0x0011_8000;
pub const YMON_END: u32 = 0x0011_C000;
/// ymon's payload area and argument block.
pub const YMON_PAYLOAD: u32 = 0x0011_E000;
pub const YMON_ARGS: u32 = 0x0011_F800;
/// Largest image `xtask dev ram` can load (everything below ymon).
pub const RAM_IMAGE_MAX: u32 = YMON_BASE - IMAGE_BASE;

/// Frame buffer: 128 × 128 RGB565, row-major, little-endian pixels.
pub const FB_BASE: u32 = SRAM1_BASE;
pub const FB_W: u32 = 128;
pub const FB_H: u32 = 128;
pub const FB_SIZE: u32 = FB_W * FB_H * 2;

/// Persistent area at the top of SRAM0 (survives resets; the boot ROM and
/// boot stages never touch it). See [`crate::mailbox`], [`crate::result`],
/// [`crate::fault`].
pub const PERSIST_BASE: u32 = 0x0013_7C00;
pub const PERSIST_SIZE: u32 = 0x400;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_do_not_overlap() {
        assert_eq!(PERSIST_BASE + PERSIST_SIZE, SRAM0_END);
        const { assert!(YMON_BASE > IMAGE_BASE) };
        assert_eq!(FB_SIZE, SRAM1_SIZE);
        assert_eq!(RAM_IMAGE_MAX, 96 * 1024);
    }

    /// Regression: the radio's RAM was overwritten by the step counter's
    /// (FM-1, 2026-11-24, first device run: `fm on` gave `khz=0 chip=0000`,
    /// the scan found nothing). Both had fixed SRAM3 addresses, `0x307A00`
    /// (1,144 B) and `0x307C00`. The map's regions are disjoint, and blocks
    /// chained with [`sram3_next`] never overlap; the firmware asserts at
    /// build time that its blocks (the sizes of its own types) end below the
    /// card I/O counter.
    #[test]
    fn regress_sram3_blocks_chained_not_overlapping() {
        let map = [
            (SRAM3_PCM_RING, SRAM3_PCM_RING + SRAM3_PCM_RING_SIZE),
            (SRAM3_PROF, SRAM3_PROF + SRAM3_PROF_SIZE),
            (SRAM3_SHARED, SRAM3_CARD_IO),
            (SRAM3_CARD_IO, SRAM3_CARD_IO + 4),
        ];
        for w in map.windows(2) {
            assert!(w[0].1 <= w[1].0, "{:#x?} overlaps {:#x?}", w[0], w[1]);
        }
        assert!(map[0].0 >= SRAM3_BASE && map[3].1 == SRAM3_BASE + SRAM23_SIZE);
        // The blocks as measured with the fix: logger 252 B, step counter
        // 1,144 B, radio and its text 520 B, then an odd-sized one.
        let mut at = SRAM3_SHARED as usize;
        let mut blocks = std::vec::Vec::new();
        for size in [252, 1144, 520, 61] {
            assert_eq!(at % 8, 0);
            blocks.push((at, at + size));
            at = sram3_next(at + size);
        }
        for w in blocks.windows(2) {
            assert!(w[0].1 <= w[1].0);
        }
        assert_eq!(blocks[1].0, 0x0030_7900);
        assert!(blocks[3].1 <= SRAM3_CARD_IO as usize);
        // The placement before the fix fails the same check.
        let old = [(0x0030_7A00usize, 0x0030_7A00 + 1144), (0x0030_7C00, 0x0030_7C00 + 848)];
        assert!(old[0].1 > old[1].0);
    }
}
