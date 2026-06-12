use alloc::string::String;
use nyx_api::*;
use crate::canvas::Canvas;

pub trait NyxApp {
    fn title(&self) -> &str;
    fn initial_width(&self) -> usize { 640 }
    fn initial_height(&self) -> usize { 400 }
    
    fn init(&mut self) {}
    
    fn update(&mut self) -> bool { false }
    fn draw(&mut self, canvas: &mut Canvas);
    fn on_mouse(&mut self, _mx: usize, _my: usize, _clicked: bool) -> bool { false }
    fn on_key(&mut self, _key: char) -> bool { false }
}

pub fn run<T: NyxApp>(mut app: T) -> ! {
    const COMPOSITOR_PID: u64 = 4;
    
    let mut width = app.initial_width();
    let mut height = app.initial_height();
    
    let mut total_size = core::mem::size_of::<WindowHeader>() + (width * height * 4);
    let mut shm_id = sys_create_shm(total_size);
    if shm_id == 0 { sys_exit(1); }

    let mut buffer_ptr = sys_map_shm(shm_id) as *mut u8;
    let mut header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };
    
    header.magic = WIN_MAGIC;
    header.requested_x = -1;
    header.requested_y = -1;
    header.width = width as u32;
    header.height = height as u32;
    header.flags = WIN_FLAG_NONE;
    
    let title_bytes = app.title().as_bytes();
    header.title.fill(0);
    let len = title_bytes.len().min(64);
    header.title[..len].copy_from_slice(&title_bytes[..len]);

    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) { sys_exit(1); }
    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop { 
        if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED { break; } 
    }

    let mut pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
    
    app.init();

    let mut needs_redraw = true;
    
    // 🚨 FIX: Track if we have a pending memory swap waiting for paint
    let mut pending_shm_swap: Option<u64> = None;

    loop {
        let mut event_redraw = false;

        if sys_ipc_recv(&mut msg, false) {
            match msg.msg_type {
                MSG_WINDOW_CLOSE => sys_exit(0),
                MSG_WINDOW_RESIZED => {
                    width = msg.data1 as usize;
                    height = msg.data2 as usize;

                    total_size = core::mem::size_of::<WindowHeader>() + (width * height * 4);
                    let new_shm_id = sys_create_shm(total_size);
                    
                    buffer_ptr = sys_map_shm(new_shm_id) as *mut u8;
                    header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };
                    
                    header.magic = WIN_MAGIC;
                    header.width = width as u32;
                    header.height = height as u32;
                    
                    pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
                    
                    // 🚨 FIX: Do NOT notify the Compositor yet!
                    // Save the ID and force the engine to paint the new buffer first.
                    pending_shm_swap = Some(new_shm_id);
                    needs_redraw = true; 
                },
                MSG_MOUSE_EVENT => {
                    event_redraw |= app.on_mouse(msg.data1 as usize, msg.data2 as usize, true);
                },
                MSG_KEY_EVENT => {
                    if let Some(key) = core::char::from_u32(msg.data1 as u32) {
                        event_redraw |= app.on_key(key);
                    }
                },
                _ => {}
            }
        }

        let update_redraw = app.update();
        
        if needs_redraw || event_redraw || update_redraw {
            let screen = unsafe { core::slice::from_raw_parts_mut(pixels_ptr, width * height) };
            let mut canvas = Canvas::new(screen, width, height);
            
            // 1. Fully paint the buffer
            app.draw(&mut canvas);
            
            // 2. NOW safely tell the Compositor the buffer is ready
            if let Some(shm_id) = pending_shm_swap {
                sys_ipc_send(COMPOSITOR_PID, MSG_WINDOW_UPDATE_SHM, shm_id, 0);
                pending_shm_swap = None;
            } else {
                // If it wasn't a resize event, just flush a normal frame update
                sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            }
            
            needs_redraw = false;
        }
        
        sys_sleep_ms(16);
    }
}