//! Fatal-error and fault reporting on the screen that is actually being scanned out.
//!
//! ## Why this module exists
//!
//! `gui::SCREEN_PAINTER` points at the **firmware GOP framebuffer** (`main.rs`, `fb.buffer_mut()`).
//! That was the scanout surface right up until P1a, when `arm_scanout_plane` re-pointed the display
//! plane at our own buffer at GVA `0x1600_0000` (`drivers/gpu/intel/mod.rs`). Nothing updated the
//! panic path to match — so `trigger_rsod`, the `#[panic_handler]` red fill, and every
//! `vga_println!` have since been writing into a buffer **nobody displays**.
//!
//! The practical effect: since P1a, *every kernel panic has been completely invisible*. The machine
//! paints red, prints the message, and to the person in front of it simply stops. That is a large
//! part of why "froze with no output" became this project's default symptom — and combined with
//! `BOOT_LOG` living in RAM that does not survive the power cycle, the kernel had no working way to
//! report a fatal error at all. The CMOS post-mortem (`postmortem.rs`) was the only channel left.
//!
//! ## The no-lock rule
//!
//! Everything here is lock-free, allocation-free and interrupt-safe, because a panic can be raised
//! **while holding any lock in the kernel**, including the one the reporting path would want:
//!
//!   * `INTEL_GPU` is a `Mutex`. Asking the driver where the scanout is would deadlock any panic
//!     raised inside the GPU driver — which is exactly where display bugs panic. So the address is
//!     copied into the atomics below at init and read back here without touching the driver.
//!   * `VGA_LOGGER` is a `Mutex` (`vga_log.rs`), so `vga_println!` is off-limits for the same
//!     reason. Glyphs are blitted directly instead; `get_raster` is a pure lookup over static data
//!     and takes nothing.
//!   * No `format!`. `core::fmt` itself needs no heap — only `format!` does — so callers pass
//!     `fmt::Arguments` and it renders straight through. The panic handler learned this the hard
//!     way already; see the comment above it in `main.rs`.
//!
//! ## Both buffers, every time
//!
//! `paint` writes to the owned scanout *and* the firmware framebuffer whenever both are known. A
//! panic can land before `arm_scanout_plane` has run (in which case the GOP buffer is still live),
//! after it (in which case the scanout is), or with the plane scan having failed altogether (P1a
//! falls back to the GVA-0 path). Deciding which is live means reading driver state behind a lock,
//! which is the one thing this module must not do. Painting both is two memory fills on a machine
//! that is already dead, and it removes the guesswork.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use noto_sans_mono_bitmap::{get_raster, FontWeight, RasterHeight};

/// Only `size_32` is enabled for `noto-sans-mono-bitmap` in `Cargo.toml`, so this is the only
/// raster available. It is also the right choice: a fatal message is read across a room, once.
const FONT_H: RasterHeight = RasterHeight::Size32;
const LINE_H: usize = 34;
const MARGIN_X: usize = 32;

/// Colours are chosen so white text stays legible on the fill — full-saturation red at 255 makes
/// antialiased glyph edges vibrate. Deep red for fatal, amber for a process that died and took
/// nothing else with it.
pub const FATAL_BG: (u8, u8, u8) = (138, 12, 20);
pub const FAULT_BG: (u8, u8, u8) = (128, 74, 4);
const TEXT: (u8, u8, u8) = (255, 255, 255);

/// One paintable surface: a linear framebuffer we hold a CPU-visible pointer to.
///
/// `stride` is in PIXELS, matching `bootloader_api`'s `FrameBufferInfo` (`gui.rs` indexes
/// `y * stride + x` and multiplies by `bpp` afterwards). `rgb` distinguishes `PixelFormat::Rgb`
/// from `Bgr`; getting it backwards swaps red and blue, which on a red panic screen would be a
/// silently blue one.
struct Target {
    virt: AtomicU64,
    w: AtomicU32,
    h: AtomicU32,
    stride: AtomicU32,
    bpp: AtomicU32,
    rgb: AtomicBool,
    /// PLANE_CTL bits 12:10. Linear for the firmware buffer; read from hardware for our scanout.
    tiling: AtomicU32,
}

impl Target {
    const fn new() -> Self {
        Target {
            virt: AtomicU64::new(0),
            w: AtomicU32::new(0),
            h: AtomicU32::new(0),
            stride: AtomicU32::new(0),
            bpp: AtomicU32::new(0),
            rgb: AtomicBool::new(false),
            tiling: AtomicU32::new(0),
        }
    }

    fn set(&self, virt: u64, w: u32, h: u32, stride: u32, bpp: u32, rgb: bool, tiling: u32) {
        self.tiling.store(tiling, Ordering::Relaxed);
        self.w.store(w, Ordering::Relaxed);
        self.h.store(h, Ordering::Relaxed);
        self.stride.store(stride, Ordering::Relaxed);
        self.bpp.store(bpp, Ordering::Relaxed);
        self.rgb.store(rgb, Ordering::Relaxed);
        // Published LAST, with Release: a reader that sees a non-zero pointer is guaranteed to see
        // the geometry that goes with it. Writing the pointer first would let a panic land in the
        // window where the address is live but the width is still 0 — and then every row lands at
        // offset 0 and the "screen" is one smear.
        self.virt.store(virt, Ordering::Release);
    }

    /// Geometry snapshot, or None if this target was never registered.
    fn get(&self) -> Option<Geom> {
        let virt = self.virt.load(Ordering::Acquire);
        if virt == 0 {
            return None;
        }
        let w = self.w.load(Ordering::Relaxed) as usize;
        let h = self.h.load(Ordering::Relaxed) as usize;
        let bpp = self.bpp.load(Ordering::Relaxed) as usize;
        if w == 0 || h == 0 || bpp < 3 {
            return None;
        }
        Some(Geom {
            base: virt as *mut u8,
            w,
            h,
            stride: self.stride.load(Ordering::Relaxed) as usize,
            tiling: self.tiling.load(Ordering::Relaxed),
            bpp,
            rgb: self.rgb.load(Ordering::Relaxed),
        })
    }
}

#[derive(Clone, Copy)]
struct Geom {
    base: *mut u8,
    w: usize,
    h: usize,
    stride: usize,
    tiling: u32,
    bpp: usize,
    rgb: bool,
}

/// Display-plane tiling, read from `PLANE_CTL` bits 12:10 at registration.
///
/// ★ This is why the fault screen painted in stripes with the desktop showing through.
///
/// The panic path writes the scanout **linearly with the CPU**, while the desktop reaches the same
/// buffer through the GPU BLT. If the plane is tiled, the display engine de-tiles what it scans out
/// — so GPU-written pixels land correctly and CPU-written ones are scattered into bands. Only the
/// panic screen looked wrong, which is exactly the shape of the symptom.
///
/// Rather than reconfigure the plane (the desktop currently renders correctly, so its tiling and
/// the BLT already agree — changing it would break the working path to fix the broken one), the
/// panic writes are swizzled to match whatever the plane is actually set to.
pub const TILE_LINEAR: u32 = 0;
pub const TILE_X: u32 = 1;
pub const TILE_Y: u32 = 4;
pub const TILE_YF: u32 = 5;

impl Geom {
    /// Byte offset of pixel (x, y), honouring the plane's tiling.
    ///
    /// X-tile: 512 bytes × 8 rows = 4 KB per tile.
    /// Y-tile: 128 bytes × 32 rows = 4 KB per tile, laid out as eight 16-byte-wide columns, each
    /// column being 32 rows of 16 bytes (512 B). That column-major order inside the tile is the
    /// part that turns a linear write into visible banding.
    #[inline]
    fn byte_offset(&self, x: usize, y: usize) -> usize {
        let bx = x * self.bpp;
        let pitch = self.stride * self.bpp;
        match self.tiling {
            TILE_X => {
                let tiles_per_row = (pitch / 512).max(1);
                let tile = (y / 8) * tiles_per_row + bx / 512;
                tile * 4096 + (y % 8) * 512 + (bx % 512)
            }
            TILE_Y | TILE_YF => {
                let tiles_per_row = (pitch / 128).max(1);
                let tile = (y / 32) * tiles_per_row + bx / 128;
                let ix = bx % 128;
                tile * 4096 + (ix / 16) * 512 + (y % 32) * 16 + (ix % 16)
            }
            _ => y * pitch + bx,
        }
    }

    #[inline]
    unsafe fn put(&self, x: usize, y: usize, c: (u8, u8, u8)) {
        if x >= self.w || y >= self.h {
            return;
        }
        let idx = self.byte_offset(x, y);
        let p = self.base.add(idx);
        // Volatile: this memory is scanned out by the display engine, and we are about to `hlt`
        // forever. A compiler that decided the stores were dead would leave a blank screen.
        if self.rgb {
            core::ptr::write_volatile(p, c.0);
            core::ptr::write_volatile(p.add(1), c.1);
            core::ptr::write_volatile(p.add(2), c.2);
        } else {
            core::ptr::write_volatile(p, c.2);
            core::ptr::write_volatile(p.add(1), c.1);
            core::ptr::write_volatile(p.add(2), c.0);
        }
    }

    /// Fill `rows` scanlines starting at `y0`. Walks a row pointer instead of going through `put`
    /// per pixel: a full-screen fill at 1080p is ~2M pixels *per target*, and paying a bounds check
    /// and a multiply on each one turns "the screen goes red" into a visible wipe. The row bounds
    /// are checked once, here, which is the only place they can actually be violated.
    ///
    /// The 32-bpp case writes one `u32` instead of three bytes. That is not micro-optimisation:
    /// the firmware framebuffer is uncached MMIO, where every store goes to the bus, so three
    /// stores per pixel costs three times the bus traffic of one.
    unsafe fn fill(&self, y0: usize, rows: usize, c: (u8, u8, u8)) {
        let (b0, b1, b2) = if self.rgb { (c.0, c.1, c.2) } else { (c.2, c.1, c.0) };
        let y1 = (y0 + rows).min(self.h);
        let word = u32::from_le_bytes([b0, b1, b2, 0xFF]);
        let pitch = self.stride * self.bpp;

        // ★★ A SOLID COLOUR IS SWIZZLE-INVARIANT. Fill the BYTES, not the pixels.
        //
        // This replaces a per-pixel `put` loop that walked `0..w` × `y0..y1` through `byte_offset`,
        // and the photograph of the last red screen is why. It came up as red with hundreds of short
        // dark dashes punched through it — leftover desktop — and dashes across the glyph rows made
        // the one diagnostic on the screen barely transcribable.
        //
        // A per-pixel tiled fill only covers the bytes that `byte_offset` maps some (x, y) onto. Any
        // error in `stride`, any padding row inside the last tile row, any mismatch between the
        // plane's real pitch and ours, and those bytes are never written — so they keep whatever the
        // compositor left there. The pixels are all "covered" and the screen is still dirty.
        //
        // But red is red at every address. If the fill covers the whole byte extent linearly, the
        // swizzle cannot matter: whatever permutation the display engine applies, it is permuting a
        // buffer that is uniformly the fill colour. So a full-surface fill needs no tiling knowledge
        // at all, and gains none of the ways to be wrong about it.
        //
        // Text still goes through `put`, which does need the swizzle — but a glyph landing a few
        // pixels off is legible, and a background full of holes is not.
        if self.tiling != TILE_LINEAR {
            if y0 == 0 && y1 >= self.h && self.bpp == 4 {
                // Tile rows are 8 scanlines on X, 32 on Y, and the surface's allocation is padded up
                // to a whole number of them. Rounding UP is what reaches the bytes belonging to
                // pixels in the final partial tile row — the ones a `0..h` pixel loop cannot name.
                let tile_h = if self.tiling == TILE_X { 8 } else { 32 };
                let rows = self.h.div_ceil(tile_h) * tile_h;
                let words = (pitch / 4) * rows;
                let mut p = self.base as *mut u32;
                for _ in 0..words {
                    core::ptr::write_volatile(p, word);
                    p = p.add(1);
                }
                return;
            }
            // A partial fill genuinely is a rectangle of pixels, so it has to know the swizzle.
            // Nothing on the fatal path takes this branch.
            for y in y0..y1 {
                for x in 0..self.w {
                    self.put(x, y, c);
                }
            }
            return;
        }

        for y in y0..y1 {
            let row = self.base.add(y * self.stride * self.bpp);
            if self.bpp == 4 {
                let mut p = row as *mut u32;
                for _ in 0..self.w {
                    core::ptr::write_volatile(p, word);
                    p = p.add(1);
                }
            } else {
                let mut p = row;
                for _ in 0..self.w {
                    core::ptr::write_volatile(p, b0);
                    core::ptr::write_volatile(p.add(1), b1);
                    core::ptr::write_volatile(p.add(2), b2);
                    p = p.add(self.bpp);
                }
            }
        }
    }
}

/// The buffer the display plane was pointed at by `arm_scanout_plane` (P1a).
static SCANOUT: Target = Target::new();
/// The firmware GOP framebuffer — live until the plane is armed, and the fallback if P1a's plane
/// scan or allocation failed and the present stayed on the GVA-0 path.
static FIRMWARE: Target = Target::new();

/// Record the firmware framebuffer. Called once from `kernel_main` as soon as `SCREEN_PAINTER` is
/// built, so a panic during the rest of boot is already reportable.
pub fn register_firmware(virt: u64, w: u32, h: u32, stride: u32, bpp: u32, rgb: bool) {
    // The firmware framebuffer from the bootloader is always linear.
    FIRMWARE.set(virt, w, h, stride, bpp, rgb, TILE_LINEAR);
}

/// Record the owned scanout buffer. Called from the P1a block in the Intel driver, which already
/// computes this CPU-visible address via `phys_to_virt` in order to zero the buffer.
///
/// Registering here rather than reading it back from the driver later is the whole point: the panic
/// path must never take the `INTEL_GPU` lock.
pub fn register_scanout(virt: u64, w: u32, h: u32, stride: u32, bpp: u32, rgb: bool, tiling: u32) {
    SCANOUT.set(virt, w, h, stride, bpp, rgb, tiling);
}

/// A live paint surface, for code outside the panic path that also has to reach the screen without
/// going through the GPU driver's lock. Today that is exactly one caller: the cold-start screen.
///
/// It lives here rather than in a module of its own because the hard part is not writing pixels —
/// it is knowing *which* buffer is being scanned out, and that knowledge is already here, kept
/// current by two registration calls at the only two moments it changes. A second copy of that
/// reasoning is a second thing to be wrong the next time the plane moves, and the failure mode is
/// the one this project has already paid for: drawing perfectly into a buffer nobody displays.
pub struct Surface(Geom);

/// The surface most likely to be live: the owned scanout once the plane has been armed, otherwise
/// the firmware framebuffer. `None` before either has been registered.
pub fn live_surface() -> Option<Surface> {
    SCANOUT.get().or_else(|| FIRMWARE.get()).map(Surface)
}

impl Surface {
    pub fn size(&self) -> (usize, usize) {
        (self.0.w, self.0.h)
    }

    /// One pixel, bounds-checked, in the surface's own channel order.
    pub fn put(&self, x: usize, y: usize, c: (u8, u8, u8)) {
        unsafe { self.0.put(x, y, c) }
    }

    /// A solid rectangle, clipped to the surface.
    pub fn fill_rect(&self, x: usize, y: usize, w: usize, h: usize, c: (u8, u8, u8)) {
        let x1 = (x + w).min(self.0.w);
        let y1 = (y + h).min(self.0.h);
        for py in y..y1 {
            for px in x..x1 {
                unsafe { self.0.put(px, py, c) }
            }
        }
    }

    /// Clear the whole surface. Goes through `Geom::fill`, which walks a row pointer and writes one
    /// `u32` per pixel where it can — on the firmware framebuffer, which is uncached MMIO, that is
    /// a third of the bus traffic of three byte stores.
    pub fn clear(&self, c: (u8, u8, u8)) {
        unsafe { self.0.fill(0, self.0.h, c) }
    }
}

/// A cursor that lays text across every live target at once.
///
/// `bg` is the fill this text sits on. Glyph coverage is blended between `bg` and `fg` rather than
/// thresholded, because at 32px on a saturated background a hard 1-bit edge reads as a jagged mess
/// — and this is text someone is trying to copy down off a dead machine.
struct Screen {
    targets: [Option<Geom>; 2],
    x: usize,
    y: usize,
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
}

impl Screen {
    /// Every surface we know about. For a fatal report, where the machine is not coming back and
    /// guessing wrong means the message is lost.
    fn all(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> Screen {
        Screen { targets: [SCANOUT.get(), FIRMWARE.get()], x: MARGIN_X, y: 0, fg, bg }
    }

    /// Just the surface most likely to be live. Used by the non-fatal banner, which runs inside a
    /// fault handler on a system that keeps going: once the GPU driver has an owned scanout, the
    /// firmware framebuffer is dark, and painting it anyway would spend milliseconds of uncached
    /// MMIO traffic — with interrupts off — on pixels nobody can see. Every dying process would pay
    /// it. Falls back to the firmware buffer when there is no owned scanout, which is the case
    /// before the plane is armed and if P1a's plane scan failed.
    fn live(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> Screen {
        let t = SCANOUT.get().or_else(|| FIRMWARE.get());
        Screen { targets: [t, None], x: MARGIN_X, y: 0, fg, bg }
    }

    /// `bg` + (`fg` - `bg`) * cov/255, per channel. Integer only — there is no FPU state worth
    /// trusting in a fault handler, and `x86-interrupt` handlers do not save SSE registers.
    #[inline]
    fn blend(&self, cov: u8) -> (u8, u8, u8) {
        let c = cov as u32;
        let mix = |f: u8, b: u8| -> u8 {
            ((b as u32 * (255 - c) + f as u32 * c) / 255) as u8
        };
        (
            mix(self.fg.0, self.bg.0),
            mix(self.fg.1, self.bg.1),
            mix(self.fg.2, self.bg.2),
        )
    }

    fn newline(&mut self) {
        self.x = MARGIN_X;
        self.y += LINE_H;
    }

    fn glyph(&mut self, c: char) {
        if c == '\n' {
            self.newline();
            return;
        }
        if c == '\r' || c == '\t' {
            self.x += 8;
            return;
        }
        let r = match get_raster(c, FontWeight::Regular, FONT_H)
            .or_else(|| get_raster('?', FontWeight::Regular, FONT_H))
        {
            Some(r) => r,
            None => return,
        };
        let raster = r.raster();
        let adv = raster.first().map(|row| row.len()).unwrap_or(16);

        // Wrap before drawing, not after, so a long path never runs off the right edge — which is
        // exactly where the interesting half of a fault message lives.
        for t in self.targets.iter().flatten() {
            if self.x + adv >= t.w.saturating_sub(MARGIN_X) {
                self.x = MARGIN_X;
                self.y += LINE_H;
                break;
            }
        }

        for (row_i, row) in raster.iter().enumerate() {
            for (col_i, &cov) in row.iter().enumerate() {
                if cov == 0 {
                    continue;
                }
                let c = self.blend(cov);
                let px = self.x + col_i;
                let py = self.y + row_i;
                for t in self.targets.iter().flatten() {
                    unsafe { t.put(px, py, c) };
                }
            }
        }
        self.x += adv;
    }

    fn puts(&mut self, s: &str) {
        for c in s.chars() {
            self.glyph(c);
        }
    }
}

impl Write for Screen {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.puts(s);
        Ok(())
    }
}

/// Full-screen fatal report. The machine is not coming back, so the whole surface is ours.
///
/// `title` is a short banner ("KERNEL PANIC", "DOUBLE FAULT"); `body` is rendered underneath and is
/// normally the `PanicInfo` itself, which carries the message *and* the file:line.
pub fn fatal(title: &str, body: fmt::Arguments) {
    // ★ Claim the screen BEFORE painting a single pixel.
    //
    // The panicking core disables its own interrupts, but the OTHER cores keep running — including
    // whichever one is executing the shell. It carries on compositing and presenting frames into
    // the very buffer being painted red here, so the two fight: the report comes up torn, partly
    // overwritten, and visibly jittering as fresh desktop frames land on top of it.
    //
    // That is not cosmetic. It made a fault IP unreadable badly enough to be transcribed one digit
    // wrong, which sent a debugging session after a nonexistent mid-instruction fault. On a machine
    // with NO SERIAL CONSOLE the screen is the only channel, so a corrupted report is a corrupted
    // diagnosis.
    //
    // The present syscalls check this flag and return without touching the scanout, which stops the
    // overwrite within a frame. It does not stop the other cores themselves — the thorough fix is a
    // broadcast NMI with a handler that halts, which needs an IDT entry this kernel does not yet
    // have. Turning a red screen into a triple fault would be a poor trade, so: flag first.
    PANICKING.store(true, Ordering::SeqCst);

    // ★ PAINT THE WHOLE REPORT TWICE, with a pause between. The last painter wins.
    //
    // Setting the flag above stops the other cores from STARTING a present. It does nothing about
    // one already inside the syscall, past the check, holding a pointer into this buffer — and
    // nothing at all about a GPU batch that was submitted before the fault and is still retiring.
    // Either lands on top of the report, which is how the last red screen came back with dark runs
    // punched through the text.
    //
    // The thorough fix is a broadcast NMI whose handler halts the other cores, and that needs an IDT
    // entry this kernel does not have. This is the cheap ninety percent: by the second pass the flag
    // has been globally visible for millions of cycles, so no new present can have begun, and
    // whatever landed during the first pass gets overwritten. A report that has to be photographed
    // and transcribed is worth painting twice.
    for pass in 0..2 {
        let mut s = Screen::all(TEXT, FATAL_BG);
        for t in s.targets.iter().flatten() {
            unsafe { t.fill(0, t.h, FATAL_BG) };
        }
        s.y = 96;
        s.puts(title);
        s.newline();
        s.newline();
        let _ = s.write_fmt(body);
        report_trailer(&mut s);
        if pass == 0 {
            // Long enough for an in-flight present to finish and for the flag to be seen everywhere;
            // short enough that nobody watching thinks the machine hung before the screen appeared.
            for _ in 0..40_000_000u64 {
                core::hint::spin_loop();
            }
        }
    }
}

/// The part of a fatal report that is not the panic message: subsystem cursors that say what the
/// machine was doing. Its own function so `fatal` can paint the whole report twice — see there.
fn report_trailer(s: &mut Screen) {
    // ★ If an ACPI namespace walk was in progress, name the node it died on.
    //
    // This is the whole diagnostic for the walk bug. A kernel #GP inside ACPICA otherwise reports an
    // IP inside `nswalk.c` and nothing else — true, and useless, because the walker is fine on
    // thousands of nodes and dies on one. The cursor says WHICH one, and the four-character ACPI
    // name looks up directly in `nyx-recv/dsdt.dsl`.
    //
    // ⚠️ Printed AFTER the body, so a fault that has nothing to do with ACPI is unaffected, and only
    // if a walk is actually live — a stale cursor claiming credit for an unrelated panic would be
    // worse than saying nothing.
    if crate::c_stubs::walk_trace::ACTIVE.load(Ordering::Relaxed) {
        use core::sync::atomic::Ordering::Relaxed as R;
        let n = crate::c_stubs::walk_trace::name_chars();
        s.newline();
        s.newline();
        let _ = s.write_fmt(format_args!(
            // ⚠️ The raw name word is printed as well as the four characters. On the first boot with
            // this report the characters came back `????` — every byte non-printable, which is
            // already the answer — but "not printable" does not say whether it was zeroed, poisoned,
            // or a valid pointer read at the wrong offset. The hex does.
            "ACPI WALK DIED HERE\n  walker  {}\n  node    #{} at depth {}\n  name    {}{}{}{}  raw {:#010x}\n  node@   {:#x}\n  parent@ {:#x}",
            match crate::c_stubs::walk_trace::KIND.load(R) {
                1 => "AcpiNsWalkNamespace",
                2 => "stepper (AcpiGetNextObject)",
                _ => "unknown",
            },
            crate::c_stubs::walk_trace::INDEX.load(R),
            crate::c_stubs::walk_trace::LEVEL.load(R),
            n[0], n[1], n[2], n[3],
            crate::c_stubs::walk_trace::NAME.load(R),
            crate::c_stubs::walk_trace::NODE.load(R),
            crate::c_stubs::walk_trace::PARENT.load(R),
        ));
    }
}

/// Set once a fatal report owns the screen. Read by the present/composite syscalls.
///
/// `SeqCst` rather than `Relaxed`: this has to be visible to the other cores immediately, and it is
/// stored exactly once in the life of the machine, so there is nothing to optimise.
pub static PANICKING: AtomicBool = AtomicBool::new(false);

/// True once a fatal report has claimed the screen. Anything that paints the scanout outside the
/// panic path must check this and do nothing.
#[inline]
pub fn screen_is_claimed() -> bool {
    PANICKING.load(Ordering::SeqCst)
}

/// A band across the top of the live frame for a fault that killed one process and left the system
/// running. Deliberately NOT a full-screen fill: the desktop below is still valid, the compositor
/// is still presenting, and blanking it over one segfaulting app would turn a recoverable event
/// into a lost session. The band survives until the next present overwrites it, which is the right
/// lifetime — long enough to read, short enough not to be debris.
pub fn fault_banner(what: &str, body: fmt::Arguments) {
    let mut s = Screen::live(TEXT, FAULT_BG);
    // Two lines plus padding. Fixed height so the band is the same shape every time and the eye
    // learns where to look.
    let band = LINE_H * 2 + 28;
    for t in s.targets.iter().flatten() {
        unsafe { t.fill(0, band, FAULT_BG) };
    }
    s.y = 10;
    s.puts(what);
    s.newline();
    let _ = s.write_fmt(body);
}
