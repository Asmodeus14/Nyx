[BITS 16]
[ORG 0x8000]

_start:
    cli
    xor ax, ax
    mov ds, ax
    mov es, ax
    mov ss, ax

    lgdt [gdt32_pointer]

    mov eax, cr0
    or eax, 1
    mov cr0, eax

    jmp 0x08:mode32

[BITS 32]
mode32:
    mov ax, 0x10
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    mov eax, cr4
    or eax, 1 << 5
    mov cr4, eax

    mov eax, [0x8F00]
    mov cr3, eax

    ; --- THE FIX: Enable Long Mode AND No-Execute (NXE) ---
    mov ecx, 0xC0000080
    rdmsr
    or eax, 0x0900 
    wrmsr
    ; ------------------------------------------------------

    mov eax, cr0
    or eax, 1 << 31
    mov cr0, eax

    lgdt [gdt64_pointer]
    jmp 0x18:mode64

[BITS 64]
mode64:
    mov ax, 0x20
    mov ds, ax
    mov es, ax
    mov ss, ax
    mov fs, ax
    mov gs, ax

    mov rdi, [0x8F08]
    mov rsi, [0x8F10]
    mov rsp, [0x8F18]
    mov rax, [0x8F20]

    call rax

hang:
    hlt
    jmp hang

align 16
gdt32:
    dq 0x0000000000000000
    dq 0x00cf9a000000ffff
    dq 0x00cf92000000ffff
gdt32_pointer:
    dw gdt32_pointer - gdt32 - 1
    dd gdt32

align 16
gdt64:
    dq 0x0000000000000000
    dq 0x00cf9a000000ffff
    dq 0x00cf92000000ffff
    dq 0x00af9a000000ffff
    dq 0x00af92000000ffff
gdt64_pointer:
    dw gdt64_pointer - gdt64 - 1
    dd gdt64