//! Per-CPU scheduler instrumentation.
//!
//! ## Why this exists
//!
//! Nothing in this kernel could previously answer "how long is a tick", "how long did that task
//! wait to run", or "how long was this core unable to take an interrupt". `CONTEXT_SWITCHES`
//! (scheduler.rs) was the entire budget: one global cumulative counter, exposed by syscall 523,
//! read by nobody. Every performance claim about the scheduler was therefore unfalsifiable.
//!
//! ## Why it is cheap
//!
//! Every field lives in `PerCpu` and is written **only by the core that owns it**, from contexts
//! that already run with interrupts masked (the timer ISR, the yield trampoline, the syscall
//! dispatcher). So they are plain `u64`/`u32` — no atomics, no locks, no contention, and no
//! cache-line ping-pong between cores. The struct is `align(64)` so a neighbouring field cannot
//! share a line with another core's data.
//!
//! This matters: the two counters that already existed, `UPTIME_MS` and `CONTEXT_SWITCHES`, are
//! global atomics bumped on *every core's* every tick. They are themselves a measurable SMP cost,
//! and adding more of that shape would have made the instrumentation a source of the very overhead
//! it is meant to measure.
//!
//! ## Measuring interrupts-off time without instrumenting cli/sti
//!
//! ★ The headline number here is `max_gap_tsc`, and it is obtained almost for free. The APIC timer
//! is *periodic*: it fires at a fixed cadence regardless of what the CPU is doing. So the interval
//! between consecutive timer interrupts on a core is exactly one tick period — **unless** that core
//! spent part of the interval unable to take an interrupt, in which case the gap stretches.
//!
//! One `rdtsc` per tick therefore measures the longest contiguous window in which this core was not
//! interruptible, with no need to wrap every `cli`/`sti` in the kernel. Pairing it with
//! `cur_syscall` — which the dispatcher sets on entry — attributes that window to whichever syscall
//! was executing, which turns "the machine stutters" into "syscall 503 masked interrupts for 40 ms".
//!
//! Everything here compiles out under `--no-default-features` / without the `sched_stats` feature;
//! the accessors become empty inline functions and the hot paths lose even the `rdtsc`.

/// How many long interrupts-off windows to retain. A recurring offender shows up repeatedly in a
/// short ring; a one-off (boot, `execve`) appears once and is pushed out.
pub const GAP_RING_LEN: usize = 16;

/// One observed interrupts-off window.
///
/// ★ Retaining only the single worst gap was not enough to act on. The histogram said there were
/// ~57 gaps of 13-27 ms and ~41 of 6-13 ms — a stall every few hundred milliseconds, which is the
/// cadence a person perceives as a hitch — but `max_gap_syscall` described exactly one of them, and
/// it happened to be the boot-time `execve` that is irrelevant to steady state. A short history
/// lets the *repeating* cause name itself instead of being guessed at.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GapSample {
    /// Length of the window in TSC cycles.
    pub cycles: u64,
    /// Syscall blamed. See `flags` for whether this is "during" or "just after".
    pub syscall: u64,
    /// `UPTIME_MS` when observed, so boot-time samples can be told from steady-state ones.
    pub at_ms: u64,
    /// Bit 0: the core was still inside `syscall` when the gap was seen. Clear means the syscall had
    /// already returned — which is the normal case for a syscall-caused stall, because `sysretq`
    /// restores IF before the suppressed tick is delivered.
    pub flags: u64,
}

/// Distinct syscalls tracked in the per-syscall stall tally. Small on purpose: the offenders are
/// always a handful, and the aim is a one-line answer rather than a complete census.
pub const GAP_TALLY_LEN: usize = 8;

/// Cumulative stall tally for one syscall.
///
/// ★ The 16-entry ring shows the most RECENT stalls, which answers "what is stalling now" but not
/// "what stalls most" — and reading it off a laptop screen means transcribing sixteen lines by
/// hand. This is the cumulative version: it survives past 16 samples and collapses to a single
/// line per offender, so a report can be read at a glance instead of copied out.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GapTally {
    /// Syscall number, or `u64::MAX` for "not in a syscall" / unused slot.
    pub syscall: u64,
    pub count: u64,
    /// Summed TSC cycles, so a mean can be shown — a rare 40 ms stall and a frequent 6 ms one need
    /// telling apart, and a count alone cannot.
    pub total_cycles: u64,
}

/// Number of power-of-two buckets in each histogram. Bucket `n` holds samples whose TSC-cycle count
/// has its highest set bit at position `n`, i.e. samples in `[2^n, 2^(n+1))`. 32 buckets covers up
/// to ~4 billion cycles (seconds), which is far past anything we expect to see and cheap enough
/// that clamping the tail is never interesting.
pub const HIST_BUCKETS: usize = 32;

/// Per-CPU scheduler statistics. See the module docs for why these are not atomics.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
pub struct SchedStats {
    // ── invocation counters ────────────────────────────────────────────────
    /// Times `Scheduler::schedule` ran on this core.
    pub schedule_calls: u64,
    /// Times it actually changed the running task. The difference between this and
    /// `schedule_calls` is pure overhead: a full interrupt entry, FXSAVE and FXRSTOR spent to
    /// decide to keep running the same thing.
    pub switches: u64,
    /// Switches entered through `int 0x41` — a task gave the CPU up.
    pub voluntary: u64,
    /// Switches entered through the timer — a task was preempted.
    pub involuntary: u64,
    /// Times this core selected its idle task (i.e. found no real work).
    pub idle_entries: u64,
    /// Tasks moved out of `Blocked` into `Ready` by this core.
    pub wakeups: u64,
    /// Tasks this core handed to a different core.
    pub migrations: u64,
    /// Timer interrupts taken by this core. Divided by uptime this is the *real* tick rate, which
    /// is the check on whether `APIC_TICKS_PER_MS` calibration agrees with reality.
    pub ticks: u64,

    // ── last-sample gauges ─────────────────────────────────────────────────
    /// Runnable (Ready or Running, non-idle) tasks seen on the last pass.
    pub rq_len: u64,
    /// Live entries in this core's task table on the last pass.
    pub tasks_live: u64,
    /// Zombie/Empty tombstones. These are never reclaimed today, so this only grows — and every
    /// per-tick O(n) scan walks them. Watching this climb is the evidence for that being worth
    /// fixing.
    pub tasks_dead: u64,

    // ── cost of scheduling itself ──────────────────────────────────────────
    /// Total TSC cycles spent inside `schedule()`. Against `ticks` and the TSC rate this gives the
    /// percentage of the machine spent deciding what to run.
    pub sched_cycles: u64,

    // ── interrupt-latency proxy (see module docs) ──────────────────────────
    /// TSC at the last timer interrupt, for differencing. 0 = no tick seen yet.
    pub last_tick_tsc: u64,
    /// Longest observed interval between consecutive timer interrupts, in TSC cycles.
    pub max_gap_tsc: u64,
    /// The syscall that was executing when `max_gap_tsc` was observed, or `u64::MAX` if the core
    /// was not inside a syscall. This is the attribution that makes the number actionable.
    pub max_gap_syscall: u64,
    /// Syscall currently executing on this core; `u64::MAX` when not in one.
    pub cur_syscall: u64,
    /// Histogram of tick-to-tick intervals.
    pub gap_hist: [u32; HIST_BUCKETS],

    // ── wakeup latency ─────────────────────────────────────────────────────
    /// Histogram of TSC cycles between a task being made `Ready` and actually being picked to run.
    pub wake_hist: [u32; HIST_BUCKETS],
    /// Worst wake-to-run delay seen, in TSC cycles.
    pub max_wake_tsc: u64,

    // ── syscall duration ───────────────────────────────────────────────────
    /// Histogram of syscall durations in TSC cycles.
    pub sys_hist: [u32; HIST_BUCKETS],
    /// TSC at syscall entry, for differencing. 0 = not in a syscall.
    pub sys_entry_tsc: u64,

    /// Longest single syscall seen on this core, in TSC cycles, and which one it was.
    ///
    /// ★★ This exists because `max_gap_syscall` **cannot** attribute the case it was built for.
    /// `SYSCALL` masks interrupts via SFMASK and `sysretq` restores IF from R11 — so a timer
    /// interrupt suppressed during a long syscall is not delivered until *after* the syscall has
    /// returned, at which point `cur_syscall` has already been reset. The gap is real, but it is
    /// always reported as "not in a syscall".
    ///
    /// Measured inside the syscall, this attribution cannot be fooled: it is the direct answer to
    /// "which syscall holds the CPU longest with interrupts off".
    pub max_sys_cycles: u64,
    pub max_sys_id: u64,
    /// The last syscall this core executed, retained after exit. A weaker hint than the above, but
    /// it is what a tick-gap observed immediately after a syscall should be blamed on.
    pub last_syscall: u64,

    // ── history of long interrupts-off windows ─────────────────────────────
    /// Cycle count above which a gap is worth remembering. Derived at runtime from the measured
    /// tick period and TSC rate — a fixed constant would be wrong, since "too long" depends on both.
    /// 0 = not yet computed.
    pub gap_threshold: u64,
    /// Ring of the most recent long gaps. See [`GapSample`].
    pub gap_ring: [GapSample; GAP_RING_LEN],
    /// Write cursor into `gap_ring`.
    pub gap_ring_head: u64,
    /// Total long gaps ever seen, which may far exceed `GAP_RING_LEN`. The ratio of this to uptime
    /// is the stall *rate*, which matters more than any single sample.
    pub gap_ring_count: u64,
    /// Cumulative stalls grouped by syscall. See [`GapTally`].
    pub gap_tally: [GapTally; GAP_TALLY_LEN],
}

// The other half of the ABI guard in `nyx_api`. The kernel memcpy's these structs straight into a
// user buffer, so a field added here and not there (or vice versa) must break the build rather than
// silently reinterpret every field after it. Both sides assert the same two numbers.
const _: () = assert!(core::mem::size_of::<SchedGlobals>() == 64);
const _: () = assert!(core::mem::size_of::<SchedStats>() == 1280);
const _: () = assert!(core::mem::align_of::<SchedStats>() == 64);

/// Collect the machine-wide facts. Reads only published atomics — no lock, so this cannot join a
/// deadlock cycle from a syscall arm at IF=0 (the same reasoning `SysMetrics` documents).
pub fn globals() -> SchedGlobals {
    use core::sync::atomic::Ordering as O;
    SchedGlobals {
        tick_period_us: crate::apic::TICK_PERIOD_US.load(O::Relaxed),
        apic_ticks_per_ms: crate::apic::APIC_TICKS_PER_MS.load(O::Relaxed),
        tsc_mhz: crate::time::TSC_MHZ.load(O::Relaxed),
        uptime_ms: crate::time::UPTIME_MS.load(O::Relaxed),
        active_cores: crate::smp::ACTIVE_CORES.load(O::Relaxed) as u64,
        context_switches: crate::scheduler::CONTEXT_SWITCHES.load(O::Relaxed),
        timer_initial_count: crate::apic::timer_initial_count() as u64,
        stats_enabled: if cfg!(feature = "sched_stats") { 1 } else { 0 },
    }
}

impl SchedStats {
    pub const fn new() -> Self {
        Self {
            schedule_calls: 0,
            switches: 0,
            voluntary: 0,
            involuntary: 0,
            idle_entries: 0,
            wakeups: 0,
            migrations: 0,
            ticks: 0,
            rq_len: 0,
            tasks_live: 0,
            tasks_dead: 0,
            sched_cycles: 0,
            last_tick_tsc: 0,
            max_gap_tsc: 0,
            max_gap_syscall: u64::MAX,
            cur_syscall: u64::MAX,
            gap_hist: [0; HIST_BUCKETS],
            wake_hist: [0; HIST_BUCKETS],
            max_wake_tsc: 0,
            sys_hist: [0; HIST_BUCKETS],
            sys_entry_tsc: 0,
            max_sys_cycles: 0,
            max_sys_id: u64::MAX,
            last_syscall: u64::MAX,
            gap_threshold: 0,
            gap_ring: [GapSample { cycles: 0, syscall: u64::MAX, at_ms: 0, flags: 0 };
                       GAP_RING_LEN],
            gap_ring_head: 0,
            gap_ring_count: 0,
            gap_tally: [GapTally { syscall: u64::MAX, count: 0, total_cycles: 0 };
                        GAP_TALLY_LEN],
        }
    }

    /// Index of the power-of-two bucket a cycle count falls in.
    #[inline(always)]
    pub fn bucket(cycles: u64) -> usize {
        // `| 1` so zero maps to bucket 0 rather than underflowing the shift.
        (63 - (cycles | 1).leading_zeros() as usize).min(HIST_BUCKETS - 1)
    }

    /// Lower edge of bucket `i`, in cycles — for rendering the histogram.
    #[inline(always)]
    pub fn bucket_floor(i: usize) -> u64 {
        1u64 << i
    }
}

/// Machine-wide scheduler facts, as handed to userspace by syscall 575 op 0.
///
/// ⚠️ Field order IS the ABI — `nyx_api::SchedGlobals` mirrors it and the two must move together,
/// same contract as `SysMetrics` (syscall 567).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct SchedGlobals {
    /// Measured duration of one scheduler tick, in microseconds. **0 = calibration did not run.**
    /// This is the headline number: the kernel has always assumed 1000.
    pub tick_period_us: u64,
    /// Measured APIC timer ticks per millisecond at divide-by-16.
    pub apic_ticks_per_ms: u64,
    /// Calibrated TSC rate, for converting every cycle count below into time.
    pub tsc_mhz: u64,
    /// `UPTIME_MS` as the kernel believes it. Compare against `ticks / (1000/tick_period_us)` per
    /// core to see how far the kernel's idea of a millisecond is from a real one.
    pub uptime_ms: u64,
    pub active_cores: u64,
    /// The global `CONTEXT_SWITCHES` counter (syscall 523's value).
    pub context_switches: u64,
    /// The raw initial count programmed into the APIC timer.
    pub timer_initial_count: u64,
    /// 1 when the kernel was built with `sched_stats`, 0 when the per-core blocks are all zero
    /// because the instrumentation was compiled out. Without this a disabled build is
    /// indistinguishable from a machine that never scheduled anything.
    pub stats_enabled: u64,
}

/// Read the timestamp counter.
///
/// Not serialising: `rdtscp`/`lfence` would be more accurate but costs more than the thing being
/// measured in the tick path. Out-of-order skew is tens of cycles against intervals measured in
/// tens of thousands, so it is noise here — and every consumer of this is a histogram or a max,
/// neither of which is sensitive to it.
#[inline(always)]
pub fn rdtsc() -> u64 {
    #[cfg(feature = "sched_stats")]
    unsafe {
        let lo: u32;
        let hi: u32;
        core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi, options(nomem, nostack));
        ((hi as u64) << 32) | (lo as u64)
    }
    #[cfg(not(feature = "sched_stats"))]
    0
}

/// Run `f` against this core's statistics block.
///
/// Compiles to nothing without the `sched_stats` feature. Safe to call from an ISR: it touches only
/// this core's `PerCpu`, and every caller is already non-preemptible.
#[cfg(feature = "sched_stats")]
#[inline(always)]
pub fn with<F: FnOnce(&mut SchedStats)>(f: F) {
    // Guard against the window before `percpu::init` has pointed GS at anything: `current()`
    // dereferences `gs:[0x10]`, which is garbage until then. Every hot-path caller already makes
    // this check for its own reasons, but the helper must be independently safe — it is called
    // from the syscall dispatcher, which an AP can reach before its GS is live.
    if x86_64::registers::model_specific::GsBase::read().as_u64() == 0 {
        return;
    }
    f(&mut crate::percpu::current().stats);
}

#[cfg(not(feature = "sched_stats"))]
#[inline(always)]
pub fn with<F: FnOnce(&mut SchedStats)>(_f: F) {}

/// Record a timer tick: bump the count and fold the gap since the previous tick into the histogram.
///
/// See the module docs — this is the interrupts-off measurement, and it is why the timer path pays
/// exactly one `rdtsc`.
#[inline(always)]
pub fn note_tick() {
    with(|s| {
        let now = rdtsc();
        s.ticks += 1;
        if s.last_tick_tsc != 0 {
            let gap = now.saturating_sub(s.last_tick_tsc);
            s.gap_hist[SchedStats::bucket(gap)] += 1;

            // Threshold = 3 tick periods, computed once the calibration has published both numbers.
            // Deliberately NOT a constant: "too long" is a function of the tick period AND the TSC
            // rate, and both are measured at boot. Before calibration lands we record nothing
            // rather than record against a made-up scale.
            if s.gap_threshold == 0 {
                let us = crate::apic::TICK_PERIOD_US.load(core::sync::atomic::Ordering::Relaxed);
                let mhz = crate::time::TSC_MHZ.load(core::sync::atomic::Ordering::Relaxed);
                if us != 0 && mhz != 0 {
                    s.gap_threshold = us.saturating_mul(mhz).saturating_mul(3);
                }
            }

            if s.gap_threshold != 0 && gap > s.gap_threshold {
                let in_syscall = s.cur_syscall != u64::MAX;
                let blame = if in_syscall { s.cur_syscall } else { s.last_syscall };
                let idx = (s.gap_ring_head as usize) % GAP_RING_LEN;
                s.gap_ring[idx] = GapSample {
                    cycles: gap,
                    syscall: blame,
                    at_ms: crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed),
                    flags: if in_syscall { 1 } else { 0 },
                };
                s.gap_ring_head = s.gap_ring_head.wrapping_add(1);
                s.gap_ring_count = s.gap_ring_count.saturating_add(1);

                // Cumulative tally, so the answer to "what stalls most" does not depend on what
                // happens to be in the last 16 samples. Linear scan over 8 entries in an ISR is
                // fine — this only runs on a stall, which is by definition rare.
                let mut slot = None;
                for (i, t) in s.gap_tally.iter().enumerate() {
                    if t.syscall == blame {
                        slot = Some(i);
                        break;
                    }
                    if t.count == 0 && slot.is_none() {
                        slot = Some(i);
                    }
                }
                // Table full of other syscalls: evict the least frequent, so a newly-dominant
                // offender can still take a slot instead of being invisible forever.
                let slot = slot.unwrap_or_else(|| {
                    let mut min_i = 0;
                    for i in 1..GAP_TALLY_LEN {
                        if s.gap_tally[i].count < s.gap_tally[min_i].count {
                            min_i = i;
                        }
                    }
                    s.gap_tally[min_i] = GapTally { syscall: blame, count: 0, total_cycles: 0 };
                    min_i
                });
                if s.gap_tally[slot].syscall != blame {
                    s.gap_tally[slot] = GapTally { syscall: blame, count: 0, total_cycles: 0 };
                }
                s.gap_tally[slot].count += 1;
                s.gap_tally[slot].total_cycles += gap;
            }

            if gap > s.max_gap_tsc {
                s.max_gap_tsc = gap;
                // Whatever was running when the core went dark. If we are not inside a syscall,
                // blame the one that just finished: a syscall that masked interrupts does not see
                // the suppressed tick until after `sysretq`, so `cur_syscall` is already cleared by
                // the time we get here. `max_sys_cycles` is the authoritative answer; this is the
                // corroborating hint.
                s.max_gap_syscall = if s.cur_syscall != u64::MAX {
                    s.cur_syscall
                } else {
                    s.last_syscall
                };
            }
        }
        s.last_tick_tsc = now;
    });
}

/// Record that a task which became runnable at `ready_tsc` is now being given the CPU.
#[inline(always)]
pub fn note_wake_to_run(ready_tsc: u64) {
    if ready_tsc == 0 {
        return;
    }
    with(|s| {
        let d = rdtsc().saturating_sub(ready_tsc);
        s.wake_hist[SchedStats::bucket(d)] += 1;
        if d > s.max_wake_tsc {
            s.max_wake_tsc = d;
        }
    });
}

/// Mark syscall entry. Pairs with `note_syscall_exit`.
#[inline(always)]
pub fn note_syscall_enter(id: u64) {
    with(|s| {
        s.cur_syscall = id;
        s.sys_entry_tsc = rdtsc();
    });
}

/// Set when F12 is pressed; cleared when the dump has been produced.
///
/// ## Why a flag and not just printing from the ISR
///
/// ★ The obvious implementation — `serial_println!` straight out of the keyboard handler — would
/// corrupt the very thing this module measures. The keyboard ISR runs with interrupts masked, and
/// `serial_println!` takes the `SERIAL1` lock and then spins the UART one byte at a time at roughly
/// 87 us per byte. A few hundred bytes of report is therefore tens of milliseconds with the timer
/// dead: it would register as an enormous `max_gap_tsc`, attributed to nothing, caused entirely by
/// the act of asking what `max_gap_tsc` was.
///
/// So the ISR does the cheapest possible thing (one relaxed atomic store) and the thermal governor
/// — a kernel task that already runs once a second with interrupts ENABLED — does the printing.
static DUMP_REQUEST: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Ask for a scheduler dump. Safe to call from an interrupt handler.
#[inline(always)]
pub fn request_dump() {
    DUMP_REQUEST.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// If a dump was requested, print one. Call ONLY from a context with interrupts enabled.
///
/// Writes through `serial_println!`, which also lands in the in-RAM `BOOT_LOG` — so on the real
/// laptop, where there is no serial port to read, this is still recoverable through sysmon's boot
/// log view. That is the whole reason it is worth having beside the `sched` terminal command.
pub fn service_dump_request() {
    use core::sync::atomic::Ordering;
    if !DUMP_REQUEST.swap(false, Ordering::Relaxed) {
        return;
    }

    let g = globals();
    crate::serial_println!("=== SCHED DUMP ===");
    if g.stats_enabled == 0 {
        crate::serial_println!("[sched] built WITHOUT sched_stats; counters below are all zero.");
    }
    crate::serial_println!(
        "[sched] tick={} us (kernel assumes 1000)  apic={} ticks/ms  count={:#x}  tsc={} MHz",
        g.tick_period_us as u32, g.apic_ticks_per_ms as u32,
        g.timer_initial_count as u32, g.tsc_mhz as u32);
    crate::serial_println!(
        "[sched] uptime={} ms (kernel's count)  cores={}  ctx_switches={}",
        g.uptime_ms as u32, g.active_cores as u32, g.context_switches as u32);

    let mhz = g.tsc_mhz.max(1);
    unsafe {
        if let Some(cores) = &crate::percpu::PER_CPU {
            for (i, c) in cores.iter().enumerate() {
                let s = &c.stats;
                if s.ticks == 0 && s.schedule_calls == 0 {
                    continue;
                }
                // `waste` is the point of the exercise: schedule() calls that changed nothing, each
                // costing a full interrupt entry plus 1 KiB of FPU save/restore.
                let waste = s.schedule_calls.saturating_sub(s.switches);
                crate::serial_println!(
                    "[sched] cpu{} ticks={} sched={} switch={} waste={} vol={} invol={} idle={}",
                    i as u32, s.ticks as u32, s.schedule_calls as u32, s.switches as u32,
                    waste as u32, s.voluntary as u32, s.involuntary as u32,
                    s.idle_entries as u32);
                crate::serial_println!(
                    "[sched] cpu{} wake={} migr={} rq={} live={} dead={} sched_us={}",
                    i as u32, s.wakeups as u32, s.migrations as u32, s.rq_len as u32,
                    s.tasks_live as u32, s.tasks_dead as u32,
                    (s.sched_cycles / mhz) as u32);
                // The interrupts-off proxy. Anything past one tick period is time this core could
                // not take an interrupt — the APIC timer is periodic, so it should have fired.
                let gap_us = s.max_gap_tsc / mhz;
                let over = gap_us.saturating_sub(g.tick_period_us.max(1));
                crate::serial_println!(
                    "[sched] cpu{} max_wake={} us  max_tick_gap={} us (over by {}) last_syscall={}",
                    i as u32, (s.max_wake_tsc / mhz) as u32, gap_us as u32, over as u32,
                    if s.max_gap_syscall == u64::MAX { -1i32 } else { s.max_gap_syscall as i32 });
                // The attribution that cannot be fooled — timed inside the syscall itself.
                crate::serial_println!(
                    "[sched] cpu{} longest_syscall={} us (syscall {})",
                    i as u32, (s.max_sys_cycles / mhz) as u32,
                    if s.max_sys_id == u64::MAX { -1i32 } else { s.max_sys_id as i32 });
            }
        }
    }
    crate::serial_println!("=== END SCHED DUMP ===");
}

/// Mark syscall exit and fold its duration into the histogram.
#[inline(always)]
pub fn note_syscall_exit() {
    with(|s| {
        if s.sys_entry_tsc != 0 {
            let d = rdtsc().saturating_sub(s.sys_entry_tsc);
            s.sys_hist[SchedStats::bucket(d)] += 1;
            // Recorded HERE, inside the syscall, because the tick-gap probe cannot see this: the
            // suppressed timer interrupt only arrives after `sysretq` restores IF.
            if d > s.max_sys_cycles {
                s.max_sys_cycles = d;
                s.max_sys_id = s.cur_syscall;
            }
        }
        s.sys_entry_tsc = 0;
        s.last_syscall = s.cur_syscall;
        s.cur_syscall = u64::MAX;
    });
}
