use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::collections::VecDeque;
use crate::vfs::OpenFile; 
use x86_64::registers::segmentation::{Segment, CS}; 
use spin::Mutex; 
use core::sync::atomic::{AtomicU64, Ordering};
use smoltcp::iface::SocketHandle;
use smoltcp::wire::IpEndpoint;

// ─────────────────────────────────────────────────────────────────────────
// VFS & SOCKET ABSTRACTIONS
// ─────────────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, Debug)]
pub enum SocketKind {
    Udp(SocketHandle),
    Tcp(SocketHandle),
}

pub struct KernelSocket {
    pub kind: SocketKind,
    pub local_port: u16,
    pub remote: Option<IpEndpoint>,
}

#[derive(Clone)]
pub enum FileDescriptor {
    File(Arc<OpenFile>),
    Socket(Arc<Mutex<KernelSocket>>), // Mutex needed because sys_connect mutates remote
}

// ─────────────────────────────────────────────────────────────────────────
// SYSTEM GLOBALS & ATOMICS
// ─────────────────────────────────────────────────────────────────────────
pub type Job = Box<dyn FnOnce() + Send + 'static>;
pub static JOB_QUEUE: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());

pub static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

pub fn submit_job<F>(f: F) where F: FnOnce() + Send + 'static {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut queue = JOB_QUEUE.lock();
        queue.push_back(Box::new(f)); 
    });
}

// ─────────────────────────────────────────────────────────────────────────
// SMP GLOBAL RUNQUEUE
// ─────────────────────────────────────────────────────────────────────────
pub static GLOBAL_RUNQUEUE: Mutex<VecDeque<Task>> = Mutex::new(VecDeque::new());

unsafe impl Send for Task {}

pub fn spawn_any_core(func: extern "C" fn(), tickets: usize) {
    let len = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut q = GLOBAL_RUNQUEUE.lock();
        q.push_back(Task::new(func, tickets)); 
        q.len()
    });  
    crate::serial_println!("[SCHED] Task added to Global Queue by core {} | size {}", crate::percpu::current().logical_id, len);
}

pub fn spawn(func: extern "C" fn(), tickets: usize) {
    spawn_any_core(func, tickets);
}

pub fn spawn_pinned(core_id: usize, func: extern "C" fn(), tickets: usize) {
    let _len = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut q = GLOBAL_RUNQUEUE.lock();
        let mut task = Task::new(func, tickets);
        task.core_id = core_id; 
        q.push_back(task); 
        q.len()
    });
}

// ─────────────────────────────────────────────────────────────────────────
// SCHEDULER CORE
// ─────────────────────────────────────────────────────────────────────────
struct QuantumRng { seed: u64 }
impl QuantumRng {
    fn new(seed: u64) -> Self { Self { seed } }
    fn next(&mut self) -> u64 { self.seed = self.seed.wrapping_mul(6364136223846793005).wrapping_add(1); self.seed }
    fn next_limit(&mut self, limit: usize) -> usize { (self.next() as usize) % limit }
}

pub struct Task {
    pub id: usize,
    pub stack: Vec<u8>,     
    pub stack_ptr: u64,     
    pub active: bool,
    pub tickets: usize,  
    pub core_id: usize, 
    pub fd_table: [Option<FileDescriptor>; 32], 
}

impl Task {
    pub fn new(func: extern "C" fn(), tickets: usize) -> Self {
        let mut stack = Vec::with_capacity(4096 * 8); 
        unsafe { stack.set_len(4096 * 8); } 
        
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

        static NEXT_ID: AtomicU64 = AtomicU64::new(1000);
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed) as usize;

        Task {
            id, stack, stack_ptr: sp, active: true, tickets,
            core_id: usize::MAX,
            fd_table: core::array::from_fn(|_| None), 
        }
    }
}

pub struct Scheduler {
    pub tasks: Vec<Task>,
    pub current_task_idx: usize, 
    pub core_task_idx: [usize; 32], 
    rng: QuantumRng,
}

impl Scheduler {
    pub fn new() -> Self {
        Self { tasks: Vec::with_capacity(256), current_task_idx: 0, core_task_idx: [usize::MAX; 32], rng: QuantumRng::new(12345) }
    }

    pub fn register_main_thread(&mut self) {
        let task = Task {
            id: self.tasks.len(), stack: Vec::new(), stack_ptr: 0, active: true, tickets: 1000, 
            core_id: usize::MAX, fd_table: core::array::from_fn(|_| None), 
        };
        self.tasks.push(task);
        self.current_task_idx = 0; 
        self.core_task_idx = [0; 32]; 
    }

    pub fn schedule(&mut self, current_rsp: u64) -> u64 {
        let core_id = crate::percpu::current().logical_id;
        let core_idx = core_id % 32; 
        let curr_idx = self.core_task_idx[core_idx];

        if curr_idx != usize::MAX && curr_idx < self.tasks.len() {
            if let Some(current) = self.tasks.get_mut(curr_idx) { current.stack_ptr = current_rsp; }
        }

        let active_tasks = self.tasks.iter().filter(|t| t.active).count();

        if active_tasks < 4 {
            if let Some(mut global_queue) = GLOBAL_RUNQUEUE.try_lock() {
                if let Some(mut new_task) = global_queue.pop_front() { 
                    if self.tasks.len() < self.tasks.capacity() {
                        if new_task.core_id == usize::MAX { new_task.core_id = core_id; }
                        self.tasks.push(new_task);
                        if curr_idx == usize::MAX { self.core_task_idx[core_idx] = self.tasks.len() - 1; }
                    } else { global_queue.push_front(new_task); }
                }
            }
        }

        let total_tickets: usize = self.tasks.iter().filter(|t| t.active && (t.core_id == core_id || t.core_id == usize::MAX)).map(|t| t.tickets).sum();

        if total_tickets > 0 {
            let winning_ticket = self.rng.next_limit(total_tickets);
            let mut counter = 0;
            for (i, task) in self.tasks.iter().enumerate() {
                if !task.active || (task.core_id != core_id && task.core_id != usize::MAX) { continue; } 
                counter += task.tickets;
                if counter > winning_ticket {
                    self.core_task_idx[core_idx] = i;
                    self.current_task_idx = i; 
                    break;
                }
            }
        }
        
        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
        let raw_idx = self.core_task_idx[core_idx];

        let safe_idx = if raw_idx != usize::MAX && raw_idx < self.tasks.len() && self.tasks[raw_idx].active && (self.tasks[raw_idx].core_id == core_id || self.tasks[raw_idx].core_id == usize::MAX) {
            raw_idx
        } else {
            match self.tasks.iter().position(|t| t.active && (t.core_id == core_id || t.core_id == usize::MAX)) {
                Some(i) => { self.core_task_idx[core_idx] = i; i },
                None => return current_rsp, 
            }
        };

        if let Some(next_task) = self.tasks.get(safe_idx) { next_task.stack_ptr } else { current_rsp }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// PER-CORE TASK RESOLUTION
// ─────────────────────────────────────────────────────────────────────────
pub fn current_task_mut<'a>() -> Option<&'a mut Task> {
    let core_id = crate::percpu::current().logical_id;
    let core_idx = core_id % 32;

    // 🚨 THE FIX: Changed `current_mut()` to `current()` to match your percpu.rs
    let scheduler = &mut crate::percpu::current().scheduler;
    let idx = scheduler.core_task_idx[core_idx];

    if idx != usize::MAX && idx < scheduler.tasks.len() {
        Some(&mut scheduler.tasks[idx])
    } else { 
        None 
    }
}

pub fn yield_now() { unsafe { core::arch::asm!("int 0x40"); } }

pub extern "C" fn clock_task() {
    loop {
        unsafe { x86_64::instructions::hlt(); }
        crate::scheduler::yield_now();
    }
}

pub extern "C" fn background_worker() {
    loop {
        let job_opt = x86_64::instructions::interrupts::without_interrupts(|| {
            let mut queue = JOB_QUEUE.lock();
            queue.pop_front()
        });

        if let Some(job) = job_opt { job(); } 
        crate::entity::state::evolve_state();
        
        for _ in 0..10_000 { core::hint::spin_loop(); }

        unsafe { x86_64::instructions::hlt(); }
        crate::scheduler::yield_now();
    }
}