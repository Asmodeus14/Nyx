use crate::gui::{Painter, Color, Rect};
use crate::time::DateTime; 
use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use alloc::format;
use core::fmt;
use spin::Mutex;
use uart_16550::SerialPort;
use core::fmt::Write;
use pc_keyboard::KeyCode;
// ADDED THIS IMPORT:
use x86_64::instructions::interrupts;

const PROMPT: &str = "nyx@kernel:~$ ";
const FONT_WIDTH: usize = 16;
const FONT_HEIGHT: usize = 32;
const TOP_BAR_HEIGHT: usize = 40;
const INPUT_BAR_HEIGHT: usize = 50;
const PADDING: usize = 10;
const MAX_HISTORY_LINES: usize = 300;
const SCREEN_WIDTH_GUESS: usize = 1920; 

pub enum InputEvent { Char(char), Key(KeyCode) }

lazy_static::lazy_static! {
    static ref INPUT_QUEUE: Mutex<VecDeque<InputEvent>> = Mutex::new(VecDeque::new());
}

pub struct ShellState {
    pub input_buffer: String,
    pub visual_history: Vec<String>, 
    pub command_history: Vec<String>,
    pub cmd_history_index: usize,
    pub scroll_offset: usize,
}

impl ShellState {
    fn new() -> Self {
        Self {
            input_buffer: String::new(),
            visual_history: Vec::new(),
            command_history: Vec::new(),
            cmd_history_index: 0,
            scroll_offset: 0,
        }
    }
}

lazy_static::lazy_static! {
    static ref STATE: Mutex<ShellState> = Mutex::new(ShellState::new());
    static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = unsafe { SerialPort::new(0x3F8) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

// --- INTERRUPT HANDLERS (Called by hardware) ---
pub fn handle_char(c: char) {
    // We don't need without_interrupts here because we are ALREADY in an interrupt
    let mut queue = INPUT_QUEUE.lock();
    queue.push_back(InputEvent::Char(c));
}

pub fn handle_key(key: KeyCode) {
    let mut queue = INPUT_QUEUE.lock();
    queue.push_back(InputEvent::Key(key));
}

// --- MAIN LOOP UPDATE (Called by kernel task) ---
pub fn update() {
    let mut events = Vec::new();
    
    // CRITICAL FIX: Disable interrupts while locking the queue!
    // If we don't do this, an interrupt can fire WHILE we hold the lock, causing a deadlock.
    interrupts::without_interrupts(|| {
        let mut queue = INPUT_QUEUE.lock();
        while let Some(event) = queue.pop_front() {
            events.push(event);
        }
    });

    if events.is_empty() { return; }

    // Now that we have the data, we can process it with interrupts enabled
    let mut state = STATE.lock();
    
    for event in events {
        match event {
            InputEvent::Char(c) => {
                match c {
                    '\u{8}' => { state.input_buffer.pop(); }
                    '\n' => { 
                        let command = state.input_buffer.clone();
                        if !command.trim().is_empty() {
                            state.command_history.push(command.clone());
                            state.cmd_history_index = state.command_history.len();
                        }
                        add_log_entry(&format!("{}{}", PROMPT, command), &mut state);
                        execute_command(&command, &mut state);
                        state.input_buffer.clear();
                        state.scroll_offset = 0; 
                    }
                    _ => { state.input_buffer.push(c); }
                }
            },
            InputEvent::Key(k) => {
                match k {
                    KeyCode::ArrowUp => {
                        if state.cmd_history_index > 0 {
                            state.cmd_history_index -= 1;
                            if let Some(cmd) = state.command_history.get(state.cmd_history_index) {
                                state.input_buffer = cmd.clone();
                            }
                        }
                    },
                    KeyCode::ArrowDown => {
                        if state.cmd_history_index < state.command_history.len() {
                            state.cmd_history_index += 1;
                            if state.cmd_history_index == state.command_history.len() {
                                state.input_buffer.clear();
                            } else if let Some(cmd) = state.command_history.get(state.cmd_history_index) {
                                state.input_buffer = cmd.clone();
                            }
                        }
                    },
                    KeyCode::PageUp => { state.scroll_offset += 1; },
                    KeyCode::PageDown => { if state.scroll_offset > 0 { state.scroll_offset -= 1; } },
                    _ => {}
                }
            }
        }
    }
}

fn add_log_entry(text: &str, state: &mut ShellState) {
    let max_text_width = SCREEN_WIDTH_GUESS - (PADDING * 2);
    let wrapped_lines = wrap_text(text, max_text_width);
    for line in wrapped_lines.into_iter().rev() {
        state.visual_history.insert(0, line);
    }
    while state.visual_history.len() > MAX_HISTORY_LINES {
        state.visual_history.pop();
    }
}

fn execute_command(command: &str, state: &mut ShellState) {
    let parts: Vec<&str> = command.trim().split_whitespace().collect();
    if parts.is_empty() { return; }

    match parts[0] {
        "help" => {
            add_log_entry("  lsusb    - Init USB 3.0", state);
            add_log_entry("  lspci    - Scan Bus", state);
            add_log_entry("  reboot   - Restart", state);
            add_log_entry("  clear    - Clear", state);
            add_log_entry("NyxOS v0.5.0 (Stable Input)", state);
        },
        "clear" => { state.visual_history.clear(); },
        "lsusb" => {
            add_log_entry("Initializing XHCI Driver...", state);
            let mut xhci = crate::xhci::XhciDriver::new();
            match xhci.init() {
                Ok(msg) => {
                    add_log_entry(&format!("SUCCESS: {}", msg), state);
                },
                Err(e) => {
                    add_log_entry(&format!("ERROR: {}", e), state);
                    add_log_entry("Try: qemu-system-x86_64 ... -device qemu-xhci", state);
                }
            }
        },
        "lspci" => {
            let mut pci = crate::pci::PciDriver::new();
            let devices = pci.scan();
            for dev in devices {
                add_log_entry(&dev.to_string(), state);
            }
        },
        "reboot" => {
            add_log_entry("Rebooting...", state);
            unsafe { perform_reboot(); }
        },
        _ => {
            add_log_entry(&format!("Unknown: '{}'", parts[0]), state);
        }
    }
}

unsafe fn perform_reboot() {
    use x86_64::instructions::port::Port;
    let mut p64 = Port::<u8>::new(0x64);
    p64.write(0xFE); 
    loop { x86_64::instructions::hlt(); }
}

fn wrap_text(text: &str, max_width_px: usize) -> Vec<String> {
    let max_chars = max_width_px / FONT_WIDTH;
    let mut lines = Vec::new();
    let mut current_line = String::new();
    for char in text.chars() {
        if current_line.len() >= max_chars {
            lines.push(current_line);
            current_line = String::new();
        }
        current_line.push(char);
    }
    if !current_line.is_empty() { lines.push(current_line); }
    lines
}

pub fn draw(painter: &mut impl Painter, time: &DateTime) {
    let width = painter.width();
    let height = painter.height();

    painter.clear(Color::new(10, 15, 20));

    // Status Bar
    painter.draw_rect(Rect::new(0, 0, width, TOP_BAR_HEIGHT), Color::new(30, 35, 40));
    painter.draw_string(20, 8, "NyxOS Kernel", Color::CYAN);
    let time_str = time.to_string();
    let time_x = width - (time_str.len() * FONT_WIDTH) - 20;
    painter.draw_string(time_x, 8, &time_str, Color::WHITE);

    // Input Deck
    let input_y = TOP_BAR_HEIGHT;
    painter.draw_rect(Rect::new(0, input_y, width, INPUT_BAR_HEIGHT), Color::new(40, 45, 50));
    
    let state = STATE.lock();
    let prompt_x = 20;
    let input_text_y = input_y + 10;
    
    painter.draw_string(prompt_x, input_text_y, PROMPT, Color::GREEN);
    let prompt_w = PROMPT.len() * FONT_WIDTH;
    painter.draw_string(prompt_x + prompt_w, input_text_y, &state.input_buffer, Color::WHITE);
    
    if (time.second % 2) == 0 {
        let cursor_x = prompt_x + prompt_w + (state.input_buffer.len() * FONT_WIDTH);
        painter.draw_rect(Rect::new(cursor_x, input_text_y + 4, 12, 24), Color::WHITE);
    }

    // Console Output
    let output_start_y = TOP_BAR_HEIGHT + INPUT_BAR_HEIGHT + PADDING;
    let mut current_y = output_start_y;
    let mut lines_skipped = 0;

    for line in &state.visual_history {
        if lines_skipped < state.scroll_offset {
            lines_skipped += 1;
            continue;
        }
        if current_y + FONT_HEIGHT > height { break; }

        painter.draw_string(PADDING, current_y, line, Color::new(200, 200, 200));
        current_y += FONT_HEIGHT;
    }
    
    if state.scroll_offset > 0 {
        painter.draw_string(width - 30, output_start_y, "^", Color::YELLOW);
    }
}

pub fn print_fmt(args: fmt::Arguments) {
    // This is safe because it uses spinlock internally for serial, but without_interrupts is good practice
    interrupts::without_interrupts(|| {
        let mut serial = SERIAL1.lock();
        let _ = serial.write_fmt(args);
    });
}