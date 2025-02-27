section .bss
    input_buffer resb 128  ; Buffer for user input

section .data
    prompt db 'nyxsh> ', 0  ; Shell prompt
    newline db 10, 0        ; Newline character
    echo_cmd db 'echo ', 0  ; "echo" command prefix
    clear_cmd db 'clear', 0 ; "clear" command
    cls_cmd db 'cls', 0     ; "cls" (same as "clear")
    exit_cmd db 'exit', 0   ; "exit" command
    clear_code db 27, '[2J', 27, '[H', 0  ; ANSI clear screen code

section .text
    global _start

_start:
shell_loop:
    call print_prompt
    call read_input
    call process_command
    jmp shell_loop

; Print shell prompt
print_prompt:
    mov eax, 4        ; sys_write
    mov ebx, 1        ; STDOUT
    mov ecx, prompt   ; Pointer to prompt string
    mov edx, 7        ; Length of prompt
    int 0x80
    ret

; Read user input
read_input:
    mov eax, 3        ; sys_read
    mov ebx, 0        ; STDIN
    mov ecx, input_buffer ; Buffer to store input
    mov edx, 128      ; Max bytes to read
    int 0x80
    ret

; Process the entered command
process_command:
    ; Check for "exit"
    mov esi, input_buffer
    mov edi, exit_cmd
    call compare_strings
    cmp al, 1
    je exit_shell

    ; Check for "clear"
    mov esi, input_buffer
    mov edi, clear_cmd
    call compare_strings
    cmp al, 1
    je clear_screen

    ; Check for "cls" (same as "clear")
    mov esi, input_buffer
    mov edi, cls_cmd
    call compare_strings
    cmp al, 1
    je clear_screen  

    ; Check for "echo"
    mov esi, input_buffer
    mov edi, echo_cmd
    call compare_prefix
    cmp al, 1
    je echo_command

    ret

; Exit shell
exit_shell:
    mov eax, 1  ; sys_exit
    xor ebx, ebx
    int 0x80

; Clear screen
clear_screen:
    mov eax, 4
    mov ebx, 1
    mov ecx, clear_code
    mov edx, 7
    int 0x80
    ret

; Echo command
echo_command:
    add esi, 5    ; Move past "echo "
    call print_string
    call print_newline  ; Ensure output ends cleanly
    ret

; Print a string
print_string:
    mov eax, 4
    mov ebx, 1
    mov edx, 128
    int 0x80
    ret

; Print a newline
print_newline:
    mov eax, 4
    mov ebx, 1
    mov ecx, newline
    mov edx, 1
    int 0x80
    ret

; Compare two strings
compare_strings:
    push esi
    push edi
.loop:
    mov al, [esi]
    cmp al, [edi]
    jne .not_equal
    test al, al
    je .equal
    inc esi
    inc edi
    jmp .loop
.equal:
    mov al, 1
    jmp .done
.not_equal:
    xor al, al
.done:
    pop edi
    pop esi
    ret

; Compare prefix (for "echo")
compare_prefix:
    push esi
    push edi
.loop:
    mov al, [edi]
    test al, al
    je .match
    cmp al, [esi]
    jne .no_match
    inc esi
    inc edi
    jmp .loop
.match:
    mov al, 1
    jmp .done
.no_match:
    xor al, al
.done:
    pop edi
    pop esi
    ret
