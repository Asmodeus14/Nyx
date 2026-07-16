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
    // DIAG (kept for the open tofu/int-Display issue; harmless): same value at each width + hex.
    println!("diag: u8={} u16={} u32={} u64={} usize={} u128={}", 7u8, 7u16, 7u32, 7u64, 7usize, 7u128);
    println!("diag2: u64 dec={} hex={:#x} big={}", 42u64, 42u64, 1234567u64);

    // FMT SELF-TEST (tofu-proof): compare format! output against expected literals IN CODE.
    // If these mismatch, integer formatting corrupts bytes IN MEMORY (explains hashmap key
    // collisions + all tofu). If all OK, corruption is downstream (ring buffer / viewer font).
    // BAD lines encode digits as letters (A=0,B=1,..,J=9) so no digit rendering is involved.
    let cases: [(u64, &str); 10] = [
        (0, "0"), (1, "1"), (7, "7"), (42, "42"), (61, "61"),
        (100, "100"), (102, "102"), (166, "166"), (1234567, "1234567"), (2000, "2000"),
    ];
    let mut fmt_ok = true;
    for (v, want) in cases {
        let got = format!("{}", v);
        let got_us = format!("{}", v as usize);
        let got_u32 = format!("{}", v as u32);
        if got != want || got_us != want || got_u32 != want {
            fmt_ok = false;
            let enc = |s: &str| -> String {
                s.bytes()
                    .map(|b| if (48..=57).contains(&b) { (b'A' + (b - 48)) as char } else { '?' })
                    .collect()
            };
            println!("fmt: BAD want={} got64={} got_usize={} got_u32={}",
                enc(want), enc(&got), enc(&got_us), enc(&got_u32));
        }
    }
    println!("fmt: {}", if fmt_ok { "OK all format! round-trips match in-memory" } else { "BAD see lines above (A=0..J=9)" });

    // B-γ.1 milestone: HashMap (exercises getrandom-seeded RandomState + heap churn).
    // Failure reporting uses u32 casts only — u64/usize interpolation renders as tofu in the
    // boot-log (open issue), and the FIRST failed run hid hits/len behind rectangles.
    use std::collections::HashMap;
    let mut map: HashMap<String, u32> = HashMap::new();

    // Stage 1: small map, no growth (initial capacity fits a few entries).
    map.insert(String::from("alpha"), 1);
    map.insert(String::from("beta"), 2);
    let s1 = map.get("alpha") == Some(&1) && map.get("beta") == Some(&2) && map.len() == 2;
    println!("hashmap.s1 small: {}", if s1 { "OK" } else { "FAIL" });

    // Stage 2a: integer keys ONLY (no format! involved) — separates HashMap-machinery bugs from
    // string-key corruption. 100 entries forces growth just like the string stage.
    let mut imap: HashMap<u32, u32> = HashMap::new();
    for i in 0..100u32 {
        imap.insert(i, i * 7);
    }
    let mut ihits: u32 = 0;
    for i in 0..100u32 {
        if imap.get(&i) == Some(&(i * 7)) {
            ihits += 1;
        }
    }
    println!("hashmap.s2a int-keys: {}", if ihits == 100 && imap.len() == 100 { "OK" } else { "FAIL" });

    // Stage 2: force table growth with 100 string keys.
    for i in 0..100u32 {
        map.insert(format!("key{}", i), i * 3);
    }
    let len_after = map.len() as u32; // 102 expected
    println!("hashmap.s2 len after inserts (want 102): {}", len_after);

    // Stage 3: full lookup sweep; report first missing key index (u32) instead of a bare count.
    let mut hits: u32 = 0;
    let mut first_bad: u32 = u32::MAX;
    for i in 0..100u32 {
        if map.get(&format!("key{}", i)) == Some(&(i * 3)) {
            hits += 1;
        } else if first_bad == u32::MAX {
            first_bad = i;
        }
    }
    if hits == 100 && len_after == 102 && s1 {
        println!("hashmap: OK (100 insert/lookup round-trips)");
    } else {
        println!("hashmap: FAIL hits={} first_bad_key={}", hits, first_bad);
        // Probe the first bad key's raw presence to split "missing" from "wrong value".
        if first_bad != u32::MAX {
            let k = format!("key{}", first_bad);
            match map.get(&k) {
                Some(v) => println!("hashmap: first_bad present but value={} (want {})", *v, first_bad * 3),
                None => println!("hashmap: first_bad key absent from map"),
            }
        }
    }

    // B-γ.2 milestone: fs WRITE — create a file, write, read it back, compare.
    let wpath = "/mnt/nvme/apps/StdHello.nyx/written.txt";
    let payload = "written by std::fs on nyx\nsecond line to prove offsets advance\n";
    match std::fs::write(wpath, payload) {
        Ok(()) => match std::fs::read_to_string(wpath) {
            Ok(back) if back == payload => {
                println!("fswrite: OK (wrote {} bytes, read back identical)", payload.len())
            }
            Ok(back) => println!("fswrite: FAIL read-back mismatch ({} vs {} bytes)", back.len(), payload.len()),
            Err(e) => println!("fswrite: FAIL read-back: {e}"),
        },
        Err(e) => println!("fswrite: FAIL write: {e}"),
    }

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

    // B-γ.3 milestone: std::process — fork(57)+execve(59) a real std child, wait4(61) it, decode the
    // exit code. The child prints one line (inherited stdio) and exits 42. (The exit(42) itself was
    // the last bug: libstd's sys::exit had no nyx arm and fell through to abort()/ud2 — now fixed.)
    use std::process::Command;
    match Command::new("/mnt/nvme/apps/StdChild.nyx/run.bin").spawn() {
        Ok(mut child) => {
            let child_pid = child.id();
            match child.wait() {
                Ok(status) => {
                    if status.code() == Some(42) {
                        println!("process: OK (spawned pid {}, waited, exit code 42)", child_pid);
                    } else {
                        println!("process: FAIL wrong exit code {:?} (want 42)", status.code());
                    }
                }
                Err(e) => println!("process: FAIL wait: {e}"),
            }
        }
        Err(e) => println!("process: FAIL spawn: {e}"),
    }
    println!("stdhello: ALL DONE (B-γ complete: HashMap + fs write + std::process)");
}
