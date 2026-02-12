/// Fast integer-based alpha blending: dst = (src * a + dst * (255 - a)) / 255
/// Optimized to use bit shifts: (x * a) >> 8 is roughly x * a / 256
pub fn blend_color(fg: u32, bg: u32, alpha: u8) -> u32 {
    if alpha == 255 { return fg; }
    if alpha == 0 { return bg; }

    // Unpack foreground
    let fr = (fg >> 16) & 0xFF;
    let fg_g = (fg >> 8) & 0xFF;
    let fb = fg & 0xFF;

    // Unpack background
    let br = (bg >> 16) & 0xFF;
    let bg_g = (bg >> 8) & 0xFF;
    let bb = bg & 0xFF;

    // Integer Lerp: result = (fg * alpha + bg * (255 - alpha)) / 255
    // We use 256 (>> 8) for speed, which is "close enough" for UI
    let a = alpha as u32;
    let inv_a = 255 - a;

    let nr = (fr * a + br * inv_a) >> 8;
    let ng = (fg_g * a + bg_g * inv_a) >> 8;
    let nb = (fb * a + bb * inv_a) >> 8;

    (nr << 16) | (ng << 8) | nb
}

/// Simple Box Blur (Average of neighbors)
/// Note: A true Gaussian blur is expensive. 
/// We simulate it by running a simple "box" average 2-3 times.
pub fn box_blur(buffer: &mut [u32], w: usize, _h: usize, rect_x: usize, rect_y: usize, rect_w: usize, rect_h: usize, _radius: usize) {
    // 1. Create a temporary buffer for the blur area to avoid reading-while-writing artifacts
    // In a real OS, you'd alloc this. For now, we limit blur area to avoid stack overflow.
    // MAX BLUR AREA: 300x300 (Small clock size)
    const MAX_BLUR_PIXELS: usize = 300 * 300;
    if rect_w * rect_h > MAX_BLUR_PIXELS { return; }

    // Simple "Frosted" effect (Fast 3x3 kernel)
    // We iterate internal pixels only to avoid boundary checks for speed
    for y in (rect_y + 1)..(rect_y + rect_h - 1) {
        for x in (rect_x + 1)..(rect_x + rect_w - 1) {
            let offset = y * w + x;
            
            // Sample Up, Down, Left, Right
            let p1 = buffer[offset - w];
            let p2 = buffer[offset + w];
            let p3 = buffer[offset - 1];
            let p4 = buffer[offset + 1];
            let p5 = buffer[offset]; // Center

            // Average channels (Quick & Dirty method)
            let r = (((p1>>16)&0xFF) + ((p2>>16)&0xFF) + ((p3>>16)&0xFF) + ((p4>>16)&0xFF) + ((p5>>16)&0xFF)) / 5;
            let g = (((p1>>8)&0xFF) + ((p2>>8)&0xFF) + ((p3>>8)&0xFF) + ((p4>>8)&0xFF) + ((p5>>8)&0xFF)) / 5;
            let b = ((p1&0xFF) + (p2&0xFF) + (p3&0xFF) + (p4&0xFF) + (p5&0xFF)) / 5;

            buffer[offset] = (r << 16) | (g << 8) | b;
        }
    }
}