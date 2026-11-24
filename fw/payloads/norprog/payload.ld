/*
 * ymon RAM payload: loaded at 0x11E000 (orpheus_proto::mem::YMON_PAYLOAD),
 * entered at its first byte as a Thumb function that returns a pointer to
 * a {addr, size} result. Arguments at 0x11F800, data buffer at 0x122000.
 * The HAL's start-up code and vector table are not wanted here.
 */
ENTRY(payload_entry)

MEMORY
{
    PAYLOAD (rwx) : ORIGIN = 0x0011E000, LENGTH = 0x1800
    /* Never touched: placement for the HAL's unused emergency stack. */
    SCRATCH (rw)  : ORIGIN = 0x00120000, LENGTH = 0x2000
}

SECTIONS
{
    .text :
    {
        KEEP(*(.entry))
        *(.text .text.*)
    } > PAYLOAD

    .rodata : ALIGN(4)
    {
        *(.rodata .rodata.*)
    } > PAYLOAD

    .data : ALIGN(4)
    {
        *(.data .data.*)
        . = ALIGN(4);
    } > PAYLOAD

    .bss (NOLOAD) : ALIGN(4)
    {
        __bss_start = .;
        *(.bss .bss.*)
        . = ALIGN(4);
        __bss_end = .;
    } > PAYLOAD

    /* Pulled in by the HAL's fault shim, which never runs here: not
       loaded, nothing writes it. */
    .uninit (NOLOAD) :
    {
        *(.uninit .uninit.*)
    } > SCRATCH

    /DISCARD/ :
    {
        *(.vectors .vectors.*)
        *(.init .init.*)
        *(.ARM.exidx .ARM.exidx.*)
    }
}

/* Symbols the HAL's start-up/fault code names; unused by the payload. */
__stack_bottom = 0x00120000;
__stack_top = 0x00120000;
__stack_guard = 0x00120000;
__sbss = 0;
__ebss = 0;
__sdata = 0;
__edata = 0;
__sidata = 0;
