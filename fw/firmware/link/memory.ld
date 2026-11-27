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
    /* Since PG-2 the rodata window is all of SRAM3's alias space (low
     * halves only: the groups have fixed addresses); cold code groups
     * (.px_gN) are executable. */
    PAGED_RO   (rx)  : ORIGIN = 0x00300000, LENGTH = 0x100000
}

ORPHEUS_FLAT_PASS = 0;
