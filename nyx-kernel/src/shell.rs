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
                        // Multimedia keys. Extended-set scancodes on the same path as the arrows —
                        // not new machinery, simply not decoded until now. There are no brightness
                        // equivalents: those are handled by the EC and never reach the 8042.
                        KeyCode::VolumeDown => Some('\u{E01C}'),
                        KeyCode::VolumeUp   => Some('\u{E01D}'),
                        KeyCode::Mute       => Some('\u{E01E}'),
                        // The function row, F1..F12 → E020..E02B, contiguous so userspace can do
                        // arithmetic on it. `pc_keyboard` has decoded these all along; we dropped
                        // them, which is why no Nyx app has ever seen an F-key.
                        //
                        // This is also the only reachable route to panel brightness. The keycap
                        // brightness symbols are `Fn` chords that the EC eats before the 8042 sees
                        // anything; the bare F-key underneath them arrives normally. The shell binds
                        // a pair and synthesises E01A/E01B from it.
                        KeyCode::F1  => Some('\u{E020}'),
                        KeyCode::F2  => Some('\u{E021}'),
                        KeyCode::F3  => Some('\u{E022}'),
                        KeyCode::F4  => Some('\u{E023}'),
                        KeyCode::F5  => Some('\u{E024}'),
                        KeyCode::F6  => Some('\u{E025}'),
                        KeyCode::F7  => Some('\u{E026}'),
                        KeyCode::F8  => Some('\u{E027}'),
                        KeyCode::F9  => Some('\u{E028}'),
                        KeyCode::F10 => Some('\u{E029}'),
                        KeyCode::F11 => Some('\u{E02A}'),
                        KeyCode::F12 => Some('\u{E02B}'),
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