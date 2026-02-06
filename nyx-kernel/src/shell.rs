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
use x86_64::instructions::interrupts;

const PROMPT: &str = "nyx@phase1:~$ ";
const FONT_WIDTH: usize = 16;
const FONT_HEIGHT: usize = 32;
const TOP_BAR_HEIGHT: usize = 40;
const INPUT_BAR_HEIGHT: usize = 50;
const PADDING: usize = 10;
const MAX_HISTORY_LINES: usize = 300;
const SCREEN_WIDTH_GUESS: usize = 1920; 

// --- DATA STRUCTURES ---
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

// --- INTERRUPT HANDLERS ---
pub fn handle_char(c: char) {
    let mut queue = INPUT_QUEUE.lock();
    queue.push_back(InputEvent::Char(c));
}

pub fn handle_key(key: KeyCode) {
    let mut queue = INPUT_QUEUE.lock();
    queue.push_back(InputEvent::Key(key));
}

// --- MAIN LOOP ---
pub fn update() {
    let mut events = Vec::new();
    
    // 1. Drain Queue (Atomic)
    interrupts::without_interrupts(|| {
        let mut queue = INPUT_QUEUE.lock();
        while let Some(event) = queue.pop_front() {
            events.push(event);
        }
    });

    if events.is_empty() { return; }

    // 2. Process Logic
    let mut state = STATE.lock();
    
    for event in events {
        match event {
            InputEvent::Char(c) => {
                match c {
                    '\u{8}' => { state.input_buffer.pop(); } // Backspace
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
                    KeyCode::PageUp => { state.scroll_offset += 1; },
                    KeyCode::PageDown => { if state.scroll_offset > 0 { state.scroll_offset -= 1; } },
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
                    _ => {}
                }
            }
        }
    }
}

// --- COMMAND EXECUTION ---
fn execute_command(command: &str, state: &mut ShellState) {
    let parts: Vec<&str> = command.trim().split_whitespace().collect();
    if parts.is_empty() { return; }

    match parts[0] {
        "help" => {
            add_log_entry("NyxOS Phase 1 Commands:", state);
            add_log_entry("  uptime   - Show System Ticks & Time", state);
            add_log_entry("  lspci    - Scan PCI Bus", state);
            add_log_entry("  sysinfo  - Kernel Stats", state);
            add_log_entry("  reboot   - Restart System (Multiple Methods)", state);
            add_log_entry("  shutdown - Power Off (ACPI/Halt)", state);
            add_log_entry("  panic    - Test Crash Handler", state);
            add_log_entry("  clear    - Clear Screen", state);
        },
        "clear" => { state.visual_history.clear(); },
        "uptime" => {
            let ticks = crate::time::get_ticks();
            let secs = crate::time::uptime_seconds();
            add_log_entry(&format!("Ticks: {} (approx {:.2}s)", ticks, secs), state);
        },
        "sysinfo" => {
            add_log_entry("NyxOS Kernel v0.8.2 (Phase 1)", state);
            add_log_entry("Scheduler: Async Cooperative", state);
            add_log_entry("Timer: PIT @ 100Hz", state);
            add_log_entry("Input: PS/2 Ring-0 Only", state);
        },
        "lspci" => {
            add_log_entry("Enumerating PCI Bus...", state);
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
        "shutdown" => {
            add_log_entry("Attempting shutdown...", state);
            unsafe { perform_shutdown(); }
        },
        "panic" => {
            panic!("Manual Panic via Shell");
        },
        "echo" => {
            if parts.len() > 1 {
                add_log_entry(&parts[1..].join(" "), state);
            }
        },
        _ => {
            add_log_entry(&format!("Unknown: '{}'. Type 'help'.", parts[0]), state);
        }
    }
}

// --- UTILITIES ---
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

// --- SYSTEM CONTROL FUNCTIONS (UPDATED) ---

unsafe fn perform_reboot() {
    use x86_64::instructions::port::Port;
    
    // Method 1: PS/2 Keyboard Controller Pulse (Legacy)
    // Writing 0xFE to port 0x64 pulses the CPU reset line.
    let mut p64 = Port::<u8>::new(0x64);
    p64.write(0xFE); 
    
    // Method 2: "Fast A20" Gate (System Control Port A)
    // Bit 0 = Fast Reset. This is very effective on modern chipsets.
    let mut p92 = Port::<u8>::new(0x92);
    let val = p92.read();
    p92.write(val | 1);

    // Method 3: ACPI Reset (0xCF9)
    // Write 0x06 (Reset) to PCI Configuration Access Port
    let mut pcf9 = Port::<u8>::new(0xCF9);
    pcf9.write(0x06);

    // Method 4: Triple Fault (The Nuclear Option)
    use x86_64::structures::idt::InterruptDescriptorTable;
    let empty_idt = alloc::boxed::Box::leak(alloc::boxed::Box::new(InterruptDescriptorTable::new()));
    empty_idt.load();
    x86_64::instructions::interrupts::int3(); 

    // If we are still here, the hardware is stubborn.
    loop { x86_64::instructions::hlt(); }
}

unsafe fn perform_shutdown() {
    use x86_64::instructions::port::Port;
    
    // 1. QEMU / Bochs Shutdown Hack
    // This only works in emulators. Real hardware ignores port 0x604.
    let mut port = Port::<u16>::new(0x604);
    port.write(0x2000);
    
    // 2. Newer QEMU / VirtualBox Shutdown
    let mut port_new = Port::<u16>::new(0xCF9);
    port_new.write(0x08);

    // 3. Real Hardware Fallback
    // Since we don't have an ACPI driver yet, we can't power off a real PC.
    // The safest thing to do is disable interrupts and halt the CPU.
    x86_64::instructions::interrupts::disable();
    loop { 
        x86_64::instructions::hlt(); 
    }
}

pub fn print_fmt(args: fmt::Arguments) {
    interrupts::without_interrupts(|| {
        let mut serial = SERIAL1.lock();
        let _ = serial.write_fmt(args);
    });
}

// --- RENDERING ---
pub fn draw(painter: &mut impl Painter, time: &DateTime) {
    let width = painter.width();
    let height = painter.height();

    painter.clear(Color::new(10, 15, 20));

    // Top Bar
    painter.draw_rect(Rect::new(0, 0, width, TOP_BAR_HEIGHT), Color::new(30, 35, 40));
    painter.draw_string(20, 8, "NyxOS Kernel", Color::CYAN);
    let time_str = time.to_string();
    let time_x = width - (time_str.len() * FONT_WIDTH) - 20;
    painter.draw_string(time_x, 8, &time_str, Color::WHITE);

    // Input Deck
    let input_y = TOP_BAR_HEIGHT;
    painter.draw_rect(Rect::new(0, input_y, width, INPUT_BAR_HEIGHT), Color::new(40, 45, 50));
    
    let state = STATE.lock();
    painter.draw_string(20, input_y + 10, PROMPT, Color::GREEN);
    let prompt_w = PROMPT.len() * FONT_WIDTH;
    painter.draw_string(20 + prompt_w, input_y + 10, &state.input_buffer, Color::WHITE);
    
    // Cursor
    if (time.second % 2) == 0 {
        let cursor_x = 20 + prompt_w + (state.input_buffer.len() * FONT_WIDTH);
        painter.draw_rect(Rect::new(cursor_x, input_y + 14, 12, 24), Color::WHITE);
    }

    // Console History
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
}