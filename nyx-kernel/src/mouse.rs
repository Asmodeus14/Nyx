use spin::Mutex;
use x86_64::instructions::port::Port;
use lazy_static::lazy_static;
use alloc::collections::VecDeque;
// ADDED IMPORT:
use x86_64::instructions::interrupts;

const DATA_PORT: u16 = 0x60;
const CMD_PORT: u16 = 0x64;

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
    pub static ref MOUSE_QUEUE: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    pub static ref MOUSE_STATE: Mutex<MouseState> = Mutex::new(MouseState {
        x: 960, y: 540, 
        left_click: false, 
        right_click: false,
        middle_click: false,
        screen_width: 1920, 
        screen_height: 1080, 
    });
}

pub struct MouseDriver {
    data_port: Port<u8>,
    cmd_port: Port<u8>,
}

impl MouseDriver {
    pub fn new() -> Self {
        Self {
            data_port: Port::new(DATA_PORT),
            cmd_port: Port::new(CMD_PORT),
        }
    }

    pub fn init(&mut self) {
        unsafe {
            self.cmd_port.write(0xA8); // Enable Mouse
            self.wait_write();
            self.cmd_port.write(0x20);
            self.wait_read();
            let mut status = self.data_port.read();
            status |= 0x02; // Enable IRQ 12
            self.wait_write();
            self.cmd_port.write(0x60);
            self.wait_write();
            self.data_port.write(status);

            self.write_mouse(0xF6); // Defaults
            self.write_mouse(0xF4); // Enable Streaming
        }
    }

    unsafe fn write_mouse(&mut self, byte: u8) {
        self.wait_write();
        self.cmd_port.write(0xD4);
        self.wait_write();
        self.data_port.write(byte);
        self.wait_read();
        let _ack = self.data_port.read(); 
    }

    fn wait_write(&mut self) {
        for _ in 0..100_000 {
            unsafe { if (self.cmd_port.read() & 0x02) == 0 { return; } }
        }
    }

    fn wait_read(&mut self) {
        for _ in 0..100_000 {
            unsafe { if (self.cmd_port.read() & 0x01) == 1 { return; } }
        }
    }
}

pub fn handle_interrupt(byte: u8) {
    let mut queue = MOUSE_QUEUE.lock();
    queue.push_back(byte);
}

// --- MAIN LOOP UPDATE (Fixes Deadlock) ---
pub fn update() {
    let mut bytes = alloc::vec::Vec::new();
    
    // CRITICAL FIX: Lock protection for the queue
    interrupts::without_interrupts(|| {
        let mut queue = MOUSE_QUEUE.lock();
        while let Some(b) = queue.pop_front() {
            bytes.push(b);
        }
    });

    if bytes.is_empty() { return; }

    static mut INTERNAL_PACKET: [u8; 3] = [0; 3];
    static mut INTERNAL_IDX: usize = 0;

    unsafe {
        for byte in bytes {
            // Alignment check: First byte always has bit 3 set
            if INTERNAL_IDX == 0 && (byte & 0x08) == 0 {
                continue;
            }

            INTERNAL_PACKET[INTERNAL_IDX] = byte;
            INTERNAL_IDX += 1;

            if INTERNAL_IDX == 3 {
                INTERNAL_IDX = 0;
                process_packet(INTERNAL_PACKET);
            }
        }
    }
}

fn process_packet(packet: [u8; 3]) {
    let flags = packet[0];
    let x_raw = packet[1];
    let y_raw = packet[2];

    let x_overflow = (flags & 0x40) != 0;
    let y_overflow = (flags & 0x80) != 0;
    if x_overflow || y_overflow { return; }

    let x_sign = (flags & 0x10) != 0;
    let y_sign = (flags & 0x20) != 0;

    let mut x_rel: i16 = x_raw as i16;
    let mut y_rel: i16 = y_raw as i16;

    if x_sign { x_rel |= 0xFF00u16 as i16; }
    if y_sign { y_rel |= 0xFF00u16 as i16; }

    let mut state = MOUSE_STATE.lock();
    
    let new_x = (state.x as i64 + x_rel as i64).clamp(0, (state.screen_width - 5) as i64);
    // Invert Y because PS/2 Y is up-positive, Screen is down-positive
    let new_y = (state.y as i64 - y_rel as i64).clamp(0, (state.screen_height - 5) as i64);

    state.x = new_x as usize;
    state.y = new_y as usize;
    state.left_click = (flags & 0x01) != 0;
    state.right_click = (flags & 0x02) != 0;
    state.middle_click = (flags & 0x04) != 0;
}