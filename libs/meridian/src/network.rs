//! The network drill-down — `parts/08b-notify-network.html`, "Progressive disclosure".
//!
//! *"Nothing in Meridian opens a settings window to answer a question. The Entity surface drills in
//! place: tap Network on the first level and the panel becomes the network view, with a back
//! affordance and no new surface. Detail is available everywhere and present nowhere."*
//!
//! This is step 20 of the build order — *"Retire `apps/wifi`. Only once the Entity drill-down is
//! doing its job. Deleting an application is the last step, never the first."* — and this module is
//! the "doing its job" half. It is the same surface as the Entity's root view, at the same width,
//! anchored to the same bottom edge, built from the same row metrics, so the two read as one panel
//! showing two things rather than as two panels.
//!
//! ## Three things the design shows that this cannot
//!
//! The design's panel has a throughput reading (`1.2 MB/s`), a DHCP lease age (`lease 12 h`) and a
//! signal strength per network (`−46 dBm`, and a whole column of them in the Available list). None
//! of the three exist on this machine, and the last one is worth spelling out because the design
//! document explicitly claims it does:
//!
//! > *"`sys_wifi_status`, `sys_wifi_list` and `sys_wifi_scan` are already in `libs/api`, along with
//! > the `WifiNetwork` struct carrying SSID and signal."*
//!
//! [`WifiNetwork`] carries `ssid`, `ssid_len`, `channel`, `band`, `flags` and `bssid`. There is no
//! RSSI field, and there is none because the driver never reads one: `parse_beacon` deliberately
//! avoids the `iwl_rx_mpdu_desc` entirely — its v1 and v3 layouts differ — and locates the 802.11
//! header by scanning for the beacon signature instead. `energy_a`/`energy_b` live in exactly the
//! structure it refuses to interpret, so producing a dBm would mean guessing an offset and
//! validating it on hardware, and a wrong guess yields a plausible number rather than an obvious
//! one. On a surface whose entire premise is that it is *the thing you trust about the machine*,
//! that is the worst possible failure.
//!
//! So the signal column is **omitted**, exactly as System Monitor omits Graphics % and QCLang omits
//! Fidelity. What each row shows instead — band, channel, and whether we can actually join it — is
//! read from the beacon and true. Add the column when the descriptor is decoded, not before.

use alloc::string::String;
use alloc::{format, vec::Vec};
use nyx_api::{
    WifiNetwork, WifiStatus, WIFI_AUTH_FAILED, WIFI_CONNECTED, WIFI_CONNECTING, WIFI_HW_FAILED,
    WIFI_NO_LEASE, WIFI_RADIO_OFF,
};

use crate::layout::{
    Rect, ENTITY_RIGHT, ENTITY_SURFACE_BOTTOM, ENTITY_SURFACE_W, ENT_CROW_H, ENT_CROW_PAD_X,
    ENT_C_PAD_X, ENT_DETAIL_LINE, ENT_F_LINE_H, ENT_F_PAD_BOTTOM, ENT_F_PAD_TOP, ENT_HD_GAP,
    ENT_HD_PAD_BOTTOM, ENT_HD_PAD_TOP, ENT_HD_PAD_X, ENT_MROW_H, ENT_M_PAD_Y, ENT_STATEMENT_H,
    ENT_TOP_MARGIN,
};
use crate::scale::px as sc;

// ─────────────────────────────── Metrics ───────────────────────────────
//
// Everything imported above is the Entity surface's own. Everything declared here is new to this
// view, and each one is named so that changing it is a decision about this view alone.

/// The back affordance sits in a `.crow` with tighter padding than `.ent-c` — it is one row, not a
/// block of them, and the design's 12px bottom pad exists to separate the last control from the
/// footer rule.
const NET_BACK_PAD: i32 = 8;

/// `AVAILABLE` — the same 11px uppercase label the Command groups its results with.
const NET_LABEL_TOP: i32 = 16;
const NET_LABEL_H: i32 = 14;
const NET_LABEL_GAP: i32 = 6;
const NET_AVAIL_PAD_BOTTOM: i32 = 12;

/// The passphrase block: a field on a hairline, then the hint line under it. Same shape as the
/// lock's field in [`crate::idle`], because it is the same gesture — type, press Return.
const NET_PSK_PAD_TOP: i32 = 14;
const NET_PSK_H: i32 = 28;
const NET_PSK_HINT_GAP: i32 = 8;
const NET_PSK_HINT_H: i32 = 14;
const NET_PSK_PAD_BOTTOM: i32 = 14;

/// The footer carries two lines: the actions, then whatever the view last had to say.
const NET_MSG_GAP: i32 = 6;

/// The dot and caret of the passphrase field, when it is masked.
const NET_DOT: i32 = 5;
const NET_DOT_GAP: i32 = 6;
const NET_CARET_W: i32 = 1;
const NET_CARET_H: i32 = 18;

/// The most link rows there can ever be — Address, Gateway, Resolver.
pub const NET_LINK_ROWS: usize = 3;

// ─────────────────────────────── Layout ───────────────────────────────

/// A resolved network view. Every rect is in screen coordinates, like [`crate::layout::EntityLayout`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelLayout {
    pub surface: Rect,
    /// The back affordance's row — `←` and the word "Network".
    pub back: Rect,
    /// The statement: the SSID when there is one.
    pub statement: Rect,
    /// One line, always. Unlike the Entity's detail this is a state phrase and never wraps, which
    /// is why this view has no `max_detail_lines` counterpart.
    pub detail: Rect,
    /// Address / Gateway / Resolver, as a block. Zero-height when nothing is connected.
    pub links: Rect,
    /// The `AVAILABLE` label and the network rows under it, as a block.
    pub rows: Rect,
    /// The passphrase field and its hint. Zero-height unless a secured network is selected.
    pub psk: Rect,
    /// The actions line and the message under it.
    pub footer: Rect,
    n_links: i32,
    n_rows: i32,
    has_psk: bool,
}

/// Lay the network view out.
///
/// `n_links` is how many of Address/Gateway/Resolver are actually known (see [`link_rows`]),
/// `n_rows` how many network rows are being drawn *including* a pager row if there is one, and
/// `has_psk` whether the passphrase field is up.
pub fn panel_layout(
    screen_w: i32,
    screen_h: i32,
    n_links: usize,
    n_rows: usize,
    has_psk: bool,
) -> PanelLayout {
    let back_h = sc(NET_BACK_PAD) * 2 + sc(ENT_CROW_H);
    let head_h = sc(ENT_HD_PAD_TOP)
        + sc(ENT_STATEMENT_H)
        + sc(ENT_HD_GAP)
        + sc(ENT_DETAIL_LINE)
        + sc(ENT_HD_PAD_BOTTOM);
    let links_h = if n_links > 0 {
        sc(ENT_M_PAD_Y) * 2 + n_links as i32 * sc(ENT_MROW_H)
    } else {
        0
    };
    let rows_h = sc(NET_LABEL_TOP)
        + sc(NET_LABEL_H)
        + sc(NET_LABEL_GAP)
        + n_rows as i32 * sc(ENT_CROW_H)
        + sc(NET_AVAIL_PAD_BOTTOM);
    let psk_h = if has_psk {
        sc(NET_PSK_PAD_TOP)
            + sc(NET_PSK_H)
            + sc(NET_PSK_HINT_GAP)
            + sc(NET_PSK_HINT_H)
            + sc(NET_PSK_PAD_BOTTOM)
    } else {
        0
    };
    let footer_h = sc(ENT_F_PAD_TOP)
        + sc(ENT_F_LINE_H)
        + sc(NET_MSG_GAP)
        + sc(ENT_F_LINE_H)
        + sc(ENT_F_PAD_BOTTOM);

    let content_h = back_h + head_h + links_h + rows_h + psk_h + footer_h;
    let mut surface = Rect::new(
        screen_w - sc(ENTITY_RIGHT) - sc(ENTITY_SURFACE_W),
        screen_h - sc(ENTITY_SURFACE_BOTTOM) - content_h,
        sc(ENTITY_SURFACE_W),
        content_h,
    );
    // The same backstop the Entity's root view has, and it matters more here: the row count is
    // driven by how many access points the neighbourhood has, which is not something the shell
    // controls. `max_rows` is what is supposed to prevent this; this is what happens if a caller
    // forgets. Losing the bottom-edge anchor beats losing the back affordance off the top.
    if surface.y < sc(ENT_TOP_MARGIN) {
        surface.y = sc(ENT_TOP_MARGIN);
    }

    let inner_w = surface.w - sc(ENT_HD_PAD_X) * 2;
    let head_y = surface.y + back_h;
    let stmt_y = head_y + sc(ENT_HD_PAD_TOP);
    let links_y = head_y + head_h;
    let rows_y = links_y + links_h;
    let psk_y = rows_y + rows_h;
    let footer_y = psk_y + psk_h;

    PanelLayout {
        surface,
        back: Rect::new(
            surface.x + sc(ENT_C_PAD_X),
            surface.y + sc(NET_BACK_PAD),
            surface.w - sc(ENT_C_PAD_X) * 2,
            sc(ENT_CROW_H),
        ),
        statement: Rect::new(surface.x + sc(ENT_HD_PAD_X), stmt_y, inner_w, sc(ENT_STATEMENT_H)),
        detail: Rect::new(
            surface.x + sc(ENT_HD_PAD_X),
            stmt_y + sc(ENT_STATEMENT_H) + sc(ENT_HD_GAP),
            inner_w,
            sc(ENT_DETAIL_LINE),
        ),
        links: Rect::new(surface.x, links_y, surface.w, links_h),
        rows: Rect::new(surface.x, rows_y, surface.w, rows_h),
        psk: Rect::new(surface.x, psk_y, surface.w, psk_h),
        footer: Rect::new(surface.x + sc(ENT_HD_PAD_X), footer_y, inner_w, footer_h),
        n_links: n_links as i32,
        n_rows: n_rows as i32,
        has_psk,
    }
}

impl PanelLayout {
    /// Link row `i` — a key left, a value right-aligned, exactly like a measurement row.
    pub fn link_row(&self, i: usize) -> Option<Rect> {
        if i as i32 >= self.n_links {
            return None;
        }
        Some(Rect::new(
            self.links.x + sc(ENT_HD_PAD_X),
            self.links.y + sc(ENT_M_PAD_Y) + i as i32 * sc(ENT_MROW_H),
            self.links.w - sc(ENT_HD_PAD_X) * 2,
            sc(ENT_MROW_H),
        ))
    }

    /// `(x, y)` of the `AVAILABLE` label.
    pub fn section_label(&self) -> (i32, i32) {
        (self.rows.x + sc(ENT_HD_PAD_X), self.rows.y + sc(NET_LABEL_TOP))
    }

    /// Network row `i` — the full-width hover plate, inset like a control row.
    pub fn row(&self, i: usize) -> Option<Rect> {
        if i as i32 >= self.n_rows {
            return None;
        }
        Some(Rect::new(
            self.rows.x + sc(ENT_C_PAD_X),
            self.rows.y + sc(NET_LABEL_TOP) + sc(NET_LABEL_H) + sc(NET_LABEL_GAP)
                + i as i32 * sc(ENT_CROW_H),
            self.rows.w - sc(ENT_C_PAD_X) * 2,
            sc(ENT_CROW_H),
        ))
    }

    /// Which network row is under `(mx, my)`.
    pub fn row_hit(&self, mx: i32, my: i32) -> Option<usize> {
        (0..self.n_rows as usize)
            .find(|&i| self.row(i).map(|r| r.contains(mx, my)).unwrap_or(false))
    }

    /// The passphrase field. `None` when it is not up.
    pub fn psk_field(&self) -> Option<Rect> {
        if !self.has_psk {
            return None;
        }
        Some(Rect::new(
            self.psk.x + sc(ENT_HD_PAD_X),
            self.psk.y + sc(NET_PSK_PAD_TOP),
            self.psk.w - sc(ENT_HD_PAD_X) * 2,
            sc(NET_PSK_H),
        ))
    }

    /// The hairline under the field — Meridian has no boxed text input, the rule *is* the field.
    pub fn psk_rule(&self) -> Option<Rect> {
        self.psk_field().map(|f| Rect::new(f.x, f.bottom(), f.w, 1))
    }

    /// The hint line under the rule ("Return to join · Show").
    pub fn psk_hint(&self) -> Option<Rect> {
        self.psk_field().map(|f| {
            Rect::new(f.x, f.bottom() + 1 + sc(NET_PSK_HINT_GAP), f.w, sc(NET_PSK_HINT_H))
        })
    }

    /// The actions line, and the message line under it.
    pub fn actions(&self) -> Rect {
        Rect::new(
            self.footer.x,
            self.footer.y + sc(ENT_F_PAD_TOP),
            self.footer.w,
            sc(ENT_F_LINE_H),
        )
    }
    pub fn message(&self) -> Rect {
        let a = self.actions();
        Rect::new(a.x, a.bottom() + sc(NET_MSG_GAP), a.w, sc(ENT_F_LINE_H))
    }

    /// The hairlines: under the back row, under the head, under the links, under the available
    /// block, and above the footer — skipping any section that collapsed to nothing, because a rule
    /// with no block under it reads as an empty section rather than as a separator.
    pub fn rules(&self) -> Vec<i32> {
        let mut out = Vec::new();
        out.push(self.back.bottom() + sc(NET_BACK_PAD));
        out.push(self.links.y);
        if self.n_links > 0 {
            out.push(self.rows.y);
        }
        if self.has_psk {
            out.push(self.psk.y);
        }
        out.push(self.footer.y);
        out
    }

    /// The masked passphrase's dots and its caret, left-aligned in the field.
    ///
    /// Left-aligned rather than centred like the lock's, because this field can hold 63 characters
    /// and a centred run of dots that grows in both directions is a caret that never sits still.
    pub fn psk_dots(&self, typed: usize) -> Option<(impl Iterator<Item = Rect>, Rect)> {
        let f = self.psk_field()?;
        let d = sc(NET_DOT).max(1);
        let gap = sc(NET_DOT_GAP);
        let caret_w = sc(NET_CARET_W).max(1);
        let caret_h = sc(NET_CARET_H);
        let cap = self.psk_dot_capacity();
        let n = typed.min(cap);
        let cy = f.y + f.h / 2;
        let x0 = f.x;
        let dots = (0..n).map(move |i| Rect::new(x0 + i as i32 * (d + gap), cy - d / 2, d, d));
        let caret = Rect::new(x0 + n as i32 * (d + gap), cy - caret_h / 2, caret_w, caret_h);
        Some((dots, caret))
    }

    /// How many dots fit before the caret would leave the field. Beyond this the dots stop growing
    /// — the count is not the point, and a row of dots that overflows its own rule looks broken.
    pub fn psk_dot_capacity(&self) -> usize {
        let Some(f) = self.psk_field() else { return 0 };
        let step = (sc(NET_DOT).max(1) + sc(NET_DOT_GAP)).max(1);
        ((f.w - sc(NET_CARET_W).max(1)) / step).max(0) as usize
    }
}

/// The most network rows that fit on this screen, pager row included.
///
/// The counterpart to [`crate::layout::entity_max_detail_lines`] and for the same reason: this
/// surface grows upward from a fixed bottom edge and nothing clips it at the top, so a flat with
/// thirty access points in range would push the back affordance off the screen. At least one row,
/// always — a view that could show no networks at all would be worse than a short list.
pub fn max_rows(screen_h: i32, n_links: usize, has_psk: bool) -> usize {
    // `screen_w` does not affect height, so any value will do.
    let bare = panel_layout(0, screen_h, n_links, 0, has_psk).surface.h;
    let room = screen_h - sc(ENTITY_SURFACE_BOTTOM) - sc(ENT_TOP_MARGIN) - bare;
    let per = sc(ENT_CROW_H).max(1);
    // Never below 2 — see [`paginate`] for why one row is not a view.
    ((room / per).max(0) as usize).max(2)
}

/// The tallest this view may be before it starts running off the top of the screen. Separate from
/// [`panel_layout`], which *clamps* rather than refusing: once it has clamped, the surface's `y`
/// no longer reveals that anything overflowed, so this is the only honest way to ask.
pub fn height_budget(screen_h: i32) -> i32 {
    screen_h - sc(ENTITY_SURFACE_BOTTOM) - sc(ENT_TOP_MARGIN)
}

/// Which slice of the network list a page shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Page {
    pub start: usize,
    pub len: usize,
    /// How many pages there are. 1 means everything fits and there is no pager row.
    pub pages: usize,
    /// The page actually shown, after clamping.
    pub index: usize,
}

impl Page {
    /// Whether the last drawn row is the pager rather than a network.
    pub fn has_pager(&self) -> bool {
        self.pages > 1
    }
    /// Rows to draw, pager included.
    pub fn rows(&self) -> usize {
        self.len + if self.has_pager() { 1 } else { 0 }
    }
}

/// Page a list of `total` networks into `cap` rows.
///
/// There is no scroll wheel on this machine — `sys_get_mouse` returns x, y and two buttons, which
/// is why every scrollable window gets a draggable scrollbar instead — and this surface has no
/// scrollbar. So a list longer than the screen is *paged*, and the last row becomes a plain-text
/// action that advances. Plain text, because *"actions are plain text, never buttons"*.
/// ⚠️ A cap of **one** cannot carry both a network and the way to the next one, so a caller that
/// passes 1 gets two rows back and [`max_rows`] never returns less than 2. There is no arrangement
/// of a single row that works, and silently dropping every network after the first would be a list
/// that lies about being complete.
pub fn paginate(total: usize, cap: usize, page: usize) -> Page {
    let cap = cap.max(2);
    if total <= cap {
        return Page { start: 0, len: total, pages: 1, index: 0 };
    }
    let per = cap - 1; // the last row is the pager
    let pages = total.div_ceil(per);
    let index = page % pages;
    let start = index * per;
    Page { start, len: per.min(total - start), pages, index }
}

// ─────────────────────────────── What the rows say ───────────────────────────────

/// Address / Gateway / Resolver, skipping anything the lease did not supply.
///
/// ⚠️ A zero address is *not known*, and `0.0.0.0` on screen is a plausible-looking lie — it is a
/// real address that means "unspecified", so a reader who knows what it means learns the wrong
/// thing and a reader who does not learns nothing. The row is dropped instead, which is why
/// [`panel_layout`] takes the count rather than assuming three.
pub fn link_rows(st: &WifiStatus) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    if st.state != WIFI_CONNECTED {
        return out;
    }
    for (k, v) in [("Address", st.ip), ("Gateway", st.router), ("Resolver", st.dns)] {
        if v != [0, 0, 0, 0] {
            out.push((k, ip_text(v)));
        }
    }
    out
}

pub fn ip_text(ip: [u8; 4]) -> String {
    format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
}

/// The statement — the SSID when there is one, because that is the answer to the question the
/// drill-down was opened to ask.
pub fn statement(st: &WifiStatus) -> String {
    if st.state == WIFI_CONNECTED && st.ssid_len > 0 {
        String::from(st.name())
    } else if st.state == WIFI_RADIO_OFF {
        String::from("Wi-Fi is off")
    } else {
        String::from("Not connected")
    }
}

/// The line under the statement — the design's *"Connected · WPA2"*.
///
/// `secure` is whether the network we are on advertises RSN; the shell resolves it from the scan
/// list, because [`WifiStatus`] does not carry it. When the current SSID is not in the scan list
/// (it happens — the list is from the last sweep, and the boot agent joins from a saved profile
/// before any sweep the user asked for) it is `None`, and the cipher is left unsaid rather than
/// guessed. Saying "Connected" alone is true; saying "Connected · WPA2" about an open network is
/// the kind of security claim this project does not make casually.
pub fn state_detail(st: &WifiStatus, secure: Option<bool>) -> String {
    match st.state {
        WIFI_RADIO_OFF => String::from("The radio is off. Nothing is being scanned or joined."),
        WIFI_CONNECTED => match secure {
            Some(true) => String::from("Connected \u{00B7} WPA2"),
            Some(false) => String::from("Connected \u{00B7} open network, unencrypted"),
            None => String::from("Connected"),
        },
        WIFI_CONNECTING => String::from("Joining\u{2026}"),
        WIFI_AUTH_FAILED => String::from("The password was rejected."),
        WIFI_NO_LEASE => String::from("Joined, but the router offered no address."),
        WIFI_HW_FAILED => String::from("The adapter did not respond."),
        _ => String::from("Pick a network below."),
    }
}

/// The right-hand detail on a network row: band and channel, both straight out of the beacon.
///
/// This is the slot the design fills with a dBm reading. See the module note for why it does not.
pub fn row_meta(n: &WifiNetwork) -> String {
    format!("{} \u{00B7} ch {}", band_text(n), n.channel)
}

pub fn band_text(n: &WifiNetwork) -> &'static str {
    if n.band == 1 {
        "2.4 GHz"
    } else {
        "5 GHz"
    }
}

/// Why a network cannot be joined, in words that name the setting to change rather than the
/// standard that was violated. `None` when it can be.
pub fn refusal(n: &WifiNetwork) -> Option<&'static str> {
    if n.needs_tkip() {
        // Inherited verbatim from apps/wifi, which learned it the hard way: the AP rejects the
        // association with status 41 and nothing in the reject says why.
        Some("TKIP group cipher \u{2014} router is in WPA/WPA2 mixed mode")
    } else if n.is_legacy_security() {
        Some("WEP or WPA1 \u{2014} not supported")
    } else {
        None
    }
}

/// The same refusal in two or three words, for the row itself.
///
/// A row is 40 units tall and its right-hand slot holds `2.4 GHz · ch 11`; the full sentence does
/// not fit and would have to be ellipsized into uselessness. So the row says *what* is wrong in the
/// space it has, greyed, and clicking it puts the full sentence — the one naming the router setting
/// to change — on the message line. The old picker printed the long form on a second line inside a
/// taller row; this surface does not have the height for one.
pub fn refusal_short(n: &WifiNetwork) -> Option<&'static str> {
    if n.needs_tkip() {
        Some("Mixed mode")
    } else if n.is_legacy_security() {
        Some("WEP or WPA1")
    } else {
        None
    }
}

/// Whether Join can do anything with this network.
pub fn joinable(n: &WifiNetwork) -> bool {
    refusal(n).is_none()
}

/// The right-hand slot of a network row: the reason it cannot be joined if there is one, the band
/// and channel otherwise. One function so the row can never show both or neither.
pub fn row_status(n: &WifiNetwork, saved: bool) -> String {
    match refusal_short(n) {
        Some(why) => String::from(why),
        // *"Remembered"* earns the slot over the band: it is the answer to "what would Forget
        // drop", which is the only question the band cannot help with. The band is on every other
        // row, so its absence on this one is not information going missing.
        None if saved => String::from("Remembered"),
        None => row_meta(n),
    }
}

/// Sort the scan list into the order it is drawn in.
///
/// Scan order is arrival order and reshuffles on every sweep, which makes a list feel unstable to
/// click at exactly the moment a click matters. So: the network we are on first, then the one we
/// remember, then everything joinable, then everything that is not — and alphabetically inside each
/// band. `sort_unstable_by` because `sort_by` wants an allocation this crate does not need to make.
pub fn order(nets: &mut [WifiNetwork], saved: Option<&str>) {
    fn rank(n: &WifiNetwork, saved: Option<&str>) -> u8 {
        if n.is_current() {
            0
        } else if saved == Some(n.name()) {
            1
        } else if joinable(n) {
            2
        } else {
            3
        }
    }
    nets.sort_unstable_by(|a, b| {
        rank(a, saved).cmp(&rank(b, saved)).then_with(|| a.name().cmp(b.name()))
    });
}

// ─────────────────────────────── Tests ───────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use nyx_api::{wifi_op_decode, wifi_op_encode, WifiOp, WIFI_IDLE, WIFI_OP_MAX};

    fn net(name: &str, flags: u8, channel: u8) -> WifiNetwork {
        let mut n = WifiNetwork::default();
        n.ssid[..name.len()].copy_from_slice(name.as_bytes());
        n.ssid_len = name.len() as u8;
        n.flags = flags;
        n.channel = channel;
        n.band = if channel <= 14 { 1 } else { 0 };
        n
    }

    fn connected(ssid: &str) -> WifiStatus {
        let mut s = WifiStatus::default();
        s.state = WIFI_CONNECTED;
        s.ssid[..ssid.len()].copy_from_slice(ssid.as_bytes());
        s.ssid_len = ssid.len() as u8;
        s.ip = [192, 168, 1, 24];
        s.router = [192, 168, 1, 1];
        s.dns = [1, 1, 1, 1];
        s
    }

    // ── geometry ──────────────────────────────────────────────────────────

    #[test]
    fn the_sections_tile_the_surface_exactly() {
        for (links, rows, psk) in
            [(0usize, 0usize, false), (3, 6, false), (0, 4, true), (2, 1, true), (3, 9, true)]
        {
            let l = panel_layout(1920, 1080, links, rows, psk);
            // back → head → links → avail → psk → footer, with nothing between and nothing over.
            let back_bottom = l.back.bottom() + sc(NET_BACK_PAD);
            assert_eq!(back_bottom, l.statement.y - sc(ENT_HD_PAD_TOP), "back/head gap");
            assert_eq!(l.links.y, l.detail.bottom() + sc(ENT_HD_PAD_BOTTOM), "head/links gap");
            assert_eq!(l.rows.y, l.links.bottom(), "links/avail gap");
            assert_eq!(l.psk.y, l.rows.bottom(), "avail/psk gap");
            assert_eq!(l.footer.y, l.psk.bottom(), "psk/footer gap");
            assert_eq!(l.footer.bottom(), l.surface.bottom(), "footer to the bottom edge");
            assert_eq!(l.surface.y, l.back.y - sc(NET_BACK_PAD), "back to the top edge");
        }
    }

    #[test]
    fn a_collapsed_section_takes_no_room_and_gets_no_rule() {
        let with = panel_layout(1920, 1080, 3, 4, true);
        let without = panel_layout(1920, 1080, 0, 4, false);
        assert!(without.surface.h < with.surface.h);
        assert_eq!(without.links.h, 0);
        assert_eq!(without.psk.h, 0);
        assert_eq!(without.link_row(0), None);
        assert_eq!(without.psk_field(), None);
        // A hairline with nothing under it reads as an empty section, not as a separator.
        assert_eq!(with.rules().len(), 5);
        assert_eq!(without.rules().len(), 3);
    }

    #[test]
    fn the_surface_keeps_the_entitys_width_and_bottom_edge() {
        // The whole point of drilling in place is that the panel does not move or change size in
        // any way the eye can catch. Only its height may change, and only downward from the top.
        let root = crate::layout::entity_layout(1920, 1080, 1, 4, 4);
        let net = panel_layout(1920, 1080, 3, 5, false);
        assert_eq!(net.surface.x, root.surface.x);
        assert_eq!(net.surface.w, root.surface.w);
        assert_eq!(net.surface.bottom(), root.surface.bottom());
    }

    #[test]
    fn rows_stay_inside_their_block() {
        let l = panel_layout(1920, 1080, 3, 7, true);
        for i in 0..3 {
            let r = l.link_row(i).unwrap();
            assert!(r.y >= l.links.y && r.bottom() <= l.links.bottom(), "link {i}");
        }
        for i in 0..7 {
            let r = l.row(i).unwrap();
            assert!(r.y >= l.rows.y && r.bottom() <= l.rows.bottom(), "row {i}");
            assert_eq!(l.row_hit(r.x + 2, r.y + 2), Some(i));
        }
        assert_eq!(l.row(7), None);
        let f = l.psk_field().unwrap();
        assert!(f.y >= l.psk.y && l.psk_hint().unwrap().bottom() <= l.psk.bottom());
        assert!(l.message().bottom() <= l.footer.bottom());
    }

    #[test]
    fn max_rows_keeps_the_view_on_the_screen() {
        // The failure this prevents is silent: the surface grows upward and nothing clips it, so an
        // over-long list takes the back affordance off the top and strands the user in the view.
        for h in [720, 768, 900, 1080, 1440, 2160] {
            for (links, psk) in [(0usize, false), (3, true)] {
                let cap = max_rows(h, links, psk);
                let budget = height_budget(h);
                let l = panel_layout(1920, h, links, cap, psk);
                // ⚠️ Assert on the HEIGHT, not on `surface.y`. `panel_layout` clamps y to the top
                // margin, so once it has overflowed y reads as exactly in-bounds — an assertion on
                // y passes in both directions and proves nothing. This caught the cap being wrong
                // by a row.
                assert!(l.surface.h <= budget || cap == 2, "{h}px: {cap} rows overflow");
                assert!(l.surface.y >= sc(ENT_TOP_MARGIN), "{h}px, {cap} rows: ran off");
                // And one more row would not have fitted — the cap is tight, not merely safe.
                let over = panel_layout(1920, h, links, cap + 1, psk);
                assert!(over.surface.h > budget, "{h}px: cap {cap} is too conservative");
            }
        }
    }

    #[test]
    fn pagination_covers_every_network_exactly_once() {
        // From 2: a single row cannot carry both a network and the way to the next. `paginate`
        // clamps to 2 and `max_rows` never returns less, so 1 is not a case that occurs.
        for total in 0..40usize {
            for cap in 2..12usize {
                let p0 = paginate(total, cap, 0);
                if total <= cap {
                    assert_eq!(p0.pages, 1);
                    assert!(!p0.has_pager());
                    assert_eq!(p0.len, total);
                    continue;
                }
                assert!(p0.has_pager());
                // Walking the pages must land on every index in 0..total, once.
                let mut seen = alloc::vec![false; total];
                for page in 0..p0.pages {
                    let p = paginate(total, cap, page);
                    assert!(p.rows() <= cap, "total {total} cap {cap}: {} rows", p.rows());
                    for i in p.start..p.start + p.len {
                        assert!(!seen[i], "total {total} cap {cap}: index {i} twice");
                        seen[i] = true;
                    }
                }
                assert!(seen.iter().all(|&s| s), "total {total} cap {cap}: missed one");
            }
        }
    }

    #[test]
    fn the_pager_wraps_rather_than_running_off_the_end() {
        let p = paginate(20, 5, 0);
        assert_eq!(p.pages, 5);
        // Clicking past the last page comes back to the first, which is the only behaviour a single
        // affordance can have — there is no "previous" row to pair it with.
        assert_eq!(paginate(20, 5, 5), p);
        assert_eq!(paginate(20, 5, 12).index, 2);
    }

    #[test]
    fn the_dots_stop_at_the_edge_of_the_field() {
        let l = panel_layout(1920, 1080, 0, 3, true);
        let f = l.psk_field().unwrap();
        let cap = l.psk_dot_capacity();
        assert!(cap > 8, "a 63-character passphrase deserves more than {cap} dots");
        for typed in [0usize, 1, 8, cap, cap + 1, 63] {
            let (dots, caret) = l.psk_dots(typed).unwrap();
            let n = dots.count();
            assert_eq!(n, typed.min(cap));
            assert!(caret.right() <= f.right(), "{typed} typed: caret escaped the field");
        }
    }

    // ── content ───────────────────────────────────────────────────────────

    #[test]
    fn an_unknown_address_is_omitted_not_printed_as_zero() {
        let mut st = connected("Meridian");
        assert_eq!(link_rows(&st).len(), 3);
        assert_eq!(link_rows(&st)[0], ("Address", String::from("192.168.1.24")));
        st.dns = [0, 0, 0, 0];
        let rows = link_rows(&st);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|(k, _)| *k != "Resolver"));
        // Not connected: a stale lease is not a lease.
        st.state = WIFI_IDLE;
        assert!(link_rows(&st).is_empty());
    }

    #[test]
    fn the_cipher_is_left_unsaid_rather_than_guessed() {
        let st = connected("Meridian");
        assert_eq!(state_detail(&st, Some(true)), "Connected \u{00B7} WPA2");
        assert_eq!(state_detail(&st, None), "Connected");
        // An open network must never be described with a cipher it does not have.
        assert!(state_detail(&st, Some(false)).contains("unencrypted"));
        assert!(!state_detail(&st, Some(false)).contains("WPA2"));
    }

    #[test]
    fn the_statement_is_the_ssid_when_there_is_one() {
        assert_eq!(statement(&connected("Meridian")), "Meridian");
        let mut off = WifiStatus::default();
        off.state = WIFI_RADIO_OFF;
        assert_eq!(statement(&off), "Wi-Fi is off");
        assert_eq!(statement(&WifiStatus::default()), "Not connected");
    }

    #[test]
    fn a_refusal_names_the_setting_to_change() {
        let tkip = net("mixed", 0x01 | 0x02 | 0x08, 6);
        let wep = net("ancient", 0x01, 6);
        let wpa2 = net("fine", 0x01 | 0x02, 6);
        let open = net("cafe", 0x00, 6);
        assert!(refusal(&tkip).unwrap().contains("mixed mode"));
        assert!(refusal(&wep).unwrap().contains("WEP"));
        assert_eq!(refusal(&wpa2), None);
        assert_eq!(refusal(&open), None);
        assert!(joinable(&wpa2) && joinable(&open) && !joinable(&tkip) && !joinable(&wep));
    }

    #[test]
    fn the_list_is_stable_and_puts_the_useful_ones_first() {
        let mut nets = alloc::vec![
            net("zulu", 0x03, 1),
            net("mixed", 0x0B, 6),      // TKIP — unjoinable, sinks
            net("alpha", 0x03, 36),
            net("home", 0x07, 11),      // current — bit2
            net("saved-one", 0x03, 3),
        ];
        order(&mut nets, Some("saved-one"));
        let names: Vec<&str> = nets.iter().map(|n| n.name()).collect();
        assert_eq!(names, ["home", "saved-one", "alpha", "zulu", "mixed"]);

        // Stability is the property that matters: the same set in a different arrival order must
        // draw identically, or rows move under the pointer between sweeps.
        let mut shuffled = alloc::vec![nets[3], nets[0], nets[4], nets[2], nets[1]];
        order(&mut shuffled, Some("saved-one"));
        let after: Vec<&str> = shuffled.iter().map(|n| n.name()).collect();
        assert_eq!(names, after);
    }

    #[test]
    fn band_comes_from_the_channel_and_the_meta_says_both() {
        assert_eq!(band_text(&net("a", 0, 11)), "2.4 GHz");
        assert_eq!(band_text(&net("a", 0, 36)), "5 GHz");
        assert_eq!(row_meta(&net("a", 0, 36)), "5 GHz \u{00B7} ch 36");
    }

    // ── the request the shell hands to the agent ──────────────────────────
    //
    // The codec lives in `nyx-api` — it is an ABI between two processes, and neither `apps/shell`
    // nor `apps/wifiagent` can depend on the other. It is tested here because this is the nearest
    // crate with a host test harness, and an untested wire format between a window server and a
    // helper is a bug you can only find by power-cycling a laptop.

    #[test]
    fn every_request_round_trips() {
        // `WifiOp::ALL`, not a hand-written list: a new verb that nobody added here would be a verb
        // with no round-trip test, and the failure would be a helper silently doing nothing.
        for op in WifiOp::ALL {
            let mut buf = [0u8; WIFI_OP_MAX];
            let n = wifi_op_encode(&mut buf, op, "Meridian", "hunter2");
            let s = core::str::from_utf8(&buf[..n]).unwrap();
            let (got, ssid, psk) = wifi_op_decode(s).unwrap();
            assert_eq!(got, op);
            if op.is_join() {
                assert_eq!((ssid, psk), ("Meridian", "hunter2"));
            } else {
                // Nothing but a join carries credentials, and a Scan request that dragged a
                // passphrase along would put it on a code path with no reason to hold one.
                assert_eq!((ssid, psk), ("", ""));
                assert_eq!(s, op.verb());
            }
        }
    }

    #[test]
    fn joining_once_is_a_different_verb_from_joining() {
        // The whole point of the separate verb: the helper decides whether to write `wifi.conf` from
        // this and nothing else. If the two ever encoded the same, a café network would silently
        // replace the one the machine comes back to at home.
        assert_ne!(WifiOp::Join.verb(), WifiOp::JoinOnce.verb());
        assert!(WifiOp::Join.is_join() && WifiOp::JoinOnce.is_join());
        assert_eq!(wifi_op_decode("join-once\ncafe\n").unwrap(), (WifiOp::JoinOnce, "cafe", ""));
        // And "join-once" must not be read as "join" with a stray suffix.
        assert_eq!(wifi_op_decode("join-once").unwrap().0, WifiOp::JoinOnce);
        assert_eq!(wifi_op_decode("join").unwrap().0, WifiOp::Join);
    }

    #[test]
    fn a_row_says_one_thing_and_the_right_one() {
        let tkip = net("mixed", 0x0B, 6);
        let wpa2 = net("home", 0x03, 11);
        // Unjoinable beats remembered: you cannot connect to it, so nothing else about it matters.
        assert_eq!(row_status(&tkip, true), "Mixed mode");
        assert_eq!(row_status(&wpa2, true), "Remembered");
        assert_eq!(row_status(&wpa2, false), "2.4 GHz \u{00B7} ch 11");
        // The short form and the long form must agree about WHETHER there is a problem, or a row
        // would grey itself and then explain nothing when clicked.
        for n in [&tkip, &wpa2, &net("cafe", 0x00, 1), &net("old", 0x01, 1)] {
            assert_eq!(refusal(n).is_some(), refusal_short(n).is_some());
            assert_eq!(refusal(n).is_none(), joinable(n));
        }
    }

    #[test]
    fn an_empty_argument_is_not_an_operation() {
        // Load-bearing: init still spawns the agent with no argument at all, and that must mean
        // "do your original boot job", not "do nothing" and certainly not "do the first verb".
        assert_eq!(wifi_op_decode(""), None);
        assert_eq!(wifi_op_decode("connect"), None);
        assert_eq!(wifi_op_decode("SCAN"), None);
    }

    #[test]
    fn a_passphrase_may_contain_anything_a_passphrase_may_contain() {
        // Spaces, punctuation and the separator itself. The passphrase is the REST of the string,
        // so an embedded newline survives; an SSID cannot forge a verb because the verb is decided
        // before either string is read.
        for psk in ["", " ", "a b c", "p\u{00E4}ssw\u{00F6}rd", "with\nnewline", "63charsxxxxxxxx"] {
            let mut buf = [0u8; WIFI_OP_MAX];
            let n = wifi_op_encode(&mut buf, WifiOp::Join, "my ssid", psk);
            let s = core::str::from_utf8(&buf[..n]).unwrap();
            let (op, ssid, got) = wifi_op_decode(s).unwrap();
            assert_eq!(op, WifiOp::Join);
            assert_eq!(ssid, "my ssid");
            assert_eq!(got, psk, "passphrase {psk:?} did not survive");
        }
    }

    #[test]
    fn an_open_network_joins_with_no_passphrase() {
        let mut buf = [0u8; WIFI_OP_MAX];
        let n = wifi_op_encode(&mut buf, WifiOp::Join, "cafe", "");
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        assert_eq!(wifi_op_decode(s).unwrap(), (WifiOp::Join, "cafe", ""));
        // And a truncated request — one that lost its trailing separator — must still name the
        // network rather than decoding to a join of nothing.
        assert_eq!(wifi_op_decode("join\ncafe").unwrap(), (WifiOp::Join, "cafe", ""));
        assert_eq!(wifi_op_decode("join").unwrap(), (WifiOp::Join, "", ""));
    }

    #[test]
    fn the_buffer_is_big_enough_for_the_longest_legal_request() {
        let ssid = "x".repeat(32);
        let psk = "y".repeat(63);
        let mut buf = [0u8; WIFI_OP_MAX];
        let n = wifi_op_encode(&mut buf, WifiOp::Join, &ssid, &psk);
        assert!(n <= WIFI_OP_MAX);
        let s = core::str::from_utf8(&buf[..n]).unwrap();
        let (_, got_ssid, got_psk) = wifi_op_decode(s).unwrap();
        assert_eq!(got_ssid.len(), 32);
        assert_eq!(got_psk.len(), 63, "the passphrase was truncated by the buffer");
    }
}
