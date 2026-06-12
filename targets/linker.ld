ENTRY(_start)

SECTIONS {
    /* 🚨 CRITICAL FIX: Push userspace up to 1GB to avoid kernel collision! */
    . = 0x40000000;

    .text : ALIGN(4K) {
        *(.text.entry)
        *(.text .text.*)
    }

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
    }
}