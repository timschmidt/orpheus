//! SPI cache controllers, `0xC0050000` ("SPI Cache") and `0xC0060000` ("SPI1
//! Cache"). **Undocumented**: the datasheet memory map (§3.4) and cache
//! feature list (§3.3: 32 KiB 4-way I-cache for SPI NOR, 32 KiB 4-way
//! D-cache for SPI SRAM) name them, but there are no register descriptions,
//! and the stock firmware does not use them. Their clock/reset bits are
//! documented ([`crate::cmu::clken0::SPI0_CACHE`], `SPI1_CACHE`) and gate the
//! RAMs used as SRAM2/SRAM3. The SPI0 chapter's `CACHE_ERROR_ADDR` /
//! `CACHE_MODE` registers live in the SPI0 block (see [`crate::spi`]).
//!
//! Addresses only, for the investigation U-XIP-1 (read-only dumps first).

/// SPI0 (NOR) cache controller block. \[DS\] §3.4 (address only).
pub const SPI0_CACHE_BASE: usize = 0xC005_0000;
/// SPI1 cache controller block. \[DS\] §3.4 (address only).
pub const SPI1_CACHE_BASE: usize = 0xC006_0000;
/// Size of each block's address window in the memory map.
pub const BLOCK_SIZE: usize = 0x1_0000;
/// Size of each cache RAM (4 ways × 8 KiB). \[DS\] §3.3
pub const CACHE_SIZE: usize = 32 * 1024;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bases() {
        assert_eq!(SPI0_CACHE_BASE + BLOCK_SIZE, SPI1_CACHE_BASE);
        assert_eq!(SPI1_CACHE_BASE + BLOCK_SIZE, crate::dma::BASE);
        assert_eq!(CACHE_SIZE, 0x8000);
    }
}
