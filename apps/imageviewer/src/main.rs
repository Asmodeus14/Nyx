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

// Guard rails: the default app heap is 4 MiB (see `_start`). A decoded image costs w*h*4 bytes,
// so cap dimensions well under that to avoid OOM on a hostile/huge file. 2048*2048*4 = 16 MiB
// would blow the heap; 1024*1024*4 = 4 MiB is the ceiling we allow a source image to occupy.
const MAX_PIXELS: usize = 1024 * 1024;

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
// Decoded image: tightly packed B8G8R8A8 (u32 = A<<24 | R<<16 | G<<8 | B),
// pitch == width. This is exactly the format Canvas::composite_buffer expects.
// ============================================================================
struct Image {
    pixels: Vec<u32>,
    w: usize,
    h: usize,
}

#[inline]
fn pack(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ----------------------------------------------------------------------------
// Decoder dispatch by magic bytes. BMP + TGA now; PNG/JPEG slot in here (A2b).
// ----------------------------------------------------------------------------
fn decode(bytes: &[u8]) -> Result<Image, &'static str> {
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        return decode_bmp(bytes);
    }
    // PNG magic \x89PNG\r\n\x1a\n (hand-written parser + miniz_oxide inflate) and
    // JPEG SOI 0xFFD8 (zune-jpeg) — Workstream A2b.
    if bytes.len() >= 8 && &bytes[0..4] == &[0x89, b'P', b'N', b'G'] {
        return decode_png(bytes);
    }
    if bytes.len() >= 2 && &bytes[0..2] == &[0xFF, 0xD8] {
        return decode_jpeg(bytes);
    }
    // TGA has no magic; try it last as a best-effort fallback.
    decode_tga(bytes)
}

// ----------------------------------------------------------------------------
// BMP: uncompressed (BI_RGB) 24/32-bit, top-down or bottom-up.
// ----------------------------------------------------------------------------
fn decode_bmp(b: &[u8]) -> Result<Image, &'static str> {
    if b.len() < 54 || &b[0..2] != b"BM" {
        return Err("bad BMP header");
    }
    let data_off = u32::from_le_bytes([b[10], b[11], b[12], b[13]]) as usize;
    // DIB header: BITMAPINFOHEADER (40 bytes) fields we care about.
    let width = i32::from_le_bytes([b[18], b[19], b[20], b[21]]);
    let height_raw = i32::from_le_bytes([b[22], b[23], b[24], b[25]]);
    let bpp = u16::from_le_bytes([b[28], b[29]]) as usize;
    let compression = u32::from_le_bytes([b[30], b[31], b[32], b[33]]);

    if compression != 0 {
        return Err("compressed BMP unsupported");
    }
    if bpp != 24 && bpp != 32 {
        return Err("only 24/32-bit BMP supported");
    }
    if width <= 0 || height_raw == 0 {
        return Err("bad BMP dimensions");
    }

    let w = width as usize;
    let top_down = height_raw < 0; // negative height => rows stored top-to-bottom
    let h = height_raw.unsigned_abs() as usize;

    if w.checked_mul(h).map_or(true, |p| p > MAX_PIXELS) {
        return Err("image too large");
    }

    let bytes_pp = bpp / 8;
    let row_stride = (w * bytes_pp + 3) & !3; // rows padded up to a 4-byte boundary

    let mut px = vec![0u32; w * h];
    for row in 0..h {
        // Bottom-up bitmaps store the last visual row first.
        let src_row = if top_down { row } else { h - 1 - row };
        let base = data_off + src_row * row_stride;
        if base + w * bytes_pp > b.len() {
            return Err("BMP pixel data truncated");
        }
        for x in 0..w {
            let p = base + x * bytes_pp;
            let bl = b[p];
            let g = b[p + 1];
            let r = b[p + 2];
            // BI_RGB has no defined alpha channel (the 4th byte of 32-bit BMP is "reserved" and
            // is commonly 0), so we always present opaque. bl/g/r are read in BMP's native B,G,R
            // memory order above.
            px[row * w + x] = pack(r, g, bl, 255);
        }
    }
    Ok(Image { pixels: px, w, h })
}

// ----------------------------------------------------------------------------
// TGA: uncompressed true-color (type 2) 24/32-bit. Minimal but useful.
// ----------------------------------------------------------------------------
fn decode_tga(b: &[u8]) -> Result<Image, &'static str> {
    if b.len() < 18 {
        return Err("unrecognized image format");
    }
    let id_len = b[0] as usize;
    let color_map_type = b[1];
    let image_type = b[2];
    if color_map_type != 0 || image_type != 2 {
        return Err("only uncompressed true-color TGA supported");
    }
    let w = u16::from_le_bytes([b[12], b[13]]) as usize;
    let h = u16::from_le_bytes([b[14], b[15]]) as usize;
    let bpp = b[16] as usize;
    let descriptor = b[17];
    let top_down = (descriptor & 0x20) != 0;

    if w == 0 || h == 0 {
        return Err("bad TGA dimensions");
    }
    if w.checked_mul(h).map_or(true, |p| p > MAX_PIXELS) {
        return Err("image too large");
    }
    if bpp != 24 && bpp != 32 {
        return Err("only 24/32-bit TGA supported");
    }

    let bytes_pp = bpp / 8;
    let data_off = 18 + id_len;
    if data_off + w * h * bytes_pp > b.len() {
        return Err("TGA pixel data truncated");
    }

    let mut px = vec![0u32; w * h];
    for row in 0..h {
        let src_row = if top_down { row } else { h - 1 - row };
        let base = data_off + src_row * w * bytes_pp;
        for x in 0..w {
            let p = base + x * bytes_pp;
            let bl = b[p];
            let g = b[p + 1];
            let r = b[p + 2];
            let a = if bytes_pp == 4 { b[p + 3] } else { 255 };
            px[row * w + x] = pack(r, g, bl, a);
        }
    }
    Ok(Image { pixels: px, w, h })
}

// ----------------------------------------------------------------------------
// PNG: hand-written chunk parser + unfilter, DEFLATE via miniz_oxide (no_std,
// with-alloc). Scope: 8-bit depth, non-interlaced; color types 0/2/3/4/6.
// ----------------------------------------------------------------------------
fn decode_png(b: &[u8]) -> Result<Image, &'static str> {
    const SIG: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    if b.len() < 8 || !b.starts_with(&SIG) {
        return Err("bad PNG signature");
    }

    let mut pos = 8usize;
    let (mut width, mut height) = (0usize, 0usize);
    let (mut bit_depth, mut color_type, mut interlace) = (0u8, 0u8, 0u8);
    let mut seen_ihdr = false;
    let mut idat: Vec<u8> = Vec::new();
    let mut palette: Vec<(u8, u8, u8)> = Vec::new();
    let mut trns: Vec<u8> = Vec::new(); // per-palette-index alpha (color type 3)

    while pos + 8 <= b.len() {
        let len = u32::from_be_bytes([b[pos], b[pos + 1], b[pos + 2], b[pos + 3]]) as usize;
        let ctype = [b[pos + 4], b[pos + 5], b[pos + 6], b[pos + 7]];
        let dstart = pos + 8;
        if dstart + len + 4 > b.len() {
            return Err("PNG chunk truncated");
        }
        let data = &b[dstart..dstart + len];
        match &ctype {
            b"IHDR" => {
                if len < 13 {
                    return Err("PNG bad IHDR");
                }
                width = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
                height = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
                bit_depth = data[8];
                color_type = data[9];
                // data[10] compression method (0), data[11] filter method (0)
                interlace = data[12];
                seen_ihdr = true;
            }
            b"PLTE" => {
                for i in 0..len / 3 {
                    palette.push((data[i * 3], data[i * 3 + 1], data[i * 3 + 2]));
                }
            }
            b"tRNS" => trns.extend_from_slice(data),
            b"IDAT" => idat.extend_from_slice(data),
            b"IEND" => break,
            _ => {}
        }
        pos = dstart + len + 4; // advance past data + 4-byte CRC (unchecked)
    }

    if !seen_ihdr {
        return Err("PNG missing IHDR");
    }
    if bit_depth != 8 {
        return Err("PNG: only 8-bit depth supported");
    }
    if interlace != 0 {
        return Err("PNG: interlaced (Adam7) unsupported");
    }
    if width == 0 || height == 0 {
        return Err("PNG: zero dimensions");
    }
    if width.checked_mul(height).map_or(true, |p| p > MAX_PIXELS) {
        return Err("PNG image too large");
    }

    let channels: usize = match color_type {
        0 => 1, // grayscale
        2 => 3, // truecolor RGB
        3 => 1, // palette index
        4 => 2, // grayscale + alpha
        6 => 4, // truecolor RGBA
        _ => return Err("PNG: unsupported color type"),
    };
    let bpp = channels; // bytes per pixel at 8-bit depth
    let stride = width * bpp;

    let raw = miniz_oxide::inflate::decompress_to_vec_zlib(&idat)
        .map_err(|_| "PNG: inflate failed")?;
    if raw.len() < height * (1 + stride) {
        return Err("PNG: short inflate output");
    }

    // Reverse the per-scanline filters into a contiguous height*stride buffer.
    let mut img = vec![0u8; height * stride];
    for y in 0..height {
        let filter = raw[y * (1 + stride)];
        let sbase = y * (1 + stride) + 1;
        for x in 0..stride {
            let a = if x >= bpp { img[y * stride + x - bpp] as i32 } else { 0 };
            let up = if y > 0 { img[(y - 1) * stride + x] as i32 } else { 0 };
            let ul = if x >= bpp && y > 0 { img[(y - 1) * stride + x - bpp] as i32 } else { 0 };
            let v = raw[sbase + x] as i32;
            let recon = match filter {
                0 => v,
                1 => v + a,
                2 => v + up,
                3 => v + (a + up) / 2,
                4 => v + paeth(a, up, ul),
                _ => return Err("PNG: bad filter type"),
            } & 0xFF;
            img[y * stride + x] = recon as u8;
        }
    }

    // Expand each pixel to packed B8G8R8A8.
    let mut px = vec![0u32; width * height];
    for y in 0..height {
        for x in 0..width {
            let i = y * stride + x * bpp;
            let (r, g, bl, a) = match color_type {
                0 => (img[i], img[i], img[i], 255),
                2 => (img[i], img[i + 1], img[i + 2], 255),
                3 => {
                    let idx = img[i] as usize;
                    let (r, g, bl) = *palette.get(idx).unwrap_or(&(0, 0, 0));
                    (r, g, bl, *trns.get(idx).unwrap_or(&255))
                }
                4 => (img[i], img[i], img[i], img[i + 1]),
                6 => (img[i], img[i + 1], img[i + 2], img[i + 3]),
                _ => unreachable!(),
            };
            px[y * width + x] = pack(r, g, bl, a);
        }
    }
    Ok(Image { pixels: px, w: width, h: height })
}

/// PNG Paeth predictor (a=left, b=above, c=upper-left).
#[inline]
fn paeth(a: i32, b: i32, c: i32) -> i32 {
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

// ----------------------------------------------------------------------------
// JPEG: baseline/progressive via zune-jpeg (no_std, scalar path). Ask for RGB
// output and pack to B8G8R8A8.
// ----------------------------------------------------------------------------
fn decode_jpeg(bytes: &[u8]) -> Result<Image, &'static str> {
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGB);
    let mut dec = zune_jpeg::JpegDecoder::new_with_options(bytes, options);
    dec.decode_headers().map_err(|_| "JPEG: bad headers")?;
    let (w, h) = dec.dimensions().ok_or("JPEG: no dimensions")?;
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 {
        return Err("JPEG: zero dimensions");
    }
    if w.checked_mul(h).map_or(true, |p| p > MAX_PIXELS) {
        return Err("JPEG image too large");
    }

    let rgb = dec.decode().map_err(|_| "JPEG decode failed")?;
    if rgb.len() < w * h * 3 {
        return Err("JPEG: short pixel buffer");
    }
    let mut px = vec![0u32; w * h];
    for i in 0..w * h {
        px[i] = pack(rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2], 255);
    }
    Ok(Image { pixels: px, w, h })
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
        canvas.fill_rect(0, height - FOOTER_H, width, FOOTER_H, Color::WARM_SURFACE);
        canvas.fill_rect(0, height - FOOTER_H, width, 1, Color::WARM_BORDER);
        canvas.print_str(12, height - FOOTER_H + 7, &self.status, Color::TEXT_DARK, 1);
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
