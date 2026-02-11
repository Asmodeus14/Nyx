use alloc::vec::Vec;
use alloc::boxed::Box;
use x86_64::VirtAddr;
use crate::gui::Painter; 
use x86_64::registers::segmentation::{Segment, CS}; // Needed to get valid Code Segment

// Global Scheduler Instance
pub static mut SCHEDULER: Option<Scheduler> = None;

// A simple Linear Congruential Generator for "Quantum" randomness
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
        
        // 2. Align Stack Pointer (16-byte alignment required by x64 ABI)
        let mut sp = stack_top & !0xF;

        unsafe {
            // --- BUILD THE INTERRUPT STACK FRAME (For IRETQ) ---
            // The CPU pops these 5 values when returning from an interrupt.
            // Order is crucial: SS, RSP, RFLAGS, CS, RIP
            
            // 5. SS (Stack Segment) - 0 is fine for kernel mode
            sp -= 8; * (sp as *mut u64) = 0; 
            
            // 4. RSP (Stack Pointer) - Where the task's stack should start
            // We use the current 'sp' (before we pushed SS) as the starting point.
            // Note: Since we are modifying 'sp' downwards, the 'sp' variable tracks the growing stack.
            // But the 'RSP' value we push here is where the stack *would be* if we hadn't pushed the trap frame.
            // Actually, let's simply point it to the top of the context we are about to create.
            // The logic: When iretq runs, it sets RSP to this value.
            let task_stack_start = sp; 
            sp -= 8; * (sp as *mut u64) = task_stack_start;

            // 3. RFLAGS (CPU Flags) - 0x202 = Interrupts Enabled
            sp -= 8; * (sp as *mut u64) = 0x202;

            // 2. CS (Code Segment) - Must match current Kernel Code Selector
            sp -= 8; * (sp as *mut u64) = CS::get_reg().0 as u64;

            // 1. RIP (Instruction Pointer) - The function to run
            sp -= 8; * (sp as *mut u64) = func as u64;

            // --- BUILD THE SAVED REGISTERS (For POP) ---
            // Our interrupt handler does 'pop rbp', 'pop r15', ... 'pop rax'.
            // We push 15 zeroed registers to match this.
            // 15 * 8 bytes = 120 bytes.
            sp -= 120;
            
            // The 'sp' is now at the bottom of the stack frame.
            // When we switch to this task, we load this 'sp' into RSP.
            // Then the `pop` instructions consume the 120 bytes of zeros.
            // Then `iretq` consumes the 5 values (RIP, CS, RFLAGS, RSP, SS).
        }

        // Prevent Rust from deallocating the stack vector
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

        // Save current task's state
        if let Some(current) = self.tasks.get_mut(self.current_task_idx) {
            current.stack_ptr = current_rsp;
        }

        // Pick next task (Quantum Lottery)
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

        // Return new stack pointer
        self.tasks[self.current_task_idx].stack_ptr
    }
}

pub extern "C" fn clock_task() {
    let mut ticks = 0;
    loop {
        ticks += 1;
        unsafe {
            if let Some(painter) = &mut crate::SCREEN_PAINTER {
                let time_str = alloc::format!("Quantum Time: {:05}", ticks);
                // Draw clock at top right
                let x_pos = if painter.width() > 200 { painter.width() - 200 } else { 0 };
                painter.draw_string(x_pos, 20, &time_str, crate::gui::Color::CYAN);
            }
        }
        // Slow down the loop so we can see the counting
        for _ in 0..5_000_000 { core::hint::spin_loop(); }
    }
}

pub extern "C" fn background_worker() {
    let mut _id = 0;
    loop {
        _id += 1;
        // Just burn CPU cycles to show preemptive multitasking works
        // (If it wasn't preemptive, this infinite loop would freeze the clock)
    }
}