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
}

#[derive(Clone)]
pub enum FileDescriptor {
    File(alloc::sync::Arc<crate::vfs::OpenFile>),
    Socket(alloc::sync::Arc<spin::Mutex<KernelSocket>>),
    PipeRead(alloc::sync::Arc<spin::Mutex<alloc::collections::VecDeque<u8>>>),
    PipeWrite(alloc::sync::Arc<spin::Mutex<alloc::collections::VecDeque<u8>>>),
}

pub fn generate_pid() -> u64 {
    static NEXT_PID: AtomicU64 = AtomicU64::new(1);
    NEXT_PID.fetch_add(1, Ordering::Relaxed)
}

pub struct Scheduler {
    pub tasks: Vec<Process>,
    pub core_task_idx: [usize; 32],
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            core_task_idx: [0; 32],
        }
    }

    /// Takes the stack pointer of the currently preempted process,
    /// selects the next ready process, swaps the hardware memory space (CR3),
    /// and returns the stack pointer of the new process.
    pub fn schedule(&mut self, current_rsp: u64) -> u64 {
        if self.tasks.is_empty() {
            return current_rsp;
        }

        // --- 1. WAKE UP SLEEPING TASKS (UPTIME CLOCK) ---
        let current_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);

        for task in self.tasks.iter_mut() {
            if task.state == TaskState::Blocked && task.wake_tsc != 0 && task.wake_tsc != u64::MAX {
                // Check if the current time has surpassed the target wakeup time
                if current_ms >= task.wake_tsc {
                    task.state = TaskState::Ready;
                    task.wake_tsc = 0; // Clear the timer
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
            
            // If it was Running (normal preemption), mark it Ready so it can run again.
            // If it was Blocked (sys_sleep or IPC wait), we leave it Blocked!
            if current_process.state == TaskState::Running {
                current_process.state = TaskState::Ready;
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

        if !found {
            // No normal user/kernel tasks are ready to run. Let the CPU sleep!
            if let Some(idle_idx) = fallback_idle_idx {
                next_idx = idle_idx;
            } else {
                return current_rsp; // Absolute worst-case fallback
            }
        }

        // --- 4. UPDATE STATE ---
        self.core_task_idx[logical_id] = next_idx;
        let next_process = &mut self.tasks[next_idx];
        next_process.state = TaskState::Running;

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
        }

        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
        
        // 6. Return the saved stack pointer so the assembly `iretq` resumes the new process
        next_process.saved_rsp
    }
}