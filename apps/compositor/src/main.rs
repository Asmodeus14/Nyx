#![no_std]
#![no_main]
// See libs/gui/src/lib.rs: some pinned nightlies ICE in the annotate_snippets diagnostic
// renderer while drawing ANY warning with a source snippet, aborting the build. Suppress lints
// (project idiom; matches the other apps) so the buggy renderer is never entered.
#![allow(warnings)]
extern crate alloc;

use linked_list_allocator::LockedHeap;
use alloc::vec::Vec;
use alloc::vec;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::{draw_taskbar, draw_window_rounded, draw_window_chrome, round_window_corners, draw_window_shadow, draw_scrollbar, SCROLLBAR_W, draw_cursor, ctl_btn_hit, Window, CursorType};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Whole-hour offset applied to the raw hardware RTC before display. 0 = show the RTC verbatim
/// (the machine's CMOS holds local time). Bump this if the panel clock is off by whole hours.
const TZ_OFFSET_HOURS: i32 = 0;

// ─────────────────────────────── Launcher ───────────────────────────────

/// One start-menu entry. Label, icon PNG, and the binary to exec — both paths inside the app's own
/// `.nyx` bundle, so an app ships its artwork alongside its code instead of relying on a central
/// icon directory the app knows nothing about.
struct MenuEntry {
    label: &'static str,
    icon: &'static str,
    exec: &'static str,
}

/// The launcher, as ONE table. This used to be three parallel lists — a labels array for the GPU
/// text pass, an identical one for the CPU fallback, and a chain of `else if rel_y < N` for the
/// clicks — so adding an app meant editing three places and a wrong threshold silently launched the
/// neighbouring entry. Now the renderer and the hit-test both iterate this.
const MENU: [MenuEntry; 12] = [
    MenuEntry { label: "Terminal",       icon: "/mnt/nvme/apps/Terminal.nyx/icon.png", exec: "/mnt/nvme/apps/Terminal.nyx/run.bin\0" },
    MenuEntry { label: "Settings",       icon: "/mnt/nvme/apps/Settings.nyx/icon.png", exec: "/mnt/nvme/apps/Settings.nyx/run.bin\0" },
    MenuEntry { label: "Explorer",       icon: "/mnt/nvme/apps/Explorer.nyx/icon.png", exec: "/mnt/nvme/apps/Explorer.nyx/run.bin\0" },
    MenuEntry { label: "Network Suite",  icon: "/mnt/nvme/apps/Network.nyx/icon.png", exec: "/mnt/nvme/apps/Network.nyx/run.bin\0" },
    MenuEntry { label: "System Monitor", icon: "/mnt/nvme/apps/SystemMonitor.nyx/icon.png", exec: "/mnt/nvme/apps/SystemMonitor.nyx/run.bin\0" },
    MenuEntry { label: "GL Cube (3D)",   icon: "/mnt/nvme/apps/GlCube.nyx/icon.png", exec: "/mnt/nvme/apps/GlCube.nyx/run.bin\0" },
    MenuEntry { label: "Image Viewer",   icon: "/mnt/nvme/apps/ImageViewer.nyx/icon.png", exec: "/mnt/nvme/apps/ImageViewer.nyx/run.bin\0" },
    MenuEntry { label: "Std Hello",      icon: "/mnt/nvme/apps/StdHello.nyx/icon.png", exec: "/mnt/nvme/apps/StdHello.nyx/run.bin\0" },
    MenuEntry { label: "QC Studio",      icon: "/mnt/nvme/apps/QcStudio.nyx/icon.png", exec: "/mnt/nvme/apps/QcStudio.nyx/run.bin\0" },
    MenuEntry { label: "Std GUI",        icon: "/mnt/nvme/apps/StdGui.nyx/icon.png", exec: "/mnt/nvme/apps/StdGui.nyx/run.bin\0" },
    MenuEntry { label: "Notepad",        icon: "/mnt/nvme/apps/Notepad.nyx/icon.png", exec: "/mnt/nvme/apps/Notepad.nyx/run.bin\0" },
    MenuEntry { label: "Wi-Fi",          icon: "/mnt/nvme/apps/Wifi.nyx/icon.png", exec: "/mnt/nvme/apps/Wifi.nyx/run.bin\0" },
];

/// The shell's own mark, in the compositor's bundle like every other app's icon.
const NYX_LOGO: &str = "/mnt/nvme/apps/WindowServer.nyx/icon.png";

// Start menu: a TILE GRID, not a list. One row per app made a 244x536 column — a pillar taller than
// it was wide, which got worse with every app added. A 3-wide grid keeps the panel close to square
// and leaves room for the search field.
const MENU_COLS: usize = 3;
const MENU_ROWS: usize = 4;          // 3x4 = 12 slots, i.e. the whole launcher with no scrolling
const MENU_TILE_W: usize = 124;
const MENU_TILE_H: usize = 88;
const MENU_PAD: usize = 14;
const MENU_W: usize = MENU_PAD * 2 + MENU_COLS * MENU_TILE_W;
const MENU_HEADER_H: usize = 52;
const MENU_SEARCH_H: usize = 30;
const MENU_SEARCH_GAP: usize = 10;
/// Top of the tile grid, relative to the panel: header, then the search field with a gap either side.
const MENU_GRID_Y: usize = MENU_HEADER_H + MENU_SEARCH_GAP + MENU_SEARCH_H + MENU_SEARCH_GAP;
const MENU_PAD_B: usize = 14;
const MENU_ICON: usize = 40;   // size icons are pre-scaled to for the menu tiles
const LOGO_BTN: usize = 20;    // and for the start button
const TB_ICON: usize = 22;     // and for taskbar window buttons
const MENU_SEARCH_MAX: usize = 24;

const BAR_H: usize = 36;
const START_BTN_W: usize = 96;
const START_BTN_H: usize = 26;

/// The panel is a FIXED size — it does not reflow as the search narrows the results. A panel that
/// resized per keystroke would jump under the pointer and leave stale pixels behind every shrink.
fn menu_h() -> usize { MENU_GRID_Y + MENU_ROWS * MENU_TILE_H + MENU_PAD_B }

/// Case-insensitive substring match, used to filter the launcher by the search box. Labels and query
/// are both ASCII, so this compares bytes.
fn label_matches(label: &str, q: &[u8]) -> bool {
    if q.is_empty() { return true; }
    let l = label.as_bytes();
    if q.len() > l.len() { return false; }
    'outer: for start in 0..=(l.len() - q.len()) {
        for i in 0..q.len() {
            if !l[start + i].eq_ignore_ascii_case(&q[i]) { continue 'outer; }
        }
        return true;
    }
    false
}

/// Longest prefix of `s` that fits in `max_w` pixels. Tile labels are centred, and under the
/// proportional font a long one ("System Monitor") would otherwise bleed into the neighbouring tile.
/// Labels are ASCII, so byte slicing can't split a char.
fn fit_label(s: &str, max_w: usize) -> &str {
    if Canvas::text_width(s, 1) <= max_w { return s; }
    let mut end = s.len();
    while end > 1 && Canvas::text_width(&s[..end], 1) > max_w { end -= 1; }
    &s[..end]
}

/// Read a whole file from the VFS. Icons are a few hundred bytes each, so one growing read is fine.
fn read_file(path: &str) -> Option<Vec<u8>> {
    let fd = sys_open(path);
    if fd < 0 { return None; }
    let mut out: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = sys_read(fd, &mut chunk);
        if n <= 0 { break; }
        out.extend_from_slice(&chunk[..n as usize]);
        if (n as usize) < chunk.len() { break; }
    }
    sys_close(fd);
    if out.is_empty() { None } else { Some(out) }
}

/// Decode an icon PNG and pre-scale it to `size`. None if the file is missing or undecodable — every
/// draw site treats that as "no icon" and falls back to text, so a half-installed bundle degrades
/// instead of leaving a blank square.
fn load_icon(path: &str, size: usize) -> Option<Vec<u32>> {
    let bytes = read_file(path)?;
    let img = nyx_image::decode(&bytes).ok()?;
    Some(Canvas::scale_rgba(&img.pixels, img.w, img.h, size, size))
}

/// Filled rectangle with rounded corners, for the chrome. Small radii only — the corner test is a
/// plain distance check per pixel, with no antialiasing (chrome sits on flat fills, so the hard
/// edge is invisible at r<=8 and it keeps this off the per-frame cost).
fn fill_round_rect(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, r: usize, color: u32) {
    if w == 0 || h == 0 { return; }
    for row in 0..h {
        let dy = if row < r { r - row } else if row >= h - r { row + 1 - (h - r) } else { 0 };
        let inset = if dy == 0 { 0 } else {
            // Largest k with k^2 + dy^2 <= r^2 → how far this row's corner is cut in.
            let mut k = r;
            while k > 0 && k * k + dy * dy > r * r { k -= 1; }
            r - k
        };
        if inset * 2 >= w { continue; }
        canvas.fill_rect(x + inset, y + row, w - inset * 2, 1, color);
    }
}

/// Append a zero-padded 2-digit number to `buf` at `pos`, returning the new position.
fn push_2d(buf: &mut [u8], pos: usize, v: u8) -> usize {
    buf[pos] = b'0' + (v / 10) % 10;
    buf[pos + 1] = b'0' + v % 10;
    pos + 2
}

/// Append a small unsigned decimal (no padding) to `buf` at `pos`, returning the new position.
fn push_num(buf: &mut [u8], mut pos: usize, mut v: usize) -> usize {
    if v == 0 {
        buf[pos] = b'0';
        return pos + 1;
    }
    let start = pos;
    let mut tmp = [0u8; 10];
    let mut n = 0;
    while v > 0 {
        tmp[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        buf[pos] = tmp[n];
        pos += 1;
    }
    let _ = start;
    pos
}

/// The 16x11 arrow (0 = transparent, 1 = dark outline, 2 = white fill) matching the software cursor,
/// rasterized into the top-left of a 64x64 ARGB buffer for the hardware cursor plane. Hotspot at (0,0).
const ARROW_HW: [[u8; 11]; 16] = [
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

/// Build the 64x64 ARGB cursor bitmap and hand it to the kernel. Transparent (alpha 0) outside the
/// arrow; 0xFF-alpha dark outline / white fill inside — the display engine alpha-composites it.
fn upload_arrow_cursor() {
    let mut img = [0u32; 64 * 64];
    for (r, row) in ARROW_HW.iter().enumerate() {
        for (c, &p) in row.iter().enumerate() {
            let color = match p {
                1 => Color::TEXT_DARK, // already 0xFF-alpha ARGB
                2 => Color::WHITE,
                _ => 0, // transparent
            };
            if color != 0 {
                img[r * 64 + c] = color;
            }
        }
    }
    sys_cursor_set_image(&img);
}

// ── U7: resize-from-any-edge ────────────────────────────────────────────────
/// Grab band (px) just inside a window's border where a drag starts a resize.
const RESIZE_BAND: usize = 8;

/// Taskbar window buttons. Icon-only now: a title-width button meant the bar was a row of text
/// labels, and a minimized window "became" its name rather than its icon. Square keeps the group
/// compact and lets far more windows fit before the cap in `tb_button_count` bites.
const TB_BTN_W: usize = 36;
const TB_BTN_H: usize = 28;
const TB_GAP: usize = 6;
/// Width reserved for the two-line clock block (time over date).
const CLOCK_W: usize = 84;

/// Edge bitmask for point (mx,my) relative to window `win`: L=1 R=2 T=4 B=8 (corners = combos), 0 =
/// not on a resize edge. The band is the inner `RESIZE_BAND` px of the FULL window rect (title + content).
/// Maximized / minimized windows aren't resizable → 0.
fn window_resize_edges(win: &Window, mx: usize, my: usize) -> u8 {
    if win.is_minimized || win.is_maximized { return 0; }
    let x0 = win.x; let y0 = win.y;
    let x1 = win.x + win.w; let y1 = win.y + win.h + 30; // +30 title bar
    if mx < x0 || mx > x1 || my < y0 || my > y1 { return 0; }
    let b = RESIZE_BAND;
    let mut e = 0u8;
    if mx < x0 + b { e |= 1; }            // left
    if mx > x1.saturating_sub(b) { e |= 2; } // right
    if my < y0 + b { e |= 4; }            // top
    if my > y1.saturating_sub(b) { e |= 8; } // bottom
    e
}

/// Map a resize-edge bitmask to the cursor shape.
fn cursor_for_edges(e: u8) -> CursorType {
    match e {
        1 | 2 => CursorType::ResizeH,          // left / right
        4 | 8 => CursorType::ResizeV,          // top / bottom
        5 | 10 => CursorType::ResizeNWSE,      // TL(4|1) / BR(8|2)
        6 | 9 => CursorType::ResizeNESW,       // TR(4|2) / BL(8|1)
        _ => CursorType::Arrow,
    }
}

fn plot_disc(img: &mut [u32; 64 * 64], cx: i32, cy: i32, r: i32, color: u32) {
    let r2 = r * r;
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r2 {
                let (x, y) = (cx + dx, cy + dy);
                if x >= 0 && x < 64 && y >= 0 && y < 64 { img[(y * 64 + x) as usize] = color; }
            }
        }
    }
}

fn thick_line(img: &mut [u32; 64 * 64], x0: i32, y0: i32, x1: i32, y1: i32, r: i32, color: u32) {
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * i / steps;
        let y = y0 + (y1 - y0) * i / steps;
        plot_disc(img, x, y, r, color);
    }
}

/// Render a double-headed resize arrow (white fill + dark outline) along unit direction (ux,uy) into a
/// 64x64 HW-cursor image, centred near the pointer hotspot (~16,16 so it reads close to the mouse point).
fn double_arrow(img: &mut [u32; 64 * 64], ux: f32, uy: f32) {
    let (cx, cy) = (16.0f32, 16.0f32);
    let l = 12.0f32;  // half shaft length
    let bh = 6.0f32;  // barb length
    let (px, py) = (-uy, ux); // perpendicular
    let (t1x, t1y) = (cx - ux * l, cy - uy * l);
    let (t2x, t2y) = (cx + ux * l, cy + uy * l);
    // Outline pass (dark, r=3) then fill pass (white, r=2).
    for &(color, r) in &[(Color::TEXT_DARK, 3i32), (Color::WHITE, 2i32)] {
        thick_line(img, t1x as i32, t1y as i32, t2x as i32, t2y as i32, r, color);
        // arrowhead barbs at tip2 (pointing +u) and tip1 (pointing -u)
        let (bx, by) = (t2x - ux * bh, t2y - uy * bh);
        thick_line(img, t2x as i32, t2y as i32, (bx + px * bh) as i32, (by + py * bh) as i32, r, color);
        thick_line(img, t2x as i32, t2y as i32, (bx - px * bh) as i32, (by - py * bh) as i32, r, color);
        let (ax, ay) = (t1x + ux * bh, t1y + uy * bh);
        thick_line(img, t1x as i32, t1y as i32, (ax + px * bh) as i32, (ay + py * bh) as i32, r, color);
        thick_line(img, t1x as i32, t1y as i32, (ax - px * bh) as i32, (ay - py * bh) as i32, r, color);
    }
}

/// Upload the 64x64 HW-cursor image for a cursor shape. Arrow uses the arrow bitmap; resize shapes are
/// drawn as double-arrows. Called only when the cursor shape CHANGES (see process_input hover logic).
fn upload_cursor(t: CursorType) {
    if matches!(t, CursorType::Arrow) { upload_arrow_cursor(); return; }
    let mut img = [0u32; 64 * 64];
    match t {
        CursorType::ResizeH => double_arrow(&mut img, 1.0, 0.0),
        CursorType::ResizeV => double_arrow(&mut img, 0.0, 1.0),
        CursorType::ResizeNWSE => double_arrow(&mut img, 0.7071, 0.7071),
        CursorType::ResizeNESW => double_arrow(&mut img, 0.7071, -0.7071),
        _ => { upload_arrow_cursor(); return; }
    }
    sys_cursor_set_image(&img);
}

/// Draw the bottom taskbar strip: the opaque bar and its top border, nothing else. The Start button,
/// window buttons, clock and tray are drawn by the main pass, which has the compositor state (icon
/// bitmaps, link status, hover) this function doesn't.
fn render_taskbar(canvas: &mut Canvas, screen_stride: usize, screen_h: usize) {
    let start_y = screen_h - BAR_H;
    canvas.fill_rect(0, start_y, screen_stride, BAR_H, 0xFF_FFFFFF); // Opaque white taskbar
    canvas.fill_rect(0, start_y, screen_stride, 1, 0xFF_D1D1D1);     // Border
}

/// Signal strength to show for a link state, as (lit bars, colour). We have no RSSI from the driver,
/// so the bars encode CONNECTION state rather than signal quality — three lit means associated with
/// a lease, not "excellent reception".
fn wifi_indicator(state: u32) -> (usize, u32) {
    match state {
        WIFI_CONNECTED => (3, Color::ACCENT_GREEN),
        WIFI_CONNECTING => (2, Color::ACCENT_PRIMARY),
        WIFI_AUTH_FAILED | WIFI_NO_LEASE | WIFI_HW_FAILED => (1, 0xFF_C0392B),
        _ => (0, Color::TEXT_MUTED),
    }
}

/// Tray Wi-Fi glyph: three rising bars, greyed as a track with the lit ones overdrawn in the state
/// colour. `x,y` is the top-left of a 22x18 box.
fn draw_wifi_glyph(canvas: &mut Canvas, x: usize, y: usize, state: u32) {
    let (lit, color) = wifi_indicator(state);
    let base = y + 16;
    for i in 0..3 {
        let h = 6 + i * 5;
        let bx = x + i * 7;
        canvas.fill_rect(bx, base - h, 5, h, 0xFF_DCDCD6);
        if i < lit {
            canvas.fill_rect(bx, base - h, 5, h, color);
        }
    }
    // Not-connected reads as "off" rather than "weak": strike the track through.
    if lit == 0 {
        canvas.fill_rect(x, base - 4, 19, 2, Color::TEXT_MUTED);
    }
}


pub struct WindowClient {
    pub win: Window,
    pub owner_pid: u64,
    pub shm_id: u64,
    pub buffer: *const u32,
    pub buf_w: usize,
    pub buf_h: usize,
    pub gpu_gva: u32,
    /// The app's icon at taskbar size, decoded once when the window was created.
    pub icon: Option<Vec<u32>>,
}

fn get_str_len(buf: &[u8; 64]) -> usize { buf.iter().position(|&c| c == 0).unwrap_or(64) }
fn get_icon_len(buf: &[u8; 96]) -> usize { buf.iter().position(|&c| c == 0).unwrap_or(96) }

/// Boot B: read a client's reported scroll state from its (already-mapped) WindowHeader. The pixel
/// buffer starts at `header + size_of::<WindowHeader>()`, so the header sits just before `buffer`.
fn client_scroll(client: &WindowClient) -> (usize, usize) {
    if client.buffer.is_null() { return (0, 0); }
    let hdr = unsafe {
        &*((client.buffer as *const u8).sub(core::mem::size_of::<WindowHeader>()) as *const WindowHeader)
    };
    (hdr.content_h as usize, hdr.scroll_off as usize)
}

/// Boot B: map a mouse Y within a scrollbar track to a content scroll offset, centering the thumb on
/// the cursor. `bar_y` = track top, `viewport` = track height (= visible content height), `content` =
/// total content height. Returns the clamped top offset in px.
fn mouse_to_scroll(my: usize, bar_y: usize, viewport: usize, content: usize) -> usize {
    if content <= viewport { return 0; }
    let (_, thumb_h) = nyx_gui::ui::scrollbar_thumb(viewport, content, viewport, 0);
    let track_range = viewport.saturating_sub(thumb_h); // how far the thumb can travel
    if track_range == 0 { return 0; }
    let rel = my.saturating_sub(bar_y).saturating_sub(thumb_h / 2).min(track_range);
    let max_scroll = content - viewport;
    rel * max_scroll / track_range
}

pub struct CompositorState {
    pub clients: Vec<WindowClient>,
    pub next_win_id: usize,

    pub mx: usize, pub my: usize,
    pub prev_mx: usize, pub prev_my: usize,
    pub left_click: bool, pub prev_left: bool,

    pub dirty_min_x: usize, pub dirty_min_y: usize,
    pub dirty_max_x: usize, pub dirty_max_y: usize,
    pub needs_redraw: bool,

    pub dragging_win_idx: Option<usize>,
    pub drag_off_x: usize, pub drag_off_y: usize,
    
    pub is_resizing: bool,
    pub resizing_win_idx: Option<usize>,
    // U7: which edges the active resize is dragging — bitmask L=1 R=2 T=4 B=8 (corners = combos).
    pub resize_edges: u8,
    // U7: the cursor shape currently uploaded to the HW cursor plane (avoid re-uploading every frame).
    pub cursor_type: CursorType,

    // U7 boot 3 (focus & hover polish): the title-bar button under the pointer — (client idx, button
    // 0=close/1=min/2=max) — and the taskbar window-button under the pointer. Diffed each frame so a
    // bare pointer move (which dirties nothing under the HW cursor) still repaints the hover highlight.
    pub hover_win_btn: Option<(usize, u8)>,
    pub hover_tb_btn: Option<usize>,

    // Boot B: the window whose scrollbar the user is dragging (None = not scrolling).
    pub scrolling_win_idx: Option<usize>,

    pub start_menu_open: bool,
    pub screen_w: usize, pub screen_h: usize, pub screen_stride: usize,

    // U1: true once the GPU hardware cursor plane is live — the compositor then stops drawing and
    // dirtying the software cursor (the kernel moves the HW cursor straight from the mouse IRQ).
    pub hw_cursor: bool,

    // U0 instrumentation + live clock.
    pub last_clock_sec: usize,   // whole-second value of the last clock tick (drives 1Hz redraw)
    pub frame_count: u64,        // total redraws since boot
    pub last_frame_ms: usize,    // draw time of the most recent frame (ms)
    pub fps: usize,              // frames composited in the last rolling 1s window
    pub fps_window_start: usize, // uptime-ms at the start of the current FPS window
    pub fps_window_frames: usize,// frames counted so far in the current window
    pub fps_logs: usize,         // number of [UI] baseline lines emitted (capped, then silent)
    pub vsync_timeouts: u64,     // U6: presents where sys_wait_vsync timed out (mis-paced panel)

    // Desktop chrome artwork, decoded once at startup. Parallel to MENU; None = that icon failed to
    // load and the entry renders label-only.
    pub icons: Vec<Option<Vec<u32>>>,
    pub logo: Option<Vec<u32>>,        // nyx mark, LOGO_BTN px, for the Start button
    pub logo_big: Option<Vec<u32>>,    // same mark at MENU_ICON px, for the menu header
    /// Last-known Wi-Fi link state for the tray glyph, refreshed on the 1 Hz clock tick (a syscall
    /// per frame would be pointless — the link changes on human timescales).
    pub wifi_state: u32,

    /// Start-menu search box. A fixed buffer rather than a String: it is bounded by construction and
    /// keeps the compositor's per-keystroke path allocation-free.
    pub search: [u8; MENU_SEARCH_MAX],
    pub search_len: usize,
}

impl CompositorState {
    pub fn new(w: usize, h: usize, stride: usize) -> Self {
        Self {
            clients: Vec::new(), next_win_id: 0,
            mx: w / 2, my: h / 2, prev_mx: w / 2, prev_my: h / 2,
            left_click: false, prev_left: false,
            dirty_min_x: 0, dirty_min_y: 0, dirty_max_x: stride, dirty_max_y: h,
            needs_redraw: true,
            dragging_win_idx: None, drag_off_x: 0, drag_off_y: 0,
            is_resizing: false, resizing_win_idx: None, resize_edges: 0,
            cursor_type: CursorType::Arrow,
            hover_win_btn: None, hover_tb_btn: None,
            scrolling_win_idx: None,
            start_menu_open: false,
            screen_w: w, screen_h: h, screen_stride: stride,
            hw_cursor: false,
            icons: Vec::new(), logo: None, logo_big: None,
            wifi_state: WIFI_IDLE,
            last_clock_sec: 0,
            frame_count: 0, last_frame_ms: 0,
            fps: 0, fps_window_start: 0, fps_window_frames: 0, fps_logs: 0,
            vsync_timeouts: 0,
            search: [0; MENU_SEARCH_MAX], search_len: 0,
        }
    }

    /// Indices into MENU that match the search box, in table order. Empty query = everything.
    fn menu_filtered(&self) -> Vec<usize> {
        let q = &self.search[..self.search_len];
        MENU.iter()
            .enumerate()
            .filter(|(_, e)| label_matches(e.label, q))
            .map(|(i, _)| i)
            .take(MENU_COLS * MENU_ROWS)
            .collect()
    }

    /// Rect of the `slot`-th visible tile (slot = position in the FILTERED list, not the MENU index).
    /// ★ One source of truth for the grid, shared by the renderer, the hover highlight and the click
    /// hit-test — the flat list already taught us what happens when those drift apart.
    fn menu_tile_rect(&self, slot: usize) -> (usize, usize, usize, usize) {
        let (mx, my, _, _) = self.start_menu_rect();
        (mx + MENU_PAD + (slot % MENU_COLS) * MENU_TILE_W,
         my + MENU_GRID_Y + (slot / MENU_COLS) * MENU_TILE_H,
         MENU_TILE_W, MENU_TILE_H)
    }

    /// Which visible slot the pointer is over, bounded by how many tiles are actually drawn.
    fn menu_slot_at(&self, mx: usize, my: usize) -> Option<usize> {
        for slot in 0..self.menu_filtered().len() {
            let (tx, ty, tw, th) = self.menu_tile_rect(slot);
            if mx >= tx && mx < tx + tw && my >= ty && my < ty + th { return Some(slot); }
        }
        None
    }

    /// The search field's rect inside the panel.
    fn menu_search_rect(&self) -> (usize, usize, usize, usize) {
        let (mx, my, mw, _) = self.start_menu_rect();
        (mx + MENU_PAD, my + MENU_HEADER_H + MENU_SEARCH_GAP,
         mw - MENU_PAD * 2, MENU_SEARCH_H)
    }

    /// Open/close the launcher. Always clears the query — a stale filter from last time would show a
    /// menu that looks half-empty for no visible reason.
    fn set_start_menu(&mut self, open: bool) {
        self.start_menu_open = open;
        self.search_len = 0;
        self.mark_full_redraw();
    }

    pub fn mark_dirty(&mut self, x: usize, y: usize, w: usize, h: usize) {
        self.dirty_min_x = self.dirty_min_x.min(x);
        self.dirty_min_y = self.dirty_min_y.min(y);
        self.dirty_max_x = self.dirty_max_x.max(x + w).min(self.screen_stride);
        self.dirty_max_y = self.dirty_max_y.max(y + h).min(self.screen_h);
        self.needs_redraw = true;
    }

    // ── Chrome geometry. Single source of truth for both the renderer and the hit-tests; these used
    // to be recomputed inline in each, which is how the WIFI button ended up with a hit box that no
    // longer matched its label. ──

    /// Start button: x, y, w, h. Leftmost item of the centred taskbar group, so it shifts as windows
    /// open and close — the group stays centred rather than the button.
    pub fn start_button_rect(&self) -> (usize, usize, usize, usize) {
        (
            self.tb_group_x(),
            self.screen_h - BAR_H + (BAR_H - START_BTN_H) / 2,
            START_BTN_W,
            START_BTN_H,
        )
    }

    /// Clock block: x, y, w, h — two stacked lines (time over date) on the right, where every desktop
    /// puts it. Anchored to the VISIBLE width, not the stride: on a panel that pads its scanline the
    /// two differ, and a stride-relative x lands outside the GPU text viewport.
    pub fn clock_rect(&self) -> (usize, usize, usize, usize) {
        (self.screen_w.saturating_sub(CLOCK_W + 16), self.screen_h - BAR_H + 4, CLOCK_W, BAR_H - 8)
    }

    /// Tray Wi-Fi indicator: x, y, w, h. Sits just left of the clock.
    pub fn tray_wifi_rect(&self) -> (usize, usize, usize, usize) {
        let (cx, _, _, _) = self.clock_rect();
        (cx.saturating_sub(34), self.screen_h - BAR_H + 9, 22, 18)
    }

    /// Start menu: x, y, w, h.
    pub fn start_menu_rect(&self) -> (usize, usize, usize, usize) {
        let mh = menu_h();
        let mx = (self.screen_w / 2) - (MENU_W / 2);
        (mx, self.screen_h.saturating_sub(BAR_H + mh + 8), MENU_W, mh)
    }

    pub fn mark_full_redraw(&mut self) {
        self.dirty_min_x = 0; self.dirty_min_y = 0;
        self.dirty_max_x = self.screen_stride; self.dirty_max_y = self.screen_h;
        self.needs_redraw = true;
    }

    /// D1: mark a window's FULL on-screen footprint dirty — the title bar (win.y is its TOP), the
    /// content, AND the drop shadow, which overhangs the window rect on every side (draw_window_shadow:
    /// blur=9 L/R, off_y+blur below, blur-off_y above). The old per-site marks used (win.x, win.y,
    /// win.w+15, win.h+45): they padded only the RIGHT/BOTTOM, so the LEFT/TOP shadow+border of a
    /// window's previous position was never re-filled → it left a border trace when dragged. A single
    /// symmetric margin (covers the 9px shadow blur with slack) fixes all sites at once. Coords are the
    /// window's own x/y/w/h; the 30px title height is added into the vertical extent here.
    fn mark_win_footprint(&mut self, x: usize, y: usize, w: usize, h: usize) {
        const M: usize = 16; // shadow/border margin, all four sides (shadow blur is 9)
        let fx = x.saturating_sub(M);
        let fy = y.saturating_sub(M);
        let fw = w + M * 2;
        let fh = h + 30 + M * 2; // +30 title bar
        self.mark_dirty(fx, fy, fw, fh);
    }

    /// How many windows currently get a taskbar button, capped so the group can never run into the
    /// clock on the right.
    fn tb_button_count(&self) -> usize {
        let live = self.clients.iter().filter(|c| c.win.exists).count();
        // Half the screen, minus the Start button, is the widest the group may be.
        let budget = (self.screen_w / 2).saturating_sub(START_BTN_W + TB_GAP) / (TB_BTN_W + TB_GAP);
        live.min(budget)
    }

    /// Total width of the Start button + window buttons, drawn as one centred group.
    fn tb_group_width(&self) -> usize {
        let n = self.tb_button_count();
        START_BTN_W + if n == 0 { 0 } else { TB_GAP + n * (TB_BTN_W + TB_GAP) - TB_GAP }
    }

    fn tb_group_x(&self) -> usize {
        (self.screen_w / 2).saturating_sub(self.tb_group_width() / 2)
    }

    /// Layout of taskbar window buttons — `(client_idx, button_x)`, running rightward from the Start
    /// button as one centred group. Shared by the render pass and the click hit-test so they can never
    /// drift out of sync.
    fn taskbar_buttons(&self) -> Vec<(usize, usize)> {
        let start = self.tb_group_x() + START_BTN_W + TB_GAP;
        let cap = self.tb_button_count();
        let mut out = Vec::new();
        for (i, client) in self.clients.iter().enumerate() {
            if !client.win.exists { continue; }
            if out.len() >= cap { break; }
            out.push((i, start + out.len() * (TB_BTN_W + TB_GAP)));
        }
        out
    }

    /// U7: the active (focused) window = topmost existing, non-minimized window (last in the z-order).
    fn active_window(&self) -> Option<usize> {
        self.clients.iter().rposition(|c| c.win.exists && !c.win.is_minimized)
    }

    /// U7 boot 3: which title-bar button (close=0/min=1/max=2) of which window sits under the pointer,
    /// top-down. Stops at the topmost window the pointer is over (a lower window's buttons can't be
    /// hovered through an upper one) — mirrors the click hit-test / cursor logic. Rects come from
    /// `nyx_gui::ui::ctl_btn_hit`, which is also what draws them, so hover can't drift from the plates.
    fn title_btn_at(&self, mx: usize, my: usize) -> Option<(usize, u8)> {
        for (idx, client) in self.clients.iter().enumerate().rev() {
            let w = &client.win;
            if !w.exists || w.is_minimized { continue; }
            if let Some(b) = ctl_btn_hit(w.x, w.y, mx, my) { return Some((idx, b)); }
            // Over this (topmost hit) window but not on a button → nothing hovered; stop descending.
            if mx >= w.x && mx <= w.x + w.w && my >= w.y && my <= w.y + w.h + 30 { return None; }
        }
        None
    }

    /// U7 boot 3: which taskbar window-button (client idx) sits under the pointer. Layout shared with
    /// `taskbar_buttons()` so hover, render, and click never drift.
    fn tb_btn_at(&self, mx: usize, my: usize) -> Option<usize> {
        let btn_y = self.screen_h.saturating_sub(36) + 6;
        if my < btn_y || my >= btn_y + TB_BTN_H { return None; }
        for (ci, bx) in self.taskbar_buttons() {
            if mx >= bx && mx < bx + TB_BTN_W { return Some(ci); }
        }
        None
    }

    /// Raise a window to the top of the z-order (end of `clients`). Returns its new index.
    fn raise_window(&mut self, idx: usize) -> usize {
        if idx + 1 >= self.clients.len() { return idx; }
        let c = self.clients.remove(idx);
        self.clients.push(c);
        self.clients.len() - 1
    }

    pub fn process_ipc(&mut self) {
        let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
        while sys_ipc_recv(&mut msg, false) {
            match msg.msg_type {
                MSG_REQ_WINDOW => {
                    let shm_id = msg.data1;
                    let vaddr = sys_map_shm(shm_id) as *mut u8;
                    let header = unsafe { &*(vaddr as *const WindowHeader) };
                    if header.magic == WIN_MAGIC {
                        let w = header.width as usize; let h = header.height as usize;
                        let x = if header.requested_x == -1 { 100 + (self.next_win_id * 30) } else { header.requested_x as usize };
                        let y = if header.requested_y == -1 { 100 + (self.next_win_id * 30) } else { header.requested_y as usize };
                        
                        let gpu_gva = 0x2000_0000 + (self.next_win_id * 0x0100_0000) as u32;
                        sys_gpu_map_shm(shm_id, gpu_gva);

                        // Decode the app's own icon once, at window-creation time. The app declares
                        // the path (it knows its bundle; we only know its pid), and an empty or
                        // unreadable one just leaves None — the button then shows its initial.
                        let icon = {
                            let n = get_icon_len(&header.icon);
                            if n == 0 { None } else {
                                core::str::from_utf8(&header.icon[..n]).ok()
                                    .and_then(|p| load_icon(p, TB_ICON))
                            }
                        };

                        self.clients.push(WindowClient {
                            win: Window { 
                                id: self.next_win_id, x, y, w, h, 
                                title: header.title, title_len: get_str_len(&header.title), 
                                active: true, exists: true, opacity: 0,
                                is_minimized: false, is_maximized: false,
                                saved_x: 0, saved_y: 0, saved_w: 0, saved_h: 0
                            },
                            owner_pid: msg.sender_pid, shm_id, buffer: unsafe { vaddr.add(core::mem::size_of::<WindowHeader>()) } as *const u32,
                            buf_w: w, buf_h: h,
                            gpu_gva, icon,
                        });
                        self.next_win_id += 1;
                        self.mark_full_redraw();
                        sys_ipc_send(msg.sender_pid, MSG_WINDOW_CREATED, shm_id, 0);
                    }
                },
                MSG_WINDOW_UPDATE_SHM => {
                    let new_shm_id = msg.data1;
                    let hdr_sz = core::mem::size_of::<WindowHeader>();
                    if let Some(client) = self.clients.iter_mut().find(|c| c.owner_pid == msg.sender_pid) {
                        // R1: capture the OLD buffer's mapping BEFORE we overwrite it, so we can release
                        // it and hand ownership back to the app for reclamation. The mapping base is the
                        // page under the pixels (client.buffer = base + header). Old size from old dims.
                        let old_shm_id = client.shm_id;
                        let old_base = (client.buffer as u64).saturating_sub(hdr_sz as u64);
                        let old_size = hdr_sz + client.buf_w * client.buf_h * 4;
                        let owner = client.owner_pid;

                        // Switch to the new buffer: CPU mapping, dims, and the GGTT remap onto the SAME
                        // GVA (this overwrites the old GGTT PTEs for the live extent → the GPU no longer
                        // references the old frames; any shrink-tail PTEs are never sampled).
                        let vaddr = sys_map_shm(new_shm_id) as *mut u8;
                        let header = unsafe { &*(vaddr as *const WindowHeader) };
                        client.shm_id = new_shm_id;
                        client.buffer = unsafe { vaddr.add(hdr_sz) } as *const u32;
                        client.buf_w = header.width as usize;
                        client.buf_h = header.height as usize;
                        sys_gpu_map_shm(new_shm_id, client.gpu_gva);

                        // R1: now that nothing here references the old buffer, release the compositor's
                        // own CPU mapping of it (frees the VA + drops the reference — but does NOT free
                        // the frames; the app owns those) and ACK the app so IT can destroy them. Guard
                        // against a no-op update (same id) so we never unmap the live buffer.
                        if old_shm_id != new_shm_id && old_base != 0 {
                            sys_unmap_shm(old_base, old_size);
                            sys_ipc_send(owner, MSG_SHM_RELEASED, old_shm_id, 0);
                        }
                        self.mark_full_redraw();
                    }
                },
                MSG_FLUSH_WINDOW => {
                    let dirty_rect = self.clients.iter()
                        .find(|c| c.owner_pid == msg.sender_pid)
                        .map(|c| (c.win.x, c.win.y, c.win.w, c.win.h));
                    if let Some((x, y, w, h)) = dirty_rect { self.mark_win_footprint(x, y, w, h); }
                },
                _ => {}
            }
        }
    }

    pub fn process_input(&mut self) {
        if let Some(key) = sys_read_key() {
            if key == nyx_api::keys::SUPER {
                // The Super/Windows key belongs to the shell, not the focused app — swallow it.
                let open = !self.start_menu_open;
                self.set_start_menu(open);
            } else if self.start_menu_open {
                // While the launcher is open it OWNS the keyboard — that is what makes the search box
                // work at all, since the compositor otherwise forwards every key to the focused app.
                match key {
                    '\x1b' => self.set_start_menu(false),               // Esc dismisses
                    '\x08' => {                                        // Backspace
                        self.search_len = self.search_len.saturating_sub(1);
                        self.mark_full_redraw();
                    }
                    '\n' | '\r' => {                                   // Enter launches the top hit
                        let first = self.menu_filtered().first().copied();
                        if let Some(i) = first {
                            if sys_fork() == 0 { sys_execve(MENU[i].exec); sys_exit(1); }
                        }
                        self.set_start_menu(false);
                    }
                    c if (c as u32) >= 0x20 && (c as u32) < 0x7F => {
                        if self.search_len < MENU_SEARCH_MAX {
                            self.search[self.search_len] = c as u8;
                            self.search_len += 1;
                            self.mark_full_redraw();
                        }
                    }
                    _ => {}                                            // arrows/PUA: ignored for now
                }
            } else if let Some(top_client) = self.clients.iter().rev().find(|c| c.win.exists && !c.win.is_minimized) {
                sys_ipc_send(top_client.owner_pid, MSG_KEY_EVENT, key as u64, 0);
            }
        }

        let (mx_raw, my_raw, left_click, _right) = sys_get_mouse();
        self.mx = mx_raw.clamp(0, self.screen_w - 1); 
        self.my = my_raw.clamp(0, self.screen_h - 1);
        self.left_click = left_click;

        if self.mx != self.prev_mx || self.my != self.prev_my {
            // Only the software cursor needs a redraw on move; the hardware cursor is composited by
            // the scanout engine, so a bare pointer move dirties nothing (the U1 win).
            if !self.hw_cursor {
                let pad = 20;
                self.mark_dirty(self.prev_mx.saturating_sub(pad), self.prev_my.saturating_sub(pad), pad * 2, pad * 2);
                self.mark_dirty(self.mx.saturating_sub(pad), self.my.saturating_sub(pad), pad * 2, pad * 2);
            }
        }

        if self.left_click && !self.prev_left {
            let mut clicked_idx: Option<usize> = None;

            let (btn_x, btn_y, btn_w, _btn_h) = self.start_button_rect();
            let (net_x, net_y, net_w, net_h) = self.tray_wifi_rect();
            let (menu_x, menu_y, menu_w, menu_h) = self.start_menu_rect();
            let tb_btn_y = self.screen_h - BAR_H + 6;

            // U7: which taskbar window button (if any) is under the pointer — computed before the click
            // chain so no borrow of self is held while we later mutate.
            let tb_btn_hit = {
                let mut h: Option<usize> = None;
                if self.my >= tb_btn_y && self.my < tb_btn_y + TB_BTN_H {
                    for (ci, bx) in self.taskbar_buttons() {
                        if self.mx >= bx && self.mx < bx + TB_BTN_W { h = Some(ci); break; }
                    }
                }
                h
            };

            if self.start_menu_open && self.mx >= menu_x && self.mx < menu_x + menu_w
                && self.my >= menu_y && self.my < menu_y + menu_h {
                // A click on a tile launches it; anywhere else in the panel (header, search field,
                // empty grid) just dismisses. Clicking the search box keeps the menu open, since
                // that's plainly an intent to type rather than to leave.
                let (sx, sy, sw, sh) = self.menu_search_rect();
                let in_search = self.mx >= sx && self.mx < sx + sw && self.my >= sy && self.my < sy + sh;
                if let Some(slot) = self.menu_slot_at(self.mx, self.my) {
                    if let Some(&i) = self.menu_filtered().get(slot) {
                        if sys_fork() == 0 { sys_execve(MENU[i].exec); sys_exit(1); }
                    }
                    self.set_start_menu(false);
                } else if !in_search {
                    self.set_start_menu(false);
                }
            }
            else if self.mx >= btn_x && self.mx < btn_x + btn_w && self.my >= btn_y && self.my < btn_y + START_BTN_H {
                let open = !self.start_menu_open;
                self.set_start_menu(open);
            }
            else if self.mx >= net_x && self.mx < net_x + net_w && self.my >= net_y && self.my < net_y + net_h {
                // The tray Wi-Fi glyph opens the network picker. (This used to exec /bin/nyx-network,
                // a path that has never existed on an installed system — the button did nothing.)
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Wifi.nyx/run.bin\0"); sys_exit(1); }
                self.start_menu_open = false;
                self.mark_full_redraw();
            }
            else if let Some(ci) = tb_btn_hit {
                // U7 taskbar button: minimized → restore + raise + focus; the active (topmost) window →
                // minimize; any other window → raise + focus. Full redraw (z-order / visibility changed).
                if self.clients[ci].win.is_minimized {
                    self.clients[ci].win.is_minimized = false;
                    self.raise_window(ci);
                } else if self.active_window() == Some(ci) {
                    self.clients[ci].win.is_minimized = true;
                } else {
                    self.raise_window(ci);
                }
                if self.start_menu_open { self.start_menu_open = false; }
                self.mark_full_redraw();
            } else {
                if self.start_menu_open { self.start_menu_open = false; self.mark_full_redraw(); }

                for (idx, client) in self.clients.iter_mut().enumerate().rev() {
                    if !client.win.exists { continue; }
                    // U7: minimized windows are hidden — they can't intercept desktop clicks (only their
                    // taskbar button, handled above). Skip them here.
                    if client.win.is_minimized { continue; }
                    let win_x = client.win.x; let win_y = client.win.y; let win_w = client.win.w;
                    let win_h = client.win.h + 30;

                    // U7: resize from ANY edge/corner. Grab band is the inner 8px of the full window
                    // border; tested BEFORE the title-drag/buttons (the top 8px band sits above the
                    // y+10 buttons, so no conflict). The edge bitmask drives both the geometry apply
                    // and the cursor shape.
                    let edges = window_resize_edges(&client.win, self.mx, self.my);
                    if edges != 0 {
                        self.is_resizing = true;
                        self.resizing_win_idx = Some(idx);
                        self.resize_edges = edges;
                        clicked_idx = Some(idx); break;
                    }

                    // Control hit-tests come from nyx_gui::ui so they track the drawn plates exactly.
                    let ctl = ctl_btn_hit(win_x, win_y, self.mx, self.my);

                    if ctl == Some(0) {
                        client.win.exists = false;
                        sys_ipc_send(client.owner_pid, MSG_WINDOW_CLOSE, 0, 0);
                        self.mark_win_footprint(win_x, win_y, win_w, win_h);
                        clicked_idx = Some(idx); break;
                    }

                    if ctl == Some(1) {
                        // U7: minimize hides the window fully (it lives as a taskbar button). Full redraw
                        // — both the window's old area AND the taskbar (new button) change.
                        client.win.is_minimized = true;
                        self.mark_full_redraw();
                        clicked_idx = Some(idx); break;
                    }

                    if ctl == Some(2) {
                        if client.win.is_maximized {
                            client.win.x = client.win.saved_x; client.win.y = client.win.saved_y;
                            client.win.w = client.win.saved_w; client.win.h = client.win.saved_h;
                            client.win.is_maximized = false;
                        } else {
                            client.win.saved_x = client.win.x; client.win.saved_y = client.win.y;
                            client.win.saved_w = client.win.w; client.win.saved_h = client.win.h;
                            client.win.x = 0; client.win.y = 0;
                            client.win.w = self.screen_w; client.win.h = self.screen_h - 36 - 30;
                            client.win.is_maximized = true;
                        }
                        sys_ipc_send(client.owner_pid, MSG_WINDOW_RESIZED, client.win.w as u64, client.win.h as u64);
                        self.mark_full_redraw();
                        clicked_idx = Some(idx); break;
                    }

                    if self.mx >= win_x && self.mx <= win_x + win_w && self.my >= win_y && self.my <= win_y + 30 {
                        if !client.win.is_maximized {
                            self.dragging_win_idx = Some(idx); 
                            self.drag_off_x = self.mx - win_x; 
                            self.drag_off_y = self.my - win_y; 
                        }
                        clicked_idx = Some(idx); break; 
                    }
                    
                    // Boot B: scrollbar hit-test (content-area right edge). Only if the app reports a
                    // scrollable content height. Takes priority over the general content click below.
                    if !client.win.is_minimized {
                        let (content_h, _) = client_scroll(client);
                        let viewport = client.win.h;
                        if content_h > viewport {
                            let bar_x0 = win_x + win_w - SCROLLBAR_W;
                            let bar_y = win_y + 30;
                            if self.mx >= bar_x0 && self.mx <= win_x + win_w && self.my >= bar_y && self.my <= bar_y + viewport {
                                self.scrolling_win_idx = Some(idx);
                                let target = mouse_to_scroll(self.my, bar_y, viewport, content_h);
                                sys_ipc_send(client.owner_pid, MSG_SCROLL, target as u64, 0);
                                self.mark_win_footprint(win_x, win_y, win_w, win_h);
                                clicked_idx = Some(idx); break;
                            }
                        }
                    }

                    if !client.win.is_minimized && self.mx >= win_x && self.mx <= win_x + win_w && self.my > win_y + 30 && self.my <= win_y + win_h {
                        sys_ipc_send(client.owner_pid, MSG_MOUSE_EVENT, (self.mx - win_x) as u64, (self.my - (win_y + 30)) as u64);
                        clicked_idx = Some(idx); break;
                    }
                }

                if let Some(idx) = clicked_idx {
                    if idx != self.clients.len() - 1 {
                        let moved_client = self.clients.remove(idx);
                        self.clients.push(moved_client);
                        if self.dragging_win_idx == Some(idx) { self.dragging_win_idx = Some(self.clients.len() - 1); }
                        if self.resizing_win_idx == Some(idx) { self.resizing_win_idx = Some(self.clients.len() - 1); }
                        self.mark_full_redraw();
                    }
                }
            }
        } else if self.left_click {
            if let Some(idx) = self.resizing_win_idx {
                let (ox, oy, ow, oh) = (self.clients[idx].win.x, self.clients[idx].win.y,
                                        self.clients[idx].win.w, self.clients[idx].win.h);
                self.mark_win_footprint(ox, oy, ow, oh);

                // U7: adjust geometry per the grabbed edges. The NON-dragged edges stay anchored: a
                // right/bottom drag only grows w/h; a left/top drag moves x/y while keeping the opposite
                // edge fixed. Width is still snapped to a multiple of 16 (GPU LINEAR pitch %64) — on a
                // left drag we re-anchor x so the right edge doesn't creep from the snap.
                const MINW: usize = 208;
                const MINH: usize = 100;
                let e = self.resize_edges;
                let right = ox + ow;           // fixed right edge (screen x)
                let bottom = oy + 30 + oh;     // fixed content bottom (screen y)
                let (mut nx, mut ny, mut nw, mut nh) = (ox, oy, ow, oh);
                if e & 2 != 0 { nw = self.mx.saturating_sub(ox).max(MINW); }                       // right
                if e & 1 != 0 { let x = self.mx.min(right.saturating_sub(MINW)); nw = right - x; nx = x; } // left
                if e & 8 != 0 { nh = self.my.saturating_sub(oy + 30).max(MINH); }                  // bottom
                if e & 4 != 0 { let y = self.my.min(bottom.saturating_sub(30 + MINH)); nh = bottom - (y + 30); ny = y; } // top
                nw = nw.max(MINW) & !15;
                if e & 1 != 0 { nx = right.saturating_sub(nw); }                                   // keep right edge fixed
                nh = nh.max(MINH);

                if nw != ow || nh != oh || nx != ox || ny != oy {
                    self.clients[idx].win.x = nx;
                    self.clients[idx].win.y = ny;
                    self.clients[idx].win.w = nw;
                    self.clients[idx].win.h = nh;
                    sys_ipc_send(self.clients[idx].owner_pid, MSG_WINDOW_RESIZED, nw as u64, nh as u64);
                }
                self.mark_win_footprint(nx, ny, nw, nh);
            } else if let Some(idx) = self.dragging_win_idx {
                let (ox, oy, ow, oh) = (self.clients[idx].win.x, self.clients[idx].win.y,
                                        self.clients[idx].win.w, self.clients[idx].win.h);
                self.mark_win_footprint(ox, oy, ow, oh);

                self.clients[idx].win.x = self.mx.saturating_sub(self.drag_off_x);
                self.clients[idx].win.y = self.my.saturating_sub(self.drag_off_y);

                let (nx, ny) = (self.clients[idx].win.x, self.clients[idx].win.y);
                self.mark_win_footprint(nx, ny, ow, oh);
            } else if let Some(idx) = self.scrolling_win_idx {
                // Boot B: dragging the scrollbar thumb — recompute the target offset from the mouse Y
                // and push it to the app, which clamps + re-renders. Redraw the window region.
                if idx < self.clients.len() {
                    let (content_h, _) = client_scroll(&self.clients[idx]);
                    let viewport = self.clients[idx].win.h;
                    if content_h > viewport {
                        let bar_y = self.clients[idx].win.y + 30;
                        let target = mouse_to_scroll(self.my, bar_y, viewport, content_h);
                        sys_ipc_send(self.clients[idx].owner_pid, MSG_SCROLL, target as u64, 0);
                        let (x, y, w, h) = (self.clients[idx].win.x, self.clients[idx].win.y,
                                            self.clients[idx].win.w, self.clients[idx].win.h);
                        self.mark_win_footprint(x, y, w, h);
                    }
                }
            }
        } else if !self.left_click {
            self.dragging_win_idx = None;
            self.resizing_win_idx = None;
            self.scrolling_win_idx = None;
            self.is_resizing = false;
            self.resize_edges = 0;
        }

        self.prev_left = self.left_click;

        // U7: pick the cursor SHAPE from what's under the pointer. During an active resize it stays the
        // resize shape; while dragging/scrolling it's the arrow; otherwise it's the resize shape of the
        // topmost window's edge the pointer is on (or arrow). Re-upload the HW cursor image only when the
        // shape actually changes (cheap: a 64x64 blit, not per-frame).
        let desired = if self.is_resizing {
            cursor_for_edges(self.resize_edges)
        } else if self.dragging_win_idx.is_some() || self.scrolling_win_idx.is_some() {
            CursorType::Arrow
        } else {
            let mut c = CursorType::Arrow;
            for client in self.clients.iter().rev() {
                if !client.win.exists { continue; }
                let e = window_resize_edges(&client.win, self.mx, self.my);
                if e != 0 { c = cursor_for_edges(e); break; }
                // pointer is over this (topmost hit) window but not on an edge → plain arrow, stop.
                let (wx1, wy1) = (client.win.x + client.win.w, client.win.y + client.win.h + 30);
                if self.mx >= client.win.x && self.mx <= wx1 && self.my >= client.win.y && self.my <= wy1 {
                    break;
                }
            }
            c
        };
        if desired != self.cursor_type {
            self.cursor_type = desired;
            if self.hw_cursor { upload_cursor(desired); }
        }

        // U7 boot 3: recompute the hover-highlight targets (title-bar button + taskbar button). While a
        // drag/resize/scroll is in progress no title-bar button is "hovered" (the pointer is captured).
        // On any change, mark just the affected region dirty so the highlight repaints — a bare pointer
        // move dirties nothing under the HW cursor, so without this the highlight would never appear.
        let new_win_btn = if self.dragging_win_idx.is_some() || self.is_resizing || self.scrolling_win_idx.is_some() {
            None
        } else {
            self.title_btn_at(self.mx, self.my)
        };
        if new_win_btn != self.hover_win_btn {
            for wb in [self.hover_win_btn, new_win_btn] {
                if let Some((idx, _)) = wb {
                    if idx < self.clients.len() {
                        let w = &self.clients[idx].win;
                        let (x, y, ww, hh) = (w.x, w.y, w.w, w.h);
                        self.mark_win_footprint(x, y, ww, hh);
                    }
                }
            }
            self.hover_win_btn = new_win_btn;
        }
        let new_tb_btn = self.tb_btn_at(self.mx, self.my);
        if new_tb_btn != self.hover_tb_btn {
            self.hover_tb_btn = new_tb_btn;
            self.mark_dirty(0, self.screen_h.saturating_sub(BAR_H), self.screen_stride, BAR_H);
        }

        // Start-menu row highlight. Same reasoning as the buttons above: under the HW cursor a bare
        // pointer move dirties nothing, so without this the highlight would only appear on the next
        // 1 Hz clock tick. Only costs anything while the menu is actually open.
        if self.start_menu_open && (self.mx != self.prev_mx || self.my != self.prev_my) {
            let (mx0, my0, mw, mh) = self.start_menu_rect();
            self.mark_dirty(mx0, my0, mw, mh);
        }
    }

    pub fn update(&mut self) {
        for i in 0..self.clients.len() {
            if self.clients[i].win.exists && self.clients[i].win.opacity < 255 {
                self.clients[i].win.opacity = self.clients[i].win.opacity.saturating_add(15);
                let (x, y, w, h) = (
                    self.clients[i].win.x, self.clients[i].win.y,
                    self.clients[i].win.w, self.clients[i].win.h
                );
                self.mark_win_footprint(x, y, w, h);
            }
        }
    }
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    const HEAP_PAGES: usize = 4096; 
    let heap_start = sys_alloc_pages(HEAP_PAGES);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, HEAP_PAGES * 4096); }

    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    let fb_ptr = sys_map_framebuffer();
    let hardware_fb = unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };
    
    let mut state = CompositorState::new(screen_w, screen_h, screen_stride);

    // U1: try to bring up the GPU hardware cursor plane. On success, upload the arrow bitmap and stop
    // drawing/dirtying the software cursor; on failure, the software cursor path stays as fallback.
    state.hw_cursor = sys_cursor_init();
    if state.hw_cursor {
        upload_arrow_cursor();
        sys_print("[COMPOSITOR] Hardware cursor enabled (GPU cursor plane).\n");
    } else {
        sys_print("[COMPOSITOR] Hardware cursor unavailable; using software cursor.\n");
    }

    // U5 Boot T1: build the GPU font atlas ONCE (rasterize printable ASCII → a mapped B8G8R8A8 atlas
    // surface) so the compositor can draw a proof banner via the GPU text path (sys_gpu_draw_text).
    // None → keep everything CPU-drawn (no banner); the desktop is unaffected either way.
    // Decode the desktop's icon set once. Missing files are not fatal — each entry falls back to a
    // label-only row, so a stale install still boots to a usable menu.
    let mut loaded = 0usize;
    for e in MENU.iter() {
        let ic = load_icon(e.icon, MENU_ICON);
        if ic.is_some() { loaded += 1; }
        state.icons.push(ic);
    }
    state.logo = load_icon(NYX_LOGO, LOGO_BTN);
    state.logo_big = load_icon(NYX_LOGO, MENU_ICON);
    {
        let mut m = [0u8; 64];
        let mut mp = 0usize;
        for &b in b"[COMPOSITOR] icons loaded: " { m[mp] = b; mp += 1; }
        mp = push_num(&mut m, mp, loaded);
        m[mp] = b'/'; mp += 1;
        mp = push_num(&mut m, mp, MENU.len());
        m[mp] = b'\n'; mp += 1;
        if let Ok(s) = core::str::from_utf8(&m[..mp]) { sys_print(s); }
    }

    let gpu_font = nyx_gui::gpu_text::GpuFont::prepare();
    if let Some(f) = &gpu_font {
        // Log the atlas dims (sanity-check from serial).
        let mut m = [0u8; 96];
        let mut mp = 0usize;
        for &b in b"[COMPOSITOR] U5 text atlas ready: atlas=" { m[mp] = b; mp += 1; }
        mp = push_num(&mut m, mp, f.atlas_w as usize);
        m[mp] = b'x'; mp += 1;
        mp = push_num(&mut m, mp, f.atlas_h as usize);
        for &b in b" pitch=" { m[mp] = b; mp += 1; }
        mp = push_num(&mut m, mp, f.atlas_pitch as usize);
        m[mp] = b'\n'; mp += 1;
        if let Ok(s) = core::str::from_utf8(&m[..mp]) { sys_print(s); }
    } else {
        sys_print("[COMPOSITOR] U5 GPU font atlas unavailable; CPU text only.\n");
    }

    let mut last_frame = sys_get_time();
    let ms_per_frame = 1000 / 60;

    sys_print("[COMPOSITOR] Nyx Window Server Online. (Floating WM Restored)\n");

    loop {
        state.process_ipc();
        state.process_input();
        state.update();

        let now = sys_get_time();

        // Live-clock tick: once the whole-second value changes, force a full recomposite so the taskbar
        // clock (and FPS overlay) advance even on an idle desktop. NOTE: this per-second full render is
        // deliberately kept (an idle-only "repaint just the taskbar strip" optimization was tried and
        // REVERTED — it let the RENDER ENGINE go cold/RC6 during long idle, and the first composite after
        // idle then hung in wait_for_idle with lost MOCS/forcewake, freezing the whole compositor while
        // the HW cursor kept moving. Keeping a full GPU frame every second keeps the engine warm. Proper
        // region damage is the scissor-based boot D1, which readies the engine before compositing.)
        let now_sec = now / 1000;
        if now_sec != state.last_clock_sec {
            state.last_clock_sec = now_sec;
            // Refresh the tray Wi-Fi state on the same tick. Once a second is the right cadence: the
            // link changes on human timescales, and the frame is being recomposited anyway.
            state.wifi_state = sys_wifi_status().map(|s| s.state).unwrap_or(WIFI_IDLE);
            state.needs_redraw = true;
        }

        if !state.needs_redraw && now.wrapping_sub(last_frame) < ms_per_frame {
            sys_sleep_ms(2);
            continue;
        }
        last_frame = now;

        if state.needs_redraw {
            let frame_start = now;
            if !state.hw_cursor {
                state.mark_dirty(state.mx.saturating_sub(15), state.my.saturating_sub(15), 35, 35);
            }

            // D1 damage tracking: decide partial-region frame vs full frame from the dirty box that
            // mark_dirty accumulated this iteration (input / IPC MSG_FLUSH_WINDOW / fade / soft-cursor).
            // We force a FULL frame when:
            //  - the box is empty  → a flag-only redraw, i.e. the 1Hz clock heartbeat. Rendering it full
            //    keeps the RENDER engine warm; an idle-silent GPU drops to RC6 and the next composite
            //    hangs on lost MOCS/forcewake (the reverted "repaint just the taskbar" bug — see loop hdr).
            //  - any window is mid-fade (opacity<255) → the scissored present could clip a translucent
            //    quad and the src-over would double-blend on the next partial frame.
            //  - damage already covers most of the screen → the partial path saves nothing.
            // On a partial frame we scissor ONLY the two full-screen BLTs (wallpaper fill + present) to
            // the damage rect. The composite still draws ALL windows (stable quad set = no scene rebuild
            // = engine stays warm); recompositing an opaque (opacity 255 = replace) window over its own
            // pixels outside the rect is idempotent, and only the damage rect is ever shipped to scanout.
            let rect_empty = state.dirty_max_x <= state.dirty_min_x
                || state.dirty_max_y <= state.dirty_min_y;
            let mut fading = false;
            for c in state.clients.iter() {
                if c.win.exists && !c.win.is_minimized && c.win.opacity < 255 { fading = true; break; }
            }
            // Clamp the box to the visible screen (mark_dirty clamps x to the stride, which can exceed
            // the width on a padded framebuffer).
            let dmg_x = state.dirty_min_x.min(screen_w);
            let dmg_y = state.dirty_min_y.min(screen_h);
            let dmg_w = state.dirty_max_x.min(screen_w).saturating_sub(dmg_x);
            let dmg_h = state.dirty_max_y.min(screen_h).saturating_sub(dmg_y);
            let full_frame = rect_empty || fading
                || dmg_w.saturating_mul(dmg_h).saturating_mul(4) >= screen_w.saturating_mul(screen_h).saturating_mul(3);

            // 1. Submit GPU background fill (D1: whole screen on a full frame — byte-identical to before;
            // only the damage rect on a partial frame). The fill re-lays wallpaper under the composite.
            if full_frame {
                sys_gpu_fill_rect(0, 0, screen_stride, screen_h, Color::WARM_BG);
            } else {
                sys_gpu_fill_rect(dmg_x, dmg_y, dmg_w, dmg_h, Color::WARM_BG);
            }

            // 2. Synchronize! Wait for GPU wallpaper clear to finish before CPU starts drawing
            sys_gpu_sync();

            // 2b. U4: GPU-composite each visible window's CONTENT as a textured ortho quad directly
            // into the backbuffer (over the wallpaper). tex_gva = the window's page-aligned pixels =
            // gpu_gva + WindowHeader size. On success the CPU skips the per-window composite_buffer
            // copy and only draws chrome; on failure (e.g. non-16-aligned width) we fall back to the
            // full CPU composite for correctness.
            let hdr = core::mem::size_of::<WindowHeader>() as u32;
            let mut quads: Vec<WindowQuad> = Vec::new();
            for client in state.clients.iter() {
                if client.win.exists && !client.win.is_minimized && client.gpu_gva != 0 {
                    quads.push(WindowQuad {
                        tex_gva: client.gpu_gva + hdr,
                        src_w: client.buf_w as u32,
                        src_h: client.buf_h as u32,
                        src_pitch: (client.buf_w as u32) * 4,
                        dst_x: client.win.x as i32,
                        dst_y: (client.win.y + 30) as i32,
                        // U4 Boot 3 live resize: draw the content at the SHELL size (win.w/win.h), not
                        // the buffer size (buf_w/buf_h). src stays the real (pitch-aligned) buffer, so
                        // the GPU stretches the current content to follow the frame instantly during a
                        // resize drag; it sharpens when the app reallocates its SHM (MSG_WINDOW_UPDATE_
                        // SHM updates buf_w/buf_h). No scene rebuild or pitch fallback during the drag.
                        dst_w: client.win.w as u32,
                        dst_h: client.win.h as u32,
                        // U4 Boot 2: honor the window's opacity on the GPU path (fade-in on open,
                        // dimmed/translucent windows). The CPU fallback already applies it via
                        // composite_buffer(.., win.opacity). Same clients-iter Z-order => back-to-front.
                        opacity: client.win.opacity as u32,
                    });
                }
            }
            let gpu_composited = sys_gpu_composite(&quads);
            if gpu_composited { sys_gpu_sync(); }

            // 3. Perform CPU drawing (Text, Window Borders, Windows, Taskbar, Cursor)
            let mut canvas = Canvas::new(hardware_fb, screen_stride, screen_h);

            // Draw window decorations and CPU client compositing in Z-order. For each window we build
            // the rects of the windows stacked ABOVE it (j>i in the back-to-front clients vec) and occlude
            // its chrome + corner carve by them — so a lower window's title bar / border / rounded-corner
            // carve never paints over an upper window (fixes the "below-window outline" z-order artifact:
            // chrome is one CPU pass after the GPU composited all content, so without this the passes fight).
            // U7 boot 3: the focused (topmost non-minimized) window gets active chrome styling; the
            // hovered title-bar button gets a highlight plate. Computed once for this frame's chrome pass.
            let active_idx = state.active_window();
            for i in 0..state.clients.len() {
                if !state.clients[i].win.exists { continue; }
                // U7: minimized windows are fully hidden (they live only as taskbar buttons now).
                if state.clients[i].win.is_minimized { continue; }
                let mut above: Vec<(i32, i32, i32, i32)> = Vec::new();
                for j in (i + 1)..state.clients.len() {
                    let u = &state.clients[j].win;
                    if u.exists && !u.is_minimized {
                        let th = (u.h + 30) as i32;
                        above.push((u.x as i32, u.y as i32, u.x as i32 + u.w as i32, u.y as i32 + th));
                    }
                }
                let is_active = active_idx == Some(i);
                let hover_btn = match state.hover_win_btn {
                    Some((hi, hb)) if hi == i => hb as i32,
                    _ => -1,
                };
                let client = &state.clients[i];
                if gpu_composited {
                    // GPU already drew the content; CPU draws only the title bar + frame on top.
                    draw_window_chrome(canvas.buffer, screen_stride, screen_h, &client.win, &above, is_active, hover_btn);
                } else {
                    // Fallback: full CPU path — white fill + copy the window's pixels.
                    draw_window_rounded(canvas.buffer, screen_stride, screen_h, &client.win);
                    if !client.win.is_minimized {
                        if client.buffer.is_null() || client.buffer as u64 == 0 { continue; }
                        let expected_size = client.buf_w * client.buf_h;
                        let client_pixels = unsafe { core::slice::from_raw_parts(client.buffer, expected_size) };
                        canvas.composite_buffer(client.win.x, client.win.y + 30, client_pixels, client.buf_w, client.buf_h, client.win.opacity);
                    }
                }
                // Boot B: if the app reports scrollable content, draw a scrollbar on the content
                // area's right edge (below the title bar, inside the border). Drawn BEFORE corner
                // rounding so the carve trims its bottom-right into the rounded corner.
                if !client.win.is_minimized {
                    let (content_h, scroll_off) = client_scroll(client);
                    if content_h > client.win.h {
                        let bar_x = client.win.x + client.win.w - SCROLLBAR_W;
                        let bar_y = client.win.y + 30;
                        draw_scrollbar(&mut canvas, bar_x, bar_y, SCROLLBAR_W, client.win.h,
                                       content_h, client.win.h, scroll_off);
                    }
                }
                // U4 Boot 3: round the TRUE window outline last, once, on whichever content is now
                // on the backbuffer (GPU or CPU) — carves the real 4 corners to the wallpaper.
                // Constant 12px radius, size-independent. Must run AFTER composite_buffer in the
                // fallback path (which would otherwise overwrite carved bottom corners).
                round_window_corners(canvas.buffer, screen_stride, screen_h, &client.win, 12, &above);
            }

            // 4. Drop shadows — FINAL pass, drawn AFTER all window content+chrome so each window's
            // shadow correctly falls ON TOP of the windows BELOW it (fixes the pre-pass, where a lower
            // window's later-composited content covered an upper window's shadow). For window i, the
            // rects of every window ABOVE it (j>i in the back-to-front clients vec) are skipped, so a
            // higher window is never darkened by a lower one's shadow. Soft/toned-down (alpha ~42).
            for i in 0..state.clients.len() {
                let c = &state.clients[i];
                if !c.win.exists || c.win.is_minimized || c.win.opacity <= 40 { continue; }
                let mut above: Vec<(i32, i32, i32, i32)> = Vec::new();
                for j in (i + 1)..state.clients.len() {
                    let u = &state.clients[j];
                    if u.win.exists && !u.win.is_minimized {
                        let th = (u.win.h + 30) as i32;
                        above.push((u.win.x as i32, u.win.y as i32,
                                    u.win.x as i32 + u.win.w as i32, u.win.y as i32 + th));
                    }
                }
                draw_window_shadow(canvas.buffer, screen_stride, screen_h, &c.win, &above);
            }

            // 5. Draw Taskbar on top of windows (CPU-based fills and text)
            render_taskbar(&mut canvas, screen_stride, screen_h);

            // U7: taskbar window buttons — one per window, left of the Start button. Active window is
            // accented; minimized windows are dimmed. Drawn CPU (fill + title) on the taskbar strip;
            // the layout matches taskbar_buttons() so clicks map correctly.
            {
                let bar_y = screen_h - BAR_H;
                let btn_y = bar_y + (BAR_H - TB_BTN_H) / 2;
                let active = state.active_window();
                for (ci, bx) in state.taskbar_buttons() {
                    let client = &state.clients[ci];
                    let win = &client.win;
                    let is_active = Some(ci) == active;
                    let is_hovered = state.hover_tb_btn == Some(ci);
                    // Active button accented; a hovered (non-active) one gets a soft accent tint;
                    // minimized dimmed; otherwise it sits flush with the bar.
                    let bg = if is_active { 0xFF_FDEBD8 }
                             else if is_hovered { 0xFF_F2F2EF }
                             else { 0xFF_FFFFFF };
                    fill_round_rect(&mut canvas, bx, btn_y, TB_BTN_W, TB_BTN_H, 6, bg);

                    match &client.icon {
                        Some(icon) => {
                            // Minimized windows are dimmed by compositing the icon over the button at
                            // reduced opacity — it still reads as "this app", just parked.
                            let op = if win.is_minimized { 130u8 } else { 255u8 };
                            if op == 255 {
                                canvas.blit_rgba(bx + (TB_BTN_W - TB_ICON) / 2,
                                                 btn_y + (TB_BTN_H - TB_ICON) / 2,
                                                 icon, TB_ICON, TB_ICON, None);
                            } else {
                                let faded: Vec<u32> = icon.iter()
                                    .map(|p| ((((p >> 24) & 0xFF) * op as u32 / 255) << 24) | (p & 0x00FF_FFFF))
                                    .collect();
                                canvas.blit_rgba(bx + (TB_BTN_W - TB_ICON) / 2,
                                                 btn_y + (TB_BTN_H - TB_ICON) / 2,
                                                 &faded, TB_ICON, TB_ICON, None);
                            }
                        }
                        None => {
                            // No declared icon: fall back to the app's initial, so the button is
                            // still identifiable rather than an empty square.
                            let title = core::str::from_utf8(&win.title[..win.title_len]).unwrap_or("A");
                            let ch = title.chars().next().unwrap_or('A');
                            let mut buf = [0u8; 4];
                            let s = ch.encode_utf8(&mut buf);
                            let tw = Canvas::text_width(s, 1);
                            let tcol = if win.is_minimized { Color::TEXT_MUTED } else { Color::TEXT_DARK };
                            canvas.print_str(bx + (TB_BTN_W.saturating_sub(tw)) / 2, btn_y + 6, s, tcol, 1);
                        }
                    }

                    // Running/active underline — the only cue left once the label is gone.
                    if is_active {
                        canvas.fill_rect(bx + 8, btn_y + TB_BTN_H - 2, TB_BTN_W - 16, 2, Color::ACCENT_PRIMARY);
                    } else if !win.is_minimized {
                        canvas.fill_rect(bx + 13, btn_y + TB_BTN_H - 2, TB_BTN_W - 26, 2, 0xFF_C8C8C2);
                    }
                }
            }

            // Start button: a rounded accent plate carrying the Nyx mark. Pressed-looking while the
            // menu is open, so the button reflects the menu's state rather than being inert.
            {
                let (bx, by, bw, bh) = state.start_button_rect();
                let bg = if state.start_menu_open { Color::ACCENT_HOVER } else { Color::ACCENT_PRIMARY };
                fill_round_rect(&mut canvas, bx, by, bw, bh, 6, bg);
                if let Some(logo) = &state.logo {
                    canvas.blit_rgba(bx + 8, by + (bh - LOGO_BTN) / 2, logo, LOGO_BTN, LOGO_BTN, None);
                }
                // The "NYX" wordmark itself is drawn in the batched chrome-text pass below.
            }

            // Tray: live Wi-Fi state. Code-drawn rather than an icon asset because it has to say
            // WHICH state the link is in, and a PNG would need four of them.
            {
                let (nx, ny, _, _) = state.tray_wifi_rect();
                draw_wifi_glyph(&mut canvas, nx, ny, state.wifi_state);
            }

            // Start menu: dark panel, accent header carrying the Nyx mark, then one row per MENU
            // entry with its icon. Entry TEXT is drawn in the batched chrome-text pass below.
            if state.start_menu_open {
                let (menu_x, menu_y, menu_w, menu_h) = state.start_menu_rect();

                fill_round_rect(&mut canvas, menu_x, menu_y, menu_w, menu_h, 10, 0xFF_111111);
                canvas.fill_rect(menu_x, menu_y, menu_w, MENU_HEADER_H, 0xFF_1B1B1B);
                canvas.fill_rect(menu_x, menu_y, menu_w, 2, Color::NYX_ORANGE);
                if let Some(logo) = &state.logo_big {
                    // The header mark stays small; MENU_ICON is the (larger) tile size now.
                    canvas.blit_rgba(menu_x + MENU_PAD, menu_y + (MENU_HEADER_H - 26) / 2,
                                     logo, 26, 26, None);
                }

                // Search field. Text (query or placeholder) is drawn in the batched pass below.
                let (sx, sy, sw, sh) = state.menu_search_rect();
                fill_round_rect(&mut canvas, sx, sy, sw, sh, 6, 0xFF_1E1E1E);
                // A small magnifier, code-drawn: a ring plus a diagonal tail.
                let (cx, cy) = (sx + 15, sy + sh / 2 - 1);
                for d in 0..12 {
                    let a = [(0i32, -5i32), (3, -4), (5, -1), (4, 3), (0, 5), (-3, 4), (-5, 1), (-4, -3),
                             (2, -5), (5, 2), (-2, 5), (-5, -2)][d];
                    canvas.fill_rect((cx as i32 + a.0) as usize, (cy as i32 + a.1) as usize, 1, 1, 0xFF_888888);
                }
                canvas.fill_rect(cx + 4, cy + 4, 4, 1, 0xFF_888888);
                canvas.fill_rect(cx + 5, cy + 5, 4, 1, 0xFF_888888);

                // Tiles: icon over a centred label, hover plate behind. Labels go in the text pass.
                let hovered = state.menu_slot_at(state.mx, state.my);
                for (slot, &i) in state.menu_filtered().iter().enumerate() {
                    let (tx, ty, tw, th) = state.menu_tile_rect(slot);
                    if hovered == Some(slot) {
                        fill_round_rect(&mut canvas, tx + 3, ty + 3, tw - 6, th - 6, 8, 0xFF_2A2A2A);
                    }
                    if let Some(Some(icon)) = state.icons.get(i) {
                        canvas.blit_rgba(tx + (tw - MENU_ICON) / 2, ty + 14, icon, MENU_ICON, MENU_ICON, None);
                    }
                }
            }

            // U0 baseline overlay: FPS + last frame's composite time (top-left, unobtrusive).
            // Shows the previous frame's numbers (this frame's are measured after present below).
            let mut ov = [0u8; 24];
            let mut op = 0usize;
            for &b in b"FPS " { ov[op] = b; op += 1; }
            op = push_num(&mut ov, op, state.fps);
            for &b in b"  " { ov[op] = b; op += 1; }
            op = push_num(&mut ov, op, state.last_frame_ms);
            ov[op] = b'm'; op += 1; ov[op] = b's'; op += 1;
            let ov_str = core::str::from_utf8(&ov[..op]).unwrap_or("FPS ?");

            // Taskbar clock: HH:MM:SS over "DD Mon YYYY", both from the hardware RTC.
            let rtc = sys_get_rtc();
            let mut disp_hour = rtc.hour as i32 + TZ_OFFSET_HOURS;
            disp_hour = ((disp_hour % 24) + 24) % 24;
            let mut clk = [0u8; 8];
            let mut cp = push_2d(&mut clk, 0, disp_hour as u8);
            clk[cp] = b':'; cp += 1;
            cp = push_2d(&mut clk, cp, rtc.min);
            clk[cp] = b':'; cp += 1;
            cp = push_2d(&mut clk, cp, rtc.sec);
            let clk_str = core::str::from_utf8(&clk[..cp]).unwrap_or("--:--:--");

            const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                        "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            let mut dbuf = [0u8; 16];
            let mut dp = push_2d(&mut dbuf, 0, rtc.day);
            dbuf[dp] = b' '; dp += 1;
            let mon = MONTHS[((rtc.month as usize).saturating_sub(1)).min(11)];
            for &b in mon.as_bytes() { dbuf[dp] = b; dp += 1; }
            dbuf[dp] = b' '; dp += 1;
            dp = push_num(&mut dbuf, dp, rtc.year as usize);
            let date_str = core::str::from_utf8(&dbuf[..dp]).unwrap_or("");

            let (sb_x, sb_y, _, sb_h) = state.start_button_rect();
            // The wordmark sits right of the logo inside the Start button.
            let nyx_x = sb_x + 8 + LOGO_BTN + 8;
            let nyx_y = sb_y + (sb_h / 2) - 8;
            let (menu_x, menu_y, _, _) = state.start_menu_rect();
            let header_x = menu_x + MENU_PAD + 26 + 12;   // right of the 26px header mark

            // Start-menu text, laid out once and drawn by whichever path is available below. Built
            // here so the GPU batch and the CPU fallback can't disagree about where anything sits.
            // Tuple: (x, y, text, colour).
            let mut menu_text: Vec<(usize, usize, &str, u32)> = Vec::new();
            if state.start_menu_open {
                menu_text.push((header_x, menu_y + 17, "NyxOS", Color::WHITE));

                let (sx, sy, sw, sh) = state.menu_search_rect();
                let ty = sy + sh / 2 - 8;
                if state.search_len == 0 {
                    menu_text.push((sx + 30, ty, "Search apps", Color::TEXT_MUTED));
                } else {
                    // The query is ASCII by construction (only 0x20..0x7F is accepted), so this is
                    // always valid UTF-8; the unwrap_or keeps a bad byte from panicking the shell.
                    let q = core::str::from_utf8(&state.search[..state.search_len]).unwrap_or("");
                    menu_text.push((sx + 30, ty, fit_label(q, sw.saturating_sub(38)), Color::WHITE));
                }

                let hits = state.menu_filtered();
                if hits.is_empty() {
                    let msg = "No matching apps";
                    let (gx, gy, _, _) = state.menu_tile_rect(0);
                    let w = Canvas::text_width(msg, 1);
                    menu_text.push((gx + (MENU_COLS * MENU_TILE_W).saturating_sub(w) / 2,
                                    gy + 24, msg, Color::TEXT_MUTED));
                }
                for (slot, &i) in hits.iter().enumerate() {
                    let (tx, ty, tw, th) = state.menu_tile_rect(slot);
                    let label = fit_label(MENU[i].label, tw - 8);
                    let lw = Canvas::text_width(label, 1);
                    menu_text.push((tx + tw.saturating_sub(lw) / 2, ty + th - 26, label, Color::WHITE));
                }
            }

            // Clock: both lines RIGHT-aligned inside the block, so the date's varying width (one- vs
            // two-digit day) doesn't make the time jitter horizontally each day.
            let (ck_x, ck_y, ck_w, _) = state.clock_rect();
            let time_x = ck_x + ck_w.saturating_sub(Canvas::text_width(clk_str, 1));
            let date_x = ck_x + ck_w.saturating_sub(Canvas::text_width(date_str, 1));
            let date_y = ck_y + 15;

            // U5 T2: CHROME TEXT on the GPU. Batch every fixed-opacity chrome label (taskbar clock,
            // the NYX wordmark, FPS overlay, and the start-menu entries when open) into ONE textured
            // glyph run drawn over the CPU-drawn chrome (same 0x1400_0000 backbuffer), tinted per-quad
            // to each label's colour via the text PS. On any failure we CPU-draw the identical text as
            // a fallback, so the chrome is never blank. Window TITLES stay CPU-drawn — they fade with
            // per-window opacity, which the luminance-only text path can't express.
            let mut chrome_drawn = false;
            if let Some(font) = &gpu_font {
                let mut glyphs: Vec<GlyphQuad> = Vec::new();
                glyphs.extend(font.layout_str(4, 2, ov_str, 1, Color::TEXT_MUTED));
                glyphs.extend(font.layout_str(time_x as i32, ck_y as i32, clk_str, 1, Color::TEXT_DARK));
                glyphs.extend(font.layout_str(date_x as i32, date_y as i32, date_str, 1, Color::TEXT_MUTED));
                glyphs.extend(font.layout_str(nyx_x as i32, nyx_y as i32, "NYX", 1, Color::WHITE));
                for &(tx, ty, text, col) in menu_text.iter() {
                    glyphs.extend(font.layout_str(tx as i32, ty as i32, text, 1, col));
                }
                if sys_gpu_draw_text(font.atlas_gva, font.atlas_w, font.atlas_h, font.atlas_pitch, &glyphs) {
                    sys_gpu_sync();
                    chrome_drawn = true;
                }
            }
            if !chrome_drawn {
                // CPU fallback — identical layout/colours (never leave chrome blank).
                canvas.print_str(4, 2, ov_str, Color::TEXT_MUTED, 1);
                canvas.print_str(time_x, ck_y, clk_str, Color::TEXT_DARK, 1);
                canvas.print_str(date_x, date_y, date_str, Color::TEXT_MUTED, 1);
                canvas.print_str(nyx_x, nyx_y, "NYX", Color::WHITE, 1);
                for &(tx, ty, text, col) in menu_text.iter() {
                    canvas.print_str(tx, ty, text, col, 1);
                }
            }

            // Software cursor only when the GPU hardware cursor plane isn't driving the pointer.
            if !state.hw_cursor {
                draw_cursor(canvas.buffer, screen_stride, screen_h, state.mx, state.my, state.cursor_type);
            }

            // D1 present into our single owned scanout buffer (P1a): full swap on a full frame, region
            // BLT on a partial frame. Single buffer, so a partial present is valid — untouched scanout
            // pixels retain the previous frame. (True double-buffered flip is deferred; it stalled on a
            // per-frame fence-wait.)
            if full_frame {
                sys_swap_buffers();
            } else {
                sys_swap_buffers_rect(dmg_x, dmg_y, dmg_w, dmg_h);
            }
            sys_gpu_sync();

            // U0 instrumentation: measure this frame's composite cost + rolling 1s FPS.
            let frame_end = sys_get_time();
            state.last_frame_ms = frame_end.wrapping_sub(frame_start);
            state.frame_count = state.frame_count.wrapping_add(1);
            state.fps_window_frames += 1;
            if frame_end.wrapping_sub(state.fps_window_start) >= 1000 {
                state.fps = state.fps_window_frames;
                state.fps_window_frames = 0;
                state.fps_window_start = frame_end;
                // Emit ONE serial baseline (first full FPS window) so the boot log records that the
                // compositor came up and is drawing, then go quiet — the once-per-second clock tick
                // must not flood the ring buffer and bury the startup lines.
                if state.fps_logs < 1 {
                    state.fps_logs += 1;
                    let mut log = [0u8; 96];
                    let mut lp = 0usize;
                    for &b in b"[UI] frame " { log[lp] = b; lp += 1; }
                    lp = push_num(&mut log, lp, state.frame_count as usize);
                    for &b in b": draw=" { log[lp] = b; lp += 1; }
                    lp = push_num(&mut log, lp, state.last_frame_ms);
                    for &b in b"ms fps=" { log[lp] = b; lp += 1; }
                    lp = push_num(&mut log, lp, state.fps);
                    for &b in b" vsync_to=" { log[lp] = b; lp += 1; }
                    lp = push_num(&mut log, lp, state.vsync_timeouts as usize);
                    log[lp] = b'\n'; lp += 1;
                    if let Ok(s) = core::str::from_utf8(&log[..lp]) { sys_print(s); }
                }
            }

            state.prev_mx = state.mx;
            state.prev_my = state.my;
            state.dirty_min_x = screen_stride; state.dirty_min_y = screen_h;
            state.dirty_max_x = 0; state.dirty_max_y = 0;
            state.needs_redraw = false;
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(99); }