use spin::Mutex;
use x86_64::instructions::port::Port;
use lazy_static::lazy_static;

const PS2_CMD_PORT: u16 = 0x64;
const PS2_DATA_PORT: u16 = 0x60;

pub struct MouseState {
    pub x: usize,
    pub y: usize,
    pub left_click: bool,
    pub right_click: bool,
    pub middle_click: bool,
    pub screen_width: usize,
    pub screen_height: usize,
}

lazy_static! {
    pub static ref MOUSE_STATE: Mutex<MouseState> = Mutex::new(MouseState {
        x: 512, y: 384, 
        left_click: false, right_click: false, middle_click: false,
        screen_width: 1024, screen_height: 768, 
    });
}

pub struct MouseDriver {
    command_port: Port<u8>,
    data_port: Port<u8>,
    cycle: u8,
    packet: [u8; 3], 
}

impl MouseDriver {
    pub fn new() -> Self {
        Self {
            command_port: Port::new(PS2_CMD_PORT),
            data_port: Port::new(PS2_DATA_PORT),
            cycle: 0,
            packet: [0; 3],
        }
    }

    unsafe fn wait_for_write(&mut self) {
        for _ in 0..10000 { if (self.command_port.read() & 0x02) == 0 { return; } }
    }

    unsafe fn wait_for_read(&mut self) {
        for _ in 0..10000 { if (self.command_port.read() & 0x01) == 1 { return; } }
    }

    pub fn init(&mut self) {
        unsafe {
            self.wait_for_write(); self.command_port.write(0xA8);
            self.wait_for_write(); self.command_port.write(0x20);
            self.wait_for_read();
            let mut status = self.data_port.read();
            status |= 0x02; status &= !0x20;
            self.wait_for_write(); self.command_port.write(0x60);
            self.wait_for_write(); self.data_port.write(status);
            
            // ⚠️⚠️ THE KEYBOARD TYPEMATIC HANDSHAKE USED TO LIVE HERE, AND IT KILLED THE TOUCHPAD.
            //
            // Setting the repeat rate (0xF3, 0x00 -> 250 ms delay / 30 cps) is a genuine
            // improvement — the 8042 powers up at its SLOWEST setting, 500 ms then ~10.9 cps — but
            // two attempts at it both left the pointer dead on real hardware, and it is not worth a
            // dead pointer on a laptop.
            //
            // First attempt put it AFTER `0xF4` ("enable data reporting"), so the mouse was already
            // streaming 3-byte packets into the same output buffer the handshake reads from.
            // `wait_for_read` only tests status bit 0 (output buffer full), never bit 5 (AUX data),
            // so it cannot tell a keyboard ACK from a mouse byte and the blind reads desynchronised
            // the packet state machine. Moving it BEFORE `0xF6`/`0xF4` did NOT fix it, so that
            // explanation was incomplete at best.
            //
            // ⚠️ QEMU reproduces NEITHER failure — its 8042 tolerates the interleaving and clicks
            // keep working — so this cannot be developed here. Reintroducing it needs, at minimum:
            // drain the output buffer first, then VERIFY each response is 0xFA instead of reading
            // blindly, and treat a missing ACK as "skip the whole thing" rather than continuing.
            // Until that can be tested against the real controller, the pointer wins.

            // 🚨 THE FIX: 0xF6 (Set Defaults) instead of 0xFF (Reset).
            // This prevents the hardware from flooding the buffer with 3 bytes and breaking the packet cycle!
            self.write_mouse(0xF6);
            self.wait_for_read(); let _ = self.data_port.read();

            self.write_mouse(0xF4);
            self.wait_for_read(); let _ = self.data_port.read();
        }
    }

    unsafe fn write_mouse(&mut self, byte: u8) {
        self.wait_for_write(); self.command_port.write(0xD4);
        self.wait_for_write(); self.data_port.write(byte);
    }
}

pub fn update_from_usb(dx: i8, dy: i8, buttons: u8) {
    update_relative(dx as i32, dy as i32, buttons);
}

/// Move the pointer by a relative delta in SCREEN convention (positive dy = down) and set the
/// buttons (bit 0 left, bit 1 right, bit 2 middle). Shared by USB HID and the I2C-HID touchpad.
pub fn update_relative(dx: i32, dy: i32, buttons: u8) {
    let (nx, ny) = {
        let mut state = MOUSE_STATE.lock();
        let new_x = state.x as i64 + (dx as i64);
        let new_y = state.y as i64 + (dy as i64);
        state.x = new_x.clamp(0, (state.screen_width - 1) as i64) as usize;
        state.y = new_y.clamp(0, (state.screen_height - 1) as i64) as usize;
        state.left_click = (buttons & 0x01) != 0;
        state.right_click = (buttons & 0x02) != 0;
        state.middle_click = (buttons & 0x04) != 0;
        (state.x, state.y)
    };
    // Drive the hardware cursor plane directly (no-op if it isn't enabled). Lockless MMIO.
    crate::drivers::gpu::intel::cursor::move_to(nx, ny);
}

pub fn handle_interrupt(packet_byte: u8) {
    // ★ Phase 4 of the I2C-HID touchpad: while it drives the pointer, PS/2 AUX bytes are dropped
    // (the caller has already read port 0x60, so the 8042 is not left holding them). A touchpad
    // that still reports on both paths would otherwise move the cursor twice. The flag falls back
    // to false on its own if the I2C side goes quiet — see `i2c_hid::poll`.
    if crate::drivers::i2c_hid::POINTER_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    static mut DRIVER_STATE: Option<MouseDriver> = None;
    unsafe {
        if DRIVER_STATE.is_none() { DRIVER_STATE = Some(MouseDriver::new()); }
        let driver = DRIVER_STATE.as_mut().unwrap();

        match driver.cycle {
            0 => { if (packet_byte & 0x08) != 0 { driver.packet[0] = packet_byte; driver.cycle += 1; } }
            1 => { driver.packet[1] = packet_byte; driver.cycle += 1; }
            2 => {
                driver.packet[2] = packet_byte;
                let flags = driver.packet[0];
                let rel_x = if (flags & 0x10) != 0 { (driver.packet[1] as i16) - 256 } else { driver.packet[1] as i16 };
                let rel_y = if (flags & 0x20) != 0 { (driver.packet[2] as i16) - 256 } else { driver.packet[2] as i16 };

                let mut state = MOUSE_STATE.lock();
                let multiplier = 2; // Increase mouse sensitivity!
                let new_x = state.x as i32 + (rel_x as i32 * multiplier);
                let new_y = state.y as i32 - (rel_y as i32 * multiplier);

                state.x = new_x.clamp(0, state.screen_width as i32 - 1) as usize;
                state.y = new_y.clamp(0, state.screen_height as i32 - 1) as usize;
                state.left_click = (flags & 0x01) != 0;
                state.right_click = (flags & 0x02) != 0;
                let (nx, ny) = (state.x, state.y);
                drop(state);
                driver.cycle = 0;

                // Drive the hardware cursor plane straight from the IRQ (no-op if disabled). This is
                // the zero-latency, zero-redraw path: a single CURPOS+CURBASE MMIO write, no locks.
                crate::drivers::gpu::intel::cursor::move_to(nx, ny);
            }
            _ => driver.cycle = 0,
        }
    }
}