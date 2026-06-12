#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::{Button, Widget};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// --- NATIVE SYSCALL WRAPPERS FOR VFS ---
#[inline]
unsafe fn syscall_2(id: u64, a: u64, b: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

#[inline]
unsafe fn syscall_3(id: u64, a: u64, b: u64, c: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, in("rdx") c, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

#[inline]
unsafe fn syscall_4(id: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

fn get_directory_contents(path: &str) -> Vec<String> {
    let mut files = Vec::new();
    let count = unsafe { syscall_2(510, path.as_ptr() as u64, path.len() as u64) };
    for i in 0..count {
        let mut buf = [0u8; 256];
        let len = unsafe { syscall_4(511, i, buf.as_mut_ptr() as u64, path.as_ptr() as u64, path.len() as u64) };
        if len > 0 {
            if let Ok(s) = core::str::from_utf8(&buf[..len as usize]) { files.push(String::from(s)); }
        }
    }
    files
}

fn read_file(path: &str) -> String {
    let fd = unsafe { syscall_2(2, path.as_ptr() as u64, path.len() as u64) } as i64;
    if fd < 0 { return String::from("Error: Could not open file (Directory or Not Found)."); }
    
    let mut buf = vec![0u8; 8192];
    let bytes_read = unsafe { syscall_3(0, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) } as i64;
    unsafe { syscall_2(3, fd as u64, 0); } 
    
    if bytes_read > 0 {
        String::from_utf8_lossy(&buf[..bytes_read as usize]).into_owned()
    } else {
        String::from("[Empty File]")
    }
}

// --- APP STATE ---
#[derive(PartialEq)]
enum AppState { Explorer, Editor }

struct ExplorerApp {
    state: AppState,
    current_path: String,
    files: Vec<String>,
    current_page: usize,
    active_file: String,
    editor_content: String,
}

impl ExplorerApp {
    fn new() -> Self {
        let initial_path = String::from("/mnt/nvme/apps");
        Self {
            state: AppState::Explorer,
            files: get_directory_contents(&initial_path),
            current_path: initial_path,
            current_page: 0,
            active_file: String::new(),
            editor_content: String::new(),
        }
    }
}

impl NyxApp for ExplorerApp {
    fn title(&self) -> &str { "Nyx Explorer Suite" }
    fn initial_width(&self) -> usize { 650 }
    fn initial_height(&self) -> usize { 450 }

    fn draw(&mut self, canvas: &mut Canvas) {
        let width = canvas.width;
        let height = canvas.height;

        canvas.fill_rect(0, 0, width, height, Color::WARM_BG); 
        canvas.fill_rect(0, 0, width, 50, Color::WARM_SURFACE); 
        canvas.fill_rect(0, 50, width, 1, Color::WARM_BORDER);

        if self.state == AppState::Explorer {
            let mut up_btn = Button { x: 10, y: 10, w: 60, h: 30, text: String::from("Up"), is_hovered: false, is_pressed: false };
            up_btn.draw(canvas);

            canvas.fill_rect(80, 10, width.saturating_sub(340), 30, Color::WHITE);
            canvas.fill_rect(80, 10, width.saturating_sub(340), 1, Color::WARM_BORDER);
            canvas.print_str(90, 17, &self.current_path, Color::TEXT_DARK, 1);

            let items_per_page = 24;
            let total_pages = if self.files.is_empty() { 1 } else { (self.files.len() + items_per_page - 1) / items_per_page };
            
            if total_pages > 1 {
                let mut prev_btn = Button { x: width - 250, y: 10, w: 30, h: 30, text: String::from("<"), is_hovered: false, is_pressed: false };
                let mut next_btn = Button { x: width - 130, y: 10, w: 30, h: 30, text: String::from(">"), is_hovered: false, is_pressed: false };
                prev_btn.draw(canvas);
                next_btn.draw(canvas);
                
                let page_text = alloc::format!("{} / {}", self.current_page + 1, total_pages);
                canvas.print_str(width - 210, 17, &page_text, Color::TEXT_DARK, 1);
            }

            let mut refresh_btn = Button { x: width - 90, y: 10, w: 80, h: 30, text: String::from("Refresh"), is_hovered: false, is_pressed: false };
            refresh_btn.draw(canvas);

            let start_idx = self.current_page * items_per_page;
            let end_idx = core::cmp::min(start_idx + items_per_page, self.files.len());
            let visible_files = &self.files[start_idx..end_idx];

            let mut fx = 20; let mut fy = 70;
            if self.files.is_empty() {
                canvas.print_str(width/2 - 50, height/2, "Folder is Empty", Color::TEXT_MUTED, 1);
            } else {
                for file in visible_files.iter() {
                    canvas.fill_rect(fx, fy, 130, 40, Color::WARM_SURFACE); 
                    canvas.fill_rect(fx, fy, 5, 40, Color::ACCENT_PRIMARY); 
                    
                    let display_name = if file.len() > 14 { alloc::format!("{}...", &file[..11]) } else { file.clone() };
                    canvas.print_str(fx + 15, fy + 12, &display_name, Color::TEXT_DARK, 1);
                    
                    fx += 150;
                    if fx > width - 150 { fx = 20; fy += 60; }
                }
            }
        } 
        else if self.state == AppState::Editor {
            let mut back_btn = Button { x: 10, y: 10, w: 70, h: 30, text: String::from("Back"), is_hovered: false, is_pressed: false };
            back_btn.draw(canvas);
            let title_str = alloc::format!("Reading: {}{}{}", self.current_path, if self.current_path.ends_with('/') {""} else {"/"}, self.active_file);
            canvas.print_str(95, 17, &title_str, Color::TEXT_DARK, 1);

            canvas.fill_rect(10, 60, width - 20, height - 70, 0xFF_1E1E1E); 
            draw_text_wrapped(canvas, 15, 65, width - 30, height - 80, &self.editor_content, 0xFF_CCCCCC);
        }
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        let width = 650; 
        let items_per_page = 24;

        if self.state == AppState::Explorer {
            let total_pages = if self.files.is_empty() { 1 } else { (self.files.len() + items_per_page - 1) / items_per_page };

            if mx >= 10 && mx <= 70 && my >= 10 && my <= 40 {
                if self.current_path != "/" {
                    let mut parts: Vec<&str> = self.current_path.split('/').filter(|s| !s.is_empty()).collect();
                    parts.pop();
                    self.current_path = if parts.is_empty() { String::from("/") } else { alloc::format!("/{}", parts.join("/")) };
                    self.files = get_directory_contents(&self.current_path);
                    self.current_page = 0; 
                    return true;
                }
            }
            else if mx >= width - 90 && mx <= width - 10 && my >= 10 && my <= 40 {
                self.files = get_directory_contents(&self.current_path);
                self.current_page = 0;
                return true;
            } 
            else if total_pages > 1 && mx >= width - 250 && mx <= width - 220 && my >= 10 && my <= 40 {
                if self.current_page > 0 { self.current_page -= 1; return true; }
            }
            else if total_pages > 1 && mx >= width - 130 && mx <= width - 100 && my >= 10 && my <= 40 {
                if self.current_page < total_pages - 1 { self.current_page += 1; return true; }
            }
            else {
                let start_idx = self.current_page * items_per_page;
                let end_idx = core::cmp::min(start_idx + items_per_page, self.files.len());
                let visible_files = &self.files[start_idx..end_idx];

                let mut fx = 20; let mut fy = 70;
                for file in visible_files.iter() {
                    if mx >= fx && mx <= fx + 130 && my >= fy && my <= fy + 40 {
                        let target_path = alloc::format!("{}{}{}", self.current_path, if self.current_path.ends_with('/') {""} else {"/"}, file);
                        
                        // 🚨 YOUR ORIGINAL LOGIC RESTORED
                        let dir_contents = get_directory_contents(&target_path);
                        
                        if !dir_contents.is_empty() {
                            self.current_path = target_path;
                            self.files = dir_contents;
                            self.current_page = 0;
                        } else {
                            self.active_file = file.clone();
                            self.editor_content = read_file(&target_path);
                            self.state = AppState::Editor;
                        }
                        return true;
                    }
                    fx += 150; if fx > width - 150 { fx = 20; fy += 60; }
                }
            }
        } 
        else if self.state == AppState::Editor {
            if mx >= 10 && mx <= 80 && my >= 10 && my <= 40 {
                self.state = AppState::Explorer;
                self.files = get_directory_contents(&self.current_path); 
                return true;
            }
        }
        false
    }
}

fn draw_text_wrapped(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, text: &str, color: u32) {
    let mut cx = x; let mut cy = y;
    for c in text.chars() {
        if c == '\n' { cx = x; cy += 16; continue; }
        canvas.draw_char(cx, cy, c, color, 1);
        cx += 9; 
        if cx > x + w - 9 { cx = x; cy += 16; }
        if cy > y + h - 16 { break; } 
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(256);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 256 * 4096); }

    nyx_gui::app::run(ExplorerApp::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }