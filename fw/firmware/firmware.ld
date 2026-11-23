/* Placeholder layout until WP-PG-1 generates the paged one: one flat
 * resident image at 0x100000, entered at its first byte (Thumb). */
ENTRY(_start)
MEMORY
{
    SRAM0 (rwx) : ORIGIN = 0x00100000, LENGTH = 0x37C00
}
SECTIONS
{
    .text : { KEEP(*(.init)) *(.text .text.*) *(.rodata .rodata.*) . = ALIGN(4); } > SRAM0
    .data : { *(.data .data.*) . = ALIGN(4); } > SRAM0
    .bss (NOLOAD) : { __bss_start = .; *(.bss .bss.* COMMON) . = ALIGN(4); __bss_end = .; } > SRAM0
    __stack_top = ORIGIN(SRAM0) + LENGTH(SRAM0);
    /DISCARD/ : { *(.ARM.exidx*) *(.ARM.extab*) }
}
