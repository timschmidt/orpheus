/*
 * Memory of the final (pass 2) link of a paged image; see layout.ld.
 * PAGED_CODE / PAGED_RO hold the page output sections at their alias
 * addresses (docs/plan/02-architecture.md §3.1). They are ordinary
 * allocated sections with their own PT_LOAD segments: lld's (COPY)/(INFO)
 * output section types drop the address and break relocations, so the
 * pages stay allocated and `pagelink` extracts them by section; only the
 * SRAM0 sections form the resident image.
 */
MEMORY
{
    SRAM0      (rwx) : ORIGIN = 0x00100000, LENGTH = 0x37400
    SRAM1      (rw)  : ORIGIN = 0x00158000, LENGTH = 0x8000
    PAGED_CODE (rx)  : ORIGIN = 0x00200000, LENGTH = 0x100000
    PAGED_RO   (r)   : ORIGIN = 0x00340000, LENGTH = 0xC0000
}

YOTON_FLAT_PASS = 0;
