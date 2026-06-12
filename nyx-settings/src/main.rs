#![no_std]
#![no_main]

extern crate alloc;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::{Button, ToggleSwitch, DropdownMenu};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const WIDTH: usize = 420;
const HEIGHT: usize = 350;

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(256);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 256 * 4096); }

    // 🚨 GUARANTEED COMPOSITOR PID
    const COMPOSITOR_PID: u64 = 4; 

    let total_size = core::mem::size_of::<WindowHeader>() + (WIDTH * HEIGHT * 4);
    let shm_id = sys_create_shm(total_size);
    if shm_id == 0 { sys_exit(1); } // This is where it was crashing before!

    let buffer_ptr = sys_map_shm(shm_id) as *mut u8;
    let header = unsafe { &mut *(buffer_ptr as *mut WindowHeader) };
    
    header.magic = WIN_MAGIC;
    header.requested_x = -1; 
    header.requested_y = -1;
    header.width = WIDTH as u32;
    header.height = HEIGHT as u32;
    header.flags = WIN_FLAG_NONE;
    
    let title = b"System Settings";
    header.title.fill(0);
    header.title[..title.len()].copy_from_slice(title);

    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) { sys_exit(1); }

    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop { if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED { break; } }

    let pixels_ptr = unsafe { buffer_ptr.add(core::mem::size_of::<WindowHeader>()) } as *mut u32;
    let screen = unsafe { core::slice::from_raw_parts_mut(pixels_ptr, WIDTH * HEIGHT) };
    let mut canvas = Canvas::new(screen, WIDTH, HEIGHT);

    let mut dark_mode = false;
    let mut animations = true;
    let resolutions = ["800x600", "1024x768", "1920x1080"];
    let mut selected_res = 1;
    let mut dropdown_open = false;
    let mut needs_redraw = true;

    loop {
        if needs_redraw {
            let bg = if dark_mode { 0xFF_1E1E1E } else { Color::WARM_BG };
            let text_color = if dark_mode { Color::WHITE } else { Color::TEXT_DARK };

            canvas.fill_rect(0, 0, WIDTH, HEIGHT, bg);
            canvas.print_str(20, 20, "Appearance", text_color, 2); 
            canvas.print_str(20, 65, "Dark Mode", text_color, 1);
            
            let toggle_dark = ToggleSwitch { x: 340, y: 60, is_on: dark_mode };
            toggle_dark.draw(&mut canvas);

            canvas.print_str(20, 105, "Fluid Animations", text_color, 1);
            let toggle_anim = ToggleSwitch { x: 340, y: 100, is_on: animations };
            toggle_anim.draw(&mut canvas);

            canvas.fill_rect(20, 150, WIDTH - 40, 1, Color::WARM_BORDER); 
            canvas.print_str(20, 170, "Display", text_color, 2);
            canvas.print_str(20, 215, "Resolution", text_color, 1);
            
            let dropdown = DropdownMenu {
                x: 230, y: 210, w: 150, h: 25,
                options: &resolutions,
                selected_idx: selected_res,
                is_open: dropdown_open,
                hover_idx: None,
            };
            dropdown.draw(&mut canvas);

            let apply_btn = Button {
                x: WIDTH - 120, y: HEIGHT - 50, w: 100, h: 30,
                text: "Apply",
                is_hovered: false, is_pressed: false,
            };
            apply_btn.draw(&mut canvas);

            sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            needs_redraw = false;
        }

        if sys_ipc_recv(&mut msg, true) {
            // 🚨 THE FIX: Gracefully die and give memory back to the Kernel
            if msg.msg_type == MSG_WINDOW_CLOSE {
                sys_exit(0);
            }
            else if msg.msg_type == MSG_MOUSE_EVENT {
                let mx = msg.data1 as usize; let my = msg.data2 as usize;

                if mx >= 340 && mx <= 380 && my >= 60 && my <= 80 {
                    dark_mode = !dark_mode; needs_redraw = true;
                } else if mx >= 340 && mx <= 380 && my >= 100 && my <= 120 {
                    animations = !animations; needs_redraw = true;
                } else if dropdown_open {
                    let drop_y = 235; let drop_h = resolutions.len() * 25;
                    if mx >= 230 && mx <= 380 && my >= drop_y && my <= drop_y + drop_h {
                        let clicked_idx = (my - drop_y) / 25;
                        if clicked_idx < resolutions.len() { selected_res = clicked_idx; }
                    }
                    dropdown_open = false; needs_redraw = true;
                } else if mx >= 230 && mx <= 380 && my >= 210 && my <= 235 {
                    dropdown_open = true; needs_redraw = true;
                }
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }