#pragma once
#include <stdint.h>
#include <stddef.h>

// 🚨 x86_64 Linux ABI Syscall Invoker
// rax = syscall_id, rdi = arg1, rsi = arg2, rdx = arg3
static inline uint64_t syscall3(uint64_t n, uint64_t a1, uint64_t a2, uint64_t a3) {
    uint64_t ret;
    __asm__ volatile (
        "syscall"
        : "=a" (ret)
        : "a" (n), "D" (a1), "S" (a2), "d" (a3)
        : "rcx", "r11", "memory"
    );
    return ret;
}

static inline uint64_t syscall1(uint64_t n, uint64_t a1) {
    uint64_t ret;
    __asm__ volatile (
        "syscall"
        : "=a" (ret)
        : "a" (n), "D" (a1)
        : "rcx", "r11", "memory"
    );
    return ret;
}

// ─────────────────────────────────────────────────────────────────────────
// POSIX WRAPPERS
// ─────────────────────────────────────────────────────────────────────────

// ID 1 = sys_write
static inline int sys_write(int fd, const char* buf, size_t len) {
    return (int)syscall3(1, (uint64_t)fd, (uint64_t)buf, (uint64_t)len);
}

// ID 60 = sys_exit
static inline void sys_exit(int code) {
    syscall1(60, (uint64_t)code);
    while (1) {} // Halt to satisfy compiler
}

// Helper to print to our Terminal (FD 1 = STDOUT)
static inline void print(const char* str, size_t len) {
    sys_write(1, str, len);
}