use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
use spin::Mutex;
use alloc::collections::vec_deque::VecDeque;
use lazy_static::lazy_static;

lazy_static! {
    // Queue for keys waiting to be read by User Space
    pub static ref KEY_QUEUE: Mutex<VecDeque<char>> = Mutex::new(VecDeque::new());
}

pub fn handle_key(scancode: u8) {
    lazy_static! {
        static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> =
            Mutex::new(Keyboard::new(ScancodeSet1::new(), layouts::Us104Key, HandleControl::Ignore));
    }

    let mut keyboard = KEYBOARD.lock();
    if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
        if let Some(key) = keyboard.process_keyevent(key_event) {
            match key {
                DecodedKey::Unicode(character) => {
                    // Push to queue for Syscalls
                    KEY_QUEUE.lock().push_back(character);
                },
                DecodedKey::RawKey(_) => {},
            }
        }
    }
}

pub fn pop_key() -> Option<char> {
    KEY_QUEUE.lock().pop_front()
}