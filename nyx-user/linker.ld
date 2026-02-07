ENTRY(_start)

SECTIONS {
    /* FIX: Use 16MB (0x1000000). This fits in the "Small" code model. */
    . = 0x1000000;

    .text : {
        *(.text.entry) 
        *(.text._start)
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