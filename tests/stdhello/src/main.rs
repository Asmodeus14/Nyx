// B-alpha spike: the smallest possible `std` binary for target_os = "nyx".
//
// `restricted_std` is rustc's required opt-in for a std built for an out-of-tree target it doesn't
// recognise — the nyx PAL is injected into rust-src by vendor/nyx-std/apply.py.
#![feature(restricted_std)]

// ELF entry. The Nyx kernel (B2) hands us a SysV stack: [argc][argv..][NULL][envp..][NULL][auxv..]
// with RSP % 16 == 8. We forward argc/argv to the rustc-generated C `main` (which runs std's
// lang_start → real runtime init), then exit_group(231) with main's return value.
core::arch::global_asm!(
    ".global _start",
    "_start:",
    // B-β.2b: install the main thread's native (fs:-relative) TLS block BEFORE anything touches a
    // #[thread_local]. rt::init reads thread::current_id() (a #[thread_local]) before sys::init, so
    // this must happen here in _start, not in the PAL's init(). Pass the untouched entry RSP (points
    // at argc) so the setup can walk auxv for the PT_TLS phdr. It preserves no caller regs we need.
    "mov rbx, rsp",         // stash entry RSP (callee-saved across the call) for argc/argv below
    "mov rdi, rsp",         // arg0 = &argc (the SysV info block)
    "and rsp, -16",         // 16-align for the call
    "call __nyx_setup_main_tls",
    "mov rdi, [rbx]",       // argc
    "lea rsi, [rbx + 8]",   // argv
    "and rsp, -16",         // re-16-align (rbx still holds the entry stack; no args on stack)
    "call main",            // rustc-generated `int main(int, char**)` → drives std lang_start
    "mov edi, eax",         // propagate the process exit code
    "mov eax, 231",         // SYS_exit_group
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

fn main() {
    println!("hello from std on nyx");
    eprintln!("stderr works too");

    // B-alpha fs milestone: open + read a bundled file through real std::fs.
    let path = "/mnt/nvme/apps/StdHello.nyx/hello.txt";
    match std::fs::read_to_string(path) {
        Ok(s) => {
            println!("fs: read {} bytes from {}", s.len(), path);
            // Print the file's own contents so the boot-log proves the bytes round-tripped.
            for line in s.lines() {
                println!("fs> {line}");
            }
        }
        Err(e) => println!("fs: FAILED to read {path}: {e}"),
    }

    // B-β.1 milestone: sync + time on the main thread (no spawn yet — that's B-β.2).
    // Exercises the futex(202) Mutex backend, clock_gettime(228) Instant, and nanosleep(35).
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    let m = Mutex::new(0u64);
    {
        let mut g = m.lock().unwrap();
        *g += 41;
        *g += 1;
    }
    println!("sync: Mutex guarded value = {}", *m.lock().unwrap());

    let t0 = Instant::now();
    std::thread::sleep(Duration::from_millis(50));
    let elapsed = t0.elapsed();
    println!("time: slept ~50ms, Instant measured {} ms", elapsed.as_millis());
    // Sanity: monotonic clock advanced and sleep didn't return early.
    if elapsed >= Duration::from_millis(40) {
        println!("time: OK (monotonic clock + nanosleep working)");
    } else {
        println!("time: WARN slept too short ({} ms)", elapsed.as_millis());
    }

    // B-β.2b milestone: native `#[thread_local]` storage (fs:-relative), set up by pal::tls in
    // _start. This exercises a real TLS read/write on the main thread — the whole point of the
    // TLS flip. A wrong FS base or bad variant-II layout would fault or read garbage here.
    use std::cell::Cell;
    thread_local! {
        static TLS_COUNTER: Cell<u32> = Cell::new(100);
    }
    TLS_COUNTER.with(|c| {
        let before = c.get();
        c.set(before + 23);
        println!("tls: thread_local read {} -> wrote {}", before, c.get());
    });
    // DIAG (integer Display bug): the same small value at each width. On bare metal, usize/u64/u128
    // rendered as garbage while u32 printed fine — but on a 64-bit target u32 and u64 share
    // display_u64, so this pins down exactly which width(s) break and whether hex differs.
    println!("diag: u8={} u16={} u32={} u64={} usize={} u128={}", 7u8, 7u16, 7u32, 7u64, 7usize, 7u128);
    println!("diag2: u64 dec={} hex={:#x} big={}", 42u64, 42u64, 1234567u64);

    // DIAG3 — ground truth: format a string with a hex prefix, then dump its RAW BYTES as decimal
    // numbers (which we've confirmed render reliably). Whatever shows as tofu, this reveals the
    // actual byte the writer received. Expected for "0x2a": 48 120 50 97  (i.e. '0' 'x' '2' 'a').
    let probe = format!("{:#x}", 42u64);
    print!("diag3: hexbytes=");
    for b in probe.as_bytes() {
        print!("{} ", *b as u32);
    }
    println!();
    // Also dump the byte length std thinks it produced, and re-print via a single write of a
    // pre-built &str (bypasses per-fragment writes) to compare against the interpolated path.
    println!("diag4: len={} oneshot=[{}]", probe.len(), probe);

    let final_val = TLS_COUNTER.with(|c| c.get());
    if final_val == 123 {
        println!("tls: OK (native #[thread_local] read/write working)");
    } else {
        println!("tls: FAIL expected 123, got {}", final_val);
    }

    // B-β.2c milestone: real thread::spawn + shared Mutex + join. Two workers each add to a shared
    // counter; per-thread TLS must isolate their thread_local counters from the main thread's 123.
    use std::sync::{Arc, Mutex as StdMutex};
    let total = Arc::new(StdMutex::new(0u32));
    let mut handles = Vec::new();
    for worker in 0..2u32 {
        let total = Arc::clone(&total);
        handles.push(std::thread::spawn(move || {
            // Each worker's TLS_COUNTER starts fresh at 100 (per-thread!), NOT the main thread's 123.
            let tls_start = TLS_COUNTER.with(|c| c.get());
            TLS_COUNTER.with(|c| c.set(tls_start + worker));
            for _ in 0..1000 {
                *total.lock().unwrap() += 1;
            }
            println!("spawn: worker {} done (tls started at {})", worker, tls_start);
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    let sum = *total.lock().unwrap();
    let main_tls = TLS_COUNTER.with(|c| c.get());
    if sum == 2000 && main_tls == 123 {
        println!("spawn: OK (2 threads joined, mutex total=2000, main tls intact)");
    } else {
        println!("spawn: FAIL total={} main_tls={}", sum, main_tls);
    }
}
