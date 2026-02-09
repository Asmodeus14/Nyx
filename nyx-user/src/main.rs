#![no_std]
#![no_main]

mod syscalls;
mod console;

use core::fmt::Write;

struct ConsoleWriter;
impl core::fmt::Write for ConsoleWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() { syscalls::sys_print(c); }
        Ok(())
    }
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    let mut console = ConsoleWriter;
    let _ = write!(console, "\n[USER] Mouse Tracker Started.\n");
    let _ = write!(console, "Move mouse to see coordinates...\n\n");

    let mut prev_x = 0;
    let mut prev_y = 0;

    loop {
        // 1. Get Mouse from Kernel via Syscall 3
        let (x, y) = syscalls::sys_get_mouse();

        // 2. Only print if changed
        if x != prev_x || y != prev_y {
            // \r returns cursor to start of line to overwrite previous coords
            let _ = write!(console, "\rMouse: X={:04} Y={:04}   ", x, y);
            prev_x = x;
            prev_y = y;
        }

        // 3. Busy wait slightly to prevent flooding syscalls
        for _ in 0..100_000 { unsafe { core::arch::asm!("nop"); } }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscalls::sys_exit(1);
}