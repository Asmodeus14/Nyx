#![no_std]
#![no_main]

mod syscalls;
mod console;

use core::str;

#[link_section = ".text.entry"]
#[no_mangle]
pub extern "C" fn _start() -> ! {
    println!("Welcome to NyxOS v0.3 (GUI Mode)");
    println!("Type 'help' for commands.");
    print!("nyx> ");

    let mut buffer = [0u8; 64];
    let mut len = 0;

    loop {
        if let Some(c) = syscalls::read_key() {
            match c {
                '\n' => {
                    print!("\n");
                    if let Ok(cmd_str) = str::from_utf8(&buffer[..len]) {
                        process_command(cmd_str);
                    }
                    len = 0;
                    print!("nyx> ");
                },
                '\x08' => {
                    if len > 0 { 
                        len -= 1; 
                        // Backspace visual hack (move back, overwrite space, move back)
                        print!("\x08 \x08"); 
                    }
                },
                _ => {
                    if len < buffer.len() {
                        buffer[len] = c as u8;
                        len += 1;
                        // FIX: Echo the character back to the screen!
                        print!("{}", c);
                    }
                }
            }
        }
        // Small delay to prevent 100% CPU usage in loop
        for _ in 0..100 { unsafe { core::arch::asm!("nop"); } }
    }
}

fn process_command(cmd: &str) {
    let cmd = cmd.trim();
    if cmd.len() == 0 { return; }
    
    let (command, args) = match cmd.find(' ') {
        Some(index) => (&cmd[..index], &cmd[index+1..]),
        None => (cmd, ""),
    };

    match command {
        "help" => {
            println!("Commands: ver, echo, clear, exit");
        },
        "ver" => {
            println!("NyxOS v0.3 - GUI Edition");
        },
        "echo" => {
            println!("{}", args);
        },
        "clear" => {
            // "Clear" by pushing text up
            for _ in 0..15 { print!("\n"); }
        },
        "exit" => {
            syscalls::exit(0);
        },
        _ => {
            print!("Unknown: '{}'\n", command);
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscalls::exit(1);
}