use crate::gui::{Painter, Color, Rect};
use crate::time::DateTime; // Import the Time struct
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;
use core::fmt;
use spin::Mutex;
use uart_16550::SerialPort;
use core::fmt::Write;

const PROMPT: &str = "root@nyx:~$ ";
const MAX_HISTORY: usize = 20;

lazy_static::lazy_static! {
    static ref INPUT_BUFFER: Mutex<String> = Mutex::new(String::new());
    static ref HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());

    static ref SERIAL1: Mutex<SerialPort> = {
        let mut serial_port = unsafe { SerialPort::new(0x3F8) };
        serial_port.init();
        Mutex::new(serial_port)
    };
}

pub fn handle_keyboard(c: char) {
    let mut input = INPUT_BUFFER.lock();
    
    match c {
        '\u{8}' => { input.pop(); } 
        '\n' => {
            let command = input.clone();
            add_to_history(&format!("{}{}", PROMPT, command));
            execute_command(&command);
            input.clear();
        }
        _ => { input.push(c); }
    }
}

fn add_to_history(line: &str) {
    let mut history = HISTORY.lock();
    if history.len() >= MAX_HISTORY {
        history.remove(0);
    }
    history.push(String::from(line));
}

fn execute_command(command: &str) {
    let parts: Vec<&str> = command.trim().split_whitespace().collect();
    if parts.is_empty() { return; }

    match parts[0] {
        "help" => {
            add_to_history("Available commands:");
            add_to_history("  help     - Show this menu");
            add_to_history("  ver      - Show OS version");
            add_to_history("  clear    - Clear the screen");
            add_to_history("  echo <t> - Print text");
            add_to_history("  reboot   - Restart system");
        },
        "ver" | "version" => {
            add_to_history("NyxOS Kernel v0.1.0");
            add_to_history("Double Buffered Graphics Engine");
        },
        "clear" | "cls" => {
            let mut history = HISTORY.lock();
            history.clear();
        },
        "echo" => {
            if parts.len() > 1 {
                let text = parts[1..].join(" ");
                add_to_history(&text);
            }
        },
        "reboot" => {
            add_to_history("Rebooting...");
            unsafe {
                let mut p = x86_64::instructions::port::Port::<u8>::new(0x64);
                p.write(0xFE);
            }
        },
        _ => {
            add_to_history(&format!("Unknown command: '{}'", parts[0]));
        }
    }
}

// FIX: Now accepts 'time' and draws the status bar
pub fn draw(painter: &mut impl Painter, time: &DateTime) {
    // 1. Clear Screen
    painter.clear(Color::BLACK);

    // 2. Draw Status Bar (Blue Top Bar)
    painter.draw_rect(Rect::new(0, 0, painter.width(), 30), Color::BLUE);
    
    // Draw OS Name (Left)
    painter.draw_string(10, 7, "NyxOS Kernel", Color::WHITE);

    // Draw Time (Right)
    let time_str = time.to_string();
    // Calculate position: Width - (Chars * 8px) - Margin
    let time_x = painter.width() - (time_str.len() * 8) - 10;
    painter.draw_string(time_x, 7, &time_str, Color::WHITE);

    // 3. Draw History
    let history = HISTORY.lock();
    let line_height = 16;
    let start_y = 40; // Moved down to make room for status bar
    
    for (i, line) in history.iter().enumerate() {
        painter.draw_string(10, start_y + (i * line_height), line, Color::WHITE);
    }

    // 4. Draw Input Line
    let input_y = start_y + (history.len() * line_height);
    let input = INPUT_BUFFER.lock();
    
    painter.draw_string(10, input_y, PROMPT, Color::GREEN);
    
    let prompt_width = PROMPT.len() * 8; 
    painter.draw_string(10 + prompt_width, input_y, &input, Color::WHITE);

    // Draw Cursor
    let cursor_x = 10 + prompt_width + (input.len() * 8);
    painter.draw_rect(Rect::new(cursor_x, input_y, 8, 16), Color::WHITE);
}

pub fn print_fmt(args: fmt::Arguments) {
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        let mut serial = SERIAL1.lock();
        let _ = serial.write_fmt(args);
    });
}