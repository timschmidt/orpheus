/*
 * Resident layout of a paged image (the firmware, and bring-up images that
 * are linked by `cargo xtask pagelink`). Included by both link passes:
 *
 *   pass 1  paged-flat.ld   memory-flat.ld + this file: everything resident
 *                           in one large fake region (not a runnable image;
 *                           pagelink measures and classifies it)
 *   pass 2  generated       memory.ld + the page output sections (code at
 *                           0x200000, rodata at 0x340000, which claim their
 *                           input sections first) + this file
 *
 * Same shape as atj2157-hal/link/ram.ld:
 *
 *   0x100000  .init (_start), .vectors (512-aligned), .text, .rodata, .data
 *   ...       .bss, .uninit
 *   ...       stack (12 KiB at least, painted)  ends at 0x137400
 *   0x137400  log ring (2 KiB)
 *   0x137C00  persistent area (fault record, result block, mailbox)
 */
ENTRY(_start)

SECTIONS
{
    .text :
    {
        KEEP(*(.init))
        . = ALIGN(512);
        KEEP(*(.vectors))
        *(.text .text.*)
    } > SRAM0

    .rodata : ALIGN(4)
    {
        *(.rodata .rodata.*)
    } > SRAM0

    .data : ALIGN(4)
    {
        *(.data .data.*)
        . = ALIGN(4);
    } > SRAM0
    __image_end = .;

    .bss (NOLOAD) : ALIGN(4)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > SRAM0

    .uninit (NOLOAD) : ALIGN(4)
    {
        *(.uninit .uninit.*)
    } > SRAM0

    .stack (NOLOAD) : ALIGN(8)
    {
        __stack_bottom = .;
        . = ORIGIN(SRAM0) + LENGTH(SRAM0);
        __stack_top = .;
    } > SRAM0

    /* Frame buffer: not zeroed, not loaded (the host reads it). */
    .framebuffer (NOLOAD) : { KEEP(*(.framebuffer)) } > SRAM1

    /DISCARD/ : { *(.ARM.exidx .ARM.exidx.*) *(.ARM.extab .ARM.extab.*) }
}

ASSERT(__stack_top - __stack_bottom >= 0x3000, "less than 12 KiB left for the stack");
ASSERT(YOTON_FLAT_PASS || __image_end <= 0x118000, "resident image overlaps ymon at 0x118000");
ASSERT(!DEFINED(__vectors) || __vectors % 512 == 0, "vector table not 512-aligned");
