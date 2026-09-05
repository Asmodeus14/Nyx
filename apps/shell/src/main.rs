#![no_std]
#![no_main]
// Same rustc workaround as every other app here: the pinned nightly's diagnostic renderer ICEs while
// drawing a warning with a source snippet, which aborts an otherwise valid build.
#![allow(warnings)]

//! # `shell` — the Meridian window server
//!
//! The replacement for `apps/compositor`, drawing `Nyx-ui/design/ver3.0/`. It is a *shell* rather
//! than a restyle: there is no panel, no menu bar, no system tray and no taskbar. The permanent
//! chrome is eight glyphs resting on the wallpaper at bottom-left and one 22px mark at bottom-right.
//!
//! Three things follow from that and shape this file:
//!
//! - **A window has no chrome.** Its name lives on a 22px caption line *above* the surface, and the
//!   surface is exactly the app's pixels with a hairline and a 6px radius. So `Win.r` is the
//!   SURFACE rect, and the caption is derived from it by `layout::caption` — never stored, never a
//!   second source of truth.
//! - **Minimise is replaced by FOLD.** A folded window collapses to its caption line and stays where
//!   it was. That is what makes a taskbar unnecessary: a folded window is still somewhere.
//! - **The dock is the launcher** until the Command lands. Eight glyphs on the wallpaper, no
//!   container.
//!
//! ## What was kept from the compositor, verbatim and on purpose
//!
//! The plumbing, all of which is correct and was expensive to get right: the IPC protocol, the SHM
//! resize lifecycle and its `MSG_SHM_RELEASED` handshake, the GGTT remap onto a stable per-window
//! GVA, the GPU composite z-order, the hardware cursor, and — importantly — the full-vs-partial
//! frame decision. That last one has a landmine in it: a "repaint only the chrome strip" idle
//! optimisation was tried once and reverted, because letting the render engine go idle into RC6
//! loses MOCS and the next composite hangs. A full GPU frame every second is what keeps it warm.
//!
//! ## Verification
//!
//! There is no QEMU here, so every on-hardware run costs a power cycle. Everything provable off the
//! machine lives in `libs/meridian` and is covered by `cargo test -p nyx-meridian`: atlas packing,
//! glyph metrics, icon coverage, corner antialiasing, the shadow, and every hit-test rect in this
//! file. What remains for hardware is the set of questions only hardware can answer — does the atlas
//! map, does the wallpaper quad stretch, does it come up at all.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::canvas::{Canvas, Color};
use nyx_gui::ui::{draw_scrollbar, SCROLLBAR_W};

use nyx_meridian::atlas::Atlas;
use nyx_meridian::icons::Icons;
use nyx_meridian::layout::{self, CmdEntry, Control, Rect};
use nyx_meridian::tokens::{self, Style, Theme};
use nyx_meridian::cursor::{self, Shape};
use nyx_meridian::brand;
use nyx_meridian::motion::{Motion, Transition, UNIT, WINDOW_OPEN_SCALE};
use nyx_meridian::{font, icons, paper, scale, shapes};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// ─────────────────────────── Applications ───────────────────────────

/// One installed application: a glyph from the Meridian icon set, a name, and its bundle.
struct App {
    icon: &'static str,
    label: &'static str,
    /// The bundle DIRECTORY, with its trailing slash. The executable is `run.bin` inside it and the
    /// icon is `icon.png` inside it, so this one string is both the launch path and the key that
    /// matches a live window back to its app (a client reports its icon path, and the two share this
    /// prefix). Storing the exec path separately is how those two drift apart.
    bundle: &'static str,
}

/// Every launchable application, as ONE table.
///
/// The dock indexes into it and the Command searches it, so adding an app is one row here — the
/// successor to the old compositor's `MENU`, which had already been collapsed from three parallel
/// lists after adding an app to only two of them silently launched its neighbour.
///
/// ★ On icons. Meridian's set was drawn for the design's eight-app dock, not for this machine's
/// thirteen, so some assignments are approximations chosen once and written down rather than
/// re-argued at each call site: `u-grid` for a picture gallery, `u-check` for a smoke test,
/// `u-shield` for a conformance harness, `u-branch` for a toolchain artefact. `a-browser` — a globe,
/// drawn for a browser that has since been deleted — goes to GL Cube, which renders a rotating solid
/// and is the only thing left in the set that is a sphere.
const APPS: [App; 13] = [
    App { icon: Icons::APP_FILES,    label: "Files",          bundle: "/mnt/nvme/apps/Explorer.nyx/" },
    App { icon: Icons::APP_TERMINAL, label: "Terminal",       bundle: "/mnt/nvme/apps/Terminal.nyx/" },
    App { icon: Icons::APP_QCLANG,   label: "QC Studio",      bundle: "/mnt/nvme/apps/QcStudio.nyx/" },
    App { icon: Icons::DOC,          label: "Notepad",        bundle: "/mnt/nvme/apps/Notepad.nyx/" },
    App { icon: Icons::APP_MONITOR,  label: "System Monitor", bundle: "/mnt/nvme/apps/SystemMonitor.nyx/" },
    App { icon: Icons::WIFI,         label: "Wi-Fi",          bundle: "/mnt/nvme/apps/Wifi.nyx/" },
    App { icon: Icons::GRID,         label: "Image Viewer",   bundle: "/mnt/nvme/apps/ImageViewer.nyx/" },
    App { icon: Icons::APP_SETTINGS, label: "Settings",       bundle: "/mnt/nvme/apps/Settings.nyx/" },
    App { icon: Icons::APP_BROWSER,  label: "GL Cube",        bundle: "/mnt/nvme/apps/GlCube.nyx/" },
    App { icon: Icons::APP_CODE,     label: "Std GUI",        bundle: "/mnt/nvme/apps/StdGui.nyx/" },
    App { icon: Icons::CHECK,        label: "Std Hello",      bundle: "/mnt/nvme/apps/StdHello.nyx/" },
    App { icon: Icons::SHIELD,       label: "POSIX Probe",    bundle: "/mnt/nvme/apps/PosixProbe.nyx/" },
    App { icon: Icons::BRANCH,       label: "Hello C (musl)", bundle: "/mnt/nvme/apps/HelloC.nyx/" },
];

/// The dock, left to right, as indices into [`APPS`]. `None` at index 7 is the divider — a hairline,
/// not a glyph.
///
/// ★ This deliberately differs from `Icons::DOCK`, which is the design document's dock. Two of the
/// design's eight slots have no application on this system: `a-browser` (the graphical browser was
/// deleted — browsing is `get`/`links`/`open` in the Terminal now) and `a-entity` (`libs/entity` is
/// step 17 of the build order and does not exist). A dock glyph whose exec path is not on the image
/// launches nothing and reads as a hang, which is exactly why those rows were cut from the old start
/// menu too. So the count the design's argument rests on — eight glyphs and one mark — is kept, with
/// the eight applications this machine actually has.
const DOCK: [Option<usize>; layout::DOCK_SLOTS] =
    [Some(0), Some(1), Some(2), Some(3), Some(4), Some(5), Some(6), None, Some(7)];

/// Fork+exec `<bundle>run.bin`.
///
/// Bracketed with serial breadcrumbs on BOTH sides of the fork, each naming the app. These are
/// `write(2)` calls, so each one proves the process is alive and can still enter the kernel; a
/// string in `.rodata` proves neither. `parent alive` is the load-bearing line — the shell IS the
/// desktop, so the shell going quiet after a dock click is indistinguishable from a whole-machine
/// freeze from the outside, and this is what tells the two apart.
fn launch(app: &App) {
    let mut buf = [0u8; 128];
    let mut n = 0usize;
    for &b in app.bundle.as_bytes() {
        if n < buf.len() - 8 {
            buf[n] = b;
            n += 1;
        }
    }
    for &b in b"run.bin\0" {
        buf[n] = b;
        n += 1;
    }
    let exec = match core::str::from_utf8(&buf[..n]) {
        Ok(s) => s,
        Err(_) => return,
    };
    breadcrumb("[LAUNCH] pre-fork: ", app.label);
    if sys_fork() == 0 {
        breadcrumb("[LAUNCH] child: ", app.label);
        sys_execve(exec);
        breadcrumb("[LAUNCH] execve returned, exec FAILED: ", app.label);
        sys_exit(1);
    }
    breadcrumb("[LAUNCH] parent alive: ", app.label);
}

/// Case-insensitive substring match. Labels and queries are both ASCII, so this compares bytes.
fn matches(label: &str, q: &[u8]) -> bool {
    if q.is_empty() {
        return true;
    }
    let l = label.as_bytes();
    if q.len() > l.len() {
        return false;
    }
    'outer: for start in 0..=(l.len() - q.len()) {
        for i in 0..q.len() {
            if !l[start + i].eq_ignore_ascii_case(&q[i]) {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

/// `prefix` + `what` + newline as ONE `write(2)`. Three separate prints would interleave with
/// whatever else is on the serial line, which defeats the purpose of a breadcrumb.
fn breadcrumb(prefix: &str, what: &str) {
    let mut m = [0u8; 96];
    let mut p = 0usize;
    for s in [prefix, what, "\n"] {
        for &b in s.as_bytes() {
            if p < m.len() {
                m[p] = b;
                p += 1;
            }
        }
    }
    if let Ok(s) = core::str::from_utf8(&m[..p]) {
        sys_print(s);
    }
}

// ─────────────────────────────── Cursor ───────────────────────────────
// The Meridian set: twelve shapes, composited from the design document at build time and living in
// `nyx_meridian::cursor`. Nothing here draws anything — the shell's whole job is deciding WHICH
// shape the pointer is over, which is the part that cannot be precomputed.
//
// This is the cheapest quality win in the design: the cursor plane is composited by the scanout
// engine after everything else, so a shape change costs one 16 KB copy and a move costs one register
// write from the mouse IRQ. There is no per-frame cost at all.


// ────────────────────────────────── The OSD ──────────────────────────────────
// A brightness or volume change does not need to interrupt the middle of the screen. It needs to
// appear where system state always appears — so the popup grows out of the Entity mark, in the same
// corner, and says its piece in the System Monitor's vocabulary at a quarter of the size.
//
// It does not dim or blur the backdrop. That would spend a full-screen pass to announce a volume
// change; this is one damage rectangle of about 21,000 pixels, held for 1.2 s after the last key.

/// How long the OSD stays up after the last keypress.
const OSD_HOLD_MS: usize = 1200;

/// The three variants. Only ever one at a time.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Osd {
    /// Real today: one PCH PWM register write.
    Brightness(u32),
    /// This display has no PWM-driven backlight — an external panel, or firmware driving it some
    /// other way. The design is explicit that the honest OSD says so rather than moving a slider
    /// that does nothing.
    BrightnessUnavailable,
    /// Designed, and gated on an audio driver that does not exist: no HDA controller, no codec
    /// enumeration, no mixer, no stream. Drawing the panel is what makes the size of the missing
    /// piece obvious — everything on screen is ready and the whole subsystem underneath is absent.
    VolumeUnavailable,
    /// Asking which F-keys carry the brightness symbols. Held open until answered — see `tick_osd`.
    Learn(Learn),
    /// What the binding ended up as, briefly, so a rebind confirms itself.
    Bound(BrightKeys),
    /// A key arrived during capture that is not an F-key, echoed back.
    ///
    /// This exists because "I pressed the key and nothing happened" is the single least useful bug
    /// report a serial-less machine can produce, and it has two completely different causes: the
    /// keystroke never reached the OS, or it reached it and was ignored. Echoing whatever *did*
    /// arrive separates them in one press.
    Saw(char),
}

impl Osd {
    fn label(&self) -> &'static str {
        match self {
            Osd::Brightness(_) | Osd::BrightnessUnavailable => "BRIGHTNESS",
            Osd::VolumeUnavailable => "VOLUME",
            Osd::Learn(_) | Osd::Bound(_) | Osd::Saw(_) => "BRIGHTNESS KEYS",
        }
    }
    /// The large numeral, or the em dash that stands in for one when there is nothing to report.
    fn value(&self) -> String {
        match self {
            Osd::Brightness(p) => alloc::format!("{}%", p),
            // Echo each key back as it is captured, so the second press is made against visible
            // evidence that the first one registered.
            Osd::Learn(Learn::Brighter(d)) => alloc::format!("F{}", d),
            Osd::Bound(b) => b.describe(),
            // Printable ASCII as itself; anything else — and every key we care about here is
            // "anything else" — as its codepoint, because that is the number that identifies it.
            Osd::Saw(c) => {
                let c = *c;
                if (' '..='~').contains(&c) {
                    alloc::format!("'{}'", c)
                } else {
                    alloc::format!("U+{:04X}", c as u32)
                }
            }
            _ => String::from("\u{2014}"),
        }
    }
    fn detail(&self) -> Option<&'static str> {
        match self {
            Osd::Brightness(_) => None,
            Osd::BrightnessUnavailable => Some("No panel backlight"),
            Osd::VolumeUnavailable => Some("No audio device"),
            // Ask for it both ways rather than asserting which one works.
            //
            // The bare F-key is what arrives on a laptop whose function row sends F-keys by default.
            // But many laptops ship in "action keys" mode, where the row sends the media action
            // instead and `Fn` is what produces a real F-key — exactly inverted. Which one this
            // machine does is a firmware setting no table here can read, so the prompt covers both
            // and the echo below reports what actually turned up.
            Osd::Learn(Learn::Dimmer) => Some("Press your dimmer key — try it with and without Fn"),
            Osd::Learn(Learn::Brighter(_)) => Some("Now the brighter key"),
            Osd::Bound(_) => Some("Saved"),
            Osd::Saw(_) => Some("That key arrived, but it is not F1-F12"),
        }
    }
    fn percent(&self) -> Option<i32> {
        match self {
            Osd::Brightness(p) => Some(*p as i32),
            _ => None,
        }
    }
}

// ─────────────────────── Which key is the brightness key ───────────────────────
//
// Nothing in this machine can answer that. Which F-key carries the sun symbol is a KEYCAP — SMBIOS
// reports the manufacturer and the model, not the legend silkscreened on F5, and there is no table
// anywhere in firmware that does.
//
// Linux never has to ask. There the brightness key arrives as its own event, `KEY_BRIGHTNESSUP`,
// synthesised by AML running on the ACPI video device — the key is never an F-key at that level at
// all. Nyx cannot follow: `nyx-kernel/src/acpi.rs` is 189 lines of table walking with no AML
// interpreter, so `_BCM` and the notify handlers behind those events are simply unreachable code we
// do not have.
//
// What IS reachable: the physical brightness keys are `Fn` chords, the embedded controller eats the
// chord before the 8042 sees it, and the *bare* F-key underneath arrives perfectly normally. So the
// shell binds a pair of F-keys and synthesises the brightness step itself.
//
// The binding is therefore REMEMBERED, not detected. Guessing it from the SMBIOS vendor was the
// obvious shortcut and is the wrong shape: it is right for the two vendors whose layout you happen
// to know and quietly wrong everywhere else, which is exactly the reasoning `smbios.rs` opens by
// arguing against. Ask once, write it down, never ask again.

/// Where the shell keeps what the user has told it. On the persistent ext4 mount, flat in the root
/// of it — the same convention `libs/entity` uses for `/mnt/nvme/entity.bin`, and it avoids needing
/// to create a directory that may not exist.
const CONFIG_PATH: &str = "/mnt/nvme/meridian.conf";
const BRIGHT_KEY: &str = "brightness-keys";

/// How far one keypress moves the panel. Matches `backlight::step`'s own 5% grid, so a press always
/// lands on a round number and holding the key walks 5, 10, 15 rather than drifting off the grid.
const BRIGHT_STEP: i32 = 5;

/// The F-key pair bound to panel brightness, as 1-based F-key numbers.
#[derive(Clone, Copy, PartialEq, Eq)]
struct BrightKeys {
    down: u8,
    up: u8,
}

impl BrightKeys {
    /// The starting pair, offered until the user says otherwise.
    ///
    /// F5/F6 is a *default*, not a detection, and the distinction matters enough to keep out of the
    /// name: it is where a good few laptops put brightness and it will be wrong on plenty of others.
    /// Being wrong here costs a keypress that does nothing, and the fix is `Brightness keys` in the
    /// Command.
    const DEFAULT: BrightKeys = BrightKeys { down: 5, up: 6 };

    /// The step this key asks for, or `None` if it is not one of the two.
    fn step_for(&self, c: char) -> Option<i32> {
        let n = keys::fkey(c)?;
        if n == self.down {
            Some(-BRIGHT_STEP)
        } else if n == self.up {
            Some(BRIGHT_STEP)
        } else {
            None
        }
    }

    fn describe(&self) -> String {
        alloc::format!("F{} / F{}", self.down, self.up)
    }

    /// Read the binding back from disk, falling back to [`Self::DEFAULT`] for anything missing,
    /// unparseable, or out of range — including "there is no disk", which is an ordinary state here
    /// and not an error worth reporting.
    fn load() -> BrightKeys {
        let text = match read_config() {
            Some(t) => t,
            None => return Self::DEFAULT,
        };
        for line in text.lines() {
            let Some(v) = setting_value(line, BRIGHT_KEY) else { continue };
            let mut it = v.split_whitespace().filter_map(|w| w.parse::<u8>().ok());
            if let (Some(d), Some(u)) = (it.next(), it.next()) {
                if (1..=12).contains(&d) && (1..=12).contains(&u) && d != u {
                    return BrightKeys { down: d, up: u };
                }
            }
        }
        Self::DEFAULT
    }

    /// Write the binding back, preserving every other line of the file.
    ///
    /// Rewriting the file from just the settings this build knows about would silently drop anything
    /// a later build had added — a config file that eats settings is worse than no config file.
    fn save(&self) {
        let line = alloc::format!("{} = {} {}", BRIGHT_KEY, self.down, self.up);
        let mut out = String::new();
        let mut replaced = false;
        if let Some(existing) = read_config() {
            for l in existing.lines() {
                if setting_value(l, BRIGHT_KEY).is_some() {
                    if !replaced {
                        out.push_str(&line);
                        out.push('\n');
                        replaced = true;
                    }
                } else {
                    out.push_str(l);
                    out.push('\n');
                }
            }
        }
        if !replaced {
            out.push_str(&line);
            out.push('\n');
        }
        write_config(&out);
    }
}

/// `key = value` for one line, or `None` if this line is a comment, blank, or some other setting.
fn setting_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (k, v) = line.split_once('=')?;
    if k.trim() == key {
        Some(v.trim())
    } else {
        None
    }
}

/// The whole config file, or `None` when there is no persistent filesystem to read it from.
fn read_config() -> Option<String> {
    let fd = sys_open(CONFIG_PATH);
    if fd < 0 {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut buf = [0u8; 512];
    loop {
        let n = sys_read(fd, &mut buf);
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&buf[..n as usize]);
        // A config this size is a corrupt file, not a config. Stop rather than grow without bound.
        if out.len() >= 8192 {
            break;
        }
    }
    sys_close(fd);
    String::from_utf8(out).ok()
}

/// Best-effort write. There may be no disk mounted at all — this box boots fine without one — so a
/// failure here means the setting lasts until reboot, which is a fair outcome and not worth an
/// error surface of its own.
fn write_config(text: &str) {
    let fd = sys_open_flags(CONFIG_PATH, O_CREAT | O_TRUNC);
    if fd < 0 {
        return;
    }
    sys_write(fd, text.as_bytes());
    sys_close(fd);
}

/// How many control rows the Entity surface has. ONE source of truth: the geometry is sized from it
/// and the rows are drawn from it, and those two disagreeing is how a hit-test lands on the row above
/// the one you clicked.
const ENT_CONTROLS: usize = 4;

/// Which of those rows is Brightness. It is LAST so the three the design specifies keep the indices
/// they already had — `on_click` matches Network on row 0.
const ENT_BRIGHT_ROW: usize = 3;

/// Waiting for the user to press the key they want. Two presses, dimmer first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Learn {
    /// Nothing captured yet.
    Dimmer,
    /// The dimmer key is bound; waiting for the brighter one.
    Brighter(u8),
}


/// The system popup: label, numeral, hairline track. Grows out of the Entity mark.
fn osd_paint(
    state: &Shell,
    canvas: &mut Canvas,
    theme: Theme,
    atlas: Option<&Atlas>,
    labels: &mut Vec<Label>,
) {
    let Some((o, _)) = state.osd else { return };
    let p = layout::osd_panel(state.screen_w, state.screen_h);

    shapes::drop_shadow(canvas, p.x, p.y, p.w, p.h, 8, &[]);
    shapes::fill_round_rect(canvas, p.x.max(0) as usize, p.y.max(0) as usize,
                            p.w as usize, p.h as usize, 8, theme.chrome);
    shapes::stroke_round_rect(canvas, p.x.max(0) as usize, p.y.max(0) as usize,
                              p.w as usize, p.h as usize, 8, theme.line_2);

    let x = p.x + layout::osd_pad_x();
    labels.push(Label {
        x,
        y: p.y + layout::osd_pad_top(),
        style: tokens::LB,
        text: String::from(o.label()),
        color: theme.fg_4,
    });
    labels.push(Label {
        x,
        y: p.y + layout::osd_pad_top() + scale::px(20),
        style: tokens::D2,
        text: o.value(),
        color: theme.fg,
    });
    // The detail line replaces the track, never sits under it: a state with something to explain
    // has no number, and a state with a number needs no explanation.
    if let Some(d) = o.detail() {
        labels.push(Label {
            x,
            y: p.bottom() - scale::px(24),
            style: tokens::B3,
            text: String::from(d),
            color: theme.fg_3,
        });
    }
    if let Some(t) = layout::osd_track(p, o.percent().is_some()) {
        canvas.fill_rect(t.x.max(0) as usize, t.y.max(0) as usize,
                         t.w as usize, t.h as usize, theme.line_2);
        let w = layout::osd_fill_w(t, o.percent().unwrap_or(0));
        if w > 0 {
            canvas.fill_rect(t.x.max(0) as usize, t.y.max(0) as usize,
                             w as usize, t.h as usize, theme.accent);
        }
    }
    let _ = atlas;
}

// ─────────────────────────── Cold start, stage 3 ───────────────────────────
// The kernel drew the mark at the centre of a black screen and left it there. The shell's first
// frames pick it up from exactly where it was and carry it to the bottom-right corner, dissolving
// into the Entity ring as it lands.
//
// The design calls this "the only animation in Nyx that carries an argument": the two marks are
// doing different jobs — one is the product, the other is the machine reporting on itself — and the
// corner belongs permanently to the second. Landing the logo there and leaving it would put a brand
// where the system status lives. Cutting straight to the desktop would show a splash being torn
// down. Dissolving one into the other says the handoff happened without ever showing a logo sitting
// somewhere it does not belong.
//
// The translation itself is nearly free: a coverage bitmap at a different destination rectangle,
// once per frame, for 22 frames, once per power-on.

/// How long the travel takes. The design's 22 frames at 60 Hz.
const HANDOFF_MS: usize = 360;
/// The fraction of the way through at which the mark starts dissolving into the Entity ring — the
/// design's "last ten of twenty-two frames".
const HANDOFF_FADE_FROM: u32 = UNIT * 12 / 22;


struct Handoff {
    /// Where the kernel left the mark. `None` when there was no graphical boot screen at all — no
    /// framebuffer, or the user asked for the log — in which case there is nothing to hand over.
    from: Option<Rect>,
    /// When the first frame carrying the mark was drawn.
    ///
    /// ★ Set on that first frame, not in `begin`. The shell builds its atlas and generates the
    /// wallpaper before it ever presents anything, which takes long enough that a clock started in
    /// the constructor would already be most of the way through — the mark would appear near the
    /// corner having skipped the journey.
    started_ms: Option<usize>,
    /// Eased progress for the frame being drawn, 0..=UNIT. Drives position and size.
    t: u32,
    /// LINEAR elapsed fraction, 0..=UNIT. Drives the cross-fade.
    ///
    /// ★ Not the same number. The design's cross-fade is "the last ten of twenty-two frames" — a
    /// span of TIME. Driving it from the eased progress instead starts it at about 85 ms rather
    /// than 195 ms, because ease-out is front-loaded: the mark would be half dissolved before it
    /// had finished travelling.
    lin: u32,
}

impl Handoff {
    fn begin() -> Handoff {
        let (x, y, w, h) = sys_boot_mark();
        // A zero rect is the kernel saying "I never painted a mark". Not an error — the boot log
        // path is a legitimate way to start — so the shell just draws its first frame normally.
        let from = if w > 0 && h > 0 { Some(Rect::new(x, y, w, h)) } else { None };
        if from.is_none() {
            sys_print("[SHELL] no boot mark to hand over; starting directly\n");
        }
        Handoff { from, started_ms: None, t: 0, lin: 0 }
    }

    /// Advance to `now` and report whether there is still something to draw.
    ///
    /// ★ Driven by elapsed TIME, not by a frame counter. The first second of the machine's life is
    /// the least predictable moment it has — apps are launching, the atlas is warm but the GPU is
    /// not — so counting loop iterations would make the travel take 360 ms on a good run and well
    /// over a second on a loaded one. Against the clock it always takes 360 ms; a slow run simply
    /// samples the curve less often.
    fn step(&mut self, now: usize) -> bool {
        if self.from.is_none() {
            return false;
        }
        let start = *self.started_ms.get_or_insert(now);
        let elapsed = now.saturating_sub(start).min(HANDOFF_MS);
        self.t = nyx_meridian::motion::ease(elapsed as u32, HANDOFF_MS as u32);
        self.lin = (elapsed * UNIT as usize / HANDOFF_MS) as u32;
        now.saturating_sub(start) < HANDOFF_MS
    }
}

/// Draw the travelling mark for this frame, if the handoff is still running.
fn handoff_paint(state: &Shell, canvas: &mut Canvas, theme: Theme) {
    let Some(from) = state.handoff.from else { return };
    let t = state.handoff.t;
    if t >= UNIT {
        return;
    }
    let to = layout::entity_mark(state.screen_w, state.screen_h);

    let px = lerp_i32(from.w, to.w, t as i32).max(1) as usize;
    let x = lerp_i32(from.x, to.x, t as i32);
    let y = lerp_i32(from.y, to.y, t as i32);

    // The mark dissolves over the last stretch while the Entity ring underneath shows through. The
    // ring is already drawn by the normal Entity path, so this only has to take the mark away.
    let lin = state.handoff.lin;
    let alpha = if lin < HANDOFF_FADE_FROM {
        255
    } else {
        let k = (lin - HANDOFF_FADE_FROM) * 255 / (UNIT - HANDOFF_FADE_FROM);
        255u32.saturating_sub(k)
    };
    if alpha == 0 {
        return;
    }

    let cov = brand::mark_at(px);
    let argb: Vec<u32> = cov.iter().map(|&c| ((c as u32 * alpha / 255) << 24)).collect();
    canvas.blit_rgba(x.max(0) as usize, y.max(0) as usize, &argb, px, px, Some(theme.fg));
}

fn lerp_i32(from: i32, to: i32, t: i32) -> i32 {
    from + (to - from) * t / UNIT as i32
}

/// Push a shape to the hardware cursor plane, hotspot and all.
///
/// The 16 KB plane image is a stack temporary rather than a resident table. See the note in
/// `nyx_meridian::cursor` — this runs a few times per session, not per frame.
fn upload_cursor(shape: Shape) {
    let mut img = [0u32; 64 * 64];
    let (hx, hy) = cursor::render(shape, &mut img);
    sys_cursor_set_image(&img, hx, hy);
}

// ─────────────────────────────── Windows ───────────────────────────────

/// Grab band just inside a surface's edge where a drag starts a resize. Scaled: it is a pointer
/// target, and on a 4K panel an 8px band is a band you keep missing.
fn resize_band() -> i32 {
    scale::px(8)
}

/// Minimum surface size.
///
/// Height scales; width does NOT — `MIN_W` is in real pixels because it exists to keep the GPU
/// composite path alive, not to look right. `sys_gpu_composite` requires `src_pitch % 64 == 0` and
/// SILENTLY falls back to CPU compositing otherwise, which shows up as an unexplained framerate
/// cliff rather than as an error, so every width is snapped to a multiple of 16 regardless of scale.
const MIN_W: i32 = 208;
fn min_h() -> i32 {
    scale::px(100)
}

/// The wallpaper band the dock and the Entity rest on. A maximised window stops above it: Meridian
/// has no taskbar to be covered, but the dock IS the launcher, and covering it would strand the user
/// with no way to start anything.
fn chrome_band() -> i32 {
    layout::dock_bottom() + layout::dock_icon() + scale::px(12)
}

/// A window as the shell sees it.
struct Win {
    /// The SURFACE rect — the app's pixels. The caption line is `layout::caption(r)`, above it.
    r: Rect,
    title: [u8; 64],
    title_len: usize,
    exists: bool,
    /// Folded: the surface is collapsed and only the caption line remains, in place. This replaces
    /// minimise; there is nowhere else for the window to go, and that is the point.
    folded: bool,
    maximized: bool,
    saved: Rect,
    /// Fade-in on open. Rides the GPU quad's own alpha, so it costs nothing.
    ///
    /// Kept as a resolved byte rather than asked of `open` at draw time: it is read from several
    /// places per frame, including the shadow and the quad list, and they must all agree.
    opacity: u8,
    /// The open transition driving `opacity` and the surface's scale.
    open: Motion,
    /// The two caption controls fading in when the window is hovered or focused.
    ///
    /// Runs forwards to reveal and backwards to hide, so a pointer that crosses a caption and
    /// leaves again does not snap — `Motion` only runs one way, so the direction is held here.
    ctl: Motion,
    ctl_shown: bool,
}

impl Win {
    fn name(&self) -> &str {
        core::str::from_utf8(&self.title[..self.title_len]).unwrap_or("")
    }
}

struct Client {
    win: Win,
    owner_pid: u64,
    shm_id: u64,
    buffer: *const u32,
    buf_w: usize,
    buf_h: usize,
    gpu_gva: u32,
    /// The app's bundle directory, derived from the icon path it declared. This is what maps a live
    /// window back to a dock slot for the running/active dot.
    bundle: [u8; 96],
    bundle_len: usize,
}

impl Client {
    fn bundle(&self) -> &str {
        core::str::from_utf8(&self.bundle[..self.bundle_len]).unwrap_or("")
    }
}

fn get_str_len(buf: &[u8; 64]) -> usize {
    buf.iter().position(|&c| c == 0).unwrap_or(64)
}
fn get_icon_len(buf: &[u8; 96]) -> usize {
    buf.iter().position(|&c| c == 0).unwrap_or(96)
}

/// Read a client's reported scroll state from its (already-mapped) `WindowHeader`, which sits just
/// before the pixel buffer.
fn client_scroll(client: &Client) -> (usize, usize) {
    if client.buffer.is_null() {
        return (0, 0);
    }
    let hdr = unsafe {
        &*((client.buffer as *const u8).sub(core::mem::size_of::<WindowHeader>())
            as *const WindowHeader)
    };
    (hdr.content_h as usize, hdr.scroll_off as usize)
}

fn mouse_to_scroll(my: usize, bar_y: usize, viewport: usize, content: usize) -> usize {
    if content <= viewport {
        return 0;
    }
    let (_, thumb_h) = nyx_gui::ui::scrollbar_thumb(viewport, content, viewport, 0);
    let track_range = viewport.saturating_sub(thumb_h);
    if track_range == 0 {
        return 0;
    }
    let rel = my.saturating_sub(bar_y).saturating_sub(thumb_h / 2).min(track_range);
    rel * (content - viewport) / track_range
}

/// Edge bitmask for a point against a SURFACE: L=1 R=2 T=4 B=8. Unlike the old compositor there is
/// no title bar folded into this rect — the caption is a separate line above, and dragging it is a
/// move, never a resize.
fn resize_edges(w: &Win, mx: i32, my: i32) -> u8 {
    if w.folded || w.maximized {
        return 0;
    }
    let r = w.r;
    if mx < r.x || mx > r.right() || my < r.y || my > r.bottom() {
        return 0;
    }
    let mut e = 0u8;
    if mx < r.x + resize_band() {
        e |= 1;
    }
    if mx > r.right() - resize_band() {
        e |= 2;
    }
    if my < r.y + resize_band() {
        e |= 4;
    }
    if my > r.bottom() - resize_band() {
        e |= 8;
    }
    e
}

// ─────────────────────────────── Text ───────────────────────────────

/// A laid-out string, resolved once and then drawn by whichever path is available.
///
/// Both the GPU batch and the CPU fallback consume this list, so they cannot disagree about where
/// anything sits — the compositor learned that lesson with its start-menu labels.
struct Label {
    x: i32,
    y: i32,
    style: Style,
    text: String,
    color: u32,
}

/// A laid-out icon. `(x, y)` is the top-left of its `px`-square box, not of its ink.
struct IconDraw {
    id: &'static str,
    px: usize,
    x: i32,
    y: i32,
    color: u32,
}

/// Draw every label and icon for this frame.
///
/// Returns true if the GPU batch was used. On any failure the identical content is CPU-drawn — in
/// the wrong typeface, because the CPU path is `nyx_gui`'s bitmap DejaVu, but a desktop with the
/// wrong font beats a desktop with no text on it.
///
/// ⚠️ Every colour handed to the GPU path here must be a NEUTRAL. `build_ps_text` in the kernel
/// interpolates a single luminance across the glyph quad, so the batched text shader is greyscale.
/// Anything accent-coloured is drawn CPU-side as a `fill_rect` or a tinted `blit_rgba` instead —
/// see the dock's active dot.
fn paint_text(atlas: Option<&Atlas>, canvas: &mut Canvas, labels: &[Label], icons: &[IconDraw]) -> bool {
    if let Some(a) = atlas {
        let mut glyphs: Vec<GlyphQuad> = Vec::new();
        for l in labels {
            a.layout(l.style, l.x, l.y, &l.text, l.color, &mut glyphs);
        }
        for i in icons {
            if let Some(q) = a.icon(i.id, i.px, i.x, i.y, i.color) {
                glyphs.push(q);
            }
        }
        if glyphs.is_empty() {
            return true;
        }
        if sys_gpu_draw_text(a.gva, a.w, a.h, a.pitch, &glyphs) {
            sys_gpu_sync();
            return true;
        }
    }
    for l in labels {
        if l.x < 0 || l.y < 0 {
            continue;
        }
        canvas.print_str(l.x as usize, l.y as usize, &l.text, l.color, 1);
    }
    for i in icons {
        if let Some(ic) = icons::get(i.id, i.px) {
            let px: Vec<u32> = ic.cov.iter().map(|&c| (c as u32) << 24).collect();
            let (x, y) = (i.x + ic.off_x as i32, i.y + ic.off_y as i32);
            if x >= 0 && y >= 0 {
                canvas.blit_rgba(x as usize, y as usize, &px, ic.w as usize, ic.h as usize, Some(i.color));
            }
        }
    }
    false
}

/// Vertical position for a line of `style` centred in a `h`-tall box starting at `top`.
fn centre_text_y(top: i32, h: i32, style: Style) -> i32 {
    top + (h - style.line() as i32) / 2
}

/// Pixel width of `text` in `style`. Falls back to the CPU font's metrics when the atlas failed to
/// build, so right-aligned text stays right-aligned on the degraded path instead of drifting.
fn measure(atlas: Option<&Atlas>, style: Style, text: &str) -> i32 {
    match atlas {
        Some(a) => a.measure(style, text) as i32,
        None => Canvas::text_width(text, 1) as i32,
    }
}

// ─────────────────────────────── The Command ───────────────────────────────

/// Longest query the Command accepts. Bounded by construction so the keystroke path allocates
/// nothing — and 48 characters is far past the point where refining by typing has stopped working.
const QUERY_MAX: usize = 48;

/// What pressing return on a result does.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    /// Launch [`APPS`]`[i]`.
    Launch(usize),
    /// Swap dark ⇄ light. The whole theme is one struct and the wallpaper is regenerated from it, so
    /// this is genuinely instant rather than a restart.
    ToggleTheme,
    /// Collapse every window to its caption line. With no taskbar, this is how you clear the desk —
    /// and unlike "minimise all" nothing goes anywhere, so it is trivially reversible.
    FoldAll,
    /// Ask which F-keys step the panel backlight. See [`BrightKeys`] for why this has to be asked.
    LearnBrightnessKeys,
}

/// One line of the Command's result list: a group heading, or a result with an action.
struct Hit {
    kind: CmdEntry,
    icon: &'static str,
    name: String,
    /// The right-hand column: "Application", a command's effect, a network's state.
    meta: String,
    action: Option<Action>,
}

// ─────────────────────────────── The Entity ───────────────────────────────

/// The Entity's three states. Each is a separate coverage glyph in the same atlas as the type, so a
/// state change costs exactly what drawing the previous state cost.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mood {
    Nominal,
    Working,
    Attention,
}

impl Mood {
    fn glyph(self) -> &'static str {
        match self {
            Mood::Nominal => Icons::ENTITY_NOMINAL,
            Mood::Working => Icons::ENTITY_WORKING,
            Mood::Attention => Icons::ENTITY_ATTENTION,
        }
    }
}

/// Everything the Entity reports, sampled once a second.
///
/// ★ Every field here is real. The design's Entity surface asks for Processor / Memory / Graphics /
/// Thermal, and this machine can answer three of those: the kernel's entity state tracks `energy`
/// (CPU load and thread switching) and `entropy` (memory and filesystem I/O) on 0..100 scales, and
/// the thermal governor tracks package temperature. There is no GPU utilisation counter, so the
/// Graphics row reports the shell's own composite rate — which is a real graphics measurement, taken
/// from the one GPU consumer that is always running.
///
/// Two rows the design shows cannot be answered at all: there is no battery driver and no HDA
/// driver. Those render and say so. Inventing an "86% · 4 h 20 m" would make the surface look
/// finished and be a lie, and the point of the Entity is that it is the thing you trust about the
/// machine.
struct Vitals {
    temp: u8,
    cores: usize,
    tasks: u64,
    /// 0..100, driven by CPU load / thread switching (`sys_get_entity_stats[0]`).
    energy: f32,
    /// 0..100, driven by memory usage and filesystem I/O (`sys_get_entity_stats[1]`).
    entropy: f32,
    fs_total: u64,
    fs_free: u64,
    wifi: u32,
    ssid: String,
    rtc: RtcTime,
    mood: Mood,
    /// The two-word phrase beside the mark, and the sentence the surface opens with.
    phrase: &'static str,
    detail: String,
}

impl Vitals {
    fn new() -> Vitals {
        Vitals {
            temp: 0,
            cores: 1,
            tasks: 0,
            energy: 0.0,
            entropy: 0.0,
            fs_total: 0,
            fs_free: 0,
            wifi: WIFI_IDLE,
            ssid: String::new(),
            rtc: RtcTime { sec: 0, min: 0, hour: 0, day: 1, month: 1, year: 2026 },
            mood: Mood::Nominal,
            phrase: "System nominal",
            detail: String::new(),
        }
    }

    /// Sample everything and decide what the Entity has to say about it.
    ///
    /// The precedence is deliberate: heat first, then storage, then load. The Entity says ONE thing,
    /// so it has to say the most important one — a "Working" ring during a thermal event would be
    /// technically true and useless.
    fn refresh(&mut self, fps: usize) {
        {
            // ~2.7 KB on the stack, once a second. Kept local rather than held as a field so the
            // shell's own struct stays small enough to live on the stack comfortably.
            let mut info: SystemInfo = unsafe { core::mem::zeroed() };
            sys_get_system_info(&mut info as *mut SystemInfo);
            self.temp = info.current_temp;
            self.tasks = info.task_count;
        }
        let mut stats = [0f32; 4];
        sys_get_entity_stats(&mut stats);
        self.energy = stats[0];
        self.entropy = stats[1];
        self.cores = sys_get_active_cores();

        if let Some(sf) = sys_statfs() {
            self.fs_total = sf.total_bytes;
            self.fs_free = sf.free_bytes;
        }

        match sys_wifi_status() {
            Some(s) => {
                self.wifi = s.state;
                let n = (s.ssid_len as usize).min(s.ssid.len());
                self.ssid = String::from(core::str::from_utf8(&s.ssid[..n]).unwrap_or(""));
            }
            None => self.wifi = WIFI_IDLE,
        }
        self.rtc = sys_get_rtc_local();

        let low_storage = self.fs_total > 0 && self.fs_free * 10 < self.fs_total;
        if self.temp >= 82 {
            self.mood = Mood::Attention;
            self.phrase = "Thermal pressure";
            self.detail = alloc::format!(
                "Package reached {} \u{00B0}C. Clocks reduced to hold the envelope; no action needed.",
                self.temp
            );
        } else if low_storage {
            self.mood = Mood::Attention;
            self.phrase = "Storage low";
            self.detail = alloc::format!(
                "{} GB free of {} GB on /mnt/nvme. Writes will start failing below a gigabyte.",
                self.fs_free / 1_000_000_000,
                self.fs_total / 1_000_000_000
            );
        } else if self.energy > 60.0 {
            self.mood = Mood::Working;
            self.phrase = "Working";
            self.detail = alloc::format!(
                "{} tasks across {} cores at {} \u{00B0}C.",
                self.tasks, self.cores, self.temp
            );
        } else {
            self.mood = Mood::Nominal;
            self.phrase = "System nominal";
            self.detail = alloc::format!(
                "{} tasks across {} cores. Compositing at {} frames per second.",
                self.tasks, self.cores, fps
            );
        }
    }

    /// The four measurements, as (key, value) pairs. `graphics` is passed in because it is the
    /// shell's own frame rate — see the struct note for why that is the honest Graphics row.
    fn measures(&self, fps: usize) -> [(&'static str, String); 4] {
        [
            ("Processor", alloc::format!("{}%", self.energy as u32)),
            ("Memory", alloc::format!("{}%", self.entropy as u32)),
            ("Graphics", alloc::format!("{} fps", fps)),
            ("Thermal", alloc::format!("{} \u{00B0}C", self.temp)),
        ]
    }

    fn network_value(&self) -> String {
        match self.wifi {
            WIFI_CONNECTED if !self.ssid.is_empty() => self.ssid.clone(),
            WIFI_CONNECTED => String::from("Connected"),
            WIFI_CONNECTING => String::from("Connecting"),
            WIFI_AUTH_FAILED => String::from("Authentication failed"),
            WIFI_NO_LEASE => String::from("No lease"),
            WIFI_HW_FAILED => String::from("Adapter unavailable"),
            _ => String::from("Not connected"),
        }
    }

    /// `HH:MM` — the Entity footer is the only clock in Meridian, because there is no tray to put
    /// one in.
    fn clock(&self) -> String {
        alloc::format!("{:02}:{:02}", self.rtc.hour, self.rtc.min)
    }

    fn date(&self) -> String {
        const MONTHS: [&str; 12] = [
            "January", "February", "March", "April", "May", "June",
            "July", "August", "September", "October", "November", "December",
        ];
        let m = MONTHS[((self.rtc.month as usize).saturating_sub(1)).min(11)];
        alloc::format!("{} {} {}", self.rtc.day, m, self.rtc.year)
    }
}

// ─────────────────────────────── State ───────────────────────────────

struct Shell {
    theme: Theme,
    clients: Vec<Client>,
    next_win_id: usize,

    mx: i32,
    my: i32,
    prev_mx: i32,
    prev_my: i32,
    left: bool,
    prev_left: bool,

    dirty_min_x: usize,
    dirty_min_y: usize,
    dirty_max_x: usize,
    dirty_max_y: usize,
    needs_redraw: bool,

    dragging: Option<usize>,
    drag_off_x: i32,
    drag_off_y: i32,
    resizing: Option<usize>,
    resize_edges: u8,
    scrolling: Option<usize>,
    cursor: Shape,
    /// Cold-start stage 3 — the mark travelling from the kernel's screen to the corner.
    handoff: Handoff,
    /// The system popup and the time it expires. `None` when nothing is showing.
    osd: Option<(Osd, usize)>,

    /// The window whose APP currently believes the pointer is inside it, as `(pid, shm_id)`.
    ///
    /// Not an index: a click raises a window, which reorders `clients`, and the index of the window
    /// we owe a leave to would move out from under us. `shm_id` is unique per live window and
    /// survives the reorder.
    hover_app: Option<(u64, u64)>,
    /// The last window-local position sent, and when. Both are rate limits — see `MSG_MOUSE_MOVE`.
    hover_app_pos: (i32, i32),
    hover_app_ms: usize,

    /// The window under the pointer. Controls fade in on hover, so a bare pointer move has to
    /// repaint a caption line — and under the hardware cursor a bare move dirties nothing at all.
    hover_win: Option<usize>,
    hover_ctl: Option<(usize, Control)>,
    hover_dock: Option<usize>,

    /// The last caption click, for the double-click that maximises. `(client id, time_ms)`.
    last_caption_click: (usize, usize),

    screen_w: i32,
    screen_h: i32,
    screen_stride: usize,
    hw_cursor: bool,

    /// The 256x256 wallpaper texture, kept on the CPU as well as in GPU memory: carving a window's
    /// rounded corners means knowing what colour the ground is at that exact pixel.
    paper: Vec<u32>,
    paper_gva: u32,
    /// The wallpaper's SHM mapping, kept so a theme change can rewrite the texture in place rather
    /// than leaking a surface per toggle.
    paper_vaddr: u64,

    // ── the Command ──
    cmd_open: bool,
    query: [u8; QUERY_MAX],
    query_len: usize,
    /// Index into `hits` of the selected row, or None when nothing matched.
    cmd_sel: Option<usize>,
    cmd_hover: Option<usize>,
    /// The current result list. Rebuilt when the query changes rather than per frame — the render
    /// pass and the return key must agree about what row 3 is.
    hits: Vec<Hit>,
    /// How many results matched before the list was capped, for the footer's `N of M`.
    hits_total: usize,

    // ── the Entity ──
    ent_open: bool,
    ent_hover: Option<usize>,
    vitals: Vitals,
    /// How many lines `vitals.detail` wraps to. Cached rather than recomputed, because the surface's
    /// whole geometry is derived from it and the click hit-test has no atlas in hand — two different
    /// answers would put the control rows somewhere other than where they were drawn.
    ent_detail_lines: i32,

    // ── brightness ──
    /// Which two F-keys step the panel. Read from disk at startup; see [`BrightKeys`].
    bright: BrightKeys,
    /// Set while the shell is asking which keys those should be. Non-`None` means the keyboard is
    /// being captured — the OSD is a prompt, and a prompt that leaks keys to the focused app would
    /// type into whatever is behind it.
    learn: Option<Learn>,
    /// The panel level as of the last time we looked, or `None` on a display with no PWM backlight.
    /// Sampled when the Entity opens, not per frame — this shell is the only thing that writes it.
    bright_pct: Option<u32>,
    /// True while the pointer is dragging the Entity's brightness track. Held so the drag keeps
    /// working after the pointer leaves the row vertically, which is what every slider does and
    /// what makes one feel attached rather than slippery.
    ent_drag_bright: bool,

    last_clock_sec: usize,
    frame_count: u64,
    last_frame_ms: usize,
    fps: usize,
    fps_window_start: usize,
    fps_window_frames: usize,
    fps_logs: usize,
}

/// Where a window is first placed. Cascaded, and clamped so the caption line — which lives ABOVE the
/// surface — is never off the top of the screen.
fn default_pos(n: usize) -> (i32, i32) {
    (100 + (n as i32 * 30), (100 + (n as i32 * 30)).max(layout::caption_h() + 8))
}

impl Shell {
    fn new(w: i32, h: i32, stride: usize) -> Shell {
        Shell {
            theme: Theme::dark(),
            clients: Vec::new(),
            next_win_id: 0,
            mx: w / 2,
            my: h / 2,
            prev_mx: w / 2,
            prev_my: h / 2,
            left: false,
            prev_left: false,
            dirty_min_x: 0,
            dirty_min_y: 0,
            dirty_max_x: stride,
            dirty_max_y: h as usize,
            needs_redraw: true,
            dragging: None,
            drag_off_x: 0,
            drag_off_y: 0,
            resizing: None,
            resize_edges: 0,
            scrolling: None,
            cursor: Shape::Pointer,
            handoff: Handoff::begin(),
            osd: None,
            hover_app: None,
            hover_app_pos: (i32::MIN, i32::MIN),
            hover_app_ms: 0,
            hover_win: None,
            hover_ctl: None,
            hover_dock: None,
            last_caption_click: (usize::MAX, 0),
            screen_w: w,
            screen_h: h,
            screen_stride: stride,
            hw_cursor: false,
            paper: Vec::new(),
            paper_gva: 0,
            paper_vaddr: 0,
            cmd_open: false,
            query: [0; QUERY_MAX],
            query_len: 0,
            cmd_sel: None,
            cmd_hover: None,
            hits: Vec::new(),
            hits_total: 0,
            ent_open: false,
            ent_hover: None,
            vitals: Vitals::new(),
            ent_detail_lines: 0,
            bright: BrightKeys::load(),
            learn: None,
            bright_pct: None,
            ent_drag_bright: false,
            last_clock_sec: 0,
            frame_count: 0,
            last_frame_ms: 0,
            fps: 0,
            fps_window_start: 0,
            fps_window_frames: 0,
            fps_logs: 0,
        }
    }

    // ── damage ──

    fn mark_dirty(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let x0 = x.max(0) as usize;
        let y0 = y.max(0) as usize;
        let x1 = ((x + w).max(0) as usize).min(self.screen_stride);
        let y1 = ((y + h).max(0) as usize).min(self.screen_h as usize);
        self.dirty_min_x = self.dirty_min_x.min(x0);
        self.dirty_min_y = self.dirty_min_y.min(y0);
        self.dirty_max_x = self.dirty_max_x.max(x1);
        self.dirty_max_y = self.dirty_max_y.max(y1);
        self.needs_redraw = true;
    }

    fn mark_full(&mut self) {
        self.dirty_min_x = 0;
        self.dirty_min_y = 0;
        self.dirty_max_x = self.screen_stride;
        self.dirty_max_y = self.screen_h as usize;
        self.needs_redraw = true;
    }

    /// Dirty a window's whole on-screen extent: the caption line, the surface, AND the drop shadow,
    /// which overhangs on every side. A margin that padded only right/bottom is how the old
    /// compositor left a border trace behind every drag.
    fn mark_win(&mut self, surface: Rect) {
        const M: i32 = 16; // shadow blur is 9; this covers it with slack on all four sides
        let f = layout::window_footprint(surface);
        self.mark_dirty(f.x - M, f.y - M, f.w + M * 2, f.h + M * 2);
    }

    fn mark_dock(&mut self) {
        // The whole bottom band: dock at bottom-left, Entity at bottom-right.
        let y = self.screen_h - chrome_band();
        self.mark_dirty(0, y, self.screen_stride as i32, chrome_band());
    }

    /// Dirty just the Command's panel and its shadow.
    ///
    /// Worth its own function rather than reaching for `mark_full`: while the Command is open every
    /// frame repaints the scrim, and the scrim is an alpha blend over most of a million pixels.
    /// Scissoring to the panel is the difference between a hover highlight that appears instantly
    /// and one that costs a whole frame.
    fn mark_command(&mut self) {
        const M: i32 = 16;
        let p = layout::command_panel(self.screen_w, layout::command_body_height(&self.kinds()));
        self.mark_dirty(p.x - M, p.y - M, p.w + M * 2, p.h + M * 2);
    }

    fn mark_entity(&mut self) {
        const M: i32 = 16;
        let s = self.entity_geometry().surface;
        self.mark_dirty(s.x - M, s.y - M, s.w + M * 2, s.h + M * 2);
    }

    // ── model ──

    /// The focused window: topmost existing, unfolded window (last in the z-order).
    fn active(&self) -> Option<usize> {
        self.clients.iter().rposition(|c| c.win.exists && !c.win.folded)
    }

    fn raise(&mut self, idx: usize) -> usize {
        if idx + 1 >= self.clients.len() {
            return idx;
        }
        let c = self.clients.remove(idx);
        self.clients.push(c);
        self.clients.len() - 1
    }

    /// The topmost window whose footprint (caption + surface, or just the caption if folded)
    /// contains the pointer.
    fn window_at(&self, mx: i32, my: i32) -> Option<usize> {
        for (i, c) in self.clients.iter().enumerate().rev() {
            if !c.win.exists {
                continue;
            }
            let f = if c.win.folded {
                layout::folded_footprint(c.win.r)
            } else {
                layout::window_footprint(c.win.r)
            };
            if f.contains(mx, my) {
                return Some(i);
            }
        }
        None
    }

    /// Whether the app in dock slot `slot` is running, and whether it is the focused window.
    fn dock_state(&self, slot: usize) -> (bool, bool) {
        let app = match DOCK[slot] {
            Some(i) => &APPS[i],
            None => return (false, false),
        };
        let act = self.active();
        let mut running = false;
        let mut active = false;
        for (i, c) in self.clients.iter().enumerate() {
            if c.win.exists && c.bundle() == app.bundle {
                running = true;
                if act == Some(i) {
                    active = true;
                }
            }
        }
        (running, active)
    }

    // ── the Command ──

    fn query(&self) -> &str {
        core::str::from_utf8(&self.query[..self.query_len]).unwrap_or("")
    }

    /// Rebuild the result list from the current query.
    ///
    /// One index, two sources: applications and shell commands. The design also groups Files, which
    /// needs a filesystem index that does not exist yet — an empty "FILES" heading would read as a
    /// rendering fault, so the group is simply absent until there is something to put in it.
    ///
    /// The list is CAPPED, not scrolled. `layout::command_visible` decides how many fit and trims so
    /// it never ends on a heading; `hits_total` keeps the real count for the footer's `N of M`,
    /// which is exactly what the design's `6 of 41 results` is saying.
    fn rebuild_command(&mut self) {
        let q: Vec<u8> = self.query[..self.query_len].to_vec();
        let mut all: Vec<Hit> = Vec::new();

        let apps: Vec<usize> = (0..APPS.len()).filter(|&i| matches(APPS[i].label, &q)).collect();
        if !apps.is_empty() {
            all.push(Hit {
                kind: CmdEntry::Group,
                icon: "",
                name: String::from("Applications"),
                meta: String::new(),
                action: None,
            });
            for i in apps {
                all.push(Hit {
                    kind: CmdEntry::Row,
                    icon: APPS[i].icon,
                    name: String::from(APPS[i].label),
                    meta: String::from("Application"),
                    action: Some(Action::Launch(i)),
                });
            }
        }

        // Shell commands: things only the shell can do, so they belong to no application.
        let theme_label = if self.theme.is_dark { "Appearance — Light" } else { "Appearance — Dark" };
        let network_label = alloc::format!("Network — {}", self.vitals.network_value());
        let wifi_app = APPS.iter().position(|a| a.label == "Wi-Fi");
        let bright_label = alloc::format!("Brightness keys — {}", self.bright.describe());
        let cmds: [(&str, &str, &str, Option<Action>); 4] = [
            (theme_label, Icons::APP_SETTINGS, "Command", Some(Action::ToggleTheme)),
            ("Fold all windows", Icons::WIN_FOLD, "Command", Some(Action::FoldAll)),
            (&network_label, Icons::WIFI, "Choose a network", wifi_app.map(Action::Launch)),
            // The current binding is in the LABEL, not the meta column, so it is searchable: typing
            // "F5" finds the row that explains why F5 does what it does.
            (&bright_label, Icons::POWER, "Rebind", Some(Action::LearnBrightnessKeys)),
        ];
        let hitting: Vec<usize> = (0..cmds.len()).filter(|&i| matches(cmds[i].0, &q)).collect();
        if !hitting.is_empty() {
            all.push(Hit {
                kind: CmdEntry::Group,
                icon: "",
                name: String::from("Commands"),
                meta: String::new(),
                action: None,
            });
            for i in hitting {
                let (name, icon, meta, action) = cmds[i];
                all.push(Hit {
                    kind: CmdEntry::Row,
                    icon,
                    name: String::from(name),
                    meta: String::from(meta),
                    action,
                });
            }
        }

        self.hits_total = all.iter().filter(|h| h.kind == CmdEntry::Row).count();
        let kinds: Vec<CmdEntry> = all.iter().map(|h| h.kind).collect();
        let n = layout::command_visible(self.screen_h, &kinds);
        all.truncate(n);
        self.hits = all;
        self.cmd_sel = layout::command_first(&self.kinds());
        self.mark_full();
    }

    /// The result list as bare entry kinds — what every `layout::command_*` function takes.
    fn kinds(&self) -> Vec<CmdEntry> {
        self.hits.iter().map(|h| h.kind).collect()
    }

    fn set_command(&mut self, open: bool) {
        self.cmd_open = open;
        self.query_len = 0;
        self.cmd_hover = None;
        if open {
            // Opening closes the Entity: they are the two modal surfaces, and two things asking to
            // be read at once is neither of them being read.
            self.ent_open = false;
            self.clear_desktop_hover();
            self.rebuild_command();
        } else {
            self.hits.clear();
            self.cmd_sel = None;
        }
        self.mark_full();
    }

    /// Drop every desktop hover target. A modal surface takes the pointer entirely, so a window that
    /// was hovered when the Command opened must not keep showing its controls underneath it.
    fn clear_desktop_hover(&mut self) {
        self.hover_win = None;
        self.hover_ctl = None;
        self.hover_dock = None;
    }

    fn run(&mut self, i: usize) {
        let action = match self.hits.get(i).and_then(|h| h.action) {
            Some(a) => a,
            None => return,
        };
        self.set_command(false);
        match action {
            Action::Launch(app) => launch(&APPS[app]),
            Action::ToggleTheme => self.set_theme(if self.theme.is_dark {
                Theme::light()
            } else {
                Theme::dark()
            }),
            Action::FoldAll => {
                for c in self.clients.iter_mut() {
                    if c.win.exists {
                        c.win.folded = true;
                    }
                }
                self.mark_full();
            }
            Action::LearnBrightnessKeys => self.begin_learn(),
        }
    }

    /// Swap the theme, which means regenerating the wallpaper.
    ///
    /// The texture is rewritten IN PLACE, into the SHM that is already GGTT-mapped — allocating a new
    /// surface per toggle would leak one every time, and remapping a live GVA mid-frame is the class
    /// of bug that cost this project a boot already.
    fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.paper = paper::generate(&self.theme);
        if self.paper_vaddr != 0 {
            let dst = unsafe {
                core::slice::from_raw_parts_mut(self.paper_vaddr as *mut u32, self.paper.len())
            };
            dst.copy_from_slice(&self.paper);
        }
        self.mark_full();
    }

    // ── the Entity ──

    fn set_entity(&mut self, open: bool) {
        self.ent_open = open;
        self.ent_hover = None;
        self.ent_drag_bright = false;
        if open {
            self.cmd_open = false;
            self.clear_desktop_hover();
            // Read the panel once on open rather than once a frame. It cannot change behind our back
            // — this shell is the only writer — so polling it would be a syscall per frame to learn
            // something we already know.
            self.bright_pct = sys_backlight(0);
        }
        self.mark_full();
    }

    /// Sample the machine and re-wrap what the Entity has to say about it. Once a second: every
    /// input here changes on human timescales, and the frame is being recomposited anyway.
    fn refresh_vitals(&mut self, atlas: Option<&Atlas>) {
        self.vitals.refresh(self.fps);
        // Cap the detail the way the Command caps its result list, and for the same reason: the
        // surface has a fixed bottom edge and grows UPWARD, so an over-long sentence — or the same
        // sentence at 2x scale — pushes the statement off the top of the screen, where nothing
        // clips it.
        let cap = layout::entity_max_detail_lines(self.screen_h, 4, ENT_CONTROLS);
        self.ent_detail_lines = (nyx_meridian::atlas::wrap(
            &self.vitals.detail,
            (layout::entity_surface_w() - layout::ent_hd_pad_x() * 2) as usize,
            |s| measure(atlas, tokens::B3, s) as usize,
        )
        .len() as i32)
            .min(cap);
        // The Command's Network row carries the live link state, so a status change while it is
        // open has to reach the list.
        if self.cmd_open {
            let sel = self.cmd_sel;
            self.rebuild_command();
            // Keep the selection where the user left it — a link state change once a second must not
            // yank the highlight back to the top row under their finger. Only if that index is still
            // a selectable row, though; `rebuild_command`'s own first-row default covers the rest.
            if let Some(i) = sel {
                if self.hits.get(i).map_or(false, |h| h.kind == CmdEntry::Row) {
                    self.cmd_sel = Some(i);
                }
            }
        }
    }

    /// The Entity surface's geometry for the current vitals.
    fn entity_geometry(&self) -> layout::EntityLayout {
        layout::entity_layout(self.screen_w, self.screen_h, self.ent_detail_lines, 4, ENT_CONTROLS)
    }

    // ── IPC ──

    fn process_ipc(&mut self) {
        let mut msg = IpcMessage { sender_pid: 0, msg_type: 0, data1: 0, data2: 0 };
        while sys_ipc_recv(&mut msg, false) {
            match msg.msg_type {
                MSG_REQ_WINDOW => {
                    let shm_id = msg.data1;
                    let vaddr = sys_map_shm(shm_id) as *mut u8;
                    let header = unsafe { &*(vaddr as *const WindowHeader) };
                    if header.magic != WIN_MAGIC {
                        continue;
                    }
                    let w = header.width as i32;
                    let h = header.height as i32;
                    let (dx, dy) = default_pos(self.next_win_id);
                    let x = if header.requested_x == -1 { dx } else { header.requested_x };
                    // An app that asks for y=0 would put its caption line off the top of the
                    // screen. It cannot know that — the caption is the shell's, not the app's.
                    let y = if header.requested_y == -1 {
                        dy
                    } else {
                        header.requested_y.max(layout::caption_h())
                    };

                    let gpu_gva = 0x2000_0000 + (self.next_win_id * 0x0100_0000) as u32;
                    sys_gpu_map_shm(shm_id, gpu_gva);

                    // The bundle directory is everything up to and including the last '/' of the
                    // icon path the app declared. That is the key the dock matches on.
                    let mut bundle = [0u8; 96];
                    let mut bundle_len = 0usize;
                    {
                        let n = get_icon_len(&header.icon);
                        if let Some(cut) = header.icon[..n].iter().rposition(|&c| c == b'/') {
                            bundle_len = cut + 1;
                            bundle[..bundle_len].copy_from_slice(&header.icon[..bundle_len]);
                        }
                    }

                    self.clients.push(Client {
                        win: Win {
                            r: Rect::new(x, y, w, h),
                            title: header.title,
                            title_len: get_str_len(&header.title),
                            exists: true,
                            folded: false,
                            maximized: false,
                            saved: Rect::new(x, y, w, h),
                            opacity: 0,
                            open: Motion::starting(Transition::WindowOpen),
                            ctl: Motion::done(Transition::Controls),
                            ctl_shown: false,
                        },
                        owner_pid: msg.sender_pid,
                        shm_id,
                        buffer: unsafe { vaddr.add(core::mem::size_of::<WindowHeader>()) } as *const u32,
                        buf_w: w as usize,
                        buf_h: h as usize,
                        gpu_gva,
                        bundle,
                        bundle_len,
                    });
                    self.next_win_id += 1;
                    self.mark_full();
                    sys_ipc_send(msg.sender_pid, MSG_WINDOW_CREATED, shm_id, 0);
                }
                MSG_WINDOW_UPDATE_SHM => {
                    // Verbatim from the compositor: the SHM resize handshake. Capture the OLD
                    // mapping BEFORE overwriting it, remap the GGTT onto the SAME GVA (which
                    // overwrites the stale PTEs so the GPU stops referencing the old frames), and
                    // only then release our mapping and ACK the app so it can free the pages.
                    let new_shm_id = msg.data1;
                    let hdr_sz = core::mem::size_of::<WindowHeader>();
                    if let Some(client) = self.clients.iter_mut().find(|c| c.owner_pid == msg.sender_pid) {
                        let old_shm_id = client.shm_id;
                        let old_base = (client.buffer as u64).saturating_sub(hdr_sz as u64);
                        let old_size = hdr_sz + client.buf_w * client.buf_h * 4;
                        let owner = client.owner_pid;

                        let vaddr = sys_map_shm(new_shm_id) as *mut u8;
                        let header = unsafe { &*(vaddr as *const WindowHeader) };
                        client.shm_id = new_shm_id;
                        client.buffer = unsafe { vaddr.add(hdr_sz) } as *const u32;
                        client.buf_w = header.width as usize;
                        client.buf_h = header.height as usize;
                        sys_gpu_map_shm(new_shm_id, client.gpu_gva);

                        if old_shm_id != new_shm_id && old_base != 0 {
                            sys_unmap_shm(old_base, old_size);
                            sys_ipc_send(owner, MSG_SHM_RELEASED, old_shm_id, 0);
                        }
                        self.mark_full();
                    }
                }
                MSG_FLUSH_WINDOW => {
                    let r = self
                        .clients
                        .iter()
                        .find(|c| c.owner_pid == msg.sender_pid)
                        .map(|c| c.win.r);
                    if let Some(r) = r {
                        self.mark_win(r);
                    }
                }
                _ => {}
            }
        }
    }

    // ── input ──

    fn fold(&mut self, idx: usize) {
        self.clients[idx].win.folded = true;
        self.mark_full();
    }

    fn unfold(&mut self, idx: usize) {
        self.clients[idx].win.folded = false;
        self.raise(idx);
        self.mark_full();
    }

    fn toggle_maximize(&mut self, idx: usize) {
        let w = &mut self.clients[idx].win;
        if w.maximized {
            w.r = w.saved;
            w.maximized = false;
        } else {
            w.saved = w.r;
            // Leave the caption line room above and the dock room below. Width snaps to 16 so the
            // GPU composite keeps its 64-byte pitch.
            w.r = Rect::new(
                0,
                layout::caption_h(),
                self.screen_w & !15,
                self.screen_h - layout::caption_h() - chrome_band(),
            );
            w.maximized = true;
        }
        let (nw, nh) = (self.clients[idx].win.r.w, self.clients[idx].win.r.h);
        sys_ipc_send(self.clients[idx].owner_pid, MSG_WINDOW_RESIZED, nw as u64, nh as u64);
        self.mark_full();
    }

    fn close(&mut self, idx: usize) {
        let r = self.clients[idx].win.r;
        self.clients[idx].win.exists = false;
        sys_ipc_send(self.clients[idx].owner_pid, MSG_WINDOW_CLOSE, 0, 0);
        self.mark_win(r);
        self.mark_dock();
    }

    fn process_input(&mut self, now: usize) {
        if let Some(key) = sys_read_key() {
            self.on_key(key);
        }

        let (mx_raw, my_raw, left, _right) = sys_get_mouse();
        self.mx = (mx_raw as i32).clamp(0, self.screen_w - 1);
        self.my = (my_raw as i32).clamp(0, self.screen_h - 1);
        self.left = left;

        if (self.mx != self.prev_mx || self.my != self.prev_my) && !self.hw_cursor {
            let pad = 20;
            self.mark_dirty(self.prev_mx - pad, self.prev_my - pad, pad * 2, pad * 2);
            self.mark_dirty(self.mx - pad, self.my - pad, pad * 2, pad * 2);
        }

        if self.left && !self.prev_left {
            self.on_press(now);
        } else if self.left {
            self.on_drag();
        } else {
            self.dragging = None;
            self.resizing = None;
            self.scrolling = None;
            self.resize_edges = 0;
            self.ent_drag_bright = false;
        }
        self.prev_left = self.left;

        self.update_cursor();
        self.update_hover();
        self.notify_app_hover(now);
    }

    /// Every keystroke the shell sees.
    ///
    /// The Command OWNS the keyboard while it is open — that is what makes typing into it work at
    /// all, since every other key goes straight to the focused app.
    fn on_key(&mut self, key: char) {
        use nyx_api::keys;

        // Capturing a binding owns the keyboard outright — see `Shell::learn`.
        if let Some(step) = self.learn {
            self.capture_bright_key(step, key);
            return;
        }

        if key == keys::SUPER {
            let open = !self.cmd_open;
            self.set_command(open);
            return;
        }

        // System keys are claimed by the shell wherever the focus is — they are about the machine,
        // not about the window. They deliberately do NOT dismiss the Command or the Entity:
        // changing the brightness is not a reason to lose what you were typing.
        //
        // The bound F-keys are tested FIRST, before the dedicated brightness codepoints, because on
        // this class of hardware they are the only ones that ever arrive. E01A/E01B stay in the
        // match because they cost nothing and are what a machine that really does deliver a
        // brightness scancode would send.
        let raised = match self.bright.step_for(key) {
            Some(d) => Some(self.step_brightness(d)),
            None => match key {
                keys::BRIGHTNESS_UP => Some(self.step_brightness(BRIGHT_STEP)),
                keys::BRIGHTNESS_DOWN => Some(self.step_brightness(-BRIGHT_STEP)),
                keys::VOLUME_UP | keys::VOLUME_DOWN | keys::VOLUME_MUTE => {
                    Some(Osd::VolumeUnavailable)
                }
                _ => None,
            },
        };
        if let Some(o) = raised {
            self.show_osd(o);
            return;
        }

        if self.cmd_open {
            match key {
                '\x1b' => self.set_command(false),
                '\x08' => {
                    self.query_len = self.query_len.saturating_sub(1);
                    self.rebuild_command();
                }
                '\n' | '\r' => {
                    if let Some(i) = self.cmd_sel {
                        self.run(i);
                    }
                }
                keys::UP => {
                    self.cmd_sel = layout::command_step(&self.kinds(), self.cmd_sel, -1);
                    self.mark_full();
                }
                keys::DOWN => {
                    self.cmd_sel = layout::command_step(&self.kinds(), self.cmd_sel, 1);
                    self.mark_full();
                }
                '\t' => {
                    // ⇥ refine: adopt the selected result's name as the query. Not completion for
                    // its own sake — it is how you get from a three-letter guess to a query you can
                    // narrow FURTHER, which is the interaction the Command is built around.
                    if let Some(name) = self.cmd_sel.and_then(|i| self.hits.get(i)).map(|h| h.name.clone()) {
                        self.query_len = 0;
                        for &b in name.as_bytes() {
                            if self.query_len < QUERY_MAX && b.is_ascii() {
                                self.query[self.query_len] = b;
                                self.query_len += 1;
                            }
                        }
                        self.rebuild_command();
                    }
                }
                c if (c as u32) >= 0x20 && (c as u32) < 0x7F => {
                    if self.query_len < QUERY_MAX {
                        self.query[self.query_len] = c as u8;
                        self.query_len += 1;
                        self.rebuild_command();
                    }
                }
                _ => {}
            }
            return;
        }

        if self.ent_open && key == '\x1b' {
            self.set_entity(false);
            return;
        }

        let focused = self
            .clients
            .iter()
            .rev()
            .find(|c| c.win.exists && !c.win.folded)
            .map(|c| c.owner_pid);
        match focused {
            Some(pid) => {
                sys_ipc_send(pid, MSG_KEY_EVENT, key as u64, 0);
            }
            // "Press Space on the desktop and the Command opens." With nothing focused there is no
            // app to send it to, so Space here can only mean the desktop — and Super works either
            // way, which is what keeps the Command reachable once a window has the keyboard.
            None if key == ' ' => self.set_command(true),
            None => {}
        }
    }

    fn on_press(&mut self, now: usize) {
        let (mx, my) = (self.mx, self.my);

        // The Command is modal: it takes every click, and one outside it dismisses. Tested first
        // because it is drawn last and over everything.
        if self.cmd_open {
            let kinds = self.kinds();
            if let Some(i) = layout::command_hit(self.screen_w, &kinds, mx, my) {
                self.run(i);
            } else if !layout::command_panel(self.screen_w, layout::command_body_height(&kinds))
                .contains(mx, my)
            {
                self.set_command(false);
            }
            return;
        }

        // The Entity: its mark toggles the surface, a click inside the surface hits a control row,
        // and a click anywhere else dismisses it.
        if layout::entity_hit(self.screen_w, self.screen_h).contains(mx, my) {
            let open = !self.ent_open;
            self.set_entity(open);
            return;
        }
        if self.ent_open {
            let l = self.entity_geometry();
            if let Some(row) = l.control_hit(mx, my) {
                // Network opens the Wi-Fi app; Brightness is a control in its own right. Power now
                // reports real EC data but has nowhere to lead — there is no power-settings surface
                // yet — and Volume has no driver at all.
                if row == 0 {
                    if let Some(i) = APPS.iter().position(|a| a.label == "Wi-Fi") {
                        self.set_entity(false);
                        launch(&APPS[i]);
                    }
                } else if row == ENT_BRIGHT_ROW && self.bright_pct.is_some() {
                    // Anywhere in the row, not just on the 2px track — a 2px hit target is a dare.
                    // The track shows the level; the whole row sets it.
                    self.ent_drag_bright = true;
                    self.set_bright_from_pointer(mx);
                }
                return;
            }
            if !l.surface.contains(mx, my) {
                self.set_entity(false);
                return;
            }
            return;
        }

        // The dock rests on the wallpaper, under nothing, so it is tested before the windows.
        if let Some(slot) = layout::dock_hit(self.screen_h, mx, my) {
            if let Some(i) = DOCK[slot] {
                launch(&APPS[i]);
            }
            return;
        }

        let mut clicked: Option<usize> = None;
        for idx in (0..self.clients.len()).rev() {
            let w_r = self.clients[idx].win.r;
            let folded = self.clients[idx].win.folded;
            if !self.clients[idx].win.exists {
                continue;
            }

            // Controls first: they sit on the caption line and a drag must not steal them.
            match layout::control_hit(w_r, mx, my) {
                Some(Control::Close) => {
                    self.close(idx);
                    clicked = Some(idx);
                    break;
                }
                Some(Control::Fold) => {
                    if folded {
                        self.unfold(idx);
                    } else {
                        self.fold(idx);
                    }
                    clicked = Some(idx);
                    break;
                }
                None => {}
            }

            if layout::caption_drag_region(w_r).contains(mx, my) {
                if folded {
                    // A folded window has no surface to drag by, and clicking its caption is the
                    // obvious way to bring it back — there is no taskbar button to click instead.
                    self.unfold(idx);
                } else if self.last_caption_click.0 == idx
                    && now.wrapping_sub(self.last_caption_click.1) < 400
                {
                    // Maximise is a double-click on the caption. It gets no glyph of its own: the
                    // design ships two controls, not three.
                    self.toggle_maximize(idx);
                    self.last_caption_click = (usize::MAX, 0);
                } else {
                    self.last_caption_click = (idx, now);
                    if !self.clients[idx].win.maximized {
                        self.dragging = Some(idx);
                        self.drag_off_x = mx - w_r.x;
                        self.drag_off_y = my - w_r.y;
                    }
                }
                clicked = Some(idx);
                break;
            }

            if folded {
                continue; // nothing else of a folded window is on screen
            }

            let e = resize_edges(&self.clients[idx].win, mx, my);
            if e != 0 {
                self.resizing = Some(idx);
                self.resize_edges = e;
                clicked = Some(idx);
                break;
            }

            // The scrollbar the apps still expect. Un-Meridian-ised on purpose: it belongs to the
            // app's interior, and the interiors are ported in steps 10-16 of the build order.
            let (content_h, _) = client_scroll(&self.clients[idx]);
            if content_h > w_r.h as usize {
                let bar_x = w_r.right() - SCROLLBAR_W as i32;
                if mx >= bar_x && mx <= w_r.right() && my >= w_r.y && my <= w_r.bottom() {
                    self.scrolling = Some(idx);
                    let target =
                        mouse_to_scroll(my as usize, w_r.y as usize, w_r.h as usize, content_h);
                    sys_ipc_send(self.clients[idx].owner_pid, MSG_SCROLL, target as u64, 0);
                    self.mark_win(w_r);
                    clicked = Some(idx);
                    break;
                }
            }

            if w_r.contains(mx, my) {
                sys_ipc_send(
                    self.clients[idx].owner_pid,
                    MSG_MOUSE_EVENT,
                    (mx - w_r.x) as u64,
                    (my - w_r.y) as u64,
                );
                clicked = Some(idx);
                break;
            }
        }

        if let Some(idx) = clicked {
            if idx + 1 != self.clients.len() {
                let moved = self.clients.remove(idx);
                self.clients.push(moved);
                if self.dragging == Some(idx) {
                    self.dragging = Some(self.clients.len() - 1);
                }
                if self.resizing == Some(idx) {
                    self.resizing = Some(self.clients.len() - 1);
                }
                if self.scrolling == Some(idx) {
                    self.scrolling = Some(self.clients.len() - 1);
                }
                if self.last_caption_click.0 == idx {
                    self.last_caption_click.0 = self.clients.len() - 1;
                }
                self.mark_full();
            }
        }
    }

    fn on_drag(&mut self) {
        // Tested before the window drags: the Entity sits above every window, so a pointer inside it
        // is never also resizing something underneath.
        if self.ent_drag_bright {
            self.set_bright_from_pointer(self.mx);
            return;
        }
        if let Some(idx) = self.resizing {
            let o = self.clients[idx].win.r;
            self.mark_win(o);

            // The non-dragged edges stay anchored: a right/bottom drag only grows w/h, a left/top
            // drag moves the origin while the opposite edge stays put.
            let e = self.resize_edges;
            let right = o.right();
            let bottom = o.bottom();
            let mut n = o;
            if e & 2 != 0 {
                n.w = (self.mx - o.x).max(MIN_W);
            }
            if e & 1 != 0 {
                let x = self.mx.min(right - MIN_W);
                n.w = right - x;
                n.x = x;
            }
            if e & 8 != 0 {
                n.h = (self.my - o.y).max(min_h());
            }
            if e & 4 != 0 {
                let y = self.my.min(bottom - min_h()).max(layout::caption_h());
                n.h = bottom - y;
                n.y = y;
            }
            n.w = n.w.max(MIN_W) & !15;
            if e & 1 != 0 {
                n.x = right - n.w; // re-anchor so the 16px snap doesn't creep the right edge
            }
            n.h = n.h.max(min_h());

            if n != o {
                self.clients[idx].win.r = n;
                sys_ipc_send(
                    self.clients[idx].owner_pid,
                    MSG_WINDOW_RESIZED,
                    n.w as u64,
                    n.h as u64,
                );
            }
            self.mark_win(n);
        } else if let Some(idx) = self.dragging {
            let o = self.clients[idx].win.r;
            self.mark_win(o);
            let mut n = o;
            n.x = self.mx - self.drag_off_x;
            // Clamp so the caption line — which is ABOVE the surface — cannot leave the screen. A
            // window dragged to the top of the display otherwise loses its only controls.
            n.y = (self.my - self.drag_off_y).max(layout::caption_h());
            self.clients[idx].win.r = n;
            self.mark_win(n);
        } else if let Some(idx) = self.scrolling {
            if idx < self.clients.len() {
                let r = self.clients[idx].win.r;
                let (content_h, _) = client_scroll(&self.clients[idx]);
                if content_h > r.h as usize {
                    let target =
                        mouse_to_scroll(self.my as usize, r.y as usize, r.h as usize, content_h);
                    sys_ipc_send(self.clients[idx].owner_pid, MSG_SCROLL, target as u64, 0);
                    self.mark_win(r);
                }
            }
        }
    }

    /// Decide which of the twelve shapes the pointer is over, and push it if it changed.
    ///
    /// Five of the set are not reachable from here yet and that is a statement about the rest of the
    /// system, not an omission: `Precision` is the only cursor an app can *ask* for and there is no
    /// message for it; `Grab`/`Grabbing` need something draggable that is not a window; and
    /// `Unavailable` needs a drop target that can refuse. Each is one arm here once its machinery
    /// exists.
    fn update_cursor(&mut self) {
        let desired = if self.resizing.is_some() {
            Shape::for_edges(self.resize_edges).unwrap_or(Shape::Pointer)
        } else if self.dragging.is_some() {
            // A caption line being dragged. Reverts on release.
            Shape::Move
        } else if self.scrolling.is_some() {
            Shape::Pointer
        } else if self.cmd_open {
            // The Command takes the pointer entirely, and its query line is the one editable region
            // the shell can identify on its own.
            let body = layout::command_body_height(&self.kinds());
            if layout::command_query(self.screen_w, body).contains(self.mx, self.my) {
                Shape::Text
            } else {
                Shape::Pointer
            }
        } else {
            let mut c = Shape::Pointer;
            for client in self.clients.iter().rev() {
                if !client.win.exists {
                    continue;
                }
                let e = resize_edges(&client.win, self.mx, self.my);
                if e != 0 {
                    c = Shape::for_edges(e).unwrap_or(Shape::Pointer);
                    break;
                }
                if client.win.r.contains(self.mx, self.my) {
                    break;
                }
            }
            c
        };
        if desired != self.cursor {
            self.cursor = desired;
            if self.hw_cursor {
                upload_cursor(desired);
            } else {
                // The software cursor is drawn into the frame, so a shape change is damage. Under
                // the hardware cursor it is not — the plane is composited by scanout and the
                // framebuffer never learns about it.
                self.mark_cursor();
            }
        }
    }

    /// Dirty the box the software cursor occupies, at its current position.
    ///
    /// Generous by 16px on every side: the shape can change size as well as picture, and the box has
    /// to cover the shape being *replaced* as well as the one arriving.
    fn mark_cursor(&mut self) {
        const M: i32 = 32;
        self.mark_dirty(self.mx - M, self.my - M, M * 2, M * 2);
    }

    /// Recompute what the pointer is over.
    ///
    /// ★ This is not optional polish. Meridian's window controls have `opacity: 0` until the window
    /// is hovered, and the dock's glyphs lift from `--fg-4` to `--fg-2`. Under the hardware cursor a
    /// bare pointer move dirties nothing at all, so without an explicit mark here the controls would
    /// only appear on the next 1 Hz heartbeat — which reads as the shell being broken.
    /// Tell the app under the pointer where the pointer is, and tell the app it just left that it
    /// left.
    ///
    /// The shell's own hover — caption controls, dock glyphs, Command rows — is tracked from `mx/my`
    /// and needs none of this. This is the other side: an application's interior is pixels in an SHM
    /// buffer the shell knows nothing about, so a file row that lifts under the pointer can only
    /// happen if the app is told. Before Meridian nothing ever told it.
    ///
    /// Enter and leave go out immediately; moves are metered, because `process_input` runs on every
    /// iteration of the shell's loop and not once per presented frame.
    fn notify_app_hover(&mut self, now: usize) {
        // A shell gesture or a modal surface owns the pointer, and the app should behave as though
        // the pointer had left — otherwise a highlight sits under the Command's scrim.
        let captured = self.cmd_open
            || self.ent_open
            || self.dragging.is_some()
            || self.resizing.is_some()
            || self.scrolling.is_some();

        // `window_at` hits the footprint — caption line and shadow included. Hover is about the
        // app's own pixels, so narrow it to the surface.
        let target = if captured {
            None
        } else {
            self.window_at(self.mx, self.my).filter(|&i| {
                let c = &self.clients[i];
                !c.win.folded && c.win.r.contains(self.mx, self.my)
            })
        };

        let id = target.map(|i| (self.clients[i].owner_pid, self.clients[i].shm_id));
        if id != self.hover_app {
            if let Some((pid, _)) = self.hover_app {
                sys_ipc_send(pid, MSG_MOUSE_LEAVE, 0, 0);
            }
            self.hover_app = id;
            // Force the entering window's first move out on this tick rather than at the next
            // meter interval: entering a window and seeing nothing light up for 16 ms reads badly.
            self.hover_app_pos = (i32::MIN, i32::MIN);
        }

        let Some(i) = target else { return };
        let r = self.clients[i].win.r;
        let pos = (self.mx - r.x, self.my - r.y);
        if pos == self.hover_app_pos {
            return;
        }
        // One per frame period. The dedup above already caps this at the mouse's sample rate, which
        // is still several times what an app on a 16 ms loop can drain.
        if self.hover_app_pos != (i32::MIN, i32::MIN)
            && now.wrapping_sub(self.hover_app_ms) < 1000 / 60
        {
            return;
        }
        self.hover_app_pos = pos;
        self.hover_app_ms = now;
        sys_ipc_send(self.clients[i].owner_pid, MSG_MOUSE_MOVE, pos.0 as u64, pos.1 as u64);
    }

    fn update_hover(&mut self) {
        // The two modal surfaces take the pointer entirely; nothing behind them can be hovered.
        if self.cmd_open {
            let kinds = self.kinds();
            let new = layout::command_hit(self.screen_w, &kinds, self.mx, self.my);
            if new != self.cmd_hover {
                self.cmd_hover = new;
                self.mark_command();
            }
            return;
        }
        if self.ent_open {
            let new = self.entity_geometry().control_hit(self.mx, self.my);
            if new != self.ent_hover {
                self.ent_hover = new;
                self.mark_entity();
            }
            return;
        }

        let captured = self.dragging.is_some() || self.resizing.is_some() || self.scrolling.is_some();

        let new_win = if captured { None } else { self.window_at(self.mx, self.my) };
        if new_win != self.hover_win {
            for w in [self.hover_win, new_win] {
                if let Some(i) = w {
                    if i < self.clients.len() {
                        let r = self.clients[i].win.r;
                        self.mark_win(r);
                    }
                }
            }
            self.hover_win = new_win;
        }

        let new_ctl = match new_win {
            Some(i) if i < self.clients.len() => {
                layout::control_hit(self.clients[i].win.r, self.mx, self.my).map(|c| (i, c))
            }
            _ => None,
        };
        if new_ctl != self.hover_ctl {
            if let Some((i, _)) = self.hover_ctl.or(new_ctl) {
                if i < self.clients.len() {
                    let r = self.clients[i].win.r;
                    self.mark_win(r);
                }
            }
            self.hover_ctl = new_ctl;
        }

        let new_dock = if captured { None } else { layout::dock_hit(self.screen_h, self.mx, self.my) };
        if new_dock != self.hover_dock {
            self.hover_dock = new_dock;
            self.mark_dock();
        }
    }

    /// Step every running animation by one frame.
    ///
    /// One place, once per frame, so a transition cannot advance twice in a frame that took two
    /// passes — and so "is anything moving?" is answerable by reading one loop.
    ///
    /// The open fade used to be a linear `opacity += 15` here. At eight frames linear reads as
    /// sluggish, which is exactly what the ease-out table in `nyx_meridian::motion` exists to fix.
    fn update(&mut self) {
        let focused = self.active();
        for i in 0..self.clients.len() {
            if !self.clients[i].win.exists {
                continue;
            }
            let mut moved = false;

            if self.clients[i].win.open.tick() {
                self.clients[i].win.opacity = self.clients[i].win.open.lerp_u8(0, 255);
                moved = true;
            }

            // The controls track hover-or-focus. Retargeting mid-flight restarts the fade rather
            // than reversing from where it had got to; at 80 ms the difference is not visible, and
            // reversing needs the curve run backwards from an arbitrary point.
            let want = self.hover_win == Some(i) || focused == Some(i);
            if want != self.clients[i].win.ctl_shown {
                self.clients[i].win.ctl_shown = want;
                self.clients[i].win.ctl.restart();
            }
            if self.clients[i].win.ctl.tick() {
                moved = true;
            }

            if moved {
                let r = self.clients[i].win.r;
                self.mark_win(r);
            }
        }
    }


    // ── the OSD ──

    /// Step the panel backlight and report what to show.
    ///
    /// The kernel refuses to touch a display that is not PWM-driven, and says so by returning
    /// nothing — an external monitor has no backlight to control, and the honest popup states that
    /// rather than animating a track that means nothing.
    fn step_brightness(&mut self, delta: i32) -> Osd {
        let got = sys_backlight(delta);
        // Keep the Entity's row honest if it happens to be open — the key and the slider are two
        // views of one number, and the OSD deliberately does not dismiss the Entity, so they can be
        // on screen together.
        if got != self.bright_pct {
            self.bright_pct = got;
            if self.ent_open {
                let s = self.entity_geometry().surface;
                self.mark_dirty(s.x, s.y, s.w, s.h);
            }
        }
        match got {
            Some(p) => Osd::Brightness(p),
            None => Osd::BrightnessUnavailable,
        }
    }

    /// Raise the OSD, or refresh the one already up.
    ///
    /// Repeated taps extend the hold rather than restarting an animation, which is what makes
    /// holding a brightness key feel like one continuous adjustment instead of a strobe.
    fn show_osd(&mut self, o: Osd) {
        let until = sys_get_time() + OSD_HOLD_MS;
        self.osd = Some((o, until));
        self.mark_osd();
    }

    /// Set the panel from where the pointer is along the Entity's brightness track.
    ///
    /// The kernel is the authority on what actually happened — it clamps to a 5% floor — so the
    /// cached level is whatever it reports back, never what we asked for. Believing our own request
    /// is how a slider ends up showing 0% on a panel that is really at 5.
    fn set_bright_from_pointer(&mut self, mx: i32) {
        let l = self.entity_geometry();
        let Some(track) = l.control_track(ENT_BRIGHT_ROW) else { return };
        // ★ Snap to the same 5% grid the keys use, and skip the call when nothing changed.
        //
        // Not cosmetic. On this laptop a set goes through ACPI `_BCM`, which ends in an SMI — the
        // firmware freezes the machine to service it. Driving that from raw pointer motion would fire
        // one per mouse move, hundreds during a single drag. Quantising caps a full-width drag at
        // twenty, and the guard below makes a slow drag across one step cost nothing at all.
        let raw = layout::track_percent(track, mx);
        let pct = ((raw + 2) / 5 * 5).clamp(0, 100) as u32;
        if self.bright_pct == Some(pct) {
            return;
        }
        let got = sys_backlight_set(pct);
        if got != self.bright_pct {
            self.bright_pct = got;
            let s = l.surface;
            self.mark_dirty(s.x, s.y, s.w, s.h);
        }
    }

    /// Start asking which F-keys carry the brightness symbols.
    fn begin_learn(&mut self) {
        self.learn = Some(Learn::Dimmer);
        self.show_osd(Osd::Learn(Learn::Dimmer));
    }

    /// One keypress while a binding is being captured.
    ///
    /// Only F-keys are accepted. Every other key is swallowed rather than passed through: the prompt
    /// is modal, and a stray letter reaching the focused editor while the user is hunting for the
    /// right key is a surprising way to lose a document.
    fn capture_bright_key(&mut self, step: Learn, key: char) {
        if key == '\x1b' {
            self.learn = None;
            self.show_osd(Osd::Bound(self.bright));
            return;
        }
        let Some(n) = keys::fkey(key) else {
            // Not an F-key — echo it and keep waiting. Silently swallowing it would make a key that
            // arrived look identical to a key that never left the keyboard, which is precisely the
            // question this prompt is being used to answer.
            self.show_osd(Osd::Saw(key));
            return;
        };
        match step {
            Learn::Dimmer => {
                self.learn = Some(Learn::Brighter(n));
                self.show_osd(Osd::Learn(Learn::Brighter(n)));
            }
            Learn::Brighter(down) => {
                // One key cannot be both directions. Rather than reject the press and leave the user
                // guessing what went wrong, treat it as re-answering the first question.
                if n == down {
                    self.show_osd(Osd::Learn(Learn::Brighter(down)));
                    return;
                }
                let b = BrightKeys { down, up: n };
                self.bright = b;
                self.learn = None;
                b.save();
                self.show_osd(Osd::Bound(b));
            }
        }
    }

    /// Retire the OSD once its hold has run out.
    fn tick_osd(&mut self, now: usize) {
        // A question stays on screen until it is answered. Everything else is a report, and a report
        // has said what it has to say after 1.2 s.
        if self.learn.is_some() {
            return;
        }
        if let Some((_, until)) = self.osd {
            if now >= until {
                // Mark BEFORE clearing: the damage is where the panel was, and once `osd` is None
                // there is nothing left to compute that rectangle from.
                self.mark_osd();
                self.osd = None;
            }
        }
    }

    /// Dirty just the OSD panel. The whole point of the surface is that it costs one small
    /// rectangle rather than a full-screen pass to announce a volume change.
    fn mark_osd(&mut self) {
        const M: i32 = 8;
        let p = layout::osd_panel(self.screen_w, self.screen_h);
        self.mark_dirty(p.x - M, p.y - M, p.w + M * 2, p.h + M * 2);
    }

    // ── wallpaper ──

    /// The wallpaper colour at a screen pixel.
    ///
    /// The texture is 256x256 stretched fullscreen, so this mirrors the GPU's own nearest sample. It
    /// only has to be right to within a pixel: the wallpaper is deliberately low-frequency (adjacent
    /// texels differ by at most 1/255 — there is a test), which is precisely what lets Meridian's
    /// flat alphas read as material without a blur pass.
    fn paper_at(&self, x: usize, y: usize) -> u32 {
        if self.paper.is_empty() {
            return self.theme.void;
        }
        let sx = (x * paper::PAPER) / (self.screen_w.max(1) as usize);
        let sy = (y * paper::PAPER) / (self.screen_h.max(1) as usize);
        self.paper[sy.min(paper::PAPER - 1) * paper::PAPER + sx.min(paper::PAPER - 1)]
    }

    /// What shows through the carved corner of window `idx` at `(x, y)`: the topmost window BELOW it
    /// if there is one, otherwise the wallpaper.
    ///
    /// Passing a single flat colour here — which is what the old desktop's carve does — paints four
    /// visible patches of the wrong shade at every window corner, because Meridian's ground is a
    /// gradient and its windows overlap.
    fn ground_at(&self, idx: usize, x: usize, y: usize) -> u32 {
        let (px, py) = (x as i32, y as i32);
        for i in (0..idx).rev() {
            let c = &self.clients[i];
            if !c.win.exists || c.win.folded || c.buffer.is_null() {
                continue;
            }
            let r = c.win.r;
            if !r.contains(px, py) || r.w <= 0 || r.h <= 0 {
                continue;
            }
            // The GPU stretches buf_w x buf_h onto r.w x r.h during a live resize; sample the same
            // way so the carve matches what is actually on screen.
            let sx = ((px - r.x) as usize * c.buf_w) / r.w as usize;
            let sy = ((py - r.y) as usize * c.buf_h) / r.h as usize;
            if sx >= c.buf_w || sy >= c.buf_h {
                continue;
            }
            let pixels = unsafe { core::slice::from_raw_parts(c.buffer, c.buf_w * c.buf_h) };
            return 0xFF00_0000 | (pixels[sy * c.buf_w + sx] & 0x00FF_FFFF);
        }
        self.paper_at(x, y)
    }
}

// ─────────────────────────────── Entry ───────────────────────────────

fn push_num(buf: &mut [u8], mut pos: usize, mut v: usize) -> usize {
    if v == 0 {
        buf[pos] = b'0';
        return pos + 1;
    }
    let mut tmp = [0u8; 20];
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
    pos
}

/// Build the wallpaper texture, publish it to the GPU, and return `(gva, cpu_vaddr)` — `(0, 0)` on
/// failure.
///
/// The CPU mapping comes back because a theme change rewrites this texture IN PLACE. Allocating a
/// fresh surface per toggle would leak one every time, and remapping a live GVA is the class of bug
/// this project has already paid for once.
///
/// The GVA sits above the window GVAs (`0x2000_0000 + id * 0x0100_0000`) and above the glyph atlas
/// (`0x6400_0000`, 1024 x ~180). Every RCS consumer needs its own GVA range.
const PAPER_GVA: u32 = 0x6800_0000;

fn publish_paper(pixels: &[u32]) -> (u32, u64) {
    let bytes = pixels.len() * 4;
    let shm = sys_create_shm(bytes);
    if shm == 0 {
        return (0, 0);
    }
    let vaddr = sys_map_shm(shm);
    if vaddr == 0 {
        return (0, 0);
    }
    let dst = unsafe { core::slice::from_raw_parts_mut(vaddr as *mut u32, pixels.len()) };
    dst.copy_from_slice(pixels);
    sys_gpu_map_shm(shm, PAPER_GVA);
    (PAPER_GVA, vaddr)
}

#[no_mangle]
#[link_section = ".text.entry"]
pub extern "C" fn _start() -> ! {
    const HEAP_PAGES: usize = 4096;
    let heap_start = sys_alloc_pages(HEAP_PAGES);
    if heap_start == 0 {
        sys_exit(1);
    }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, HEAP_PAGES * 4096) };

    let (screen_w, screen_h, screen_stride) = sys_get_screen_info();
    let fb_ptr = sys_map_framebuffer();
    let hardware_fb =
        unsafe { core::slice::from_raw_parts_mut(fb_ptr as *mut u32, screen_stride * screen_h) };

    // ★ FIRST, before anything measures anything. The design is drawn against a 1440x900 scene; on a
    // 1080p panel that is 20% too small and on a 4K one it is unusable. Every dimension in `layout`
    // and every type size in `tokens` reads this, and the atlas rasterizes glyphs and packs icons at
    // the scaled sizes — so setting it after the atlas is built would leave every glyph the wrong
    // size with nothing on screen to say why.
    let ui_scale = scale::init(screen_w as i32, screen_h as i32);
    {
        let mut m = [0u8; 80];
        let mut p = 0usize;
        for &b in b"[SHELL] " {
            m[p] = b;
            p += 1;
        }
        p = push_num(&mut m, p, screen_w);
        m[p] = b'x';
        p += 1;
        p = push_num(&mut m, p, screen_h);
        for &b in b" -> ui scale " {
            m[p] = b;
            p += 1;
        }
        p = push_num(&mut m, p, ui_scale as usize);
        for &b in b"/1000\n" {
            m[p] = b;
            p += 1;
        }
        if let Ok(s) = core::str::from_utf8(&m[..p]) {
            sys_print(s);
        }
    }

    let mut state = Shell::new(screen_w as i32, screen_h as i32, screen_stride);

    state.hw_cursor = sys_cursor_init();
    if state.hw_cursor {
        upload_cursor(Shape::Pointer);
        sys_print("[SHELL] hardware cursor enabled\n");
    } else {
        sys_print("[SHELL] hardware cursor unavailable; software cursor\n");
    }

    // Register Inter + JetBrains Mono into nyx-gui's face registry BEFORE building the atlas —
    // otherwise every cell rasterizes from the DejaVu fallback and Meridian's typography is silently
    // absent, which looks like a wrong-looking desktop rather than a missing font.
    let font_failures = font::register_all();
    {
        let mut m = [0u8; 64];
        let mut p = 0usize;
        for &b in b"[SHELL] faces registered, failures=" {
            m[p] = b;
            p += 1;
        }
        p = push_num(&mut m, p, font_failures);
        m[p] = b'\n';
        p += 1;
        if let Ok(s) = core::str::from_utf8(&m[..p]) {
            sys_print(s);
        }
    }

    let atlas = Atlas::build();
    match &atlas {
        Some(a) => {
            let mut m = [0u8; 96];
            let mut p = 0usize;
            for &b in b"[SHELL] atlas " {
                m[p] = b;
                p += 1;
            }
            p = push_num(&mut m, p, a.w as usize);
            m[p] = b'x';
            p += 1;
            p = push_num(&mut m, p, a.h as usize);
            for &b in b" pitch=" {
                m[p] = b;
                p += 1;
            }
            p = push_num(&mut m, p, a.pitch as usize);
            m[p] = b'\n';
            p += 1;
            if let Ok(s) = core::str::from_utf8(&m[..p]) {
                sys_print(s);
            }
        }
        None => sys_print("[SHELL] atlas unavailable; CPU text only\n"),
    }

    state.paper = paper::generate(&state.theme);
    let (gva, vaddr) = publish_paper(&state.paper);
    state.paper_gva = gva;
    state.paper_vaddr = vaddr;
    if state.paper_gva == 0 {
        sys_print("[SHELL] wallpaper texture unavailable; flat ground\n");
    }

    // Sample the machine once before the first frame, so the Entity opens with a reading rather
    // than with zeros.
    state.refresh_vitals(atlas.as_ref());

    let mut last_frame = sys_get_time();
    let ms_per_frame = 1000 / 60;

    sys_print("[SHELL] Meridian online.\n");

    loop {
        let now = sys_get_time();
        state.process_ipc();
        state.process_input(now);
        state.update();
        state.tick_osd(now);

        // Cold start, stage 3. Runs for 22 frames on the first second of the machine's life and
        // then never again — so it simply forces a full frame while it runs, rather than scissoring
        // a mark that moves AND shrinks every frame. That would need the union of two rects per
        // frame and would buy nothing on a screen being composited from scratch anyway.
        if state.handoff.step(now) {
            state.needs_redraw = true;
            state.mark_full();
        }

        // 1 Hz heartbeat. ⚠️ Do NOT turn this into a partial repaint of just the chrome. That was
        // tried on the old compositor and reverted: letting the RENDER engine idle into RC6 loses
        // MOCS and forcewake, and the first composite afterwards hangs in `wait_for_idle` while the
        // hardware cursor keeps moving — indistinguishable from a freeze. A full GPU frame every
        // second is what keeps the engine warm.
        let now_sec = now / 1000;
        if now_sec != state.last_clock_sec {
            state.last_clock_sec = now_sec;
            // Re-sample the machine on the same tick. Everything the Entity reports — temperature,
            // load, the link, the clock — changes on human timescales, and this frame is being
            // fully recomposited regardless.
            state.refresh_vitals(atlas.as_ref());
            state.needs_redraw = true;
        }

        if !state.needs_redraw && now.wrapping_sub(last_frame) < ms_per_frame {
            sys_sleep_ms(2);
            continue;
        }
        last_frame = now;
        if !state.needs_redraw {
            continue;
        }

        let frame_start = now;
        if !state.hw_cursor {
            state.mark_dirty(state.mx - 15, state.my - 15, 35, 35);
        }
        render(&mut state, hardware_fb, atlas.as_ref());

        let frame_end = sys_get_time();
        state.last_frame_ms = frame_end.wrapping_sub(frame_start);
        state.frame_count = state.frame_count.wrapping_add(1);
        state.fps_window_frames += 1;
        if frame_end.wrapping_sub(state.fps_window_start) >= 1000 {
            state.fps = state.fps_window_frames;
            state.fps_window_frames = 0;
            state.fps_window_start = frame_end;
            // One serial baseline, then silence — the 1 Hz tick must not bury the boot lines.
            if state.fps_logs < 1 {
                state.fps_logs += 1;
                let mut log = [0u8; 96];
                let mut lp = 0usize;
                for &b in b"[UI] frame " {
                    log[lp] = b;
                    lp += 1;
                }
                lp = push_num(&mut log, lp, state.frame_count as usize);
                for &b in b": draw=" {
                    log[lp] = b;
                    lp += 1;
                }
                lp = push_num(&mut log, lp, state.last_frame_ms);
                for &b in b"ms fps=" {
                    log[lp] = b;
                    lp += 1;
                }
                lp = push_num(&mut log, lp, state.fps);
                if let Some(a) = atlas.as_ref() {
                    let (n, _) = a.misses();
                    for &b in b" glyph_misses=" {
                        log[lp] = b;
                        lp += 1;
                    }
                    lp = push_num(&mut log, lp, n as usize);
                }
                log[lp] = b'\n';
                lp += 1;
                if let Ok(s) = core::str::from_utf8(&log[..lp]) {
                    sys_print(s);
                }
            }
        }

        state.prev_mx = state.mx;
        state.prev_my = state.my;
        state.dirty_min_x = state.screen_stride;
        state.dirty_min_y = state.screen_h as usize;
        state.dirty_max_x = 0;
        state.dirty_max_y = 0;
        state.needs_redraw = false;
    }
}

/// One frame.
fn render(state: &mut Shell, fb: &mut [u32], atlas: Option<&Atlas>) {
    let screen_w = state.screen_w as usize;
    let screen_h = state.screen_h as usize;
    let stride = state.screen_stride;
    let theme = state.theme;

    // ── damage: full frame or a scissored region? ──
    // A full frame is forced when the box is empty (the 1 Hz heartbeat — see the loop header for why
    // that must stay a full GPU frame), when anything is mid-fade (a scissored present could clip a
    // translucent quad and the next partial frame would double-blend it), or when the damage already
    // covers most of the screen and the partial path saves nothing.
    let rect_empty =
        state.dirty_max_x <= state.dirty_min_x || state.dirty_max_y <= state.dirty_min_y;
    let fading = state
        .clients
        .iter()
        .any(|c| c.win.exists && !c.win.folded && c.win.opacity < 255);
    let dmg_x = state.dirty_min_x.min(screen_w);
    let dmg_y = state.dirty_min_y.min(screen_h);
    let dmg_w = state.dirty_max_x.min(screen_w).saturating_sub(dmg_x);
    let dmg_h = state.dirty_max_y.min(screen_h).saturating_sub(dmg_y);
    let full_frame = rect_empty
        || fading
        || dmg_w.saturating_mul(dmg_h).saturating_mul(4) >= screen_w.saturating_mul(screen_h) * 3;

    // 1. Ground. The wallpaper quad covers this on the GPU path, but the fill also clears the
    //    scissor region and is what the CPU fallback lands on.
    if full_frame {
        sys_gpu_fill_rect(0, 0, stride, screen_h, theme.void);
    } else {
        sys_gpu_fill_rect(dmg_x, dmg_y, dmg_w, dmg_h, theme.void);
    }
    sys_gpu_sync();

    // 2. Composite: the wallpaper as the bottom quad, then every visible window's content, back to
    //    front. The wallpaper being a quad rather than a CPU blit is what makes it affordable — it
    //    is a 256x256 texture stretched fullscreen by the sampler.
    let hdr = core::mem::size_of::<WindowHeader>() as u32;
    let mut quads: Vec<WindowQuad> = Vec::new();
    if state.paper_gva != 0 {
        quads.push(WindowQuad {
            tex_gva: state.paper_gva,
            src_w: paper::PAPER as u32,
            src_h: paper::PAPER as u32,
            src_pitch: (paper::PAPER * 4) as u32,
            dst_x: 0,
            dst_y: 0,
            dst_w: screen_w as u32,
            dst_h: screen_h as u32,
            opacity: 255,
        });
    }
    for c in state.clients.iter() {
        if c.win.exists && !c.win.folded && c.gpu_gva != 0 {
            quads.push(WindowQuad {
                tex_gva: c.gpu_gva + hdr,
                src_w: c.buf_w as u32,
                src_h: c.buf_h as u32,
                src_pitch: (c.buf_w as u32) * 4,
                dst_x: c.win.r.x,
                dst_y: c.win.r.y,
                // Draw at the SHELL size, not the buffer size, so a resize drag follows the pointer
                // instantly; it sharpens when the app reallocates its SHM.
                dst_w: c.win.r.w as u32,
                dst_h: c.win.r.h as u32,
                opacity: c.win.opacity as u32,
            });
        }
    }
    let gpu_composited = sys_gpu_composite(&quads);
    if gpu_composited {
        sys_gpu_sync();
    }

    let mut canvas = Canvas::new(fb, stride, screen_h);

    // 3. CPU fallback for the composite. Flat ground rather than a scaled wallpaper: stretching
    //    256x256 to the panel on the CPU every frame is not affordable, and a flat `--void` desktop
    //    is a working one.
    if !gpu_composited {
        canvas.fill_rect(0, 0, stride, screen_h, theme.void);
        for c in state.clients.iter() {
            if !c.win.exists || c.win.folded || c.buffer.is_null() {
                continue;
            }
            let pixels = unsafe { core::slice::from_raw_parts(c.buffer, c.buf_w * c.buf_h) };
            canvas.composite_buffer(
                c.win.r.x.max(0) as usize,
                c.win.r.y.max(0) as usize,
                pixels,
                c.buf_w,
                c.buf_h,
                c.win.opacity,
            );
        }
    }

    let active = state.active();
    let mut labels: Vec<Label> = Vec::new();
    let mut icons_out: Vec<IconDraw> = Vec::new();

    // 4. Window chrome. There is barely any: a hairline round the surface, the corners carved to
    //    whatever is behind, and a caption line above with two controls that appear on hover.
    for i in 0..state.clients.len() {
        if !state.clients[i].win.exists {
            continue;
        }
        let r = state.clients[i].win.r;
        let folded = state.clients[i].win.folded;
        let focused = active == Some(i);

        if !folded {
            // Round the composited quad. `ground_at` supplies the colour behind each carved pixel,
            // which is the wallpaper on the desktop and the window below where they overlap.
            {
                let s = &*state;
                shapes::carve_round_corners(
                    &mut canvas,
                    r.x.max(0) as usize,
                    r.y.max(0) as usize,
                    r.w as usize,
                    r.h as usize,
                    layout::window_radius(),
                    |x, y| s.ground_at(i, x, y),
                );
            }
            shapes::stroke_round_rect(
                &mut canvas,
                r.x.max(0) as usize,
                r.y.max(0) as usize,
                r.w as usize,
                r.h as usize,
                layout::window_radius(),
                if focused { theme.line_2 } else { theme.line },
            );

            let (content_h, scroll_off) = client_scroll(&state.clients[i]);
            if content_h > r.h as usize {
                draw_scrollbar(
                    &mut canvas,
                    (r.right() - SCROLLBAR_W as i32).max(0) as usize,
                    r.y.max(0) as usize,
                    SCROLLBAR_W,
                    r.h as usize,
                    content_h,
                    r.h as usize,
                    scroll_off,
                );
            }
        } else {
            // A folded window is a caption line over a hairline, with a 3px accent tick. Zero quads.
            let rule = layout::folded_rule(r);
            shapes::hairline_h(
                &mut canvas,
                rule.x.max(0) as usize,
                rule.y.max(0) as usize,
                rule.w as usize,
                theme.line_2,
            );
            canvas.fill_rect(
                rule.x.max(0) as usize,
                rule.y.max(0) as usize,
                layout::fold_tick() as usize,
                1,
                theme.accent,
            );
        }

        // The caption. Its name sits OUTSIDE the surface — this is the single most visible thing
        // about Meridian, and it is why a window needs no title bar at all.
        let cap = layout::caption(r);
        labels.push(Label {
            x: cap.x + layout::caption_pad_x(),
            y: centre_text_y(cap.y, cap.h, tokens::CAPTION),
            style: tokens::CAPTION,
            text: String::from(state.clients[i].win.name()),
            color: if focused { theme.fg } else { theme.fg_3 },
        });

        // Controls appear only on hover or focus, and they FADE — `opacity 0→1` over 5 frames.
        // There are two: FOLD and CLOSE. Maximise is a double-click on the caption and gets no
        // glyph.
        //
        // The fade is done in colour rather than in alpha because these ride the batched glyph
        // path, and `build_ps_text` carries one luminance per run — there is no per-quad alpha to
        // turn down. Mixing toward the ground is the same picture and costs nothing extra.
        let w = &state.clients[i].win;
        let ctl_t = if w.ctl_shown { w.ctl.progress() } else { UNIT - w.ctl.progress() };
        if ctl_t > 0 {
            let t = (ctl_t * 255 / UNIT) as u32;
            for (ci, ctl) in layout::CONTROLS.iter().enumerate() {
                let b = match layout::control(r, ci) {
                    Some(b) => b,
                    None => continue,
                };
                let on = state.hover_ctl == Some((i, *ctl));
                if on {
                    shapes::fill_round_rect(
                        &mut canvas,
                        b.x.max(0) as usize,
                        b.y.max(0) as usize,
                        b.w as usize,
                        b.h as usize,
                        4,
                        shapes::lerp_rgb(theme.surface, theme.wash, t),
                    );
                }
                icons_out.push(IconDraw {
                    id: match ctl {
                        Control::Fold => Icons::WIN_FOLD,
                        Control::Close => Icons::WIN_CLOSE,
                    },
                    px: layout::ctl_icon() as usize,
                    x: b.x + (b.w - layout::ctl_icon()) / 2,
                    y: b.y + (b.h - layout::ctl_icon()) / 2,
                    color: shapes::lerp_rgb(
                        theme.surface,
                        if on { theme.fg } else { theme.fg_3 },
                        t,
                    ),
                });
            }
        }
    }

    // 5. Shadows, last, so each window's shadow falls on the windows BELOW it. Only the focused
    //    window casts one — the design uses the shadow to say which window is focused, so putting
    //    one under every window would say nothing.
    if let Some(i) = active {
        let r = state.clients[i].win.r;
        let mut above: Vec<(i32, i32, i32, i32)> = Vec::new();
        for j in (i + 1)..state.clients.len() {
            let u = &state.clients[j].win;
            if u.exists && !u.folded {
                above.push((u.r.x, u.r.y, u.r.right(), u.r.bottom()));
            }
        }
        if !state.clients[i].win.folded && state.clients[i].win.opacity > 40 {
            shapes::drop_shadow(&mut canvas, r.x, r.y, r.w, r.h, layout::window_radius(), &above);
        }
    }

    // 6. The dock: eight glyphs resting on the wallpaper. No container — not a pill, not a bar.
    for slot in 0..layout::DOCK_SLOTS {
        let s = match layout::dock_slot(state.screen_h, slot) {
            Some(s) => s,
            None => continue,
        };
        if layout::is_divider(slot) {
            shapes::hairline_v(&mut canvas, s.x as usize, s.y as usize, s.h as usize, theme.line);
            continue;
        }
        let app = match DOCK[slot] {
            Some(i) => &APPS[i],
            None => continue,
        };
        let (running, is_active) = state.dock_state(slot);
        let hovered = state.hover_dock == Some(slot);
        icons_out.push(IconDraw {
            id: app.icon,
            px: layout::dock_icon() as usize,
            x: s.x,
            y: s.y,
            color: if is_active {
                theme.fg
            } else if hovered || running {
                theme.fg_2
            } else {
                theme.fg_4
            },
        });
        // The running dot. Accent-tinted when active, so it is a CPU `fill_rect` and not a glyph —
        // the batched text shader carries one luminance and would render the accent as grey.
        if running {
            if let Some(d) = layout::dock_dot(state.screen_h, slot) {
                canvas.fill_rect(
                    d.x as usize,
                    d.y as usize,
                    d.w as usize,
                    d.h as usize,
                    if is_active { theme.accent } else { theme.fg_4 },
                );
            }
        }
    }

    // 7. The Entity: one 22px mark and a two-word phrase at bottom-right. This is the entire
    //    permanent chrome on that side of the screen — there is no tray, because this IS the tray.
    let v = &state.vitals;
    let mark = layout::entity_mark(state.screen_w, state.screen_h);
    {
        let pw = measure(atlas, tokens::B3, v.phrase);
        let p = layout::entity_phrase(state.screen_w, state.screen_h, pw);
        labels.push(Label {
            x: p.x,
            y: centre_text_y(p.y, p.h, tokens::B3),
            style: tokens::B3,
            text: String::from(v.phrase),
            color: if v.mood == Mood::Attention { theme.fg_2 } else { theme.fg_3 },
        });
    }
    if v.mood == Mood::Attention {
        // ★ THE ONE DOCUMENTED EXCEPTION. In the attention state the mark is accent-tinted, and a
        // tinted glyph cannot ride the batched text path: `build_ps_text` interpolates a single
        // luminance across the quad, so that shader is greyscale-only and the accent would arrive as
        // grey. So this one mark leaves the fast path and is blended CPU-side — a single 22px glyph
        // per frame, against a budget of about a million pixels. Every other accent in Meridian is a
        // `fill_rect` (the caret, the running dot, the selection bar) and is unaffected.
        if let Some(ic) = icons::get(v.mood.glyph(), layout::entity_mark_px() as usize) {
            let px: Vec<u32> = ic.cov.iter().map(|&c| (c as u32) << 24).collect();
            canvas.blit_rgba(
                (mark.x + ic.off_x as i32).max(0) as usize,
                (mark.y + ic.off_y as i32).max(0) as usize,
                &px,
                ic.w as usize,
                ic.h as usize,
                Some(theme.accent),
            );
        }
    } else {
        icons_out.push(IconDraw {
            id: v.mood.glyph(),
            px: layout::entity_mark_px() as usize,
            x: mark.x,
            y: mark.y,
            color: theme.fg_3,
        });
    }

    // 7b. The Entity surface, when disclosed: a statement, then measurements separated by hairlines
    //     and space. Not a card grid.
    if state.ent_open {
        let l = state.entity_geometry();
        shapes::drop_shadow(&mut canvas, l.surface.x, l.surface.y, l.surface.w, l.surface.h,
                            layout::surface_radius(), &[]);
        shapes::fill_round_rect(&mut canvas, l.surface.x.max(0) as usize, l.surface.y.max(0) as usize,
                                l.surface.w as usize, l.surface.h as usize,
                                layout::surface_radius(), theme.chrome);
        shapes::stroke_round_rect(&mut canvas, l.surface.x.max(0) as usize, l.surface.y.max(0) as usize,
                                  l.surface.w as usize, l.surface.h as usize,
                                  layout::surface_radius(), theme.line_2);
        for y in l.rules() {
            shapes::hairline_h(&mut canvas, l.surface.x as usize, y.max(0) as usize,
                               l.surface.w as usize, theme.line);
        }

        labels.push(Label {
            x: l.statement.x,
            y: l.statement.y,
            style: tokens::D3,
            text: String::from(v.phrase),
            color: theme.fg,
        });
        // Same cap the layout was measured with, or the text would run past the box it sized.
        for (n, line) in nyx_meridian::atlas::wrap(&v.detail, l.detail.w as usize, |s| {
            measure(atlas, tokens::B3, s) as usize
        })
        .iter()
        .take(state.ent_detail_lines.max(0) as usize)
        .enumerate()
        {
            labels.push(Label {
                x: l.detail.x,
                y: l.detail.y + n as i32 * layout::ent_detail_line(),
                style: tokens::B3,
                text: String::from(*line),
                color: theme.fg_3,
            });
        }

        // Measurements: a key on the left, a value right-aligned. Every one is a real reading — see
        // `Vitals` for what this machine can and cannot answer.
        for (i, (k, val)) in v.measures(state.fps).iter().enumerate() {
            let row = match l.measure_row(i) {
                Some(r) => r,
                None => continue,
            };
            labels.push(Label {
                x: row.x,
                y: centre_text_y(row.y, row.h, tokens::B3),
                style: tokens::B3,
                text: String::from(*k),
                color: theme.fg_3,
            });
            let vw = measure(atlas, tokens::B2, val);
            labels.push(Label {
                x: row.right() - vw,
                y: centre_text_y(row.y, row.h, tokens::B2),
                style: tokens::B2,
                text: val.clone(),
                color: theme.fg,
            });
        }

        // Controls. Volume still has no driver behind it and says so rather than showing a
        // plausible number — the Entity is the thing you are supposed to trust about the machine.
        //
        // Power is now real: read straight from the embedded controller (ports 0x930/0x934, register
        // map decoded from this laptop's own DSDT). Verified against Fedora on the same hardware.
        // Percentage is charge/LAST-FULL, not design — a worn pack should read 100% when it is as
        // full as it can get, which is what every other OS shows and what this one is at 47% health.
        let bat = sys_battery();
        let power_value = match (bat.present, bat.percent()) {
            (false, _) | (_, None) => String::from("No battery"),
            (true, Some(p)) => {
                if bat.critical() {
                    alloc::format!("{}% · critical", p)
                } else if bat.charging() {
                    alloc::format!("{}% · charging", p)
                } else {
                    // Time-to-empty ONLY while genuinely discharging and drawing current.
                    // `minutes_remaining` returns None otherwise, and inventing an estimate from a
                    // zero rate is exactly the kind of plausible-but-false number this surface
                    // exists not to show.
                    match bat.minutes_remaining() {
                        Some(m) => alloc::format!("{}% · {}h {:02}m left", p, m / 60, m % 60),
                        None => alloc::format!("{}%", p),
                    }
                }
            }
        };
        //
        // Brightness is the fourth, and is an extension: the design stops at three. It earns the
        // place because it is the one system control on this machine that has real hardware behind
        // it AND no key that can reach it — the brightness keycaps are `Fn` chords the embedded
        // controller eats. Without a row here, a laptop whose F-key binding is wrong has no way at
        // all to change its own screen.
        let bright_value = match state.bright_pct {
            Some(p) => alloc::format!("{}%", p),
            None => String::from("No panel backlight"),
        };
        let controls: [(&str, &str, String); ENT_CONTROLS] = [
            (Icons::WIFI, "Network", v.network_value()),
            (Icons::POWER, "Power", power_value),
            (Icons::VOLUME, "Volume", String::from("No audio device")),
            (Icons::BRIGHT, "Brightness", bright_value),
        ];
        for (i, (icon, k, val)) in controls.iter().enumerate() {
            let row = match l.control_row(i) {
                Some(r) => r,
                None => continue,
            };
            if state.ent_hover == Some(i) {
                shapes::fill_round_rect(&mut canvas, row.x.max(0) as usize, row.y.max(0) as usize,
                                        row.w as usize, row.h as usize, 5, theme.wash);
            }
            icons_out.push(IconDraw {
                id: icon,
                px: layout::ent_crow_icon() as usize,
                x: row.x + layout::ent_crow_pad_x(),
                y: row.y + (row.h - layout::ent_crow_icon()) / 2,
                color: theme.fg_3,
            });
            labels.push(Label {
                x: row.x + layout::ent_crow_pad_x() + 16 + 12,
                y: centre_text_y(row.y, row.h, tokens::B2),
                style: tokens::B2,
                text: String::from(*k),
                color: theme.fg,
            });
            let vw = measure(atlas, tokens::B3, val);
            labels.push(Label {
                x: row.right() - layout::ent_crow_pad_x() - vw,
                y: centre_text_y(row.y, row.h, tokens::B3),
                style: tokens::B3,
                text: val.clone(),
                color: theme.fg_3,
            });

            // The level track, for the one row that has a level. Drawn only when there is genuinely
            // something to report: an empty track under "No panel backlight" would read as zero
            // brightness, which is a different and untrue statement.
            if i == ENT_BRIGHT_ROW {
                if let (Some(pct), Some(t)) = (state.bright_pct, l.control_track(i)) {
                    canvas.fill_rect(
                        t.x.max(0) as usize,
                        t.y.max(0) as usize,
                        t.w as usize,
                        t.h as usize,
                        theme.line_2,
                    );
                    let fill = (t.w * pct as i32 / 100).max(0);
                    if fill > 0 {
                        canvas.fill_rect(
                            t.x.max(0) as usize,
                            t.y.max(0) as usize,
                            fill as usize,
                            t.h as usize,
                            theme.accent,
                        );
                    }
                }
            }
        }

        // The footer, and the only clock in Meridian — there is no tray to put one in.
        let clock = v.clock();
        let cw = measure(atlas, tokens::B2, &clock);
        labels.push(Label {
            x: l.footer.x,
            y: l.footer.y + layout::ent_f_pad_top(),
            style: tokens::B2,
            text: clock,
            color: theme.fg_2,
        });
        labels.push(Label {
            x: l.footer.x + cw + 10,
            y: l.footer.y + layout::ent_f_pad_top(),
            style: tokens::B3,
            text: v.date(),
            color: theme.fg_3,
        });
    }

    // 7c. The Command. Drawn last of all because it is modal: a scrim over the whole desktop, then
    //     one translucent surface. Everything on it is text and coverage glyphs, so a full result
    //     set costs the same single glyph batch as an empty one.
    if state.cmd_open {
        // The scrim is one alpha fill over the desktop. It is a CPU blend rather than a GPU fill
        // because `sys_gpu_fill_rect` writes a solid colour and does not blend — so it is scissored
        // to the damage rect exactly like the background fill above. On a partial frame the GPU only
        // recomposited that rect, so that is the only place the scrim has been undone.
        if full_frame {
            canvas.fill_rect(0, 0, stride, screen_h, theme.scrim);
        } else {
            canvas.fill_rect(dmg_x, dmg_y, dmg_w, dmg_h, theme.scrim);
        }

        let kinds = state.kinds();
        let panel = layout::command_panel(state.screen_w, layout::command_body_height(&kinds));
        shapes::drop_shadow(&mut canvas, panel.x, panel.y, panel.w, panel.h,
                            layout::surface_radius(), &[]);
        shapes::fill_round_rect(&mut canvas, panel.x.max(0) as usize, panel.y.max(0) as usize,
                                panel.w as usize, panel.h as usize,
                                layout::surface_radius(), theme.chrome);
        shapes::stroke_round_rect(&mut canvas, panel.x.max(0) as usize, panel.y.max(0) as usize,
                                  panel.w as usize, panel.h as usize,
                                  layout::surface_radius(), theme.line_2);

        // Query line: an icon, the query at 20px light, a 1px accent caret. There is no rounded
        // search box in this operating system — the hairline under the row is the whole field.
        let q_base = panel.x + layout::cmd_q_pad_x();
        let q_mid = panel.y + layout::cmd_q_pad_top() + 13;
        icons_out.push(IconDraw {
            id: Icons::SEARCH,
            px: layout::cmd_q_icon() as usize,
            x: q_base,
            y: q_mid - layout::cmd_q_icon() / 2,
            color: theme.fg_3,
        });
        let q_text_x = q_base + layout::cmd_q_icon() + layout::cmd_q_gap();
        let query = String::from(state.query());
        let qw = measure(atlas, tokens::D3, &query);
        labels.push(Label {
            x: q_text_x,
            y: panel.y + layout::cmd_q_pad_top(),
            style: tokens::D3,
            text: query,
            color: theme.fg,
        });
        canvas.fill_rect((q_text_x + qw + 2).max(0) as usize, (q_mid - 11).max(0) as usize,
                         1, 22, theme.accent);
        let hint = "esc to dismiss";
        let hw = measure(atlas, tokens::HINT, hint);
        labels.push(Label {
            x: panel.right() - layout::cmd_q_pad_x() - hw,
            y: centre_text_y(q_mid - 7, 14, tokens::HINT),
            style: tokens::HINT,
            text: String::from(hint),
            color: theme.fg_4,
        });
        shapes::hairline_h(&mut canvas, panel.x as usize,
                           (panel.y + layout::cmd_query_h()).max(0) as usize,
                           panel.w as usize, theme.line);

        // Results. Grouped by an 11px uppercase label and separated by space, not rules.
        for (i, hit) in state.hits.iter().enumerate() {
            let row = match layout::command_entry(state.screen_w, &kinds, i) {
                Some(r) => r,
                None => continue,
            };
            if hit.kind == CmdEntry::Group {
                labels.push(Label {
                    x: row.x + layout::cmd_row_pad_x(),
                    y: row.bottom() - layout::cmd_group_pad_bottom() - 14,
                    style: tokens::LB,
                    text: hit.name.clone(),
                    color: theme.fg_4,
                });
                continue;
            }
            let selected = state.cmd_sel == Some(i);
            let hovered = state.cmd_hover == Some(i);
            if selected || hovered {
                shapes::fill_round_rect(&mut canvas, row.x.max(0) as usize, row.y.max(0) as usize,
                                        row.w as usize, row.h as usize, 5, theme.wash);
            }
            if selected {
                // The selection bar: 2px of accent, inset 11px top and bottom. A `fill_rect`, so the
                // accent survives — unlike anything in the glyph batch.
                canvas.fill_rect(row.x.max(0) as usize,
                                 (row.y + layout::cmd_sel_bar_inset()).max(0) as usize,
                                 layout::cmd_sel_bar_w() as usize,
                                 (row.h - layout::cmd_sel_bar_inset() * 2).max(0) as usize,
                                 theme.accent);
            }
            icons_out.push(IconDraw {
                id: hit.icon,
                px: layout::cmd_row_icon() as usize,
                x: row.x + layout::cmd_row_pad_x(),
                y: row.y + (row.h - layout::cmd_row_icon()) / 2,
                color: if selected { theme.fg_2 } else { theme.fg_3 },
            });
            labels.push(Label {
                x: row.x + layout::cmd_row_pad_x() + layout::cmd_row_icon() + layout::cmd_row_gap(),
                y: centre_text_y(row.y, row.h, tokens::B1),
                style: tokens::B1,
                text: hit.name.clone(),
                color: if selected { theme.fg } else { theme.fg_2 },
            });
            let mw = measure(atlas, tokens::B3, &hit.meta);
            labels.push(Label {
                x: row.right() - layout::cmd_row_pad_x() - mw,
                y: centre_text_y(row.y, row.h, tokens::B3),
                style: tokens::B3,
                text: hit.meta.clone(),
                color: theme.fg_4,
            });
        }

        // Nothing matched. Say so — an empty gap between the query and the footer reads as the
        // Command having stopped working rather than as a query with no answer.
        if state.hits.is_empty() {
            let msg = "No results";
            let w = measure(atlas, tokens::B2, msg);
            labels.push(Label {
                x: panel.x + (panel.w - w) / 2,
                y: panel.y + layout::cmd_query_h() + layout::cmd_b_pad_top() + 8,
                style: tokens::B2,
                text: String::from(msg),
                color: theme.fg_4,
            });
        }

        // Footer: the Nyx mark, the key hints, and how much of the result set is showing. That last
        // number is not decoration — the list is capped rather than scrolled, so it is the only
        // thing that says there is more.
        let f_y = panel.bottom() - layout::cmd_footer_h();
        shapes::hairline_h(&mut canvas, panel.x as usize, f_y.max(0) as usize,
                           panel.w as usize, theme.line);
        let f_mid = f_y + layout::cmd_footer_h() / 2;
        icons_out.push(IconDraw {
            id: Icons::MARK,
            px: layout::cmd_footer_mark() as usize,
            x: panel.x + layout::cmd_q_pad_x(),
            y: f_mid - layout::cmd_footer_mark() / 2,
            color: theme.fg_4,
        });
        let mut fx = panel.x + layout::cmd_q_pad_x() + layout::cmd_footer_mark() + 22;
        for (keycap, what) in [("\u{2191}\u{2193}", "navigate"), ("\u{21B5}", "open"), ("\u{21E5}", "refine")] {
            // The key caps are mono, matching `.cmd-f b { font-family: JetBrains Mono }` — they are
            // keys, and setting them in the body face makes them read as words.
            labels.push(Label {
                x: fx,
                y: centre_text_y(f_y, layout::cmd_footer_h(), tokens::MONO),
                style: tokens::MONO,
                text: String::from(keycap),
                color: theme.fg_3,
            });
            fx += measure(atlas, tokens::MONO, keycap) + 7;
            labels.push(Label {
                x: fx,
                y: centre_text_y(f_y, layout::cmd_footer_h(), tokens::HINT),
                style: tokens::HINT,
                text: String::from(what),
                color: theme.fg_4,
            });
            fx += measure(atlas, tokens::HINT, what) + 22;
        }
        let shown = state.hits.iter().filter(|h| h.kind == CmdEntry::Row).count();
        let count = alloc::format!("{} of {} results", shown, state.hits_total);
        let cw = measure(atlas, tokens::HINT, &count);
        labels.push(Label {
            x: panel.right() - layout::cmd_q_pad_x() - cw,
            y: centre_text_y(f_y, layout::cmd_footer_h(), tokens::HINT),
            style: tokens::HINT,
            text: count,
            color: theme.fg_4,
        });
    }

    // 8. Text and icons, batched into one GPU glyph run over everything the CPU just drew.
    paint_text(atlas, &mut canvas, &labels, &icons_out);

    // 8a. The system popup, above the desktop and below the travelling mark.
    //
    // ★ Its own label batch, flushed separately. `osd_paint` used to append into `labels` — which
    // `paint_text` above has ALREADY drawn — so the panel and its track appeared (those go straight
    // to the canvas) while the "BRIGHTNESS" caption and the numeral silently did not. A popup with
    // a moving bar and no words on it.
    //
    // It cannot simply move above the flush: the OSD must cover the desktop's text, so its panel has
    // to be painted after it. Two batches is the only ordering that gets both right — desktop text
    // under the popup, popup text on top of the popup.
    let mut osd_labels: Vec<Label> = Vec::new();
    osd_paint(state, &mut canvas, theme, atlas, &mut osd_labels);
    if !osd_labels.is_empty() {
        paint_text(atlas, &mut canvas, &osd_labels, &[]);
    }

    // 8b. Cold start, stage 3 — the identity hands the corner over.
    //
    // Drawn after the batched text so it sits on top of the Entity mark it is dissolving into, and
    // before the cursor, which is on top of everything always.
    handoff_paint(state, &mut canvas, theme);

    // The software fallback draws the SAME bitmap the plane would have shown, so a machine whose
    // cursor plane refused to come up looks like Meridian rather than like an older system.
    if !state.hw_cursor {
        cursor::blit(state.cursor, canvas.buffer, stride, screen_h, state.mx, state.my);
    }

    // 9. Present into the single owned scanout buffer (P1a): a full swap, or a region BLT when only
    //    part of the screen changed. Single-buffered, so a partial present is valid — untouched
    //    scanout pixels keep the previous frame.
    if full_frame {
        sys_swap_buffers();
    } else {
        sys_swap_buffers_rect(dmg_x, dmg_y, dmg_w, dmg_h);
    }
    sys_gpu_sync();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(99);
}
