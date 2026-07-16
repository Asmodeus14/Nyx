// B-gamma.3 child: minimal std binary spawned by stdhello via std::process::Command.
// Prints one line (proves inherited stdio survives fork+execve) and exits 42 (proves the
// parent's wait4 WEXITSTATUS decode). Same _start/TLS bring-up as stdhello.
#![feature(restricted_std)]

core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, rsp",         // stash entry RSP for argc/argv below
    "mov rdi, rsp",         // arg0 = &argc (SysV info block, walked for AT_NYX_TLS_* auxv)
    "and rsp, -16",
    "call __nyx_setup_main_tls",
    "mov rdi, [rbx]",       // argc
    "lea rsi, [rbx + 8]",   // argv
    "and rsp, -16",
    "call main",
    "mov edi, eax",
    "mov eax, 231",         // SYS_exit_group
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

fn main() {
    println!("stdchild: hello from a spawned std process");
    std::process::exit(42);
}
