#![no_std]
#![no_main]
#![allow(warnings)] // avoid the annotate_snippets renderer ICE on snippet-bearing warnings (see libs/gui)

//! Phase 9 validation app — a spinning, SSAA-antialiased, textured cube driven ENTIRELY
//! through the mini-GL syscalls (514 gl_init, 515 gl_upload_mesh, 516 gl_render, 527 gl_reset).
//!
//! This is the end-to-end proof of the userspace GL path: it uploads one cube mesh + a
//! checkerboard texture once, then each frame builds a single MVP matrix (model-rotate *
//! view-translate * perspective) and hands it to the kernel, which renders the supersampled
//! scene and bilinear-downsamples it to the backbuffer.
//!
//! It uses ONLY nyx-api (no nyx-gui, no allocator, no Vec) so it is immune to the nyx-gui
//! rustc ICE and can run as a bare `run.bin`.

use nyx_api::*;

// ---------------------------------------------------------------------------------------------
// Minimal f32 math (no libm in userspace). Mirrors the kernel's render/math.rs closely enough
// for a rotating cube — the kernel does the real projection; we only need model rotation here.
// ---------------------------------------------------------------------------------------------

const PI: f32 = 3.141_592_7;
const TAU: f32 = 6.283_185_5;

fn wrap_pi(mut x: f32) -> f32 {
    while x > PI {
        x -= TAU;
    }
    while x < -PI {
        x += TAU;
    }
    x
}

/// sin via range-reduced Taylor poly (same shape as the kernel's, good to ~1e-6 near 0).
fn sinf(x: f32) -> f32 {
    let mut x = wrap_pi(x);
    if x > PI / 2.0 {
        x = PI - x;
    } else if x < -PI / 2.0 {
        x = -PI - x;
    }
    let x2 = x * x;
    x * (1.0 + x2 * (-1.0 / 6.0 + x2 * (1.0 / 120.0 + x2 * (-1.0 / 5040.0 + x2 * (1.0 / 362880.0)))))
}

#[inline]
fn cosf(x: f32) -> f32 {
    sinf(x + PI / 2.0)
}

#[inline]
fn tanf(x: f32) -> f32 {
    sinf(x) / cosf(x)
}

// ---------------------------------------------------------------------------------------------
// Column-major 4x4 (m[col*4 + row]), matching the kernel Mat4 the syscall expects.
// ---------------------------------------------------------------------------------------------

type M4 = [f32; 16];

fn identity() -> M4 {
    let mut m = [0.0f32; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

fn mul(a: &M4, b: &M4) -> M4 {
    // a * b, both column-major.
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0;
            for k in 0..4 {
                // a[row,k] * b[k,col]
                s += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = s;
        }
    }
    out
}

fn translate(x: f32, y: f32, z: f32) -> M4 {
    let mut m = identity();
    m[12] = x;
    m[13] = y;
    m[14] = z;
    m
}

fn rotate_y(a: f32) -> M4 {
    let (s, c) = (sinf(a), cosf(a));
    let mut m = identity();
    m[0] = c;
    m[2] = -s; // col0,row2
    m[8] = s; // col2,row0
    m[10] = c;
    m
}

fn rotate_x(a: f32) -> M4 {
    let (s, c) = (sinf(a), cosf(a));
    let mut m = identity();
    m[5] = c;
    m[6] = s; // col1,row2
    m[9] = -s; // col2,row1
    m[10] = c;
    m
}

fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let f = 1.0 / tanf(fovy / 2.0);
    let mut m = [0.0f32; 16];
    m[0] = f / aspect; // col0,row0
    m[5] = f; // col1,row1
    m[10] = (far + near) / (near - far); // col2,row2
    m[11] = -1.0; // col2,row3 (w = -z_eye)
    m[14] = (2.0 * far * near) / (near - far); // col3,row2
    m
}

// ---------------------------------------------------------------------------------------------
// Cube geometry: 24 verts (4 per face so each face gets full 0..1 UVs), 36 indices.
// ---------------------------------------------------------------------------------------------

// One face as 4 corners (positions) — we place all 6 faces below.
fn build_cube() -> ([GlVertex; 24], [u32; 36]) {
    // 8 corners of a unit cube centered at origin.
    // Face order: +Z, -Z, +X, -X, +Y, -Y. Each face: BL, BR, TR, TL with UVs (0,0)(1,0)(1,1)(0,1).
    const H: f32 = 0.8;
    // (px,py,pz) for the 4 corners of each face, in BL,BR,TR,TL order.
    let faces: [[(f32, f32, f32); 4]; 6] = [
        // +Z
        [(-H, -H, H), (H, -H, H), (H, H, H), (-H, H, H)],
        // -Z
        [(H, -H, -H), (-H, -H, -H), (-H, H, -H), (H, H, -H)],
        // +X
        [(H, -H, H), (H, -H, -H), (H, H, -H), (H, H, H)],
        // -X
        [(-H, -H, -H), (-H, -H, H), (-H, H, H), (-H, H, -H)],
        // +Y
        [(-H, H, H), (H, H, H), (H, H, -H), (-H, H, -H)],
        // -Y
        [(-H, -H, -H), (H, -H, -H), (H, -H, H), (-H, -H, H)],
    ];
    let uvs: [(f32, f32); 4] = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];

    let mut verts = [GlVertex::textured(0.0, 0.0, 0.0, 0.0, 0.0); 24];
    let mut indices = [0u32; 36];
    let mut vi = 0usize;
    let mut ii = 0usize;
    let mut f = 0usize;
    while f < 6 {
        let base = (f * 4) as u32;
        let mut c = 0usize;
        while c < 4 {
            let (px, py, pz) = faces[f][c];
            let (u, v) = uvs[c];
            verts[vi] = GlVertex::textured(px, py, pz, u, v);
            vi += 1;
            c += 1;
        }
        // Two triangles: BL,BR,TR and BL,TR,TL.
        indices[ii] = base;
        indices[ii + 1] = base + 1;
        indices[ii + 2] = base + 2;
        indices[ii + 3] = base;
        indices[ii + 4] = base + 2;
        indices[ii + 5] = base + 3;
        ii += 6;
        f += 1;
    }
    (verts, indices)
}

// ---------------------------------------------------------------------------------------------
// A small checkerboard texture (B8G8R8A8: A<<24|R<<16|G<<8|B).
// ---------------------------------------------------------------------------------------------

// 32x32 (not 8x8): a LINEAR surface sampled by the Gen9 texture unit needs its row pitch aligned to
// ~64 bytes. An 8x8 texture is pitch 32 (below that) and sampled BLACK; 32x32 is pitch 128 — safely
// above, and matches the proven boot self-test's 64x64 checker. 4 KB stack array, checker cell 4 px.
const TEX_W: u32 = 32;
const TEX_H: u32 = 32;

fn build_texture() -> [u32; (TEX_W * TEX_H) as usize] {
    let mut t = [0u32; (TEX_W * TEX_H) as usize];
    let mut y = 0u32;
    while y < TEX_H {
        let mut x = 0u32;
        while x < TEX_W {
            let on = (((x / 4) ^ (y / 4)) & 1) == 0;
            // teal vs deep-purple so both the checker AND the AA edges are obvious.
            let c: u32 = if on { 0xFF20_C0B0 } else { 0xFF40_2060 };
            t[(y * TEX_W + x) as usize] = c;
            x += 1;
        }
        y += 1;
    }
    t
}

// ---------------------------------------------------------------------------------------------
// Loader screen. glcube is nyx-api-only (no nyx-gui / no font engine), so we embed a tiny 5x7
// bitmap font covering ONLY the glyphs in the startup message and CPU-paint the window's SHM
// buffer directly. Shown while the GL context spins up (context alloc + mesh upload + first
// frame), so the window reads "INITIALIZING 3D ENGINE …" instead of flashing blank until the
// first GPU frame overwrites it.
// ---------------------------------------------------------------------------------------------

/// 5x7 glyph (7 rows, low 5 bits each, bit4 = leftmost column). Only the characters used by the
/// loader message are encoded; everything else (incl. space) is blank.
fn glyph(c: u8) -> [u8; 7] {
    match c {
        b'I' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1F],
        b'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        b'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        b'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        b'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        b'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        b'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0E],
        b'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        b'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        b'3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        _ => [0, 0, 0, 0, 0, 0, 0],
    }
}

#[inline]
unsafe fn put_px(px: *mut u32, w: u32, h: u32, x: i32, y: i32, c: u32) {
    if x >= 0 && y >= 0 && (x as u32) < w && (y as u32) < h {
        *px.add((y as u32 * w + x as u32) as usize) = c;
    }
}

/// Fill an axis-aligned rect (clipped) with a solid colour.
unsafe fn fill_rect(px: *mut u32, w: u32, h: u32, x0: i32, y0: i32, rw: i32, rh: i32, c: u32) {
    let mut dy = 0;
    while dy < rh {
        let mut dx = 0;
        while dx < rw {
            put_px(px, w, h, x0 + dx, y0 + dy, c);
            dx += 1;
        }
        dy += 1;
    }
}

/// Draw an ASCII string in the 5x7 font, `scale`x, left-aligned at (x0,y0).
unsafe fn draw_text(px: *mut u32, w: u32, h: u32, s: &[u8], x0: i32, y0: i32, scale: i32, color: u32) {
    let mut cx = x0;
    for &ch in s {
        let g = glyph(ch);
        let mut row = 0i32;
        while row < 7 {
            let bits = g[row as usize];
            let mut col = 0i32;
            while col < 5 {
                if (bits >> (4 - col) as u32) & 1 == 1 {
                    fill_rect(px, w, h, cx + col * scale, y0 + row * scale, scale, scale, color);
                }
                col += 1;
            }
            row += 1;
        }
        cx += 6 * scale; // 5 columns + 1 blank spacing
    }
}

/// Paint the whole loader frame into the window SHM buffer. `phase` animates the trailing dots.
unsafe fn draw_loader(px: *mut u32, w: u32, h: u32, phase: u32) {
    // Dark slate background.
    fill_rect(px, w, h, 0, 0, w as i32, h as i32, 0xFF14181C);

    let msg: &[u8] = b"INITIALIZING 3D ENGINE";
    let scale = 3i32;
    let text_w = msg.len() as i32 * 6 * scale - scale; // drop the trailing inter-glyph gap
    let x0 = (w as i32 - text_w) / 2;
    let y0 = h as i32 / 2 - 40;
    draw_text(px, w, h, msg, x0, y0, scale, 0xFFE8E8E8);

    // Three dots below the message; (phase % 4) of them light up in the accent colour.
    let lit = (phase % 4) as i32;
    let dot = 12i32;
    let gap = 24i32;
    let base_x = w as i32 / 2 - (gap + dot / 2);
    let dy = y0 + 44;
    let mut i = 0i32;
    while i < 3 {
        let c = if i < lit { 0xFFE67E22 } else { 0xFF33383E };
        fill_rect(px, w, h, base_x + i * gap, dy, dot, dot, c);
        i += 1;
    }
}

// ---------------------------------------------------------------------------------------------

const COMPOSITOR_PID: u64 = 4;
const WIN_W: u32 = 640;
const WIN_H: u32 = 480;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // ── Create a normal compositor window (same protocol as libs/gui app::run, but we drive GL
    //    instead of a CPU canvas). The GL path renders into THIS window's SHM buffer; the compositor
    //    composites it — glcube no longer blits the whole scanout, so it can't fight the desktop. ──
    let hdr_size = core::mem::size_of::<WindowHeader>();
    let total_size = hdr_size + (WIN_W * WIN_H * 4) as usize;
    let shm_id = sys_create_shm(total_size);
    if shm_id == 0 {
        sys_print("[GLCUBE] FATAL: sys_create_shm failed\n");
        sys_exit(1);
    }
    let base_ptr = sys_map_shm(shm_id) as *mut u8;
    if base_ptr.is_null() {
        sys_print("[GLCUBE] FATAL: sys_map_shm failed\n");
        sys_exit(1);
    }

    // Fill the window header (compositor reads this on MSG_REQ_WINDOW).
    unsafe {
        let header = &mut *(base_ptr as *mut WindowHeader);
        header.magic = WIN_MAGIC;
        header.requested_x = -1;
        header.requested_y = -1;
        header.width = WIN_W;
        header.height = WIN_H;
        header.flags = WIN_FLAG_NONE;
        header.title.fill(0);
        let title = b"GL Cube (3D)";
        let n = title.len().min(64);
        header.title[..n].copy_from_slice(&title[..n]);
    }

    // Register the window and wait for the compositor's ack.
    if !sys_ipc_send(COMPOSITOR_PID, MSG_REQ_WINDOW, shm_id, 0) {
        sys_print("[GLCUBE] FATAL: MSG_REQ_WINDOW send failed\n");
        sys_exit(1);
    }
    let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
    loop {
        if sys_ipc_recv(&mut msg, true) && msg.msg_type == MSG_WINDOW_CREATED {
            break;
        }
    }
    let pixels_ptr = unsafe { base_ptr.add(hdr_size) } as *mut u32;

    // ── Show the loader immediately: paint "INITIALIZING 3D ENGINE" into the window buffer and ask
    //    the compositor to composite it, so the window never flashes blank while the GL context spins
    //    up (context alloc + mesh upload + first GPU frame). Pure CPU paint into our own SHM. ──
    unsafe { draw_loader(pixels_ptr, WIN_W, WIN_H, 0); }
    sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);

    // ── Init GL to render into THIS window's pixel buffer, then upload the cube + checkerboard. ──
    if !sys_gl_init(WIN_W, WIN_H, pixels_ptr as *const u32) {
        sys_print("[GLCUBE] FATAL: sys_gl_init failed (render engine not ready?)\n");
        sys_exit(1);
    }
    let (verts, indices) = build_cube();
    let tex = build_texture();
    if sys_gl_upload_mesh(&verts, &indices, &tex, TEX_W, TEX_H).is_none() {
        sys_print("[GLCUBE] FATAL: sys_gl_upload_mesh failed\n");
        sys_exit(2);
    }

    let aspect = WIN_W as f32 / WIN_H as f32;
    let proj = perspective(PI / 4.0, aspect, 0.1, 100.0);
    let view = translate(0.0, 0.0, -3.0); // push the cube back from the camera

    // ── Render loop: draw a frame into the window buffer, tell the compositor to composite it, and
    //    pace. Runs until the compositor closes the window. It lives in a window now, so the desktop
    //    is always visible; a GPU failure just leaves a stale/black WINDOW, never a black machine. ──
    let mut angle: f32 = 0.0;
    let mut fails: u32 = 0;
    // Until the FIRST successful GPU frame lands, keep animating the loader (dots cycle) so the user
    // sees progress rather than a frozen screen — the first good `sys_gl_render` overwrites it with
    // the cube.
    let mut engine_up = false;
    let mut spin: u32 = 0;
    loop {
        // Drain compositor events (non-blocking). Close = tear down GL + exit cleanly.
        while sys_ipc_recv(&mut msg, false) {
            if msg.msg_type == MSG_WINDOW_CLOSE {
                sys_gl_reset();
                sys_exit(0);
            }
            // MSG_MOUSE_EVENT / MSG_KEY_EVENT / MSG_WINDOW_RESIZED ignored for this validation app.
        }

        // model = rot_y(angle) * rot_x(angle*0.6): tumble on two axes.
        let model = mul(&rotate_y(angle), &rotate_x(angle * 0.6));
        // mvp = proj * view * model  (column-major, applied right-to-left to a column vector).
        let mvp = mul(&proj, &mul(&view, &model));
        let mvps: [[f32; 16]; 1] = [mvp];

        if sys_gl_render(&mvps) {
            fails = 0;
            if !engine_up {
                engine_up = true;
            }
            // Frame is now in our SHM window buffer — ask the compositor to composite it.
            sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
        } else if !engine_up {
            // Still coming up (context warm-up / first-frame RC6 wake): keep the ANIMATED loader on
            // screen instead of flashing blank or bailing early. Patient window (~5s) before giving up.
            fails += 1;
            spin += 1;
            unsafe { draw_loader(pixels_ptr, WIN_W, WIN_H, spin / 8); }
            sys_ipc_send(COMPOSITOR_PID, MSG_FLUSH_WINDOW, 0, 0);
            if fails >= 300 {
                sys_print("[GLCUBE] engine never rendered a frame (300 tries) — exiting\n");
                sys_gl_reset();
                sys_exit(3);
            }
        } else {
            // Was rendering fine, now failing — don't hot-spin a persistently failing render.
            fails += 1;
            sys_print("[GLCUBE] render call returned false (see sysmon [SCENE] HUNG/FAULT_VA)\n");
            if fails >= 3 {
                // Bail FAST so a GPU fault/hang doesn't freeze userspace: 3 failed frames give us the
                // [SCENE] HUNG FAULT_VA lines, then we exit to the desktop.
                sys_print("[GLCUBE] giving up after 3 render failures — exiting (read sysmon FAULT_VA)\n");
                sys_gl_reset();
                sys_exit(3);
            }
        }

        angle += 0.02;
        if angle > TAU {
            angle -= TAU;
        }
        sys_sleep_ms(16); // ~60 fps pacing
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_print("[GLCUBE] FATAL: panicked!\n");
    sys_exit(99);
}
