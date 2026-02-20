use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use x86_64::VirtAddr;
use crate::gui::Painter; 
use x86_64::registers::segmentation::{Segment, CS}; 
use spin::Mutex; 
use core::sync::atomic::{AtomicU64, Ordering};
use crate::vfs::OpenFile; // --- NEW: Import OpenFile for the FD Table ---

// --- DYNAMIC JOB SYSTEM ---
pub type Job = Box<dyn FnOnce() + Send + 'static>;
pub static JOB_QUEUE: Mutex<Vec<Job>> = Mutex::new(Vec::new());

// --- TELEMETRY ---
pub static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

pub fn submit_job<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut queue = JOB_QUEUE.lock();
        queue.push(Box::new(f));
    });
}

pub static mut SCHEDULER: Option<Scheduler> = None;

struct QuantumRng { seed: u64 }
impl QuantumRng {
    fn new(seed: u64) -> Self { Self { seed } }
    fn next(&mut self) -> u64 {
        self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        self.seed
    }
    fn next_limit(&mut self, limit: usize) -> usize { (self.next() as usize) % limit }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct TaskContext {
    r15: u64, r14: u64, r13: u64, r12: u64, r11: u64,
    r10: u64, r9: u64,  r8: u64,  rdi: u64, rsi: u64,
    rdx: u64, rcx: u64, rbx: u64, rax: u64, rbp: u64,
}

pub struct Task {
    pub id: usize,
    pub stack: Vec<u8>,     
    pub stack_ptr: u64,     
    pub active: bool,
    pub tickets: usize,  
    // --- NEW: Per-Process File Descriptor Table ---
    // FD 0, 1, 2 are traditionally stdin, stdout, stderr. 
    // FD 3+ are for files and GPUs.
    pub fd_table: [Option<Arc<OpenFile>>; 32], 
}

pub struct Scheduler {
    pub tasks: Vec<Task>,
    pub current_task_idx: usize,
    rng: QuantumRng,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            tasks: Vec::new(),
            current_task_idx: 0,
            rng: QuantumRng::new(12345),
        }
    }

    // Register the booting thread so it doesn't overwrite others
    pub fn register_main_thread(&mut self) {
        let task = Task {
            id: self.tasks.len(),
            stack: Vec::new(), // Current thread already has a stack
            stack_ptr: 0,      // Populated on the first timer tick
            active: true,
            tickets: 50,       // UI gets high priority
            fd_table: core::array::from_fn(|_| None), // Initialize empty FDs
        };
        self.tasks.push(task);
        self.current_task_idx = 0; // Make this the active task
    }

    pub fn spawn(&mut self, func: extern "C" fn(), tickets: usize) {
        let stack = Vec::with_capacity(4096 * 8); 
        let stack_bottom = stack.as_ptr() as u64;
        let stack_top = stack_bottom + stack.capacity() as u64;
        let mut sp = stack_top & !0xF;

        unsafe {
            sp -= 8; * (sp as *mut u64) = 0; 
            let task_stack_start = sp; 
            sp -= 8; * (sp as *mut u64) = task_stack_start;
            sp -= 8; * (sp as *mut u64) = 0x202; // RFLAGS
            sp -= 8; * (sp as *mut u64) = CS::get_reg().0 as u64;
            sp -= 8; * (sp as *mut u64) = func as u64; // RIP
            sp -= 120; // Saved Registers
        }

        core::mem::forget(stack.clone()); 

        let task = Task {
            id: self.tasks.len(),
            stack, 
            stack_ptr: sp,
            active: true,
            tickets,
            fd_table: core::array::from_fn(|_| None), // Initialize empty FDs
        };
        
        self.tasks.push(task);
    }

    pub fn schedule(&mut self, current_rsp: u64) -> u64 {
        if self.tasks.is_empty() { return current_rsp; }

        if let Some(current) = self.tasks.get_mut(self.current_task_idx) {
            current.stack_ptr = current_rsp;
        }

        let total_tickets: usize = self.tasks.iter()
            .filter(|t| t.active)
            .map(|t| t.tickets)
            .sum();

        if total_tickets > 0 {
            let winning_ticket = self.rng.next_limit(total_tickets);
            let mut counter = 0;
            for (i, task) in self.tasks.iter().enumerate() {
                if !task.active { continue; }
                counter += task.tickets;
                if counter > winning_ticket {
                    self.current_task_idx = i;
                    break;
                }
            }
        }
        
        // --- TELEMETRY UPDATE ---
        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
        
        self.tasks[self.current_task_idx].stack_ptr
    }
}

pub extern "C" fn clock_task() {
    let mut ticks = 0;
    loop {
        ticks += 1;
        unsafe {
            if let Some(painter) = &mut crate::SCREEN_PAINTER {
                let time_str = alloc::format!("Time: {:05}", ticks);
                let x_pos = if painter.width() > 200 { painter.width() - 200 } else { 0 };
                // Draw in the top right corner
                painter.draw_string(x_pos, 20, &time_str, crate::gui::Color::CYAN);
            }
        }
        for _ in 0..1_000_000 { core::hint::spin_loop(); }
    }
}

pub extern "C" fn background_worker() {
    loop {
        let job_opt = x86_64::instructions::interrupts::without_interrupts(|| {
            let mut queue = JOB_QUEUE.lock();
            if !queue.is_empty() { Some(queue.remove(0)) } else { None }
        });

        if let Some(job) = job_opt { job(); } 
        else { for _ in 0..10_000 { core::hint::spin_loop(); } }
    }
}