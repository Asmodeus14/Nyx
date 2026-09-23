use spin::Mutex;
use x86_64::instructions::port::Port;
use lazy_static::lazy_static;

/// How the keyboard repeat-rate setup went at boot — the `keyboard` command reports it.
pub static TYPEMATIC: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
pub const TYPEMATIC_NOT_TRIED: u8 = 0;
pub const TYPEMATIC_OK: u8 = 1;
/// The keyboard did not acknowledge 0xF3 (set typematic): nothing changed.
pub const TYPEMATIC_NO_ACK_CMD: u8 = 2;
/// It acknowledged 0xF3 but not the rate byte: re-enabled, rate unchanged.
pub const TYPEMATIC_NO_ACK_RATE: u8 = 3;

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
            
            // ★ Keyboard repeat rate: 250 ms delay, 30 characters/s (0xF3, 0x00). The 8042 powers
            //   up at its SLOWEST setting — 500 ms, then ~10.9 cps — which is most of why held keys
            //   felt sluggish.
            //
            // ⚠️⚠️ Two earlier attempts killed the (then PS/2) touchpad on the hardware. Both read
            // the controller's replies BLINDLY: one byte after each command, whatever it was. The
            // output buffer can hold stale bytes from the firmware at this point, so one stale byte
            // shifts every "ACK" after it by one — and the mouse's own 0xFA ACK is then left behind
            // for the packet decoder, where 0xFA (bit 3 set) looks like a packet start. Moving the
            // handshake before 0xF6/0xF4 could not fix that, which is what was observed.
            //
            // This version drains first, VERIFIES each reply is 0xFA (discarding any byte the
            // status register marks as AUX), gives up on a missing ACK instead of reading on, and
            // drains again so the mouse sequence below — unchanged from the known-good version —
            // starts from an empty buffer. Interrupts are still off here (main.rs enables them
            // after this), so no handler can take a reply first. The outcome is recorded in
            // TYPEMATIC for the `keyboard` command: QEMU's 8042 cannot reproduce the old failure,
            // so the hardware has to be able to say how this went.
            self.drain();
            let result = if !self.kbd_command(0xF3) {
                TYPEMATIC_NO_ACK_CMD
            } else if !self.kbd_command(0x00) {
                // The keyboard took 0xF3 and is waiting for a parameter it did not acknowledge.
                // "Enable" ends that state rather than leaving it to swallow the next byte.
                let _ = self.kbd_command(0xF4);
                TYPEMATIC_NO_ACK_RATE
            } else {
                TYPEMATIC_OK
            };
            TYPEMATIC.store(result, core::sync::atomic::Ordering::Relaxed);
            self.drain();

            // 🚨 THE FIX: 0xF6 (Set Defaults) instead of 0xFF (Reset).
            // This prevents the hardware from flooding the buffer with 3 bytes and breaking the packet cycle!
            self.write_mouse(0xF6);
            self.wait_for_read(); let _ = self.data_port.read();

            self.write_mouse(0xF4);
            self.wait_for_read(); let _ = self.data_port.read();
        }
    }

    /// Empty the controller's output buffer. Bounded: a stuck status bit must not hang boot.
    unsafe fn drain(&mut self) {
        for _ in 0..32 {
            if self.command_port.read() & 0x01 == 0 {
                return;
            }
            let _ = self.data_port.read();
        }
    }

    /// Send one byte to the KEYBOARD and wait for its reply. True only for a genuine 0xFA ACK.
    ///
    /// A byte the status register marks as AUX (bit 5) came from the mouse port — discarded, and the
    /// wait continues. 0xFE (resend) or anything else is a failure. 20 ms is far longer than any
    /// keyboard needs, and bounds the wait if there is no keyboard at all.
    unsafe fn kbd_command(&mut self, byte: u8) -> bool {
        self.wait_for_write();
        self.data_port.write(byte);
        let dl = crate::drivers::gpu::intel::render::SpinDeadline::new(20_000);
        loop {
            let status = self.command_port.read();
            if status & 0x01 != 0 {
                let b = self.data_port.read();
                if status & 0x20 != 0 {
                    continue; // mouse data, not our reply
                }
                return b == 0xFA;
            }
            if dl.expired() {
                return false;
            }
            core::hint::spin_loop();
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

/// The screen width the pointer is clamped to, for drivers that scale absolute device units to
/// pixels (the precision touchpad). Interrupts masked while the lock is held — see
/// [`update_relative`].
pub fn screen_width() -> usize {
    x86_64::instructions::interrupts::without_interrupts(|| MOUSE_STATE.lock().screen_width)
}

/// Move the pointer by a relative delta in SCREEN convention (positive dy = down) and set the
/// buttons (bit 0 left, bit 1 right, bit 2 middle). Shared by USB HID and the I2C-HID touchpad.
pub fn update_relative(dx: i32, dy: i32, buttons: u8) {
    // ⚠️ Interrupts masked while MOUSE_STATE is held. Callers are kernel TASKS (IF=1) — the USB and
    // I2C-HID pollers — and syscall 505 takes this same lock at IF=0. A task preempted while holding
    // it would leave that syscall spinning on its core forever: the preemption-boundary deadlock.
    // The PS/2 path never had this problem only because it runs inside its IRQ handler.
    let (nx, ny) = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut state = MOUSE_STATE.lock();
        let new_x = state.x as i64 + (dx as i64);
        let new_y = state.y as i64 + (dy as i64);
        state.x = new_x.clamp(0, (state.screen_width - 1) as i64) as usize;
        state.y = new_y.clamp(0, (state.screen_height - 1) as i64) as usize;
        state.left_click = (buttons & 0x01) != 0;
        state.right_click = (buttons & 0x02) != 0;
        state.middle_click = (buttons & 0x04) != 0;
        (state.x, state.y)
    });
    // Drive the hardware cursor plane directly (no-op if it isn't enabled). Lockless MMIO.
    crate::drivers::gpu::intel::cursor::move_to(nx, ny);
}

/// PS/2 AUX bytes, arriving while the I2C touchpad is silent, after which I2C is given up on.
const PS2_FALLBACK_BYTES: u32 = 60;

pub fn handle_interrupt(packet_byte: u8) {
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
    // ★ Packet framing by TIME. A PS/2 packet's three bytes arrive back to back (well under 1 ms
    // apart); packets are several ms apart. So a byte after a gap longer than this is the START of
    // a packet, whatever the state machine thought. The bit-3 test alone cannot re-find the
    // boundary — a motion byte can have bit 3 set — and one misaligned packet is a pointer jump.
    const PACKET_GAP_US: u64 = 5_000;
    static LAST_BYTE_US: AtomicU64 = AtomicU64::new(0);
    // Set while PS/2 bytes are being dropped for the I2C touchpad: the state machine stopped
    // mid-packet, so when PS/2 resumes it must wait for a packet boundary before decoding again.
    static RESYNC: AtomicBool = AtomicBool::new(false);
    let mhz = crate::time::TSC_MHZ.load(Relaxed).max(1);
    let now_us = crate::time::rdtsc() / mhz;
    let gap = now_us.wrapping_sub(LAST_BYTE_US.swap(now_us, Relaxed));
    let boundary = gap > PACKET_GAP_US;

    // ★ Phase 4 of the I2C-HID touchpad: while it drives the pointer, PS/2 AUX bytes are dropped
    // (the caller has already read port 0x60, so the 8042 is not left holding them). A touchpad
    // that still reports on both paths would otherwise move the cursor twice. The flag falls back
    // to false on its own if the I2C side goes quiet — see `i2c_hid::poll`.
    if crate::drivers::i2c_hid::POINTER_ACTIVE.load(core::sync::atomic::Ordering::Relaxed) {
        RESYNC.store(true, Relaxed);
        // After the firmware handover the EC's PS/2 emulation is OFF: whatever still arrives here
        // is noise, and there is no working PS/2 pointer to fall back to. Drop it uncounted — a
        // "fallback" now would only switch a working precision touchpad into a dead mode.
        if crate::drivers::i2c_hid::HANDED_OVER.load(Relaxed) {
            return;
        }
        // ★ Fallback. The boot-time enable takes the pointer on the strength of the interrupt
        // firing at RESET — nobody is touching the pad to prove more. If that was wrong, the tell
        // is PS/2 still delivering packets while I2C delivers nothing: every I2C report resets this
        // counter, so it only climbs while the I2C side is silent. ~20 PS/2 packets (60 bytes) of
        // that and the PS/2 path is evidently the one alive — give the pointer back to it.
        let n = crate::drivers::i2c_hid::PS2_WHILE_SILENT.fetch_add(1, Relaxed) + 1;
        if n < PS2_FALLBACK_BYTES {
            return;
        }
        // ★ In precision mode, silence means precision mode is what failed — not I2C. On the
        // hardware, switching to it stopped every I2C report while the EC's PS/2 emulation started
        // emitting garbage (the pointer zig-zagged, ran to the bottom and vanished). I2C mouse mode
        // is proven, so go back to THAT, and keep ignoring PS/2 rather than trusting its garbage.
        if crate::drivers::i2c_hid::PTP_ACTIVE.load(Relaxed) {
            crate::drivers::i2c_hid::PTP_ACTIVE.store(false, Relaxed);
            crate::drivers::i2c_hid::PTP_FAILED.store(true, Relaxed);
            crate::drivers::i2c_hid::MODE_REQUEST.store(1, Relaxed);
            crate::drivers::i2c_hid::PS2_WHILE_SILENT.store(0, Relaxed);
            return;
        }
        crate::drivers::i2c_hid::POINTER_ACTIVE.store(false, Relaxed);
        crate::drivers::i2c_hid::FELL_BACK.store(true, Relaxed);
        // This byte is mid-stream: fall through to the resync below, which drops it.
    }
    static mut DRIVER_STATE: Option<MouseDriver> = None;
    unsafe {
        if DRIVER_STATE.is_none() { DRIVER_STATE = Some(MouseDriver::new()); }
        let driver = DRIVER_STATE.as_mut().unwrap();

        // Resuming after bytes were dropped: decode nothing until a packet boundary. This is what
        // stops the pointer being thrown when PS/2 takes back over from I2C mid-packet.
        if RESYNC.load(Relaxed) {
            if !boundary {
                return;
            }
            RESYNC.store(false, Relaxed);
        }
        if boundary {
            driver.cycle = 0;
        }

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