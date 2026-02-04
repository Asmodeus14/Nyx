use spin::Mutex;
use x86_64::instructions::port::Port;
use lazy_static::lazy_static;

const DATA_PORT: u16 = 0x60;
const CMD_PORT: u16 = 0x64;

pub struct MouseState {
    pub x: usize, pub y: usize,
    pub left_click: bool, pub right_click: bool,
    pub screen_width: usize, pub screen_height: usize,
}

pub struct MouseDriver {
    data_port: Port<u8>,
    cmd_port: Port<u8>,
    packet: [u8; 3],
    packet_idx: usize,
}

lazy_static! {
    pub static ref MOUSE: Mutex<MouseDriver> = Mutex::new(MouseDriver::new());
    pub static ref MOUSE_STATE: Mutex<MouseState> = Mutex::new(MouseState {
        x: 960, y: 540, // Center of 1080p
        left_click: false, right_click: false,
        screen_width: 1920, screen_height: 1080, 
    });
}

impl MouseDriver {
    pub fn new() -> Self {
        Self {
            data_port: Port::new(DATA_PORT),
            cmd_port: Port::new(CMD_PORT),
            packet: [0; 3],
            packet_idx: 0,
        }
    }

    fn wait_write(&mut self) -> bool {
        for _ in 0..10_000 {
            unsafe { if (self.cmd_port.read() & 0x02) == 0 { return true; } }
        }
        false 
    }

    fn wait_read(&mut self) -> bool {
        for _ in 0..10_000 {
            unsafe { if (self.cmd_port.read() & 0x01) == 1 { return true; } }
        }
        false
    }

    pub fn init(&mut self) {
        unsafe {
            if self.wait_write() { self.cmd_port.write(0xA8); }
            if self.wait_write() { self.cmd_port.write(0x20); }
            if self.wait_read() {
                let mut status = self.data_port.read();
                status |= 0x02;
                if self.wait_write() { self.cmd_port.write(0x60); }
                if self.wait_write() { self.data_port.write(status); }
            }
            self.write_mouse(0xF6);
            self.write_mouse(0xF4);
        }
    }

    unsafe fn write_mouse(&mut self, byte: u8) {
        if self.wait_write() { self.cmd_port.write(0xD4); }
        if self.wait_write() { self.data_port.write(byte); }
        if self.wait_read() { self.data_port.read(); }
    }

    pub fn process_packet(&mut self, byte: u8) {
        if self.packet_idx == 0 && (byte & 0x08) == 0 { return; }

        self.packet[self.packet_idx] = byte;
        self.packet_idx += 1;

        if self.packet_idx == 3 {
            self.packet_idx = 0;
            let flags = self.packet[0];
            
            // SPEED BOOSTER
            let speed = 4;
            let x_rel = (self.packet[1] as i8) as i64 * speed;
            let y_rel = -(self.packet[2] as i8) as i64 * speed; 

            let mut state = MOUSE_STATE.lock();
            let new_x = state.x as i64 + x_rel;
            let new_y = state.y as i64 + y_rel;
            
            state.x = new_x.clamp(0, (state.screen_width - 5) as i64) as usize;
            state.y = new_y.clamp(0, (state.screen_height - 5) as i64) as usize;
            state.left_click = (flags & 0b0000_0001) != 0;
            state.right_click = (flags & 0b0000_0010) != 0;
        }
    }
}