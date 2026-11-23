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
}
