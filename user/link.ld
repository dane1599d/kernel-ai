/* Ring-3 program image, loaded at kabi::layout::IMAGE_BASE. One segment per
   permission so the loader can map code read-only and data non-executable. */
OUTPUT_FORMAT(elf64-x86-64)
ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
    note PT_NOTE FLAGS(4);
}

SECTIONS
{
    . = 0x400000;
    .text : { *(.text .text.*) } :text

    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) } :rodata
    /* The "KAI" note with the ABI version (ADR 0001 D10), in its own PT_NOTE. */
    .note.kai : { KEEP(*(.note.kai)) } :rodata :note

    . = ALIGN(4096);
    .data : { *(.data .data.*) } :data
    .bss : { *(.bss .bss.*) *(COMMON) } :data

    /DISCARD/ : { *(.eh_frame*) *(.note .note.*) }
}
