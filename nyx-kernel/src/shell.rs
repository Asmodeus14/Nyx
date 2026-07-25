use pc_keyboard::{layouts, DecodedKey, HandleControl, KeyCode, Keyboard, ScancodeSet1};
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
                // The pc_keyboard crate decodes navigation/editing keys (arrows, Home/End, Delete,
                // PageUp/Down) as RawKey, which carry no Unicode. We used to drop them, so userspace
                // apps could never see them. Instead, encode each as a Unicode Private-Use-Area char
                // and push it through the SAME char queue — so it flows through pop_key(506) →
                // MSG_KEY_EVENT → NyxApp::on_key(char) untouched, no protocol change. Apps decode
                // these via nyx_api::keys::* (libs/api/src/lib.rs), whose values MUST match these.
                DecodedKey::RawKey(code) => {
                    let mapped = match code {
                        KeyCode::ArrowLeft  => Some('\u{E010}'),
                        KeyCode::ArrowRight => Some('\u{E011}'),
                        KeyCode::ArrowUp    => Some('\u{E012}'),
                        KeyCode::ArrowDown  => Some('\u{E013}'),
                        KeyCode::Home       => Some('\u{E014}'),
                        KeyCode::End        => Some('\u{E015}'),
                        KeyCode::Delete     => Some('\u{E016}'),
                        KeyCode::PageUp     => Some('\u{E017}'),
                        KeyCode::PageDown   => Some('\u{E018}'),
                        // Either Windows/Super key. Also Unicode-less; the compositor swallows this
                        // one to toggle the start menu rather than forwarding it to the focused app.
                        KeyCode::LWin | KeyCode::RWin => Some('\u{E019}'),
                        _ => None,
                    };
                    if let Some(c) = mapped {
                        KEY_QUEUE.lock().push_back(c);
                    }
                },
            }
        }
    }
}

pub fn pop_key() -> Option<char> {
    KEY_QUEUE.lock().pop_front()
}