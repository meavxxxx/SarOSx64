OUTPUT_FORMAT(elf64-x86-64)
ENTRY(kernel_main)

PHDRS
{
    text   PT_LOAD FLAGS(5); /* R-X */
    rodata PT_LOAD FLAGS(4); /* R-- */
    data   PT_LOAD FLAGS(6); /* RW- */
}

SECTIONS
{
    . = 0xFFFFFFFF80200000;

    .text : ALIGN(0x1000) {
        *(.text .text.*)
    } :text

    .rodata : ALIGN(0x1000) {
        *(.rodata .rodata.*)
        *(.limine_reqs .limine_reqs.*)
    } :rodata

    .data : ALIGN(0x1000) {
        *(.data .data.*)
    } :data

    .bss (NOLOAD) : ALIGN(0x1000) {
        *(COMMON)
        *(.bss .bss.*)
    } :data

    /DISCARD/ : {
        *(.eh_frame .eh_frame.*)
        *(.note .note.*)
        *(.comment)
    }
}
