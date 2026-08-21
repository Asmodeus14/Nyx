#!/usr/bin/env python3
"""Inject the Nyx std PAL into the active rustup rust-src (idempotent).

Source of truth = this directory (vendor/nyx-std/). We patch the pinned toolchain's std
`cfg_select!` sites to add a `target_os = "nyx"` arm and drop in the matching impl files, so
`-Z build-std=std` picks up a real `nyx` platform. Safe to run repeatedly: every edit checks
for its own marker first.

Usage:  python3 vendor/nyx-std/apply.py            # patch the active nightly's rust-src
        python3 vendor/nyx-std/apply.py --check    # report patched/unpatched, make no changes
"""
import os, sys, subprocess, shutil

HERE = os.path.dirname(os.path.abspath(__file__))


def rust_src_sys():
    """Locate library/std/src/sys inside the ACTIVE toolchain's rust-src component."""
    sysroot = subprocess.check_output(["rustc", "--print", "sysroot"]).decode().strip()
    p = os.path.join(sysroot, "lib", "rustlib", "src", "rust", "library", "std", "src", "sys")
    if not os.path.isdir(p):
        sys.exit(f"rust-src not found at {p}\nInstall it:  rustup component add rust-src")
    return p


def insert_arm_after_cfg_select(path, arm_text, marker):
    """Insert `arm_text` immediately after the first `cfg_select! {` in `path`, unless `marker`
    is already present. Returns 'patched' | 'already' | 'skip'."""
    if not os.path.isfile(path):
        return "skip"
    text = open(path, encoding="utf-8").read()
    if marker in text:
        return "already"
    needle = "cfg_select! {"
    i = text.find(needle)
    if i < 0:
        return "skip"
    j = i + len(needle)
    new = text[:j] + "\n" + arm_text + text[j:]
    open(path, "w", encoding="utf-8").write(new)
    return "patched"


def add_os_to_arm(path, module_name, marker):
    """For subsystems that already have a suitable module (e.g. io/error `generic`), just add
    `target_os = "nyx"` into that arm's predicate list. `module_name` is the `mod X;` to find."""
    if not os.path.isfile(path):
        return "skip"
    text = open(path, encoding="utf-8").read()
    if marker in text:
        return "already"
    # Find the arm `... => { mod <module_name>;` and prepend `target_os = "nyx",` into its any(...).
    key = f"mod {module_name};"
    mi = text.find(key)
    if mi < 0:
        return "skip"
    # Walk back to the `=>` that opens this arm, then to the `any(` or predicate start.
    arrow = text.rfind("=>", 0, mi)
    if arrow < 0:
        return "skip"
    anyi = text.rfind("any(", 0, arrow)
    predi = text.rfind("\n", 0, arrow)
    if anyi > predi:  # predicate is an any(...) — inject a new alternative
        new = text[:anyi + 4] + f'\n        target_os = "nyx",' + text[anyi + 4:]
    else:  # single predicate `X => {` — widen to any(X, nyx)
        pred = text[predi + 1:arrow].strip()
        new = text[:predi + 1] + f'    any({pred}, target_os = "nyx") => ' + text[arrow + 2:]
    open(path, "w", encoding="utf-8").write(new)
    return "patched"


def add_nyx_beside_motor(path, marker):
    """Insert `target_os = "nyx",` next to the first `target_os = "motor",` in a cfg_select
    predicate list. Used for the sync mods (mutex/condvar/rwlock/once/thread_parking) whose futex
    arm already lists motor — nyx wants the same futex backend."""
    if not os.path.isfile(path):
        return "skip"
    text = open(path, encoding="utf-8").read()
    if marker in text:
        return "already"
    anchor = 'target_os = "motor",'
    i = text.find(anchor)
    if i < 0:
        return "skip"
    # Match the indentation of the motor line so the inserted line lines up.
    line_start = text.rfind("\n", 0, i) + 1
    indent = text[line_start:i]
    new = text[:i] + f'target_os = "nyx",\n{indent}' + text[i:]
    open(path, "w", encoding="utf-8").write(new)
    return "patched"


def copy(src_rel, dst_abs):
    src = os.path.join(HERE, src_rel)
    os.makedirs(os.path.dirname(dst_abs), exist_ok=True)
    shutil.copyfile(src, dst_abs)


MARK = '// NYX-STD-ARM'  # marker embedded in every arm we insert, for idempotency


def main():
    check = "--check" in sys.argv
    B = rust_src_sys()
    results = []

    def do(desc, fn):
        r = "check-only" if check else fn()
        results.append((desc, r))

    # --- pal/mod.rs: add a real `nyx` PAL module (before the `_ => unsupported` default). The pal
    #     also carries `futex` (sys::futex resolves via `pub use pal::*` in sys/mod.rs). ---
    def pal():
        copy("pal/mod.rs", os.path.join(B, "pal", "nyx", "mod.rs"))
        copy("pal/futex.rs", os.path.join(B, "pal", "nyx", "futex.rs"))
        copy("pal/tls.rs", os.path.join(B, "pal", "nyx", "tls.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        pub use self::nyx::*;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "pal", "mod.rs"), arm, MARK)
    do("pal/mod.rs", pal)

    # --- alloc: real mmap-backed allocator. ---
    def alloc():
        copy("sys/alloc_nyx.rs", os.path.join(B, "alloc", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "alloc", "mod.rs"), arm, MARK)
    do("alloc/mod.rs", alloc)

    # --- stdio: real fd 0/1/2. ---
    def stdio():
        copy("sys/stdio_nyx.rs", os.path.join(B, "stdio", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        pub use nyx::*;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "stdio", "mod.rs"), arm, MARK)
    do("stdio/mod.rs", stdio)

    # --- random: getrandom(318). Only export fill_bytes; the module's generic hashmap_random_keys
    #     already covers non-linux targets (incl. nyx). ---
    def random():
        copy("sys/random_nyx.rs", os.path.join(B, "random", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        pub use nyx::fill_bytes;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "random", "mod.rs"), arm, MARK)
    do("random/mod.rs", random)

    # --- fs: real File::open/read over the nyx open(2)/read(0)/close(3) syscalls (read-only). The
    #     generic `with_native_path` in fs/mod.rs already passes &Path straight through for nyx. ---
    def fs():
        copy("sys/fs_nyx.rs", os.path.join(B, "fs", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        use nyx as imp;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "fs", "mod.rs"), arm, MARK)
    do("fs/mod.rs", fs)

    # --- B-β.1: sync backends. nyx has a real futex (syscall 202), so join the `futex` arm that
    #     motor/linux use in each of the 5 sync primitives — just add `nyx` beside `motor`. ---
    for prim in ("mutex", "condvar", "rwlock", "once", "thread_parking"):
        do(f"sync/{prim}/mod.rs (futex)",
           (lambda p=prim: add_nyx_beside_motor(os.path.join(B, "sync", p, "mod.rs"), 'target_os = "nyx",')))

    # --- B-β.1: time. Real Instant/SystemTime over clock_gettime(228). ---
    def time():
        copy("sys/time_nyx.rs", os.path.join(B, "time", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        use nyx as imp;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "time", "mod.rs"), arm, MARK)
    do("time/mod.rs", time)

    # --- B-β.1: thread. Real sleep/yield_now/available_parallelism; Thread::new stays unsupported
    #     until B-β.2 (needs kernel thread-safe exit + per-thread TLS). ---
    def thread():
        copy("sys/thread_nyx.rs", os.path.join(B, "thread", "nyx.rs"))
        arm = (f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n'
               f'        pub use nyx::{{Thread, available_parallelism, current_os_id, set_name, '
               f'sleep, yield_now, DEFAULT_MIN_STACK_SIZE}};\n    }}\n')
        return insert_arm_after_cfg_select(os.path.join(B, "thread", "mod.rs"), arm, MARK)
    do("thread/mod.rs", thread)

    # --- B-γ.3: process. Command spawn/wait over fork(57)+execve(59)+wait4(61). Path-only spawn
    #     (kernel execve carries no argv/env); stdio inherited; kill unsupported (no signals). ---
    def process():
        copy("sys/process_nyx.rs", os.path.join(B, "process", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        use nyx as imp;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "process", "mod.rs"), arm, MARK)
    do("process/mod.rs", process)

    # --- Browser blocker #3: std::net. Real TcpStream + lookup_host over the kernel's
    #     socket(41)/connect(42)/read/write/close and dns_resolve(534); TcpListener and UdpSocket
    #     stay as the unsupported stubs (the kernel has no listen/accept and no bound UDP). rustls
    #     and every HTTP client are written against std::net, so nothing above this could link
    #     without it. The arm goes in sys/net/connection/mod.rs, whose FIRST cfg_select is the
    #     connection one — sys/net/mod.rs itself has none. Hostname falls through to unsupported. ---
    def net():
        copy("sys/net_nyx.rs", os.path.join(B, "net", "connection", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        pub use nyx::*;\n    }}\n'
        return insert_arm_after_cfg_select(os.path.join(B, "net", "connection", "mod.rs"), arm, MARK)
    do("net/connection/mod.rs", net)

    # --- B-γ.3: sys::exit. `std::process::exit` (and returning from `main`) calls
    #     `crate::sys::exit::exit(code)`. nyx has no arm in that cfg_select, so it fell through to
    #     `_ => crate::intrinsics::abort()` (a `ud2` invalid opcode) — every std process that reached
    #     exit died on #UD instead of terminating. Add a nyx arm that does exit_group(231) inline
    #     (self-contained asm; no dependency on pal re-exports). Anchored on the motor exit arm, which
    #     is unique to the `exit()` fn's cfg_select (the file's SECOND cfg_select, so the generic
    #     insert_arm_after_cfg_select — which targets the first — can't be used here). ---
    def sys_exit():
        path = os.path.join(B, "exit.rs")
        if not os.path.isfile(path):
            return "skip"
        text = open(path, encoding="utf-8").read()
        if MARK in text:
            return "already"
        anchor = 'target_os = "motor" => {\n            moto_rt::process::exit(code)\n        }\n'
        i = text.find(anchor)
        if i < 0:
            return "anchor-not-found"
        j = i + len(anchor)
        nyx_arm = (
            f'        target_os = "nyx" => {{ {MARK}\n'
            '            // Nyx: terminate via exit_group(231). Never returns.\n'
            '            unsafe {\n'
            '                core::arch::asm!(\n'
            '                    "syscall",\n'
            '                    in("rax") 231usize,\n'
            '                    in("rdi") code as usize,\n'
            '                    options(noreturn, nostack),\n'
            '                );\n'
            '            }\n'
            '        }\n'
        )
        new = text[:j] + nyx_arm + text[j:]
        open(path, "w", encoding="utf-8").write(new)
        return "patched"
    do("exit.rs (nyx exit_group arm)", sys_exit)

    # --- io/error: nyx gets its OWN errno table.
    #     It used to reuse std's `generic` module, which is a stub: `decode_error_kind` returns
    #     `Uncategorized` for every code and `error_string` returns the literal "operation
    #     successful" for every errno. That produced the message "operation successful (os error
    #     110)" for a timeout, and — far worse — made every `e.kind()` test in std-facing code
    #     silently false. The browser's stepped fetch relies on `ErrorKind::TimedOut` meaning "no
    #     data yet", so with the stub in place a routine short read was read as a hard failure. ---
    def io_error():
        path = os.path.join(B, "io", "error", "mod.rs")
        # Undo the old `add_os_to_arm` edit first. That helper assumed a single-predicate arm and
        # met a multi-line `any( ... )`, so it rewrote the closing `    ) => {` into
        # `    any(), target_os = "nyx") =>  {`. That happens to still parse (a nested empty `any()`
        # is simply false), which is why it went unnoticed — but leaving nyx listed on the generic
        # arm as well as its own is a trap for whoever reads this next.
        if os.path.isfile(path):
            text = open(path, encoding="utf-8").read()
            broken = '    any(), target_os = "nyx") =>  {'
            if broken in text:
                open(path, "w", encoding="utf-8").write(text.replace(broken, "    ) => {"))
        copy("sys/io_error_nyx.rs", os.path.join(B, "io", "error", "nyx.rs"))
        arm = f'    target_os = "nyx" => {{ {MARK}\n        mod nyx;\n        pub use nyx::*;\n    }}\n'
        return insert_arm_after_cfg_select(path, arm, MARK)
    do("io/error/mod.rs (nyx errno table)", io_error)

    # --- C2: std/build.rs allowlist. std's build.rs emits `cfg(restricted_std)` for any target NOT
    #     in its known-platform list; that cfg makes std (and EVERY downstream crate that touches it,
    #     e.g. logos/thiserror) require `#![feature(restricted_std)]`. Our own crates opt in, but
    #     registry deps can't — and `-Zcrate-attr` can't be scoped to skip core/compiler_builtins.
    #     Fix at the source: add nyx beside motor in the allowlist so std is treated as a fully
    #     supported platform and never sets restricted_std. build.rs is two dirs up from src/sys. ---
    def build_allowlist():
        path = os.path.join(B, "..", "..", "build.rs")
        if not os.path.isfile(path):
            return "skip"
        text = open(path, encoding="utf-8").read()
        if 'target_os == "nyx"' in text:
            return "already"
        anchor = '|| target_os == "motor"\n'
        i = text.find(anchor)
        if i < 0:
            return "anchor-not-found"
        j = i + len(anchor)
        line_start = text.rfind("\n", 0, i) + 1
        indent = text[line_start:i]
        new = text[:j] + f'{indent}|| target_os == "nyx"\n' + text[j:]
        open(path, "w", encoding="utf-8").write(new)
        return "patched"
    do("build.rs (nyx in restricted_std allowlist)", build_allowlist)

    # --- thread_local (B-β.2b): nyx now has NATIVE fs:-relative TLS (target_thread_local = true in
    #     the target json + a variant-II block installed by pal::tls before main). So nyx must NOT be
    #     in the `no_threads` arm — remove it so cfg_select falls through to the `target_thread_local`
    #     (native) arm. The `guard::enable` no-op arm KEEPS nyx (see tls_guard below) — leaking TLS
    #     destructors is fine for now. Idempotent: the removal target is the exact 2-line nyx entry. ---
    def tls():
        path = os.path.join(B, "thread_local", "mod.rs")
        if not os.path.isfile(path):
            return "skip"
        text = open(path, encoding="utf-8").read()
        # The no_threads arm lists `target_os = "nyx",` on its own line before `target_os = "vexos",`.
        # Only touch the FIRST occurrence (the no_threads storage arm), never the guard arm below it.
        needle = 'target_os = "nyx",\n        target_os = "vexos",'
        if needle not in text:
            return "already"  # nyx already removed from no_threads arm
        new = text.replace(needle, 'target_os = "vexos",', 1)
        open(path, "w", encoding="utf-8").write(new)
        return "patched"
    do("thread_local/mod.rs (drop nyx from no_threads -> native)", tls)

    # --- thread_local `guard`: nyx has no thread-exit hook, so use the no-op `enable()` arm (the
    #     uefi/zkvm/vexos one) instead of the `_ => mod key` fallback that needs a TLS key table. ---
    def tls_guard():
        path = os.path.join(B, "thread_local", "mod.rs")
        if not os.path.isfile(path):
            return "skip"
        text = open(path, encoding="utf-8").read()
        # Anchor on the guard arm specifically: the `target_os = "vexos",` immediately followed by
        # the no-op `enable()` body (") => {\n            pub(crate) fn enable()").
        anchor = 'target_os = "vexos",\n        ) => {\n            pub(crate) fn enable() {'
        marker = 'target_os = "nyx",\n            target_os = "vexos",\n        ) => {\n            pub(crate) fn enable() {'
        if marker in text:
            return "already"
        i = text.find(anchor)
        if i < 0:
            return "skip"
        # Insert nyx just before this arm's `target_os = "vexos",`
        vex = text.rfind('target_os = "vexos",', 0, i + len('target_os = "vexos",'))
        new = text[:vex] + 'target_os = "nyx",\n            ' + text[vex:]
        open(path, "w", encoding="utf-8").write(new)
        return "patched"
    do("thread_local/mod.rs (guard no-op)", tls_guard)

    print(f"rust-src sys: {B}")
    for d, r in results:
        print(f"  [{r:>10}]  {d}")


if __name__ == "__main__":
    main()
