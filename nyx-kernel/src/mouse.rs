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
        screen_width: 1024, screen_height: 768, 
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
        self.log("PS/2: INIT...", 0);
        
        unsafe {
            self.cmd_port.write(0xA8);
            self.wait_write();

            self.cmd_port.write(0x20); 
            let mut status = self.read_byte().unwrap_or(0);
            status |= 0x02; 
            self.cmd_port.write(0x60); 
            self.wait_write();
            self.data_port.write(status);

            self.write_mouse(0xFF);
            let _ = self.read_byte(); 
            let _ = self.read_byte();
            let _ = self.read_byte();

            self.write_mouse(0xF6);
            let _ = self.read_byte(); 

            self.write_mouse(0xF4);
            let _ = self.read_byte(); 

            self.log("PS/2: READY", 20);
        }
    }

    unsafe fn write_mouse(&mut self, byte: u8) {
        self.wait_write();
        self.cmd_port.write(0xD4); 
        self.wait_write();
        self.data_port.write(byte);
        self.wait_read();
    }

    unsafe fn read_byte(&mut self) -> Option<u8> {
        for _ in 0..100_000 {
            if (self.cmd_port.read() & 0x01) == 1 {
                return Some(self.data_port.read());
            }
        }
        None
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

pub fn update_from_usb(dx: i8, dy: i8, buttons: u8) {
    let mut state = MOUSE_STATE.lock();
    let new_x = state.x as i64 + (dx as i64); 
    let new_y = state.y as i64 + (dy as i64);
    state.x = new_x.clamp(0, (state.screen_width - 10) as i64) as usize;
    state.y = new_y.clamp(0, (state.screen_height - 10) as i64) as usize;
    state.left_click = (buttons & 0x01) != 0;
    state.right_click = (buttons & 0x02) != 0;
    state.middle_click = (buttons & 0x04) != 0;
}

pub fn handle_interrupt(byte: u8) {
    MOUSE_PACKETS.fetch_add(1, Ordering::Relaxed);
    if let Some(mut queue) = MOUSE_QUEUE.try_lock() {
        if queue.len() < 128 { queue.push_back(byte); }
    }
}

// FIX: Use stack buffer instead of Vec to avoid Interrupt Allocation Deadlocks
pub fn drain_queue() {
    let mut buffer = [0u8; 64]; 
    let mut count = 0;

    interrupts::without_interrupts(|| {
        if let Some(mut queue) = MOUSE_QUEUE.try_lock() {
            while let Some(b) = queue.pop_front() { 
                if count < 64 {
                    buffer[count] = b;
                    count += 1;
                } else {
                    break;
                }
            }
        }
    });

    if count == 0 { return; }

    static mut PACKET: [u8; 3] = [0; 3];
    static mut IDX: usize = 0;

    unsafe {
        for i in 0..count {
            let byte = buffer[i];
            
            if IDX == 0 && (byte & 0x08) == 0 { 
                continue; 
            }

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
    state.right_click = (flags & 0x02) != 0;
}