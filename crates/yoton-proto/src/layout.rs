//! NOR flash layout (docs/plan/02-architecture.md §2.4) and the guards every
//! writer checks.

/// NOR geometry.
pub const NOR_SIZE: u32 = 8 * 1024 * 1024;
pub const SECTOR: u32 = 4096;
pub const PAGE: u32 = 256;

/// Vendor data ends here (the region below is never written in the
/// dual-boot layout).
pub const VENDOR_END: u32 = 0x0076_0C00;

/// Dual-boot layout.
pub mod dual {
    /// First byte yotonrs may erase or program.
    pub const FREE_START: u32 = 0x0076_2000;
    /// Slot A (480 KiB).
    pub const SLOT_A: u32 = 0x0076_2000;
    pub const SLOT_A_SIZE: u32 = 0x0007_8000;
    /// Settings store, four sectors.
    pub const SETTINGS: u32 = 0x007D_A000;
    pub const SETTINGS_SIZE: u32 = 0x4000;
    /// Authenticator vault, one sector.
    pub const VAULT: u32 = 0x007D_E000;
    pub const VAULT_SIZE: u32 = 0x1000;
    /// Copy of the vendor boot record (1 KiB, descrambled) and stage 2.
    pub const BOOT_SECTOR: u32 = 0x007D_F000;
    pub const VENDOR_RECORD_COPY: u32 = 0x007D_F000;
    pub const STAGE2: u32 = 0x007D_F400;
    pub const STAGE2_MAX: u32 = 0x400;
    /// One past the last byte yotonrs may write.
    pub const FREE_END: u32 = 0x007E_0000;
    /// The Rockbox port's resident image header ("YRES" + len + sum), kept
    /// here so `restore rockbox` and stage 2 compatibility can be checked.
    pub const ROCKBOX_RESIDENT: u32 = 0x007C_C000;
}

/// Full layout (02 §2.4; decision D-2): the stock firmware is gone and
/// yotonrs owns everything except sector 0 (stage 1, written only by
/// `install-boot`) and the top 128 KiB (vendor data, left alone). The
/// settings store, vault and stage 2 stay where the dual layout has them,
/// so a switch keeps the owner's settings.
pub mod full {
    /// First byte yotonrs may erase or program (sector 0 excluded).
    pub const FREE_START: u32 = 0x0000_1000;
    /// Slot A (2 MiB less sector 0).
    pub const SLOT_A: u32 = 0x0000_1000;
    pub const SLOT_A_SIZE: u32 = 0x001F_F000;
    /// Slot B (the updater writes the inactive slot).
    pub const SLOT_B: u32 = 0x0020_0000;
    pub const SLOT_B_SIZE: u32 = 0x0020_0000;
    /// Resources: font pack, language packs, wallpapers.
    pub const RESOURCES: u32 = 0x0040_0000;
    pub const RESOURCES_SIZE: u32 = 0x0024_0000;
    /// Reserved up to the settings store (the old dual-boot slot lies here).
    pub const RESERVED: u32 = 0x0064_0000;
    /// One past the last byte yotonrs may write (the vendor data above is
    /// kept).
    pub const FREE_END: u32 = 0x007E_0000;
}

/// Firmware slots stage 2 tries, in order; stage 2 records the index it
/// booted in the low byte of the boot-info word.
pub const SLOTS: [u32; 3] = [full::SLOT_A, full::SLOT_B, dual::SLOT_A];

/// Whether `[addr, addr + len)` lies in the region the full layout lets
/// yotonrs write.
#[must_use]
pub const fn full_writable(addr: u32, len: u32) -> bool {
    match addr.checked_add(len) {
        Some(end) => addr >= full::FREE_START && end <= full::FREE_END && len > 0,
        None => false,
    }
}

/// Stage 2 magic at offset 0 of its 1 KiB record (`"YST2"`); entry at +0x11.
pub const STAGE2_MAGIC: u32 = crate::magic(b"YST2");
pub const STAGE2_LOAD: u32 = 0x0010_2400;
pub const STAGE2_ENTRY_OFFSET: u32 = 0x11;

/// Whether `[addr, addr + len)` lies entirely in the region the dual-boot
/// layout lets yotonrs write. Every NOR writer checks this.
#[must_use]
pub const fn dual_writable(addr: u32, len: u32) -> bool {
    match addr.checked_add(len) {
        Some(end) => addr >= dual::FREE_START && end <= dual::FREE_END && len > 0,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_tile() {
        assert_eq!(dual::SLOT_A + dual::SLOT_A_SIZE, dual::SETTINGS);
        assert_eq!(dual::SETTINGS + dual::SETTINGS_SIZE, dual::VAULT);
        assert_eq!(dual::VAULT + dual::VAULT_SIZE, dual::BOOT_SECTOR);
        assert_eq!(dual::STAGE2 + dual::STAGE2_MAX, dual::BOOT_SECTOR + 0x800);
        const { assert!(VENDOR_END < dual::FREE_START) };
    }

    #[test]
    fn full_regions() {
        assert_eq!(full::SLOT_A + full::SLOT_A_SIZE, full::SLOT_B);
        assert_eq!(full::SLOT_B + full::SLOT_B_SIZE, full::RESOURCES);
        assert_eq!(full::RESOURCES + full::RESOURCES_SIZE, full::RESERVED);
        const { assert!(full::RESERVED <= dual::SETTINGS) };
        assert_eq!(full::FREE_END, dual::FREE_END);
        assert!(!full_writable(0, 0x1000));
        assert!(!full_writable(0xFFF, 2));
        assert!(full_writable(0x1000, 0x1000));
        assert!(full_writable(0x7DF000, 0x1000));
        assert!(!full_writable(0x7DF000, 0x1001));
    }

    #[test]
    fn guard() {
        assert!(dual_writable(0x762000, 0x1000));
        assert!(dual_writable(0x7DF000, 0x1000));
        assert!(!dual_writable(0x761FFF, 2));
        assert!(!dual_writable(0x7DF000, 0x1001));
        assert!(!dual_writable(0, 0x200));
        assert!(!dual_writable(0x762000, 0));
        assert!(!dual_writable(u32::MAX, 2));
    }
}
