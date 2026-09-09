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
    /// Meridian step 15: the **caption context** — the secondary, dimmer run the window server draws
    /// after the window's name, `.w-cap .ctx`. NUL-padded, re-read every frame.
    ///
    /// The design's own words: *"Every text editor spends a strip of its own window telling you the
    /// filename, the word count and whether there are unsaved changes. Here all three are already
    /// outside the surface."* That is only true if a window can say those things, and until this
    /// field the protocol carried exactly one string per window — set once, at creation. Text is the
    /// application that makes the gap unavoidable, but it is not specific to it: QCLang's
    /// `sample.ql · 2 qubits · depth 3` and Files' path belong here too.
    ///
    /// ⚠️ Unlike `title`, this is **read live from the mapped header each frame**, the same way
    /// `content_h`/`scroll_off` are. A context that could only be set at creation would be a title
    /// with extra steps.
    pub context: [u8; 64],
    /// Non-zero when the window has unsaved changes. Drawn as the same 3px accent tick a folded
    /// window uses — the design asks for exactly that, and reusing the mark means "there is
    /// something here you have not dealt with" has one appearance in the system.
    pub dirty: u32,
    // U4: pad the header so the pixel data that follows (`shm + size_of::<WindowHeader>()`) is
    // PAGE-ALIGNED. The GPU compositor binds that pixel region as a sampled texture, and every proven
    // texture surface in the render engine uses a 4KB-aligned base — this makes window surfaces match,
    // with zero alignment risk. All apps compute the pixel offset via size_of, so this is transparent.
    // Fixed part is now 260 bytes (9×u32 + 64 title + 96 icon + 64 context), so pad to 4096.
    pub _pad: [u8; 4096 - 260],
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

/// Shell → app: the pointer moved inside this window, with no button held. `data1`/`data2` are
/// window-local pixels.
///
/// This is what makes hover possible in an application at all. Until Meridian there was only
/// [`MSG_MOUSE_EVENT`], which the shell sends on a *click* — so an app could learn where it had been
/// pressed and nothing else, and `NyxApp::on_mouse`'s `clicked` argument was hardcoded `true`
/// because it was always true. Meridian's interiors need the other half: a file row lifts under the
/// pointer, a nav entry lightens, System Monitor shows a sparkline only on the metric being looked
/// at.
///
/// Rate-limited by the shell to one per frame, and coalesced again in `nyx_gui::app`'s drain loop.
/// Both, deliberately: the shell polls the mouse far faster than 60 Hz (`process_input` runs on
/// every loop iteration, not every presented frame) and an app's loop wakes every 16 ms, so an
/// un-metered drag would push several hundred messages a second into a queue nobody is emptying that
/// fast — and the messages that got squeezed out would be the ones that matter, like close.
pub const MSG_MOUSE_MOVE: u64 = 11;

/// Shell → app: the pointer has left this window. Sent exactly once, on the transition.
///
/// Without it, hover is a one-way door: the last row the pointer touched stays lit after the pointer
/// has gone somewhere else entirely, because nothing ever tells the app otherwise. Also sent when a
/// modal shell surface (the Command, the Entity) takes the pointer, and when a shell gesture —
/// drag, resize, scrollbar — captures it.
pub const MSG_MOUSE_LEAVE: u64 = 12;

/// Shell → app: the theme changed. `data1` is 1 for dark, 0 for light.
///
/// Sent to every client when the theme is switched, and once to a new client just after
/// `MSG_WINDOW_CREATED` so an app that starts up mid-session is not the only light window on a dark
/// desktop.
///
/// Until this existed there was **no way at all for an app to learn the shell's theme** — no message,
/// no header field, no syscall — so every ported interior hardcoded `Theme::dark()` and a light-mode
/// shell left all of them dark. The design's whole claim about the light ramp is that it is "one
/// design with eight token values swapped", which is only true if the swap reaches the applications.
pub const MSG_THEME_CHANGED: u64 = 13;

/// App → shell: please switch the theme. `data1` is 1 for dark, 0 for light, 2 for "toggle".
///
/// A request, not a command: the shell owns the theme (it has to regenerate the wallpaper texture),
/// and it answers with a `MSG_THEME_CHANGED` broadcast that the requesting app receives like anyone
/// else. So an app never sets its own theme directly — it asks, and then it is told, which keeps the
/// shell the single source of truth even when the request came from Settings.
pub const MSG_SET_THEME: u64 = 14;

/// `data1` for the two messages above.
pub const THEME_LIGHT: u64 = 0;
pub const THEME_DARK: u64 = 1;
pub const THEME_TOGGLE: u64 = 2;

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
    /// Panel brightness. WARNING: no PS/2 scancode exists for these on a laptop — brightness keys
    /// are consumed by the EC/firmware and never reach the 8042. The codepoints are defined so the
    /// shell path is complete and so nothing else claims them; whether anything ever emits one is a
    /// property of the machine, not of this table. Nothing in the kernel emits them: they are
    /// synthesised by the shell from whichever F-key the user has bound (see `F1`..`F12`).
    pub const BRIGHTNESS_DOWN: char = '\u{E01A}';
    pub const BRIGHTNESS_UP: char = '\u{E01B}';
    /// Volume. These DO arrive: the multimedia keys are extended-set scancodes on the same PS/2
    /// path the arrows use, and `pc_keyboard` already decodes them. The keys work; the audio
    /// subsystem behind them does not exist, so the OSD they raise says exactly that.
    pub const VOLUME_DOWN: char = '\u{E01C}';
    pub const VOLUME_UP: char = '\u{E01D}';
    pub const VOLUME_MUTE: char = '\u{E01E}';

    /// The function row, F1..F12, contiguous. `pc_keyboard` has always decoded these as `RawKey`
    /// and the kernel has always dropped them, so until now no Nyx app could see an F-key at all.
    ///
    /// These matter for brightness. The physical brightness keys are `Fn`+F-something, and the
    /// embedded controller swallows that chord — it never reaches the 8042, so no decoder can
    /// recover it. Pressing the *same key without `Fn`* does arrive, as an ordinary F-key. That is
    /// the only route to the panel backlight on this class of machine, so the shell lets you bind
    /// a pair (see `nyx_api::keys::fkey`).
    pub const F1: char = '\u{E020}';
    pub const F12: char = '\u{E02B}';

    /// The 1-based F-key number for a codepoint in the `F1..=F12` block, else `None`.
    pub const fn fkey(c: char) -> Option<u8> {
        let c = c as u32;
        if c >= F1 as u32 && c <= F12 as u32 {
            Some((c - F1 as u32) as u8 + 1)
        } else {
            None
        }
    }

    /// The codepoint for F`n`, `n` in `1..=12`. Inverse of [`fkey`].
    pub const fn fkey_char(n: u8) -> Option<char> {
        if n >= 1 && n <= 12 {
            char::from_u32(F1 as u32 + (n - 1) as u32)
        } else {
            None
        }
    }
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

/// `execve` with a single argument — the file the new program should open.
///
/// This is what makes "open this with that application" expressible: Files hands a PNG's path to
/// the Image Viewer. It reaches the new program two ways, and both are the same string: as
/// **`argv[1]`** on the SysV entry stack (so a std binary sees it in `std::env::args()`), and
/// through [`sys_launch_arg`] (so a `no_std` binary, whose `_start` cannot reach its own argv, can
/// ask for it).
///
/// One argument, not a vector: that is what the system can currently produce, and every pointer in
/// an array of pointers is a separate unvalidated user read in the syscall path. Widen it when
/// there is a caller that needs it.
pub fn sys_execve_arg(path: &str, arg: &str) -> i64 {
    syscall(
        SYS_EXECVE,
        path.as_ptr() as u64,
        path.len() as u64,
        arg.as_ptr() as u64,
        arg.len() as u64,
        0,
        0,
    ) as i64
}

/// The argument this process was launched with, written into `buf`; returns how many bytes.
///
/// Zero means "launched without one" — the normal case for anything started from the dock or the
/// Command. Truncates rather than failing if `buf` is short, so a too-small buffer surfaces as a
/// path that fails to open with a name the app can print, rather than as an indistinguishable
/// "no argument".
///
/// A `no_std` app cannot read its own `argv`: its `_start` is a plain `extern "C" fn`, and Rust's
/// prologue has already moved RSP by the time any of its code runs. Hence a syscall rather than the
/// stack. A std binary can use either this or `std::env::args()`.
pub fn sys_launch_arg(buf: &mut [u8]) -> usize {
    if buf.is_empty() {
        return 0;
    }
    syscall(SYS_LAUNCH_ARG, buf.as_mut_ptr() as u64, buf.len() as u64, 0, 0, 0, 0) as usize
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

impl RtcTime {
    /// Seconds since 1970-01-01, or `0` when the RTC read failed (`year == 0`).
    ///
    /// Added for `libs/entity`, whose `born_unix` and hourly tick want an absolute instant rather
    /// than a calendar. Zero is a legitimate answer here and not a sentinel to be worked around: a
    /// machine with no working RTC has no birthday to record, and recording an invented one would be
    /// worse than recording none.
    pub fn unix(&self) -> u64 {
        if self.year == 0 {
            return 0;
        }
        let secs = days_from_civil(self.year as i64, self.month as i64, self.day as i64) * 86_400
            + self.hour as i64 * 3600
            + self.min as i64 * 60
            + self.sec as i64;
        secs.max(0) as u64
    }
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

/// The civil date for a unix instant. The public face of [`civil_from_unix`], which the Entity's
/// birth date needs and which nothing else outside this file could reach.
pub fn civil_from_unix_public(secs: i64) -> RtcTime {
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
///
/// `hot_x`/`hot_y` are the pixel inside the bitmap that is the pointer — (0,0) for an arrow whose
/// tip is its top-left corner, the centre for anything drawn around a point. The kernel subtracts it
/// from the pointer position and lets CURPOS carry the sign, so a centred shape near the top-left
/// corner of the screen hangs off the edge correctly instead of being shoved back on screen.
///
/// The hotspot is a required argument rather than a defaulted one on purpose: a cursor uploaded
/// without one points 12px away from where it looks like it points, and nothing about that reads as
/// a bug until you try to click something small.
pub fn sys_cursor_set_image(img: &[u32; 64 * 64], hot_x: u32, hot_y: u32) -> bool {
    syscall(SYS_CURSOR_SET_IMAGE, img.as_ptr() as u64, hot_x as u64, hot_y as u64, 0, 0, 0) == 1
}

/// ★ 557, not 555. 501..=555 were ALL taken when this was added, and 555 was already
/// `SYS_FAULT_SELFTEST`. The duplicate arm sat earlier in the dispatch `match` and silently shadowed
/// it, which `#![allow(warnings)]` in `interrupts.rs` hid by suppressing `unreachable_patterns`.
/// Check `grep -oE '^\s{8}[0-9]+ =>' nyx-kernel/src/interrupts.rs | sort -n | uniq -d` before
/// claiming a number.
pub const SYS_BACKLIGHT: u64 = 557;

/// Panel brightness, 0..=100, or `None` when this display has no PWM-driven backlight — an external
/// panel, or firmware that drives it some other way.
///
/// `delta` of 0 reads without changing anything; non-zero steps by that many percent, rounded onto
/// a 5% grid. The kernel clamps to a floor: a panel driven to invisible is indistinguishable from a
/// hang, and on a machine with no serial console the recovery is a blind power cycle.
pub fn sys_backlight(delta: i32) -> Option<u32> {
    let v = syscall(SYS_BACKLIGHT, delta as i64 as u64, 0, 0, 0, 0, 0);
    if v > 100 { None } else { Some(v as u32) }
}

/// Set panel brightness outright, for a control you drag rather than step. Same return contract and
/// the same clamping as [`sys_backlight`] — asking for 0 gets you the floor, not a black panel.
pub fn sys_backlight_set(percent: u32) -> Option<u32> {
    let v = syscall(SYS_BACKLIGHT, percent as u64, 1, 0, 0, 0, 0);
    if v > 100 { None } else { Some(v as u32) }
}

/// ★ 556, not 554 — 554 was already `SYS_SET_TZ`. See the note on [`SYS_BACKLIGHT`].
pub const SYS_BOOT_MARK: u64 = 556;

pub const SYS_PANEL_PROBE: u64 = 558;

/// What the firmware says about the panel's backlight. A diagnostic — [`sys_backlight`] is the
/// control path.
pub struct PanelInfo {
    /// `_BCL`'s level table. Per the ACPI spec the first two entries are the AC and battery
    /// defaults and the rest are the selectable levels; passed through unfiltered.
    pub levels: [u8; 64],
    pub n_levels: usize,
    /// `_BQC` — the level the firmware believes the panel is at, or `None` if it would not say.
    pub current: Option<u32>,
}

/// Ask ACPI what brightness levels this panel supports. **Read-only.**
///
/// Empty `levels` means the LCD device or its `_BCL` method could not be evaluated, which is a
/// different failure from "evaluated and returned nothing" — worth keeping visible, because on a
/// machine with no serial console the shape of the failure is most of the diagnosis.
pub fn sys_panel_probe() -> PanelInfo {
    let mut levels = [0u8; 64];
    let r = syscall(SYS_PANEL_PROBE, levels.as_mut_ptr() as u64, 64, 0, 0, 0, 0);
    let n = (r & 0xFF) as usize;
    let cur = (r >> 8) & 0xFF;
    PanelInfo {
        levels,
        n_levels: n.min(64),
        current: if cur == 0xFF { None } else { Some(cur as u32) },
    }
}

pub const SYS_PANEL_DIAG: u64 = 559;

/// Ask the kernel why panel brightness is or is not working. **Read-only.**
///
/// Returns a plain-text report: whether an object carrying `_BCL` exists and where, the raw status
/// from each method, every step of ACPICA bring-up, a ladder of path lookups down to the panel, and
/// what a full namespace walk can actually see.
///
/// Text rather than a struct because the interesting facts change with each round of elimination,
/// and only the kernel can see them. Writes into `buf` and returns the number of bytes written.
///
/// Distinct from [`sys_panel_probe`], which reports the levels and folds every failure into an empty
/// table. Conflating "no such device" with "the method refused to run" costs a power cycle to undo.
pub fn sys_panel_diag(buf: &mut [u8]) -> usize {
    let cap = buf.len().min(4096);
    let n = syscall(SYS_PANEL_DIAG, buf.as_mut_ptr() as u64, cap as u64, 0, 0, 0, 0) as usize;
    n.min(cap)
}

pub const SYS_ACPI_LOG: u64 = 560;

/// Everything ACPICA has printed since boot. **Read-only.**
///
/// ACPICA reports table-load failures loudly and specifically, but its output went to an empty
/// `AcpiOsPrintf` stub for the life of this project. This is that channel. Worth reading whenever
/// something ACPI-shaped is missing, because `AcpiLoadTables` returning `AE_OK` does **not** mean
/// the namespace loaded — it rewrites the any-table-failed status to success on purpose.
pub fn sys_acpi_log(buf: &mut [u8]) -> usize {
    let cap = buf.len();
    let n = syscall(SYS_ACPI_LOG, buf.as_mut_ptr() as u64, cap as u64, 0, 0, 0, 0) as usize;
    n.min(cap)
}

pub const SYS_EC_DUMP: u64 = 565;

/// ⚠️ Duplicate syscall numbers are SILENT — `#![allow(warnings)]` suppresses
/// `unreachable_patterns`, so a reused number shadows the original with no error. 501-567 are all
/// taken; the next free is **568**. Before claiming one, run:
///
/// ```text
/// grep -oE "pub const SYS_[A-Z_0-9]+: u64 = [0-9]+" libs/api/src/lib.rs | awk '{print $NF}' | sort -n | uniq -d
/// ```
pub const SYS_LAUNCH_ARG: u64 = 566;

/// Whole-machine metrics for System Monitor — see [`SysMetrics`] and [`sys_get_metrics`].
pub const SYS_SYS_METRICS: u64 = 567;

/// Raw EC register space, captured by `acpi probe 7`. Returns `(ok, data_port, cmd_port, bytes)`.
///
/// This is deliberately raw. ACPI's `_BIF`/`_BST` give a vendor-neutral battery layout, but they
/// need an EmbeddedControl handler and installing one walks the namespace, which crashes this
/// kernel. Reading the EC ports directly needs no walk — the cost is that the register map is
/// model-specific, so the offsets have to be found by correlating against known-good values.
/// Returns `(state, data_port, cmd_port, len)`. `state`: 0 nothing probed · 1 ok ·
/// 2 EC did not come up · 3 EC up but reads timed out. Three causes, three answers — collapsing
/// them into a bool is what made "not captured" unactionable.
pub fn sys_ec_dump(out: &mut [u8; 256]) -> (u8, u16, u16, usize, i32) {
    // 272: the 256-byte dump fills [4..260], so the port/status trailer needs room BEYOND it.
    // At 260 they overlapped and the trailer overwrote EC offsets 0xFA-0xFF.
    let mut buf = [0u8; 272];
    syscall(SYS_EC_DUMP, buf.as_mut_ptr() as u64, 272, 0, 0, 0, 0);
    // 16-bit length: a full dump is 256 bytes, which does not fit in a byte. Reading it as one
    // made a complete, successful dump report "0 bytes".
    let n = (u16::from_le_bytes([buf[1], buf[2]]) as usize).min(256);
    out[..n].copy_from_slice(&buf[4..4 + n]);
    // Ports are 16-bit and live at the tail — see the kernel note; byte-packing them truncated
    // 0x930 to 0x30 and cost a debugging round.
    let dp = u16::from_le_bytes([buf[264], buf[265]]);
    let cp = u16::from_le_bytes([buf[266], buf[267]]);
    let st = i32::from(u16::from_le_bytes([buf[268], buf[269]]));
    (buf[0], dp, cp, n, st)
}

pub const SYS_ACPI_NS_STEP: u64 = 563;
pub const SYS_ACPI_NS_HANDLE: u64 = 564;

/// One node of the ACPI namespace.
pub struct AcpiNode {
    pub handle: u64,
    pub name: [u8; 96],
    pub name_len: usize,
    pub obj_type: i32,
    /// The node's own `Parent` pointer. **Every genuine sibling shares one**, so the first node
    /// whose parent differs from the first node's parent is where a chain left the real child list.
    pub parent: u64,
    /// The raw four-byte ACPI name field, one load, unprocessed.
    pub name4: u32,
}

impl AcpiNode {
    pub fn name_str(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len]).unwrap_or("<non-utf8>")
    }

    /// The raw name as four characters, non-printables shown as `?`.
    ///
    /// ⚠️ Prefer this over [`name_str`] when auditing a suspect chain. `name_str` is built by
    /// `AcpiGetName(ACPI_FULL_PATHNAME)`, which walks `Parent` upward — so on a node that is not
    /// really in the tree it either fails or assembles a plausible path out of garbage. This is one
    /// load of the node's own name field and cannot do that.
    pub fn name4_chars(&self) -> [char; 4] {
        let mut out = ['?'; 4];
        for (i, c) in out.iter_mut().enumerate() {
            let b = (self.name4 >> (i * 8)) as u8;
            if b.is_ascii_graphic() {
                *c = b as char;
            }
        }
        out
    }

    /// A real ACPI name is four printable characters, `_` padded. Anything else is not a node.
    pub fn name_looks_real(&self) -> bool {
        (0..4).all(|i| {
            let b = (self.name4 >> (i * 8)) as u8;
            b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_'
        })
    }
    /// ACPI object type name.
    pub fn type_name(&self) -> &'static str {
        match self.obj_type {
            1 => "Integer", 2 => "String", 3 => "Buffer", 4 => "Package", 5 => "FieldUnit",
            6 => "Device", 7 => "Event", 8 => "Method", 9 => "Mutex", 10 => "Region",
            11 => "Power", 12 => "Processor", 13 => "Thermal", 14 => "BufferField",
            19 => "Reference", 26 => "Scope", 0 => "Any", _ => "?",
        }
    }
}

/// Step to the next child of `parent`. `prev = 0` starts at the first. `None` ends the scope.
///
/// **Print each name before calling this again.** The point of stepping from userspace rather than
/// walking in the kernel is that a node which kills the machine leaves its predecessor on screen —
/// `AcpiWalkNamespace` crashes this kernel and can only report "it died", never where.
pub fn sys_acpi_ns_step(parent: u64, prev: u64) -> Option<AcpiNode> {
    let mut buf = [0u8; 128];
    let ok = syscall(SYS_ACPI_NS_STEP, parent, prev, buf.as_mut_ptr() as u64, 0, 0, 0);
    if ok != 1 { return None; }
    let handle = u64::from_le_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]]);
    let obj_type = i32::from_le_bytes([buf[8], buf[9], buf[10], buf[11]]);
    // ⚠️ Fixed offsets, AHEAD of the variable-length name. Keep in step with the writer in
    // `interrupts.rs`; these two are the only places that know this record's shape.
    let parent = u64::from_le_bytes([
        buf[12], buf[13], buf[14], buf[15], buf[16], buf[17], buf[18], buf[19],
    ]);
    let name4 = u32::from_le_bytes([buf[20], buf[21], buf[22], buf[23]]);
    let l = (buf[24] as usize).min(96);
    let mut name = [0u8; 96];
    name[..l].copy_from_slice(&buf[25..25 + l]);
    Some(AcpiNode { handle, name, name_len: l, obj_type, parent, name4 })
}

/// Resolve an absolute ACPI path (e.g. `\_SB.PCI0`) to a handle. 0 = not found.
pub fn sys_acpi_ns_handle(path: &str) -> u64 {
    syscall(SYS_ACPI_NS_HANDLE, path.as_ptr() as u64, path.len() as u64, 0, 0, 0, 0)
}

pub const SYS_ACPI_PROBE: u64 = 562;

/// Ask the kernel to evaluate ONE ACPI step on the thermal governor's next tick.
///
/// Steps: 1 namespace walk · 2 panel diag · 3 `_BCL` · 4 `_BQC` · 5 battery · 6 `_BCM`.
///
/// Nothing ACPI runs automatically: three boots died inside ACPICA on the governor's first pass and
/// each guess at which call was responsible cost a power cycle. The kernel writes the step number
/// to its CMOS breadcrumb before making the call, so a death names the culprit on the next boot.
/// `arg` is step-specific. For step 1 it is the **walk depth** (0 = unlimited), which lets the
/// namespace-walk crash be bisected by depth without rebuilding the kernel for each attempt.
pub fn sys_acpi_probe(step: u8, arg: u32) {
    syscall(SYS_ACPI_PROBE, step as u64, arg as u64, 0, 0, 0, 0);
}

pub const SYS_BATTERY: u64 = 561;

/// The ACPI control-method battery (`PNP0C0A`), from `_BIF` and `_BST`.
#[derive(Clone, Copy, Default)]
pub struct Battery {
    /// False until an `acpi probe 5` has actually evaluated `_BIF`/`_BST`. Distinct from
    /// `present`, because "nobody has asked yet" and "there is no battery" are different answers.
    pub sampled: bool,
    /// True once the EmbeddedControl address-space handler is registered. Battery methods are not
    /// evaluated at all without it — doing so crashes the kernel.
    pub ec_ok: bool,
    pub present: bool,
    /// 0 = capacities in mWh and rates in mW; 1 = mAh and mA. Chosen by firmware, not by us.
    pub power_unit: i32,
    pub design_cap: i32,
    pub last_full_cap: i32,
    pub design_voltage: i32,
    /// bit0 discharging, bit1 charging, bit2 critical.
    pub state: i32,
    pub present_rate: i32,
    pub remaining_cap: i32,
    pub voltage: i32,
    pub have_bif: bool,
    pub have_bst: bool,
}

impl Battery {
    /// Charge as a percentage of **last-full**, not design capacity — a worn cell should read 100%
    /// when it is as full as it can get, which is what every other OS shows.
    pub fn percent(&self) -> Option<u32> {
        if !self.have_bst || self.last_full_cap <= 0 { return None; }
        Some(((self.remaining_cap as i64 * 100) / self.last_full_cap as i64).clamp(0, 100) as u32)
    }
    pub fn charging(&self) -> bool { self.state & 0b10 != 0 }
    pub fn discharging(&self) -> bool { self.state & 0b01 != 0 }
    pub fn critical(&self) -> bool { self.state & 0b100 != 0 }
    /// `(capacity unit, rate unit)` — `("mWh", "mW")` or `("mAh", "mA")`.
    pub fn units(&self) -> (&'static str, &'static str) {
        if self.power_unit == 1 { ("mAh", "mA") } else { ("mWh", "mW") }
    }
    /// Minutes left at the present rate. `None` unless actively discharging.
    pub fn minutes_remaining(&self) -> Option<u32> {
        if !self.discharging() || self.present_rate <= 0 { return None; }
        Some(((self.remaining_cap as i64 * 60) / self.present_rate as i64).min(u32::MAX as i64) as u32)
    }
    /// Battery health: last-full against design capacity. This is the one that *should* use design.
    pub fn health_percent(&self) -> Option<u32> {
        if !self.have_bif || self.design_cap <= 0 { return None; }
        Some(((self.last_full_cap as i64 * 100) / self.design_cap as i64).clamp(0, 200) as u32)
    }
}

pub fn sys_battery() -> Battery {
    let mut v = [0i32; 11];
    // 2 = never sampled, which is NOT the same as "no battery" — see the kernel note on 561.
    let r = syscall(SYS_BATTERY, v.as_mut_ptr() as u64, 0, 0, 0, 0, 0);
    Battery {
        sampled: r != 2,
        // 3 = the EC handler was never registered, so no battery method was evaluated. Distinct
        // from "sampled and absent" — different cause, different fix.
        ec_ok: r != 3,
        present: v[0] == 1,
        power_unit: v[1],
        design_cap: v[2],
        last_full_cap: v[3],
        design_voltage: v[4],
        state: v[5],
        present_rate: v[6],
        remaining_cap: v[7],
        voltage: v[8],
        have_bif: v[9] == 1,
        have_bst: v[10] == 1,
    }
}

/// Where the kernel's cold-start screen last painted the Nyx mark, packed
/// `x<<48 | y<<32 | w<<16 | h`. Zero when there was no graphical boot screen — no framebuffer,
/// or the user asked for the log instead — in which case there is no handoff to animate and the
/// shell should simply draw its first frame.
///
/// The shell asks rather than deriving it, because a second derivation of "where is the mark"
/// would agree today and drift the first time either layout is touched. The symptom would be a
/// jump on the very first frame of the desktop, which is the one frame this exists to remove.
pub fn sys_boot_mark() -> (i32, i32, i32, i32) {
    let v = syscall(SYS_BOOT_MARK, 0, 0, 0, 0, 0, 0);
    (
        ((v >> 48) & 0xFFFF) as i32,
        ((v >> 32) & 0xFFFF) as i32,
        ((v >> 16) & 0xFFFF) as i32,
        (v & 0xFFFF) as i32,
    )
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

// ── The radio-operation request, Meridian step 20 ────────────────────────────
//
// ★ Why this exists at all.
//
// Meridian retires `apps/wifi` and moves the network picker into a drill-down on the Entity
// surface — which is drawn by `apps/shell`, and `apps/shell` IS the window server. Four of the six
// Wi-Fi syscalls BLOCK for seconds: `sys_wifi_scan` (~7 s of channel dwell), `sys_wifi_connect`
// (~10-15 s of association, 4-way handshake and DHCP), `sys_wifi_disconnect` and
// `sys_wifi_set_radio` (firmware teardown). Calling any of them from the shell stops compositing
// for that whole time, and on this machine that is not merely ugly — it is a bug with two heads:
//
//   1. The hardware cursor is driven from the mouse IRQ, so it keeps moving over a frozen desktop.
//      That is the exact signature of the GL-runtime freeze, and indistinguishable from it.
//   2. The shell's 1 Hz full-frame heartbeat is what keeps the render engine out of RC6. Fifteen
//      seconds without it loses MOCS and forcewake, and the first composite afterwards hangs.
//
// So the shell never makes those calls. It hands the operation to a short-lived helper —
// `apps/wifiagent`, which already existed to do exactly one of them at boot — through
// [`sys_execve_arg`], and then polls `sys_wifi_status` / `sys_wifi_list`, which read a published
// snapshot and never touch the driver lock (see the kernel's own note on syscall 544). The desktop
// keeps drawing at full rate for the entire join.
//
// This block is the wire format between those two processes. It lives in `nyx-api` because that is
// what `nyx-api` is for — the shell and the agent both already depend on it, and neither can afford
// to depend on the other. It is round-tripped by tests in `nyx-meridian`, which is the nearest
// crate with a host test harness.

/// One radio operation the shell wants performed on its behalf.
///
/// Deliberately NOT `Forget`: forgetting only rewrites `wifi.conf`, blocks on nothing, and the
/// shell does it itself. Only operations that enter the driver are worth a process.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WifiOp {
    /// Re-sweep every channel. Refused by the kernel while associated.
    Scan,
    /// Join a network and remember it, so the boot agent reconnects next time. Carries the SSID and
    /// passphrase.
    Join,
    /// Join without writing `wifi.conf`.
    ///
    /// Exactly one network is remembered — saving overwrites — so joining a café network the normal
    /// way would quietly cost you the one your machine comes back to at home. That is what the old
    /// picker's "Connect to this network automatically" checkbox was for, and it is why this is a
    /// separate verb rather than a flag: the helper must not be able to mistake one for the other.
    JoinOnce,
    /// Leave the current network.
    Leave,
    /// Turn the radio on.
    RadioOn,
    /// Turn the radio off.
    RadioOff,
}

impl WifiOp {
    /// The verb as it appears on the wire. One place, so the encoder and the decoder cannot drift.
    pub fn verb(self) -> &'static str {
        match self {
            WifiOp::Scan => "scan",
            WifiOp::Join => "join",
            WifiOp::JoinOnce => "join-once",
            WifiOp::Leave => "leave",
            WifiOp::RadioOn => "radio-on",
            WifiOp::RadioOff => "radio-off",
        }
    }

    /// Every verb, so the decoder and the tests cannot fall behind a new variant.
    pub const ALL: [WifiOp; 6] = [
        WifiOp::Scan,
        WifiOp::Join,
        WifiOp::JoinOnce,
        WifiOp::Leave,
        WifiOp::RadioOn,
        WifiOp::RadioOff,
    ];

    /// Whether this op carries an SSID and passphrase.
    pub fn is_join(self) -> bool {
        matches!(self, WifiOp::Join | WifiOp::JoinOnce)
    }

    fn from_verb(v: &[u8]) -> Option<WifiOp> {
        for op in WifiOp::ALL {
            if op.verb().as_bytes() == v {
                return Some(op);
            }
        }
        None
    }

    /// Roughly how long the kernel call behind this takes, in milliseconds. The shell uses it as a
    /// backstop only — the *outcome* always comes from `sys_wifi_status`, never from a timer — so
    /// these are deliberately generous. Being slow to stop saying "Connecting…" is a much smaller
    /// lie than saying "Couldn't connect" about a join that is still in flight.
    pub fn budget_ms(self) -> usize {
        match self {
            WifiOp::Scan => 20_000,
            WifiOp::Join | WifiOp::JoinOnce => 40_000,
            WifiOp::Leave | WifiOp::RadioOn | WifiOp::RadioOff => 15_000,
        }
    }
}

/// The largest a request can be: a verb, a 32-byte SSID, a 63-character passphrase, two separators.
pub const WIFI_OP_MAX: usize = 16 + 32 + 63 + 2;

/// Encode a request into `buf`, returning its length. `ssid`/`psk` are ignored for every op but
/// [`WifiOp::Join`].
///
/// The format is line-oriented — `verb\nssid\npsk` — because the argument reaches the helper as a
/// `&str` either way, and a self-describing text line is the one encoding that stays debuggable on
/// a machine with no serial console: the whole request can be printed.
///
/// ⚠️ An SSID may legally contain a newline. One that did would be truncated here rather than
/// mis-parsed there, because the decoder splits on the FIRST two newlines and takes the rest
/// verbatim as the passphrase — so a stray newline in an SSID moves bytes into the passphrase and
/// the join simply fails authentication. It cannot forge a different verb, which is the property
/// that actually matters.
pub fn wifi_op_encode(buf: &mut [u8], op: WifiOp, ssid: &str, psk: &str) -> usize {
    let mut n = 0usize;
    let mut put = |src: &[u8], n: &mut usize| {
        for &b in src {
            if *n < buf.len() {
                buf[*n] = b;
                *n += 1;
            }
        }
    };
    put(op.verb().as_bytes(), &mut n);
    if op.is_join() {
        put(b"\n", &mut n);
        put(ssid.as_bytes(), &mut n);
        put(b"\n", &mut n);
        put(psk.as_bytes(), &mut n);
    }
    n
}

/// Decode a request. Returns `(op, ssid, psk)`; the two strings are empty for every op but Join.
///
/// `None` for an unrecognised verb — which is also what an *empty* argument decodes to, and that is
/// load-bearing: `apps/wifiagent` is still spawned by init with no argument at all, and must fall
/// through to its original boot behaviour rather than treat "nothing" as an operation.
pub fn wifi_op_decode(arg: &str) -> Option<(WifiOp, &str, &str)> {
    let b = arg.as_bytes();
    let end = b.iter().position(|&c| c == b'\n').unwrap_or(b.len());
    let op = WifiOp::from_verb(&b[..end])?;
    if !op.is_join() || end == b.len() {
        return Some((op, "", ""));
    }
    let rest = &arg[end + 1..];
    match rest.as_bytes().iter().position(|&c| c == b'\n') {
        Some(i) => Some((op, &rest[..i], &rest[i + 1..])),
        None => Some((op, rest, "")),
    }
}

// ─────────────────────────── Power ───────────────────────────

pub const SYS_POWER: u64 = 568;

/// Switch the machine off through ACPI S5.
pub const POWER_OFF: u64 = 0;
/// Restart. The FADT reset register, then `0xCF9`, then the 8042, then a triple fault.
pub const POWER_RESTART: u64 = 1;

/// Shut down or restart. **Does not return on success.**
///
/// ⚠️ There is no `POWER_SLEEP`, and adding one would be a lie. Nyx implements no ACPI S3: nothing
/// saves or restores device state, and the GPU's wake path is the RC6/MOCS hazard that has already
/// cost this project a run of freezes. What Meridian calls sleep is a *shell* state — the panel
/// drops to 15% and the desktop shows the Entity — and it is deliberately described as the machine
/// staying awake with the screen dimmed, because that is what it is.
///
/// The return value is only ever an error: `EIO` if the firmware declined S5, `EINVAL` for an
/// action that does not exist.
pub fn sys_power(action: u64) -> i64 {
    syscall(SYS_POWER, action, 0, 0, 0, 0, 0) as i64
}

pub fn sys_alloc_pages(pages: usize) -> u64 {
    syscall(519, pages as u64, 0, 0, 0, 0, 0)
}

pub fn sys_get_system_info(info_ptr: *mut SystemInfo) -> u64 {
    syscall(524, info_ptr as u64, 0, 0, 0, 0, 0)
}

/// Whole-machine metrics, for System Monitor. Separate from `SystemInfo` because that struct carries
/// a 64-entry task array (several KB) and these are three scalars a monitor wants every second.
///
/// `repr(C)`, and the kernel writes these bytes directly, so field order here IS the ABI — it is
/// mirrored in `interrupts.rs`. Append only.
#[repr(C)]
#[derive(Default, Clone, Copy)]
pub struct SysMetrics {
    /// CPU busy percentage over the last governor cycle (0-100), published once a second by
    /// `thermal.rs`. A *sampled* figure, not an instantaneous one: the share of scheduler ticks that
    /// went to a non-idle task across the whole second.
    pub cpu_pct: u32,
    pub _reserved: u32,
    /// Physical RAM handed out by the frame allocator, and the total it can ever hand out. The total
    /// counts only bootloader-`Usable` regions, so it reads lower than the installed RAM — firmware
    /// and MMIO holes are not memory this kernel can allocate.
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
}

/// Read the machine's live metrics. `None` if the kernel rejected the buffer.
pub fn sys_get_metrics() -> Option<SysMetrics> {
    let mut m = SysMetrics::default();
    let rc = syscall(SYS_SYS_METRICS, (&mut m as *mut SysMetrics) as u64, 0, 0, 0, 0, 0) as i64;
    if rc == 0 { Some(m) } else { None }
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