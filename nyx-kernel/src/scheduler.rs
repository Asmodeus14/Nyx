use alloc::vec::Vec;
use alloc::boxed::Box;
use x86_64::VirtAddr;
use crate::gui::Painter; 
use x86_64::registers::segmentation::{Segment, CS}; 
use spin::Mutex; 

// --- DYNAMIC JOB SYSTEM ---
// A 'Job' is just a closure function waiting to be run.
// Send + 'static allows it to be moved between threads safely.
pub type Job = Box<dyn FnOnce() + Send + 'static>;

// The centralized work queue for the entire OS
pub static JOB_QUEUE: Mutex<Vec<Job>> = Mutex::new(Vec::new());

// Public API to submit work from anywhere (Syscalls, Drivers, etc.)
pub fn submit_job<F>(f: F)
where
    F: FnOnce() + Send + 'static,
{
    // Wrap the function in a Box and push to queue
    // Usage: scheduler::submit_job(move || { some_heavy_function(); });
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut queue = JOB_QUEUE.lock();
        queue.push(Box::new(f));
    });
}
// -------------------------------

// Global Scheduler Instance
pub static mut SCHEDULER: Option<Scheduler> = None;

struct QuantumRng {
    seed: u64,
}

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
}

pub struct Scheduler {
    tasks: Vec<Task>,
    current_task_idx: usize,
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

    pub fn spawn(&mut self, func: extern "C" fn(), tickets: usize) {
        // 1. Allocate Stack (32KB)
        let stack = Vec::with_capacity(4096 * 8); 
        let stack_bottom = stack.as_ptr() as u64;
        let stack_top = stack_bottom + stack.capacity() as u64;
        
        let mut sp = stack_top & !0xF;

        unsafe {
            // Build Interrupt Stack Frame
            sp -= 8; * (sp as *mut u64) = 0; 
            let task_stack_start = sp; 
            sp -= 8; * (sp as *mut u64) = task_stack_start;
            sp -= 8; * (sp as *mut u64) = 0x202; // RFLAGS: Interrupts Enabled
            sp -= 8; * (sp as *mut u64) = CS::get_reg().0 as u64;
            sp -= 8; * (sp as *mut u64) = func as u64; // RIP
            
            // Saved Registers
            sp -= 120;
        }

        core::mem::forget(stack.clone()); 

        let task = Task {
            id: self.tasks.len(),
            stack, 
            stack_ptr: sp,
            active: true,
            tickets,
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
                painter.draw_string(x_pos, 20, &time_str, crate::gui::Color::CYAN);
            }
        }
        // Yield time
        for _ in 0..1_000_000 { core::hint::spin_loop(); }
    }
}

// --- UNIVERSAL BACKGROUND WORKER ---
pub extern "C" fn background_worker() {
    loop {
        // 1. Check for ANY generic job
        // We use without_interrupts to safely pop from the queue
        let job_opt = x86_64::instructions::interrupts::without_interrupts(|| {
            let mut queue = JOB_QUEUE.lock();
            if !queue.is_empty() {
                // Remove from front (FIFO Queue)
                Some(queue.remove(0))
            } else {
                None
            }
        });

        // 2. Execute it
        if let Some(job) = job_opt {
            // This calls the closure we created in syscalls.
            // The worker doesn't know it's a file write, it just runs it!
            job(); 
        } else {
            // 3. Sleep if empty (prevents CPU burning)
            for _ in 0..10_000 { core::hint::spin_loop(); }
        }
    }
}