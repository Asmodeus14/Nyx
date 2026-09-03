#![no_std]
#![allow(warnings)] // avoid the annotate_snippets renderer ICE on snippet-bearing warnings (see libs/gui)

use core::arch::asm;

#[repr(C)]
pub struct WindowHeader {
    pub magic: u32,
    pub requested_x: i32,
    pub requested_y: i32,
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    // Scrollbar (Boot B): the app reports its FULL scrollable content height and current top offset
    // here each frame; the compositor reads them to draw + drive a window-aligned scrollbar. `content_h
    // <= height` (or 0) means "not scrollable" → no bar. No new IPC round-trip (already-mapped header).
    pub content_h: u32,
    pub scroll_off: u32,
    pub title: [u8; 64],
    /// Absolute path to this app's icon PNG, NUL-padded. Apps live in `.nyx` bundles and ship their
    /// own artwork, so this is normally `/mnt/nvme/apps/<Name>.nyx/icon.png`. The compositor decodes
    /// it once and uses it for the window's taskbar button. Empty = no icon; the button falls back
    /// to the window's initial letter, so an app that never sets this still gets a usable button.
    pub icon: [u8; 96],
    // U4: pad the header so the pixel data that follows (`shm + size_of::<WindowHeader>()`) is
    // PAGE-ALIGNED. The GPU compositor binds that pixel region as a sampled texture, and every proven
    // texture surface in the render engine uses a 4KB-aligned base — this makes window surfaces match,
    // with zero alignment risk. All apps compute the pixel offset via size_of, so this is transparent.
    // Fixed part is now 192 bytes (8×u32 + 64 title + 96 icon), so pad to keep the total exactly 4096.
    pub _pad: [u8; 4096 - 192],
}

// U4 invariant: the header MUST stay exactly one page so the pixel data after it is 4KB-aligned for
// the GPU texture binding. This const-assert breaks the build if a field addition changes the size.
const _: () = assert!(core::mem::size_of::<WindowHeader>() == 4096);

pub const WIN_MAGIC: u32 = 0x4E595857;
pub const WIN_FLAG_NONE: u32 = 0;
pub const WIN_FLAG_FRAMELESS: u32 = 1;
pub const WIN_FLAG_TRANSPARENT: u32 = 2;

// ─────────────────────────────────────────────────────────────────────────
// NYX-OS IPC CORE PROTOCOL CONSTANTS
// ─────────────────────────────────────────────────────────────────────────
pub const MSG_REQ_WINDOW: u64 = 1;
pub const MSG_WINDOW_CREATED: u64 = 2;
pub const MSG_FLUSH_WINDOW: u64 = 3;
pub const MSG_KEY_EVENT: u64 = 4;
pub const MSG_MOUSE_EVENT: u64 = 5;
pub const MSG_WINDOW_CLOSE: u64 = 6;
pub const MSG_WINDOW_RESIZED: u64 = 7;
pub const MSG_WINDOW_UPDATE_SHM: u64 = 8;
/// Compositor → app: the user moved this window's scrollbar. `data1` = the new top scroll offset (px).
/// The app clamps it, updates its scroll state, re-renders, and flushes.
pub const MSG_SCROLL: u64 = 9;
/// R1: Compositor → app. The compositor has finished switching to the app's RESIZED buffer and has
/// released (unmapped) its own mapping of the OLD buffer, whose SHM id is in `data1`. On receipt the
/// old buffer is single-owner (only the app still maps it), so the app may safely `sys_destroy_shm` it.
/// This ACK is what makes the resize re-allocation loop reclaim memory instead of leaking every drag.
pub const MSG_SHM_RELEASED: u64 = 10;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct IpcMessage {
    pub sender_pid: u64,
    pub msg_type: u64,
    pub data1: u64,
    pub data2: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskInfo {
    pub pid: u64,
    pub cpu_ticks: u64,
    pub state: u8, 
    pub name: [u8; 16],
}

#[repr(C)]
pub struct SystemInfo {
    pub current_temp: u8,
    pub active_cooling: u8,
    pub cpu_fan_rpm: u32,
    pub gpu_fan_rpm: u32,
    pub task_count: u64,
    pub tasks: [TaskInfo; 64],
    /// SMBIOS manufacturer + product ("Dell Inc. Latitude 5410"), NUL-padded, empty when the
    /// firmware reported no identity. See `machine_name()`.
    pub machine: [u8; 80],
}

impl SystemInfo {
    /// The SMBIOS machine name, or "" when unknown.
    pub fn machine_name(&self) -> &str {
        let end = self.machine.iter().position(|&b| b == 0).unwrap_or(self.machine.len());
        core::str::from_utf8(&self.machine[..end]).unwrap_or("")
    }
}

// ─────────────────────────────────────────────────────────────────────────
// KEYBOARD: navigation/editing keys delivered as Private-Use-Area chars
// ─────────────────────────────────────────────────────────────────────────
// The kernel (nyx-kernel/src/shell.rs) has no Unicode for arrows/Home/End/Delete/PageUp/Down, so it
// encodes them as these PUA chars and pushes them through the normal key path → NyxApp::on_key(char).
// These values MUST stay in sync with the match in shell.rs::handle_key.
pub mod keys {
    pub const LEFT: char = '\u{E010}';
    pub const RIGHT: char = '\u{E011}';
    pub const UP: char = '\u{E012}';
    pub const DOWN: char = '\u{E013}';
    pub const HOME: char = '\u{E014}';
    pub const END: char = '\u{E015}';
    pub const DELETE: char = '\u{E016}';
    pub const PAGE_UP: char = '\u{E017}';
    pub const PAGE_DOWN: char = '\u{E018}';
    /// Either Windows/Super key. The compositor consumes this to toggle the start menu, so apps
    /// never see it — it's listed here so nothing else claims the codepoint.
    pub const SUPER: char = '\u{E019}';
}

// ─────────────────────────────────────────────────────────────────────────
// LINUX X86_64 SYSCALL ID CONSTANTS
// ─────────────────────────────────────────────────────────────────────────
pub const SYS_READ: u64 = 0;
pub const SYS_WRITE: u64 = 1;
pub const SYS_OPEN: u64 = 2;
pub const SYS_CLOSE: u64 = 3;
pub const SYS_STAT: u64 = 4;
pub const SYS_LSEEK: u64 = 8;
pub const SYS_MMAP: u64 = 9;
pub const SYS_MPROTECT: u64 = 10;
pub const SYS_MUNMAP: u64 = 11;
pub const SYS_IOCTL: u64 = 16;
pub const SYS_SOCKET: u64 = 41;
pub const SYS_CONNECT: u64 = 42;
pub const SYS_SENDTO: u64 = 44;
pub const SYS_RECVFROM: u64 = 45;
pub const SYS_CLONE: u64 = 56;
pub const SYS_FORK: u64 = 57;
pub const SYS_EXECVE: u64 = 59;
pub const SYS_EXIT: u64 = 60;
pub const SYS_FUTEX: u64 = 202;
pub const SYS_CLOCK_GETTIME: u64 = 228;
pub const SYS_PIPE: u64 = 22;
pub const SYS_DUP2: u64 = 33;

#[inline(always)]
pub fn syscall(n: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64, arg6: u64) -> u64 {
    let mut ret: u64;
    unsafe {
        asm!(
            "syscall",
            in("rax") n,
            in("rdi") arg1,
            in("rsi") arg2,
            in("rdx") arg3,
            in("r10") arg4,
            in("r8") arg5,
            in("r9") arg6,
            lateout("rax") ret,
            out("rcx") _,
            out("r11") _,
            options(nostack)
        );
    }
    ret
}

pub fn sys_read(fd: i64, buf: &mut [u8]) -> i64 {
    syscall(SYS_READ, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0) as i64
}

pub fn sys_write(fd: i64, buf: &[u8]) -> i64 {
    syscall(SYS_WRITE, fd as u64, buf.as_ptr() as u64, buf.len() as u64, 0, 0, 0) as i64
}

pub fn sys_open(path: &str) -> i64 {
    syscall(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0) as i64
}

pub const O_CREAT: u64 = 0x40;
pub const O_TRUNC: u64 = 0x200;

/// `open` with Linux-style flags — `O_CREAT` to create, `O_TRUNC` to overwrite. Plain `sys_open`
/// passes 0, so it can only open a file that already exists.
pub fn sys_open_flags(path: &str, flags: u64) -> i64 {
    syscall(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags, 0, 0, 0) as i64
}

pub fn sys_close(fd: i64) -> i64 {
    syscall(SYS_CLOSE, fd as u64, 0, 0, 0, 0, 0) as i64
}

pub fn sys_exit(code: i64) -> ! {
    syscall(SYS_EXIT, code as u64, 0, 0, 0, 0, 0);
    loop {}
}

pub fn sys_pipe(fds: &mut [i32; 2]) -> i64 {
    syscall(SYS_PIPE, fds.as_mut_ptr() as u64, 0, 0, 0, 0, 0) as i64
}

pub fn sys_dup2(oldfd: i64, newfd: i64) -> i64 {
    syscall(SYS_DUP2, oldfd as u64, newfd as u64, 0, 0, 0, 0) as i64
}

pub fn sys_execve(path: &str) -> i64 {
    syscall(SYS_EXECVE, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0) as i64
}

pub fn sys_fork() -> i64 {
    syscall(SYS_FORK, 0, 0, 0, 0, 0, 0) as i64
}

pub fn sys_print(text: &str) {
    sys_write(1, text.as_bytes());
}

pub fn sys_gpu_sync() {
    syscall(503, 0, 0, 0, 0, 0, 0);
}

pub fn sys_swap_buffers() {
    syscall(502, 0, 0, 0, 0, 0, 0);
}

/// D1 region present: BLT only the damage sub-rect (x,y,w,h in screen px) from the backbuffer to
/// scanout, instead of the whole screen. Lands at the exact same scanout pixels `sys_swap_buffers`
/// would write for that rect (same source/dest/pitch), so it's a drop-in partial present. The kernel
/// clamps the rect to the screen. Use when only a bounded region changed; fall back to
/// `sys_swap_buffers` for a full-screen frame.
pub fn sys_swap_buffers_rect(x: usize, y: usize, w: usize, h: usize) {
    syscall(538, x as u64, y as u64, w as u64, h as u64, 0, 0);
}

pub fn sys_gpu_fill_rect(x: usize, y: usize, w: usize, h: usize, color: u32) {
    syscall(501, x as u64, y as u64, w as u64, h as u64, color as u64, 0);
}

// ---------------------------------------------------------------------------
// Phase 9: mini-GL 3D API. Wraps the kernel's RCS render engine (textured,
// multi-mesh, SSAA-antialiased). Typical use:
//
//     sys_gl_init(w, h);
//     let cube = sys_gl_upload_mesh(&verts, &indices, &texels, tw, th);
//     loop { sys_gl_render(&mvps); }   // one Mat4 (16 f32, column-major) per mesh
//
// Vertex layout is GlVertex (pos xyz + varying: uv,0,1 for textured, or rgba).
// ---------------------------------------------------------------------------

pub const SYS_GL_INIT: u64 = 514;
pub const SYS_GL_UPLOAD_MESH: u64 = 515;
pub const SYS_GL_RENDER: u64 = 516;
pub const SYS_GL_RESET: u64 = 527;

/// One interleaved 3D vertex, matching the kernel `GlVertex` (repr(C), 7 f32 = 28 bytes):
/// position (x,y,z) in model space + a 4-component varying. For a textured mesh set the varying to
/// (u, v, 0.0, 1.0); the pixel shader samples the mesh's texture at (u,v).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GlVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub vx: f32,
    pub vy: f32,
    pub vz: f32,
    pub vw: f32,
}

impl GlVertex {
    /// Convenience: a textured vertex at (x,y,z) sampling UV (u,v).
    pub const fn textured(x: f32, y: f32, z: f32, u: f32, v: f32) -> Self {
        Self { x, y, z, vx: u, vy: v, vz: 0.0, vw: 1.0 }
    }
}

/// Descriptor passed to sys_gl_upload_mesh (repr(C), 7 u64) — pointers + counts for the mesh's
/// vertices, indices, and B8G8R8A8 texels. Built internally by the wrapper.
#[repr(C)]
struct GlMeshDesc {
    verts_ptr: u64,
    vert_count: u64,
    idx_ptr: u64,
    idx_count: u64,
    texel_ptr: u64,
    tex_w: u64,
    tex_h: u64,
}

/// Initialize the GL context for a `width`x`height` WINDOW. `dst_pixels` is this app's SHM window
/// pixel buffer (B8G8R8A8, tightly packed at `width*4` pitch); the kernel copies each resolved frame
/// into it so the compositor composites the cube as a normal window (no scanout blit). Returns true on
/// success. Call once before uploading meshes.
pub fn sys_gl_init(width: u32, height: u32, dst_pixels: *const u32) -> bool {
    syscall(SYS_GL_INIT, width as u64, height as u64, dst_pixels as u64, 0, 0, 0) == 1
}

/// Upload a mesh + its texture. `texels` is `tex_w * tex_h` B8G8R8A8 pixels (A<<24|R<<16|G<<8|B).
/// Returns the mesh handle (0-based index) on success, or None on error. Call before the first
/// render; the mesh set is fixed once rendering begins (use sys_gl_reset to rebuild).
pub fn sys_gl_upload_mesh(
    verts: &[GlVertex],
    indices: &[u32],
    texels: &[u32],
    tex_w: u32,
    tex_h: u32,
) -> Option<usize> {
    let desc = GlMeshDesc {
        verts_ptr: verts.as_ptr() as u64,
        vert_count: verts.len() as u64,
        idx_ptr: indices.as_ptr() as u64,
        idx_count: indices.len() as u64,
        texel_ptr: texels.as_ptr() as u64,
        tex_w: tex_w as u64,
        tex_h: tex_h as u64,
    };
    let r = syscall(SYS_GL_UPLOAD_MESH, &desc as *const GlMeshDesc as u64, 0, 0, 0, 0, 0);
    if r == u64::MAX { None } else { Some(r as usize) }
}

/// Render one frame. `mvps` is one 4x4 MVP matrix per uploaded mesh (in upload order), each 16 f32
/// column-major. The kernel renders all meshes (SSAA-antialiased) and presents. Returns true on ok.
pub fn sys_gl_render(mvps: &[[f32; 16]]) -> bool {
    syscall(SYS_GL_RENDER, mvps.as_ptr() as u64, mvps.len() as u64, 0, 0, 0, 0) == 1
}

/// Tear down the GL context so it can be re-initialized and re-uploaded.
pub fn sys_gl_reset() {
    syscall(SYS_GL_RESET, 0, 0, 0, 0, 0, 0);
}

pub fn sys_get_time() -> usize {
    syscall(504, 0, 0, 0, 0, 0, 0) as usize
}

pub const SYS_GET_RTC: u64 = 528;

/// A wall-clock reading from the machine's battery-backed CMOS RTC (syscall 528). Fields are binary
/// and 24-hour normalized by the kernel. This is real calendar time, unlike `sys_get_time` (uptime).
#[derive(Clone, Copy)]
pub struct RtcTime {
    pub sec: u8,
    pub min: u8,
    pub hour: u8,
    pub day: u8,
    pub month: u8,
    pub year: u16,
}

/// Read the hardware wall clock. Unpacks the kernel's packed u64
/// (year<<40 | month<<32 | day<<24 | hour<<16 | min<<8 | sec).
pub fn sys_get_rtc() -> RtcTime {
    let p = syscall(SYS_GET_RTC, 0, 0, 0, 0, 0, 0);
    RtcTime {
        sec: (p & 0xFF) as u8,
        min: ((p >> 8) & 0xFF) as u8,
        hour: ((p >> 16) & 0xFF) as u8,
        day: ((p >> 24) & 0xFF) as u8,
        month: ((p >> 32) & 0xFF) as u8,
        year: ((p >> 40) & 0xFFFF) as u16,
    }
}

/// Minutes east of UTC, for DISPLAY only (syscall 553). The RTC itself is always UTC.
pub const SYS_GET_TZ: u64 = 553;

/// The configured display timezone, in minutes east of UTC. 0 if never set.
pub fn sys_get_tz_minutes() -> i64 {
    syscall(SYS_GET_TZ, 0, 0, 0, 0, 0, 0) as i64
}

/// The wall clock already shifted into the configured display timezone.
///
/// ★ Use this for anything a person reads; use `sys_get_rtc` only when you specifically want UTC.
///
/// The offset goes through a real date conversion rather than being added to `hour`, because the
/// naive version is wrong twice over: it cannot express +5:30 (India), +5:45 (Nepal) or +12:45
/// (Chatham), and when the shift crosses midnight it leaves the DATE on the wrong day — so for five
/// and a half hours every night a panel clock would show yesterday.
pub fn sys_get_rtc_local() -> RtcTime {
    let utc = sys_get_rtc();
    let off = sys_get_tz_minutes();
    if off == 0 || utc.year == 0 {
        return utc;
    }
    let secs = days_from_civil(utc.year as i64, utc.month as i64, utc.day as i64) * 86_400
        + utc.hour as i64 * 3600
        + utc.min as i64 * 60
        + utc.sec as i64
        + off * 60;
    civil_from_unix(secs)
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant). Matches the kernel's copy in
/// `rtc_packed_to_unix`; they must agree or a converted time drifts by the difference.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse: Unix seconds -> calendar fields.
fn civil_from_unix(secs: i64) -> RtcTime {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    RtcTime {
        sec: (tod % 60) as u8,
        min: ((tod % 3600) / 60) as u8,
        hour: (tod / 3600) as u8,
        day: d as u8,
        month: m as u8,
        year: y as u16,
    }
}

pub const SYS_CURSOR_INIT: u64 = 529;
pub const SYS_CURSOR_SET_IMAGE: u64 = 535;

/// Bring up the display controller's hardware cursor plane. Returns true if the GPU cursor is now
/// live (the caller should then stop drawing a software cursor); false if it fell back (keep the
/// software cursor). Once up, the kernel moves the cursor straight from the mouse IRQ — the caller
/// only needs to upload an image.
pub fn sys_cursor_init() -> bool {
    syscall(SYS_CURSOR_INIT, 0, 0, 0, 0, 0, 0) == 1
}

/// Upload a 64x64 ARGB (0xAARRGGBB) bitmap for the hardware cursor. `img` must be exactly 64*64 = 4096
/// pixels. Returns true on success. No-op (false) if the hardware cursor isn't initialized.
pub fn sys_cursor_set_image(img: &[u32; 64 * 64]) -> bool {
    syscall(SYS_CURSOR_SET_IMAGE, img.as_ptr() as u64, 0, 0, 0, 0, 0) == 1
}

pub const SYS_GPU_COMPOSITE: u64 = 536;

/// One window to composite on the GPU (U4). `tex_gva` is the window's pixel data GVA — the SHM is
/// already GGTT-mapped by sys_gpu_map_shm at `gpu_gva`, and the pixels are page-aligned at
/// `gpu_gva + size_of::<WindowHeader>()`. `src_w/src_h/src_pitch` describe the window's B8G8R8A8
/// surface; `dst_*` is where to draw it on screen (pixels). The kernel draws it as one textured ortho
/// quad into the backbuffer.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WindowQuad {
    pub tex_gva: u32,
    pub src_w: u32,
    pub src_h: u32,
    pub src_pitch: u32,
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: u32,
    pub dst_h: u32,
    /// U4 Boot 2: per-window opacity, 0..=255 (255 = opaque). The kernel writes this into the
    /// window quad's RT alpha and src-over-blends it over what's behind — enabling fades and
    /// translucent/inactive windows on the GPU path. Must match the kernel `WindowQuad` layout.
    pub opacity: u32,
}

/// GPU-composite `quads` (back-to-front z-order) into the backbuffer over the current wallpaper.
/// The compositor calls this instead of CPU-copying each window's pixels. Returns true on success;
/// on false the caller should fall back to software compositing. Chrome/taskbar are drawn by the
/// caller on top afterward.
pub fn sys_gpu_composite(quads: &[WindowQuad]) -> bool {
    syscall(SYS_GPU_COMPOSITE, quads.as_ptr() as u64, quads.len() as u64, 0, 0, 0, 0) == 1
}

pub const SYS_GPU_DRAW_TEXT: u64 = 537;

/// One glyph to GPU-draw (U5), matching the kernel `GlyphQuad` (repr(C), 9 x 4 = 36 bytes). `dst_*` is
/// the destination pixel rect in the backbuffer; `u0,v0..u1,v1` is the glyph's normalized sub-rect in
/// the font atlas (0..1). `color` is reserved for the colored-text path (ignored by the coverage/white
/// path — a coverage-in-alpha atlas blends as white antialiased text through the plain textured PS).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct GlyphQuad {
    pub dst_x: i32,
    pub dst_y: i32,
    pub dst_w: u32,
    pub dst_h: u32,
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
    pub color: u32,
}

/// GPU-draw a batched glyph run (U5). `atlas_gva` is the font atlas surface's GVA — the atlas SHM must
/// already be GGTT-mapped via `sys_gpu_map_shm`, is LINEAR B8G8R8A8, `atlas_w` x `atlas_h`, `atlas_pitch`
/// bytes/row (64-byte aligned). Each `GlyphQuad` places one glyph; the kernel draws them all as one
/// textured-quad mesh sampling the atlas, blended over whatever is already in the backbuffer. Returns
/// true on success; false → the caller should fall back to CPU text (`canvas.print_str`).
pub fn sys_gpu_draw_text(atlas_gva: u32, atlas_w: u32, atlas_h: u32, atlas_pitch: u32, glyphs: &[GlyphQuad]) -> bool {
    syscall(
        SYS_GPU_DRAW_TEXT,
        atlas_gva as u64,
        atlas_w as u64,
        atlas_h as u64,
        atlas_pitch as u64,
        glyphs.as_ptr() as u64,
        glyphs.len() as u64,
    ) == 1
}


pub fn sys_get_context_switches() -> u64 {
    syscall(523, 0, 0, 0, 0, 0, 0)
}

pub fn sys_get_boot_logs(buf: &mut [u8]) -> usize {
    syscall(518, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0) as usize
}

pub fn sys_get_hw_info(buf: &mut [u8]) -> usize {
    syscall(517, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0) as usize
}

pub fn sys_get_entity_state(state: &mut [u8; 32]) -> bool {
    syscall(520, state.as_mut_ptr() as u64, 32, 0, 0, 0, 0) == 1
}

pub fn sys_get_entity_stats(stats: &mut [f32; 4]) {
    syscall(521, stats.as_mut_ptr() as u64, 4, 0, 0, 0, 0);
}

pub fn sys_get_active_cores() -> usize {
    syscall(522, 0, 0, 0, 0, 0, 0) as usize
}

/// Block until the display's next vertical blank (pipe A). Returns true if the vblank was observed,
/// false on timeout (pipe off / bounded spin elapsed) so the caller can present unpaced rather than
/// stall. Call right before `sys_swap_buffers` to start the scanout BLT inside the blanking interval.
pub fn sys_wait_vsync() -> bool {
    syscall(513, 0, 0, 0, 0, 0, 0) != 0
}

pub fn sys_get_mouse() -> (usize, usize, bool, bool) {
    let m = syscall(505, 0, 0, 0, 0, 0, 0);
    let x = (m >> 32) as usize;
    let y = ((m >> 16) & 0xFFFF) as usize;
    let left = ((m >> 1) & 1) == 1;
    let right = (m & 1) == 1;
    (x, y, left, right)
}

pub fn sys_read_key() -> Option<char> {
    let k = syscall(506, 0, 0, 0, 0, 0, 0);
    if k == 0 { None } else { core::char::from_u32(k as u32) }
}

pub fn sys_get_screen_info() -> (usize, usize, usize) {
    let mut w: u64 = 0;
    let mut h: u64 = 0;
    let mut s: u64 = 0;
    syscall(507, &mut w as *mut u64 as u64, &mut h as *mut u64 as u64, &mut s as *mut u64 as u64, 0, 0, 0);
    (w as usize, h as usize, s as usize)
}

pub fn sys_map_framebuffer() -> u64 {
    syscall(508, 0, 0, 0, 0, 0, 0)
}

pub fn sys_gpu_map_shm(shm_id: u64, gva: u32) -> u64 {
    syscall(509, shm_id, gva as u64, 0, 0, 0, 0)
}

pub fn sys_gpu_copy_rect(src_gva: u32, dst_gva: u32, w: usize, h: usize, dst_x: usize, dst_y: usize) {
    syscall(512, src_gva as u64, dst_gva as u64, w as u64, h as u64, dst_x as u64, dst_y as u64);
}



pub fn sys_fs_count(path: &str) -> usize {
    syscall(510, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0) as usize
}

pub fn sys_fs_get_name(path: &str, idx: usize, buf: &mut [u8]) -> usize {
    syscall(511, idx as u64, buf.as_mut_ptr() as u64, path.as_ptr() as u64, path.len() as u64, 0, 0) as usize
}

/// F: size of a file in bytes, or -1 if it doesn't exist / is a directory. Backed by the kernel's
/// existing get_file_size (ext4 fsize) surfaced via syscall 541.
pub fn sys_file_size(path: &str) -> i64 {
    syscall(541, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0) as i64
}

/// F: filesystem capacity for the nvme mount. Field offsets are fixed (repr(C)) so the kernel can
/// write total@0, free@8, block_size@16 directly into this struct via syscall 542.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct StatFs {
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub block_size: u32,
}

/// F: query total/free bytes of the /mnt/nvme ext4 filesystem. `None` if the kernel couldn't stat it.
pub fn sys_statfs() -> Option<StatFs> {
    let mut sf = StatFs::default();
    let rc = syscall(542, (&mut sf as *mut StatFs) as u64, 0, 0, 0, 0, 0) as i64;
    if rc == 0 { Some(sf) } else { None }
}

// ─────────────────────────── P8: WiFi ───────────────────────────
// The kernel driver holds the radio; these four calls are the whole userspace surface. Layouts are
// repr(C) and the kernel writes the bytes directly, so field order here IS the ABI.

/// One network from the last scan. `flags`: bit0 = privacy (needs a passphrase), bit1 = RSN present
/// (WPA2/WPA3 — the only secured flavour our supplicant speaks), bit2 = this is the current network.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct WifiNetwork {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    pub channel: u8,
    pub band: u8,       // 1 = 2.4 GHz, 0 = 5 GHz
    pub flags: u8,
    pub bssid: [u8; 6],
    pub _pad: [u8; 2],
}

impl Default for WifiNetwork {
    fn default() -> Self {
        Self { ssid: [0; 32], ssid_len: 0, channel: 0, band: 0, flags: 0, bssid: [0; 6], _pad: [0; 2] }
    }
}

impl WifiNetwork {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.ssid[..self.ssid_len as usize]).unwrap_or("?")
    }
    pub fn is_secure(&self) -> bool { self.flags & 0x01 != 0 }
    /// WEP or WPA1: the privacy bit is set but there is no RSN IE.
    pub fn is_legacy_security(&self) -> bool { self.is_secure() && self.flags & 0x02 == 0 }
    /// WPA2, but the AP's GROUP cipher is TKIP rather than CCMP — a router left in WPA/WPA2 mixed
    /// mode. Nyx speaks CCMP only, so the AP rejects the association outright (status 41).
    pub fn needs_tkip(&self) -> bool { self.flags & 0x08 != 0 }
    /// Anything secured in a way we cannot actually join. Kept as one predicate because every
    /// caller wants the same behaviour — grey the row out, refuse Join — while the two accessors
    /// above let the UI say *which* problem it is rather than a bare "unsupported".
    pub fn is_unsupported_security(&self) -> bool { self.is_legacy_security() || self.needs_tkip() }
    pub fn is_current(&self) -> bool { self.flags & 0x04 != 0 }
}

/// Link state — mirrors the kernel's `LinkState`. Values are part of the ABI.
pub const WIFI_IDLE: u32 = 0;
pub const WIFI_CONNECTING: u32 = 1;
pub const WIFI_CONNECTED: u32 = 2;
pub const WIFI_AUTH_FAILED: u32 = 3;
pub const WIFI_NO_LEASE: u32 = 4;
pub const WIFI_HW_FAILED: u32 = 5;
/// The radio is administratively off (`sys_wifi_set_radio(false)`). Not a driver state — the kernel
/// substitutes it into the published status, so every reader sees it without asking separately.
pub const WIFI_RADIO_OFF: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct WifiStatus {
    pub state: u32,
    pub ip: [u8; 4],
    pub mask: [u8; 4],
    pub router: [u8; 4],
    pub dns: [u8; 4],
    pub ssid_len: u8,
    pub ssid: [u8; 32],
}

impl Default for WifiStatus {
    fn default() -> Self {
        Self { state: 0, ip: [0; 4], mask: [0; 4], router: [0; 4], dns: [0; 4], ssid_len: 0, ssid: [0; 32] }
    }
}

impl WifiStatus {
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.ssid[..self.ssid_len as usize]).unwrap_or("")
    }
}

/// Re-run a passive sweep of every channel and return how many networks are visible. BLOCKS for
/// several seconds (the radio dwells on each channel), and is refused while associated.
pub fn sys_wifi_scan() -> usize {
    syscall(543, 0, 0, 0, 0, 0, 0) as usize
}

/// Fill `out` from the last scan; returns how many entries were written.
pub fn sys_wifi_list(out: &mut [WifiNetwork]) -> usize {
    syscall(544, out.as_mut_ptr() as u64, out.len() as u64, 0, 0, 0, 0) as usize
}

/// Join `ssid`. BLOCKS for the association, WPA2 4-way handshake and DHCP exchange — expect ~5-15 s
/// with no chance to repaint, so paint a "Connecting…" frame BEFORE calling. Returns a WIFI_* state.
pub fn sys_wifi_connect(ssid: &str, psk: &str) -> u32 {
    syscall(545, ssid.as_ptr() as u64, ssid.len() as u64,
                 psk.as_ptr() as u64, psk.len() as u64, 0, 0) as u32
}

/// Where the remembered network lives: `ssid\npassphrase\n`, plain text. Written by the Wi-Fi
/// picker after a successful join, read by the boot agent. Note it is NOT encrypted — anything that
/// can read the filesystem can read the passphrase.
pub const WIFI_CONF_PATH: &str = "/mnt/nvme/wifi.conf";

/// Remember `ssid`/`psk` so the boot agent can reconnect without asking. Overwrites any previous
/// entry (one network is remembered, like a phone's "last used").
pub fn wifi_save_network(ssid: &str, psk: &str) -> bool {
    let fd = sys_open_flags(WIFI_CONF_PATH, O_CREAT | O_TRUNC);
    if fd < 0 { return false; }
    let mut buf = [0u8; 160];
    let mut n = 0usize;
    for &b in ssid.as_bytes() { if n < buf.len() { buf[n] = b; n += 1; } }
    if n < buf.len() { buf[n] = b'\n'; n += 1; }
    for &b in psk.as_bytes() { if n < buf.len() { buf[n] = b; n += 1; } }
    if n < buf.len() { buf[n] = b'\n'; n += 1; }
    let ok = sys_write(fd, &buf[..n]) == n as i64;
    sys_close(fd);
    ok
}

/// Blank the remembered network. (There is no unlink syscall, so this truncates the file to zero —
/// `wifi_load_network` then reports nothing saved.)
pub fn wifi_forget_network() -> bool {
    let fd = sys_open_flags(WIFI_CONF_PATH, O_CREAT | O_TRUNC);
    if fd < 0 { return false; }
    sys_close(fd);
    true
}

/// Read the remembered network into `buf`. Returns `(ssid_len, psk_len)`: the SSID occupies
/// `buf[..ssid_len]` and the passphrase `buf[ssid_len..ssid_len + psk_len]`. `None` if nothing is
/// saved or the file is malformed. (Split-buffer rather than `&str` pairs so this stays usable from
/// no_std callers without alloc.)
pub fn wifi_load_network(buf: &mut [u8; 160]) -> Option<(usize, usize)> {
    let fd = sys_open(WIFI_CONF_PATH);
    if fd < 0 { return None; }
    let mut raw = [0u8; 160];
    let n = sys_read(fd, &mut raw);
    sys_close(fd);
    if n <= 0 { return None; }
    let raw = &raw[..n as usize];

    let nl = raw.iter().position(|&b| b == b'\n')?;
    let ssid = &raw[..nl];
    let rest = &raw[nl + 1..];
    let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
    let psk = &rest[..end];
    if ssid.is_empty() { return None; }

    buf[..ssid.len()].copy_from_slice(ssid);
    buf[ssid.len()..ssid.len() + psk.len()].copy_from_slice(psk);
    Some((ssid.len(), psk.len()))
}

/// Leave the current network. The radio becomes idle and scannable again. Takes a moment (the
/// driver deauthenticates and removes the firmware contexts) but nothing like a connect.
pub fn sys_wifi_disconnect() {
    syscall(547, 0, 0, 0, 0, 0, 0);
}

/// Turn the Wi-Fi radio on or off, returning the link state afterwards.
///
/// Off is a soft-kill: it leaves the network, drops the interface, and makes scans and joins no-ops
/// until it is turned back on — but the firmware stays loaded, so switching back on is instant.
/// Turning it on does NOT rejoin anything; it only makes joining possible again.
///
/// BLOCKS on the way off (the teardown talks to the firmware). Returns the state unchanged if
/// another Wi-Fi operation is already running, so compare the result against what you asked for.
pub fn sys_wifi_set_radio(on: bool) -> u32 {
    syscall(548, on as u64, 0, 0, 0, 0, 0) as u32
}

/// Current link state + lease. `None` if the machine has no WiFi adapter.
pub fn sys_wifi_status() -> Option<WifiStatus> {
    let mut st = WifiStatus::default();
    let rc = syscall(546, (&mut st as *mut WifiStatus) as u64, 0, 0, 0, 0, 0) as i64;
    if rc == 0 { Some(st) } else { None }
}

pub fn sys_alloc_pages(pages: usize) -> u64 {
    syscall(519, pages as u64, 0, 0, 0, 0, 0)
}

pub fn sys_get_system_info(info_ptr: *mut SystemInfo) -> u64 {
    syscall(524, info_ptr as u64, 0, 0, 0, 0, 0)
}

pub fn sys_sleep_ms(ms: u64) {
    syscall(525, ms, 0, 0, 0, 0, 0);
}

pub fn sys_get_dsdt(buf: &mut [u8]) -> usize {
    syscall(526, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0) as usize
}

pub fn sys_create_shm(size_bytes: usize) -> u64 {
    syscall(530, size_bytes as u64, 0, 0, 0, 0, 0)
}

pub fn sys_map_shm(shm_id: u64) -> u64 {
    syscall(531, shm_id, 0, 0, 0, 0, 0)
}

/// R1: tear down the CALLER's mapping of an SHM region (pages covering `size` bytes at `base_vaddr`)
/// WITHOUT freeing the physical frames. The compositor calls this on its OLD window mapping after it has
/// switched to the resized buffer, so it no longer references the old frames when the owner frees them.
pub fn sys_unmap_shm(base_vaddr: u64, size: usize) {
    syscall(540, base_vaddr, size as u64, 0, 0, 0, 0);
}

/// R1: destroy an SHM block created with `sys_create_shm` — unmap the CALLER's mapping at `base_vaddr`,
/// return every physical frame to the kernel allocator, and remove it from the registry. Only the owner
/// (the app that created it) should call this, and only once the block is single-owner (after the
/// compositor's `MSG_SHM_RELEASED` ACK). Returns true on success.
pub fn sys_destroy_shm(shm_id: u64, base_vaddr: u64) -> bool {
    syscall(539, shm_id, base_vaddr, 0, 0, 0, 0) == 1
}

pub fn sys_ipc_send(target_pid: u64, msg_type: u64, data1: u64, data2: u64) -> bool {
    syscall(532, target_pid, msg_type, data1, data2, 0, 0) == 1
}

pub fn sys_ipc_recv(msg_ptr: *mut IpcMessage, block: bool) -> bool {
    syscall(533, msg_ptr as u64, if block { 1 } else { 0 }, 0, 0, 0, 0) == 1
}

pub fn sys_socket(domain: u64, typ: u64, protocol: u64) -> i64 {
    syscall(SYS_SOCKET, domain, typ, protocol, 0, 0, 0) as i64
}

pub fn sys_connect(fd: i64, addr: *const u8, len: usize) -> i64 {
    syscall(SYS_CONNECT, fd as u64, addr as u64, len as u64, 0, 0, 0) as i64
}

#[repr(C)]
pub struct sockaddr_in {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

pub struct UdpSocket {
    pub fd: i64,
}

impl UdpSocket {
    pub fn new() -> Option<Self> {
        let fd = sys_socket(2, 2, 0); 
        if fd >= 0 { Some(Self { fd }) } else { None }
    }

    pub fn connect(&self, ip_a: u8, ip_b: u8, ip_c: u8, ip_d: u8, port: u16) -> bool {
        let addr = sockaddr_in {
            sin_family: 2,          
            sin_port: port.to_be(), 
            sin_addr: [ip_a, ip_b, ip_c, ip_d],
            sin_zero: [0; 8],
        };
        sys_connect(self.fd, &addr as *const _ as *const u8, core::mem::size_of::<sockaddr_in>()) == 0
    }

    pub fn send(&self, data: &[u8]) -> bool {
        sys_write(self.fd, data) > 0
    }

    pub fn recv(&self, buf: &mut [u8]) -> i64 {
        sys_read(self.fd, buf)
    }
}

/// A blocking TCP connection.
///
/// The kernel routes this to whichever stack has a link — WiFi when it is up, the wired NIC
/// otherwise — and the choice is fixed for the socket's lifetime. `read` returning 0 means the peer
/// closed (EOF) *or* the link went away; there is no way to tell those apart from here, and for a
/// fetch-then-retry client there is no useful difference.
pub struct TcpStream {
    pub fd: i64,
}

impl TcpStream {
    /// Open a connection. Blocks through the handshake (the kernel gives up after 10 s).
    pub fn connect(ip: [u8; 4], port: u16) -> Option<Self> {
        let fd = sys_socket(2, 1, 0); // AF_INET, SOCK_STREAM
        if fd < 0 {
            return None;
        }
        let addr = sockaddr_in {
            sin_family: 2,
            sin_port: port.to_be(),
            sin_addr: ip,
            sin_zero: [0; 8],
        };
        if sys_connect(fd, &addr as *const _ as *const u8, core::mem::size_of::<sockaddr_in>()) != 0 {
            sys_close(fd);
            return None;
        }
        Some(Self { fd })
    }

    /// Resolve `host`, then connect to it.
    pub fn connect_host(host: &str, port: u16) -> Option<Self> {
        Self::connect(sys_dns_resolve(host)?, port)
    }

    /// One write. May be short — TCP only takes what fits in the send window, so use `write_all`
    /// for anything that has to arrive in full.
    pub fn write(&self, data: &[u8]) -> i64 {
        sys_write(self.fd, data)
    }

    /// Write every byte, looping over short writes. False if the connection failed partway.
    pub fn write_all(&self, mut data: &[u8]) -> bool {
        while !data.is_empty() {
            let n = self.write(data);
            if n <= 0 {
                return false;
            }
            data = &data[n as usize..];
        }
        true
    }

    /// Read up to `buf.len()` bytes. 0 = EOF, negative = error.
    pub fn read(&self, buf: &mut [u8]) -> i64 {
        sys_read(self.fd, buf)
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        sys_close(self.fd);
    }
}

pub fn sys_getrandom(buf: &mut [u8]) -> i64 {
    syscall(318, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0) as i64
}

pub fn sys_yield() {
    syscall(24, 0, 0, 0, 0, 0, 0);
}

pub fn sys_spawn_thread(entry_point: extern "C" fn(), stack_top: *mut u8) -> u64 {
    syscall(58, entry_point as u64, stack_top as u64, 0, 0, 0, 0)
}

pub fn sys_dns_resolve(hostname: &str) -> Option<[u8; 4]> {
    let res = syscall(534, hostname.as_ptr() as u64, hostname.len() as u64, 0, 0, 0, 0);
    if res == 0 {
        None // NXDOMAIN or Timeout
    } else {
        Some([
            (res & 0xFF) as u8,
            ((res >> 8) & 0xFF) as u8,
            ((res >> 16) & 0xFF) as u8,
            ((res >> 24) & 0xFF) as u8,
        ])
    }
}