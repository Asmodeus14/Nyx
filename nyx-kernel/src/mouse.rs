use spin::Mutex;
use x86_64::instructions::port::Port;
use lazy_static::lazy_static;

const PS2_CMD_PORT: u16 = 0x64;
const PS2_DATA_PORT: u16 = 0x60;

const CMD_ENABLE_AUX: u8 = 0xA8;
const CMD_GET_CONFIG: u8 = 0x20;
const CMD_SET_CONFIG: u8 = 0x60;
const CMD_WRITE_AUX: u8 = 0xD4;
const MOUSE_CMD_RESET: u8 = 0xFF;
const MOUSE_CMD_ENABLE_DATA: u8 = 0xF4;

// --- GLOBAL MOUSE STATE ---
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

// --- PS/2 DRIVER ---
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
        for _ in 0..10000 {
            if (self.command_port.read() & 0x02) == 0 { return; }
        }
    }

    unsafe fn wait_for_read(&mut self) {
        for _ in 0..10000 {
            if (self.command_port.read() & 0x01) == 1 { return; }
        }
    }

    pub fn init(&mut self) {
        unsafe {
            self.wait_for_write(); self.command_port.write(CMD_ENABLE_AUX);
            self.wait_for_write(); self.command_port.write(CMD_GET_CONFIG);
            self.wait_for_read();
            let mut status = self.data_port.read();
            status |= 0x02; status &= !0x20;
            self.wait_for_write(); self.command_port.write(CMD_SET_CONFIG);
            self.wait_for_write(); self.data_port.write(status);
            self.write_mouse(MOUSE_CMD_RESET);
            self.wait_for_read(); let _ = self.data_port.read();
            self.write_mouse(MOUSE_CMD_ENABLE_DATA);
            self.wait_for_read(); let _ = self.data_port.read();
        }
    }

    unsafe fn write_mouse(&mut self, byte: u8) {
        self.wait_for_write(); self.command_port.write(CMD_WRITE_AUX);
        self.wait_for_write(); self.data_port.write(byte);
    }
}

// --- PUBLIC API ---
pub fn update_from_usb(dx: i8, dy: i8, buttons: u8) {
    let mut state = MOUSE_STATE.lock();
    let new_x = state.x as i64 + (dx as i64); 
    let new_y = state.y as i64 + (dy as i64); 
    state.x = new_x.clamp(0, (state.screen_width - 1) as i64) as usize;
    state.y = new_y.clamp(0, (state.screen_height - 1) as i64) as usize;
    state.left_click = (buttons & 0x01) != 0;
    state.right_click = (buttons & 0x02) != 0;
    state.middle_click = (buttons & 0x04) != 0;
}

// --- INTERRUPT HANDLER ---
pub fn handle_interrupt(packet_byte: u8) {
    static mut DRIVER_STATE: Option<MouseDriver> = None;
    unsafe {
        if DRIVER_STATE.is_none() { DRIVER_STATE = Some(MouseDriver::new()); }
        let driver = DRIVER_STATE.as_mut().unwrap();

        match driver.cycle {
            0 => {
                if (packet_byte & 0x08) != 0 { 
                    driver.packet[0] = packet_byte; driver.cycle += 1; 
                }
            }
            1 => { driver.packet[1] = packet_byte; driver.cycle += 1; }
            2 => {
                driver.packet[2] = packet_byte;
                let flags = driver.packet[0];
                let x_raw = driver.packet[1];
                let y_raw = driver.packet[2];

                let x_neg = (flags & 0x10) != 0;
                let y_neg = (flags & 0x20) != 0;
                
                let rel_x = if x_neg { (x_raw as i16) - 256 } else { x_raw as i16 };
                let rel_y = if y_neg { (y_raw as i16) - 256 } else { y_raw as i16 };

                let mut state = MOUSE_STATE.lock();
                
                // Y-Axis Inversion Fix (Up is Minus)
                let new_x = state.x as i32 + rel_x as i32;
                let new_y = state.y as i32 - (rel_y as i32); 

                state.x = new_x.clamp(0, state.screen_width as i32 - 1) as usize;
                state.y = new_y.clamp(0, state.screen_height as i32 - 1) as usize;
                
                state.left_click = (flags & 0x01) != 0;
                state.right_click = (flags & 0x02) != 0;
                
                // NO DRAWING CODE HERE.
                
                driver.cycle = 0;
            }
            _ => driver.cycle = 0,
        }
    }
}