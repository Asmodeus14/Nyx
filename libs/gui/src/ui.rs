use alloc::string::String;
use alloc::vec::Vec;
use alloc::boxed::Box;
use crate::canvas::{Canvas, Color};
use crate::effects::{alpha_blend, apply_opacity};

// ─────────────────────────────────────────────────────────────────────────
// COMPOSITOR & KERNEL UI ELEMENTS (Used by nyx-user)
// ─────────────────────────────────────────────────────────────────────────
pub struct Window {
    pub id: usize,
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub title: [u8; 64],    
    pub title_len: usize,   
    pub active: bool, pub exists: bool, pub opacity: u8,
    pub is_minimized: bool, pub is_maximized: bool,
    pub saved_x: usize, pub saved_y: usize, pub saved_w: usize, pub saved_h: usize,
}

pub fn draw_taskbar(buffer: &mut [u32], stride: usize, screen_h: usize) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    let bar_h = 36; let start_y = screen_h - bar_h;
    
    canvas.fill_rect(0, start_y, stride, bar_h, 0xD8_FFFFFF); 
    canvas.fill_rect(0, start_y, stride, 1, 0xFF_D1D1D1);     
    
    canvas.print_str(20, start_y + 14, "10:20 AM", Color::TEXT_DARK, 1);
    
    let btn_x = (stride / 2) - 35;
    canvas.fill_rect(btn_x, start_y + 6, 70, 24, Color::ACCENT_PRIMARY);
    canvas.print_str(btn_x + 15, start_y + 8, "NYX", Color::WHITE, 1);
}

#[derive(PartialEq, Clone, Copy)]
pub enum CursorType {
    Arrow,
    IBeam,
    Hand,
    // U7: resize cursors, one per edge orientation. The HW cursor path renders real double-arrow
    // bitmaps for these; the SW fallback draws the plain arrow.
    ResizeH,     // ↔  left / right edge
    ResizeV,     // ↕  top / bottom edge
    ResizeNWSE,  // ⤡  top-left / bottom-right corner
    ResizeNESW,  // ⤢  top-right / bottom-left corner
}

const ARROW_BITMAP: [[u8; 11]; 16] = [
    [1,1,0,0,0,0,0,0,0,0,0],
    [1,2,1,0,0,0,0,0,0,0,0],
    [1,2,2,1,0,0,0,0,0,0,0],
    [1,2,2,2,1,0,0,0,0,0,0],
    [1,2,2,2,2,1,0,0,0,0,0],
    [1,2,2,2,2,2,1,0,0,0,0],
    [1,2,2,2,2,2,2,1,0,0,0],
    [1,2,2,2,2,2,2,2,1,0,0],
    [1,2,2,2,2,2,2,2,2,1,0],
    [1,2,2,2,2,2,2,2,2,2,1],
    [1,2,2,2,2,2,2,1,1,1,1],
    [1,2,2,1,2,2,1,0,0,0,0],
    [1,2,1,0,1,2,2,1,0,0,0],
    [1,1,0,0,1,2,2,1,0,0,0],
    [1,0,0,0,0,1,2,2,1,0,0],
    [0,0,0,0,0,0,1,1,0,0,0],
];

const IBEAM_BITMAP: [[u8; 5]; 16] = [
    [1,1,1,1,1],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [0,0,1,0,0],
    [1,1,1,1,1],
];

const HAND_BITMAP: [[u8; 11]; 16] = [
    [0,0,0,1,1,0,0,0,0,0,0],
    [0,0,1,2,2,1,0,0,0,0,0],
    [0,0,1,2,2,1,0,0,0,0,0],
    [0,0,1,2,2,1,0,0,0,0,0],
    [0,0,1,2,2,1,1,1,1,0,0],
    [0,1,1,2,2,2,2,2,2,1,0],
    [1,2,2,1,2,2,2,2,2,2,1],
    [1,2,2,2,1,2,2,2,2,2,1],
    [1,2,2,2,2,2,2,2,2,2,1],
    [1,2,2,2,2,2,2,2,2,2,1],
    [0,1,2,2,2,2,2,2,2,1,0],
    [0,0,1,2,2,2,2,2,1,0,0],
    [0,0,1,2,2,2,2,2,1,0,0],
    [0,0,0,1,2,2,2,1,0,0,0],
    [0,0,0,1,2,2,2,1,0,0,0],
    [0,0,0,0,1,1,1,0,0,0,0],
];

pub fn draw_cursor(buffer: &mut [u32], stride: usize, screen_h: usize, mx: usize, my: usize, c_type: CursorType) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    
    match c_type {
        CursorType::Arrow => {
            for (row_idx, row) in ARROW_BITMAP.iter().enumerate() {
                for (col_idx, &pixel) in row.iter().enumerate() {
                    if pixel == 1 { canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK); } 
                    else if pixel == 2 { canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::WHITE); }
                }
            }
        },
        CursorType::IBeam => {
            let offset_x = mx.saturating_sub(2);
            for (row_idx, row) in IBEAM_BITMAP.iter().enumerate() {
                for (col_idx, &pixel) in row.iter().enumerate() {
                    if pixel == 1 { canvas.fill_rect(offset_x + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK); }
                }
            }
        },
        CursorType::Hand => {
            let offset_x = mx.saturating_sub(4);
            for (row_idx, row) in HAND_BITMAP.iter().enumerate() {
                for (col_idx, &pixel) in row.iter().enumerate() {
                    if pixel == 1 { canvas.fill_rect(offset_x + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK); }
                    else if pixel == 2 { canvas.fill_rect(offset_x + col_idx, my + row_idx, 1, 1, Color::WHITE); }
                }
            }
        }
        _ => {
            // U7 resize cursors: software fallback draws the plain arrow (the HW cursor path shows the
            // real double-arrow shapes). Keeps the SW fallback functional without extra bitmaps.
            for (row_idx, row) in ARROW_BITMAP.iter().enumerate() {
                for (col_idx, &pixel) in row.iter().enumerate() {
                    if pixel == 1 { canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::TEXT_DARK); }
                    else if pixel == 2 { canvas.fill_rect(mx + col_idx, my + row_idx, 1, 1, Color::WHITE); }
                }
            }
        }
    }
}

pub fn draw_window_rounded(buffer: &mut [u32], stride: usize, screen_h: usize, win: &Window) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    let surface = apply_opacity(Color::WARM_SURFACE, win.opacity);
    let border = apply_opacity(Color::WARM_BORDER, win.opacity);
    let total_h = if win.is_minimized { 30 } else { win.h + 30 };
    canvas.fill_rect(win.x, win.y, win.w, total_h, surface);
    canvas.fill_rect(win.x, win.y, win.w, 1, border); 
    canvas.fill_rect(win.x, win.y + total_h, win.w, 1, border); 
    canvas.fill_rect(win.x, win.y, 1, total_h, border); 
    canvas.fill_rect(win.x + win.w, win.y, 1, total_h + 1, border); 

    // Header Controls (Now with symbols!)
    let icon_color = apply_opacity(0x60_000000, win.opacity); // Dark semi-transparent text

    canvas.fill_rect(win.x + 12, win.y + 10, 12, 12, apply_opacity(0xFF_FF5F56, win.opacity)); // Close
    canvas.print_str(win.x + 14, win.y + 12, "x", icon_color, 1);

    canvas.fill_rect(win.x + 28, win.y + 10, 12, 12, apply_opacity(0xFF_FFBD2E, win.opacity)); // Min
    canvas.print_str(win.x + 30, win.y + 11, "-", icon_color, 1);

    canvas.fill_rect(win.x + 44, win.y + 10, 12, 12, apply_opacity(0xFF_28C940, win.opacity)); // Max
    canvas.print_str(win.x + 46, win.y + 12, "+", icon_color, 1);
    
    let title_str = core::str::from_utf8(&win.title[..win.title_len]).unwrap_or("App");
    let tw = crate::canvas::Canvas::text_width(title_str, 1);
    canvas.print_str(win.x + (win.w / 2).saturating_sub(tw / 2), win.y + 12, title_str, apply_opacity(Color::TEXT_DARK, win.opacity), 1);
}

/// U4 GPU-compositor chrome: draw ONLY the title bar + border frame, NOT the content-area fill (the
/// GPU composited the window's pixels into the content region, so filling it white would erase them).
/// Mirrors `draw_window_rounded` minus the full-surface fill. Content area is `[x, y+30]..`.
/// Turn the four square corners of `[x,y, w×h]` into a proper ROUNDED-RECTANGLE outline of `radius`.
/// For each pixel in a corner's rxr box, by its distance `d` to the inset arc center (radius,radius):
///   d >  radius+0.5  → OUTSIDE the window: paint `bg` (carves the square corner; the compositor's
///                       wallpaper shows through). This also erases the straight frame border's tips.
///   d in ~[r] (±0.5) → ON the arc: paint `border` — a 1px curved stroke that continues the straight
///                       top/left/bottom/right edges (which stop at `radius` from each corner) around
///                       the curve, so the BORDER follows the rounding instead of staying square.
///   d <  radius-0.5  → INSIDE: leave the window content/fill untouched.
/// Constant pixel `radius` regardless of window size.
fn round_rect_corners(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, radius: usize, bg: u32, border: u32, above: &[(i32, i32, i32, i32)]) {
    if w < 2 * radius || h < 2 * radius { return; }
    let r = radius as f32;
    let outer = (r + 0.5) * (r + 0.5); // squared thresholds — avoids sqrt (no libm in this crate)
    let inner = (r - 0.5) * (r - 0.5);
    for dy in 0..radius {
        for dx in 0..radius {
            // Squared distance from this corner cell to the arc center inset (radius, radius).
            let fx = (radius - dx) as f32;
            let fy = (radius - dy) as f32;
            let d2 = fx * fx + fy * fy;
            let c = if d2 > outer {
                bg
            } else if d2 > inner {
                border
            } else {
                continue; // inside the arc — keep the window content
            };
            // Four corners; skip any pixel covered by a window ABOVE (else carving to wallpaper would
            // punch a hole in an upper window at this window's corner).
            let pts = [
                (x + dx, y + dy),                     // top-left
                (x + w - 1 - dx, y + dy),             // top-right
                (x + dx, y + h - 1 - dy),             // bottom-left
                (x + w - 1 - dx, y + h - 1 - dy),     // bottom-right
            ];
            for (px, py) in pts {
                if !Canvas::point_occluded(px as i32, py as i32, above) {
                    canvas.fill_rect(px, py, 1, 1, c);
                }
            }
        }
    }
}

/// Round the four corners of a window's TRUE outline (title bar + content) into a rounded-rectangle
/// border, constant `radius` px. The compositor calls this ONCE per window AFTER the content is on the
/// backbuffer (GPU composite or CPU fallback) and AFTER `draw_window_chrome`/`draw_window_rounded` drew
/// the straight frame — so it carves the real corners to the wallpaper and draws the connecting arc
/// border. Size-independent (unlike the window-proportional GPU-mask experiment).
pub fn round_window_corners(buffer: &mut [u32], stride: usize, screen_h: usize, win: &Window, radius: usize, above: &[(i32, i32, i32, i32)]) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    let total_h = if win.is_minimized { 30 } else { win.h + 30 };
    let border = apply_opacity(Color::WARM_BORDER, win.opacity);
    // The frame borders (draw_window_chrome/draw_window_rounded) put the RIGHT border at column x+w and
    // the BOTTOM border at row y+total_h — one past the x..x+w-1 / y..y+total_h-1 content box. So the
    // rounded rect must be w+1 × total_h+1 to include (and curve) those right/bottom border lines;
    // otherwise they stay straight with square corners.
    round_rect_corners(&mut canvas, win.x, win.y, win.w + 1, total_h + 1, radius, Color::WARM_BG, border, above);
}

/// Newton-Raphson sqrt (2 iters off a bit-twiddle seed). No libm in this crate; adequate for the
/// shadow's soft-edge falloff.
fn fsqrt(x: f32) -> f32 {
    if x <= 0.0 { return 0.0; }
    let i = x.to_bits();
    let i = 0x5f37_5a86u32.wrapping_sub(i >> 1);
    let mut y = f32::from_bits(i);
    y = y * (1.5 - 0.5 * x * y * y);
    y = y * (1.5 - 0.5 * x * y * y);
    x * y
}

/// Draw a soft rounded DROP SHADOW for a window onto `buffer`. Called by the compositor AFTER window
/// content is composited: the shadow lands on the wallpaper and on any window BELOW this one, but
/// `above` (on-screen rects of windows stacked higher) is skipped so a higher window is never darkened
/// by a lower window's shadow. The casting window's own rect is skipped too (its content covers it).
/// Rounded outline offset down a few px, feathered by a rounded-rect signed-distance field; only the
/// visible MARGIN band is blended (interior jumped past). Pure alpha `fill_rect`, no GPU.
pub fn draw_window_shadow(buffer: &mut [u32], stride: usize, screen_h: usize, win: &Window, above: &[(i32, i32, i32, i32)]) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    let total_h = if win.is_minimized { 30 } else { win.h + 30 } as i32;
    let rw = win.w as i32;
    if rw <= 0 || total_h <= 0 { return; }

    let blur: i32 = 9;      // feather width (px) — softer/tighter
    let off_y: i32 = 4;     // shadow drop below the window
    let max_a: f32 = 42.0;  // peak shadow alpha (0..255) — toned down from 90
    let radius: f32 = 12.0; // match the window corner radius

    // Shadow rounded-rect = window rect shifted down by off_y. SDF uses center + half-extents.
    let rx = win.x as i32;
    let ry = win.y as i32 + off_y;
    let rh = total_h;
    let cx = rx as f32 + rw as f32 * 0.5;
    let cy = ry as f32 + rh as f32 * 0.5;
    let hx = rw as f32 * 0.5;
    let hy = rh as f32 * 0.5;
    let r = radius.min(hx).min(hy);

    // The window's own opaque rect (its content will cover these pixels — skip them).
    let wx0 = win.x as i32;
    let wy0 = win.y as i32;
    let wx1 = wx0 + rw;
    let wy1 = wy0 + total_h;

    let y0 = (ry - blur).max(0);
    let y1 = (ry + rh + blur).min(screen_h as i32);
    let x0 = (rx - blur).max(0);
    let x1 = (rx + rw + blur).min(stride as i32);

    // Window's ROUNDED outline (same radius as round_window_corners). The shadow must hug THIS, not the
    // square bounding box: skipping the full square box left the shadow's inner edge square (a 90° notch
    // at each corner) while the window corners are carved round → the shadow read as a square. Undo the
    // shadow's downward shift to recover the window center (the window itself is not shifted).
    let wcx = cx;
    let wcy = cy - off_y as f32;
    let ri = r as i32;

    let mut py = y0;
    while py < y1 {
        let mut px = x0;
        // A row in the window's straight (non-corner) vertical span covers the full window width inside
        // the rounded outline — skip that whole span in one jump (the original perf win preserved).
        let straight_row = py >= wy0 + ri && py < wy1 - ri;
        while px < x1 {
            // Inside the window's bounding box: decide skip by the ROUNDED outline.
            if px >= wx0 && px < wx1 && py >= wy0 && py < wy1 {
                if straight_row {
                    px = wx1; // whole interior row is under the window — jump past it
                    continue;
                }
                // Corner band: skip only pixels actually inside the rounded window. A carved corner
                // (outside the arc but inside the square box) falls through and receives shadow, so the
                // shadow curves around the rounded corner instead of forming a square notch.
                let wqx = (px as f32 - wcx).abs() - (hx - r);
                let wqy = (py as f32 - wcy).abs() - (hy - r);
                let dwin = fsqrt(wqx.max(0.0) * wqx.max(0.0) + wqy.max(0.0) * wqy.max(0.0))
                    + wqx.max(wqy).min(0.0) - r;
                if dwin <= 0.0 { px += 1; continue; }
            }
            // Skip pixels covered by any window stacked ABOVE this one — a higher window must never be
            // darkened by a lower window's shadow (this is the correct painter order the pre-pass lacked).
            let mut occluded = false;
            for &(ox0, oy0, ox1, oy1) in above {
                if px >= ox0 && px < ox1 && py >= oy0 && py < oy1 { occluded = true; break; }
            }
            if occluded { px += 1; continue; }
            // Rounded-rect signed distance (negative inside, positive outside).
            let qx = (px as f32 - cx).abs() - (hx - r);
            let qy = (py as f32 - cy).abs() - (hy - r);
            let mx = qx.max(0.0);
            let my = qy.max(0.0);
            let d = fsqrt(mx * mx + my * my) + qx.max(qy).min(0.0) - r;
            let a = if d <= 0.0 {
                max_a
            } else if d < blur as f32 {
                max_a * (1.0 - d / blur as f32)
            } else {
                px += 1;
                continue;
            };
            let ai = a as u32;
            if ai > 0 {
                canvas.fill_rect(px as usize, py as usize, 1, 1, ai << 24); // black, alpha=ai
            }
            px += 1;
        }
        py += 1;
    }
}

/// Fixed pixel width of a window scrollbar.
pub const SCROLLBAR_W: usize = 12;
/// Minimum thumb height so a tiny thumb stays grabbable.
const SCROLLBAR_MIN_THUMB: usize = 24;

/// Compute a vertical scrollbar thumb's `(offset_within_track, height)` in px, for a `track_h` track
/// showing `viewport` of `content` px scrolled to `scroll_off`. Shared by the draw and hit-test paths
/// so the visual thumb and the grabbable region always match. If not scrollable, returns the full track.
pub fn scrollbar_thumb(track_h: usize, content: usize, viewport: usize, scroll_off: usize) -> (usize, usize) {
    if content <= viewport || track_h == 0 { return (0, track_h); }
    let min_thumb = SCROLLBAR_MIN_THUMB.min(track_h);
    let mut thumb_h = viewport * track_h / content;
    if thumb_h < min_thumb { thumb_h = min_thumb; }
    if thumb_h > track_h { thumb_h = track_h; }
    let max_scroll = content - viewport;
    let off = scroll_off.min(max_scroll);
    let thumb_y = if max_scroll == 0 { 0 } else { (track_h - thumb_h) * off / max_scroll };
    (thumb_y, thumb_h)
}

/// Draw a vertical scrollbar (track + rounded thumb) at `(x, y)`, `w × track_h`, reflecting
/// content/viewport/scroll. No-op when not scrollable (`content <= viewport`). Palette colors.
pub fn draw_scrollbar(canvas: &mut Canvas, x: usize, y: usize, w: usize, track_h: usize, content: usize, viewport: usize, scroll_off: usize) {
    if content <= viewport || w == 0 || track_h == 0 { return; }
    // Subtle track.
    canvas.fill_rect(x, y, w, track_h, Color::WARM_BORDER);
    let (ty, th) = scrollbar_thumb(track_h, content, viewport, scroll_off);
    // Thumb inset 2px on each side; darker than the track.
    let inset = 2;
    if w > 2 * inset && th > 2 * inset {
        canvas.fill_rect(x + inset, y + ty + inset, w - 2 * inset, th - 2 * inset, Color::TEXT_MUTED);
    }
}

/// Draw a window's title bar + border frame. `above` = the screen rects `(x0,y0,x1,y1)` of every window
/// stacked ON TOP of this one; all chrome is occluded by them so a lower window's title/border never
/// bleeds over an upper window (the compositor draws chrome in one CPU pass after GPU-compositing all
/// content, so without occlusion the passes fight — the "below-window outline" z-order artifact).
/// U7 boot 3 (focus & hover polish): `is_active` marks the focused (topmost) window — inactive windows
/// get a dimmed title-bar surface, muted title text, and greyed traffic-light buttons (classic focus
/// cue). `hover_btn` = the title-bar button under the pointer (0=close, 1=min, 2=max; -1 = none) and
/// draws a soft highlight plate behind that button. Both are pure CPU chrome (GPU-safe).
pub fn draw_window_chrome(buffer: &mut [u32], stride: usize, screen_h: usize, win: &Window, above: &[(i32, i32, i32, i32)], is_active: bool, hover_btn: i32) {
    let mut canvas = Canvas::new(buffer, stride, screen_h);
    // Inactive windows use a slightly greyed surface so the focused window reads as "on top".
    let base_surface = if is_active { Color::WARM_SURFACE } else { 0xFF_F1F1EE };
    let surface = apply_opacity(base_surface, win.opacity);
    let border = apply_opacity(Color::WARM_BORDER, win.opacity);
    let total_h = if win.is_minimized { 30 } else { win.h + 30 };

    // Title bar strip (top 30px) — solid surface fill; the content below is GPU-drawn.
    canvas.fill_rect_occluded(win.x, win.y, win.w, 30, surface, above);
    // Frame borders around the whole window.
    canvas.fill_rect_occluded(win.x, win.y, win.w, 1, border, above);
    canvas.fill_rect_occluded(win.x, win.y + total_h, win.w, 1, border, above);
    canvas.fill_rect_occluded(win.x, win.y, 1, total_h, border, above);
    canvas.fill_rect_occluded(win.x + win.w, win.y, 1, total_h + 1, border, above);
    // Separator under the title bar.
    canvas.fill_rect_occluded(win.x, win.y + 30, win.w, 1, border, above);

    // Header controls (close / min / max) — same layout as draw_window_rounded. When the window isn't
    // focused the traffic lights grey out; the hovered button gets a soft highlight plate behind it.
    let icon_color = apply_opacity(0x60_000000, win.opacity);
    let (c_close, c_min, c_max) = if is_active {
        (0xFF_FF5F56u32, 0xFF_FFBD2Eu32, 0xFF_28C940u32)
    } else {
        (0xFF_CFCFCAu32, 0xFF_CFCFCAu32, 0xFF_CFCFCAu32)
    };
    let hover_plate = apply_opacity(0xFF_FCE9D8, win.opacity); // soft accent plate under a hovered button
    let btns = [(win.x + 12, c_close), (win.x + 28, c_min), (win.x + 44, c_max)];
    for (bi, &(bxx, col)) in btns.iter().enumerate() {
        if hover_btn == bi as i32 {
            canvas.fill_rect_occluded(bxx - 2, win.y + 8, 16, 16, hover_plate, above);
        }
        canvas.fill_rect_occluded(bxx, win.y + 10, 12, 12, apply_opacity(col, win.opacity), above);
    }
    // Glyphs/text can't be per-pixel occluded cheaply — skip each when its anchor is under an upper
    // window (coarse, but the fills above are already clipped so this only prevents floating text).
    let gy = win.y as i32 + 14;
    if !Canvas::point_occluded(win.x as i32 + 14, gy, above) { canvas.print_str(win.x + 14, win.y + 12, "x", icon_color, 1); }
    if !Canvas::point_occluded(win.x as i32 + 30, gy, above) { canvas.print_str(win.x + 30, win.y + 11, "-", icon_color, 1); }
    if !Canvas::point_occluded(win.x as i32 + 46, gy, above) { canvas.print_str(win.x + 46, win.y + 12, "+", icon_color, 1); }

    let title_str = core::str::from_utf8(&win.title[..win.title_len]).unwrap_or("App");
    let title_col = if is_active { Color::TEXT_DARK } else { Color::TEXT_MUTED };
    let tw = crate::canvas::Canvas::text_width(title_str, 1);
    let tx = win.x + (win.w / 2).saturating_sub(tw / 2);
    if !Canvas::point_occluded(tx as i32, gy, above) {
        canvas.print_str(tx, win.y + 12, title_str, apply_opacity(title_col, win.opacity), 1);
    }
}

// ─────────────────────────────────────────────────────────────────────────
// PHASE 3: POLYMORPHIC WIDGET TOOLKIT
// ─────────────────────────────────────────────────────────────────────────

/// Base trait for all UI components
pub trait Widget {
    fn draw(&mut self, canvas: &mut Canvas);
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool; // returns true if needs redraw
    fn on_key(&mut self, key: char) -> bool;
}

// --- 1. PANEL ---
pub struct Panel {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize, pub bg_color: u32,
    pub children: Vec<Box<dyn Widget>>,
}
impl Widget for Panel {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, self.bg_color);
        for child in &mut self.children { child.draw(canvas); }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_mouse(mx, my, clicked); }
        redraw
    }
    fn on_key(&mut self, key: char) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_key(key); }
        redraw
    }
}

// --- 2. LABEL ---
pub struct Label {
    pub x: usize, pub y: usize, pub text: String, pub color: u32,
}
impl Widget for Label {
    fn draw(&mut self, canvas: &mut Canvas) { canvas.print_str(self.x, self.y, &self.text, self.color, 1); }
    fn on_mouse(&mut self, _mx: usize, _my: usize, _clicked: bool) -> bool { false }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 3. BUTTON ---
pub struct Button {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub text: String,
    pub is_hovered: bool, pub is_pressed: bool,
}
impl Widget for Button {
    fn draw(&mut self, canvas: &mut Canvas) {
        let bg = if self.is_pressed { Color::ACCENT_HOVER } else if self.is_hovered { Color::ACCENT_PRIMARY } else { Color::WARM_BORDER };
        canvas.fill_rect(self.x, self.y, self.w, self.h, bg);
        canvas.print_str(self.x + 10, self.y + (self.h/2) - 4, &self.text, if self.is_hovered {Color::WHITE} else {Color::TEXT_DARK}, 1);
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let in_bounds = mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h;
        let old_hover = self.is_hovered; let old_pressed = self.is_pressed;
        self.is_hovered = in_bounds;
        self.is_pressed = in_bounds && clicked;
        old_hover != self.is_hovered || old_pressed != self.is_pressed
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 4. TEXTBOX ---
pub struct TextBox {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub text: String, pub is_focused: bool,
}
impl Widget for TextBox {
    fn draw(&mut self, canvas: &mut Canvas) {
        let border = if self.is_focused { Color::ACCENT_PRIMARY } else { Color::WARM_BORDER };
        canvas.fill_rect(self.x, self.y, self.w, self.h, Color::WHITE);
        canvas.fill_rect(self.x, self.y, self.w, 1, border);
        canvas.fill_rect(self.x, self.y + self.h, self.w, 1, border);
        canvas.fill_rect(self.x, self.y, 1, self.h, border);
        canvas.fill_rect(self.x + self.w, self.y, 1, self.h, border);
        canvas.print_str(self.x + 5, self.y + 8, &self.text, Color::TEXT_DARK, 1);
        if self.is_focused { canvas.fill_rect(self.x + 5 + (self.text.len() * 8), self.y + 6, 2, 12, Color::TEXT_DARK); }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked {
            let in_bounds = mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h;
            if self.is_focused != in_bounds { self.is_focused = in_bounds; return true; }
        }
        false
    }
    fn on_key(&mut self, key: char) -> bool {
        if self.is_focused {
            if key == '\x08' { self.text.pop(); } 
            else if key != '\n' && key != '\r' && key != '?' { self.text.push(key); }
            return true;
        }
        false
    }
}

// --- 5. CHECKBOX ---
pub struct CheckBox {
    pub x: usize, pub y: usize, pub text: String, pub is_checked: bool,
}
impl Widget for CheckBox {
    fn draw(&mut self, canvas: &mut Canvas) {
        let bg = if self.is_checked { Color::ACCENT_PRIMARY } else { Color::WHITE };
        canvas.fill_rect(self.x, self.y, 16, 16, bg);
        canvas.fill_rect(self.x, self.y, 16, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y+16, 16, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y, 1, 16, Color::WARM_BORDER);
        canvas.fill_rect(self.x+16, self.y, 1, 16, Color::WARM_BORDER);
        canvas.print_str(self.x + 25, self.y + 4, &self.text, Color::TEXT_DARK, 1);
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked && mx >= self.x && mx <= self.x + 16 && my >= self.y && my <= self.y + 16 {
            self.is_checked = !self.is_checked; return true;
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 6. LISTBOX ---
pub struct ListBox {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub items: Vec<String>, pub selected_idx: Option<usize>,
}
impl Widget for ListBox {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, Color::WHITE);
        for (i, item) in self.items.iter().enumerate() {
            let item_y = self.y + (i * 20);
            if item_y + 20 > self.y + self.h { break; } 
            if Some(i) == self.selected_idx {
                canvas.fill_rect(self.x, item_y, self.w, 20, Color::ACCENT_PRIMARY);
                canvas.print_str(self.x + 5, item_y + 6, item, Color::WHITE, 1);
            } else {
                canvas.print_str(self.x + 5, item_y + 6, item, Color::TEXT_DARK, 1);
            }
        }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked && mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h {
            let idx = (my - self.y) / 20;
            if idx < self.items.len() { self.selected_idx = Some(idx); return true; }
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 7. MENU (Dropdown) ---
pub struct Menu {
    pub x: usize, pub y: usize, pub w: usize,
    pub items: Vec<String>, pub is_open: bool, pub selected_idx: usize,
}
impl Widget for Menu {
    fn draw(&mut self, canvas: &mut Canvas) {
        // 🚨 FIX E0716: Treat it purely as a string slice (&str) rather than an allocated String reference
        let text = self.items.get(self.selected_idx).map(|s| s.as_str()).unwrap_or("Select");
        
        canvas.fill_rect(self.x, self.y, self.w, 25, Color::WARM_SURFACE);
        canvas.print_str(self.x + 5, self.y + 8, text, Color::TEXT_DARK, 1);
        canvas.print_str(self.x + self.w - 15, self.y + 8, "v", Color::TEXT_DARK, 1);
        
        if self.is_open {
            let drop_y = self.y + 25;
            canvas.fill_rect(self.x, drop_y, self.w, self.items.len() * 25, Color::WHITE);
            for (i, item) in self.items.iter().enumerate() {
                canvas.print_str(self.x + 5, drop_y + (i * 25) + 8, item, Color::TEXT_DARK, 1);
            }
        }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked {
            if mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + 25 {
                self.is_open = !self.is_open; return true;
            } else if self.is_open && mx >= self.x && mx <= self.x + self.w && my > self.y + 25 && my <= self.y + 25 + (self.items.len() * 25) {
                self.selected_idx = (my - (self.y + 25)) / 25;
                self.is_open = false; return true;
            } else if self.is_open {
                self.is_open = false; return true;
            }
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 8. SCROLLBAR ---
pub struct ScrollBar {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub value: usize, pub max_value: usize,
}
impl Widget for ScrollBar {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x, self.y, self.w, self.h, 0xFF_E0E0E0);
        let thumb_h = core::cmp::max(20, self.h / core::cmp::max(1, self.max_value));
        let thumb_y = self.y + ((self.h - thumb_h) * self.value) / core::cmp::max(1, self.max_value);
        canvas.fill_rect(self.x + 2, thumb_y, self.w - 4, thumb_h, 0xFF_999999);
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if clicked && mx >= self.x && mx <= self.x + self.w && my >= self.y && my <= self.y + self.h {
            self.value = ((my - self.y) * self.max_value) / self.h;
            return true;
        }
        false
    }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 9. IMAGEVIEW ---
pub struct ImageView {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub pixels: *const u32,
}
impl Widget for ImageView {
    fn draw(&mut self, canvas: &mut Canvas) {
        if self.pixels.is_null() { return; }
        let slice = unsafe { core::slice::from_raw_parts(self.pixels, self.w * self.h) };
        canvas.composite_buffer(self.x, self.y, slice, self.w, self.h, 255);
    }
    fn on_mouse(&mut self, _mx: usize, _my: usize, _clicked: bool) -> bool { false }
    fn on_key(&mut self, _key: char) -> bool { false }
}

// --- 10. DIALOG (Modal Box) ---
pub struct Dialog {
    pub x: usize, pub y: usize, pub w: usize, pub h: usize,
    pub title: String, pub children: Vec<Box<dyn Widget>>,
}
impl Widget for Dialog {
    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(self.x + 5, self.y + 5, self.w, self.h, 0x40_000000); 
        canvas.fill_rect(self.x, self.y, self.w, self.h, Color::WARM_BG);
        canvas.fill_rect(self.x, self.y, self.w, 30, Color::WARM_SURFACE); 
        canvas.fill_rect(self.x, self.y, self.w, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y + self.h, self.w, 1, Color::WARM_BORDER);
        canvas.fill_rect(self.x, self.y, 1, self.h, Color::WARM_BORDER);
        canvas.fill_rect(self.x + self.w, self.y, 1, self.h, Color::WARM_BORDER);
        canvas.print_str(self.x + 10, self.y + 8, &self.title, Color::TEXT_DARK, 1);
        for child in &mut self.children { child.draw(canvas); }
    }
    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_mouse(mx, my, clicked); }
        redraw
    }
    fn on_key(&mut self, key: char) -> bool {
        let mut redraw = false;
        for child in &mut self.children { redraw |= child.on_key(key); }
        redraw
    }
}