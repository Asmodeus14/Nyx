//! Cold start, stage 1 — the Nyx mark on black while the kernel comes up.
//!
//! ## What this replaces
//!
//! A wall of `[BOOT]` lines in yellow. That stream still exists and is still the real diagnostic
//! surface on a box with no serial console — it is one keypress away (see [`begin`]) and every line
//! still goes to the serial port unconditionally. What changes is the default: a machine that is
//! working normally shows the identity and a rule that advances on completed milestones, and says
//! nothing else.
//!
//! ## The three rendering worlds, and why only the first one is here
//!
//! The design crosses three:
//!
//! * **Stage 1** — kernel on the bootloader framebuffer. No GPU, no blitter, no TTF rasterizer,
//!   because the rasterizer is `libs/gui` and userspace does not exist yet. Two coverage bitmaps
//!   compiled into the kernel image, alpha-blended by hand; one 208x1 rule; one line of bitmap
//!   text. **That is this module.**
//! * **Stage 2** — the Intel driver is up, the wallpaper arrives behind the mark and the mark goes
//!   to full strength. Nothing moves and nothing is re-laid-out.
//! * **Stage 3** — the shell takes the screen and the mark travels to the corner, cross-fading into
//!   the Entity ring as it lands.
//!
//! Stages 2 and 3 are not implemented. Stage 1 stands on its own — it is the screen you actually
//! look at while the machine boots — and the seam it leaves is the one that already exists today.
//!
//! ## Why the mark starts at half strength
//!
//! Because the system is not ready, and reaching full strength is what says it is. That is also why
//! there is no percentage: the kernel does not know how long NVMe enumeration will take, so a
//! number would be fiction. The rule advances on discrete completed milestones, and if one stalls
//! the rule simply stops — which is exactly the signal you want.

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use noto_sans_mono_bitmap::{get_raster, get_raster_width, FontWeight, RasterHeight};

use crate::brand_gen::{self, Coverage};
use crate::panic_screen::{live_surface, Surface};

/// 8x16 per cell. The only type available at stage 1, and the reason the line below the rule is
/// monospace — a hardware fact, not a style choice.
const FONT_H: RasterHeight = RasterHeight::Size16;

// ── palette ─────────────────────────────────────────────────────────────────
// From the design's own stylesheet. Held here as literals rather than reaching into
// `nyx_meridian::tokens`, because that crate is userspace and the kernel does not link it — the two
// copies are three colours and a comment, which is cheaper than a dependency in the wrong direction.
const GROUND: (u8, u8, u8) = (0x00, 0x00, 0x00);
/// `--fg`. The mark and wordmark are drawn in this, scaled by `strength`.
const IDENTITY: (u8, u8, u8) = (0xF4, 0xF5, 0xF6);
/// `rgba(255,255,255,.13)` over black — the unfilled part of the rule.
const RULE_TRACK: (u8, u8, u8) = (0x21, 0x21, 0x21);
/// `--accent`.
const RULE_FILL: (u8, u8, u8) = (0x46, 0x76, 0xF0);
/// `.bt-l`, the status line.
const LABEL: (u8, u8, u8) = (0x56, 0x5B, 0x61);
/// The attention state, for the boot after a freeze. Amber rather than red: the machine recovered,
/// and the same amber names a ring-3 fault in `panic_screen`. Lifted well above that background
/// tone because this is ink on black rather than a fill behind white text.
const ATTENTION: (u8, u8, u8) = (0xE0, 0x99, 0x28);

// ── layout, in the design's own numbers ─────────────────────────────────────
// The scene in `11-boot.html` is 1440x900 and draws the mark at 76 and the word at 76x33. The
// COMPILED bitmaps are larger — 96x96 and 152x66, the sizes the design names for the kernel — and
// there is no scaler at stage 1, so they are drawn at their native size and the design's gaps are
// kept verbatim. On a 1080p panel a 96px mark occupies almost exactly the fraction of the screen
// that a 76px mark occupies of the design's 900.
const GAP_MARK_WORD: usize = 26;
const GAP_WORD_RULE: usize = 39;
const GAP_RULE_LABEL: usize = 27;
const RULE_W: usize = 208;

/// Where the block's CENTRE sits, as a fraction of screen height.
///
/// ★ Derived from the design, and derived carefully — the first version of this got it wrong. Most
/// scenes in the design document are 1440x900, but the cold-start scenes declare `data-h="520"`.
/// Against 900 the block lands at 14% of the screen and sits absurdly high; against its real 520 it
/// runs 126..342, whose centre is 234 — **45% of the scene**. Very slightly above the middle, which
/// is the intent, and nowhere near the top.
///
/// Expressed as the centre rather than the top on purpose: the compiled assets are larger than the
/// ones the mockup draws, so anchoring the top would push everything below it down by the
/// difference. Anchoring the centre reproduces the design's placement whatever the block's height.
const CENTRE_NUM: usize = 45;
const CENTRE_DEN: usize = 100;

/// Out of 255. Half strength while the kernel is still coming up; full once the GPU's own scanout
/// is armed. The mark reaching full strength is the design's signal that the system is ready, and
/// it carries more information than a percentage would.
const STRENGTH_BOOTING: u8 = 128;
const STRENGTH_READY: u8 = 255;

/// The milestones the rule advances on. Seven, matching the design.
///
/// Deliberately coarse and deliberately *completions*: each one is written after the thing it names
/// has finished, so the rule can only ever under-report progress. A milestone marked on entry would
/// let the rule reach the end and then hang, which is precisely the case the rule exists to make
/// visible.
/// ★ The ORDER here is the order they happen in, and that is the whole point. The first version
/// numbered them by subsystem rather than by when they complete, which put the rule at 3/7 — just
/// under halfway — for the entire NVMe-plus-mount-plus-unpack stretch, which is most of the boot.
/// It looked exactly like a machine that had stalled and then teleported to the desktop.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Milestone {
    Memory = 1,
    Acpi = 2,
    Pci = 3,
    /// NVMe controller up and its queues built.
    Storage = 4,
    /// `/mnt/nvme` mounted — marked before the initrd is unpacked, not after.
    Filesystem = 5,
    /// The initrd is on disk.
    Applications = 6,
    /// The render engine answered, init is loaded, and the screen is about to change hands.
    Userspace = 7,
}

const MILESTONES: u8 = 7;

/// Still painting. Cleared at the handoff to userspace, and when the user asks for the log.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// The `[BOOT]` stream is serial-only.
///
/// ★ Separate from [`ACTIVE`], and it stays set after the handoff. The kernel does not go quiet the
/// moment it enters userspace — drivers, the scheduler and the syscall path all keep logging, and
/// init takes a moment to fork the window server. Tying this to `ACTIVE` meant every one of those
/// lines painted itself in yellow over the identity during exactly the window where the screen is
/// supposed to be holding still.
static QUIET: AtomicBool = AtomicBool::new(false);

/// Someone pressed a key. Acted on at the next milestone rather than here, because this is set from
/// inside the keyboard IRQ and tearing the screen down means a full-screen fill — tens of
/// milliseconds of uncached MMIO with interrupts off is not something to do in an interrupt.
static WANT_LOG: AtomicBool = AtomicBool::new(false);
/// Highest milestone reached. Monotonic — a caller cannot walk the rule backwards.
static REACHED: AtomicU8 = AtomicU8::new(0);
/// What the post-mortem found, so a repaint at stage 2 keeps saying it. Written once during
/// single-threaded early boot; a `Mutex` only because `&'static str` will not fit in an atomic.
static DEATH: spin::Mutex<Option<crate::postmortem::Death>> = spin::Mutex::new(None);

/// Should the `[BOOT]` stream stay off the screen?
///
/// `vga_log` asks. Once the graphical screen has been shown this stays true for the rest of the
/// run: the two draw to the same pixels, and the log winning that race is what put yellow text over
/// the mark. Serial keeps every line either way.
pub fn is_quiet() -> bool {
    QUIET.load(Ordering::Relaxed)
}

/// A key arrived. Called from the keyboard IRQ.
///
/// This is what makes "hold any key during stage 1" actually work. Polling the 8042 once in
/// [`begin`] cannot: at that point the controller has not been initialised, the firmware has
/// usually drained it, and once the IRQ is routed the handler consumes every byte before anything
/// else can see it. The handler is the only place that reliably observes a keystroke, so it is the
/// place that has to report one.
pub fn key_pressed() {
    if ACTIVE.load(Ordering::Relaxed) {
        WANT_LOG.store(true, Ordering::Relaxed);
        return;
    }
    // ★ After the handoff there are no more milestones to consume the flag, so act now — two atomic
    // stores, nothing that could not run in an interrupt.
    //
    // This is the escape hatch for the case that matters most: the kernel finished, the mark is on
    // screen, and the window server never appeared. Before the cold-start screen existed the boot
    // log was simply still there and told you why. Latching `QUIET` past the handoff took that
    // away, and "beautiful screen, no information" is the worst possible failure mode on a machine
    // whose usual symptom is already "it just froze".
    //
    // Harmless once the desktop is up: the kernel rarely logs in steady state, and the shell
    // repaints everything on its 1 Hz heartbeat, so a stray line is gone within a second.
    QUIET.store(false, Ordering::Relaxed);
}

/// Tear the screen down and give the log the framebuffer back.
fn abandon() {
    ACTIVE.store(false, Ordering::Relaxed);
    QUIET.store(false, Ordering::Relaxed);
    if let Some(s) = live_surface() {
        s.clear(GROUND);
    }
    // Start the log at the top of a clean screen rather than wherever it had got to before the
    // graphical screen took over.
    crate::vga_log::reset_cursor();
    crate::serial_println!("[BOOT] key pressed — showing the boot log");
}

/// Paint stage 1. Call once, as soon as a framebuffer has been registered.
///
/// `died` is what the CMOS post-mortem found, or `None` on a clean boot. When it is `Some`, the
/// mark comes up in its attention state and one line names what died — the only circumstance in
/// which this screen says anything at all.
///
/// Holding any key opts out entirely and leaves the `[BOOT]` log on screen, which is not a designed
/// surface and is not meant to be: it is the existing stream, and a workstation OS that hides its
/// boot log from the person who compiled it has the priorities backwards.
pub fn begin(died: Option<crate::postmortem::Death>) {
    if key_is_held() {
        crate::serial_println!("[BOOT] key held — graphical cold start suppressed, showing the log");
        return;
    }
    let Some(s) = live_surface() else {
        crate::serial_println!("[BOOT] no framebuffer registered; cold-start screen skipped");
        return;
    };
    *DEATH.lock() = died;
    ACTIVE.store(true, Ordering::Relaxed);
    QUIET.store(true, Ordering::Relaxed);
    s.clear(GROUND);
    repaint(&s, STRENGTH_BOOTING);
}

/// Repaint onto whatever surface is live now, at full strength — cold start, stage 2.
///
/// Called when the display plane is re-pointed at the GPU driver's own buffer. That buffer is
/// freshly zeroed, so without this the identity vanishes mid-boot and the rule goes on advancing
/// across a black screen. The strength change is not a workaround for that; it is the design's own
/// signal that the machine has stopped being a bootloader.
///
/// No-op when the screen was never up, so the driver calls it unconditionally.
pub fn rearm() {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    let Some(s) = live_surface() else { return };
    s.clear(GROUND);
    repaint(&s, STRENGTH_READY);
}

/// Record a completed milestone and redraw the rule.
///
/// Cheap enough to call from anywhere in boot: it repaints 208 pixels and nothing else. Ignored
/// when the screen is not active, so every call site is unconditional.
pub fn milestone(m: Milestone) {
    if !ACTIVE.load(Ordering::Relaxed) {
        return;
    }
    // The keyboard IRQ only sets a flag; the teardown happens here, out of interrupt context.
    if WANT_LOG.load(Ordering::Relaxed) {
        abandon();
        return;
    }
    let n = m as u8;
    // Monotonic. Boot order is not perfectly linear — a degraded path can skip ACPI and still reach
    // PCI — and a rule that jumps backwards reads as a fault rather than as a skipped step.
    if REACHED.fetch_max(n, Ordering::Relaxed) >= n {
        return;
    }
    if let Some(s) = live_surface() {
        let (w, h) = s.size();
        let l = Layout::for_screen(w, h);
        draw_rule(&s, &l, n);
    }
}

/// Where the mark was last painted, packed `x<<48 | y<<32 | w<<16 | px`, or 0 if it was never drawn.
///
/// Read once by the shell, through a syscall, so that cold-start stage 3 can pick the mark up from
/// exactly where the kernel left it. Handing over the coordinate rather than having both sides
/// compute it from the design is the whole point: two independent derivations of "where is the
/// mark" would agree today and drift the first time either layout is touched, and the symptom would
/// be a jump on the first frame of the desktop — the single frame this animation exists to remove.
static MARK_RECT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// The last painted mark rect, for the syscall. Zero means there is no handoff to make.
pub fn mark_rect() -> u64 {
    MARK_RECT.load(Ordering::Relaxed)
}

/// Stop painting. Called at the handoff to userspace.
///
/// Does not clear: whatever comes next paints the whole screen anyway, and a black flash between
/// the mark and the desktop is exactly the "splash torn down, shell put up" seam the design exists
/// to avoid.
///
/// ★ Does not lift [`QUIET`] either. The kernel keeps logging after this point — drivers, the
/// scheduler, the syscall path — and init needs a moment to fork the window server. Restoring the
/// on-screen log here painted yellow text over the identity for exactly that window.
pub fn end() {
    ACTIVE.store(false, Ordering::Relaxed);
}

// ── layout ──────────────────────────────────────────────────────────────────

struct Layout {
    mark_x: usize,
    mark_y: usize,
    word_x: usize,
    word_y: usize,
    rule_x: usize,
    rule_y: usize,
    label_y: usize,
}

impl Layout {
    fn for_screen(w: usize, h: usize) -> Layout {
        let mark = &brand_gen::MARK;
        let word = &brand_gen::WORDMARK;
        // Mark, gap, wordmark, gap, rule, gap, one line of type.
        let block_h = mark.box_h
            + GAP_MARK_WORD
            + word.box_h
            + GAP_WORD_RULE
            + 1
            + GAP_RULE_LABEL
            + FONT_H.val();
        let top = (h * CENTRE_NUM / CENTRE_DEN).saturating_sub(block_h / 2);
        let mark_y = top;
        let word_y = mark_y + mark.box_h + GAP_MARK_WORD;
        let rule_y = word_y + word.box_h + GAP_WORD_RULE;
        let label_y = rule_y + 1 + GAP_RULE_LABEL;
        Layout {
            // Centre on the design's BOX, not on the cropped ink — the two assets were trimmed by
            // different margins, and centring on the ink would leave the word visibly off-axis from
            // the mark above it.
            mark_x: w.saturating_sub(mark.box_w) / 2,
            mark_y,
            word_x: w.saturating_sub(word.box_w) / 2,
            word_y,
            rule_x: w.saturating_sub(RULE_W) / 2,
            rule_y,
            label_y,
        }
    }
}

// ── painting ────────────────────────────────────────────────────────────────

fn repaint(s: &Surface, strength: u8) {
    let (w, h) = s.size();
    let l = Layout::for_screen(w, h);
    let died = *DEATH.lock();
    // Only the MARK takes the attention colour. The wordmark stays neutral: the mark is the thing
    // that also becomes the Entity, and the Entity is what reports on the machine — colouring the
    // logotype would be the identity claiming a status it does not hold.
    let ink = if died.is_some() { ATTENTION } else { IDENTITY };

    MARK_RECT.store(
        ((l.mark_x as u64) << 48)
            | ((l.mark_y as u64) << 32)
            | ((brand_gen::MARK.box_w as u64) << 16)
            | brand_gen::MARK.box_h as u64,
        Ordering::Relaxed,
    );
    blit(s, &brand_gen::MARK, l.mark_x, l.mark_y, ink, strength);
    blit(s, &brand_gen::WORDMARK, l.word_x, l.word_y, IDENTITY, strength);
    draw_rule(s, &l, REACHED.load(Ordering::Relaxed));

    match died {
        // The one line worth designing. On a machine with no serial port the CMOS record is the
        // only evidence that survives a freeze, and this screen no longer stays silent about it.
        Some(d) => {
            centred(s, l.label_y, "PREVIOUS RUN DID NOT SHUT DOWN CLEANLY", ATTENTION);
            centred(s, l.label_y + 20, d.why, LABEL);
            centred(s, l.label_y + 38, d.stage, LABEL);
        }
        None => centred(s, l.label_y, "Starting Nyx OS", LABEL),
    }
}

fn draw_rule(s: &Surface, l: &Layout, reached: u8) {
    s.fill_rect(l.rule_x, l.rule_y, RULE_W, 1, RULE_TRACK);
    let filled = RULE_W * reached.min(MILESTONES) as usize / MILESTONES as usize;
    if filled > 0 {
        s.fill_rect(l.rule_x, l.rule_y, filled, 1, RULE_FILL);
    }
}

/// Alpha-blend a coverage bitmap onto the surface.
///
/// The ground is known black, so this multiplies rather than reading the destination back. That is
/// not a micro-optimisation: at stage 1 the target is usually the firmware framebuffer, which is
/// uncached MMIO, and a read-modify-write there costs a bus round trip per pixel.
fn blit(s: &Surface, c: &Coverage, box_x: usize, box_y: usize, ink: (u8, u8, u8), strength: u8) {
    let x0 = box_x + c.off_x;
    let y0 = box_y + c.off_y;
    for y in 0..c.h {
        for x in 0..c.w {
            let a = c.cov[y * c.w + x] as u32 * strength as u32 / 255;
            if a == 0 {
                continue;
            }
            let m = |v: u8| ((v as u32 * a) / 255) as u8;
            s.put(x0 + x, y0 + y, (m(ink.0), m(ink.1), m(ink.2)));
        }
    }
}

/// One line of 8x16 monospace, centred on the screen.
fn centred(s: &Surface, y: usize, text: &str, colour: (u8, u8, u8)) {
    let adv = get_raster_width(FontWeight::Regular, FONT_H);
    let w = text.chars().count() * adv;
    let mut x = s.size().0.saturating_sub(w) / 2;
    for ch in text.chars() {
        let g = match get_raster(ch, FontWeight::Regular, FONT_H) {
            Some(g) => g,
            None => {
                x += adv;
                continue;
            }
        };
        for (ry, row) in g.raster().iter().enumerate() {
            for (rx, &cov) in row.iter().enumerate() {
                if cov == 0 {
                    continue;
                }
                let m = |v: u8| ((v as u32 * cov as u32) / 255) as u8;
                s.put(x + rx, y + ry, (m(colour.0), m(colour.1), m(colour.2)));
            }
        }
        x += adv;
    }
}

// ── the opt-out ─────────────────────────────────────────────────────────────

/// Is a key being held right now?
///
/// Polled directly off the PS/2 controller, because this runs before the keyboard driver, before
/// the IDT, and before interrupts are enabled — there is no other way to ask. Status bit 0 is
/// "output buffer full", bit 5 distinguishes a byte from the AUX (mouse) port, and a make code is
/// below 0x80. All three have to hold, so a stale mouse packet or a key *release* left in the
/// buffer by the firmware cannot be mistaken for someone holding a key down.
fn key_is_held() -> bool {
    use x86_64::instructions::port::Port;
    let mut status: Port<u8> = Port::new(0x64);
    let mut data: Port<u8> = Port::new(0x60);
    unsafe {
        let st = status.read();
        if st & 0x01 == 0 || st & 0x20 != 0 {
            return false;
        }
        let code = data.read();
        (0x01..0x80).contains(&code)
    }
}
