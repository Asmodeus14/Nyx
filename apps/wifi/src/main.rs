//! Wi-Fi — the desktop's network picker.
//!
//! The kernel driver owns the radio and has no built-in SSID; this app is how a network gets chosen.
//! It lists what the last scan saw, takes a passphrase, asks the kernel to join, and (by default)
//! remembers the network so the boot agent can reconnect on its own next time.
//!
//! The one structural wrinkle: `sys_wifi_scan`, `sys_wifi_connect` and `sys_wifi_disconnect` BLOCK
//! inside the kernel — a passive sweep dwells on every channel, a join runs association, the WPA2
//! 4-way handshake and a DHCP exchange. Nothing repaints while they run, so all three are deferred
//! by a frame (see `Pending`); otherwise the window would freeze on a stale frame with no
//! explanation of what it was doing.

#![no_std]
#![no_main]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

const ROW_H: usize = 38;
const LIST_TOP: usize = 116;
const PANEL_H: usize = 132;
const MAX_NETS: usize = 48;

/// x, y, w, h.
type Rect = (usize, usize, usize, usize);

fn hit(r: Rect, mx: usize, my: usize) -> bool {
    mx >= r.0 && mx < r.0 + r.2 && my >= r.1 && my < r.1 + r.3
}

/// A blocking kernel call the user asked for, armed with a one-frame delay so the "Scanning…" /
/// "Connecting…" frame is painted and flushed BEFORE we disappear into the kernel. The event loop
/// runs `update()` before `draw()`, so firing on the click itself would freeze the window on the
/// previous frame for the whole operation.
#[derive(PartialEq, Clone, Copy)]
enum Pending {
    None,
    Scan,
    Connect,
    Disconnect,
    RadioOn,
    RadioOff,
}

struct WifiApp {
    nets: Vec<WifiNetwork>,
    sel: Option<usize>,
    psk: String,
    status: WifiStatus,
    message: String,
    pending: Pending,
    arm: u8,
    scroll: usize,
    busy: bool,
    /// Whether a successful join is written to the saved-network file (Windows' "connect
    /// automatically"). On by default — that's what makes the machine come back online by itself.
    remember: bool,
    /// SSID currently in the saved-network file, so the UI can name what Forget would drop.
    saved: Option<String>,
    tick: u32,
    /// Window size from the last draw, so `on_mouse` hit-tests against what is actually on screen.
    w: usize,
    h: usize,
}

impl WifiApp {
    fn new() -> Self {
        Self {
            nets: Vec::new(),
            sel: None,
            psk: String::new(),
            status: WifiStatus::default(),
            message: String::from("Select a network to connect."),
            pending: Pending::None,
            arm: 0,
            scroll: 0,
            busy: false,
            remember: true,
            saved: None,
            tick: 0,
            w: 560,
            h: 520,
        }
    }

    /// Pull the current scan list out of the kernel. Cheap — this reads the stored results of the
    /// last sweep, it does not re-scan, so it's safe to call on launch for an instant list.
    fn refresh_list(&mut self) {
        let mut buf = [WifiNetwork::default(); MAX_NETS];
        let n = sys_wifi_list(&mut buf);
        self.nets.clear();
        for e in buf.iter().take(n) {
            self.nets.push(*e);
        }
        // Alphabetical: scan order is arrival order, which reshuffles on every sweep and makes the
        // list feel unstable to click.
        self.nets.sort_by(|a, b| a.name().cmp(b.name()));
        if let Some(i) = self.sel {
            if i >= self.nets.len() {
                self.sel = None;
            }
        }
    }

    fn refresh_status(&mut self) {
        if let Some(s) = sys_wifi_status() {
            self.status = s;
        }
    }

    fn refresh_saved(&mut self) {
        let mut buf = [0u8; 160];
        self.saved = match wifi_load_network(&mut buf) {
            Some((slen, _)) => core::str::from_utf8(&buf[..slen]).ok().map(String::from),
            None => None,
        };
    }

    fn connected(&self) -> bool {
        self.status.state == WIFI_CONNECTED
    }

    /// Whether the radio is switched on. Read from the published status rather than kept locally, so
    /// the switch shows what the kernel actually did — including "the request was refused because
    /// something else had the radio", which a locally-mirrored bool would silently lie about.
    fn radio_on(&self) -> bool {
        self.status.state != WIFI_RADIO_OFF
    }

    // ── Button geometry. Defined once so draw() and on_mouse() can never drift apart. ──
    /// The Wi-Fi on/off switch, next to the title.
    fn sw_radio(&self) -> Rect {
        (108, 18, 44, 22)
    }
    fn btn_rescan(&self) -> Rect {
        (self.w.saturating_sub(100), 14, 80, 26)
    }
    fn btn_disconnect(&self) -> Option<Rect> {
        if self.connected() {
            Some((self.w.saturating_sub(196), 14, 88, 26))
        } else {
            None
        }
    }
    fn btn_forget(&self) -> Option<Rect> {
        self.saved
            .as_ref()
            .map(|_| (self.w.saturating_sub(100), 78, 80, 24))
    }

    fn list_h(&self) -> usize {
        let panel = if self.sel.is_some() { PANEL_H } else { 0 };
        self.h.saturating_sub(LIST_TOP + panel + 8)
    }
    fn panel_top(&self) -> usize {
        LIST_TOP + self.list_h() + 8
    }
    fn btn_cancel(&self) -> Rect {
        (self.w.saturating_sub(150), self.panel_top() + 42, 60, 30)
    }
    fn btn_join(&self) -> Rect {
        (self.w.saturating_sub(84), self.panel_top() + 42, 64, 30)
    }
    fn box_remember(&self) -> Rect {
        (20, self.panel_top() + 88, 16, 16)
    }

    fn state_text(&self) -> (&'static str, u32) {
        match self.status.state {
            WIFI_RADIO_OFF => ("Wi-Fi is off", Color::TEXT_MUTED),
            WIFI_CONNECTED => ("Connected", Color::ACCENT_GREEN),
            WIFI_CONNECTING => ("Connecting", Color::ACCENT_PRIMARY),
            WIFI_AUTH_FAILED => ("Authentication failed", 0xFF_C0392B),
            WIFI_NO_LEASE => ("No IP address", 0xFF_C0392B),
            WIFI_HW_FAILED => ("Adapter error", 0xFF_C0392B),
            _ => ("Not connected", Color::TEXT_MUTED),
        }
    }

    /// A small padlock, drawn from rectangles — the UI font has no symbol for one.
    fn draw_lock(canvas: &mut Canvas, x: usize, y: usize, color: u32) {
        canvas.fill_rect(x, y + 5, 9, 7, color); // body
        canvas.fill_rect(x + 2, y + 1, 2, 4, color); // shackle, left
        canvas.fill_rect(x + 5, y + 1, 2, 4, color); // shackle, right
        canvas.fill_rect(x + 2, y, 5, 2, color); // shackle, top
    }

    /// A sliding on/off switch: a coloured track with the knob parked at one end.
    ///
    /// Drawn as a switch rather than as another "Turn off" button on purpose — a button states the
    /// action and leaves the current state to be inferred, and for a radio the state is the thing you
    /// came to check. The knob's position reads at a glance from across the room; button text doesn't.
    fn draw_switch(canvas: &mut Canvas, r: Rect, on: bool, enabled: bool) {
        let track = if !enabled {
            Color::WARM_BORDER
        } else if on {
            Color::ACCENT_PRIMARY
        } else {
            0xFF_B9B4AC
        };
        canvas.fill_rect(r.0, r.1, r.2, r.3, track);
        // Knock the four corner pixels back out to the page colour: with only fill_rect to hand,
        // that's what turns a hard rectangle into something that reads as a rounded pill.
        for (cx, cy) in [
            (r.0, r.1),
            (r.0 + r.2 - 1, r.1),
            (r.0, r.1 + r.3 - 1),
            (r.0 + r.2 - 1, r.1 + r.3 - 1),
        ] {
            canvas.fill_rect(cx, cy, 1, 1, Color::WARM_BG);
        }

        let k = r.3 - 6; // knob side, 3px of track showing all round
        let kx = if on { r.0 + r.2 - k - 3 } else { r.0 + 3 };
        canvas.fill_rect(kx, r.1 + 3, k, k, Color::WHITE);
    }

    fn draw_button(canvas: &mut Canvas, r: Rect, label: &str, enabled: bool, primary: bool) {
        let bg = if !enabled {
            Color::WARM_BORDER
        } else if primary {
            Color::ACCENT_PRIMARY
        } else {
            Color::WARM_SURFACE
        };
        canvas.fill_rect(r.0, r.1, r.2, r.3, bg);
        if enabled && !primary {
            canvas.fill_rect(r.0, r.1, r.2, 1, Color::WARM_BORDER);
            canvas.fill_rect(r.0, r.1 + r.3 - 1, r.2, 1, Color::WARM_BORDER);
        }
        let fg = if !enabled {
            Color::TEXT_MUTED
        } else if primary {
            Color::WHITE
        } else {
            Color::TEXT_DARK
        };
        let tw = Canvas::text_width(label, 1).min(r.2);
        canvas.print_str(r.0 + (r.2 - tw) / 2, r.1 + (r.3 / 2) - 8, label, fg, 1);
    }

    fn queue(&mut self, job: Pending, msg: String) {
        self.message = msg;
        self.pending = job;
        self.arm = 1;
        self.busy = true;
    }

    fn selected_name(&self) -> String {
        self.sel
            .and_then(|i| self.nets.get(i))
            .map(|n| String::from(n.name()))
            .unwrap_or_default()
    }
}

impl NyxApp for WifiApp {
    fn icon_path(&self) -> &str { "/mnt/nvme/apps/Wifi.nyx/icon.png" }
    fn title(&self) -> &str {
        "Wi-Fi"
    }
    fn initial_width(&self) -> usize {
        560
    }
    fn initial_height(&self) -> usize {
        520
    }

    fn init(&mut self) {
        self.refresh_status();
        self.refresh_list();
        self.refresh_saved();
        if self.nets.is_empty() {
            self.message = String::from("No networks found yet — press Rescan.");
        }
    }

    fn update(&mut self) -> bool {
        // Fire a deferred blocking call, but only once the frame that announced it has been painted.
        if self.pending != Pending::None {
            if self.arm > 0 {
                self.arm -= 1;
                return false;
            }
            let job = self.pending;
            self.pending = Pending::None;
            self.busy = false;

            match job {
                Pending::Scan => {
                    let n = sys_wifi_scan();
                    self.refresh_list();
                    self.scroll = 0;
                    self.message = if n == 0 {
                        String::from("Scan finished — no networks in range.")
                    } else {
                        alloc::format!("Scan finished — {} network(s) found.", self.nets.len())
                    };
                }
                Pending::Connect => {
                    let (ssid, secure, supported) = match self.sel.and_then(|i| self.nets.get(i)) {
                        Some(n) => (
                            String::from(n.name()),
                            n.is_secure(),
                            !n.is_unsupported_security(),
                        ),
                        None => (String::new(), false, false),
                    };
                    if ssid.is_empty() {
                        self.message = String::from("No network selected.");
                    } else if !supported {
                        self.message =
                            String::from("This network uses WEP or WPA1, which isn't supported.");
                    } else {
                        let rc = sys_wifi_connect(&ssid, &self.psk);
                        self.message = match rc {
                            WIFI_CONNECTED => {
                                // Save only now: a passphrase that completed the 4-way handshake is
                                // known-good, so the boot agent can never be handed a wrong one.
                                let saved = self.remember && wifi_save_network(&ssid, &self.psk);
                                self.psk.clear();
                                self.sel = None;
                                if saved {
                                    alloc::format!(
                                        "Connected to \"{}\" — will reconnect automatically.",
                                        ssid
                                    )
                                } else {
                                    alloc::format!("Connected to \"{}\".", ssid)
                                }
                            }
                            WIFI_AUTH_FAILED if secure => {
                                String::from("Couldn't connect — check the password and try again.")
                            }
                            WIFI_AUTH_FAILED => {
                                String::from("Couldn't associate with this network.")
                            }
                            WIFI_NO_LEASE => String::from(
                                "Joined the network, but the router never assigned an IP address.",
                            ),
                            WIFI_HW_FAILED => {
                                String::from("The adapter refused to tune to this network.")
                            }
                            _ => String::from("Connection attempt finished with no result."),
                        };
                    }
                    self.refresh_list();
                    self.refresh_saved();
                }
                Pending::Disconnect => {
                    sys_wifi_disconnect();
                    self.sel = None;
                    self.psk.clear();
                    self.message = String::from("Disconnected. Pick a network, or rescan.");
                    self.refresh_list();
                }
                Pending::RadioOn | Pending::RadioOff => {
                    let want_on = job == Pending::RadioOn;
                    // The kernel refuses while another Wi-Fi operation owns the radio and hands back
                    // the unchanged state, so believe the return value, not the request.
                    let got_on = sys_wifi_set_radio(want_on) != WIFI_RADIO_OFF;
                    self.sel = None;
                    self.psk.clear();
                    self.scroll = 0;
                    self.message = if got_on != want_on {
                        String::from("The radio is busy right now — try the switch again.")
                    } else if want_on {
                        String::from("Wi-Fi is on. Press Rescan to look for networks.")
                    } else {
                        String::from("Wi-Fi is off.")
                    };
                    self.refresh_list();
                }
                Pending::None => {}
            }
            self.refresh_status();
            return true;
        }

        // Idle: re-read the link state about twice a second, repaint only when it moved.
        self.tick = self.tick.wrapping_add(1);
        if self.tick % 30 == 0 {
            let before = (self.status.state, self.status.ip);
            self.refresh_status();
            if before != (self.status.state, self.status.ip) {
                return true;
            }
        }
        false
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let w = canvas.width;
        let h = canvas.height;
        self.w = w;
        self.h = h;

        canvas.fill_rect(0, 0, w, h, Color::WARM_BG);

        let radio_on = self.radio_on();

        // ── Header ──
        canvas.print_str(20, 16, "Wi-Fi", Color::TEXT_DARK, 2);

        let sw = self.sw_radio();
        Self::draw_switch(canvas, sw, radio_on, !self.busy);
        canvas.print_str(
            sw.0 + sw.2 + 10,
            sw.1 + 3,
            if radio_on { "On" } else { "Off" },
            if radio_on { Color::TEXT_DARK } else { Color::TEXT_MUTED },
            1,
        );

        let (st_text, st_color) = self.state_text();
        canvas.fill_rect(20, 54, 8, 8, st_color);
        if self.connected() && self.status.ssid_len > 0 {
            let line = alloc::format!(
                "{} to \"{}\"  —  {}.{}.{}.{}",
                st_text,
                self.status.name(),
                self.status.ip[0],
                self.status.ip[1],
                self.status.ip[2],
                self.status.ip[3]
            );
            canvas.print_str(36, 50, &line, Color::TEXT_DARK, 1);
        } else {
            canvas.print_str(36, 50, st_text, Color::TEXT_MUTED, 1);
        }

        // Rescan is refused by the kernel while associated (it retunes the radio off the AP channel
        // for seconds), so grey it out rather than letting the click fail.
        Self::draw_button(
            canvas,
            self.btn_rescan(),
            "Rescan",
            radio_on && !self.connected() && !self.busy,
            true,
        );
        if let Some(r) = self.btn_disconnect() {
            Self::draw_button(canvas, r, "Disconnect", !self.busy, false);
        }

        // ── Saved-network row ──
        match &self.saved {
            Some(name) => {
                let line = alloc::format!("Reconnects automatically to \"{}\"", name);
                canvas.print_str(20, 82, &line, Color::TEXT_MUTED, 1);
            }
            None => {
                canvas.print_str(20, 82, "No network is remembered yet.", Color::TEXT_MUTED, 1);
            }
        }
        if let Some(r) = self.btn_forget() {
            Self::draw_button(canvas, r, "Forget", !self.busy, false);
        }

        canvas.fill_rect(20, 106, w.saturating_sub(40), 1, Color::WARM_BORDER);

        // ── Network list ──
        let list_h = self.list_h();

        if !radio_on {
            // The kernel empties the network list while the radio is off, so without this the panel
            // would say "No networks in range" — which blames the neighbourhood for a switch the
            // user just flicked themselves.
            canvas.print_str(
                24,
                LIST_TOP + 20,
                "Wi-Fi is off. Turn it on to see available networks.",
                Color::TEXT_MUTED,
                1,
            );
        } else if self.nets.is_empty() {
            canvas.print_str(24, LIST_TOP + 20, "No networks in range.", Color::TEXT_MUTED, 1);
        }

        for (i, net) in self.nets.iter().enumerate() {
            let row_top = LIST_TOP + i * ROW_H;
            if row_top + ROW_H <= LIST_TOP + self.scroll {
                continue;
            }
            let y = row_top - self.scroll;
            if y + ROW_H > LIST_TOP + list_h {
                break;
            }

            let selected = self.sel == Some(i);
            if selected {
                canvas.fill_rect(20, y, w.saturating_sub(40), ROW_H - 4, 0xFF_FDEBD8);
                canvas.fill_rect(20, y, 3, ROW_H - 4, Color::ACCENT_PRIMARY);
            } else if net.is_current() {
                canvas.fill_rect(20, y, w.saturating_sub(40), ROW_H - 4, Color::WARM_SURFACE);
            }

            let unsupported = net.is_unsupported_security();
            let name_color = if unsupported {
                Color::TEXT_MUTED
            } else {
                Color::TEXT_DARK
            };
            canvas.print_str(36, y + 9, net.name(), name_color, 1);

            // Right-hand detail: band/channel, then the padlock.
            let band = if net.band == 1 { "2.4 GHz" } else { "5 GHz" };
            let detail = alloc::format!("{}  ch {}", band, net.channel);
            let dw = Canvas::text_width(&detail, 1);
            let lock_x = w.saturating_sub(44);
            canvas.print_str(
                lock_x.saturating_sub(dw + 14),
                y + 9,
                &detail,
                Color::TEXT_MUTED,
                1,
            );
            if net.is_secure() {
                Self::draw_lock(canvas, lock_x, y + 11, name_color);
            }

            if net.is_current() && self.connected() {
                canvas.print_str(36, y + 22, "Connected", Color::ACCENT_GREEN, 1);
            } else if unsupported {
                canvas.print_str(36, y + 22, "Unsupported security", Color::TEXT_MUTED, 1);
            } else if self.saved.as_deref() == Some(net.name()) {
                canvas.print_str(36, y + 22, "Remembered", Color::TEXT_MUTED, 1);
            }
        }

        // ── Password / join panel ──
        if let Some(i) = self.sel {
            let pt = self.panel_top();
            let (secure, unsupported, name) = match self.nets.get(i) {
                Some(n) => (n.is_secure(), n.is_unsupported_security(), String::from(n.name())),
                None => (false, false, String::new()),
            };

            canvas.fill_rect(0, pt, w, h.saturating_sub(pt), Color::WARM_SURFACE);
            canvas.fill_rect(0, pt, w, 1, Color::WARM_BORDER);

            let prompt = if unsupported {
                alloc::format!("\"{}\" uses WEP or WPA1 — not supported.", name)
            } else if secure {
                alloc::format!("Enter the password for \"{}\"", name)
            } else {
                alloc::format!("\"{}\" is an open network — no password needed.", name)
            };
            canvas.print_str(20, pt + 14, &prompt, Color::TEXT_DARK, 1);

            if secure {
                let fw = w.saturating_sub(180);
                canvas.fill_rect(20, pt + 42, fw, 30, Color::WARM_BG);
                canvas.fill_rect(20, pt + 42, fw, 1, Color::WARM_BORDER);
                canvas.fill_rect(20, pt + 71, fw, 1, Color::WARM_BORDER);

                // Masked, so a shoulder can't read it back off the screen.
                let mut dots = String::new();
                for _ in 0..self.psk.chars().count() {
                    dots.push('*');
                }
                canvas.print_str(30, pt + 50, &dots, Color::TEXT_DARK, 1);
                let cw = Canvas::text_width(&dots, 1);
                canvas.fill_rect(30 + cw + 1, pt + 48, 1, 18, Color::TEXT_DARK);
            }

            Self::draw_button(canvas, self.btn_cancel(), "Cancel", !self.busy, false);
            Self::draw_button(canvas, self.btn_join(), "Join", !unsupported && !self.busy, true);

            // "Connect automatically" checkbox.
            let cb = self.box_remember();
            canvas.fill_rect(cb.0, cb.1, cb.2, cb.3, Color::WARM_BG);
            canvas.fill_rect(cb.0, cb.1, cb.2, 1, Color::WARM_BORDER);
            canvas.fill_rect(cb.0, cb.1 + cb.3 - 1, cb.2, 1, Color::WARM_BORDER);
            canvas.fill_rect(cb.0, cb.1, 1, cb.3, Color::WARM_BORDER);
            canvas.fill_rect(cb.0 + cb.2 - 1, cb.1, 1, cb.3, Color::WARM_BORDER);
            if self.remember {
                canvas.fill_rect(cb.0 + 4, cb.1 + 4, cb.2 - 8, cb.3 - 8, Color::ACCENT_PRIMARY);
            }
            canvas.print_str(
                cb.0 + cb.2 + 10,
                cb.1,
                "Connect to this network automatically",
                Color::TEXT_DARK,
                1,
            );
        }

        // ── Status message, bottom-left ──
        canvas.print_str(20, h.saturating_sub(24), &self.message, Color::TEXT_MUTED, 1);
    }

    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if !clicked || self.busy {
            return false;
        }

        // The switch is checked first and works in every state — it's the way out of "off", so it can
        // never be gated behind something that "off" itself disables.
        if hit(self.sw_radio(), mx, my) {
            if self.radio_on() {
                self.queue(Pending::RadioOff, String::from("Turning Wi-Fi off…"));
            } else {
                self.queue(Pending::RadioOn, String::from("Turning Wi-Fi on…"));
            }
            return true;
        }

        if !self.radio_on() {
            // Everything below acts on the radio. Forget is the exception — it only rewrites a file
            // — so it stays reachable with Wi-Fi off.
            if let Some(r) = self.btn_forget() {
                if hit(r, mx, my) {
                    if wifi_forget_network() {
                        self.saved = None;
                        self.remember = false;
                        self.message =
                            String::from("Forgotten — this machine won't reconnect on boot.");
                    } else {
                        self.message = String::from("Couldn't update the saved-network file.");
                    }
                    return true;
                }
            }
            return false;
        }

        if hit(self.btn_rescan(), mx, my) {
            if self.connected() {
                self.message =
                    String::from("Disconnect first — scanning would drop the current link.");
            } else {
                self.queue(
                    Pending::Scan,
                    String::from("Scanning all channels, this takes a few seconds…"),
                );
            }
            return true;
        }

        if let Some(r) = self.btn_disconnect() {
            if hit(r, mx, my) {
                self.queue(Pending::Disconnect, String::from("Disconnecting…"));
                return true;
            }
        }

        if let Some(r) = self.btn_forget() {
            if hit(r, mx, my) {
                if wifi_forget_network() {
                    self.saved = None;
                    self.remember = false;
                    self.message = String::from("Forgotten — this machine won't reconnect on boot.");
                } else {
                    self.message = String::from("Couldn't update the saved-network file.");
                }
                return true;
            }
        }

        // Panel widgets (checked before the list — they sit below it).
        if self.sel.is_some() {
            if hit(self.box_remember(), mx, my) {
                self.remember = !self.remember;
                return true;
            }
            if hit(self.btn_cancel(), mx, my) {
                self.sel = None;
                self.psk.clear();
                self.message = String::from("Select a network to connect.");
                return true;
            }
            if hit(self.btn_join(), mx, my) {
                let name = self.selected_name();
                let msg = if self.connected() {
                    alloc::format!("Switching to \"{}\"…", name)
                } else {
                    alloc::format!("Connecting to \"{}\"…", name)
                };
                self.queue(Pending::Connect, msg);
                return true;
            }
            if my >= self.panel_top() {
                return false;
            }
        }

        // List rows
        let list_h = self.list_h();
        if my >= LIST_TOP && my < LIST_TOP + list_h {
            let idx = (my - LIST_TOP + self.scroll) / ROW_H;
            if idx < self.nets.len() {
                if self.sel == Some(idx) {
                    return false;
                }
                self.sel = Some(idx);
                self.psk.clear();
                // Default the checkbox to "yes" for a fresh pick, so remembering is the norm.
                self.remember = true;
                self.message = String::from("Enter the password, then press Join.");
                return true;
            }
        }
        false
    }

    fn on_key(&mut self, key: char) -> bool {
        if self.busy || self.sel.is_none() {
            return false;
        }
        match key {
            '\x08' => self.psk.pop().is_some(),
            '\n' | '\r' => {
                let name = self.selected_name();
                if name.is_empty() {
                    return false;
                }
                let msg = if self.connected() {
                    alloc::format!("Switching to \"{}\"…", name)
                } else {
                    alloc::format!("Connecting to \"{}\"…", name)
                };
                self.queue(Pending::Connect, msg);
                true
            }
            c if (c as u32) >= 0x20 && (c as u32) < 0x7f => {
                if self.psk.len() < 63 {
                    self.psk.push(c);
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    // Scrollbar: the compositor draws and drives it from these three.
    fn content_height(&self) -> usize {
        LIST_TOP + self.nets.len() * ROW_H + 8
    }
    fn scroll_offset(&self) -> usize {
        self.scroll
    }
    fn on_scroll(&mut self, new_offset: usize) -> bool {
        let max = (self.nets.len() * ROW_H).saturating_sub(self.list_h());
        let clamped = new_offset.min(max);
        if clamped == self.scroll {
            return false;
        }
        self.scroll = clamped;
        true
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let heap_start = sys_alloc_pages(256);
    if heap_start == 0 {
        sys_exit(1);
    }
    unsafe {
        ALLOCATOR.lock().init(heap_start as *mut u8, 256 * 4096);
    }
    nyx_gui::app::run(WifiApp::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(111);
}
