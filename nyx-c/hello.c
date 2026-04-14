#include "nyx_syscalls.h"

// Calculate string length at compile time for convenience
#define MSG "\n[C-SPACE] Hello from Native C running on NyxOS!\n"
#define MSG_LEN (sizeof(MSG) - 1)

// The true entry point for our ELF executable.
// 🚨 We force this into the .text.entry section so it matches 
// the Rust linker script and satisfies the NyxOS kernel loader!
void __attribute__((section(".text.entry"))) _start() {
    
    // 1. Ask the kernel to print our message to STDOUT (FD 1)!
    print(MSG, MSG_LEN);

    // 2. Ask the kernel to gracefully terminate us with an exit code!
    sys_exit(42);
}