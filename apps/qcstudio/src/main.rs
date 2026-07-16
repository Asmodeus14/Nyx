// Workstream C (C2): qcstudio — the first real consumer of the Nyx std port.
//
// A `target_os = "nyx"` std binary that compiles a QCLang (.ql) source to OpenQASM entirely
// on-device: read source via std::fs, run it through the PORTABLE qclang core, write the .qasm
// via std::fs, and echo QASM + diagnostics to the boot log. Headless (unblocks C3 byte-match
// validation vs host qclang); a nyx-gui front-end can layer on later.
//
// `restricted_std` is rustc's required opt-in for a std built for an out-of-tree target it doesn't
// recognise — the nyx PAL is injected into rust-src by vendor/nyx-std/apply.py.
#![feature(restricted_std)]

// ELF entry — identical contract to tests/stdhello: the kernel hands us a SysV stack
// [argc][argv..][NULL][envp..][NULL][auxv..] with RSP % 16 == 8. Install the main thread's
// native (fs:-relative) TLS block BEFORE anything touches a #[thread_local] (rt::init reads
// thread::current_id() before sys::init), then forward argc/argv to the rustc-generated C `main`
// and exit_group(231) with its return value.
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, rsp",         // stash entry RSP (callee-saved across the call)
    "mov rdi, rsp",         // arg0 = &argc (the SysV info block) for TLS auxv walk
    "and rsp, -16",
    "call __nyx_setup_main_tls",
    "mov rdi, [rbx]",       // argc
    "lea rsi, [rbx + 8]",   // argv
    "and rsp, -16",
    "call main",            // rustc-generated `int main(int, char**)` → std lang_start
    "mov edi, eax",         // propagate exit code
    "mov eax, 231",         // SYS_exit_group
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

use qclang_compiler::Compiler;

// Bundled by Build.sh into the app dir; a self-contained Bell-pair circuit so the round-trip has
// real gates/measurements to emit (see apps/qcstudio/samples/sample.ql).
const SRC_PATH: &str = "/mnt/nvme/apps/QcStudio.nyx/sample.ql";
const OUT_PATH: &str = "/mnt/nvme/apps/QcStudio.nyx/out.qasm";
// C3 reference: the QASM host qclang emits for sample.ql (bundled by Build.sh). qcstudio
// byte-compares its own on-device output against this so the round-trip is a real pass/fail.
const REF_PATH: &str = "/mnt/nvme/apps/QcStudio.nyx/expected.qasm";

fn main() {
    println!("qcstudio: on-device QCLang -> OpenQASM (Workstream C)");

    // 1. READ the .ql source through real std::fs (the B-alpha fs path).
    let source = match std::fs::read_to_string(SRC_PATH) {
        Ok(s) => {
            println!("qcstudio: read {} bytes from {}", s.len(), SRC_PATH);
            s
        }
        Err(e) => {
            println!("qcstudio: FAIL read {SRC_PATH}: {e}");
            std::process::exit(1);
        }
    };

    // 2. COMPILE with the portable core (logos + thiserror only; no stderr on this path).
    let qasm = match Compiler::compile(&source) {
        Ok(q) => q,
        Err(errors) => {
            println!("qcstudio: FAIL compile ({} error(s)):", errors.len());
            for e in &errors {
                println!("  - {e}");
            }
            std::process::exit(2);
        }
    };
    println!("qcstudio: compiled OK, {} bytes of QASM", qasm.len());

    // 3. WRITE the emitted QASM back to the VFS (the B-γ.2 fs-write path).
    match std::fs::write(OUT_PATH, qasm.as_bytes()) {
        Ok(()) => println!("qcstudio: wrote {} -> {} bytes", OUT_PATH, qasm.len()),
        Err(e) => {
            println!("qcstudio: FAIL write {OUT_PATH}: {e}");
            std::process::exit(3);
        }
    }

    // 4. ECHO the QASM to the boot log so the round-trip is visible on-device (and so C3 can eyeball
    //    it next to the file). Prefix each line so it's greppable in the serial dump.
    println!("qcstudio: --- BEGIN QASM ---");
    for line in qasm.lines() {
        println!("qasm> {line}");
    }
    println!("qcstudio: --- END QASM ---");

    // 5. C3 VALIDATION: byte-compare against the bundled host-qclang reference. This makes the
    //    on-device round-trip a real pass/fail milestone (".ql -> .qasm byte-matches host qclang").
    //    A missing reference is non-fatal — C2 (the round-trip itself) already succeeded above.
    match std::fs::read_to_string(REF_PATH) {
        Ok(reference) => {
            if qasm == reference {
                println!("qcstudio: C3 PASS — on-device QASM byte-matches host qclang ({} bytes)", qasm.len());
            } else {
                println!(
                    "qcstudio: C3 FAIL — output differs from host reference (got {} bytes, expected {} bytes)",
                    qasm.len(),
                    reference.len()
                );
                std::process::exit(4);
            }
        }
        Err(e) => println!("qcstudio: C3 SKIP — no reference at {REF_PATH}: {e}"),
    }

    println!("qcstudio: ALL DONE (C2: .ql -> .qasm on-device)");
}
