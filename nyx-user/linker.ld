ENTRY(_start)

SECTIONS {
    /* We load at 16MB. The kernel jumps here. */
    . = 0x1000000;

    .text : {
        /* CRITICAL: Place the entry section first! */
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