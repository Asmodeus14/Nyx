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
// A2b decoders moved to libs/image so the compositor can share them (icon PNGs).
use nyx_image::{decode, Image, MAX_PIXELS};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// The image the viewer opens on launch. The launcher (compositor Start menu) has no way to pass
// argv yet, so the default target is bundled next to the binary in the .nyx bundle. Explorer can
// later exec us with a real path once argv exists (Workstream B).
const DEFAULT_IMAGE: &str = "/mnt/nvme/apps/ImageViewer.nyx/sample.bmp";

// Bundled samples the viewer cycles through on click (no argv yet, so this is how
// PNG/JPEG get exercised on-device). Only the ones that actually exist are kept.
const SAMPLE_DIR: &str = "/mnt/nvme/apps/ImageViewer.nyx/";
const SAMPLE_NAMES: [&str; 3] = ["sample.png", "sample.jpg", "sample.bmp"];

// Guard rails against a hostile/huge file now live with the decoders — see nyx_image::MAX_PIXELS.

// ============================================================================
// File loading (nyx-api helpers: sys_open=2 / sys_read=0 / sys_close=3)
// ============================================================================
fn file_exists(path: &str) -> bool {
    let fd = sys_open(path);
    if fd >= 0 {
        sys_close(fd);
        true
    } else {
        false
    }
}

fn read_file_bytes(path: &str) -> Vec<u8> {
    let fd = sys_open(path);
    if fd < 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 65536];
    loop {
        let n = sys_read(fd, &mut chunk);
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n as usize]);
    }
    sys_close(fd);
    out
}

// ============================================================================
// Nearest-neighbour scale into a dst_w x dst_h buffer (composite_buffer does
// not scale, so we resize here to fit the window while preserving aspect).
// ============================================================================
fn scale_nearest(src: &Image, dst_w: usize, dst_h: usize) -> Vec<u32> {
    let mut out = vec![0u32; dst_w * dst_h];
    if dst_w == 0 || dst_h == 0 || src.w == 0 || src.h == 0 {
        return out;
    }
    for dy in 0..dst_h {
        let sy = dy * src.h / dst_h;
        let src_row = sy * src.w;
        let dst_row = dy * dst_w;
        for dx in 0..dst_w {
            let sx = dx * src.w / dst_w;
            out[dst_row + dx] = src.pixels[src_row + sx];
        }
    }
    out
}

// Compute the largest (w,h) that fits inside (max_w,max_h) preserving aspect ratio.
fn fit_dims(iw: usize, ih: usize, max_w: usize, max_h: usize) -> (usize, usize) {
    if iw == 0 || ih == 0 || max_w == 0 || max_h == 0 {
        return (0, 0);
    }
    // Scale factor = min(max_w/iw, max_h/ih), done with integer math on a fixed-point ratio.
    // Use u64 to avoid overflow on large intermediate products.
    let w_by_width = max_w;
    let h_by_width = (ih as u64 * max_w as u64 / iw as u64) as usize;
    if h_by_width <= max_h {
        (w_by_width.max(1), h_by_width.max(1))
    } else {
        let w_by_height = (iw as u64 * max_h as u64 / ih as u64) as usize;
        (w_by_height.max(1), max_h.max(1))
    }
}

// ============================================================================
// App
// ============================================================================
struct ImageViewerApp {
    path: String,
    image: Option<Image>,
    status: String,
    // Cache of the last scaled frame so we don't rescale every tick; keyed by target dims.
    scaled: Vec<u32>,
    scaled_w: usize,
    scaled_h: usize,
    // Bundled samples that exist, cycled on click; `cur` indexes the one shown.
    candidates: Vec<String>,
    cur: usize,
}

const FOOTER_H: usize = 28;

impl ImageViewerApp {
    fn new() -> Self {
        let mut candidates: Vec<String> = Vec::new();
        for name in SAMPLE_NAMES.iter() {
            let p = alloc::format!("{}{}", SAMPLE_DIR, name);
            if file_exists(&p) {
                candidates.push(p);
            }
        }
        if candidates.is_empty() {
            candidates.push(String::from(DEFAULT_IMAGE));
        }
        let mut app = Self {
            path: String::new(),
            image: None,
            status: String::new(),
            scaled: Vec::new(),
            scaled_w: 0,
            scaled_h: 0,
            candidates,
            cur: 0,
        };
        app.load_index(0);
        app
    }

    // Load + decode candidate `i` (wrapping), resetting the scaled-frame cache.
    fn load_index(&mut self, i: usize) {
        if self.candidates.is_empty() {
            return;
        }
        let i = i % self.candidates.len();
        self.cur = i;
        let path = self.candidates[i].clone();
        let bytes = read_file_bytes(&path);
        let (image, status) = if bytes.is_empty() {
            (None, alloc::format!("Could not open {}   (click to cycle)", path))
        } else {
            match decode(&bytes) {
                Ok(img) => {
                    // w/h are Copy — read them before `img` moves into the tuple.
                    let s = alloc::format!(
                        "{}  —  {} x {}   (click to cycle)",
                        short_name(&path),
                        img.w,
                        img.h
                    );
                    (Some(img), s)
                }
                Err(e) => (None, alloc::format!("{}: {}   (click to cycle)", short_name(&path), e)),
            }
        };
        self.path = path;
        self.image = image;
        self.status = status;
        self.scaled.clear();
        self.scaled_w = 0;
        self.scaled_h = 0;
    }

    // Ensure `self.scaled` holds the image fit into (avail_w, avail_h). Returns the drawn dims.
    fn ensure_scaled(&mut self, avail_w: usize, avail_h: usize) -> (usize, usize) {
        let img = match &self.image {
            Some(i) => i,
            None => return (0, 0),
        };
        let (fw, fh) = fit_dims(img.w, img.h, avail_w, avail_h);
        if fw != self.scaled_w || fh != self.scaled_h {
            self.scaled = scale_nearest(img, fw, fh);
            self.scaled_w = fw;
            self.scaled_h = fh;
        }
        (self.scaled_w, self.scaled_h)
    }
}

fn short_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

impl NyxApp for ImageViewerApp {
    fn icon_path(&self) -> &str { "/mnt/nvme/apps/ImageViewer.nyx/icon.png" }
    fn title(&self) -> &str {
        "Nyx Image Viewer"
    }
    fn initial_width(&self) -> usize {
        800
    }
    fn initial_height(&self) -> usize {
        600
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let width = canvas.width;
        let height = canvas.height;

        // Checkerboard-free flat backdrop; a mid-grey reads well for both light and dark images.
        canvas.fill_rect(0, 0, width, height, 0xFF_1E1E1E);

        let avail_h = height.saturating_sub(FOOTER_H);

        if self.image.is_some() {
            let (dw, dh) = self.ensure_scaled(width, avail_h);
            if dw > 0 && dh > 0 {
                let x = (width.saturating_sub(dw)) / 2;
                let y = (avail_h.saturating_sub(dh)) / 2;
                canvas.composite_buffer(x, y, &self.scaled, dw, dh, 255);
            }
        } else {
            canvas.print_str(20, avail_h / 2, &self.status, Color::WHITE, 1);
        }

        // Footer: filename + dimensions (or the error), on an opaque strip.
        canvas.fill_rect(0, height - FOOTER_H, width, FOOTER_H, Color::RAISED);
        canvas.fill_rect(0, height - FOOTER_H, width, 1, Color::LINE);
        canvas.print_str(12, height - FOOTER_H + 7, &self.status, Color::FG, 1);
    }

    // Any click advances to the next bundled sample (BMP → PNG → JPG → TGA → …),
    // the only way to reach PNG/JPEG until the launcher can pass a file argv.
    fn on_mouse(&mut self, _mx: usize, _my: usize, _clicked: bool) -> bool {
        let n = self.candidates.len().max(1);
        self.load_index((self.cur + 1) % n);
        true
    }
}

// ============================================================================
// Entry
// ============================================================================
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // 4096 pages * 4 KiB = 16 MiB heap. A PNG/JPEG decode holds several full-frame
    // buffers at once: the decoder's own output (up to ~4 MiB), our packed u32 copy
    // (4 MiB for 1024x1024), plus the scaled-to-window copy — 4 MiB was too tight.
    let heap_start = sys_alloc_pages(4096);
    if heap_start == 0 {
        sys_exit(1);
    }
    unsafe {
        ALLOCATOR.lock().init(heap_start as *mut u8, 4096 * 4096);
    }

    nyx_gui::app::run(ImageViewerApp::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(111);
}
