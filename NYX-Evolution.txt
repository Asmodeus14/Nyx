
================================================================================
Nyx OS — Userspace Platform Evolution (std, apps, qclang)
================================================================================

This roadmap tracks Nyx growing from bare-metal no_std apps into a real userspace
platform: a desktop Image Viewer, a genuine Rust `std` port (target_os = "nyx"),
and the qclang quantum compiler running ON-DEVICE. The three efforts are staged
so each ships value on its own; qclang-on-device is gated on the std port.

Dependency chain:
  Workstream A (Image Viewer)  -> independent, no_std, ships first.
  Workstream B (real std)      -> the keystone; a Linux-personality PAL.
  Workstream C (qclang)        -> a std program; depends on B reaching milestone B-gamma.

KEY DISCOVERY that makes the std port tractable: the kernel ALREADY speaks the
Linux x86-64 syscall ABI and was deliberately pre-wired for std. Implemented in
nyx-kernel/src/interrupts.rs (syscall_dispatcher): mmap(9, real page alloc),
arch_prctl(158, ARCH_SET_FS for TLS), getrandom(318, commented "Required for Rust
HashMaps"), set_tid_address(218), spawn_thread(58), fork(57), execve(59), exit(60),
writev(20), pipe(22), dup2(33), sockets(41/42/44/45), signal stubs(13/14/131).
The std port is mostly FILLING GAPS, not inventing an ABI.

--------------------------------------------------------------------------------
STATUS SUMMARY
--------------------------------------------------------------------------------

[DONE] A1  Image Viewer app skeleton (no_std nyx-gui app)
[DONE] A2a BMP/TGA hand-rolled decoder + blit + fit-to-window scale
[DONE] A3  Register Image Viewer (Start menu + Build.sh + workspace)
[TODO] A2b PNG/JPEG decoder (no_std crate, default-features=false + libm)
[DONE] B1  Kernel syscall gaps for std (futex, clocks, sleep, yield, wait4, ...)
[DONE] B2  SysV process startup (argc/argv/envp/auxv stack image; PT_TLS parsed, block-install deferred to PAL)
[WIP ] B3  The `nyx` std PAL (library/std/src/sys/pal/nyx) — B-alpha + B-beta FULLY HW-VERIFIED (println+fs read+Mutex/Condvar/RwLock/Once+Instant+sleep+native thread_local+REAL thread::spawn/join across cores); B-gamma (HashMap/fs write/process) + net PAL still TODO
[DONE] B4  Toolchain / build-std integration (target json + apply.py inject; stdhello links+RUNS)
[TODO] C1  Slim qclang to a portable lib (prune deps, no stderr on compile path)
[TODO] C2  On-device qclang front-end app (qcstudio, a std target_os=nyx binary)
[TODO] C3  Validation: on-device .ql -> .qasm byte-matches host qclang

Milestone ladder for B (each is a real boot test):
  B-alpha  std hello-world: println! + std::fs read; serial "[PID n] Exited (Code: 0)".  [FULLY HW-VERIFIED: println+eprintln 2026-07-12; std::fs::read_to_string of bundled hello.txt round-tripped 2026-07-15, exit 0]
  B-beta   thread::spawn + Mutex/Condvar + Instant::now() + thread::sleep.  [FULLY HW-VERIFIED 2026-07-15: sync/time on main thread, native fs:-TLS, then 2 spawned workers on separate cores + shared futex Mutex (2000 increments) + join + per-thread TLS isolation, exit 0]
  B-gamma  HashMap + std::process spawn/wait + full fs read/write. (Unlocks C.)

--------------------------------------------------------------------------------
Workstream A: Image Viewer  (independent, no_std, low risk)
--------------------------------------------------------------------------------
Objective: open image files from the desktop and display them in a window.

Reuses primitives that ALREADY exist:
  - Canvas::composite_buffer  (blit a &[u32] B8G8R8A8 buffer)  libs/gui/src/canvas.rs:113
  - ImageView widget                                           libs/gui/src/ui.rs:650
  - NyxApp trait + nyx_gui::app::run event loop                libs/gui/src/app.rs:6
Model app to copy: apps/explorer/src/main.rs.

Actionable Steps:

A1. App skeleton.
    - New crate apps/imageviewer, package name `nyx-imageviewer`; deps nyx-api,
      nyx-gui, linked_list_allocator (copy apps/explorer/Cargo.toml).
    - #![no_std] #![no_main]; LockedHeap global allocator; _start with
      #[unsafe(link_section=".text.entry")] + sys_alloc_pages heap init;
      #[panic_handler] -> sys_exit. Implement NyxApp (title/draw/on_key/on_mouse).
    - Load bytes with the nyx-api helpers sys_open/sys_read/sys_close
      (libs/api/src/lib.rs:133-146). User files live under /mnt/nvme/...

A2a. Hand-rolled decoders (zero deps).
    - 24/32-bit uncompressed BMP (and TGA) -> Vec<u32> B8G8R8A8. Proves the whole
      load -> decode -> blit -> present path.
    - Nearest-neighbour scale to fit the window (composite_buffer does NOT scale).
      Keep the decoded Vec<u32> owned by the app struct (ImageView borrows a raw ptr).

A2b. Real formats (no_std crate).
    - Pull a decoder the SAME way libs/gui/Cargo.toml pulls ttf-parser /
      ab_glyph_rasterizer: default-features = false + a libm feature, and add libm.
      Candidates: zune-png / zune-jpeg (alloc-only) or hand-rolled inflate.
    - Dispatch by extension/magic. Cap image dimensions to protect the heap; a
      pathological file can blow the 1 MiB default heap (bump sys_alloc_pages).

A3. Registration (exact edits).
    - Workspace Cargo.toml `members`: add "apps/imageviewer".
    - apps/compositor/src/main.rs Start menu: append "> Image Viewer" to BOTH item
      arrays (~:1111, ~:1131); extend click dispatch (~:521-528) — turn the final
      `else` (GlCube) into a bounded `else if rel_y < 240` and add a new arm doing
      sys_fork()==0 -> sys_execve("/mnt/nvme/apps/ImageViewer.nyx/run.bin\0"); bump
      menu_h 240 -> 280 in all three geometry spots (~:506, ~:1057, ~:1108/1128).
    - Build.sh: add the build, the mkdir build_initrd/apps/ImageViewer.nyx, and the
      cp target/x86_64-nyx/release/nyx-imageviewer .../run.bin. Bundle a sample image.

Milestone A: Image Viewer opens from the Start menu and displays a bundled BMP,
then PNG/JPEG, scaled to the window — verified by panel photo.

--------------------------------------------------------------------------------
Workstream B: Real userspace std (target_os = "nyx")   <- keystone / long pole
--------------------------------------------------------------------------------
Objective: genuine Rust `std` in userspace so real std crates (and qclang) build
and run on Nyx.

Strategy: a custom target + a `nyx` platform-abstraction layer (PAL) added to a
patched rust-src, built with -Z build-std=std,panic_abort. Chosen over a rustc
fork (too heavy to maintain) and over a musl-ABI shim (needs the same kernel work
with less control). The kernel's Linux-numbered syscalls back the PAL directly.

B1. Kernel syscall gaps (nyx-kernel/src/interrupts.rs, the `match id` dispatch).
    Currently missing -> fall to the `_ => EINVAL` arm (:1867). Add:
    - futex(202) — linchpin for std Mutex/Condvar/RwLock/thread park; needs a
      wait-queue keyed by user address in the scheduler.
    - clock_gettime(228) MONOTONIC+REALTIME (UPTIME_MS/TSC + RTC); nanosleep(35) /
      clock_nanosleep(230) (Process already has a wake_tsc field).
    - sched_yield(24) (yield path already exists via the 0x41 vector),
      getpid(39)/gettid(186), wait4(61), exit_group(231) aliased to exit.
    - Real exit codes + reaping: exit(60) currently only marks Zombie (:1073-1118);
      store the code and let wait4 retrieve/reap it.
    - Optionally replace getrandom's constant-42 fill (:1135) with a TSC/RTC PRNG.

B2. SysV process startup (nyx-kernel/src/process.rs load_elf + the execve arm).
    Today the loader handles only PT_LOAD and hands _start a BARE stack (:1040-1048).
    A real std/CRT needs:
    - Initial stack image: [argc][argv..][NULL][envp..][NULL][auxv..][AT_NULL] + strings.
    - auxv: AT_PAGESZ, AT_RANDOM(16), AT_PHDR/AT_PHENT/AT_PHNUM/AT_ENTRY, AT_HWCAP, AT_NULL.
    - PT_TLS: record the template (image ptr, filesz, memsz, align); allocate + install
      the initial thread's TLS block and set FS base (existing arch_prctl path) so
      thread-locals resolve. Reject PT_INTERP (static only).

B3. The `nyx` std PAL (library/std/src/sys/pal/nyx in the patched rust-src).
    Implement over nyx-api syscalls: alloc(mmap-backed), thread(spawn_thread/clone +
    futex park/unpark), thread_local/TLS(FS base + PT_TLS), time(clock_gettime),
    sync(futex Mutex/Condvar/RwLock), fs(open/read/write/close/seek/stat — EXTEND the
    VFS for seek+stat+readdir; confirm write support), stdio(fd 0/1/2),
    os/args/env(from auxv/argv/envp), process(fork/execve/wait4), net(smoltcp,
    best-effort), random(getrandom), panic = abort.

B4. Toolchain / build integration.
    - New targets/x86_64-unknown-nyx.json: "os":"nyx", static reloc, disable-redzone,
      same SSE feature set as x86_64-nyx.json, panic-strategy:"abort". [DONE]
    - EMPIRICAL FINDING (spike tests/stdhello, build-std=std,panic_abort): core+alloc build
      fine, but std FAILS because on nightly-2026-07-01 the `sys/*` subsystem modules use
      `cfg_select!` with NO fallback arm (only `pal/mod.rs` has `_ => unsupported`). So os="nyx"
      must add an explicit `nyx` (or `_`) arm to EACH of: sys/alloc/mod.rs, sys/io/error/mod.rs,
      sys/thread_local/*, sys/random, sys/fs, sys/net, sys/process, sys/thread, sys/time,
      sys/stdio, sys/pal/mod.rs. The compiler errors enumerate each subsystem's exact contract
      (e.g. io::error needs errno/error_string/decode_error_kind/is_interrupted; random needs
      fill_bytes). This is an error-driven grind: each build reveals the next required symbol.
    - Mechanism: patch the ACTIVE rustup rust-src in place from an in-repo source of truth
      (vendor/nyx-std/) via an idempotent apply script, then -Z build-std=std,panic_abort
      -Z json-target-spec. Pin to nightly-2026-07-01 (rust-toolchain.toml). Reproducibility of
      this patch step is the maintainability crux — wire it into Build.sh + CI.
    - Provide a naked `_start` shim in the PAL that reads the B2 SysV stack (argc/argv/auxv at
      RSP, RSP%16==8), installs TLS from PT_TLS via AT_PHDR + arch_prctl, then calls the Rust
      lang-start. std's own entry can't be used directly (no crt0).

Milestone B: the B-alpha -> B-beta -> B-gamma ladder above, each a real boot test.

--------------------------------------------------------------------------------
Workstream C: qclang on-device   (depends on B-gamma)
--------------------------------------------------------------------------------
Objective: compile .ql -> OpenQASM entirely on Nyx.

Findings: Compiler::compile(&str) -> Result<String, Vec<String>> ALREADY exists
(tools/compiler/src/lib.rs:162); the whole lex -> parse -> semantics -> QIR ->
codegen chain is in-memory, string-in/string-out, with NO process/net/threads on
the compile path. The in-tree and standalone copies are byte-identical.

C1. Slim the compiler to a portable lib.
    - Build qclang_compiler as lib-only (drop both [[bin]]).
    - Prune deps to the compile path: keep logos + thiserror; drop dead
      serde/serde_json/regex/lazy_static/chrono and peripheral
      clap/indicatif/colored/self_update; drop num-complex/rand unless the optional
      simulator.rs is shipped (it needs entropy — leave out initially).
    - Replace the two eprintln! diagnostics (lib.rs:118, lexer.rs:195) with returned
      warnings so the core never touches stderr.

C2. On-device front-end app (a std target_os=nyx binary).
    - New apps/qcstudio (or extend apps/terminal): read a .ql via std::fs, call
      qclang_compiler::Compiler::compile, write .qasm via std::fs, render QASM +
      diagnostics through nyx-gui. First real consumer of Workstream B.
    - Register in Start menu + Build.sh like Workstream A; bundle a sample .ql.

C3. Validation.
    - Compile a bundled sample.ql on-device and byte-compare the emitted .qasm
      against the host qclang output for the same input.

Milestone C: .ql -> .qasm compiled and displayed entirely on Nyx.

--------------------------------------------------------------------------------
BUILD / TEST NOTES (this workstream)
--------------------------------------------------------------------------------
- Image Viewer (A) is a GPU path -> bare-metal only on the Comet Lake test laptop
  (no QEMU), same as the 3D engine. Verify by panel photo.
- std + qclang (B/C) are CPU-only (no GPU), so QEMU is worth trying for fast
  iteration on the SysV/TLS/PAL bring-up; fall back to bare metal if it misbehaves.
- Build everything in WSL Ubuntu (no Rust on the Windows host):
    wsl.exe -- bash -lc 'cd /mnt/c/CODE/Nyx && ./Build.sh'
