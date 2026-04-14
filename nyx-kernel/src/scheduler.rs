use alloc::vec::Vec;
use alloc::sync::Arc;
use spin::Mutex;
use core::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Ready,
    Running,
    Blocked,
    Empty,
}

pub enum FileDescriptor {
    File(Arc<crate::vfs::OpenFile>),
    Socket(Arc<Mutex<crate::scheduler::KernelSocket>>),
}

pub struct KernelSocket {
    pub kind: SocketKind,
    pub local_port: u16,
    pub remote: Option<smoltcp::wire::IpEndpoint>,
}

pub enum SocketKind {
    Udp(smoltcp::iface::SocketHandle),
}

pub struct Task {
    pub id: u64,
    pub rsp: u64,
    pub state: TaskState,
    pub fd_table: [Option<FileDescriptor>; 32],
    pub mmap_bump: u64, // 🚨 THE FIX: Dynamic memory tracker for each Task!
}

pub static CONTEXT_SWITCHES: AtomicU64 = AtomicU64::new(0);

pub struct Scheduler {
    pub tasks: Vec<Task>,
    pub core_task_idx: [usize; 32],
}

impl Scheduler {
    pub fn new() -> Self {
        let mut tasks = Vec::with_capacity(128);
        
        for i in 0..32 {
            tasks.push(Task {
                id: i as u64,
                rsp: 0,
                state: TaskState::Running,
                fd_table: core::array::from_fn(|_| None),
                mmap_bump: 0x4000_0000_0000 + (i as u64 * 0x1_0000_0000), 
            });
        }
        
        for i in 32..128 {
            tasks.push(Task {
                id: i as u64,
                rsp: 0,
                state: TaskState::Empty, 
                fd_table: core::array::from_fn(|_| None),
                mmap_bump: 0x4000_0000_0000 + (i as u64 * 0x1_0000_0000), 
            });
        }

        let mut core_task_idx = [0; 32];
        for i in 0..32 {
            core_task_idx[i] = i; 
        }

        Self {
            tasks,
            core_task_idx,
        }
    }

    pub fn schedule(&mut self, current_rsp: u64) -> u64 {
        let core_id = unsafe { crate::percpu::current().logical_id % 32 };
        let prev_idx = self.core_task_idx[core_id];
        
        self.tasks[prev_idx].rsp = current_rsp;
        if self.tasks[prev_idx].state == TaskState::Running {
            self.tasks[prev_idx].state = TaskState::Ready;
        }

        let mut next_idx = (prev_idx + 1) % self.tasks.len();
        let mut found = false;

        for _ in 0..self.tasks.len() {
            if self.tasks[next_idx].state == TaskState::Ready {
                found = true;
                break;
            }
            next_idx = (next_idx + 1) % self.tasks.len();
        }

        if !found { next_idx = prev_idx; }

        self.tasks[next_idx].state = TaskState::Running;
        self.core_task_idx[core_id] = next_idx;
        
        CONTEXT_SWITCHES.fetch_add(1, Ordering::Relaxed);
        self.tasks[next_idx].rsp
    }
}