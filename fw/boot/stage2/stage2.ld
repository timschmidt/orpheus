/* Stage 2: 1 KiB record at NOR 0x7DF400, loaded to 0x102400 by stage 1,
 * which checks "YST2" at +0 and enters +0x11 (Thumb). The trampoline (NOR
 * read + jump) runs from the top of SRAM1 because the image it loads
 * overwrites 0x100000..; its stack sits just below it. SRAM1 holds the
 * frame buffer, which a normal boot may overwrite; the ADFU path returns
 * before touching it. */
ENTRY(_stage2_start)
SECTIONS
{
    . = 0x102400;
    .hdr : { KEEP(*(.yst2_header)) }
    .text 0x102410 : {
        KEEP(*(.yst2_entry))
        *(.text .text.*)
        *(.rodata .rodata.*)
        . = ALIGN(4);
    }
    .tramp 0x0015FC00 : AT(ADDR(.text) + SIZEOF(.text)) {
        __tramp_start = .;
        KEEP(*(.tramp .tramp.*))
        . = ALIGN(4);
        __tramp_end = .;
    }
    __tramp_load = LOADADDR(.tramp);
    __stage2_end = LOADADDR(.tramp) + SIZEOF(.tramp);
    .data : { *(.data .data.*) }
    .bss (NOLOAD) : { *(.bss .bss.* COMMON) }
    ASSERT(__stage2_end <= 0x102800, "stage 2 must fit 1 KiB")
    ASSERT(SIZEOF(.bss) == 0, "stage 2 must not use .bss")
    ASSERT(SIZEOF(.data) == 0, "stage 2 must not use .data")
    /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) *(.comment) }
}
