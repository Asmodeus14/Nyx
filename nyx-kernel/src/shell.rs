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

/// Translate one USB HID boot-protocol keypress into the same char stream the 8042 path produces.
///
/// `usage` is a HID Keyboard/Keypad page (0x07) usage ID; `modifiers` is byte 0 of the boot report.
///
/// ★ Deliberately lands in the SAME `KEY_QUEUE` and uses the SAME Private-Use-Area encoding as
/// `handle_key` above. Userspace decodes these via `nyx_api::keys::*`, so a USB keyboard and the
/// built-in 8042 one must be indistinguishable above this line — otherwise every app would need to
/// know which keyboard a keystroke came from.
///
/// ⚠️ Ctrl and Alt are dropped, matching `HandleControl::Ignore` on the 8042 path. Nyx has no
/// modifier chords anywhere (that is why Notepad saves with F2), and quietly introducing them on
/// one input device only would be worse than not having them: apps have no way to express or
/// receive one. Shift IS honoured, because it produces ordinary characters.
pub fn handle_hid_key(usage: u8, modifiers: u8) {
    const SHIFT: u8 = 0b0010_0010; // bit1 = LeftShift, bit5 = RightShift
    let shift = (modifiers & SHIFT) != 0;

    let c = match usage {
        // a-z. HID gives position, not case; Shift decides.
        0x04..=0x1D => {
            let base = b'a' + (usage - 0x04);
            Some(if shift { base.to_ascii_uppercase() as char } else { base as char })
        }
        // 1-9 then 0 — note 0 sorts AFTER 9 in HID, which is why it is a separate arm.
        0x1E..=0x26 => {
            let n = usage - 0x1E; // 0..8 => '1'..'9'
            Some(if shift {
                [')', '!', '@', '#', '$', '%', '^', '&', '*', '('][(n + 1) as usize]
            } else {
                (b'1' + n) as char
            })
        }
        0x27 => Some(if shift { ')' } else { '0' }),

        0x28 => Some('\n'),       // Enter
        0x2A => Some('\u{8}'),    // Backspace
        0x2B => Some('\t'),       // Tab
        0x2C => Some(' '),        // Space

        0x2D => Some(if shift { '_' } else { '-' }),
        0x2E => Some(if shift { '+' } else { '=' }),
        0x2F => Some(if shift { '{' } else { '[' }),
        0x30 => Some(if shift { '}' } else { ']' }),
        0x31 => Some(if shift { '|' } else { '\\' }),
        0x33 => Some(if shift { ':' } else { ';' }),
        0x34 => Some(if shift { '"' } else { '\'' }),
        0x35 => Some(if shift { '~' } else { '`' }),
        0x36 => Some(if shift { '<' } else { ',' }),
        0x37 => Some(if shift { '>' } else { '.' }),
        0x38 => Some(if shift { '?' } else { '/' }),

        // F1..F12 -> E020..E02B, contiguous, matching `handle_key`.
        0x3A..=0x45 => char::from_u32(0xE020 + (usage - 0x3A) as u32),

        // Navigation. Same PUA values as the 8042 path — these MUST match or an app would see a
        // different arrow key depending on which keyboard produced it.
        0x4A => Some('\u{E014}'), // Home
        0x4B => Some('\u{E017}'), // PageUp
        0x4C => Some('\u{E016}'), // Delete
        0x4D => Some('\u{E015}'), // End
        0x4E => Some('\u{E018}'), // PageDown
        0x4F => Some('\u{E011}'), // Right
        0x50 => Some('\u{E010}'), // Left
        0x51 => Some('\u{E013}'), // Down
        0x52 => Some('\u{E012}'), // Up

        _ => None,
    };

    if let Some(c) = c {
        push_key(c);
    }
}

/// The Super/Windows key, reported in the modifier byte rather than as a usage ID.
///
/// Separate from `handle_hid_key` because it has no usage code of its own — it is bit 3 / bit 7 of
/// the modifier byte, so it can only be detected by diffing modifiers between reports.
pub fn handle_hid_super() {
    push_key('\u{E019}');
}

/// Queue one keystroke from a kernel TASK (USB HID, touchpad gestures).
///
/// ⚠️ Interrupts masked while KEY_QUEUE is held: the keyboard IRQ takes this same lock, and a task
/// holding it when that IRQ lands on the same core would leave the handler spinning forever.
/// `handle_key` itself runs inside the IRQ and needs none of this.
pub fn push_key(c: char) {
    x86_64::instructions::interrupts::without_interrupts(|| KEY_QUEUE.lock().push_back(c));
}