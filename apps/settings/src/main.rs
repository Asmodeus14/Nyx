//! # Settings — Meridian build-order step 13
//!
//! The design's lede: *"Categories are plain text in a column, with a 2px accent mark on the current
//! one. No icons, no pills, no cards. Rows are label, description and control, separated by hairlines
//! and space."*
//!
//! So the four `Button` widgets in a raised sidebar, the `CheckBox`es, the `TextBox` and the `Menu`
//! are gone, and with them the last use of `nyx_gui::ui`'s widget set in a ported app. What replaces
//! them is `panes`' `.nav` — the *same* component Files uses, which is the argument for that module
//! existing — plus `.sh` headings and `.sr` rows.
//!
//! ## The theme switch had to become real first
//!
//! This window's centrepiece is the Light/Dark control, and the design's claim about the light ramp
//! is that it is "one design with eight token values swapped". That was not true of Nyx: there was
//! **no way at all for an app to learn the shell's theme** — no message, no header field, no syscall
//! — so every ported interior hardcoded `Theme::dark()` and a light-mode shell left every window
//! dark. A Settings app whose headline control did nothing would be a mock.
//!
//! Fixed with two IPC messages and no kernel change: `MSG_SET_THEME` (app → shell, a *request*) and
//! `MSG_THEME_CHANGED` (shell → app, broadcast on change and once at window creation). The shell
//! stays the single source of truth — it owns the wallpaper texture — so this app never sets its own
//! theme; it asks, and then it is told, along with everyone else. Files, Terminal and System Monitor
//! get the same message for free.
//!
//! ## What this window does NOT offer, and why
//!
//! The design's nav lists thirteen destinations. Most have nothing behind them, and a settings screen
//! full of controls that do nothing is worse than a short one — it is a screen of lies about what the
//! machine can do. Checked against the tree, not assumed:
//!
//! - **Sound.** There is no audio driver in Nyx: no HDA controller, no codec enumeration, no mixer,
//!   no stream. The design's own "what this is not" table lists audio as deliberately absent, so a
//!   volume slider would contradict the document it came from.
//! - **Privacy, Security, Developer, Devices, General.** Nothing in the kernel backs any of them.
//! - **Translucency, Window shadow, Reduce motion.** All three are real *tokens* and real drawing
//!   code, but none is a runtime switch — the shell reads them from constants. They are three more
//!   IPC messages each, not settings that exist and are merely unexposed.
//! - **Wallpaper.** `paper::generate` derives the one wallpaper from the theme. There is nothing to
//!   choose between.
//! - **Accent.** Shown, but read-only: it is a token constant, and it is the one value the design
//!   says is "used for focus, selection and measurement only". A picker implies a palette that does
//!   not exist.
//!
//! What is left is genuinely backed: the theme, the panel backlight (`sys_backlight`, the same
//! register the Entity's brightness row drives), and honest read-only reporting of the screen,
//! network, battery, storage and machine.

#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
use nyx_meridian::layout::{self, Rect};
use nyx_meridian::panes::{self, NavKind, RowKind};
use nyx_meridian::shapes;
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B1, B2, B3, D2, LB};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// `.main { padding: 30px 36px 0 }`
const MAIN_PAD_TOP: i32 = 30;
const MAIN_PAD_X: i32 = 36;
/// `.main-h { margin-bottom: 26px }`
const HEAD_GAP_BELOW: i32 = 26;
/// `.main-h .s { margin-top: 5px }`
const SUB_GAP_ABOVE: i32 = 5;
/// The design's Settings nav is 198px, one step narrower than Files' 208.
const NAV_W: i32 = 198;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Cat {
    Appearance,
    Display,
    Network,
    Power,
    Storage,
    About,
}

/// The nav pane, as a flat list so a heading and a destination cannot get out of order.
const NAV: [(&str, Option<Cat>); 8] = [
    ("System", None),
    ("Appearance", Some(Cat::Appearance)),
    ("Display", Some(Cat::Display)),
    ("Network", Some(Cat::Network)),
    ("Power", Some(Cat::Power)),
    ("Machine", None),
    ("Storage", Some(Cat::Storage)),
    ("About", Some(Cat::About)),
];

impl Cat {
    fn title(self) -> &'static str {
        match self {
            Cat::Appearance => "Appearance",
            Cat::Display => "Display",
            Cat::Network => "Network",
            Cat::Power => "Power",
            Cat::Storage => "Storage",
            Cat::About => "About",
        }
    }
    fn subtitle(self) -> &'static str {
        match self {
            Cat::Appearance => "How Nyx renders surfaces, type and colour.",
            Cat::Display => "The panel, and what is driving it.",
            Cat::Network => "The wireless link and its addresses.",
            Cat::Power => "What the battery reports.",
            Cat::Storage => "The filesystem this system runs from.",
            Cat::About => "What this machine is.",
        }
    }
}

/// What a row's right-hand side is. The variant IS the control, so drawing and hit-testing cannot
/// disagree about which one a row has.
enum Control {
    /// A read-only value. The honest default for anything Nyx can measure but not change.
    Text(String),
    /// The theme control. `on` is the index of the selected segment.
    Segments(&'static [&'static str], usize),
    /// A draggable 0-100 track — brightness.
    Track(i32),
    /// A read-only value with a colour swatch after it.
    Swatch(String, u32),
}

struct Row {
    label: &'static str,
    desc: &'static str,
    control: Control,
}

const THEME_SEGS: [&str; 2] = ["Light", "Dark"];

struct Settings {
    width: i32,
    height: i32,
    cat: Cat,
    dark: bool,
    /// None until the panel answers. `sys_backlight` is not guaranteed to work on every machine, and
    /// a slider pinned at 0 would look like a panel turned off rather than one we cannot read.
    brightness: Option<i32>,
    dragging_brightness: bool,

    screen: (usize, usize, usize),
    wifi: Option<WifiStatus>,
    battery: Battery,
    disk: Option<StatFs>,
    info: SystemInfo,
    metrics: Option<SysMetrics>,
    last_poll: usize,

    nav_hover: Option<usize>,
    row_hover: Option<usize>,
    scroll: i32,
}

impl Settings {
    fn new() -> Self {
        Self {
            width: 896,
            height: 540,
            cat: Cat::Appearance,
            dark: true,
            brightness: None,
            dragging_brightness: false,
            screen: (0, 0, 0),
            wifi: None,
            battery: Battery::default(),
            disk: None,
            info: unsafe { core::mem::zeroed() },
            metrics: None,
            last_poll: 0,
            nav_hover: None,
            row_hover: None,
            scroll: 0,
        }
    }

    fn theme(&self) -> Theme {
        if self.dark { Theme::dark() } else { Theme::light() }
    }

    fn nav_kinds(&self) -> Vec<NavKind> {
        NAV.iter()
            .map(|(_, c)| if c.is_some() { NavKind::Item } else { NavKind::Group })
            .collect()
    }

    fn poll(&mut self) {
        self.screen = sys_get_screen_info();
        self.wifi = sys_wifi_status();
        self.battery = sys_battery();
        self.disk = sys_statfs();
        self.metrics = sys_get_metrics();
        sys_get_system_info(&mut self.info);
        if self.brightness.is_none() {
            self.brightness = sys_backlight(0).map(|p| p as i32);
        }
    }

    /// The rows of the current category. Built fresh each time rather than cached: they are a
    /// handful of strings, and a cache is one more thing that can disagree with the machine.
    fn rows(&self) -> Vec<Row> {
        let mut v: Vec<Row> = Vec::new();
        match self.cat {
            Cat::Appearance => {
                v.push(Row {
                    label: "Mode",
                    desc: "Applies to the shell and every Nyx application",
                    control: Control::Segments(&THEME_SEGS, if self.dark { 1 } else { 0 }),
                });
                v.push(Row {
                    label: "Accent",
                    desc: "Used for focus, selection and measurement only",
                    control: Control::Swatch(
                        alloc::format!("#{:06X}", self.theme().accent & 0x00FF_FFFF),
                        self.theme().accent,
                    ),
                });
            }
            Cat::Display => {
                v.push(Row {
                    label: "Brightness",
                    desc: "Drives the panel's PWM directly",
                    control: match self.brightness {
                        Some(p) => Control::Track(p),
                        None => Control::Text(String::from("not readable")),
                    },
                });
                let (w, h, _) = self.screen;
                v.push(Row {
                    label: "Resolution",
                    desc: "Set by the firmware at boot; Nyx does not mode-set",
                    control: Control::Text(if w == 0 {
                        String::from("unknown")
                    } else {
                        alloc::format!("{} \u{00D7} {}", w, h)
                    }),
                });
            }
            Cat::Network => {
                let (status, addr) = match &self.wifi {
                    Some(s) if s.state >= 2 => {
                        let n = (s.ssid_len as usize).min(s.ssid.len());
                        let ssid = core::str::from_utf8(&s.ssid[..n]).unwrap_or("");
                        (
                            if ssid.is_empty() { String::from("Connected") } else { String::from(ssid) },
                            alloc::format!("{}.{}.{}.{}", s.ip[0], s.ip[1], s.ip[2], s.ip[3]),
                        )
                    }
                    Some(_) => (String::from("Not connected"), String::from("\u{2014}")),
                    None => (String::from("No radio"), String::from("\u{2014}")),
                };
                v.push(Row { label: "Wi-Fi", desc: "The network this machine is joined to", control: Control::Text(status) });
                v.push(Row { label: "Address", desc: "Leased over DHCP", control: Control::Text(addr) });
            }
            Cat::Power => {
                // Three distinct answers, and folding them together is the mistake the battery code
                // already documents: "nobody has asked yet", "there is no battery" and a reading.
                let charge = if !self.battery.sampled {
                    String::from("not sampled")
                } else if !self.battery.present {
                    String::from("no battery")
                } else if self.battery.design_cap > 0 {
                    alloc::format!("{}%", (self.battery.remaining_cap.max(0) as i64 * 100
                        / self.battery.design_cap.max(1) as i64).min(100))
                } else {
                    String::from("unknown")
                };
                v.push(Row { label: "Charge", desc: "Read from the embedded controller", control: Control::Text(charge) });
                v.push(Row {
                    label: "Controller",
                    desc: "ACPI battery methods need the EC handler registered",
                    control: Control::Text(String::from(if self.battery.ec_ok { "ready" } else { "not registered" })),
                });
            }
            Cat::Storage => {
                let (used, cap) = match &self.disk {
                    Some(d) if d.total_bytes > 0 => (
                        human(d.total_bytes.saturating_sub(d.free_bytes)),
                        human(d.total_bytes),
                    ),
                    _ => (String::from("\u{2014}"), String::from("\u{2014}")),
                };
                v.push(Row { label: "nvme0", desc: "The ext4 volume mounted at /mnt/nvme", control: Control::Text(cap) });
                v.push(Row { label: "Used", desc: "Everything the installer and apps have written", control: Control::Text(used) });
            }
            Cat::About => {
                let machine = self.info.machine_name();
                v.push(Row {
                    label: "Machine",
                    desc: "Reported by SMBIOS, or by the ACPI OEM id",
                    control: Control::Text(if machine.is_empty() {
                        String::from("not reported by firmware")
                    } else {
                        String::from(machine)
                    }),
                });
                v.push(Row { label: "Architecture", desc: "", control: Control::Text(String::from("x86_64")) });
                v.push(Row {
                    label: "Memory",
                    desc: "Physical RAM the kernel can allocate",
                    control: Control::Text(match self.metrics {
                        Some(m) if m.mem_total_bytes > 0 => human(m.mem_total_bytes),
                        _ => String::from("\u{2014}"),
                    }),
                });
                v.push(Row { label: "Window server", desc: "", control: Control::Text(String::from("Meridian")) });
            }
        }
        v
    }

    /// The rows as a `panes` kind list, with a heading in front. One heading per category here —
    /// the design's Appearance pane has three, but a two-row section under its own heading reads as
    /// filler when the section is the whole pane.
    fn row_kinds(&self, rows: &[Row]) -> Vec<RowKind> {
        let mut k = alloc::vec![RowKind::Heading];
        for r in rows {
            k.push(if r.desc.is_empty() { RowKind::Row } else { RowKind::RowWithDesc });
        }
        k
    }

    fn content_x(&self) -> i32 {
        sc(NAV_W) + sc(MAIN_PAD_X)
    }

    fn content_w(&self) -> i32 {
        (self.width - self.content_x() - sc(MAIN_PAD_X) - layout::scrollbar_reserve()).max(0)
    }

    /// Where the settings column starts: under the `.main-h` heading block.
    fn rows_top(&self) -> i32 {
        sc(MAIN_PAD_TOP)
            + text::line_box(D2)
            + sc(SUB_GAP_ABOVE)
            + text::line_box(B3)
            + sc(HEAD_GAP_BELOW)
    }

    fn content_h(&self) -> i32 {
        let rows = self.rows();
        self.rows_top() + panes::settings_height(&self.row_kinds(&rows))
    }

    fn max_scroll(&self) -> i32 {
        (self.content_h() - self.height).max(0)
    }

    /// The `panes` row rect for row `i` (0-based over `rows`, so +1 past the heading), in surface
    /// coordinates with the scroll already applied.
    fn row_rect(&self, kinds: &[RowKind], i: usize) -> Option<panes::SettingRow> {
        let mut r = panes::setting_row(kinds, self.content_w(), self.content_x(), i)?;
        let dy = self.rows_top() - self.scroll;
        r.rect.y += dy;
        r.label.y += dy;
        r.desc.y += dy;
        Some(r)
    }

    // ── drawing ──────────────────────────────────────────────────────────────────────────────────

    fn draw_nav(&self, canvas: &mut Canvas, t: &Theme) {
        let kinds = self.nav_kinds();
        for (i, (label, cat)) in NAV.iter().enumerate() {
            let Some(e) = panes::nav_entry(&kinds, i) else { continue };
            match cat {
                None => {
                    text::draw(canvas, e.x, e.y, LB, t.fg_4, label);
                }
                Some(c) => {
                    let on = *c == self.cat;
                    if on || self.nav_hover == Some(i) {
                        let wash = if on { t.wash_2 } else { t.wash };
                        fill(canvas, 0, e.y, sc(NAV_W), e.h, wash);
                    }
                    if on {
                        // "a 2px accent mark on the current one"
                        let b = panes::nav_bar(e);
                        fill(canvas, b.x, b.y, b.w, b.h, t.accent);
                    }
                    let col = if on { t.fg } else { t.fg_2 };
                    let y = text::centre_y(e.y, e.h, B1);
                    text::draw(canvas, panes::nav_label_x(e), y, B1, col, label);
                }
            }
        }
    }

    fn draw_head(&self, canvas: &mut Canvas, t: &Theme) {
        let x = self.content_x();
        let y = sc(MAIN_PAD_TOP) - self.scroll;
        text::draw(canvas, x, y, D2, t.fg, self.cat.title());
        let sy = y + text::line_box(D2) + sc(SUB_GAP_ABOVE);
        text::draw(canvas, x, sy, B3, t.fg_3, self.cat.subtitle());
    }

    fn draw_rows(&self, canvas: &mut Canvas, t: &Theme, rows: &[Row]) {
        let kinds = self.row_kinds(rows);

        // The section heading is entry 0 and names the group, not the category.
        if let Some(h) = self.row_rect(&kinds, 0) {
            let name = match self.cat {
                Cat::Appearance => "Theme",
                Cat::Display => "Panel",
                Cat::Network => "Wireless",
                Cat::Power => "Battery",
                Cat::Storage => "Volume",
                Cat::About => "System",
            };
            text::draw(canvas, h.label.x, h.label.y, LB, t.fg_4, name);
        }

        for (i, row) in rows.iter().enumerate() {
            let Some(r) = self.row_rect(&kinds, i + 1) else { continue };
            if r.rect.bottom() < 0 || r.rect.y > self.height {
                continue;
            }
            if self.row_hover == Some(i) {
                fill(canvas, r.rect.x, r.rect.y, r.rect.w, r.rect.h, t.wash);
            }
            text::draw(canvas, r.label.x, r.label.y, B1, t.fg, row.label);
            if !row.desc.is_empty() {
                text::draw(canvas, r.desc.x, r.desc.y, B3, t.fg_3, row.desc);
            }
            if r.rule {
                fill(canvas, r.rect.x, r.rect.bottom() - 1, r.rect.w, 1, t.line);
            }
            self.draw_control(canvas, t, &r, &row.control);
        }
    }

    fn draw_control(&self, canvas: &mut Canvas, t: &Theme, r: &panes::SettingRow, c: &Control) {
        match c {
            Control::Text(s) => {
                let y = text::centre_y(r.rect.y, r.rect.h, B2);
                text::draw_right(canvas, r.value_right, y, B2, t.fg_2, s);
            }
            Control::Swatch(s, colour) => {
                let box_px = sc(18);
                let bx = r.value_right - box_px;
                let by = r.rect.y + (r.rect.h - box_px) / 2;
                if by >= 0 && by + box_px <= self.height {
                    shapes::fill_round_rect(
                        canvas, bx as usize, by as usize,
                        box_px as usize, box_px as usize, sc(4) as usize, *colour,
                    );
                }
                let y = text::centre_y(r.rect.y, r.rect.h, B3);
                text::draw_right(canvas, bx - sc(12), y, B3, t.fg_3, s);
            }
            Control::Segments(labels, on) => {
                let widths: Vec<i32> = labels.iter().map(|s| text::width(B2, s)).collect();
                for (i, label) in labels.iter().enumerate() {
                    let Some(s) = panes::segment(r.rect, r.value_right, &widths, i) else { continue };
                    if s.y < 0 || s.bottom() > self.height {
                        continue;
                    }
                    let selected = i == *on;
                    if selected {
                        shapes::fill_round_rect(
                            canvas, s.x as usize, s.y as usize, s.w as usize, s.h as usize,
                            panes::segment_radius(), t.wash_2,
                        );
                    }
                    let col = if selected { t.fg } else { t.fg_3 };
                    let ty = text::centre_y(s.y, s.h, B2);
                    text::draw(canvas, s.x + (s.w - widths[i]) / 2, ty, B2, col, label);
                }
            }
            Control::Track(pct) => {
                // The number first, so the track is right-aligned against a fixed column rather
                // than shifting as the value goes from 9% to 100%.
                let ty = text::centre_y(r.rect.y, r.rect.h, B3);
                let num_right = r.value_right;
                text::draw_right(canvas, num_right, ty, B3, t.fg_3, &alloc::format!("{}%", pct));
                let trk = panes::track(r.rect, num_right - sc(46));
                fill(canvas, trk.x, trk.y, trk.w, trk.h, t.wash_2);
                let f = panes::track_fill(trk, *pct);
                fill(canvas, f.x, f.y, f.w, f.h, t.accent);
            }
        }
    }

    /// The brightness track's rect, if this category has one. One function for the draw and the
    /// drag, so the bar cannot be grabbable anywhere other than where it is drawn.
    fn brightness_track(&self) -> Option<Rect> {
        if self.cat != Cat::Display || self.brightness.is_none() {
            return None;
        }
        let rows = self.rows();
        let kinds = self.row_kinds(&rows);
        let r = self.row_rect(&kinds, 1)?;
        Some(panes::track(r.rect, r.value_right - sc(46)))
    }

    fn set_brightness(&mut self, pct: i32) -> bool {
        let pct = pct.clamp(0, 100);
        if self.brightness == Some(pct) {
            return false;
        }
        sys_backlight_set(pct as u32);
        self.brightness = Some(pct);
        true
    }
}

fn sc(v: i32) -> i32 {
    nyx_meridian::scale::px(v)
}

/// `Canvas::fill_rect` takes usize and has no notion of a negative origin, so every fill goes
/// through here — a scrolled row has a negative `y` on the way out of view, and passing that in as
/// `usize` wraps it to an enormous coordinate.
fn fill(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(cw), (y + h).min(ch));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    canvas.fill_rect(x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, color);
}

fn human(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if b < KB {
        alloc::format!("{} B", b)
    } else if b < MB {
        alloc::format!("{}.{} KB", b / KB, (b % KB) * 10 / KB)
    } else if b < GB {
        alloc::format!("{}.{} MB", b / MB, (b % MB) * 10 / MB)
    } else {
        alloc::format!("{}.{} GB", b / GB, (b % GB) * 10 / GB)
    }
}

impl NyxApp for Settings {
    fn icon_path(&self) -> &str {
        "/mnt/nvme/apps/Settings.nyx/icon.png"
    }
    fn title(&self) -> &str {
        "Settings"
    }
    /// The design's own window, and 896 is on the 16px quantisation every width in the system uses.
    fn initial_width(&self) -> usize {
        896
    }
    fn initial_height(&self) -> usize {
        540
    }

    fn init(&mut self) {
        self.poll();
    }

    fn update(&mut self) -> bool {
        let now = sys_get_time();
        // Slow: everything here changes on human timescales, and only two categories show anything
        // that moves at all.
        if now.wrapping_sub(self.last_poll) < 2000 {
            return false;
        }
        self.last_poll = now;
        let before = (self.wifi.map(|w| w.state), self.battery.remaining_cap);
        self.poll();
        (self.wifi.map(|w| w.state), self.battery.remaining_cap) != before
    }

    /// The shell told us the theme. This is the app that asked for it, but it still learns the
    /// result the same way every other window does.
    fn on_theme(&mut self, dark: bool) -> bool {
        if self.dark == dark {
            return false;
        }
        self.dark = dark;
        true
    }

    fn content_height(&self) -> usize {
        let h = self.content_h();
        if h > self.height { h as usize } else { 0 }
    }
    fn scroll_offset(&self) -> usize {
        self.scroll.max(0) as usize
    }
    fn on_scroll(&mut self, off: usize) -> bool {
        let new = (off as i32).clamp(0, self.max_scroll());
        if new != self.scroll {
            self.scroll = new;
            return true;
        }
        false
    }

    fn on_mouse_move(&mut self, mx: usize, my: usize) -> bool {
        let (x, y) = (mx as i32, my as i32);

        // A drag in progress owns the pointer; nothing else should light up under it.
        if self.dragging_brightness {
            if let Some(t) = self.brightness_track() {
                return self.set_brightness(panes::track_pct(t, x));
            }
        }

        let nav = if x < sc(NAV_W) {
            panes::nav_hit(&self.nav_kinds(), x, y + self.scroll)
        } else {
            None
        };
        let rows = self.rows();
        let kinds = self.row_kinds(&rows);
        let row = if x >= self.content_x() {
            panes::setting_hit(&kinds, self.content_w(), self.content_x(),
                x, y - self.rows_top() + self.scroll)
                .map(|i| i - 1) // entry 0 is the heading
        } else {
            None
        };

        if nav != self.nav_hover || row != self.row_hover {
            self.nav_hover = nav;
            self.row_hover = row;
            return true;
        }
        false
    }

    fn on_mouse_leave(&mut self) -> bool {
        self.dragging_brightness = false;
        if self.nav_hover.is_some() || self.row_hover.is_some() {
            self.nav_hover = None;
            self.row_hover = None;
            return true;
        }
        false
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        let (x, y) = (mx as i32, my as i32);

        if x < sc(NAV_W) {
            if let Some(i) = panes::nav_hit(&self.nav_kinds(), x, y + self.scroll) {
                if let Some(c) = NAV[i].1 {
                    if c != self.cat {
                        self.cat = c;
                        self.scroll = 0;
                        self.row_hover = None;
                        return true;
                    }
                }
            }
            return false;
        }

        // The brightness track, before the row hit-test: the track lives inside a row, and the row
        // has no click action of its own, so testing the control first costs nothing and keeps a
        // drag from being swallowed.
        if let Some(t) = self.brightness_track() {
            if panes::track_hit(t, x, y) {
                self.dragging_brightness = true;
                return self.set_brightness(panes::track_pct(t, x));
            }
        }

        // The theme segments.
        if self.cat == Cat::Appearance {
            let rows = self.rows();
            let kinds = self.row_kinds(&rows);
            if let Some(r) = self.row_rect(&kinds, 1) {
                let widths: Vec<i32> = THEME_SEGS.iter().map(|s| text::width(B2, s)).collect();
                if let Some(i) = panes::segment_hit(r.rect, r.value_right, &widths, x, y) {
                    // Ask; do not set. The shell owns the theme and answers with a broadcast that
                    // reaches this app through `on_theme` like any other window.
                    sys_ipc_send(
                        4,
                        MSG_SET_THEME,
                        if i == 1 { THEME_DARK } else { THEME_LIGHT },
                        0,
                    );
                    return false;
                }
            }
        }
        false
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        self.width = canvas.width as i32;
        self.height = canvas.height as i32;
        self.scroll = self.scroll.clamp(0, self.max_scroll());
        let t = self.theme();

        canvas.fill_rect(0, 0, self.width as usize, self.height as usize, t.surface);
        self.draw_nav(canvas, &t);

        let rows = self.rows();
        // ★ Same painter's order as Files: the rows scroll under the heading, and nothing in
        // `Canvas` clips, so the heading is repainted on a fresh plate after the rows.
        self.draw_rows(canvas, &t, &rows);
        let mask_h = self.rows_top() - sc(HEAD_GAP_BELOW) / 2;
        fill(canvas, sc(NAV_W), 0, self.width - sc(NAV_W), mask_h, t.surface);
        self.draw_head(canvas, &t);
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // ★ 2026-09-08: the typefaces are no longer `include_bytes!`d into this binary — they are
    // read out of the shared bundle at /mnt/nvme/fonts/ (see `nyx_meridian::font`). That took
    // 1.24 MB off the boot image and put it on THIS HEAP instead, so the heap had to grow to
    // hold it. A heap too small to take the fonts is not a crash: the read fails, every face
    // falls back to DejaVu, and the window renders in the wrong typeface at the right size.
    let heap_start = sys_alloc_pages(768);
    if heap_start == 0 {
        sys_exit(1);
    }
    unsafe {
        ALLOCATOR.lock().init(heap_start as *mut u8, 768 * 4096);
    }

    // Three faces: `d2` headings, `b1`/`b2`/`b3` rows, `lb` labels. No `d1`, no mono.
    nyx_meridian::font::register_text();

    nyx_gui::app::run(Settings::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(111);
}
