use spin::Mutex;
use lazy_static::lazy_static;
use crate::window::{Window, WindowType, WINDOW_MANAGER};

// --- RING BUFFER ---
const QUEUE_SIZE: usize = 128; 

struct RingBuffer {
    buffer: [KeyEvent; QUEUE_SIZE],
    head: usize,
    tail: usize,
}

impl RingBuffer {
    const fn new() -> Self {
        Self { buffer: [KeyEvent::None; QUEUE_SIZE], head: 0, tail: 0 }
    }

    fn push(&mut self, event: KeyEvent) {
        let next = (self.head + 1) % QUEUE_SIZE;
        if next != self.tail {
            self.buffer[self.head] = event;
            self.head = next;
        }
    }

    fn pop(&mut self) -> Option<KeyEvent> {
        if self.head == self.tail { None } 
        else {
            let val = self.buffer[self.tail];
            self.tail = (self.tail + 1) % QUEUE_SIZE;
            Some(val)
        }
    }
}

#[derive(Clone, Copy)]
pub enum KeyEvent {
    None,
    Char(char),
    Scancode(u8),
}

lazy_static! {
    // Two Buffers: One for Hardware, One for Syscalls
    static ref INPUT_QUEUE: Mutex<RingBuffer> = Mutex::new(RingBuffer::new());
    static ref SYS_KEY_BUFFER: Mutex<RingBuffer> = Mutex::new(RingBuffer::new());
}

// 1. HARDWARE INPUT (Called by Interrupts)
pub fn handle_char(c: char) {
    if let Some(mut queue) = INPUT_QUEUE.try_lock() {
        queue.push(KeyEvent::Char(c));
    }
}

pub fn handle_key(scancode: u8) {
    if let Some(mut queue) = INPUT_QUEUE.try_lock() {
        queue.push(KeyEvent::Scancode(scancode));
    }
}

// 2. SYSCALL INPUT (Called by Userspace "read_key")
pub fn pop_char() -> Option<char> {
    if let Some(mut queue) = SYS_KEY_BUFFER.try_lock() {
        if let Some(event) = queue.pop() {
            if let KeyEvent::Char(c) = event {
                return Some(c);
            }
        }
    }
    None
}

// 3. KERNEL PROCESSOR (Called by Timer)
pub fn process_keys() {
    let mut event = None;
    
    if let Some(mut queue) = INPUT_QUEUE.try_lock() {
        event = queue.pop();
    }

    if let Some(ev) = event {
        match ev {
            KeyEvent::Char(c) => {
                // Pass to Userspace Buffer
                if let Some(mut sys_queue) = SYS_KEY_BUFFER.try_lock() {
                    sys_queue.push(KeyEvent::Char(c));
                }
            },
            KeyEvent::Scancode(code) => {
                // F1 Key to open Monitor
                if code == 0x3B { 
                    if let Some(mut wm) = WINDOW_MANAGER.try_lock() {
                        let w = wm.screen_width / 2 - 150;
                        let h = wm.screen_height / 2 - 100;
                        wm.add(Window::new(w, h, 300, 200, "System Monitor", WindowType::SystemMonitor));
                    }
                }
            },
            KeyEvent::None => {}
        }
    }
}