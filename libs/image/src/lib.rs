//! Image decoding for Nyx — BMP, TGA, PNG and JPEG to tightly-packed B8G8R8A8.
//!
//! Lifted out of apps/imageviewer so the compositor can decode icon PNGs from the same code. Two
//! consumers means a divergence bug would be invisible in one of them, which is exactly the reason
//! this is a crate rather than a copy.
//!
//! no_std + alloc: every consumer builds for the bare x86_64-nyx target under
//! `-Z build-std=core,alloc`, so no dependency here may pull in `std`.

#![no_std]

extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

/// Ceiling on decoded pixel count. A decoded image costs w*h*4 bytes and app heaps are a few MiB,
/// so this is what stops a hostile or simply huge file from exhausting the allocator.
pub const MAX_PIXELS: usize = 1024 * 1024;

// ============================================================================
// Decoded image: tightly packed B8G8R8A8 (u32 = A<<24 | R<<16 | G<<8 | B),
// pitch == width. This is exactly the format Canvas::composite_buffer expects.
// ============================================================================
pub struct Image {
    pub pixels: Vec<u32>,
    pub w: usize,
    pub h: usize,
}

#[inline]
pub fn pack(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

// ----------------------------------------------------------------------------
// Decoder dispatch by magic bytes. BMP + TGA now; PNG/JPEG slot in here (A2b).
// ----------------------------------------------------------------------------
pub fn decode(bytes: &[u8]) -> Result<Image, &'static str> {
    if bytes.len() >= 2 && &bytes[0..2] == b"BM" {
        return decode_bmp(bytes);
    }
    // PNG magic \x89PNG\r\n\x1a\n (hand-written parser + miniz_oxide inflate) and
    // JPEG SOI 0xFFD8 (zune-jpeg) — Workstream A2b.
    if bytes.len() >= 8 && &bytes[0..4] == &[0x89, b'P', b'N', b'G'] {
        return decode_png(bytes);
    }
    if bytes.len() >= 2 && &bytes[0..2] == &[0xFF, 0xD8] {
        #[cfg(feature = "jpeg")]
        return decode_jpeg(bytes);
        #[cfg(not(feature = "jpeg"))]
        return Err("JPEG support not compiled in");
    }
    // TGA has no magic; try it last as a best-effort fallback.
    decode_tga(bytes)
}

// ----------------------------------------------------------------------------
// BMP: uncompressed (BI_RGB) 24/32-bit, top-down or bottom-up.
// ----------------------------------------------------------------------------
pub fn decode_bmp(b: &[u8]) -> Result<Image, &'static str> {
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
pub fn decode_tga(b: &[u8]) -> Result<Image, &'static str> {
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
pub fn decode_png(b: &[u8]) -> Result<Image, &'static str> {
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
#[cfg(feature = "jpeg")]
pub fn decode_jpeg(bytes: &[u8]) -> Result<Image, &'static str> {
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

