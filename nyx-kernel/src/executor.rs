use super::task::{Task, Priority};
use alloc::collections::VecDeque;
use core::task::{Context, Poll, Waker, RawWaker, RawWakerVTable};

pub struct Executor {
    high_queue: VecDeque<Task>,
    low_queue: VecDeque<Task>,
}

impl Executor {
    pub fn new() -> Self {
        Self {
            high_queue: VecDeque::new(),
            low_queue: VecDeque::new(),
        }
    }

    pub fn spawn(&mut self, task: Task) {
        match task.priority {
            Priority::High => self.high_queue.push_back(task),
            Priority::Low => self.low_queue.push_back(task),
        }
    }

    pub fn run(&mut self) {
        loop {
            self.run_ready_tasks();
            self.sleep_if_idle();
        }
    }

    fn run_ready_tasks(&mut self) {
        // 1. DRAIN HIGH PRIORITY (Quantum)
        // Keep processing high priority tasks until the queue is empty or they all yield.
        let mut high_count = self.high_queue.len();
        while high_count > 0 {
            if let Some(mut task) = self.high_queue.pop_front() {
                let waker = dummy_waker();
                let mut context = Context::from_waker(&waker);
                
                match task.poll(&mut context) {
                    Poll::Ready(()) => {
                        // Task finished
                    },
                    Poll::Pending => {
                        // Task yielded, put back in queue
                        self.high_queue.push_back(task);
                    }
                }
            }
            high_count -= 1;
        }

        // 2. RUN ONE LOW PRIORITY (UI)
        // Only run ONE low priority task per cycle to ensure responsiveness 
        // without starving the Quantum/Hardware queue.
        if let Some(mut task) = self.low_queue.pop_front() {
            let waker = dummy_waker();
            let mut context = Context::from_waker(&waker);
            
            match task.poll(&mut context) {
                Poll::Ready(()) => {},
                Poll::Pending => {
                    self.low_queue.push_back(task);
                }
            }
        }
    }

    fn sleep_if_idle(&mut self) {
        use x86_64::instructions::interrupts;
        
        // If NO tasks are ready, sleep CPU to save power/heat
        interrupts::disable();
        if self.high_queue.is_empty() && self.low_queue.is_empty() {
            interrupts::enable_and_hlt();
        } else {
            interrupts::enable();
        }
    }
}

// --- DUMMY WAKER ---
// Required by Rust Async to create a "Context". 
// In a full implementation, this hooks into the interrupt handler to wake specific tasks.
fn dummy_waker() -> Waker {
    unsafe { Waker::from_raw(dummy_raw_waker()) }
}

fn dummy_raw_waker() -> RawWaker {
    fn no_op(_: *const ()) {}
    fn clone(_: *const ()) -> RawWaker { dummy_raw_waker() }
    
    let vtable = &RawWakerVTable::new(clone, no_op, no_op, no_op);
    RawWaker::new(0 as *const (), vtable)
}

/// Helper to allow tasks to yield control manually
pub async fn yield_now() {
    struct YieldNow { yielded: bool }
    impl core::future::Future for YieldNow {
        type Output = ();
        fn poll(mut self: core::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.yielded {
                return Poll::Ready(());
            } else {
                self.yielded = true;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
        }
    }
    YieldNow { yielded: false }.await;
}