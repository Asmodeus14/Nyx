use core::future::Future;
use core::pin::Pin;
use alloc::boxed::Box;
use core::task::{Context, Poll};
use core::sync::atomic::{AtomicU64, Ordering};

/// Global Task ID counter
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskId(u64);

impl TaskId {
    fn new() -> Self {
        TaskId(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Priority Levels
/// High = Quantum Control / Hardware Drivers (Must run immediately)
/// Low  = UI / Shell / Background (Can wait)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    High,
    Low,
}

pub struct Task {
    pub id: TaskId,
    pub future: Pin<Box<dyn Future<Output = ()> + Send>>,
    pub priority: Priority,
}

impl Task {
    pub fn new(future: impl Future<Output = ()> + Send + 'static, priority: Priority) -> Task {
        Task {
            id: TaskId::new(),
            future: Box::pin(future),
            priority,
        }
    }

    pub fn poll(&mut self, context: &mut Context) -> Poll<()> {
        self.future.as_mut().poll(context)
    }
}