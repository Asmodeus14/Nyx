section .data
    hello db 'Welcome to Nyx!', 0xA, 0  ; Added newline (0xA) before the null terminator

section .bss

section .text
    global _start

_start:
    ; Write the string to stdout
    mov rax, 1          ; syscall number for sys_write (1)
    mov rdi, 1          ; file descriptor 1 is stdout
    mov rsi, hello      ; pointer to the string
    mov rdx, 14         ; length of the string (13 characters + 1 newline)
    syscall             ; call kernel

    ; Exit the program
    mov rax, 60         ; syscall number for sys_exit (60)
    xor rdi, rdi        ; exit code 0
    syscall             ; call kernel
