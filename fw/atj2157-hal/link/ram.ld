/*
 * Layout of a RAM-loaded image (bring-up images; also the firmware until
 * WP-PG-1 generates the paged layout). Loaded at 0x100000 by ymon, which
 * lives at 0x118000-0x11BFFF while loading: the *loaded* part (text,
 * rodata, data) must end below it. .bss may extend past it (zeroed after
 * the jump; ymon is never returned to).
 *
 *   0x100000  .init (_start), .vectors (512-aligned), .text, .rodata, .data
 *   ...       .bss
 *   ...       stack (12 KiB, painted)            ends at 0x137400
 *   0x137400  log ring (2 KiB, yoton_proto::logring::ALT_BASE)
 *   0x137C00  persistent area (fault record, result block, mailbox)
 */
ENTRY(_start)

MEMORY
{
    SRAM0 (rwx) : ORIGIN = 0x00100000, LENGTH = 0x37400
}

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

    /DISCARD/ : { *(.ARM.exidx .ARM.exidx.*) *(.ARM.extab .ARM.extab.*) }
}

ASSERT(__stack_top - __stack_bottom >= 0x3000, "less than 12 KiB left for the stack");
ASSERT(__image_end <= 0x118000, "loaded image overlaps ymon at 0x118000");
ASSERT(__vectors % 512 == 0, "vector table not 512-aligned");
