# B-β.2 + B-γ Plan — real threads, then qclang-unlock

User chose **threads first, then B-γ**. Both land as a sequence of boot-verified sub-stages
(matches the established "boot-test every milestone" pattern). Each sub-stage is one build.

## Decisive facts found during exploration
- Scheduler (`scheduler.rs:61-157`) swaps kernel-stack / TSS / CR3 on context switch but **never
  saves/restores the FS register**. Native per-thread `thread_local!` needs FS-base TLS, so this is
  the one *universal-path* kernel change (risk-gated by a no-regression boot before any std flip).
- Exit (`interrupts.rs:1101`, `60 | 231`) calls `clear_user_address_space(cr3)`. Threads **share
  cr3**, so a worker exiting would wipe the whole process. Fix: only clear when this task is the
  **sole owner** of its cr3; otherwise it's a thread exit → skip the clear.
- `spawn_thread(58)` zeroes all GP regs and passes **no argument** to the entry. So the worker needs
  a **trampoline** that reads its args off its own initial stack (which we build).
- std's `thread::current()` / `set_current` (`thread/lifecycle.rs:121`) **aborts if CURRENT is
  already set**. In the `no_threads` TLS arm (where nyx sits for B-α) CURRENT is a single global →
  the 2nd thread would abort. Real threads therefore require **native FS-base TLS**.
- **Key simplification:** the trampoline sets its own FS via `arch_prctl(158)` in userspace before
  touching any thread-local, so `spawn_thread` needs **no** kernel FS change — only the scheduler's
  save/restore does.

---

## B-β.2a — Kernel: FS save/restore + thread-safe exit  (build #1, no-regression gate)
Kernel-only. No std change yet; the goal is to prove the universal-path edits don't break the
working B-α/β.1 boot.
1. `process.rs`: add `saved_fs_base: u64` to `Process` + all 3 initializers (`new`, `new_thread`,
   and the struct literal). Init 0.
2. `scheduler.rs schedule()`:
   - Step 2 (save): `current_process.saved_fs_base = FsBase::read().as_u64();`
   - Step 5 (restore): after the CR3 swap, `FsBase::write(VirtAddr::new(next.saved_fs_base));`
     (only meaningful once a task sets FS; 0 is harmless for the no_std apps that never use FS.)
3. `interrupts.rs` exit arm `60 | 231`: before `clear_user_address_space`, count tasks across all
   cores sharing `task.cr3`. If **>1** (sibling threads alive) → skip the clear (thread exit), mark
   this slot `Empty` (self-reap; kernel-stack leak is consistent with the existing process-exit
   path), then the usual `hlt` loop. If **sole owner** → current behavior (shred + Zombie) unchanged.
**Boot test:** desktop + existing apps + Std Hello still run exactly as before (FS restore is a
no-op for single-thread; exit still shreds for sole-owner processes). Pure regression check.

## B-β.2b — Native TLS for the main thread  (build #2, the scary flip, isolated)
Flip nyx from the `no_threads` TLS arm to native `#[thread_local]` (FS-base) TLS, and set up the
**main thread's** TLS block before any thread-local is touched.
1. `targets/x86_64-unknown-nyx.json`: add `"has-thread-local": true`.
2. `apply.py`: move nyx **out** of the `thread_local/mod.rs` `no_threads` arm (so it falls to the
   `target_thread_local` native arm). Keep nyx in the **no-op `guard::enable`** arm (TLS-destructors
   just leak — acceptable for the milestone; revisit in B-γ if needed).
3. PAL: add `pub unsafe extern "C" fn __nyx_setup_main_tls(stack_at_entry: *const usize)`. It walks
   the SysV stack (argc/argv/envp/auxv) for `AT_PHDR/AT_PHNUM/AT_PHENT`, finds the `PT_TLS` phdr,
   `mmap`s a variant-II TLS block (memsz+align, copy filesz init image, zero .tbss, TCB self-pointer
   at the block end), and `arch_prctl(SET_FS, tcb)`. Uses only raw syscalls — no TLS, no alloc.
4. `tests/stdhello/src/main.rs` `_start`: capture RSP at entry, `call __nyx_setup_main_tls`, then
   proceed to `call main` as today. (This is the generalizable entry shim — later every nyx std
   binary uses it.)
5. Add a `thread_local!` to the test that reads/writes a value on the main thread.
**Boot test:** Std Hello still prints all the B-α/β.1 lines **and** the new `thread_local!` line.
Proves native TLS works for the main thread without spawning. If it faults, the fault is isolated to
the TLS flip (no spawn code in play yet).

## B-β.2c — thread::spawn / join  (build #3, the payoff)
1. PAL `sys/thread_nyx.rs` `Thread::new(stack, init)`:
   - `mmap` a worker stack (`stack` bytes) and a worker TLS block (same variant-II setup as the main
     thread, from the cached PT_TLS template).
   - Build the worker's initial stack: a small record the trampoline reads — `{ tls_ptr,
     ThreadInit* , completion: *AtomicU32 }`.
   - `extern "C"` **trampoline** (entry point handed to `spawn_thread`): `arch_prctl(SET_FS, tls_ptr)`
     → `ThreadInit::init()` → run the returned closure → store `completion=1` + `futex_wake` →
     `exit(60)` (thread-exit path from 2a; never returns).
   - `spawn_thread(58, trampoline, worker_stack_top)`; store the completion `Arc<AtomicU32>` +
     child tid in the returned `Thread`.
2. `Thread::join(self)`: `futex_wait` on the completion word until it reads 1 (works regardless of
   which thread joins — no wait4 parent-restriction).
3. `available_parallelism`: report `sys_get_active_cores()` (syscall already exists) instead of 1.
4. Test: spawn 2 workers, each locks a shared `Arc<Mutex<u64>>` and adds; `join` both; assert total.
**Boot test:** boot-log shows both workers ran, the joined total is correct, exit 0. Real
`thread::spawn` + `Mutex` sharing + `join` on bare metal. Completes **B-β**.

---

## B-γ — HashMap + std::process + fs write  (unlocks qclang; after B-β)
Sequenced after threads per the user's choice. Sub-stages, each a boot test:
- **B-γ.1 HashMap:** likely already works (getrandom(318) seeds RandomState). Just add a HashMap
  exercise to the test and confirm — no new PAL expected.
- **B-γ.2 fs write:** kernel `OpenFile::write` is currently a stub (`vfs.rs:278 returns 0`) and open
  has no create/flags. Add write-through in the VFS driver + an `open` create path; PAL `fs_nyx.rs`
  gains real `File::write`/`OpenOptions(write/create/truncate)`. Test: write a file, read it back.
- **B-γ.3 std::process:** map `Command::spawn`/`wait` onto `fork(57)`+`execve(59)`+`wait4(61)`
  (all exist). PAL `process` module. Test: spawn a child .nyx binary, wait, check exit status.
- Then **C1** (slim qclang to a lib) → **C2** (`apps/qcstudio` std front-end) → **C3** (on-device
  `.ql`→`.qasm` byte-matches host qclang).

## Build discipline (locked-in gotcha)
Every build: `rm -rf tests/stdhello/target` before `./Build.sh`, because `cargo -Z build-std`
fingerprints libstd by rustc-version+target, **not** rust-src contents — stale libstd otherwise
links the pre-patch PAL. Symptom: `Finished in <1s` with no `Compiling std`.
