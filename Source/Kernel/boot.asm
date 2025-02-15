bits 32
section .multiboot
align 8
dd 0xE85250D6            ; Multiboot2 magic number
dd 0                     ; Architecture (0 = 32-bit protected mode)
dd header_end - header_start ; Header length
dd -(0xE85250D6 + 0 + (header_end - header_start)) ; Checksum

header_start:
dd 0                   ; No flags
dd 0, 0                ; No load address
header_end:

section .text
global _start   ; Make _start visible to the linker
_start:
    mov esp, stack_top  ; Set up stack
    mov edi, 0xB8000    ; VGA text buffer
    mov esi, msg
    mov ah, 0x0F        ; White-on-black text

.loop:
    lodsb
    test al, al
    jz .halt
    stosw               ; Write character + attribute
    jmp .loop

.halt:
    cli
    hlt

section .data
msg db 'Welcome to Nyx!', 0

section .bss
resb 4096
stack_top:
