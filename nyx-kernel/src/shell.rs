use crate::gui::{Painter, Color, Rect};
use crate::time::DateTime; 
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
            add_to_history("  help   - Show this menu");
            add_to_history("  ver    - Show OS version");
            add_to_history("  clear  - Clear screen");
            add_to_history("  echo   - Print text");
            add_to_history("  reboot - Force Restart");
        },
        "ver" | "version" => {
            add_to_history("NyxOS Kernel v0.1.0");
            add_to_history("Scaled Graphics Engine (2x)");
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
            unsafe { perform_reboot(); }
        },
        _ => {
            add_to_history(&format!("Unknown command: '{}'", parts[0]));
        }
    }
}

// --- NUCLEAR REBOOT STRATEGY ---
unsafe fn perform_reboot() {
    use x86_64::instructions::port::Port;
    use alloc::boxed::Box;
    use x86_64::instructions::interrupts;

    // 1. DISABLE INTERRUPTS
    // Ensure the CPU isn't distracted while we try to reset
    interrupts::disable();

    // 2. METHOD A: Port 0xCF9 (PCI Hard Reset) - Best for Modern Dell
    // Write 0x06 (Reset + Transition)
    let mut pcf9 = Port::<u8>::new(0xCF9);
    pcf9.write(0x06);
    for _ in 0..10000 { core::hint::spin_loop(); }

    // 3. METHOD B: 8042 Keyboard Controller Pulse
    let mut p64 = Port::<u8>::new(0x64);
    p64.write(0xFE); 
    for _ in 0..10000 { core::hint::spin_loop(); }

    // 4. METHOD C: Port 0x92 (Fast A20)
    let mut p92 = Port::<u8>::new(0x92);
    let original = p92.read();
    p92.write(original | 1);

    // 5. METHOD D: Triple Fault
    use x86_64::structures::idt::InterruptDescriptorTable;
    // Fix: Use Box::leak to make the empty IDT live forever ('static)
    // This satisfies the compiler constraints
    let empty_idt = Box::leak(Box::new(InterruptDescriptorTable::new()));
    empty_idt.load();
    x86_64::instructions::interrupts::int3(); // CRASH!

    loop {
        x86_64::instructions::hlt();
    }
}

pub fn draw(painter: &mut impl Painter, time: &DateTime) {
    painter.clear(Color::BLACK);

    // Draw Status Bar
    painter.draw_rect(Rect::new(0, 0, painter.width(), 30), Color::BLUE);
    painter.draw_string(10, 7, "NyxOS Kernel", Color::WHITE);

    let time_str = time.to_string();
    let time_x = painter.width() - (time_str.len() * 8) - 10;
    painter.draw_string(time_x, 7, &time_str, Color::WHITE);

    // Draw History
    let history = HISTORY.lock();
    let line_height = 16;
    let start_y = 40; 
    
    for (i, line) in history.iter().enumerate() {
        painter.draw_string(10, start_y + (i * line_height), line, Color::WHITE);
    }

    // Draw Input Line
    let input_y = start_y + (history.len() * line_height);
    let input = INPUT_BUFFER.lock();
    
    painter.draw_string(10, input_y, PROMPT, Color::GREEN);
    let prompt_width = PROMPT.len() * 8; 
    painter.draw_string(10 + prompt_width, input_y, &input, Color::WHITE);

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