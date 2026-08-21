// A C program with NO libc at all: its own `_start`, raw `syscall` instructions, nothing else.
//
// Why this exists: `hello.c` links against musl AND lands at gcc's default base of 0x400000.
// Every Nyx-native app is linked at 0x40000000, so that one binary changes two things at once
// and cannot tell "musl's startup breaks on Nyx" apart from "loading anywhere near 0x400000
// breaks". This file is the musl-free half of that 2x2 — see tests/helloc/build.sh.
//
// Deliberately minimal: no static data beyond one string, no BSS, no relocations, no stack use
// before the syscall. If THIS cannot run, the problem is the kernel's exec/load path and
// nothing else, because there is nothing else left in the program.

static long sys(long n, long a, long b, long c) {
    long ret;
    __asm__ volatile("syscall"
                     : "=a"(ret)
                     : "a"(n), "D"(a), "S"(b), "d"(c)
                     : "rcx", "r11", "memory");
    return ret;
}

void _start(void) {
    static const char msg[] = "hello from a libc-free C binary on Nyx\n";
    sys(1, 1, (long)msg, sizeof(msg) - 1);   // write(1, msg, len)
    sys(231, 0, 0, 0);                        // exit_group(0)
    __builtin_unreachable();
}
