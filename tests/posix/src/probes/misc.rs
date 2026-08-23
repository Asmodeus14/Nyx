//! Inert probes (nothing observable changes) plus the presence-only block.

use crate::raw;
use crate::report::{render_ret, Baseline, Row, Verdict};

use super::announce;

pub fn probe(base: Baseline, rows: &mut Vec<Row>) {
    // --- getpid / gettid -------------------------------------------------------------------
    announce("getpid");
    let pid = unsafe { raw::sys0(raw::SYS_GETPID) };
    let mut row = Row::new(39, "getpid");
    row.raw = render_ret(pid);
    row.std_api = format!("{}", std::process::id());
    row.verdict = if base.is_missing(pid) {
        Verdict::Missing
    } else if pid > 0 {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    rows.push(row);

    announce("gettid");
    let tid = unsafe { raw::sys0(raw::SYS_GETTID) };
    let mut row = Row::new(186, "gettid");
    row.raw = render_ret(tid);
    row.verdict = if base.is_missing(tid) {
        Verdict::Missing
    } else if tid > 0 {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    // Nyx gives threads their own pid, so on the main thread these must agree.
    if tid > 0 && pid > 0 && tid != pid {
        row.note = String::from("differs from getpid on the main thread");
    }
    rows.push(row);

    // --- sched_yield -----------------------------------------------------------------------
    announce("sched_yield");
    let r = unsafe { raw::sys0(raw::SYS_SCHED_YIELD) };
    let mut row = Row::new(24, "sched_yield");
    row.raw = render_ret(r);
    row.verdict = classify_simple(base, r);
    rows.push(row);

    // --- clock_gettime: verified by the clock actually advancing ---------------------------
    announce("clock_gettime");
    let mut ts: [i64; 2] = [0, 0];
    let r = unsafe { raw::sys2(raw::SYS_CLOCK_GETTIME, 1, ts.as_mut_ptr() as usize) };
    let first = ts;
    crate::sandbox::sleep_ms(30);
    let mut ts2: [i64; 2] = [0, 0];
    unsafe { raw::sys2(raw::SYS_CLOCK_GETTIME, 1, ts2.as_mut_ptr() as usize) };
    let advanced = ts2[0] > first[0] || (ts2[0] == first[0] && ts2[1] > first[1]);

    let mut row = Row::new(228, "clock_gettime");
    row.raw = render_ret(r);
    row.std_api = String::from("Instant::now");
    row.verdict = if base.is_missing(r) {
        Verdict::Missing
    } else if r < 0 {
        Verdict::Fail
    } else if advanced {
        Verdict::Pass
    } else {
        // Returning success with a frozen clock is exactly the failure this column exists to catch:
        // every timeout in userspace silently becomes infinite.
        Verdict::Stub
    };
    if !advanced && r >= 0 {
        row.note = String::from("CLOCK_MONOTONIC did not advance across a 30ms sleep");
    }
    rows.push(row);

    // --- getrandom: verified by two draws differing ----------------------------------------
    announce("getrandom");
    let mut a = [0u8; 16];
    let mut b = [0u8; 16];
    let r = unsafe { raw::sys3(raw::SYS_GETRANDOM, a.as_mut_ptr() as usize, a.len(), 0) };
    unsafe { raw::sys3(raw::SYS_GETRANDOM, b.as_mut_ptr() as usize, b.len(), 0) };
    let mut row = Row::new(318, "getrandom");
    row.raw = render_ret(r);
    row.verdict = if base.is_missing(r) {
        Verdict::Missing
    } else if r < 0 {
        Verdict::Fail
    } else if a == b || a == [0u8; 16] {
        Verdict::Stub
    } else {
        Verdict::Pass
    };
    if a == b {
        row.note = String::from("two draws returned identical bytes");
    }
    rows.push(row);

    // --- brk / set_tid_address / arch_prctl: stubs today -----------------------------------
    announce("brk");
    let r = unsafe { raw::sys1(raw::SYS_BRK, 0) };
    let mut row = Row::new(12, "brk");
    row.raw = render_ret(r);
    row.verdict = if base.is_missing(r) {
        Verdict::Missing
    } else if r == 0 {
        // brk(0) must report the current break; 0 means it is not tracking one.
        Verdict::Stub
    } else {
        Verdict::Pass
    };
    if r == 0 {
        row.note = String::from("brk(0) returned 0, so no break is tracked");
    }
    rows.push(row);

    announce("arch_prctl");
    // ARCH_GET_FS = 0x1003, into a live local.
    let mut fsbase: u64 = 0;
    let r = unsafe { raw::sys2(raw::SYS_ARCH_PRCTL, 0x1003, &mut fsbase as *mut u64 as usize) };
    let mut row = Row::new(158, "arch_prctl");
    row.raw = render_ret(r);
    row.verdict = if base.is_missing(r) {
        Verdict::Missing
    } else if r < 0 {
        Verdict::Fail
    } else if fsbase != 0 {
        Verdict::Pass
    } else {
        Verdict::Stub
    };
    if r >= 0 && fsbase == 0 {
        row.note = String::from("ARCH_GET_FS reported 0 but TLS is in use");
    }
    rows.push(row);

    // --- EFER / NXE ------------------------------------------------------------------------
    // `rdmsr` is ring 0 only; executing it here raises #GP and kills this process. Nothing in
    // userspace can read EFER, so this row is honest about being unanswerable rather than
    // reporting a fabricated value. The question it was meant to settle — is NX actually
    // enforced? — is answered empirically by the mprotect/NX-exec rows in the mem block.
    let mut row = Row::new(0, "EFER.NXE");
    row.raw = String::from("ring0 only");
    row.verdict = Verdict::Skip;
    row.note = String::from("rdmsr is privileged; see the nx-exec row for the real answer");
    rows.push(row);
}

/// Syscalls reported present/absent, with no attempt to make them do their job.
///
/// Two rules govern this block, and both are here because breaking either takes the **machine**
/// down rather than this process:
///
/// 1. **No pointer argument is ever zero.** Nyx's `is_valid_user_ptr` accepts NULL whenever the
///    length is non-zero, and a kernel-mode page fault panics the box. `futex(NULL, FUTEX_WAIT, …)`
///    is not hypothetical — the kernel's own arm reads `*uaddr` straight after the check. So every
///    pointer slot below gets a slice of one live, zeroed scratch buffer. A presence probe only
///    compares the return against the unknown-syscall baseline, so the arguments' *meaning* is
///    irrelevant; only their safety is.
/// 2. **`clone` and `execveat` are not called at all.** `clone(0, NULL, …)` is legal Linux for
///    "fork me", and an `execveat` that half-replaces the image would take the harness with it.
///    Their status is reported as unprobed rather than measured by a call that could succeed.
pub fn probe_presence_only(base: Baseline, rows: &mut Vec<Row>) {
    // One live, zeroed buffer that every pointer argument below points into.
    let mut scratch = [0u8; 256];
    let p = scratch.as_mut_ptr() as usize;

    // futex: FUTEX_WAKE (op 1), never FUTEX_WAIT. Waking a word nothing is parked on scans the run
    // queues and returns 0 — it cannot block, which WAIT on an unowned word certainly would.
    announce("futex");
    let mut word: u32 = 0;
    let r = unsafe { raw::sys3(raw::SYS_FUTEX, &mut word as *mut u32 as usize, 1, 1) };
    let mut row = Row::new(202, "futex");
    row.raw = render_ret(r);
    row.verdict = if base.is_missing(r) { Verdict::Missing } else { Verdict::Pass };
    row.note = String::from("FUTEX_WAKE on an unwaited word; std's mutexes already ride this");
    rows.push(row);

    // Signals. A zeroed `sigaction` is SIG_DFL with an empty mask — installing it on SIGUSR1 is the
    // state SIGUSR1 is already in, so this asks the question without changing anything.
    for (num, name, a1, a2, a3, a4) in [
        (raw::SYS_RT_SIGACTION, "rt_sigaction", 10usize, p, p + 64, 8usize),
        (raw::SYS_RT_SIGPROCMASK, "rt_sigprocmask", 0, p, p + 64, 8),
        (raw::SYS_SIGALTSTACK, "sigaltstack", p + 128, p + 160, 0, 0),
        (raw::SYS_SET_TID_ADDRESS, "set_tid_address", p + 192, 0, 0, 0),
    ] {
        announce(name);
        let r = unsafe { raw::sys4(num, a1, a2, a3, a4) };
        let mut row = Row::new(num as u32, name);
        row.raw = render_ret(r);
        row.verdict = if base.is_missing(r) { Verdict::Missing } else { Verdict::Stub };
        row.note = if base.is_missing(r) {
            String::new()
        } else {
            String::from("a fixed return value with no mechanism behind it")
        };
        rows.push(row);
    }

    for (num, name) in [(raw::SYS_CLONE, "clone"), (raw::SYS_EXECVEAT, "execveat")] {
        let mut row = Row::new(num as u32, name);
        row.raw = String::from("not called");
        row.verdict = Verdict::Skip;
        row.note = String::from("task-creating / image-replacing: too dangerous to blind-probe");
        rows.push(row);
    }

    // (A hardcoded `signal delivery = Stub` row used to sit here, asserting there was no pending
    // mask, no delivery path and no rt_sigreturn. All three now exist, so it had become a fixed
    // verdict contradicting the MEASURED rows in the same report — `signals::probe` raises a real
    // SIGUSR1 and watches the handler run, and `signals::probe_eintr` breaks a blocking poll with
    // one. A harness that states conclusions it does not measure is the same failure that let the
    // path-syscall ABI bug pass 39 rows; deleted rather than updated, since the real rows say it.)
}

fn classify_simple(base: Baseline, ret: isize) -> Verdict {
    if base.is_missing(ret) {
        Verdict::Missing
    } else if ret < 0 {
        Verdict::Fail
    } else {
        Verdict::Pass
    }
}
