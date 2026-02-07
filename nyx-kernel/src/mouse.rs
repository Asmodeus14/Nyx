use spin::Mutex;
use x86_64::instructions::port::Port;
use lazy_static::lazy_static;
use alloc::collections::VecDeque;
use x86_64::instructions::interrupts;
use core::sync::atomic::{AtomicUsize, Ordering};
use crate::gui::{Color, Rect, Painter}; 

const DATA_PORT: u16 = 0x60;
const CMD_PORT: u16 = 0x64;

pub static MOUSE_PACKETS: AtomicUsize = AtomicUsize::new(0);

pub struct MouseState {
    pub x: usize, pub y: usize,
    pub left_click: bool, pub right_click: bool, pub middle_click: bool,
    pub screen_width: usize, pub screen_height: usize,
}

lazy_static! {
    pub static ref MOUSE_QUEUE: Mutex<VecDeque<u8>> = Mutex::new(VecDeque::new());
    pub static ref MOUSE_STATE: Mutex<MouseState> = Mutex::new(MouseState {
        x: 400, y: 300, 
        left_click: false, right_click: false, middle_click: false,
        screen_width: 800, screen_height: 600, 
    });
}

pub struct MouseDriver {
    data_port: Port<u8>,
    cmd_port: Port<u8>,
}

impl MouseDriver {
    pub fn new() -> Self {
        Self { data_port: Port::new(DATA_PORT), cmd_port: Port::new(CMD_PORT) }
    }

    fn log(&self, msg: &str, y_offset: usize) {
        unsafe {
            if let Some(painter) = &mut crate::SCREEN_PAINTER {
                painter.draw_string(0, y_offset, msg, Color::YELLOW);
            }
        }
    }

    pub fn init(&mut self) {
        self.log("MOUSE: POLLING INIT...", 0);
        
        unsafe {
            // Enable Aux Port
            self.cmd_port.write(0xA8);
            self.wait_write();

            // Try to enable interrupts (just in case)
            self.cmd_port.write(0x20);
            self.wait_read();
            let mut status = self.data_port.read();
            status |= 0x02; 
            self.cmd_port.write(0x60);
            self.wait_write();
            self.data_port.write(status);

            // Enable Streaming
            self.write_mouse(0xF4);
            let _ = self.read_byte(); 

            self.log("MOUSE: READY (POLL)", 20);
        }
    }

    unsafe fn write_mouse(&mut self, byte: u8) {
        self.wait_write();
        self.cmd_port.write(0xD4);
        self.wait_write();
        self.data_port.write(byte);
        self.wait_read();
    }

    unsafe fn read_byte(&mut self) -> u8 {
        for _ in 0..10000 {
            if (self.cmd_port.read() & 0x01) == 1 {
                return self.data_port.read();
            }
        }
        0
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
    MOUSE_PACKETS.fetch_add(1, Ordering::Relaxed);
    let mut queue = MOUSE_QUEUE.lock();
    queue.push_back(byte);
}

// Called 1000 times/sec
pub fn update() {
    let mut data_port = Port::<u8>::new(DATA_PORT);
    let mut cmd_port = Port::<u8>::new(CMD_PORT);

    // Manual Poll
    unsafe {
        let status = cmd_port.read();
        if (status & 0x01) == 1 && (status & 0x20) != 0 {
            let byte = data_port.read();
            handle_interrupt(byte); 
        }
    }

    let mut bytes = alloc::vec::Vec::new();
    interrupts::without_interrupts(|| {
        let mut queue = MOUSE_QUEUE.lock();
        while let Some(b) = queue.pop_front() { bytes.push(b); }
    });

    if bytes.is_empty() { return; }

    static mut PACKET: [u8; 3] = [0; 3];
    static mut IDX: usize = 0;

    unsafe {
        for byte in bytes {
            if IDX == 0 && (byte & 0x08) == 0 { continue; }

            PACKET[IDX] = byte;
            IDX += 1;

            if IDX == 3 {
                IDX = 0;
                process_packet(PACKET);
            }
        }
    }
}

fn process_packet(packet: [u8; 3]) {
    let flags = packet[0];
    let x_raw = packet[1];
    let y_raw = packet[2];

    if (flags & 0xC0) != 0 { return; }

    let x_sign = (flags & 0x10) != 0;
    let y_sign = (flags & 0x20) != 0;

    let mut x_rel: i16 = x_raw as i16;
    let mut y_rel: i16 = y_raw as i16;

    if x_sign { x_rel |= 0xFF00u16 as i16; }
    if y_sign { y_rel |= 0xFF00u16 as i16; }

    let mut state = MOUSE_STATE.lock();
    let new_x = state.x as i64 + x_rel as i64;
    let new_y = state.y as i64 - y_rel as i64; 

    state.x = new_x.clamp(0, (state.screen_width - 10) as i64) as usize;
    state.y = new_y.clamp(0, (state.screen_height - 10) as i64) as usize;
    state.left_click = (flags & 0x01) != 0;
}