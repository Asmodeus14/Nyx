#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::{String, ToString};
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

fn get_directory_names(path: &str) -> Vec<String> {
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

fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        alloc::format!("{}{}", dir, name)
    } else {
        alloc::format!("{}/{}", dir, name)
    }
}

/// Human-readable byte size (one decimal for KB/MB).
fn human_size(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if b < KB {
        alloc::format!("{} B", b)
    } else if b < MB {
        alloc::format!("{}.{} KB", b / KB, (b % KB) * 10 / KB)
    } else {
        alloc::format!("{}.{} MB", b / MB, (b % MB) * 10 / MB)
    }
}

// --- A directory entry with cached metadata (computed once per navigation, not per frame) ---
struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
}

/// List a directory into entries with type + size resolved. A path that can't be opened as a file
/// (sys_file_size < 0) is a directory — a reliable detector that also covers synthetic mount folders,
/// complementing the trailing-'/' the kernel already appends to ext4 dirs.
fn load_dir(path: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for raw in get_directory_names(path) {
        let trimmed = raw.trim_end_matches('/');
        let full = join_path(path, trimmed);
        let sz = sys_file_size(&full);
        let is_dir = raw.ends_with('/') || sz < 0;
        out.push(Entry {
            name: trimmed.to_string(),
            is_dir,
            size: if sz >= 0 { sz as u64 } else { 0 },
        });
    }
    // Directories first, then files; each group alphabetical.
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out
}

// --- Layout constants ---
const HEADER_H: usize = 50;
const COLHDR_Y: usize = 54;
const LIST_TOP: usize = 78;
const ROW_H: usize = 22;
const FOOTER_H: usize = 28;

// --- APP STATE ---
#[derive(PartialEq)]
enum AppState { Explorer, Editor }

struct ExplorerApp {
    state: AppState,
    current_path: String,
    entries: Vec<Entry>,
    scroll_px: usize,
    active_file: String,
    editor_content: String,
    width: usize,
    height: usize,
}

impl ExplorerApp {
    fn new() -> Self {
        let initial_path = String::from("/mnt/nvme/apps");
        let entries = load_dir(&initial_path);
        Self {
            state: AppState::Explorer,
            entries,
            current_path: initial_path,
            scroll_px: 0,
            active_file: String::new(),
            editor_content: String::new(),
            width: 650,
            height: 450,
        }
    }

    fn reload(&mut self) {
        self.entries = load_dir(&self.current_path);
        self.scroll_px = 0;
    }

    fn list_height(&self) -> usize {
        self.height.saturating_sub(LIST_TOP + FOOTER_H)
    }

    fn max_scroll(&self) -> usize {
        (self.entries.len() * ROW_H).saturating_sub(self.list_height())
    }
}

impl NyxApp for ExplorerApp {
    fn title(&self) -> &str { "Nyx Explorer Suite" }
    fn initial_width(&self) -> usize { 650 }
    fn initial_height(&self) -> usize { 450 }

    fn content_height(&self) -> usize {
        if self.state == AppState::Explorer { self.entries.len() * ROW_H } else { 0 }
    }
    fn scroll_offset(&self) -> usize { self.scroll_px }
    fn on_scroll(&mut self, new_offset: usize) -> bool {
        self.scroll_px = new_offset.min(self.max_scroll());
        true
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        self.width = canvas.width;
        self.height = canvas.height;
        let width = self.width;
        let height = self.height;

        canvas.fill_rect(0, 0, width, height, Color::WARM_BG);
        canvas.fill_rect(0, 0, width, HEADER_H, Color::WARM_SURFACE);
        canvas.fill_rect(0, HEADER_H, width, 1, Color::WARM_BORDER);

        if self.state == AppState::Explorer {
            // clamp scroll after any resize
            if self.scroll_px > self.max_scroll() { self.scroll_px = self.max_scroll(); }

            let mut up_btn = Button { x: 10, y: 10, w: 60, h: 30, text: String::from("Up"), is_hovered: false, is_pressed: false };
            up_btn.draw(canvas);
            let mut refresh_btn = Button { x: width.saturating_sub(90), y: 10, w: 80, h: 30, text: String::from("Refresh"), is_hovered: false, is_pressed: false };
            refresh_btn.draw(canvas);

            // Path bar
            let path_w = width.saturating_sub(80 + 100);
            canvas.fill_rect(80, 10, path_w, 30, Color::WHITE);
            canvas.fill_rect(80, 10, path_w, 1, Color::WARM_BORDER);
            canvas.print_str(90, 17, &self.current_path, Color::TEXT_DARK, 1);

            // Column geometry
            let size_right = width.saturating_sub(90); // size text right-aligned here
            let type_x = width.saturating_sub(78);

            // Column header
            canvas.fill_rect(0, COLHDR_Y, width, ROW_H - 2, Color::WARM_BG);
            canvas.print_str(16, COLHDR_Y + 3, "Name", Color::TEXT_MUTED, 1);
            let sz_hdr = "Size";
            canvas.print_str(size_right.saturating_sub(Canvas::text_width(sz_hdr, 1)), COLHDR_Y + 3, sz_hdr, Color::TEXT_MUTED, 1);
            canvas.print_str(type_x, COLHDR_Y + 3, "Type", Color::TEXT_MUTED, 1);
            canvas.fill_rect(0, LIST_TOP - 2, width, 1, Color::WARM_BORDER);

            // Rows (scrolled)
            let list_top = LIST_TOP as isize;
            let list_bottom = (height - FOOTER_H) as isize;
            if self.entries.is_empty() {
                canvas.print_str(width / 2 - 50, LIST_TOP + 20, "Folder is Empty", Color::TEXT_MUTED, 1);
            } else {
                for (i, e) in self.entries.iter().enumerate() {
                    let y_i = list_top + (i * ROW_H) as isize - self.scroll_px as isize;
                    if y_i < list_top { continue; }         // clipped above the list area
                    if y_i >= list_bottom { break; }         // and everything after is lower
                    let y = y_i as usize;

                    // Name (with an accent chip for folders)
                    let name_color = if e.is_dir { Color::ACCENT_PRIMARY } else { Color::TEXT_DARK };
                    let icon = if e.is_dir { "[]" } else { "  " };
                    canvas.print_str(16, y + 3, icon, name_color, 1);
                    // Truncate the name so it doesn't collide with the Size column.
                    let name = truncate_to(&e.name, size_right.saturating_sub(40), canvas);
                    canvas.print_str(38, y + 3, &name, Color::TEXT_DARK, 1);

                    // Size (right-aligned) — dirs have none
                    if !e.is_dir {
                        let s = human_size(e.size);
                        canvas.print_str(size_right.saturating_sub(Canvas::text_width(&s, 1)), y + 3, &s, Color::TEXT_MUTED, 1);
                    }
                    // Type
                    let ty = if e.is_dir { "<DIR>" } else { "file" };
                    canvas.print_str(type_x, y + 3, ty, Color::TEXT_MUTED, 1);
                }
            }

            // Footer: drive capacity bar + text
            let fy = height - FOOTER_H;
            canvas.fill_rect(0, fy, width, FOOTER_H, Color::WARM_SURFACE);
            canvas.fill_rect(0, fy, width, 1, Color::WARM_BORDER);
            if let Some(sf) = sys_statfs() {
                let used = sf.total_bytes.saturating_sub(sf.free_bytes);
                let bar_x = 10;
                let bar_y = fy + 9;
                let bar_w = 160usize;
                let bar_h = 10usize;
                canvas.fill_rect(bar_x, bar_y, bar_w, bar_h, Color::WARM_BORDER);
                let filled = if sf.total_bytes > 0 { (used.saturating_mul(bar_w as u64) / sf.total_bytes) as usize } else { 0 };
                canvas.fill_rect(bar_x, bar_y, filled.min(bar_w), bar_h, Color::ACCENT_GREEN);
                let txt = alloc::format!("NVMe: {} free of {}", human_size(sf.free_bytes), human_size(sf.total_bytes));
                canvas.print_str(bar_x + bar_w + 12, fy + 8, &txt, Color::TEXT_DARK, 1);
            } else {
                canvas.print_str(10, fy + 8, "NVMe: capacity unavailable", Color::TEXT_MUTED, 1);
            }
        }
        else if self.state == AppState::Editor {
            let mut back_btn = Button { x: 10, y: 10, w: 70, h: 30, text: String::from("Back"), is_hovered: false, is_pressed: false };
            back_btn.draw(canvas);
            let title_str = alloc::format!("Reading: {}", join_path(&self.current_path, &self.active_file));
            canvas.print_str(95, 17, &title_str, Color::TEXT_DARK, 1);

            canvas.fill_rect(10, 60, width - 20, height - 70, 0xFF_1E1E1E);
            draw_text_wrapped(canvas, 15, 65, width - 30, height - 80, &self.editor_content, 0xFF_CCCCCC);
        }
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        let width = self.width;
        let height = self.height;

        if self.state == AppState::Explorer {
            // Up
            if mx >= 10 && mx <= 70 && my >= 10 && my <= 40 {
                if self.current_path != "/" {
                    let mut parts: Vec<&str> = self.current_path.split('/').filter(|s| !s.is_empty()).collect();
                    parts.pop();
                    self.current_path = if parts.is_empty() { String::from("/") } else { alloc::format!("/{}", parts.join("/")) };
                    self.reload();
                    return true;
                }
            }
            // Refresh
            else if mx >= width.saturating_sub(90) && mx <= width.saturating_sub(10) && my >= 10 && my <= 40 {
                self.reload();
                return true;
            }
            // A list row
            else if my >= LIST_TOP && my < height - FOOTER_H {
                let idx = (my - LIST_TOP + self.scroll_px) / ROW_H;
                if idx < self.entries.len() {
                    let e = &self.entries[idx];
                    let target = join_path(&self.current_path, &e.name);
                    if e.is_dir {
                        self.current_path = target;
                        self.reload();
                    } else {
                        self.active_file = e.name.clone();
                        self.editor_content = read_file(&target);
                        self.state = AppState::Editor;
                    }
                    return true;
                }
            }
        }
        else if self.state == AppState::Editor {
            if mx >= 10 && mx <= 80 && my >= 10 && my <= 40 {
                self.state = AppState::Explorer;
                self.reload();
                return true;
            }
        }
        false
    }
}

/// Truncate `s` (appending "…") until it fits in `max_px` at scale 1.
fn truncate_to(s: &str, max_px: usize, _canvas: &Canvas) -> String {
    if Canvas::text_width(s, 1) <= max_px {
        return s.to_string();
    }
    let mut chars: Vec<char> = s.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let candidate: String = chars.iter().collect::<String>() + "…";
        if Canvas::text_width(&candidate, 1) <= max_px {
            return candidate;
        }
    }
    s.chars().take(1).collect()
}

fn draw_text_wrapped(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, text: &str, color: u32) {
    let lh = nyx_gui::font::line_height(1);
    let mut cx = x; let mut cy = y;
    for c in text.chars() {
        if c == '\n' { cx = x; cy += lh; continue; }
        canvas.draw_char(cx, cy, c, color, 1);
        cx += nyx_gui::font::advance(c, 1);
        if cx > x + w.saturating_sub(9) { cx = x; cy += lh; }
        if cy > y + h.saturating_sub(lh) { break; }
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
