/*
 * Memory of pass 1 of `cargo xtask pagelink` (see layout.ld): one 16 MiB
 * fake region so that everything links resident; pagelink reads the map
 * and the relocations of this link to decide what is paged.
 */
MEMORY
{
    SRAM0 (rwx) : ORIGIN = 0x10000000, LENGTH = 0x1000000
    SRAM1 (rw)  : ORIGIN = 0x00158000, LENGTH = 0x8000
}

ORPHEUS_FLAT_PASS = 1;
/* Page counts (the generated pass-2 script sets the real ones). The
 * firmware reads them as `&__orpheus_code_pages as *const u8 as usize`. */
__orpheus_code_pages = 0;
__orpheus_rodata_pages = 0;
