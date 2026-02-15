ENTRY(_start)

SECTIONS {
    /* FIX: Load at 32MB (0x2000000) to match Kernel ELF Loader expectations */
    . = 0x2000000;

    .text : {
        /* Ensure entry point is first */
        *(.text.entry) 
        *(.text*)
    }

    .rodata : {
        *(.rodata*)
    }

    .data : {
        *(.data*)
    }

    .bss : {
        *(.bss*)
    }

    /DISCARD/ : {
        *(.eh_frame)
        *(.comment)
        *(.note*)
    }
}