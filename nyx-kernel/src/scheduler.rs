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
}

pub struct KernelSocket {
    pub kind: SocketKind,
    pub local_port: u16,
    pub remote: Option<smoltcp::wire::IpEndpoint>,
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
        let logical_id = crate::percpu::current().logical_id as usize % 32;
        let curr_idx = self.core_task_idx[logical_id];

        // 1. Save the hardware state of the currently running process
        if curr_idx < self.tasks.len() {
            let current_process = &mut self.tasks[curr_idx];
            if current_process.state == TaskState::Running {
                current_process.saved_rsp = current_rsp;
                current_process.state = TaskState::Ready;
            }
        }

        if self.tasks.is_empty() {
            return current_rsp;
        }

        // 2. Simple Round-Robin: Find the next Ready process
        let mut next_idx = (curr_idx + 1) % self.tasks.len();
        let mut found = false;

        for _ in 0..self.tasks.len() {
            if self.tasks[next_idx].state == TaskState::Ready || self.tasks[next_idx].state == TaskState::Running {
                found = true;
                break;
            }
            next_idx = (next_idx + 1) % self.tasks.len();
        }

        if !found {
            return current_rsp; // Fallback to current if nothing else is ready
        }

        // 3. Update state
        self.core_task_idx[logical_id] = next_idx;
        let next_process = &mut self.tasks[next_idx];
        next_process.state = TaskState::Running;

        // 🚨 4. THE HARDWARE BRAIN SWAP 🚨
        unsafe {
            // A. Point the Syscall Gateway to this process's specific Kernel Stack.
            // When `syscall` is called, the CPU looks at `gs:[0]`.
            let percpu_base = crate::percpu::current() as *const _ as *mut u64;
            *percpu_base = next_process.kernel_stack_top; 
            
            // 🚨 B. THE FATAL TSS FIX: Point the Hardware Interrupt Gateway to this process's Kernel Stack!
            // When a hardware timer interrupts userspace, the CPU reads the TSS to find a secure Ring 0 stack.
            // This ensures every process pushes its saved state to its own isolated memory, preventing collisions!
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
        
        // 5. Return the saved stack pointer so the assembly `iretq` resumes the new process
        next_process.saved_rsp
    }
}