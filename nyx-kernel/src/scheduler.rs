use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};
use crate::process::Process;

// Keep track of context switches for sysinfo (Syscall 523)
pub static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Running,
    Ready,
    Blocked,
    Zombie,
    Empty,
}

/// Why a `Blocked` task is blocked — i.e. what event should wake it.
///
/// ## Why this exists
///
/// ★ Before this, `Blocked` carried no reason, so every waker had to guess. The keyboard and mouse
/// ISRs guessed by waking **every** task with a finite `wake_tsc` — which meant a moving PS/2 mouse
/// (3-4 IRQs per motion event, up to 200 Hz) dragged every sleeping task on the core to `Ready`,
/// each costing an O(n) scan, a full context switch and a 1 KiB FPU save/restore. It also silently
/// broke `sleep()` system-wide: `sys_sleep_ms` treats a cleared `wake_tsc` as a legal early return,
/// so while the mouse moved, every app's 16 ms frame sleep, `wifiagent`'s 500 ms and `init`'s
/// 1000 ms all returned immediately.
///
/// With a reason recorded, a waker can wake exactly the tasks waiting for the thing that happened.
///
/// ⚠️ `wake_tsc` remains the DEADLINE and is orthogonal: a task can be `Ipc` with a deadline (wake
/// on a message *or* at a time), which is precisely what a GUI app's frame loop needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitReason {
    /// Not blocked, or blocked with no specific waker (deadline only).
    None,
    /// `sleep`/`nanosleep` — nothing but the clock should wake this.
    Timer,
    /// Waiting in `ipc_recv`. Woken by `ipc_send`, or by the deadline if it has one.
    Ipc,
    /// Waiting for keyboard/mouse input. Woken by the input ISRs.
    Input,
    /// Waiting on a futex word.
    Futex,
    /// `wait4` — waiting for a child to exit.
    Child,
}

#[derive(Clone)]
pub enum SocketKind {
    Udp(smoltcp::iface::SocketHandle),
    Tcp(smoltcp::iface::SocketHandle),
}

pub struct KernelSocket {
    pub kind: SocketKind,
    pub local_port: u16,
    pub remote: Option<smoltcp::wire::IpEndpoint>,
    pub non_blocking: bool,
    /// Which of the two smoltcp stacks this socket lives in. A `SocketHandle` is an index into one
    /// specific `SocketSet`, so looking it up in the other stack would alias an unrelated socket.
    pub stack: crate::drivers::net::NetStack,
    /// The WiFi interface generation this socket was created against (0 for wired, which is never
    /// destroyed). Goes stale when a disconnect/reconnect rebuilds the set — see `stack_alive`.
    pub gen: u64,
    /// Blocking read/write deadline in milliseconds; 0 = block forever (the historical behaviour).
    ///
    /// Without this a stalled peer wedges the calling thread permanently: the blocking loops in
    /// `sys_read`/`sys_write` only exit on data or on the link dying, so a half-open connection or a
    /// TLS handshake that stops mid-flight has no way out — and userspace cannot impose its own
    /// deadline, because it never regains control to check one. Set via syscall 549.
    pub timeout_ms: u64,
}

#[derive(Clone)]
pub enum FileDescriptor {
    File(alloc::sync::Arc<crate::vfs::OpenFile>),
    Socket(alloc::sync::Arc<spin::Mutex<KernelSocket>>),
    PipeRead(alloc::sync::Arc<spin::Mutex<alloc::collections::VecDeque<u8>>>),
    PipeWrite(alloc::sync::Arc<spin::Mutex<alloc::collections::VecDeque<u8>>>),
    /// One end of a `socketpair(AF_UNIX, ...)`.
    ///
    /// ★ It cannot reuse PipeRead/PipeWrite, because a socketpair endpoint is BIDIRECTIONAL: one
    /// descriptor that is both readable and writable. `rx` is the queue this end drains, `tx` the
    /// queue it fills. The peer holds the same two queues with the fields SWAPPED — that swap IS
    /// the connection; there is no shared "socket" object to own.
    ///
    /// Needed because multi-process programs use this for IPC (Ladybird's chrome talks to
    /// WebContent over exactly one), and `socket(AF_UNIX)` cannot substitute: at fork time there
    /// is no name to bind and no listener to connect to.
    UnixStream {
        rx: alloc::sync::Arc<spin::Mutex<alloc::collections::VecDeque<u8>>>,
        tx: alloc::sync::Arc<spin::Mutex<alloc::collections::VecDeque<u8>>>,
    },
}

pub fn generate_pid() -> u64 {
    static NEXT_PID: AtomicU64 = AtomicU64::new(1);
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

// Liveness heartbeat state. See `postmortem::heartbeat` — these three exist so that a boot AFTER a
// freeze can say whether the kernel was still scheduling, which is otherwise unknowable on a
// machine with no serial port and a GPU-owned framebuffer.
/// Uptime at the last beat written to CMOS.
static HB_LAST_MS: AtomicU64 = AtomicU64::new(0);
/// Uptime at the last time a NON-idle task was actually picked to run.
static HB_LAST_USER_MS: AtomicU64 = AtomicU64::new(0);
/// Rolling beat counter, so a stalled heartbeat is distinguishable from a repeated one.
static HB_TICK: AtomicU64 = AtomicU64::new(0);
/// Uptime at the last time each core ran `schedule()`.
///
/// ★★★ This is the value the first version of the heartbeat needed and did not have. `PerCpu` owns
/// its `Scheduler` BY VALUE (percpu.rs), so every core has an independent task list — which makes
/// `tasks`/`runnable`/`current` describe only whichever core happened to write the beat. The first
/// report said `tasks=1 runnable=0` and confidently concluded "every userspace task was Blocked",
/// when all it actually meant was "the core that wrote this had nothing but its idle task" — which
/// is the normal state of a secondary core.
///
/// A core that wedges with interrupts masked takes no timer, so it never reaches `schedule()` and
/// its entry stops advancing, while other cores keep beating. That is precisely the signature to
/// look for, and it is invisible to a per-core snapshot.
static HB_CORE_LAST_MS: [AtomicU64; 32] = [const { AtomicU64::new(0) }; 32];

/// Place a newly created task on the least-loaded core and return which one took it.
///
/// ★★★ This is the change that makes the machine multi-core at all. `fork` and `clone` pushed onto
/// the CALLING core unconditionally, and since every process descends from init on core 0, the
/// entire system ran there. Measured on 8-core hardware over 42 s: core 0 did 21,349 context
/// switches while cores 1-7 took ~42,000 timer interrupts each and switched task **zero** times —
/// seven cores doing nothing, expensively.
///
/// Placement is by live load (see [`Scheduler::load`]), with a deliberate bias to the caller's own
/// core: a fresh task shares its parent's address space and page tables, so keeping it local is
/// worth more than perfect balance. Only a core that is genuinely less loaded wins it.
///
/// ⚠️ Handover goes through the target's INBOX, never a direct push into its `tasks`. See
/// [`Scheduler::inbox`] for why writing another core's Vec is unsound.
///
/// Safety: caller must ensure `PER_CPU` is initialised.
pub unsafe fn place_task(task: Process, local_id: usize) -> Option<usize> {
    let cores = match &mut crate::percpu::PER_CPU {
        Some(c) => c,
        None => return None,
    };
    let active = crate::smp::ACTIVE_CORES.load(Ordering::SeqCst).min(cores.len());
    if active <= 1 {
        return match cores[local_id].scheduler.alloc_slot(task) {
            Ok(_) => Some(local_id),
            Err(_) => None,
        };
    }

    let local_load = cores[local_id].scheduler.load();
    let mut best = local_id;
    // Strictly-less-than, plus the +1 margin below, so a tie never moves a task off its parent's
    // core for nothing.
    let mut best_load = local_load;
    for i in 0..active {
        if i == local_id {
            continue;
        }
        let l = cores[i].scheduler.load();
        if l + 1 < best_load {
            best_load = l;
            best = i;
        }
    }

    if best == local_id {
        return match cores[local_id].scheduler.alloc_slot(task) {
            Ok(_) => Some(local_id),
            Err(_) => None,
        };
    }

    // Remote: hand off via the inbox and let the owner splice it in on its next tick.
    cores[best].scheduler.inbox.lock().push(task);
    Some(best)
}

pub struct Scheduler {
    pub tasks: Vec<Process>,
    pub core_task_idx: [usize; 32],
    /// Tasks handed to this core by a DIFFERENT core, waiting to be adopted.
    ///
    /// ★ This is how work reaches a core other than the one that created it, and it exists because
    /// the obvious alternative is unsound. `tasks` is only ever mutated by its owning core, and
    /// several syscalls (`ipc_send`, `futex` wake, `wait4`, `sysinfo`) *iterate other cores'*
    /// `tasks` to find a pid. A remote `push` that reallocated the Vec under one of those walks
    /// would leave it following a freed pointer. Handing the task through a lock-guarded inbox and
    /// letting the OWNER splice it in keeps every mutation of `tasks` on its own core.
    ///
    /// The lock is only ever taken to hand over or to drain, never on the scheduling hot path —
    /// `schedule` checks emptiness with a `try_lock` and skips it entirely in the common case.
    pub inbox: spin::Mutex<Vec<Process>>,
}

/// Slots reserved per core, so `tasks` never reallocates.
///
/// ⚠️ Load-bearing for memory safety, not just performance. The cross-core scans above read
/// `tasks` while its owner may be pushing; if the Vec's backing store can move, those reads are a
/// use-after-free. Reserving up front pins the allocation, so a remote reader may observe a stale
/// length but never a dangling pointer. Slots are also RECYCLED rather than appended (see
/// `alloc_slot`), so the reservation is a ceiling on live tasks, not on total tasks ever created.
const TASK_SLOTS: usize = 192;

impl Scheduler {
    pub fn new() -> Self {
        Self {
            tasks: Vec::with_capacity(TASK_SLOTS),
            core_task_idx: [0; 32],
            inbox: spin::Mutex::new(Vec::new()),
        }
    }

    /// Find a home for a new task: recycle a dead slot if there is one, else append.
    ///
    /// ★★ Recycling rather than appending is what stops `tasks` growing for the life of the boot.
    /// `TaskState::Empty` is a tombstone left by `wait4`/thread-reap, and nothing ever removed one —
    /// so every per-tick O(n) pass (the wake sweep, the round-robin search) walked every process
    /// that had *ever* existed. Entries cannot simply be removed instead, because `core_task_idx`
    /// holds an INDEX into this Vec and removing would silently re-point every core at the wrong
    /// task.
    ///
    /// On failure the task is handed BACK rather than dropped — `Err(task)`. A dropped `Process`
    /// here would be a process that silently ceased to exist, which is a far worse outcome than
    /// refusing the placement; every caller has somewhere else to put it.
    pub fn alloc_slot(&mut self, task: Process) -> Result<usize, Process> {
        if let Some(i) = self
            .tasks
            .iter()
            .position(|t| t.state == TaskState::Empty)
        {
            self.tasks[i] = task;
            return Ok(i);
        }
        if self.tasks.len() < TASK_SLOTS {
            self.tasks.push(task);
            return Ok(self.tasks.len() - 1);
        }
        Err(task)
    }

    /// Live, schedulable tasks on this core — the real load, for placement decisions.
    ///
    /// ⚠️ NOT `tasks.len()`. That is what the old thread-spawn balancer used, and it counts the
    /// idle task plus every Zombie/Empty tombstone — so a core that had reaped a few processes
    /// looked permanently busier than one that had not, and placement drifted to whichever core had
    /// done least work historically rather than least work now.
    pub fn load(&self) -> usize {
        self.tasks
            .iter()
            .filter(|t| {
                !t.is_idle
                    && t.state != TaskState::Empty
                    && t.state != TaskState::Zombie
            })
            .count()
    }

    /// Adopt anything another core has handed us. Owner-only; called from `schedule`.
    fn drain_inbox(&mut self) {
        // `try_lock`, and only when a handover is actually pending: this runs on every tick of
        // every core, and blocking here would put a cross-core lock on the scheduling hot path.
        // Missing a handover costs one tick — the next pass picks it up.
        // Take the whole queue out under the lock and release it immediately, rather than holding
        // it across the slot search. Shorter critical section, and it keeps the lock off the path
        // that another core may be spinning on.
        let taken = {
            let mut pending = match self.inbox.try_lock() {
                Some(g) if !g.is_empty() => g,
                _ => return,
            };
            core::mem::take(&mut *pending)
        };

        let mut rejected = Vec::new();
        for task in taken {
            match self.alloc_slot(task) {
                Ok(_) => crate::schedstats::with(|s| s.migrations += 1),
                // Full. Kept, not dropped — dropping a `Process` here would make a process vanish.
                Err(t) => rejected.push(t),
            }
        }
        if !rejected.is_empty() {
            // Blocking `lock` on this rare path: a `try_lock` that failed would drop the tasks it
            // is trying to preserve, which is the one outcome this whole branch exists to avoid.
            self.inbox.lock().extend(rejected);
        }
    }

    /// Takes the stack pointer of the currently preempted process,
    /// selects the next ready process, swaps the hardware memory space (CR3),
    /// and returns the stack pointer of the new process.
    pub fn schedule(&mut self, current_rsp: u64) -> u64 {
        // Adopt anything another core handed us. Must happen before the emptiness check below, or
        // a core whose only task arrives by handover would never pick it up.
        self.drain_inbox();

        if self.tasks.is_empty() {
            return current_rsp;
        }

        let sched_entry_tsc = crate::schedstats::rdtsc();
        crate::schedstats::with(|s| s.schedule_calls += 1);

        // --- 1. WAKE UP SLEEPING TASKS (UPTIME CLOCK) ---
        let current_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);

        for task in self.tasks.iter_mut() {
            if task.state == TaskState::Blocked && task.wake_tsc != 0 && task.wake_tsc != u64::MAX {
                // Check if the current time has surpassed the target wakeup time
                if current_ms >= task.wake_tsc {
                    task.state = TaskState::Ready;
                    task.wake_tsc = 0; // Clear the timer
                    // The deadline was the waker, so the reason is spent. Left set, a task that
                    // timed out of `ipc_recv` would still look like an IPC waiter to `ipc_send`.
                    task.wait_reason = WaitReason::None;
                    // Stamped here so the wake-to-run histogram measures the gap between becoming
                    // runnable and actually getting the CPU — which for a timer wake is the
                    // scheduler's own responsiveness, with no driver in the way.
                    task.ready_tsc = sched_entry_tsc;
                    crate::schedstats::with(|s| s.wakeups += 1);
                }
            }
        }

        // --- 2. SAVE HARDWARE STATE ---
        let logical_id = crate::percpu::current().logical_id as usize % 32;
        let curr_idx = self.core_task_idx[logical_id];

        if curr_idx < self.tasks.len() {
            let current_process = &mut self.tasks[curr_idx];
            
            // FIX: ALWAYS save the stack pointer so we don't jump backward in time!
            current_process.saved_rsp = current_rsp;

            // B-β.2a: save the FS base so per-thread TLS survives preemption. Threads of one
            // process share cr3 but each has its own FS base (set via arch_prctl in userspace);
            // without this, a context switch would leave the previous thread's FS loaded and
            // corrupt thread-local access. 0 for the no_std apps that never set FS (harmless).
            current_process.saved_fs_base =
                x86_64::registers::model_specific::FsBase::read().as_u64();
            
            // If it was Running (normal preemption), mark it Ready so it can run again.
            // If it was Blocked (sys_sleep or IPC wait), we leave it Blocked!
            if current_process.state == TaskState::Running {
                current_process.state = TaskState::Ready;
            }
        }

        // --- 2b. LIVENESS HEARTBEAT (once a second, into CMOS) ---
        //
        // Written from here because this is the only place that knows both the task states and
        // whether a real task actually got the CPU. See `postmortem::heartbeat` for why NVRAM is
        // the only channel that survives the freeze this exists to explain.
        {
            // Stamp THIS core first, every pass — a core that stops reaching here is the thing we
            // are hunting, and it must be recorded even when another core writes the beat.
            HB_CORE_LAST_MS[logical_id].store(current_ms, Ordering::Relaxed);

            let last = HB_LAST_MS.load(Ordering::Relaxed);
            if current_ms >= last.saturating_add(1000) {
                HB_LAST_MS.store(current_ms, Ordering::Relaxed);
                let runnable = self
                    .tasks
                    .iter()
                    .filter(|t| {
                        !t.is_idle
                            && (t.state == TaskState::Ready || t.state == TaskState::Running)
                    })
                    .count();

                // Piggy-backed on the heartbeat's existing once-a-second walk rather than adding a
                // per-tick scan of its own: these are gauges, a 1 Hz sample is plenty, and the
                // whole point of this module is to not become the overhead it measures.
                //
                // `tasks_dead` is the interesting one — Zombie/Empty entries are never reclaimed,
                // so it only ever climbs, and every O(n) pass above walks them forever.
                {
                    let dead = self
                        .tasks
                        .iter()
                        .filter(|t| t.state == TaskState::Zombie || t.state == TaskState::Empty)
                        .count();
                    let total = self.tasks.len();
                    crate::schedstats::with(|s| {
                        s.rq_len = runnable as u64;
                        s.tasks_dead = dead as u64;
                        s.tasks_live = (total - dead) as u64;
                    });
                }
                let idle_ms =
                    current_ms.saturating_sub(HB_LAST_USER_MS.load(Ordering::Relaxed));

                // One bit per core, set if that core reached `schedule()` in the last 2 s. A core
                // spinning with interrupts masked takes no timer, so its bit goes dark while the
                // rest keep beating — which is the difference between "the kernel died" and "ONE
                // core wedged and the tasks it owned never ran again".
                let mut alive: u8 = 0;
                for c in 0..8 {
                    let seen = HB_CORE_LAST_MS[c].load(Ordering::Relaxed);
                    if seen != 0 && current_ms.saturating_sub(seen) < 2000 {
                        alive |= 1 << c;
                    }
                }

                crate::postmortem::heartbeat(
                    HB_TICK.fetch_add(1, Ordering::Relaxed) as u8,
                    self.tasks.len().min(255) as u8,
                    runnable.min(255) as u8,
                    curr_idx.min(255) as u8,
                    (idle_ms / 1000).min(255) as u8,
                    alive,
                    logical_id.min(255) as u8,
                );
            }
        }

        // --- 3. SMART PRIORITY ROUND-ROBIN ---
        let mut next_idx = (curr_idx + 1) % self.tasks.len();
        let mut fallback_idle_idx = None;
        let mut found = false;

        for _ in 0..self.tasks.len() {
            let state = self.tasks[next_idx].state;
            
            if state == TaskState::Ready || state == TaskState::Running {
                // If it's the Idle Task, remember it, but keep looking for real work!
                if self.tasks[next_idx].is_idle {
                    fallback_idle_idx = Some(next_idx);
                } else {
                    // We found a REAL task! Stop searching.
                    found = true;
                    break; 
                }
            }
            next_idx = (next_idx + 1) % self.tasks.len();
        }

        // `found` means a NON-idle task is about to run, which is the definition of "userspace is
        // still being scheduled". Stamped here rather than at the top, because the wake pass above
        // can mark tasks Ready without any of them actually reaching the CPU.
        if found {
            HB_LAST_USER_MS.store(current_ms, Ordering::Relaxed);
        }

        if !found {
            // No normal user/kernel tasks are ready to run. Let the CPU sleep!
            if let Some(idle_idx) = fallback_idle_idx {
                next_idx = idle_idx;
                crate::schedstats::with(|s| s.idle_entries += 1);
            } else {
                crate::schedstats::with(|s| {
                    s.sched_cycles += crate::schedstats::rdtsc().saturating_sub(sched_entry_tsc)
                });
                return current_rsp; // Absolute worst-case fallback
            }
        }

        // --- 4. UPDATE STATE ---
        self.core_task_idx[logical_id] = next_idx;
        let next_process = &mut self.tasks[next_idx];
        next_process.state = TaskState::Running;

        // ★ The gap between `schedule_calls` and `switches` is pure waste: a full interrupt entry,
        // 512 bytes of FXSAVE and 512 of FXRSTOR spent to conclude that the task already running
        // should keep running. Counting both is what makes that waste visible.
        if next_idx != curr_idx {
            crate::schedstats::with(|s| s.switches += 1);
        }
        crate::schedstats::note_wake_to_run(next_process.ready_tsc);
        next_process.ready_tsc = 0;

        // 🚨 5. THE HARDWARE BRAIN SWAP 🚨
        unsafe {
            // A. Point the Syscall Gateway to this process's specific Kernel Stack.
            // When `syscall` is called, the CPU looks at `gs:[0]`.
            let percpu_base = crate::percpu::current() as *const _ as *mut u64;
            *percpu_base = next_process.kernel_stack_top; 
            
            // B. Point the Hardware Interrupt Gateway to this process's Kernel Stack!
            // When a hardware timer interrupts userspace, the CPU reads the TSS to find a secure Ring 0 stack.
            let tss_ptr = crate::percpu::current().gdt_state.tss as *const _ as *mut x86_64::structures::tss::TaskStateSegment;
            (*tss_ptr).privilege_stack_table[0] = x86_64::VirtAddr::new(next_process.kernel_stack_top);

            // C. Swap the Virtual Memory Space!
            let next_cr3 = next_process.cr3.as_u64();
            let mut current_cr3: u64;
            core::arch::asm!("mov {}, cr3", out(reg) current_cr3, options(nomem, nostack, preserves_flags));
            
            if current_cr3 != next_cr3 {
                core::arch::asm!("mov cr3, {}", in(reg) next_cr3, options(nostack, preserves_flags));
            }

            // D. Restore this task's FS base for per-thread TLS (B-β.2a). Paired with the save in
            // step 2. Writing 0 for tasks that never use FS is harmless (they never read fs:).
            x86_64::registers::model_specific::FsBase::write(
                x86_64::VirtAddr::new(next_process.saved_fs_base),
            );
        }

        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);

        let rsp = next_process.saved_rsp;
        crate::schedstats::with(|s| {
            s.sched_cycles += crate::schedstats::rdtsc().saturating_sub(sched_entry_tsc)
        });

        // 6. Return the saved stack pointer so the assembly `iretq` resumes the new process
        rsp
    }
}