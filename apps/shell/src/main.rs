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
// `nyx_gui::ui`'s scrollbar is gone from here: the shell draws Meridian's own (see
// `layout::scrollbar_track`). Since step 20 retired `apps/compositor` that whole module has no
// caller left on the image — it is marked dead at the top of the file rather than removed.

use nyx_meridian::atlas::Atlas;
use nyx_meridian::icons::Icons;
use nyx_meridian::layout::{self, CmdEntry, Control, Rect};
use nyx_meridian::tokens::{self, Style, Theme};
use nyx_meridian::cursor::{self, Shape};
use nyx_meridian::brand;
use nyx_meridian::idle::{self, Phase};
use nyx_meridian::motion::{Motion, Transition, UNIT, WINDOW_OPEN_SCALE};
use nyx_meridian::network::{self, PanelLayout, Page};
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
/// ★ Step 20 removed a row: **Wi-Fi**. It was an application because there was nowhere else to put
/// a network picker; now there is. The Entity surface drills into the network view in place, and the
/// Command still reaches it by the same "Network — …" row it always had — so nothing that used to
/// be reachable stopped being reachable, only the window went away. That is the whole of the build
/// order's *"only once the Entity drill-down is doing its job"*.
const APPS: [App; 12] = [
    App { icon: Icons::APP_FILES,    label: "Files",          bundle: "/mnt/nvme/apps/Explorer.nyx/" },
    App { icon: Icons::APP_TERMINAL, label: "Terminal",       bundle: "/mnt/nvme/apps/Terminal.nyx/" },
    App { icon: Icons::APP_QCLANG,   label: "QCLang",         bundle: "/mnt/nvme/apps/QcStudio.nyx/" },
    App { icon: Icons::DOC,          label: "Notepad",        bundle: "/mnt/nvme/apps/Notepad.nyx/" },
    App { icon: Icons::APP_MONITOR,  label: "System Monitor", bundle: "/mnt/nvme/apps/SystemMonitor.nyx/" },
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
/// a library the shell links, not a window you open). A dock glyph whose exec path is not on the
/// image launches nothing and reads as a hang, which is exactly why those rows were cut from the old
/// start menu too. So the count the design's argument rests on — eight glyphs and one mark — is
/// kept, with the eight applications this machine actually has.
///
/// ⚠️ These are indices into [`APPS`], so **any change to that table renumbers this one**. Step 20
/// removed the Wi-Fi row from the middle of `APPS` and every slot after it shifted by one; the
/// symptom of getting this wrong is a dock glyph that launches its neighbour, which is precisely
/// what the old compositor's three parallel lists used to do before they were collapsed into one
/// table. Wi-Fi's slot went to GL Cube rather than being left empty — the design's argument rests on
/// there being eight.
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

// ───────────────────────────── The creature, step 18 ─────────────────────────────
//
// The Entity has had a mark, three states and a disclosure surface since step 6. What it has not had
// is a face. `libs/entity` (step 17) generates one from a 32-bit seed struck at first boot; this is
// where that lands on screen.
//
// The creature is NOT a live readout. Its appearance changes when the evolution stage advances or an
// influence counter crosses its threshold — a few times a year — and between those events it is one
// cached sprite. A creature that responded to CPU load would twitch every time a build started, and
// within a week nobody would look at it. That is `libs/entity`'s design decision; this file's job is
// to honour it by regenerating only when told to.

/// Where the Entity lives. Flat in the root of the persistent mount, the same convention
/// [`CONFIG_PATH`] uses and for the same reason — it avoids creating a directory that may not exist.
///
/// ⚠️ **Deleting this file is the only way to get a different Entity**, and that is deliberate. There
/// is no reroll, no regenerate button and no setting. An Entity you could reroll until you liked it
/// would be a character creator, and the whole claim of the feature is that you did not choose this
/// one.
const ENTITY_PATH: &str = "/mnt/nvme/entity.bin";

/// The creature's sprite, and the state it was generated from.
struct Creature {
    state: nyx_entity::State,
    /// `px * px` ARGB, regenerated only when [`nyx_entity::State::tick_hour`] says the appearance
    /// changed. `None` when the surface has no room to draw one at all — see
    /// `layout::creature_px_for`.
    sprite: Vec<u32>,
    px: i32,
    /// The idle screen's creature, at [`idle::IDLE_PX`] — eyes open, and eyes shut.
    ///
    /// Two sprites rather than one plus a per-frame lid pass, because the lid is a *palette* edit on
    /// the field and re-blitting 120x120 on every blink frame to save 57 KB of RAM on a machine with
    /// gigabytes of it is the wrong trade. Both come from a single `build`.
    idle_open: Vec<u32>,
    idle_shut: Vec<u32>,
    idle_px: i32,
    /// Uptime in whole hours since this shell started, so `tick_hour` is called once per hour and
    /// not once per frame.
    ticked_hours: usize,
    /// True until the birth has been written to disk. A first boot that fails to persist must retry
    /// rather than silently strike a new seed on the next boot.
    unsaved: bool,
}

impl Creature {
    /// Load the Entity, or strike a new one.
    ///
    /// ⚠️ The seed is mixed from the RTC **and** three machine-unique inputs, because two machines
    /// imaged from the same disk would otherwise share a creature. `Seed::strike` wants a CPU brand,
    /// an NVMe serial and a MAC; what this box can actually answer with is the SMBIOS string
    /// `sys_get_hw_info` returns, so that stands in for the first two and the MAC is zero until
    /// there is a syscall for it. The clock is what makes it unique in practice, which is exactly
    /// the role `strike`'s own documentation gives it.
    fn load_or_birth(screen_h: i32) -> Creature {
        let now = sys_get_rtc().unix();
        let mut buf = [0u8; nyx_entity::STATE_BYTES];
        let (state, unsaved) = match read_entity(&mut buf).and_then(|_| nyx_entity::State::load(&buf)) {
            Some(s) => (s, false),
            None => {
                let mut hw = [0u8; 256];
                let n = sys_get_hw_info(&mut hw);
                let seed = nyx_entity::Seed::strike(now, &hw[..n], &hw[..n], [0; 6]);
                sys_print("[ENTITY] no entity.bin — striking a new seed\n");
                (nyx_entity::State::birth(seed, now), true)
            }
        };
        let mut c = Creature {
            state,
            sprite: Vec::new(),
            px: layout::creature_px_for(screen_h, 4, ENT_CONTROLS),
            idle_open: Vec::new(),
            idle_shut: Vec::new(),
            idle_px: 0,
            ticked_hours: 0,
            unsaved,
        };
        c.regenerate();
        c.persist();
        c
    }

    /// Rasterize the creature at the size this screen has room for.
    ///
    /// `appearance::blit` writes `cell * 24` pixels square, so the cell is chosen from the size the
    /// layout picked rather than rasterizing at 96 and scaling — a scaled creature is a blurred one,
    /// and 24 cells at 2px is a genuinely different (and still correct) drawing.
    fn regenerate(&mut self) {
        let g = nyx_entity::appearance::G;
        // ONE build, three blits. The field is 576 bytes of palette indices and building it is pure
        // integer work, but it is also the thing that must not disagree between the surface creature
        // and the idle creature — they are the same animal.
        let mut field = nyx_entity::appearance::build(
            &self.state.genome(),
            self.state.stage(),
            &self.state.influence,
        );

        // The Entity surface, at whatever size the layout had room for. Zero means the surface has
        // no room for a creature at all — see `layout::creature_px_for`.
        if self.px > 0 {
            let cell = (self.px / g as i32).max(1) as usize;
            let side = cell * g;
            self.sprite = alloc::vec![0u32; side * side];
            nyx_entity::appearance::blit(&field, cell, &mut self.sprite, side);
            self.px = side as i32;
        } else {
            self.sprite.clear();
        }

        // The idle screen, which always has room — it is the whole screen.
        let cell = (idle::IDLE_PX / g as i32).max(1) as usize;
        let side = cell * g;
        self.idle_px = side as i32;
        self.idle_open = alloc::vec![0u32; side * side];
        nyx_entity::appearance::blit(&field, cell, &mut self.idle_open, side);
        field.close_eyes();
        self.idle_shut = alloc::vec![0u32; side * side];
        nyx_entity::appearance::blit(&field, cell, &mut self.idle_shut, side);
    }

    /// The sprite to draw right now: shut while blinking, and shut for the whole of sleep.
    fn idle_sprite(&self, ms: usize, asleep: bool) -> &[u32] {
        let blink = self.state.genome().personality.blink_ticks;
        if asleep || idle::blink_closed(ms, blink) { &self.idle_shut } else { &self.idle_open }
    }

    /// Where the creature sits, relative to its resting place. Sleep slows the breath to a constant
    /// eleven seconds for every personality — which is where the design's *"a Calm Entity barely
    /// changes, an Energetic one visibly settles"* comes from without anything being written to make
    /// it true.
    fn idle_offset(&self, ms: usize, asleep: bool) -> (i32, i32) {
        let float = if asleep {
            idle::ASLEEP_FLOAT_TICKS
        } else {
            self.state.genome().personality.float_ticks
        };
        let (dx, dy) = idle::drift(ms);
        (dx, dy + idle::float_dy(ms, float, self.idle_px))
    }

    /// Advance the hour counter, and regenerate only if the creature actually changed.
    fn tick(&mut self, uptime_ms: usize) {
        let hours = uptime_ms / 3_600_000;
        while self.ticked_hours < hours {
            self.ticked_hours += 1;
            if self.state.tick_hour(sys_get_rtc().unix()) {
                sys_print("[ENTITY] appearance changed — regenerating\n");
                self.regenerate();
            }
            self.unsaved = true;
        }
        if self.unsaved {
            self.persist();
        }
    }

    fn persist(&mut self) {
        let mut buf = [0u8; nyx_entity::STATE_BYTES];
        self.state.save(&mut buf);
        if write_entity(&buf) {
            self.unsaved = false;
        }
    }

    /// `NX-7F3A-91C2`.
    fn seed_text(&self) -> String {
        let f = self.state.seed.format();
        String::from(core::str::from_utf8(&f).unwrap_or("NX-????-????"))
    }

    /// `Stage 4 · Intelligence`.
    fn stage_text(&self) -> String {
        let s = self.state.stage();
        alloc::format!("Stage {} \u{00B7} {}", s.index(), s.name())
    }

    /// `Curious · Protective · Focused`.
    fn traits_text(&self) -> String {
        let p = self.state.genome().personality;
        alloc::format!("{} \u{00B7} {} \u{00B7} {}", p.name(0), p.name(1), p.name(2))
    }

    /// `Born 4 March · 312 h`, or just the hours when the RTC could not answer at birth.
    fn born_text(&self) -> String {
        const MONTHS: [&str; 12] = [
            "January", "February", "March", "April", "May", "June",
            "July", "August", "September", "October", "November", "December",
        ];
        let h = self.state.counters.uptime_hours;
        if self.state.born_unix == 0 {
            return alloc::format!("{} h", h);
        }
        let d = civil(self.state.born_unix);
        alloc::format!("Born {} {} \u{00B7} {} h", d.1, MONTHS[(d.0 as usize - 1).min(11)], h)
    }
}

/// `(month, day)` for a unix instant. Only ever used for the birth date, which is why it does not
/// bother with the year: the design's footer is `Born 4 March`, and an Entity old enough for the
/// year to matter has earned a line of its own rather than a longer one here.
fn civil(unix: u64) -> (u8, u8) {
    let t = nyx_api::civil_from_unix_public(unix as i64);
    (t.month, t.day)
}

// ─────────────────────────────── The lock ───────────────────────────────

/// `/mnt/nvme/auth.bin` — version, iteration count, salt, derived key. 53 bytes.
///
/// ⚠️ **Say plainly what this protects against, because it is not much.** Nyx has no disk
/// encryption. The NVMe is readable by anyone who takes the machine apart, and this file gates a
/// drawing routine rather than access to data. The lock stops a person borrowing your desk. It does
/// not stop a person with a screwdriver, and it must never be described in the interface as though
/// it does — no padlock icon, no "secure", no shield. The honest fix is full-disk encryption, which
/// is a genuinely large piece of work and is not this one.
///
/// **An absent file means no password is set and the lock never appears.** That is the default, and
/// it is why the file is read rather than created at boot.
const AUTH_PATH: &str = "/mnt/nvme/auth.bin";
const AUTH_VERSION: u8 = 1;
const AUTH_SALT: usize = 16;
const AUTH_KEY: usize = 32;
const AUTH_BYTES: usize = 1 + 4 + AUTH_SALT + AUTH_KEY;

/// The stored credential.
struct Auth {
    iters: u32,
    salt: [u8; AUTH_SALT],
    key: [u8; AUTH_KEY],
}

impl Auth {
    fn load() -> Option<Auth> {
        let fd = sys_open(AUTH_PATH);
        if fd < 0 { return None; }
        let mut buf = [0u8; AUTH_BYTES];
        let n = sys_read(fd, &mut buf);
        sys_close(fd);
        if (n as usize) < AUTH_BYTES || buf[0] != AUTH_VERSION { return None; }
        let mut a = Auth {
            iters: u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]),
            salt: [0; AUTH_SALT],
            key: [0; AUTH_KEY],
        };
        a.salt.copy_from_slice(&buf[5..5 + AUTH_SALT]);
        a.key.copy_from_slice(&buf[5 + AUTH_SALT..AUTH_BYTES]);
        // A zero iteration count would make PBKDF2 a single HMAC. Refuse the file rather than
        // silently accepting a weaker one than was written.
        if a.iters == 0 { None } else { Some(a) }
    }

    /// Constant-time. See `nyx_crypto::ct_eq` for why this is worth four lines behind a PBKDF2
    /// front door: it is free now and awkward to retrofit later.
    fn matches(&self, attempt: &[u8]) -> bool {
        let got = nyx_crypto::pbkdf2_sha1(attempt, &self.salt, self.iters, AUTH_KEY);
        nyx_crypto::ct_eq(&got, &self.key)
    }
}

fn read_entity(buf: &mut [u8; nyx_entity::STATE_BYTES]) -> Option<()> {
    let fd = sys_open(ENTITY_PATH);
    if fd < 0 {
        return None;
    }
    let n = sys_read(fd, buf);
    sys_close(fd);
    (n as usize >= nyx_entity::STATE_BYTES).then_some(())
}

/// Best-effort, like [`write_config`]. A box with no disk keeps its Entity until reboot and strikes
/// a new one next time — which is a fair outcome for a machine with nowhere to remember anything,
/// and better than refusing to show a creature at all.
fn write_entity(buf: &[u8; nyx_entity::STATE_BYTES]) -> bool {
    let fd = sys_open_flags(ENTITY_PATH, O_CREAT | O_TRUNC);
    if fd < 0 {
        return false;
    }
    let n = sys_write(fd, buf);
    sys_close(fd);
    n as usize == buf.len()
}

/// How many control rows the Entity surface has. ONE source of truth: the geometry is sized from it
/// and the rows are drawn from it, and those two disagreeing is how a hit-test lands on the row above
/// the one you clicked.
const ENT_CONTROLS: usize = 4;

/// Which of those rows is Brightness. It is LAST so the three the design specifies keep the indices
/// they already had — Network stays row 0.
const ENT_BRIGHT_ROW: usize = 3;

/// Which of those rows drills into the network view. Named rather than written as `0` at the click
/// site, because that literal is how a row index and a row order drift apart silently.
const ENT_NETWORK_ROW: usize = 0;

/// How far the pointer must travel in one [`IDLE_PROBE_MS`] window to count as a person being here.
///
/// A deliberate hand movement covers hundreds of pixels a second. Trackpoint drift and a resting
/// palm cover single digits. 24 is comfortably above the second and far below the first.
const IDLE_MOVE_PX: i32 = 24;
/// How far the pointer must jump to WAKE an idle screen. Smaller, because by then the intent is not
/// in doubt — but not zero, or drift dismisses the screen it just prevented.
const IDLE_WAKE_PX: i32 = 8;
/// The sampling window. One second: long enough that drift cannot cross the threshold inside it,
/// short enough that letting go of the mouse costs at most a second of the 90 before the chrome
/// recedes.
const IDLE_PROBE_MS: usize = 1000;

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
/// Idle, sleep and the lock — Meridian step 19.
///
/// Drawn as an overlay over a fully composited desktop rather than as a separate frame path. That
/// costs a wasted composite on every idle frame and it is the right trade: the alternative skips
/// `sys_gpu_composite` entirely, which lets the render engine settle into RC6, and the first
/// composite after waking then hangs in `wait_for_idle` while the hardware cursor keeps moving. See
/// the note in the main loop.
///
/// ⚠️ The clock here is the ONLY thing on either screen that is not the creature. There is no
/// notification list, no calendar and no "3 unread" — *"a lock screen that leaks your notifications
/// to anyone walking past is a lock screen that has misunderstood its job."*
fn idle_paint(state: &Shell, canvas: &mut Canvas, atlas: Option<&Atlas>, now: usize) {
    let fade = idle::idle_fade(state.idle_ms(now));
    if fade == 0 && !state.locked {
        return;
    }
    let (w, h) = (state.screen_stride, state.screen_h as usize);
    let asleep = state.phase == Phase::Asleep;
    // The cross-fade to black IS this fill: `fill_rect` alpha-blends anything under 255, so a ramp
    // from 0 to 255 dissolves the desktop into the ground over the design's 40 frames.
    let bg = if asleep { idle::SLEEP_BG } else { idle::IDLE_BG };
    let a = if state.locked { 255 } else { fade as u32 };
    canvas.fill_rect(0, 0, w, h, (a << 24) | (bg & 0x00FF_FFFF));

    let mut labels: Vec<Label> = Vec::new();
    let px = state.creature.idle_px;
    let clock = state.vitals.clock();
    let clock_h = tokens::MONO.line() as i32;

    // The whole stack — creature, then the clock — is centred, not the creature alone. Getting this
    // wrong is what the design warns about: *"left to itself it aligns against the widest sibling,
    // which throws the whole stack off axis."*
    let base = idle::idle_creature(state.screen_w, state.screen_h, px, clock_h);
    let (dx, dy) = state.creature.idle_offset(now, asleep);

    let sprite = state.creature.idle_sprite(now, asleep);
    if !sprite.is_empty() && px > 0 {
        let x = (base.x + dx).max(0) as usize;
        let y = (base.y + dy).max(0) as usize;
        canvas.composite_buffer(x, y, sprite, px as usize, px as usize, a as u8);
    }

    // The clock travels with the creature. It is part of the same drifting stack — a clock nailed to
    // the centre while the creature wanders away from it would come apart over a night, which is
    // exactly the timescale the drift is for.
    let cw = measure(atlas, tokens::MONO, &clock);
    labels.push(Label {
        x: base.x + dx + (px - cw) / 2,
        y: idle::idle_clock_y(base) + dy,
        style: tokens::MONO,
        text: clock,
        color: dim(idle::IDLE_CLOCK, a),
    });

    if state.locked {
        lock_labels(state, canvas, atlas, base, now, &mut labels);
    }

    if !labels.is_empty() {
        paint_text(atlas, canvas, &labels, &[]);
    }
}

/// The lock's field, its rule, its hint and the seed. Split out only for length.
fn lock_labels(
    state: &Shell,
    canvas: &mut Canvas,
    atlas: Option<&Atlas>,
    creature: Rect,
    now: usize,
    labels: &mut Vec<Label>,
) {
    let f = idle::lock_field(state.screen_w, creature);

    // One dot per character, drawn the way every other place you type in Nyx is drawn — plain marks
    // on a hairline.
    let (dots, caret) = idle::lock_dots(f, state.typed_chars());
    for d in dots {
        // `border-radius: 3px` on a 6px box is a circle, so the radius is half the box whatever the
        // panel scale makes the box.
        shapes::fill_round_rect(canvas, d.x.max(0) as usize, d.y.max(0) as usize,
                                d.w as usize, d.h as usize, (d.w / 2) as usize, state.theme.fg_2);
    }
    canvas.fill_rect(caret.x.max(0) as usize, caret.y.max(0) as usize,
                     caret.w as usize, caret.h as usize, state.theme.accent);

    // `.lock-r.act` — the field always has focus, because it is the only thing on the screen.
    let rule = idle::lock_rule(f);
    canvas.fill_rect(rule.x.max(0) as usize, rule.y.max(0) as usize,
                     rule.w as usize, rule.h.max(1) as usize, state.theme.line_2);

    // *"Failure is a sentence, not an animation."* No shake, no flash, no red.
    let penalty = idle::penalty_remaining(state.fails, state.fail_at, now);
    let (hint, color) = match penalty {
        Some(secs) => (alloc::format!("Too many attempts. Wait {} s.", secs), idle::LOCK_HINT_ERR),
        None if state.lock_err => (String::from("Incorrect. Try again."), idle::LOCK_HINT_ERR),
        None => (String::from("Return to unlock"), idle::LOCK_HINT),
    };
    let hw = measure(atlas, tokens::B3, &hint);
    labels.push(Label {
        x: f.x + (f.w - hw) / 2,
        y: idle::lock_hint_y(f),
        style: tokens::B3,
        text: hint,
        color,
    });

    // *"The seed is the only identifier on the screen."* Not a hostname, not a user name, not an
    // email address — it is unique to the installation, it means nothing to anyone who has not seen
    // this machine before, and it is the one string that makes a locked Nyx recognisably yours
    // without disclosing anything about you.
    let seed = state.creature.seed_text();
    let sw = measure(atlas, tokens::MONO, &seed);
    let sh = tokens::MONO.line() as i32;
    labels.push(Label {
        x: (state.screen_w - sw) / 2,
        y: idle::lock_seed_y(state.screen_h, sh),
        style: tokens::MONO,
        text: seed,
        color: idle::IDLE_DIM,
    });
}

/// Scale a 0xAARRGGBB colour's alpha by `a`/255, for the cross-fade.
fn dim(color: u32, a: u32) -> u32 {
    let src = (color >> 24) & 0xFF;
    ((src * a / 255) << 24) | (color & 0x00FF_FFFF)
}

/// Fade `color` toward `ground` as `a` falls from 255 to 0, keeping it fully opaque.
///
/// ★ Why this is not just [`dim`]. The batched glyph path CANNOT fade. `build_ps_text` carries one
/// per-quad **luminance** and outputs `RGB = lum, A = coverage` — the alpha the caller asked for is
/// discarded, and the blend uses the glyph's antialiasing coverage instead. So setting a Label's
/// alpha channel does nothing at all on the GPU path; a text fade has to be done by moving the
/// COLOUR toward the ground, which is a luminance change and therefore something that shader can
/// express.
///
/// This is the same reason the Entity's attention-state mark leaves the batch and is blended
/// CPU-side, and the same reason every accent in Meridian is a `fill_rect`.
fn fade_to(color: u32, ground: u32, a: u32) -> u32 {
    if a >= 255 {
        return color;
    }
    // A straight per-channel lerp — `ground + (color - ground) * a / 255` — done in SIGNED
    // arithmetic so it works whichever of the two is brighter. That matters: the dark theme fades
    // light chrome down toward a near-black ground, and the light theme fades dark chrome UP toward
    // a near-white one. Unsigned subtraction would wrap on one of the two and invert the fade.
    let lerp = |sh: u32| -> u32 {
        let c = ((color >> sh) & 0xFF) as i32;
        let g = ((ground >> sh) & 0xFF) as i32;
        (g + (c - g) * a as i32 / 255).clamp(0, 255) as u32
    };
    0xFF00_0000 | (lerp(16) << 16) | (lerp(8) << 8) | lerp(0)
}

/// The Entity surface's second view — the network panel. Meridian step 20.
///
/// Takes `&mut Shell` for one reason: the action words are plain text of measured width, so where
/// they landed is only knowable here, and the click path has no atlas. It is cached into
/// `net.action_rects` the same way `ent_detail_lines` is cached, and for the same reason — two
/// answers to "where is that word" is a click that lands next to it.
fn net_paint(
    state: &mut Shell,
    canvas: &mut Canvas,
    atlas: Option<&Atlas>,
    labels: &mut Vec<Label>,
    icons_out: &mut Vec<IconDraw>,
    theme: Theme,
) {
    let (l, page) = state.net_geometry();

    shapes::drop_shadow(canvas, l.surface.x, l.surface.y, l.surface.w, l.surface.h,
                        layout::surface_radius(), &[]);
    shapes::fill_round_rect(canvas, l.surface.x.max(0) as usize, l.surface.y.max(0) as usize,
                            l.surface.w as usize, l.surface.h as usize,
                            layout::surface_radius(), theme.chrome);
    shapes::stroke_round_rect(canvas, l.surface.x.max(0) as usize, l.surface.y.max(0) as usize,
                              l.surface.w as usize, l.surface.h as usize,
                              layout::surface_radius(), theme.line_2);
    for y in l.rules() {
        shapes::hairline_h(canvas, l.surface.x as usize, y.max(0) as usize,
                           l.surface.w as usize, theme.line);
    }

    // ── The back affordance ───────────────────────────────────────────────────────────────────
    // *"with a back affordance and no new surface."* One row, one glyph, one word.
    if state.net.hover == Some(NetHit::Back) {
        shapes::fill_round_rect(canvas, l.back.x.max(0) as usize, l.back.y.max(0) as usize,
                                l.back.w as usize, l.back.h as usize, 5, theme.wash);
    }
    icons_out.push(IconDraw {
        id: Icons::BACK,
        px: layout::ent_crow_icon() as usize,
        x: l.back.x + layout::ent_crow_pad_x(),
        y: l.back.y + (l.back.h - layout::ent_crow_icon()) / 2,
        color: theme.fg_3,
    });
    labels.push(Label {
        x: l.back.x + layout::ent_crow_pad_x() + 16 + 12,
        y: centre_text_y(l.back.y, l.back.h, tokens::B2),
        style: tokens::B2,
        text: String::from("Network"),
        color: theme.fg,
    });

    // ── The statement and its one line ────────────────────────────────────────────────────────
    labels.push(Label {
        x: l.statement.x,
        y: l.statement.y,
        style: tokens::D3,
        text: network::statement(&state.net.status),
        color: theme.fg,
    });
    let detail = if !state.net.present {
        String::from("There is no Wi-Fi adapter on this machine.")
    } else {
        network::state_detail(&state.net.status, state.net.current_secure())
    };
    labels.push(Label {
        x: l.detail.x,
        y: l.detail.y,
        style: tokens::B3,
        text: fit(atlas, tokens::B3, &detail, l.detail.w),
        color: theme.fg_3,
    });

    // ── Address / Gateway / Resolver ──────────────────────────────────────────────────────────
    // All three come straight out of the DHCP lease the kernel published. The design's `Signal` row
    // sits between the state line and these; see `nyx_meridian::network` for why it is not here.
    for (i, (k, val)) in network::link_rows(&state.net.status).iter().enumerate() {
        let Some(row) = l.link_row(i) else { continue };
        labels.push(Label {
            x: row.x,
            y: centre_text_y(row.y, row.h, tokens::B3),
            style: tokens::B3,
            text: String::from(*k),
            color: theme.fg_3,
        });
        // An address is a number to be read back, typed in or compared — the same argument that
        // puts the Entity's seed in MONO puts these there too.
        let vw = measure(atlas, tokens::MONO, val);
        labels.push(Label {
            x: row.right() - vw,
            y: centre_text_y(row.y, row.h, tokens::MONO),
            style: tokens::MONO,
            text: val.clone(),
            color: theme.fg,
        });
    }

    // ── AVAILABLE ─────────────────────────────────────────────────────────────────────────────
    let (lx, ly) = l.section_label();
    labels.push(Label {
        x: lx,
        y: ly,
        style: tokens::LB,
        text: String::from("Available"),
        color: theme.fg_4,
    });

    if state.net.nets.is_empty() {
        if let Some(row) = l.row(0) {
            // Never "no networks in range" when the radio is off — that blames the neighbourhood
            // for a switch the user just flicked themselves.
            let msg = if !state.net.present {
                "No adapter."
            } else if !state.net.radio_on() {
                "Turn Wi-Fi on to see what is in range."
            } else {
                "Nothing in range. Rescan to look again."
            };
            labels.push(Label {
                x: row.x + layout::ent_crow_pad_x(),
                y: centre_text_y(row.y, row.h, tokens::B3),
                style: tokens::B3,
                text: String::from(msg),
                color: theme.fg_4,
            });
        }
    }

    for i in 0..page.len {
        let Some(row) = l.row(i) else { continue };
        let Some(n) = state.net.nets.get(page.start + i) else { continue };
        let selected = state.net.sel == Some(page.start + i);
        let hovered = state.net.hover == Some(NetHit::Row(i));
        if selected || hovered {
            shapes::fill_round_rect(canvas, row.x.max(0) as usize, row.y.max(0) as usize,
                                    row.w as usize, row.h as usize, 5, theme.wash);
        }
        if selected {
            // The Command's selection bar, on the same argument: it is a `fill_rect`, so the accent
            // survives — nothing in the batched glyph path can carry a colour.
            canvas.fill_rect(row.x.max(0) as usize,
                             (row.y + layout::cmd_sel_bar_inset()).max(0) as usize,
                             layout::cmd_sel_bar_w() as usize,
                             (row.h - layout::cmd_sel_bar_inset() * 2).max(0) as usize,
                             theme.accent);
        }

        // A network we cannot join is drawn at the muted tier throughout — the row is still there,
        // still readable and still explains itself when clicked, but it never looks available.
        let joinable = network::joinable(n);
        let name_color = if joinable { theme.fg } else { theme.fg_3 };
        let text_x = row.x + layout::ent_crow_pad_x();

        if n.is_secure() {
            icons_out.push(IconDraw {
                id: Icons::LOCK,
                px: layout::ent_crow_icon() as usize,
                x: text_x,
                y: row.y + (row.h - layout::ent_crow_icon()) / 2,
                color: if joinable { theme.fg_3 } else { theme.fg_4 },
            });
        }
        // The name starts past the padlock's column whether or not there is one, so an open network
        // does not sit a glyph to the left of its neighbours.
        let name_x = text_x + 16 + 10;
        let saved = state.net.saved.as_deref() == Some(n.name());
        let meta = network::row_status(n, saved);
        let mw = measure(atlas, tokens::B3, &meta);
        let budget = row.right() - layout::ent_crow_pad_x() - mw - 12 - name_x;
        labels.push(Label {
            x: name_x,
            y: centre_text_y(row.y, row.h, tokens::B2),
            style: tokens::B2,
            text: fit(atlas, tokens::B2, n.name(), budget),
            color: name_color,
        });
        labels.push(Label {
            x: row.right() - layout::ent_crow_pad_x() - mw,
            y: centre_text_y(row.y, row.h, tokens::B3),
            style: tokens::B3,
            text: meta,
            color: theme.fg_4,
        });
    }

    // The pager. Plain text in the last row, because *"actions are plain text, never buttons"* —
    // and there is no scroll wheel on this machine to make it unnecessary.
    if page.has_pager() {
        if let Some(row) = l.row(page.len) {
            if state.net.hover == Some(NetHit::Pager) {
                shapes::fill_round_rect(canvas, row.x.max(0) as usize, row.y.max(0) as usize,
                                        row.w as usize, row.h as usize, 5, theme.wash);
            }
            labels.push(Label {
                x: row.x + layout::ent_crow_pad_x(),
                y: centre_text_y(row.y, row.h, tokens::B3),
                style: tokens::B3,
                text: alloc::format!(
                    "More \u{00B7} {} of {}",
                    page.index + 1,
                    page.pages
                ),
                color: theme.fg_3,
            });
        }
    }

    // ── The passphrase ────────────────────────────────────────────────────────────────────────
    if let Some(field) = l.psk_field() {
        let rule = l.psk_rule().unwrap();
        canvas.fill_rect(rule.x.max(0) as usize, rule.y.max(0) as usize,
                         rule.w as usize, 1, theme.accent);
        if state.net.show_psk {
            // Revealed. Shown for the one case masking actively hurts: a long passphrase typed
            // wrong, where the only other feedback is a failed handshake ten seconds later. It is
            // never the default and never survives a change of selection — see `reset_psk`.
            let text = String::from(core::str::from_utf8(&state.net.psk).unwrap_or(""));
            labels.push(Label {
                x: field.x,
                y: centre_text_y(field.y, field.h, tokens::MONO),
                style: tokens::MONO,
                // Tail-anchored: the caret is at the end, and that is the part being looked at.
                text: fit_tail(atlas, tokens::MONO, &text, field.w - 2),
                color: theme.fg,
            });
            let tw = measure(atlas, tokens::MONO, &text).min(field.w - 2);
            canvas.fill_rect((field.x + tw + 1).max(0) as usize,
                             (field.y + field.h / 2 - 9).max(0) as usize,
                             1, 18, theme.accent);
        } else if let Some((dots, caret)) = l.psk_dots(state.net.typed()) {
            for d in dots {
                shapes::fill_round_rect(canvas, d.x.max(0) as usize, d.y.max(0) as usize,
                                        d.w as usize, d.h as usize, (d.w / 2).max(1) as usize,
                                        theme.fg_2);
            }
            canvas.fill_rect(caret.x.max(0) as usize, caret.y.max(0) as usize,
                             caret.w as usize, caret.h as usize, theme.accent);
        }
        if let Some(hint) = l.psk_hint() {
            labels.push(Label {
                x: hint.x,
                y: hint.y,
                style: tokens::HINT,
                text: String::from("Return to join"),
                color: theme.fg_4,
            });
        }
    }

    // ── The actions, then the message ─────────────────────────────────────────────────────────
    let actions = state.net.actions();
    let revealed = state.net.show_psk;
    let remember = state.net.remember;
    let bar = l.actions();
    let mut x = bar.x;
    let mut rects: Vec<(NetAction, Rect)> = Vec::new();
    for (i, a) in actions.iter().enumerate() {
        let text = a.label(revealed, remember);
        let w = measure(atlas, tokens::B3, text);
        if x + w > bar.right() {
            // Out of room. Dropping the tail is better than overrunning the panel: everything that
            // could be dropped here is also reachable another way (the radio switch is the last
            // action, and Escape always backs out).
            break;
        }
        let hovered = state.net.hover == Some(NetHit::Action(i));
        labels.push(Label {
            x,
            y: centre_text_y(bar.y, bar.h, tokens::B3),
            style: tokens::B3,
            text: String::from(text),
            color: if hovered { theme.fg } else { theme.fg_2 },
        });
        if hovered {
            // Underlined on hover rather than plated: a plate would turn a word into a button, and
            // the design is explicit that these are not buttons.
            canvas.fill_rect(x.max(0) as usize,
                             (bar.y + bar.h - 1).max(0) as usize,
                             w as usize, 1, theme.fg_3);
        }
        // The hit box is the word plus the gap's worth of slack on each side — a 17px-tall run of
        // text is a small target, and the gap belongs to nothing else.
        rects.push((*a, Rect::new(x - 5, bar.y - 4, w + 10, bar.h + 8)));
        x += w;
        if i + 1 < actions.len() {
            let sep = " \u{00B7} ";
            let sw = measure(atlas, tokens::B3, sep);
            labels.push(Label {
                x,
                y: centre_text_y(bar.y, bar.h, tokens::B3),
                style: tokens::B3,
                text: String::from(sep),
                color: theme.fg_4,
            });
            x += sw;
        }
    }
    state.net.action_rects = rects;

    if !state.net.message.is_empty() {
        let msg = l.message();
        labels.push(Label {
            x: msg.x,
            y: msg.y,
            style: tokens::B3,
            text: fit(atlas, tokens::B3, &state.net.message, msg.w),
            color: theme.fg_3,
        });
    }
}

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

/// Grab band just inside a surface's edge where a drag starts a resize.
///
/// ★ Now `layout::resize_band` rather than a local `scale::px(8)`. The scrollbar's inset is derived
/// from this value — it exists solely to sit outside the band, because the resize edge is tested
/// first and would otherwise claim most of the bar — and a test in that module proves the two clear
/// each other. Two copies of the number would let the proof pass while the desktop disagreed.
use nyx_meridian::layout::resize_band;

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

/// A client's live caption context and dirty flag — `.w-cap .ctx` and the unsaved tick.
///
/// Read from the mapped header on every frame, exactly like [`client_scroll`], because the whole
/// point is that it changes: a word count that only updated when the window was created would be
/// worse than showing none. Empty for every app that never sets it, which is all of them but Text
/// and QCLang.
///
/// Invalid UTF-8 is treated as **absent** rather than substituted, so a truncation bug in an app
/// shows up as "no context" instead of as replacement characters sitting on the desktop.
fn client_context(client: &Client) -> (&str, bool) {
    if client.buffer.is_null() {
        return ("", false);
    }
    let hdr = unsafe {
        &*((client.buffer as *const u8).sub(core::mem::size_of::<WindowHeader>())
            as *const WindowHeader)
    };
    let len = hdr.context.iter().position(|&c| c == 0).unwrap_or(hdr.context.len());
    (core::str::from_utf8(&hdr.context[..len]).unwrap_or(""), hdr.dirty != 0)
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

/// The **tail** of `text` that fits `budget`, dropping characters off the front.
///
/// [`fit`]'s mirror image, for the one place the interesting end is the last character rather than
/// the first: a revealed passphrase, where the caret is at the end and the reason it was revealed at
/// all is to check what was just typed. No ellipsis — a leading `…` in a password field reads as a
/// character of the password.
fn fit_tail(atlas: Option<&Atlas>, style: Style, text: &str, budget: i32) -> String {
    if budget <= 0 {
        return String::new();
    }
    let mut start = 0usize;
    while start < text.len() && measure(atlas, style, &text[start..]) > budget {
        start += 1;
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
    }
    String::from(&text[start..])
}

/// `text` trimmed with a trailing `…` until it fits `budget`, measured the way it will be drawn.
///
/// Returns **empty** for a budget too narrow even for the ellipsis, rather than a lone `…`. A single
/// character where a filename belongs looks like a rendering fault; nothing looks like the window
/// being too narrow, which is what has actually happened.
///
/// The shell's own, rather than `text::ellipsize`, because that measures with the CPU rasterizer and
/// the shell sets type through the atlas — measuring with one and drawing with the other is the
/// drift `text.rs` exists to prevent, one layer up.
fn fit(atlas: Option<&Atlas>, style: Style, text: &str, budget: i32) -> String {
    if budget <= 0 {
        return String::new();
    }
    if measure(atlas, style, text) <= budget {
        return String::from(text);
    }
    let ell = measure(atlas, style, "…");
    if ell > budget {
        return String::new();
    }
    let mut end = text.len();
    while end > 0 {
        end -= 1;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        if measure(atlas, style, &text[..end]) + ell <= budget {
            break;
        }
    }
    let mut out = String::from(&text[..end]);
    out.push('…');
    out
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
    /// Open the Entity surface on its network view. Step 20's replacement for launching `apps/wifi`.
    OpenNetwork,
}

/// The power view's rows, in order. ONE source of truth: the geometry is sized from it, the rows
/// are drawn from it and the hit-test indexes it, which is what stops a click landing on the row
/// above the one under the pointer.
const POWER_ROWS: [Power; 2] = [Power::Off, Power::Restart];

/// The two things that end a session. There is no third: Nyx implements no ACPI S3, so a Sleep row
/// here would be a control that either does nothing or parks the machine somewhere it cannot wake
/// from. Meridian's "sleep" is the idle screen with the panel at 15%, which is a different claim and
/// an honest one.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Power {
    Off,
    Restart,
}

impl Power {
    fn action(self) -> u64 {
        match self {
            Power::Off => POWER_OFF,
            Power::Restart => POWER_RESTART,
        }
    }
    /// The row before it is armed.
    fn label(self) -> &'static str {
        match self {
            Power::Off => "Shut down",
            Power::Restart => "Restart",
        }
    }
    /// The row once it is armed. Restating the verb rather than saying "Confirm" alone, so a glance
    /// at the highlighted row still says which of the two is about to happen.
    fn armed_label(self) -> &'static str {
        match self {
            Power::Off => "Shut down \u{2014} press Return again",
            Power::Restart => "Restart \u{2014} press Return again",
        }
    }
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
    /// `idle_ms` is how long since the last input, and it is here for one reason: **it is the only
    /// way to see whether the idle clock is running without waiting five minutes and guessing.**
    ///
    /// Step 19 shipped and then did not fire on hardware, and the whole failure is invisible — a
    /// desktop that has not idled looks exactly like a desktop you have not left alone long enough.
    /// Open the Entity, take your hands off, and this line counts. If it stays at zero the input
    /// test is wrong; if it counts and nothing happens at 90 s, the phase machine is. That is a
    /// question answered in ten seconds instead of a power cycle, on a machine with no serial
    /// console — and it is the Entity's own voice, which is exactly the surface for "nothing has
    /// touched me for a while".
    fn refresh(&mut self, fps: usize, idle_ms: usize) {
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
            // Past half a minute the idle time replaces the frame rate. Both are "the machine is
            // fine"; only one of them is also the answer to a question you might be asking.
            self.detail = if idle_ms >= 30_000 {
                alloc::format!(
                    "{} tasks across {} cores. Untouched for {} min {:02} s.",
                    self.tasks, self.cores, idle_ms / 60_000, (idle_ms / 1000) % 60
                )
            } else {
                alloc::format!(
                    "{} tasks across {} cores. Compositing at {} frames per second.",
                    self.tasks, self.cores, fps
                )
            };
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

}

// ───────────────────── The network drill-down (Meridian step 20) ─────────────────────
//
// *"The Entity surface drills in place: tap Network on the first level and the panel becomes the
// network view, with a back affordance and no new surface."*
//
// The geometry and every content decision live in `nyx_meridian::network`, host-tested. What is
// here is the part that cannot be: the scan list, the passphrase being typed, and the one rule that
// makes this safe to put in the window server at all — the shell never calls a blocking radio
// syscall. It launches `apps/wifiagent` and polls the published snapshot instead. See
// `nyx_api::WifiOp`.

/// Which view the Entity surface is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntView {
    /// The Entity proper — creature, statement, measurements, identity, controls.
    Root,
    /// The network panel, drilled into from the Network control row.
    Network,
}

/// What the pointer is over inside the network view.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NetHit {
    Back,
    /// A network row, as an index into the *visible page*, not into `nets`.
    Row(usize),
    /// The last row, when the list is paged.
    Pager,
    /// An action on the footer line, by index into `NetView::actions`.
    Action(usize),
}

/// One plain-text action. *"Actions are plain text, never buttons."*
#[derive(Clone, Copy, PartialEq, Eq)]
enum NetAction {
    Rescan,
    Disconnect,
    Forget,
    RadioOn,
    RadioOff,
    /// Reveal or re-mask the passphrase. Only present while the field is up.
    Reveal,
    /// Flip whether a successful join is written to `wifi.conf`.
    Remember,
}

impl NetAction {
    /// The label states what CLICKING does, not what is currently true — the same convention the
    /// Command's rows use, where "Appearance — Light" means *click to go there*. A toggle labelled
    /// with its own state reads as a status line that mysteriously reacts to being clicked.
    fn label(self, revealed: bool, remember: bool) -> &'static str {
        match self {
            NetAction::Rescan => "Rescan",
            NetAction::Disconnect => "Disconnect",
            NetAction::Forget => "Forget",
            NetAction::RadioOn => "Turn Wi-Fi on",
            NetAction::RadioOff => "Turn Wi-Fi off",
            NetAction::Reveal => {
                if revealed {
                    "Hide"
                } else {
                    "Show"
                }
            }
            NetAction::Remember => {
                if remember {
                    "Join once only"
                } else {
                    "Remember this network"
                }
            }
        }
    }
}

/// How long the shell waits between re-reading the link while the network view is open. Both calls
/// it makes — `sys_wifi_status` and `sys_wifi_list` — read a published snapshot and never touch the
/// driver lock (the kernel's own note on syscall 544: *"the picker calls this on a timer, and a
/// blocking acquire here is one of the two ways the machine used to wedge"*), so this is cheap. It
/// is faster than the 1 Hz heartbeat because a join changes state in steps a person is watching for.
const NET_POLL_MS: usize = 400;

struct NetView {
    /// The last scan, in `network::order`.
    nets: Vec<WifiNetwork>,
    status: WifiStatus,
    /// Whether the adapter exists at all. `false` makes the whole view say so and offer nothing.
    present: bool,
    /// The SSID in `wifi.conf`, so Forget can name what it would drop.
    saved: Option<String>,
    /// The selected row, as an index into `nets`.
    sel: Option<usize>,
    /// The passphrase being typed, as bytes — it is UTF-8 under construction and may not be a valid
    /// `str` between keystrokes.
    psk: Vec<u8>,
    /// Whether it is shown in the clear. Forced back to `false` whenever the selection changes, so
    /// revealing is a decision about ONE passphrase and can never outlive it.
    show_psk: bool,
    /// Whether a successful join is written to `wifi.conf`. On by default — that is what makes the
    /// machine come back online by itself, and it is what the old picker defaulted to. Reset with
    /// the passphrase, so it is a decision about one join.
    remember: bool,
    page: usize,
    hover: Option<NetHit>,
    /// The operation in flight: what it is, when it started, and the pid doing it.
    busy: Option<(WifiOp, usize, u64)>,
    /// The last thing the view has to say. Always about something that actually happened.
    message: String,
    /// Where each action was drawn last frame.
    ///
    /// Cached rather than recomputed, for the same reason `ent_detail_lines` is: the hit-test has no
    /// atlas in hand, and these rects are sized to measured text. Two different answers would put
    /// the click somewhere other than the word. Empty before the first paint, which is correct —
    /// there is nothing on screen to click yet.
    action_rects: Vec<(NetAction, Rect)>,
    /// When the snapshot was last re-read.
    polled: usize,
}

impl NetView {
    fn new() -> NetView {
        NetView {
            nets: Vec::new(),
            status: WifiStatus::default(),
            present: false,
            saved: None,
            sel: None,
            psk: Vec::new(),
            show_psk: false,
            remember: true,
            page: 0,
            hover: None,
            busy: None,
            message: String::new(),
            action_rects: Vec::new(),
            polled: 0,
        }
    }

    fn radio_on(&self) -> bool {
        self.present && self.status.state != WIFI_RADIO_OFF
    }
    fn connected(&self) -> bool {
        self.status.state == WIFI_CONNECTED
    }

    /// The selected network, if the selection still points at one. The list is re-sorted on every
    /// poll, so this is checked rather than assumed.
    fn selected(&self) -> Option<&WifiNetwork> {
        self.sel.and_then(|i| self.nets.get(i))
    }

    /// Whether the passphrase field is up: a secured, joinable network is selected.
    fn wants_psk(&self) -> bool {
        self.selected()
            .map(|n| n.is_secure() && network::joinable(n))
            .unwrap_or(false)
    }

    /// Whether the network we are ON advertises RSN, for the "Connected · WPA2" line. `None` when
    /// the current SSID is not in the scan list — see `network::state_detail`.
    fn current_secure(&self) -> Option<bool> {
        let name = self.status.name();
        if name.is_empty() {
            return None;
        }
        self.nets.iter().find(|n| n.name() == name).map(|n| n.is_secure())
    }

    /// Drop the passphrase and re-mask. Both together, always: clearing the text while leaving
    /// `show_psk` set means the next network's first keystroke appears in the clear.
    fn reset_psk(&mut self) {
        self.psk.clear();
        self.show_psk = false;
        // Back to remembering. A "join once" chosen for a café must not still be in force the next
        // time you pick your own network and expect the machine to come back to it.
        self.remember = true;
    }

    /// How many characters have been typed. Counts UTF-8 lead bytes, so a half-entered multi-byte
    /// sequence cannot make this disagree with what was drawn.
    fn typed(&self) -> usize {
        self.psk.iter().filter(|&&b| (b & 0xC0) != 0x80).count()
    }

    /// The actions offered right now, left to right.
    ///
    /// Rescan is absent while connected because the kernel refuses it — a scan retunes the radio off
    /// the AP's channel for seconds — and an action that cannot work is worse than one that is not
    /// offered. Everything is absent while an operation is in flight: the radio is taken, and the
    /// message line is already saying so.
    fn actions(&self) -> Vec<NetAction> {
        let mut out = Vec::new();
        if !self.present || self.busy.is_some() {
            return out;
        }
        if !self.radio_on() {
            out.push(NetAction::RadioOn);
            // Forget only rewrites a file, so it stays reachable with the radio off — it is the one
            // way to stop a machine reconnecting to a network you have left for good.
            if self.saved.is_some() {
                out.push(NetAction::Forget);
            }
            return out;
        }
        // Both of these belong to the selected network, so they are only offered while one is
        // selected and joinable — an "Join once only" with nothing selected would be a toggle over
        // nothing.
        if self.wants_psk() {
            out.push(NetAction::Reveal);
        }
        if self.selected().map_or(false, network::joinable) {
            out.push(NetAction::Remember);
        }
        if self.connected() {
            out.push(NetAction::Disconnect);
        } else {
            out.push(NetAction::Rescan);
        }
        if self.saved.is_some() {
            out.push(NetAction::Forget);
        }
        out.push(NetAction::RadioOff);
        out
    }
}

/// Whether a process is still running, by looking for its pid in the task table.
///
/// This is how the shell knows a radio operation has finished. There is no `waitpid` on this system
/// and the shell must not block on one anyway, so it asks the same question the System Monitor asks
/// — sixty tasks, once every [`NET_POLL_MS`], and only while something is actually in flight.
fn pid_alive(pid: u64) -> bool {
    // ~2.7 KB on the stack, the same pattern `Vitals::refresh` uses and for the same reason.
    let mut info: SystemInfo = unsafe { core::mem::zeroed() };
    sys_get_system_info(&mut info as *mut SystemInfo);
    let n = (info.task_count as usize).min(info.tasks.len());
    info.tasks[..n].iter().any(|t| t.pid == pid)
}

/// Fork and exec the Wi-Fi agent with one operation, returning its pid.
///
/// ★ The shell NEVER calls `sys_wifi_scan`, `sys_wifi_connect`, `sys_wifi_disconnect` or
/// `sys_wifi_set_radio` itself. All four block in the driver for seconds, and this process is the
/// window server: the desktop would stop compositing for the whole join while the hardware cursor —
/// driven from the mouse IRQ — kept gliding over the frozen frame. That is not a cosmetic problem.
/// It is exactly what the GL-runtime freeze looks like, and fifteen seconds without the 1 Hz
/// heartbeat is long enough for the render engine to enter RC6 and lose MOCS, at which point the
/// first composite afterwards really does hang.
fn launch_agent(op: WifiOp, ssid: &str, psk: &str) -> Option<u64> {
    let mut buf = [0u8; WIFI_OP_MAX];
    let n = wifi_op_encode(&mut buf, op, ssid, psk);
    let arg = core::str::from_utf8(&buf[..n]).ok()?;
    breadcrumb("[NET] pre-fork: ", op.verb());
    let pid = sys_fork();
    if pid == 0 {
        sys_execve_arg("/mnt/nvme/apps/WifiAgent.nyx/run.bin\0", arg);
        breadcrumb("[NET] execve returned, exec FAILED: ", op.verb());
        sys_exit(1);
    }
    if pid < 0 {
        return None;
    }
    Some(pid as u64)
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
    /// The Entity itself — seed, stage, influence and the sprite generated from them. Step 18.
    creature: Creature,
    /// Which of the surface's two views is up. Reset to `Root` whenever the surface closes: a
    /// disclosure that reopens three levels deep is a surface with hidden state. Step 20.
    ent_view: EntView,
    /// The network drill-down's own state, live only while `ent_view` is `Network`.
    net: NetView,
    /// The pointer position and time of the last idle sample — `(x, y, ms)`.
    ///
    /// Deliberately NOT `prev_mx`/`prev_my`: those advance once per rendered frame and answer a
    /// question about damage, not about presence. See the note in `process_input`.
    idle_probe: (i32, i32, usize),
    /// The chrome opacity of the last drawn frame, so the 90 s recede can ask for frames only
    /// while it is actually moving. Starts at 255 — fully present — which is what it is at boot.
    chrome_key: u8,
    /// Which of the Command footer's power glyphs the pointer is over — an index into
    /// [`POWER_ROWS`], which is also what `layout::cmd_power_hit` returns.
    power_hover: Option<usize>,
    /// A power action that has been clicked once and is waiting for a second click.
    ///
    /// Meridian has no dialogs — no modal box, no OK/Cancel pair — so a destructive action cannot
    /// ask "are you sure?" the way other systems do. It arms instead: the glyph lights, the footer
    /// line stops counting results and says what is about to happen, and the second click on the
    /// same glyph does it. Escape, any key, or closing the Command all disarm.
    power_armed: Option<Power>,

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

    /// Meridian step 19. When the last key or pointer movement arrived; everything about idle is
    /// arithmetic on `now - last_input`.
    last_input: usize,
    /// The phase as of the last frame, so entering one can be detected rather than re-applied every
    /// frame — the backlight drop in particular must happen once.
    phase: Phase,
    /// The panel level to put back on wake, captured the moment before sleep dropped it. `None`
    /// means sleep never touched it: either this display has no PWM backlight, or we have not slept.
    bright_before_sleep: Option<u32>,
    /// The animation key of the last drawn idle frame — `(dx, dy, lid)`. The idle screen is redrawn
    /// only when this changes, which is a handful of frames a second rather than sixty.
    idle_key: (i32, i32, bool),
    /// The stored credential, read once at startup. `None` = no password is set and the lock never
    /// appears, which is the default state of every Nyx installation.
    auth: Option<Auth>,
    /// True while the lock is covering the desktop. Windows keep running behind it — this is a
    /// display gate on a single-user machine, not a session boundary, and pretending otherwise
    /// would be a lie about what it does.
    locked: bool,
    /// The passphrase being typed. Cleared on success, on failure, and on lock.
    attempt: Vec<u8>,
    /// Consecutive failures, and when the fifth landed.
    fails: u32,
    fail_at: usize,
    /// True once a wrong passphrase has been entered, so the hint reads *Incorrect. Try again.*
    /// until the next one is accepted.
    lock_err: bool,

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
            creature: Creature::load_or_birth(h),
            ent_view: EntView::Root,
            net: NetView::new(),
            idle_probe: (w / 2, h / 2, 0),
            chrome_key: 255,
            power_hover: None,
            power_armed: None,
            bright: BrightKeys::load(),
            learn: None,
            bright_pct: None,
            ent_drag_bright: false,
            last_input: 0,
            phase: Phase::Active,
            bright_before_sleep: None,
            idle_key: (i32::MIN, i32::MIN, false),
            auth: Auth::load(),
            locked: false,
            attempt: Vec::new(),
            fails: 0,
            fail_at: 0,
            lock_err: false,
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
        // ⚠️ Whichever view is up. `entity_geometry` and `net_geometry` return DIFFERENT heights for
        // the same surface, and marking the root's rect while the network view is drawn leaves the
        // taller one's overhang stale — a band of last frame's pixels along the top edge.
        let s = match self.ent_view {
            EntView::Network => self.net_geometry().0.surface,
            EntView::Root => self.entity_geometry().surface,
        };
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
        let bright_label = alloc::format!("Brightness keys — {}", self.bright.describe());
        // ⚠️ Shut down and Restart are NOT rows here, and were briefly. A verb that ends the session
        // must not be reachable by typing three letters and pressing Return with the selection still
        // resting on the first match — which is exactly what a result row is for. They are glyphs in
        // this panel's footer instead; see `layout::cmd_power_slot`.
        let cmds: [(&str, &str, &str, Option<Action>); 4] = [
            (theme_label, Icons::APP_SETTINGS, "Command", Some(Action::ToggleTheme)),
            ("Fold all windows", Icons::WIN_FOLD, "Command", Some(Action::FoldAll)),
            // ★ Step 20. This row launched `apps/wifi` until the Entity's drill-down existed to
            // take the job; now it opens the surface on the network view. That is the whole of
            // *"deleting an application is the last step, never the first"* in one line — the row
            // that reaches the network never went away, only what it reaches.
            (&network_label, Icons::WIFI, "Choose a network", Some(Action::OpenNetwork)),
            // The current binding is in the LABEL, not the meta column, so it is searchable: typing
            // "F5" finds the row that explains why F5 does what it does.
            (&bright_label, Icons::BRIGHT, "Rebind", Some(Action::LearnBrightnessKeys)),
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
        // Opening OR closing disarms. An armed shutdown must not survive the Command being
        // dismissed and reopened — the second Return would then land on a row the user has no
        // reason to think is still loaded.
        self.power_armed = None;
        if open {
            // Opening closes the Entity: they are the two modal surfaces, and two things asking to
            // be read at once is neither of them being read.
            //
            // ⚠️ Through `set_entity`, not by clearing `ent_open` here. Since step 20 closing the
            // surface also has to leave the network view and drop the passphrase typed into it —
            // and a second path that only clears the flag is exactly how a half-typed passphrase
            // survives into the next time the surface opens.
            self.set_entity(false);
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
            Action::OpenNetwork => {
                // `set_entity(true)` first: it samples the backlight and closes the Command, and
                // `enter_network` is what puts the surface on the second level.
                self.set_entity(true);
                self.enter_network();
            }
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
        self.announce_theme_to_all();
        self.mark_full();
    }

    /// Tell every client which theme it is now.
    ///
    /// Apps draw into their own SHM with their own copy of the token table, so the shell repainting
    /// itself changes nothing inside a window. Before this existed, switching to light gave a light
    /// desktop full of dark windows.
    fn announce_theme_to_all(&self) {
        let d = if self.theme.is_dark { THEME_DARK } else { THEME_LIGHT };
        for c in self.clients.iter() {
            if c.win.exists {
                sys_ipc_send(c.owner_pid, MSG_THEME_CHANGED, d, 0);
            }
        }
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
        } else {
            // Always back to the root. A disclosure surface that reopens where it was left is one
            // with hidden state, and the Entity's whole job is to be the thing you can trust at a
            // glance. It also drops the typed passphrase and disarms any pending shutdown, both of
            // which are exactly the right moment to drop.
            self.leave_network();
        }
        self.mark_full();
    }

    // ── the network drill-down (Meridian step 20) ──

    /// Enter the network view. Reads the snapshot immediately so the panel opens with the list it
    /// already has rather than with an empty "Available" block that fills in half a second later.
    fn enter_network(&mut self) {
        self.ent_view = EntView::Network;
        self.net = NetView::new();
        self.net.message = String::from("Pick a network to join.");
        self.poll_network(0);
        self.mark_full();
    }

    /// Perform, or arm, one power action.
    ///
    /// ★ Meridian has no dialogs, so a verb that ends the session cannot ask "are you sure?". It
    /// **arms** instead: the first press lights the glyph and names itself in the footer, the second
    /// fires. Two presses on the same 30px target, with the machine saying what it is about to do in
    /// between — which is the whole of what a confirmation dialog buys, without a dialog.
    fn power_press(&mut self, p: Power) {
        if self.power_armed == Some(p) {
            self.power_armed = None;
            // The Entity is written on a 1 Hz tick, so a shutdown between ticks would drop up to a
            // second of it. The last useful thing this process does.
            self.creature.persist();
            sys_power(p.action());
            // Only reachable if the firmware declined; the kernel has already said so on screen.
        } else {
            self.power_armed = Some(p);
        }
        self.needs_redraw = true;
        self.mark_full();
    }

    fn leave_network(&mut self) {
        // The passphrase is dropped here and nowhere else — every path out of the view goes through
        // this function, which is what makes "it is not kept" a property rather than a hope.
        self.net.reset_psk();
        self.net = NetView::new();
        self.ent_view = EntView::Root;
        self.mark_full();
    }

    /// Re-read the published link snapshot and the last scan.
    ///
    /// Both syscalls read `WIFI_SNAPSHOT` and never take the driver lock, so this cannot block even
    /// while the agent is mid-join. That is the entire reason the drill-down can live in the window
    /// server; see [`launch_agent`].
    fn poll_network(&mut self, now: usize) {
        self.net.polled = now;

        match sys_wifi_status() {
            Some(s) => {
                self.net.status = s;
                self.net.present = true;
            }
            None => {
                self.net.present = false;
                self.net.message = String::from("This machine has no Wi-Fi adapter.");
            }
        }

        let mut buf = [WifiNetwork::default(); 48];
        let n = sys_wifi_list(&mut buf);
        // Remember what was selected BY NAME before the list is rebuilt. The list is re-sorted every
        // poll — the current network moves to the top the instant a join lands — so holding an index
        // across a refresh would silently move the selection to a different network, with a
        // passphrase already half-typed for the old one.
        let was = self.net.selected().map(|n| String::from(n.name()));
        self.net.nets.clear();
        for e in buf.iter().take(n) {
            self.net.nets.push(*e);
        }
        let mut buf = [0u8; 160];
        self.net.saved = match wifi_load_network(&mut buf) {
            Some((slen, _)) => core::str::from_utf8(&buf[..slen]).ok().map(String::from),
            None => None,
        };
        network::order(&mut self.net.nets, self.net.saved.as_deref());
        self.net.sel = was.and_then(|name| self.net.nets.iter().position(|n| n.name() == name));
        if self.net.sel.is_none() {
            self.net.reset_psk();
        }
    }

    /// One tick of the network view: poll, and notice when the agent has finished.
    fn tick_network(&mut self, now: usize) {
        if !self.ent_open || self.ent_view != EntView::Network {
            return;
        }
        if now.wrapping_sub(self.net.polled) < NET_POLL_MS {
            return;
        }
        let before = (self.net.status.state, self.net.status.ip, self.net.nets.len());
        self.poll_network(now);

        if let Some((op, started, pid)) = self.net.busy {
            let elapsed = now.wrapping_sub(started);
            // The pid leaving the task table is the real signal; the budget is only a backstop for
            // a helper that died in a way that left it there. The OUTCOME never comes from either —
            // it comes from the snapshot below, so a timeout that fires early cannot invent a
            // result, it can only stop saying "Connecting…" too soon.
            if !pid_alive(pid) || elapsed > op.budget_ms() {
                self.net.busy = None;
                self.net.message = self.net_outcome(op);
                if op.is_join() && self.net.connected() {
                    self.net.sel = None;
                    self.net.reset_psk();
                }
            }
        }

        let after = (self.net.status.state, self.net.status.ip, self.net.nets.len());
        if before != after || self.net.busy.is_some() {
            self.needs_redraw = true;
            self.mark_full();
        }
    }

    /// What to say now that `op` has finished. Read from the link state, never from what we asked
    /// for — the kernel refuses any radio operation while another owns the radio, and reports the
    /// state unchanged when it does, so "we asked for it" is not evidence it happened.
    fn net_outcome(&self, op: WifiOp) -> String {
        match op {
            WifiOp::Scan => {
                if self.net.nets.is_empty() {
                    String::from("Scan finished. No networks in range.")
                } else {
                    alloc::format!("Scan finished. {} networks in range.", self.net.nets.len())
                }
            }
            WifiOp::Join | WifiOp::JoinOnce => match self.net.status.state {
                WIFI_CONNECTED => {
                    alloc::format!("Connected to \"{}\".", self.net.status.name())
                }
                WIFI_AUTH_FAILED => String::from("The password was rejected. Try again."),
                WIFI_NO_LEASE => {
                    String::from("Joined, but the router never offered an address.")
                }
                WIFI_HW_FAILED => String::from("The adapter refused to tune to that network."),
                _ => String::from("The join did not complete."),
            },
            WifiOp::Leave => String::from("Disconnected."),
            WifiOp::RadioOn => {
                if self.net.radio_on() {
                    String::from("Wi-Fi is on. Rescan to look for networks.")
                } else {
                    String::from("The radio was busy. Try again.")
                }
            }
            WifiOp::RadioOff => {
                if self.net.radio_on() {
                    String::from("The radio was busy. Try again.")
                } else {
                    String::from("Wi-Fi is off.")
                }
            }
        }
    }

    /// Hand an operation to the agent. Everything the view does that touches the radio goes through
    /// here, so there is exactly one place that can start one and exactly one that can be busy.
    fn start_op(&mut self, op: WifiOp, now: usize) {
        if self.net.busy.is_some() {
            return;
        }
        let (ssid, psk) = if op.is_join() {
            let Some(n) = self.net.selected() else { return };
            let ssid = String::from(n.name());
            let psk = String::from(core::str::from_utf8(&self.net.psk).unwrap_or(""));
            (ssid, psk)
        } else {
            (String::new(), String::new())
        };
        match launch_agent(op, &ssid, &psk) {
            Some(pid) => {
                self.net.busy = Some((op, now, pid));
                self.net.message = match op {
                    WifiOp::Scan => String::from("Scanning every channel\u{2026}"),
                    WifiOp::Join | WifiOp::JoinOnce => {
                        alloc::format!("Joining \"{}\"\u{2026}", ssid)
                    }
                    WifiOp::Leave => String::from("Disconnecting\u{2026}"),
                    WifiOp::RadioOn => String::from("Turning Wi-Fi on\u{2026}"),
                    WifiOp::RadioOff => String::from("Turning Wi-Fi off\u{2026}"),
                };
            }
            None => {
                // Naming the helper matters: this is the one failure here that is not about the
                // radio at all, and "couldn't connect" would send someone to look at the router.
                self.net.message = String::from("Couldn't start the Wi-Fi agent.");
            }
        }
        self.needs_redraw = true;
        self.mark_full();
    }

    /// The network view's geometry, and which slice of the list it is showing.
    ///
    /// Recomputed from the same inputs everywhere it is needed rather than cached, so the frame that
    /// draws a row and the click that lands on it cannot disagree.
    fn net_geometry(&self) -> (PanelLayout, Page) {
        let links = network::link_rows(&self.net.status).len();
        let psk = self.net.wants_psk();
        let cap = network::max_rows(self.screen_h, links, psk);
        let page = network::paginate(self.net.nets.len(), cap, self.net.page);
        // ⚠️ `.max(1)` — an empty list still needs one row, because that is where "Nothing in range"
        // goes. Without it `avail_row(0)` returns `None` and the empty state draws nothing at all:
        // a hairline, a heading, and then the footer. Which reads as the scan having broken rather
        // than as it having found nothing, and those are opposite messages.
        let rows = page.rows().max(1);
        let l = network::panel_layout(self.screen_w, self.screen_h, links, rows, psk);
        (l, page)
    }

    /// What the pointer is over. `action_rects` is what the last frame drew — see the field's note.
    fn net_hit(&self, mx: i32, my: i32) -> Option<NetHit> {
        let (l, page) = self.net_geometry();
        if l.back.contains(mx, my) {
            return Some(NetHit::Back);
        }
        if let Some(i) = l.row_hit(mx, my) {
            return if page.has_pager() && i == page.len {
                Some(NetHit::Pager)
            } else {
                Some(NetHit::Row(i))
            };
        }
        for (i, (_, r)) in self.net.action_rects.iter().enumerate() {
            if r.contains(mx, my) {
                return Some(NetHit::Action(i));
            }
        }
        None
    }

    /// A click inside the network view.
    fn net_press(&mut self, mx: i32, my: i32, now: usize) {
        let Some(hit) = self.net_hit(mx, my) else { return };
        let (_, page) = self.net_geometry();
        match hit {
            NetHit::Back => self.leave_network(),
            NetHit::Pager => {
                self.net.page = self.net.page.wrapping_add(1);
                self.needs_redraw = true;
                self.mark_full();
            }
            NetHit::Row(i) => {
                let idx = page.start + i;
                if self.net.sel == Some(idx) {
                    // Clicking the selected row again puts it down. There is no Cancel button in
                    // this language, so the way out of a selection is the way into it.
                    self.net.sel = None;
                    self.net.reset_psk();
                    self.net.message = String::from("Pick a network to join.");
                } else if let Some(n) = self.net.nets.get(idx) {
                    let refusal = network::refusal(n);
                    let secure = n.is_secure();
                    self.net.sel = Some(idx);
                    self.net.reset_psk();
                    self.net.message = match refusal {
                        // The obstacle, not the standard. "Unsupported security" tells nobody what
                        // to do; naming the router setting does.
                        Some(why) => String::from(why),
                        None if secure => String::from("Type the password, then press Return."),
                        None => String::from("Open network. Press Return to join."),
                    };
                }
                self.needs_redraw = true;
                self.mark_full();
            }
            NetHit::Action(i) => {
                let Some(&(action, _)) = self.net.action_rects.get(i) else { return };
                match action {
                    NetAction::Rescan => self.start_op(WifiOp::Scan, now),
                    NetAction::Disconnect => self.start_op(WifiOp::Leave, now),
                    NetAction::RadioOn => self.start_op(WifiOp::RadioOn, now),
                    NetAction::RadioOff => self.start_op(WifiOp::RadioOff, now),
                    NetAction::Reveal => {
                        self.net.show_psk = !self.net.show_psk;
                        self.needs_redraw = true;
                        self.mark_full();
                    }
                    NetAction::Remember => {
                        self.net.remember = !self.net.remember;
                        self.net.message = if self.net.remember {
                            String::from("This network will be remembered and rejoined at boot.")
                        } else {
                            // Naming the consequence, not the setting: "one network is remembered"
                            // is the fact that makes the choice matter at all.
                            String::from(
                                "Joining once. The remembered network will not be replaced.",
                            )
                        };
                        self.needs_redraw = true;
                        self.mark_full();
                    }
                    NetAction::Forget => {
                        // Forget rewrites a file and blocks on nothing, so the shell does it itself
                        // — a whole process to truncate 40 bytes would be ceremony.
                        self.net.message = if wifi_forget_network() {
                            self.net.saved = None;
                            String::from("Forgotten. This machine won't reconnect on its own.")
                        } else {
                            String::from("Couldn't update the saved-network file.")
                        };
                        self.needs_redraw = true;
                        self.mark_full();
                    }
                }
            }
        }
    }

    /// A key while the network view is up. Returns whether it was consumed.
    fn net_key(&mut self, key: char, now: usize) -> bool {
        if self.net.busy.is_some() {
            // Everything is in flight. Swallowing the key rather than queueing it is deliberate:
            // a passphrase typed at a frozen field and applied ten seconds later is worse than one
            // that visibly did nothing.
            return true;
        }
        match key {
            '\x1b' => {
                self.leave_network();
                true
            }
            '\n' | '\r' => {
                let Some(n) = self.net.selected() else { return true };
                if !network::joinable(n) {
                    return true;
                }
                if n.is_secure() && self.net.psk.len() < 8 {
                    // WPA2-PSK is 8..63 characters. Refusing here saves a ten-second handshake
                    // whose only outcome could have been "the password was rejected".
                    self.net.message =
                        String::from("A WPA2 password is at least 8 characters.");
                    self.needs_redraw = true;
                    self.mark_full();
                    return true;
                }
                let op = if self.net.remember { WifiOp::Join } else { WifiOp::JoinOnce };
                self.start_op(op, now);
                true
            }
            '\x08' => {
                if !self.net.wants_psk() {
                    return true;
                }
                // Pop a whole character, not a byte — the field counts characters and a lone
                // continuation byte left behind is a string that is no longer UTF-8.
                while self.net.psk.pop().map_or(false, |b| (b & 0xC0) == 0x80) {}
                self.needs_redraw = true;
                self.mark_full();
                true
            }
            c if (c as u32) >= 0x20 && (c as u32) < 0x7f => {
                if self.net.wants_psk() && self.net.psk.len() < 63 {
                    self.net.psk.push(c as u8);
                    self.needs_redraw = true;
                    self.mark_full();
                }
                true
            }
            // Everything else — the arrow keys and the rest of the PUA block — is swallowed rather
            // than falling through to the desktop underneath. The surface is modal while it is up.
            _ => true,
        }
    }

    /// Sample the machine and re-wrap what the Entity has to say about it. Once a second: every
    /// input here changes on human timescales, and the frame is being recomposited anyway.
    fn refresh_vitals(&mut self, atlas: Option<&Atlas>) {
        self.vitals.refresh(self.fps, self.idle_ms(sys_get_time()));
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
                    // Straight after the ack, so an app launched into an already-light session
                    // paints light on its very first frame instead of flashing dark once.
                    sys_ipc_send(
                        msg.sender_pid,
                        MSG_THEME_CHANGED,
                        if self.theme.is_dark { THEME_DARK } else { THEME_LIGHT },
                        0,
                    );
                }
                MSG_SET_THEME => {
                    // A request from an app — Settings. The shell owns the theme because it owns the
                    // wallpaper texture, so this goes through the same `set_theme` the Command uses
                    // and the requester learns the result from the broadcast like everyone else.
                    let want_dark = match msg.data1 {
                        THEME_DARK => true,
                        THEME_LIGHT => false,
                        _ => !self.theme.is_dark,
                    };
                    if want_dark != self.theme.is_dark {
                        self.set_theme(if want_dark { Theme::dark() } else { Theme::light() });
                    }
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

    // ── idle, sleep and the lock (Meridian step 19) ──

    /// How long since the last key or pointer movement.
    fn idle_ms(&self, now: usize) -> usize {
        now.saturating_sub(self.last_input)
    }

    /// Advance the state machine. Called once per loop, after input.
    ///
    /// Only *transitions* do work here. Everything continuous — the fades, the drift, the float — is
    /// a pure function of `idle_ms` computed at draw time, so nothing has to be stepped and nothing
    /// can drift out of sync with the clock.
    fn tick_idle(&mut self, now: usize) {
        let next = idle::phase_for(self.idle_ms(now));
        if next == self.phase {
            return;
        }
        let prev = self.phase;
        self.phase = next;

        // Entering sleep: drop the panel. ⚠️ 15%, not 0. *"A screen that looks asleep on a machine
        // that is fully awake will get carried into a bag, and the difference is a hot laptop."*
        // Nyx has no ACPI S3 — the machine stays on and only the display darkens, so the panel must
        // stay visibly lit.
        if next == Phase::Asleep {
            // `bright_pct` is None on a display with no PWM backlight. Nothing to drop, nothing to
            // restore, and no pretending otherwise.
            if let Some(before) = self.bright_pct {
                self.bright_before_sleep = Some(before);
                sys_backlight_set(15);
                self.bright_pct = Some(15);
            }
        } else if prev == Phase::Asleep {
            if let Some(before) = self.bright_before_sleep.take() {
                let got = sys_backlight_set(before);
                self.bright_pct = got.or(self.bright_pct);
            }
        }

        // Every phase change repaints the whole screen: the recede fade, the cross-fade to black and
        // the wake all cover everything.
        self.needs_redraw = true;
        self.mark_full();
    }

    /// Characters typed into the lock, not bytes. Counts the non-continuation bytes of the UTF-8 in
    /// `attempt`, so it cannot fail on a partial sequence and needs no second copy of the string.
    fn typed_chars(&self) -> usize {
        self.attempt.iter().filter(|b| (**b & 0xC0) != 0x80).count()
    }

    /// Everything about the idle screen that can change between frames: where the creature sits and
    /// whether its eyes are shut. Two frames with the same key are the same picture.
    fn idle_anim_key(&self, now: usize) -> (i32, i32, bool) {
        let asleep = self.phase == Phase::Asleep;
        let (dx, dy) = self.creature.idle_offset(now, asleep);
        let blink = self.creature.state.genome().personality.blink_ticks;
        (dx, dy, asleep || idle::blink_closed(now, blink))
    }

    /// Any key, or any pointer movement. Straight back to the desktop if no password is set; to the
    /// lock if one is.
    fn wake(&mut self, now: usize) {
        self.last_input = now;
        // Re-anchor here too. `wake` is also reached from the key path, which never touches the
        // probe — leaving it at wherever the pointer was five minutes ago would make the very next
        // sample look like a large movement and hold the machine awake for no reason.
        self.idle_probe = (self.mx, self.my, now);
        if self.phase.shows_creature() && self.auth.is_some() && !self.locked {
            self.lock();
        }
        self.tick_idle(now);
        self.needs_redraw = true;
        self.mark_full();
    }

    fn lock(&mut self) {
        self.locked = true;
        self.attempt.clear();
        self.lock_err = false;
        // ⚠️ Failures are NOT reset by locking. Otherwise the thirty-second penalty is escaped by
        // waiting for the machine to idle again, which takes less than thirty seconds of doing
        // nothing.
        self.needs_redraw = true;
        self.mark_full();
    }

    /// Every keystroke while the lock is up.
    fn on_lock_key(&mut self, key: char, now: usize) {
        if idle::penalty_remaining(self.fails, self.fail_at, now).is_some() {
            // Rate limiting rather than theatre. The field simply does not accept input; the hint
            // line is already saying so.
            return;
        }
        match key {
            '\n' | '\r' => {
                let ok = self.auth.as_ref().is_some_and(|a| a.matches(&self.attempt));
                self.attempt.clear();
                if ok {
                    self.locked = false;
                    self.fails = 0;
                    self.lock_err = false;
                } else {
                    self.lock_err = true;
                    self.fails += 1;
                    if self.fails >= idle::MAX_FAILURES {
                        self.fail_at = now;
                    }
                }
            }
            // `pc_keyboard` decodes Backspace through the Unicode branch as U+0008, so it arrives on
            // the ordinary char queue. A password field needs nothing added to the input path.
            '\u{8}' => { self.attempt.pop(); }
            c if !c.is_control() => {
                // Bounded so a stuck key cannot grow this without limit. Far above any real
                // passphrase, and silent — telling an onlooker the length is the disclosure the
                // design's "no username, no avatar" rule exists to avoid.
                if self.attempt.len() < 128 {
                    let mut b = [0u8; 4];
                    self.attempt.extend_from_slice(c.encode_utf8(&mut b).as_bytes());
                }
            }
            _ => {}
        }
        self.needs_redraw = true;
        self.mark_full();
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
        // ⚠️ `process_input` runs at ~500 Hz, not once per frame. That is why idle is timed from
        // here and not from the render loop: a wake has to be felt on the *next* frame, and polling
        // the clock at 60 Hz would put up to 16 ms of slack in front of the one interaction where
        // responsiveness is the whole point.
        let was = self.phase;

        if let Some(key) = sys_read_key() {
            self.last_input = now;
            // The lock OWNS the keyboard, exactly as the Command does. Nothing reaches the desktop
            // or the focused app while it is up — that is what makes it a gate rather than a
            // decoration painted over live windows.
            if self.locked {
                self.on_lock_key(key, now);
            } else if was.shows_creature() {
                // The keystroke that wakes the machine is consumed by the waking. Anything else
                // means the first character of whatever you typed to wake it up lands in whatever
                // window happened to be focused five minutes ago.
                self.wake(now);
            } else {
                self.on_key(key, now);
            }
        }

        let (mx_raw, my_raw, left, _right) = sys_get_mouse();
        self.mx = (mx_raw as i32).clamp(0, self.screen_w - 1);
        self.my = (my_raw as i32).clamp(0, self.screen_h - 1);
        self.left = left;

        let moved = self.mx != self.prev_mx || self.my != self.prev_my;
        if moved && !self.hw_cursor {
            let pad = 20;
            self.mark_dirty(self.prev_mx - pad, self.prev_my - pad, pad * 2, pad * 2);
            self.mark_dirty(self.mx - pad, self.my - pad, pad * 2, pad * 2);
        }

        // ★★ THE IDLE INPUT TEST. Rewritten 2026-09-08: the desktop never idled on hardware.
        //
        // ⚠️ It used to be `if moved || left`, reusing the `moved` above — and `moved` compares
        // against `prev_mx`/`prev_my`, which are advanced ONCE PER RENDERED FRAME at the foot of the
        // main loop. That is a *rendering* variable answering an *input* question, and the two want
        // opposite things: rendering wants to know "has the pointer left the rect I last drew it
        // in", idle wants "has a person touched this machine". One field cannot be both, and while
        // the 1 Hz heartbeat usually re-syncs it within a second, nothing guarantees that.
        //
        // ⚠️ And a pixel is not a person. This is a Latitude with a trackpoint: a resting palm, or
        // simple electrical drift, produces a steady dribble of one-pixel motion that the hardware
        // cursor is far too small to make visible. Any test that treats a single pixel as presence
        // pins `last_input` to now, forever, and the machine can never idle — which is exactly the
        // symptom. So presence is a question of SPEED, not distance: sample the pointer once a
        // second and ask whether it covered real ground. A hand covers hundreds of pixels; drift
        // covers three.
        if left {
            // A button is unambiguous. No threshold, no sampling window.
            self.last_input = now;
            self.idle_probe = (self.mx, self.my, now);
            if was.shows_creature() {
                self.wake(now);
            }
        } else if was.shows_creature() {
            // Waking is the one interaction where latency is the whole point, so it does not wait
            // for the sampling window — but it still needs a threshold, or the same drift that
            // stopped the screen appearing would tear it away again a second later.
            let d = (self.mx - self.idle_probe.0).abs() + (self.my - self.idle_probe.1).abs();
            if d >= IDLE_WAKE_PX {
                self.last_input = now;
                self.idle_probe = (self.mx, self.my, now);
                self.wake(now);
            }
        } else if now.wrapping_sub(self.idle_probe.2) >= IDLE_PROBE_MS {
            let d = (self.mx - self.idle_probe.0).abs() + (self.my - self.idle_probe.1).abs();
            if d >= IDLE_MOVE_PX {
                self.last_input = now;
            }
            // Re-anchored either way. Anchoring only on the trip would let a slow creep drag the
            // anchor along behind it and read as continuous presence.
            self.idle_probe = (self.mx, self.my, now);
        }

        // The lock swallows the pointer as well as the keyboard. Without this a click lands on a
        // window that is not on screen, which is both a security hole and — because the click would
        // raise and focus something invisible — an excellent way to make the lock look broken.
        // ⚠️ `prev_mx`/`prev_my` are advanced once per FRAME at the foot of the main loop, not here
        // — `process_input` runs at ~500 Hz and would otherwise clear `moved` before the frame that
        // has to act on it. So this returns without touching them.
        if self.locked {
            self.prev_left = self.left;
            self.cursor = Shape::Pointer;
            return;
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
    fn on_key(&mut self, key: char, now: usize) {
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
            // ★ ANY key disarms a pending power glyph — including Return, unlike the row version
            // this replaced. The glyphs are pointer-only targets with no keyboard path in, so the
            // moment attention moves back to the query the arming is stale, and a shutdown that is
            // still loaded while you are typing a search is a shutdown waiting for a stray click.
            if self.power_armed.is_some() {
                self.power_armed = None;
                self.mark_full();
                self.needs_redraw = true;
            }
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

        // The network view owns the keyboard while it is up — a passphrase is being typed, and a
        // key that leaked past it would land in whatever window was focused before the surface
        // opened. Tested before the Entity's own Escape so that Escape steps BACK one level rather
        // than closing the surface outright.
        if self.ent_open && self.ent_view == EntView::Network {
            self.net_key(key, now);
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
            let body = layout::command_body_height(&kinds);
            // ⚠️ The footer glyphs are tested BEFORE `command_hit`. They sit inside the panel, and
            // the result list is the thing everything else in here falls through to — so a glyph
            // tested second is a glyph that never fires.
            if let Some(i) = layout::cmd_power_hit(self.screen_w, body, mx, my) {
                if let Some(p) = POWER_ROWS.get(i).copied() {
                    self.power_press(p);
                }
            } else if let Some(i) = layout::command_hit(self.screen_w, &kinds, mx, my) {
                // Any click that is not on the armed glyph disarms it. A shutdown left loaded while
                // the pointer wanders the result list is a shutdown one mis-click away.
                self.power_armed = None;
                self.run(i);
            } else if !layout::command_panel(self.screen_w, body).contains(mx, my) {
                self.set_command(false);
            } else {
                self.power_armed = None;
                self.mark_full();
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
        if self.ent_open && self.ent_view == EntView::Network {
            let (l, _) = self.net_geometry();
            if l.surface.contains(mx, my) {
                self.net_press(mx, my, now);
            } else {
                self.set_entity(false);
            }
            return;
        }
        if self.ent_open {
            let l = self.entity_geometry();
            if let Some(row) = l.control_hit(mx, my) {
                // Network drills into the network view — step 20, and the reason `apps/wifi` could
                // finally be deleted. Brightness is a control in its own right. Power is a READOUT
                // and stays one: the verbs that END a session live in the Command's footer, where
                // the Super key reaches them, not on the surface that reports what the machine IS.
                // Volume has no driver at all.
                if row == ENT_NETWORK_ROW {
                    self.enter_network();
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

            // ★ The scrollbar is tested BEFORE the resize edge, and the order is the whole reason
            // the bar can sit against the wall instead of floating ~14px inside it. `scrollbar_hit`
            // declines when there is nothing to scroll and reserves both corners, so the resizer
            // keeps everything it actually needs — see that function. Draw and hit-test go through
            // the same geometry, so the column that lights up is the column that grabs.
            let (content_h, _) = client_scroll(&self.clients[idx]);
            if layout::scrollbar_hit(w_r, content_h as i32, w_r.h, mx, my) {
                self.scrolling = Some(idx);
                let target = layout::scroll_from_mouse(w_r, content_h as i32, w_r.h, my);
                sys_ipc_send(self.clients[idx].owner_pid, MSG_SCROLL, target as u64, 0);
                self.mark_win(w_r);
                clicked = Some(idx);
                break;
            }

            let e = resize_edges(&self.clients[idx].win, mx, my);
            if e != 0 {
                self.resizing = Some(idx);
                self.resize_edges = e;
                clicked = Some(idx);
                break;
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
                    let target = layout::scroll_from_mouse(r, content_h as i32, r.h, self.my);
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
        } else if self.ent_open {
            // The Entity takes the pointer entirely, exactly as the Command does. Without this the
            // resize arrows of a window UNDERNEATH the surface show through it — and the click that
            // follows lands on the surface, so the cursor was promising something the click would
            // not do. The network view's passphrase field is the one region here with an editable
            // shape of its own.
            let text = self.ent_view == EntView::Network
                && self
                    .net_geometry()
                    .0
                    .psk_field()
                    .map_or(false, |f| f.contains(self.mx, self.my));
            if text {
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
                // Same order as the click path above, and it has to be: the cursor is the promise
                // the click keeps. Showing a resize arrow over a column that scrolls was the
                // original complaint, and it comes straight back if these two disagree.
                let (content_h, _) = client_scroll(client);
                let w_r = client.win.r;
                if !client.win.folded
                    && layout::scrollbar_hit(w_r, content_h as i32, w_r.h, self.mx, self.my)
                {
                    break; // Shape::Pointer — the bar is not a resize affordance
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
            let body = layout::command_body_height(&kinds);
            let glyph = layout::cmd_power_hit(self.screen_w, body, self.mx, self.my);
            // A pointer on a power glyph is NOT also on a result row — same reason the press path
            // tests the glyphs first.
            let new = if glyph.is_some() {
                None
            } else {
                layout::command_hit(self.screen_w, &kinds, self.mx, self.my)
            };
            if new != self.cmd_hover || glyph != self.power_hover {
                self.cmd_hover = new;
                self.power_hover = glyph;
                self.mark_command();
            }
            return;
        }
        if self.ent_open && self.ent_view == EntView::Network {
            let new = self.net_hit(self.mx, self.my);
            if new != self.net.hover {
                self.net.hover = new;
                // The whole SURFACE, not the whole screen and not just the row: a hover moves a
                // plate off one row and onto another, and the action words underline in a different
                // section entirely. `mark_entity` now covers whichever view is up.
                self.mark_entity();
                self.needs_redraw = true;
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
    // Start the idle clock now rather than at zero. `sys_get_time` is uptime and the shell starts
    // some seconds into it, so a zero would hand the desktop 90 seconds of credit it never earned.
    state.last_input = last_frame;

    sys_print("[SHELL] Meridian online.\n");

    loop {
        let now = sys_get_time();
        state.process_ipc();
        state.process_input(now);
        state.update();
        state.tick_osd(now);
        state.tick_idle(now);
        // The network view's own beat — 2.5 Hz, and only while it is open. It reads the published
        // Wi-Fi snapshot, which never touches the driver lock, and notices when the agent process
        // holding the radio has exited. See `Shell::tick_network`.
        state.tick_network(now);

        // The idle screen animates, so it needs frames — but not sixty of them. The creature moves
        // when the drift ticks a pixel (about once every six seconds), when the float steps (a few
        // times a second) or when the lid drops (twice per blink cycle). Redrawing on a change of
        // that key rather than unconditionally is the difference between a screensaver that costs a
        // few frames a second and one that pins the GPU all night on a laptop with a thermal
        // history.
        //
        // ⚠️ This does NOT stop the 1 Hz heartbeat, and must not be "optimised" into doing so — see
        // the note on that block below. The design's *"the framebuffer stops being redrawn except
        // for the drift"* is deliberately not implemented: letting the render engine idle into RC6
        // loses MOCS and forcewake, and the first composite after waking hangs in `wait_for_idle`
        // while the hardware cursor keeps moving. Indistinguishable from a freeze, and it would only
        // ever be found on hardware after a fifteen-minute wait.
        // ⚠️ `locked` is in this condition as well as the phase. The lock screen shows the same
        // creature and the same float, but `phase` is Active while someone is standing there typing
        // — so keying off the phase alone would animate it at the 1 Hz heartbeat and the breath
        // would read as a stutter.
        if state.phase.shows_creature() || state.locked {
            let key = state.idle_anim_key(now);
            if key != state.idle_key {
                state.idle_key = key;
                state.needs_redraw = true;
                state.mark_full();
            }
        }

        // ★ The recede needs FRAMES. `chrome_alpha` steps over 20 of them once past 90 s, and
        // between the 1 Hz heartbeat and the pointer standing still there is nothing else asking for
        // a repaint at that moment — so without this the dock would fade in twenty one-second jumps
        // instead of a third of a second, which reads as the desktop glitching rather than settling.
        //
        // Only while it is actually moving: the alpha is constant at 255 before 90 s and at 0 after
        // the ramp, so this asks for nothing at all except during the ramp itself.
        {
            let a = idle::chrome_alpha(state.idle_ms(now));
            if a != state.chrome_key {
                state.chrome_key = a;
                state.needs_redraw = true;
                state.mark_dock();
            }
        }

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
            // The Entity ages on the same beat. `tick` is cheap when nothing is due — it compares
            // whole hours of uptime against a counter — and it is the only thing that ever writes
            // `entity.bin`, so a birth that failed to persist gets retried here every second until
            // it lands rather than silently striking a new seed on the next boot.
            state.creature.tick(now);
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

            // The scrollbar, in Meridian's idiom rather than `nyx_gui`'s 12px flush widget — see
            // `layout::scrollbar_track`. No container: a hairline track and a rounded thumb, inset
            // so it is a mark ON the surface instead of something stuck to its edge, and so it
            // clears the resize band that would otherwise steal two thirds of it.
            let (content_h, scroll_off) = client_scroll(&state.clients[i]);
            let track = layout::scrollbar_track(r);
            if let Some(thumb) =
                layout::scrollbar_thumb(track, content_h as i32, r.h, scroll_off as i32)
            {
                if track.w > 0 && track.h > 0 && track.x >= 0 && track.y >= 0 {
                    let radius = (track.w / 2).max(1) as usize;
                    shapes::fill_round_rect(
                        &mut canvas,
                        track.x as usize,
                        track.y as usize,
                        track.w as usize,
                        track.h as usize,
                        radius,
                        theme.line,
                    );
                    // The thumb lifts a tier while it is being dragged. On a long list a drag moves
                    // the content by a few pixels, which is not obviously a response; this is what
                    // says the grab was picked up.
                    let live = state.scrolling == Some(i);
                    shapes::fill_round_rect(
                        &mut canvas,
                        thumb.x.max(0) as usize,
                        thumb.y.max(0) as usize,
                        thumb.w as usize,
                        thumb.h as usize,
                        radius,
                        if live { theme.fg_3 } else { theme.fg_4 },
                    );
                }
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
        let name = String::from(state.clients[i].win.name());
        let cap_y = centre_text_y(cap.y, cap.h, tokens::CAPTION);
        let mut pen = cap.x + layout::caption_pad_x();
        labels.push(Label {
            x: pen,
            y: cap_y,
            style: tokens::CAPTION,
            text: name.clone(),
            color: if focused { theme.fg } else { theme.fg_3 },
        });
        pen += measure(atlas, tokens::CAPTION, &name);

        // `.w-cap .sep` then `.w-cap .ctx` — the window's own context, read live from its header.
        // An app that sets none gets neither, not even the separator: the dot is punctuation between
        // two runs, and a dot with nothing after it reads as a rendering fault.
        let (ctx, dirty) = client_context(&state.clients[i]);
        // The caption line is shared with the two window controls, which are right-aligned on it and
        // appear on hover. A context long enough to reach them would be drawn straight through
        // FOLD and CLOSE — so the run is ellipsized to stop short of the first one, with room left
        // for the unsaved tick that may follow it.
        let ctl_left = layout::control(r, 0).map(|b| b.x).unwrap_or(cap.right() - layout::caption_pad_x());
        let ctx_budget = ctl_left
            - layout::caption_gap()
            - if dirty { layout::fold_tick() + layout::caption_gap() } else { 0 }
            - (pen + layout::caption_gap() + layout::caption_sep() + layout::caption_gap());
        let ctx = &fit(atlas, tokens::B3, ctx, ctx_budget);
        if !ctx.is_empty() {
            let d = layout::caption_sep();
            pen += layout::caption_gap();
            // Drawn CPU-side rather than as a quad: it is two pixels, and it is not a glyph.
            shapes::fill_round_rect(
                &mut canvas,
                pen.max(0) as usize,
                (cap_y + tokens::CAPTION.line() as i32 / 2 - d / 2).max(0) as usize,
                d as usize,
                d as usize,
                (d / 2).max(1) as usize,
                theme.fg_4,
            );
            pen += d + layout::caption_gap();
            labels.push(Label {
                x: pen,
                y: cap_y,
                style: tokens::B3,
                text: String::from(ctx),
                color: if focused { theme.fg_3 } else { theme.fg_4 },
            });
            pen += measure(atlas, tokens::B3, ctx);
        }

        // Unsaved: the SAME 3px accent tick a folded window carries. One appearance in the system
        // for "there is something here you have not dealt with", which is the design's own ask.
        if dirty {
            pen += layout::caption_gap();
            canvas.fill_rect(
                pen.max(0) as usize,
                (cap_y + tokens::CAPTION.line() as i32 / 2).max(0) as usize,
                layout::fold_tick() as usize,
                1,
                theme.accent,
            );
        }

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

    // ★★ THE RECEDE, Meridian step 19 — *"after 90 s the dock and the Entity mark fade out; windows
    //    stay untouched, nothing has been hidden."*
    //
    // ⚠️ This was SPECIFIED, host-tested in `idle::chrome_alpha`, and then never wired to anything.
    // `chrome_alpha` had no caller at all, so `Phase::Receded` was a state the machine entered and
    // did nothing about — which is why leaving the desktop alone appeared to do nothing at 90 s even
    // when the clock underneath was running perfectly.
    //
    // ⚠️ It fades by COLOUR, not by alpha, and it has to. The batched glyph shader carries one
    // luminance per quad and discards the requested alpha (see `fade_to`), so an alpha ramp on these
    // labels would be a no-op that looked like the recede failing again. Lerping toward the ground
    // is a luminance change, which is exactly what that shader can express.
    let chrome_a = idle::chrome_alpha(state.idle_ms(sys_get_time())) as u32;
    let ground = theme.void;

    // 6. The dock: eight glyphs resting on the wallpaper. No container — not a pill, not a bar.
    //    Skipped outright once fully receded: at alpha 0 every glyph would be exactly the ground
    //    colour, so this is the same picture for less work.
    for slot in 0..if chrome_a > 0 { layout::DOCK_SLOTS } else { 0 } {
        let s = match layout::dock_slot(state.screen_h, slot) {
            Some(s) => s,
            None => continue,
        };
        if layout::is_divider(slot) {
            shapes::hairline_v(&mut canvas, s.x as usize, s.y as usize, s.h as usize,
                               fade_to(theme.line, ground, chrome_a));
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
            color: fade_to(
                if is_active {
                    theme.fg
                } else if hovered || running {
                    theme.fg_2
                } else {
                    theme.fg_4
                },
                ground,
                chrome_a,
            ),
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
                    // The running dot IS a `fill_rect`, so unlike the glyphs it can carry a real
                    // alpha — but it is faded the same way as everything else beside it, or the
                    // accent dot would be the one thing left burning on a receded dock.
                    fade_to(if is_active { theme.accent } else { theme.fg_4 }, ground, chrome_a),
                );
            }
        }
    }

    // 7. The Entity: one 22px mark and a two-word phrase at bottom-right. This is the entire
    //    permanent chrome on that side of the screen — there is no tray, because this IS the tray.
    // Copied out rather than held as a borrow of `state`: since step 20 the surface below may be
    // drawn by `net_paint`, which needs `&mut Shell` to cache where it put its action words. Both
    // are `Copy`, so this costs nothing and keeps the borrow out of the way.
    let phrase = state.vitals.phrase;
    let mood = state.vitals.mood;
    let mark = layout::entity_mark(state.screen_w, state.screen_h);
    // Faded on the same ramp as the dock. The mark is the other half of the permanent chrome, and
    // the design fades them together — one of the two staying lit would read as a notification.
    if chrome_a > 0 {
        let pw = measure(atlas, tokens::B3, phrase);
        let p = layout::entity_phrase(state.screen_w, state.screen_h, pw);
        labels.push(Label {
            x: p.x,
            y: centre_text_y(p.y, p.h, tokens::B3),
            style: tokens::B3,
            text: String::from(phrase),
            color: fade_to(
                if mood == Mood::Attention { theme.fg_2 } else { theme.fg_3 },
                ground,
                chrome_a,
            ),
        });
    }
    if chrome_a == 0 {
        // Fully receded: no mark at all. Falling through would draw it at full strength, because
        // the attention branch below blends CPU-side and the normal branch pushes a glyph.
    } else if mood == Mood::Attention {
        // ★ THE ONE DOCUMENTED EXCEPTION. In the attention state the mark is accent-tinted, and a
        // tinted glyph cannot ride the batched text path: `build_ps_text` interpolates a single
        // luminance across the quad, so that shader is greyscale-only and the accent would arrive as
        // grey. So this one mark leaves the fast path and is blended CPU-side — a single 22px glyph
        // per frame, against a budget of about a million pixels. Every other accent in Meridian is a
        // `fill_rect` (the caret, the running dot, the selection bar) and is unaffected.
        if let Some(ic) = icons::get(mood.glyph(), layout::entity_mark_px() as usize) {
            let px: Vec<u32> = ic.cov.iter().map(|&c| (c as u32) << 24).collect();
            canvas.blit_rgba(
                (mark.x + ic.off_x as i32).max(0) as usize,
                (mark.y + ic.off_y as i32).max(0) as usize,
                &px,
                ic.w as usize,
                ic.h as usize,
                Some(fade_to(theme.accent, ground, chrome_a)),
            );
        }
    } else {
        icons_out.push(IconDraw {
            id: mood.glyph(),
            px: layout::entity_mark_px() as usize,
            x: mark.x,
            y: mark.y,
            color: fade_to(theme.fg_3, ground, chrome_a),
        });
    }

    // 7b. The Entity surface, when disclosed: a statement, then measurements separated by hairlines
    //     and space. Not a card grid.
    //
    //     ★ Step 20 gave the surface a SECOND view. `EntView::Network` replaces everything below in
    //     place — same rect, same width, same bottom edge — rather than opening a window, because
    //     *"nothing in Meridian opens a settings window to answer a question."*
    if state.ent_open && state.ent_view == EntView::Network {
        net_paint(state, &mut canvas, atlas, &mut labels, &mut icons_out, theme);
    } else if state.ent_open {
        let v = &state.vitals;
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

        // ── The creature block, step 18 ──────────────────────────────────────────────────────────
        //
        // `.ent-cr { background: var(--sunken) }` in BOTH themes — the design says so explicitly,
        // and it is the same device the terminal and the code pane use to say *this region is
        // content, not chrome*. It sits at the very top of the surface, so its own corners have to
        // be carved to the surface's radius or it draws square shoulders over the rounded panel.
        if l.creature.h > 0 {
            canvas.fill_rect(
                l.creature.x.max(0) as usize, l.creature.y.max(0) as usize,
                l.creature.w as usize, l.creature.h as usize, theme.sunken,
            );
            shapes::carve_round_corners(
                &mut canvas, l.surface.x.max(0) as usize, l.surface.y.max(0) as usize,
                l.surface.w as usize, l.creature.h as usize * 2,
                layout::surface_radius(), |_, _| theme.chrome,
            );
            let rule = l.creature_rule();
            shapes::hairline_h(&mut canvas, rule.x as usize, rule.y.max(0) as usize,
                               rule.w as usize, theme.line);

            if !state.creature.sprite.is_empty() {
                let b = l.creature_box();
                canvas.composite_buffer(
                    b.x.max(0) as usize, b.y.max(0) as usize,
                    &state.creature.sprite, b.w as usize, b.h as usize, 255,
                );
            }

            let (tx, ty) = l.creature_tag();
            labels.push(Label {
                x: tx, y: ty, style: tokens::LB,
                text: String::from("Nyx Entity"), color: theme.fg_4,
            });
            // "● ONLINE". The pip is the accent, and it is the truth: the Entity is a library this
            // process calls, so if this surface is drawing at all the Entity is running.
            let live = "Online";
            let lw = measure(atlas, tokens::LB, live);
            let (pip, text_x, top) = l.creature_live(lw, tokens::LB.line() as i32);
            shapes::fill_round_rect(
                &mut canvas, pip.x.max(0) as usize, pip.y.max(0) as usize,
                pip.w as usize, pip.h as usize, (pip.w / 2).max(1) as usize, theme.accent,
            );
            labels.push(Label {
                x: text_x, y: top, style: tokens::LB,
                text: String::from(live), color: theme.fg_3,
            });
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

        // The identity rows — *"three facts and no statistics."* Seed, stage, traits. No experience
        // bar, no percentage to the next stage, nothing to optimise; the design is explicit that
        // there is nothing here to grind at. Same row treatment as the measurements above them,
        // because both are the machine telling you something. See `layout`'s note for why they are
        // not the design's taller stacked form.
        let identity: [(&str, String); layout::ENT_IDENTITY_ROWS] = [
            ("Entity", state.creature.seed_text()),
            ("Evolution", state.creature.stage_text()),
            ("Traits", state.creature.traits_text()),
        ];
        for (i, (k, val)) in identity.iter().enumerate() {
            let Some(row) = l.identity_row(i) else { continue };
            labels.push(Label {
                x: row.x,
                y: centre_text_y(row.y, row.h, tokens::B3),
                style: tokens::B3,
                text: String::from(*k),
                color: theme.fg_3,
            });
            // The seed is set in MONO. It is a hex identifier, not a word — and it is the one string
            // in the system a person might read aloud or type back in, which is what tabular figures
            // and unambiguous glyphs are for.
            let style = if i == 0 { tokens::MONO } else { tokens::B2 };
            let vw = measure(atlas, style, val);
            labels.push(Label {
                x: row.right() - vw,
                y: centre_text_y(row.y, row.h, style),
                style,
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
        // ⚠️ This slot held the calendar date until step 18. The design gives it to the Entity's age
        // instead — "the footer carries the one number that means anything, how long this Entity has
        // existed" — so the date is now shown nowhere in Meridian. That is the design's call and not
        // an oversight; the clock beside it is still the only clock there is.
        labels.push(Label {
            x: l.footer.x + cw + 10,
            y: l.footer.y + layout::ent_f_pad_top(),
            style: tokens::B3,
            text: state.creature.born_text(),
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
        // Kept as its own binding rather than inlined: every `layout::cmd_power_*` call below takes
        // the same body height, and two of them computing it differently is how a glyph ends up
        // drawn in one place and hit-tested in another.
        let body = layout::command_body_height(&kinds);
        let panel = layout::command_panel(state.screen_w, body);
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
        // ★ Shut down and Restart, as two bare glyphs in the right-hand corner. Icon only: this is
        // the footer, everything else in it is a hint rather than a control, and a labelled button
        // here would read as the most important thing in the Command — which is a list of things to
        // open, not a list of ways to stop.
        //
        // ⚠️ Draw and hit-test BOTH go through `layout::cmd_power_slot`. The one bug this shape is
        // prone to is a target that has drifted off what it looks like, and a glyph you can miss by
        // three pixels is bad here in a way it is not for a dock icon.
        for (i, p) in POWER_ROWS.iter().enumerate() {
            let (Some(slot), Some(g)) = (
                layout::cmd_power_slot(state.screen_w, body, i),
                layout::cmd_power_glyph(state.screen_w, body, i),
            ) else {
                continue;
            };
            let armed = state.power_armed == Some(*p);
            let hot = state.power_hover == Some(i);
            if armed || hot {
                // The armed ground is the accent, not a red: Meridian has no danger colour, and
                // inventing one for two glyphs would be the only red on the whole system.
                let ground = if armed { theme.acc_dim } else { theme.wash };
                shapes::fill_round_rect(
                    &mut canvas,
                    slot.x.max(0) as usize,
                    slot.y.max(0) as usize,
                    slot.w as usize,
                    (slot.h - 8).max(1) as usize,
                    6,
                    ground,
                );
            }
            icons_out.push(IconDraw {
                id: match p {
                    // ⚠️ NOT `Icons::POWER` — that is the Entity's BATTERY gauge, and shipping it
                    // here read on screen as "why is the shutdown icon a battery?".
                    Power::Off => Icons::SHUTDOWN,
                    Power::Restart => Icons::RELOAD,
                },
                px: layout::cmd_power_icon() as usize,
                x: g.x,
                y: g.y,
                color: if armed {
                    theme.accent
                } else if hot {
                    theme.fg_2
                } else {
                    theme.fg_4
                },
            });
        }

        // The count, or — once a power glyph is armed — what that glyph is about to do. Meridian
        // has no dialogs, so this line IS the confirmation prompt: it is the only place the machine
        // can say "shut down" in words before it does it.
        let shown = state.hits.iter().filter(|h| h.kind == CmdEntry::Row).count();
        let (count, tone) = match state.power_armed {
            Some(p) => (String::from(p.armed_label()), theme.accent),
            None => (
                alloc::format!("{} of {} results", shown, state.hits_total),
                theme.fg_4,
            ),
        };
        let cw = measure(atlas, tokens::HINT, &count);
        labels.push(Label {
            // ⚠️ Stops at `cmd_footer_text_right`, not at the panel edge. This text is right-aligned
            // and grows leftward from wherever its right edge is, so anchoring it to the panel would
            // run it straight under the glyphs above.
            x: layout::cmd_footer_text_right(state.screen_w, body) - cw,
            y: centre_text_y(f_y, layout::cmd_footer_h(), tokens::HINT),
            style: tokens::HINT,
            text: count,
            color: tone,
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

    // 8c. Idle, sleep and the lock. Over everything, because that is what they are — the desktop is
    //     still there and still running underneath, it is simply not being shown.
    idle_paint(state, &mut canvas, atlas, sys_get_time());

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
