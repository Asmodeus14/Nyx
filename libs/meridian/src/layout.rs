//! Meridian's geometry: where every piece of the shell sits, and what is under the pointer.
//!
//! ## Why this is a library and not part of `apps/shell`
//!
//! Two reasons, and both are lessons this codebase already paid for.
//!
//!
//! ⚠️ `nyx_gui::ui` is cited by name several times below and **no longer exists** — it was
//! the old desktop's widget set and was deleted with `apps/compositor` at Meridian step 20
//! (2026-09-08). Those citations are history, not directions: they record what this module
//! replaced and why, and `git log libs/gui/src/ui.rs` is where to read the original.
//! **One source of truth.** `nyx_gui::ui` carries a comment above `ctl_btn_rect` explaining that the
//! window controls' drawn plates and clickable regions "used to be five hand-copied literal rects,
//! which is how [they] drifted apart in the first place". Every rect here is computed by exactly one
//! function, and the shell calls it for both painting and hit-testing. A control you can see but not
//! click is not a crash — it is a thing that quietly does not work.
//!
//! **It can be tested without a boot.** `apps/shell` is a `no_std` binary with a `_start`; its tests
//! cannot run on the host. Geometry moved in here runs under `cargo test`, and on a machine with no
//! QEMU that is the difference between finding an off-by-one in seconds and finding it in a power
//! cycle.
//!
//! Every constant below is transcribed from `Nyx-ui/design/ver3.0/`, and the doc comments name the
//! CSS rule each one came from so a change there can be traced to here.
//!
//! ## Units
//!
//! ★ The `const`s in this file are **design units**, measured against the design's 1440x900 scene,
//! and they are all PRIVATE. Everything public returns **real pixels**, having passed through
//! [`crate::scale::px`].
//!
//! That split is deliberate. If the constants were public, a caller could reach past the scaling and
//! place something at an unscaled 22px next to something the layout put at 28px — and it would look
//! right on the one panel that happens to be 1:1. Making the raw values unreachable means the only
//! way to get a coordinate is to ask for one, and every answer is in the same units.
//!
//! At scale 1.0 the scaling is the identity, which is why the tests below still read as the design's
//! own numbers.

use crate::scale::px as sc;
use crate::tokens::{RADIUS_SURFACE, RADIUS_WINDOW};

// ── Scaled accessors ────────────────────────────────────────────────────────
//
// The dimensions `apps/shell` needs directly — everything else it gets as a `Rect` from the layout
// functions below. Named in lower case so a call site reads as a question rather than a constant,
// which is the point: the answer depends on the panel.

/// `.w-cap { height: 22px }` — the caption line above a window's surface.
pub fn caption_h() -> i32 { sc(CAPTION_H) }
/// `.w-cap { padding: 0 2px }`
pub fn caption_pad_x() -> i32 { sc(CAPTION_PAD_X) }
/// `.w-cap { gap: 9px }` — between the name, the separator dot and the context run.
pub fn caption_gap() -> i32 { sc(CAPTION_GAP) }
/// `.w-cap .sep { width: 2px; height: 2px; border-radius: 1px }` — the dot that separates a
/// window's name from its context. Floored at 1px: at scale 1.0 it is two pixels, and rounding a
/// two-pixel mark away on some panel would silently delete the punctuation.
pub fn caption_sep() -> i32 { sc(CAPTION_SEP).max(1) }
/// The glyph inside a window control's 18px plate.
pub fn ctl_icon() -> i32 { sc(CTL_ICON) }
/// `.w-folded .tick { width: 3px }`
pub fn fold_tick() -> i32 { sc(FOLD_TICK) }

/// `.i { width: 20px }` — the dock's glyph size.
pub fn dock_icon() -> i32 { sc(DOCK_ICON) }
/// `.dock { bottom: 34px }`
pub fn dock_bottom() -> i32 { sc(DOCK_BOTTOM) }

/// `.i-22` — the Entity's mark.
pub fn entity_mark_px() -> i32 { sc(ENTITY_MARK) }
/// `.ent-s { width: 372px }`
pub fn entity_surface_w() -> i32 { sc(ENTITY_SURFACE_W) }
/// `.ent-hd { padding: … 26px }`
pub fn ent_hd_pad_x() -> i32 { sc(ENT_HD_PAD_X) }
/// `.ent-hd .dt { line-height: 18px }`
pub fn ent_detail_line() -> i32 { sc(ENT_DETAIL_LINE) }
/// `.crow { padding: 0 12px }`
pub fn ent_crow_pad_x() -> i32 { sc(ENT_CROW_PAD_X) }
/// `.ent-f { padding-top: 14px }`
pub fn ent_f_pad_top() -> i32 { sc(ENT_F_PAD_TOP) }
/// The icon in an Entity control row.
pub fn ent_crow_icon() -> i32 { sc(16) }

/// `.cmd-q { padding: 22px 26px 20px }`
pub fn cmd_q_pad_x() -> i32 { sc(CMD_Q_PAD_X) }
pub fn cmd_q_pad_top() -> i32 { sc(CMD_Q_PAD_TOP) }
/// `.cmd-q { gap: 14px }`
pub fn cmd_q_gap() -> i32 { sc(CMD_Q_GAP) }
/// The search glyph on the query line.
pub fn cmd_q_icon() -> i32 { sc(CMD_Q_ICON) }
/// Height of the whole query row, hairline included.
pub fn cmd_query_h() -> i32 { sc(CMD_QUERY_H) }
/// `.cmd-b { padding-top: 10px }`
pub fn cmd_b_pad_top() -> i32 { sc(CMD_B_PAD_TOP) }
/// `.cmd-g { padding-bottom: 8px }`
pub fn cmd_group_pad_bottom() -> i32 { sc(CMD_GROUP_PAD_BOTTOM) }
/// `.cr { padding: 0 14px }`
pub fn cmd_row_pad_x() -> i32 { sc(CMD_ROW_PAD_X) }
/// `.cr { gap: 14px }`
pub fn cmd_row_gap() -> i32 { sc(CMD_ROW_GAP) }
/// The 16px glyph on a result row.
pub fn cmd_row_icon() -> i32 { sc(CMD_ROW_ICON) }
/// `.cr.on::before { width: 2px; top: 11px; bottom: 11px }` — the accent selection bar.
pub fn cmd_sel_bar_w() -> i32 { sc(CMD_SEL_BAR_W) }
pub fn cmd_sel_bar_inset() -> i32 { sc(CMD_SEL_BAR_INSET) }
/// `.cmd-f { padding: 12px 26px }` around one line.
pub fn cmd_footer_h() -> i32 { sc(CMD_FOOTER_H) }
/// The Nyx mark in the Command's footer.
pub fn cmd_footer_mark() -> i32 { sc(CMD_FOOTER_MARK) }
/// The 11px label line height, for centring the footer hints.
pub fn cmd_footer_line() -> i32 { sc(14) }

// ── The power glyphs in the Command's footer ────────────────────────────────
//
// ★ An extension. Meridian was drawn for a desktop that is simply always on, so the design has no
// power control anywhere — but a machine has to be able to stop, and until this existed the only way
// to stop this one was to hold the power button with a live ext4 mount.
//
// They live at the right-hand end of the Command's footer, as **glyphs with no label**. That corner
// already holds the one piece of chrome in the Command that is about the session rather than about
// the query (`6 of 41 results`), the Command is what the Super key opens, and two 14px glyphs are
// the smallest thing that can be a target without becoming a row in a list of applications.
//
// Deliberately NOT rows in the result list, which is where they started: a verb that ends the
// session should not be reachable by typing three letters and hitting Return with the selection
// still on the first match.

/// `.cmd-f` power glyph, 14px like the key hints beside it.
const CMD_POWER_ICON: i32 = 14;
/// Gap between the two glyphs.
const CMD_POWER_GAP: i32 = 16;
/// Clear space between them and the result count to their left.
const CMD_POWER_LEAD: i32 = 22;

/// How many glyphs there are: shut down, restart.
pub const CMD_POWER_SLOTS: usize = 2;

pub fn cmd_power_icon() -> i32 { sc(CMD_POWER_ICON) }

/// The hit box for power glyph `i`, counted from the RIGHT edge inward — so slot 0 is the rightmost.
///
/// Padded well past the glyph itself: a 14px square is a dare, and the footer band has nothing else
/// in that corner to collide with. Draw and hit-test both go through here, which is the whole reason
/// it is in this file at all.
pub fn cmd_power_slot(screen_w: i32, body_h: i32, i: usize) -> Option<Rect> {
    if i >= CMD_POWER_SLOTS {
        return None;
    }
    let panel = command_panel(screen_w, body_h);
    let f_y = panel.bottom() - sc(CMD_FOOTER_H);
    let icon = sc(CMD_POWER_ICON);
    let step = icon + sc(CMD_POWER_GAP);
    let right = panel.right() - sc(CMD_Q_PAD_X);
    // Pad to the full footer height vertically and half the gap horizontally.
    let pad = sc(CMD_POWER_GAP) / 2;
    Some(Rect::new(
        right - (i as i32 + 1) * step + sc(CMD_POWER_GAP) - pad,
        f_y,
        icon + pad * 2,
        sc(CMD_FOOTER_H),
    ))
}

/// Where the glyph itself is drawn inside slot `i` — centred in its hit box.
pub fn cmd_power_glyph(screen_w: i32, body_h: i32, i: usize) -> Option<Rect> {
    let r = cmd_power_slot(screen_w, body_h, i)?;
    let icon = sc(CMD_POWER_ICON);
    Some(Rect::new(r.x + (r.w - icon) / 2, r.y + (r.h - icon) / 2, icon, icon))
}

/// Which power glyph is under `(mx, my)`.
pub fn cmd_power_hit(screen_w: i32, body_h: i32, mx: i32, my: i32) -> Option<usize> {
    (0..CMD_POWER_SLOTS)
        .find(|&i| cmd_power_slot(screen_w, body_h, i).map_or(false, |r| r.contains(mx, my)))
}

/// The right edge the result count must stop at, so it cannot run under the power glyphs.
pub fn cmd_footer_text_right(screen_w: i32, body_h: i32) -> i32 {
    match cmd_power_slot(screen_w, body_h, CMD_POWER_SLOTS - 1) {
        Some(r) => r.x - sc(CMD_POWER_LEAD),
        None => command_panel(screen_w, body_h).right() - sc(CMD_Q_PAD_X),
    }
}

/// A rectangle in screen pixels.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// THE DOCK  —  `.dock`, `.dk`, `.dock-gap`
// ─────────────────────────────────────────────────────────────────────────────

/// `.dock { left: 40px }`
const DOCK_LEFT: i32 = 40;
/// `.dock { bottom: 34px }` — distance from the bottom of the screen to the bottom of the glyphs.
const DOCK_BOTTOM: i32 = 34;
/// `.i { width: 20px; height: 20px }` — the dock draws icons at the base size.
const DOCK_ICON: i32 = 20;
/// `.dock { gap: 22px }`
const DOCK_GAP: i32 = 22;
/// `.dock-gap { width: 1px; height: 14px; margin: 0 4px }`
const DOCK_DIVIDER_W: i32 = 1;
const DOCK_DIVIDER_H: i32 = 14;
const DOCK_DIVIDER_MARGIN: i32 = 4;
/// `.dk.run::after { bottom: -9px; width: 3px; height: 3px }` — the running/active dot.
const DOCK_DOT: i32 = 3;
const DOCK_DOT_DROP: i32 = 9;

/// The dock's slots, in draw order. `None` is the divider before Settings — a hairline, not a glyph.
///
/// Mirrors [`crate::icons::Icons::DOCK`]; kept as a length here so the geometry does not depend on
/// the icon table, and cross-checked by a test.
pub const DOCK_SLOTS: usize = 9;

/// Box of dock slot `i` — the 20px square an icon is drawn into, or the divider's own 1×14 rect.
///
/// Returns None past the end of the dock. Slot indices match [`crate::icons::Icons::DOCK`], so slot
/// 7 is the divider and has a different width; a caller that assumes uniform spacing will place
/// Settings 21px too far left.
pub fn dock_slot(screen_h: i32, i: usize) -> Option<Rect> {
    if i >= DOCK_SLOTS {
        return None;
    }
    let bottom = screen_h - sc(DOCK_BOTTOM);
    let mut x = sc(DOCK_LEFT);
    for slot in 0..i {
        x += slot_advance(slot);
    }
    Some(if is_divider(i) {
        Rect::new(
            x + sc(DOCK_DIVIDER_MARGIN),
            bottom - sc(DOCK_ICON) + (sc(DOCK_ICON) - sc(DOCK_DIVIDER_H)) / 2,
            sc(DOCK_DIVIDER_W),
            sc(DOCK_DIVIDER_H),
        )
    } else {
        Rect::new(x, bottom - sc(DOCK_ICON), sc(DOCK_ICON), sc(DOCK_ICON))
    })
}

/// Slot 7 is the divider — the only non-glyph in the dock.
pub const fn is_divider(i: usize) -> bool {
    i == 7
}

/// How far the pen moves after slot `i`, including the gap that follows it.
fn slot_advance(i: usize) -> i32 {
    if is_divider(i) {
        sc(DOCK_DIVIDER_W) + sc(DOCK_DIVIDER_MARGIN) * 2 + sc(DOCK_GAP)
    } else {
        sc(DOCK_ICON) + sc(DOCK_GAP)
    }
}

/// The running/active dot under dock slot `i`, or None for the divider.
pub fn dock_dot(screen_h: i32, i: usize) -> Option<Rect> {
    if is_divider(i) {
        return None;
    }
    let s = dock_slot(screen_h, i)?;
    Some(Rect::new(
        s.x + (sc(DOCK_ICON) - sc(DOCK_DOT)) / 2,
        s.bottom() + sc(DOCK_DOT_DROP) - sc(DOCK_DOT),
        sc(DOCK_DOT),
        sc(DOCK_DOT),
    ))
}

/// Which dock slot is under `(mx, my)`, ignoring the divider.
///
/// The hit box is the glyph's 20px square padded to the full gap, because 20×20 targets separated by
/// 22px of dead space are unpleasant to hit and the dock has no container to aim at. The padding
/// stops halfway into the gap so two slots can never both claim a point.
pub fn dock_hit(screen_h: i32, mx: i32, my: i32) -> Option<usize> {
    let pad = sc(DOCK_GAP) / 2;
    for i in 0..DOCK_SLOTS {
        if is_divider(i) {
            continue;
        }
        let s = dock_slot(screen_h, i)?;
        let box_ = Rect::new(s.x - pad, s.y - pad, s.w + pad * 2, s.h + pad * 2);
        if box_.contains(mx, my) {
            return Some(i);
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// THE ENTITY  —  `.ent`, `.ent-s`
// ─────────────────────────────────────────────────────────────────────────────

/// `.ent { right: 40px; bottom: 32px }`
pub(crate) const ENTITY_RIGHT: i32 = 40;
const ENTITY_BOTTOM: i32 = 32;
/// `.i-22 { width: 22px; height: 22px }` — the mark.
const ENTITY_MARK: i32 = 22;
/// `.ent { gap: 12px }` between the phrase and the mark.
const ENTITY_GAP: i32 = 12;
/// `.ent-s { width: 372px }`
pub(crate) const ENTITY_SURFACE_W: i32 = 372;
/// `.ent-s { bottom: 78px }`
pub(crate) const ENTITY_SURFACE_BOTTOM: i32 = 78;

/// The 22px mark at bottom-right. This is the entire permanent chrome on that side of the screen —
/// there is no tray, because this is the tray.
pub fn entity_mark(screen_w: i32, screen_h: i32) -> Rect {
    Rect::new(
        screen_w - sc(ENTITY_RIGHT) - sc(ENTITY_MARK),
        screen_h - sc(ENTITY_BOTTOM) - sc(ENTITY_MARK),
        sc(ENTITY_MARK),
        sc(ENTITY_MARK),
    )
}

/// Where the Entity's phrase sits — right-aligned against the mark, `phrase_w` wide.
pub fn entity_phrase(screen_w: i32, screen_h: i32, phrase_w: i32) -> Rect {
    let m = entity_mark(screen_w, screen_h);
    Rect::new(m.x - sc(ENTITY_GAP) - phrase_w, m.y, phrase_w, sc(ENTITY_MARK))
}

/// The mark's hit box, padded so a 22px target is comfortable to click.
pub fn entity_hit(screen_w: i32, screen_h: i32) -> Rect {
    let m = entity_mark(screen_w, screen_h);
    Rect::new(m.x - 8, m.y - 8, m.w + 16, m.h + 16)
}

/// The disclosure surface, anchored to the mark it grew from. `content_h` is however tall the
/// statement, measurements, controls and footer came out.
pub fn entity_surface(screen_w: i32, screen_h: i32, content_h: i32) -> Rect {
    Rect::new(
        screen_w - sc(ENTITY_RIGHT) - sc(ENTITY_SURFACE_W),
        screen_h - sc(ENTITY_SURFACE_BOTTOM) - content_h,
        sc(ENTITY_SURFACE_W),
        content_h,
    )
}

// ── The Entity surface's four sections ──────────────────────────────────────
//
// `.ent-hd` a statement and its detail · `.ent-m` measurements · `.ent-c` controls · `.ent-f` the
// time. Separated by hairlines and space; it is deliberately not a card grid.
//
// The surface's height is DERIVED, not fixed, because the detail text wraps to a variable number of
// lines and the surface grows UPWARD from a fixed bottom edge. Getting that arithmetic wrong moves
// every row inside it, so it lives here with tests rather than inline in the renderer.

/// `.ent-hd { padding: 26px 26px 22px }`
pub(crate) const ENT_HD_PAD_TOP: i32 = 26;
pub(crate) const ENT_HD_PAD_X: i32 = 26;
pub(crate) const ENT_HD_PAD_BOTTOM: i32 = 22;
/// `.ent-hd .st { margin-bottom: 6px }`
pub(crate) const ENT_HD_GAP: i32 = 6;
/// `.ent-hd .st { line-height: 27px }` — the statement is one `d3` line.
pub(crate) const ENT_STATEMENT_H: i32 = 27;
/// `.ent-hd .dt { line-height: 18px }`
pub(crate) const ENT_DETAIL_LINE: i32 = 18;
/// `.ent-m { padding: 18px 26px }`
pub(crate) const ENT_M_PAD_Y: i32 = 18;
/// `.mrow { padding: 7px 0 }` around a 17px line.
pub(crate) const ENT_MROW_H: i32 = 31;
/// `.ent-c { padding: 8px 14px 12px }`
pub(crate) const ENT_C_PAD_TOP: i32 = 8;
pub(crate) const ENT_C_PAD_BOTTOM: i32 = 12;
pub(crate) const ENT_C_PAD_X: i32 = 14;
/// `.crow { height: 40px; padding: 0 12px }`
pub(crate) const ENT_CROW_H: i32 = 40;
pub(crate) const ENT_CROW_PAD_X: i32 = 12;
/// `.ent-f { padding: 14px 26px 18px }` around one 17px line.
pub(crate) const ENT_F_PAD_TOP: i32 = 14;
pub(crate) const ENT_F_PAD_BOTTOM: i32 = 18;
pub(crate) const ENT_F_LINE_H: i32 = 17;

// ── The creature block, step 18 — `.ent-cr` ──────────────────────────────────
//
// `.ent-cr { padding: 26px 0 20px; display: grid; place-items: center;
//            border-bottom: 1px solid var(--line); background: var(--sunken) }`
// `.ent-cr .tag  { left: 20px; top: 16px }`      — "NYX ENTITY", `lb`, `--fg-4`
// `.ent-cr .live { right: 20px; top: 16px }`     — a 4px accent pip + "ONLINE"

const ENT_CR_PAD_TOP: i32 = 26;
const ENT_CR_PAD_BOTTOM: i32 = 20;
const ENT_CR_LABEL_INSET: i32 = 20;
const ENT_CR_LABEL_TOP: i32 = 16;
/// `.ent-cr .live i { width: 4px; height: 4px; border-radius: 2px }`
const ENT_CR_PIP: i32 = 4;
/// `.ent-cr .live { gap: 7px }`
const ENT_CR_PIP_GAP: i32 = 7;

/// ★ The creature's size, largest first — and **0 means the block is omitted entirely**.
///
/// 96 is the design's number and the one to use whenever it fits. It does not always fit, and that
/// is not a rounding problem — it is a real conflict between the design and what ships:
///
/// **The design's own scene for step 18 has no controls block.** The shipping surface has four rows
/// (Network, Power, Thermal, Brightness) at 40px each plus padding — about 140 design units the
/// design never budgeted for, because in the document those rows live in a different scene. The
/// creature block is another 142 on top of that.
///
/// The design's note forbids the obvious saving: *"Processor, memory, graphics and thermal stay
/// exactly where they were, in the same row treatment, at the same size."* So the creature is the
/// element that flexes. `appearance::blit` takes a cell size, so 72 and 48 are the same creature at
/// 3px and 2px per cell — a genuinely smaller drawing, not a scaled bitmap.
///
/// On a screen where not even 48 fits (a 720-unit panel, once the controls and the identity rows are
/// counted) the block is **dropped** and the surface is exactly what it was before this step. A
/// creature is worth less than the statement, the measurements and the controls it would push off
/// the top of the screen — and off the top is where they would go, because this surface grows upward
/// and nothing clips it there.
const CREATURE_SIZES: [i32; 3] = [96, 72, 48];

// ── The identity rows, step 18 — `.ent-id`, `.idrow` ─────────────────────────
//
// ★ DEPARTURE, forced by the same conflict as the creature's size. The design stacks each fact as a
// label above a value with a rule between rows:
//
//   .ent-id { padding: 4px 26px 20px }
//   .idrow  { padding: 16px 0 0; border-top: 1px solid var(--line); margin-top: 16px }
//   .idrow .k { margin-bottom: 7px }
//
// That is 219 units for three facts. The measurement rows immediately above them carry the same
// amount of information in 31 units each, key left and value right — and with the controls block
// present, the tall form does not leave room for a single line of the Entity's detail text on a
// 1080-unit panel. The detail is the Entity's actual voice ("Nothing needs your attention. Last
// thermal event 6 days ago."); three rules and some air are not worth losing it.
//
// So the identity rows use the `.mrow` treatment that already exists three inches above them, which
// also reads as one continuous list of things the machine is telling you rather than as two blocks
// in different styles.

const ENT_ID_PAD_TOP: i32 = 4;
const ENT_ID_PAD_BOTTOM: i32 = 18;
/// The same 31 units as `ENT_MROW_H`, named separately so a change to one is a decision about it.
const ENT_ID_ROW_H: i32 = 31;

/// Seed, evolution, traits. *"Three facts and no statistics."* Fixed, because there is no fourth
/// fact the design admits and no way to add one without inventing a statistic.
pub const ENT_IDENTITY_ROWS: usize = 3;

/// A resolved Entity surface. Every rect is in screen coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EntityLayout {
    pub surface: Rect,
    /// The creature block at the head — `.ent-cr`, on the sunken ground with a hairline under it.
    pub creature: Rect,
    /// The statement — one `d3` line.
    pub statement: Rect,
    /// The detail paragraph, `detail_lines` lines of [`ENT_DETAIL_LINE`].
    pub detail: Rect,
    /// The measurements block, as a whole.
    pub measures: Rect,
    /// The identity rows — seed, evolution, traits.
    pub identity: Rect,
    /// The control rows, as a whole.
    pub controls: Rect,
    /// The time and date.
    pub footer: Rect,
    /// The creature's drawn size in real pixels — 96 wherever it fits. See [`CREATURE_SIZES`].
    creature_px: i32,
    n_measures: i32,
    n_controls: i32,
}

/// Clearance kept between the top of the Entity surface and the top of the screen.
pub(crate) const ENT_TOP_MARGIN: i32 = 24;

/// The most detail lines that fit on this screen.
///
/// The Entity's counterpart to [`command_visible`], and for the same reason: the surface has a fixed
/// bottom edge and grows upward, so an over-long detail runs off the TOP where nothing clips it. The
/// shell caps its wrapping with this before laying out.
///
/// Returns 0 when even a bare statement is a squeeze — the statement is the part that matters, and a
/// detail is elaboration.
pub fn entity_max_detail_lines(screen_h: i32, n_measures: usize, n_controls: usize) -> i32 {
    // Everything except the detail. `screen_w` does not affect height, so any value will do.
    let base = entity_layout(0, screen_h, 0, n_measures, n_controls).surface.h;
    let room = screen_h - sc(ENTITY_SURFACE_BOTTOM) - sc(ENT_TOP_MARGIN) - base - sc(ENT_HD_GAP);
    let per = sc(ENT_DETAIL_LINE).max(1);
    (room / per).max(0)
}

/// Lay the Entity surface out around its content.
///
/// `detail_lines` is however many lines the detail wrapped to — pass 0 for a statement with no
/// detail, which is the nominal state. Cap it with [`entity_max_detail_lines`].
pub fn entity_layout(
    screen_w: i32,
    screen_h: i32,
    detail_lines: i32,
    n_measures: usize,
    n_controls: usize,
) -> EntityLayout {
    let detail_h = if detail_lines > 0 {
        sc(ENT_HD_GAP) + detail_lines * sc(ENT_DETAIL_LINE)
    } else {
        0
    };
    let head_h = sc(ENT_HD_PAD_TOP) + sc(ENT_STATEMENT_H) + detail_h + sc(ENT_HD_PAD_BOTTOM);
    let measures_h = sc(ENT_M_PAD_Y) * 2 + n_measures as i32 * sc(ENT_MROW_H);
    let identity_h = sc(ENT_ID_PAD_TOP) + identity_rows_h() + sc(ENT_ID_PAD_BOTTOM);
    let controls_h = sc(ENT_C_PAD_TOP) + n_controls as i32 * sc(ENT_CROW_H) + sc(ENT_C_PAD_BOTTOM);
    let footer_h = sc(ENT_F_PAD_TOP) + sc(ENT_F_LINE_H) + sc(ENT_F_PAD_BOTTOM);
    let fixed_h = head_h + measures_h + identity_h + controls_h + footer_h;

    // The creature takes whatever is left over, at the largest of its three sizes that fits — or is
    // dropped entirely if none does. See `CREATURE_SIZES`.
    //
    // ⚠️ Sized against the surface WITHOUT its detail, deliberately. Sizing it against `fixed_h`
    // would make the creature shrink a step the moment the Entity had a second line to say, so the
    // picture would visibly change size in response to a sentence appearing. The detail is what
    // `entity_max_detail_lines` caps instead, against the creature this picked.
    let creature_px = creature_px_for(screen_h, n_measures, n_controls);
    let creature_h = if creature_px > 0 { creature_px + creature_chrome_h() } else { 0 };

    let mut surface = entity_surface(screen_w, screen_h, creature_h + fixed_h);
    // ★ Backstop. The surface is anchored to its BOTTOM and grows upward, so an over-long detail or
    // a large interface scale pushes it off the TOP of the screen — where nothing clips it, and the
    // statement is simply gone. Callers are expected to cap the detail with
    // [`entity_max_detail_lines`]; this is what happens if one does not. Losing the bottom-edge
    // anchor is much better than losing the thing the Entity is trying to say.
    if surface.y < sc(ENT_TOP_MARGIN) {
        surface.y = sc(ENT_TOP_MARGIN);
    }
    let inner_w = surface.w - sc(ENT_HD_PAD_X) * 2;
    // The creature block is FIRST — "a new first section, not a new window".
    let head_y = surface.y + creature_h;
    let stmt_y = head_y + sc(ENT_HD_PAD_TOP);
    let measures_y = head_y + head_h;
    // Identity sits with the measurements, above the controls: both are things the machine is
    // telling you, and a control is a thing you do.
    let identity_y = measures_y + measures_h;
    let controls_y = identity_y + identity_h;
    let footer_y = controls_y + controls_h;

    EntityLayout {
        surface,
        creature: Rect::new(surface.x, surface.y, surface.w, creature_h),
        statement: Rect::new(surface.x + sc(ENT_HD_PAD_X), stmt_y, inner_w, sc(ENT_STATEMENT_H)),
        detail: Rect::new(
            surface.x + sc(ENT_HD_PAD_X),
            stmt_y + sc(ENT_STATEMENT_H) + sc(ENT_HD_GAP),
            inner_w,
            (detail_h - sc(ENT_HD_GAP)).max(0),
        ),
        measures: Rect::new(surface.x, measures_y, surface.w, measures_h),
        identity: Rect::new(surface.x, identity_y, surface.w, identity_h),
        controls: Rect::new(surface.x, controls_y, surface.w, controls_h),
        footer: Rect::new(surface.x + sc(ENT_HD_PAD_X), footer_y, inner_w, footer_h),
        creature_px,
        n_measures: n_measures as i32,
        n_controls: n_controls as i32,
    }
}

/// The stacked height of the three identity rows.
fn identity_rows_h() -> i32 {
    ENT_IDENTITY_ROWS as i32 * sc(ENT_ID_ROW_H)
}

/// The creature block's padding and its hairline — everything but the creature itself.
fn creature_chrome_h() -> i32 {
    sc(ENT_CR_PAD_TOP) + sc(ENT_CR_PAD_BOTTOM) + 1
}

/// How big the creature is drawn on this screen, or **0 when there is no room for it at all**.
///
/// Public because `apps/shell` has to know before it generates a sprite: `appearance::blit` writes
/// `cell * 24` pixels square, and rasterizing 96px to then draw 48 would be doing the work twice and
/// throwing half of it away.
///
/// Reserves one detail line, so a machine that can only just fit the creature still has room for the
/// Entity to say something. The creature is the new thing here; the sentence is the point of the
/// surface.
pub fn creature_px_for(screen_h: i32, n_measures: usize, n_controls: usize) -> i32 {
    let bare = entity_layout_h(0, n_measures, n_controls);
    let room = screen_h
        - sc(ENTITY_SURFACE_BOTTOM)
        - sc(ENT_TOP_MARGIN)
        - bare
        - sc(ENT_HD_GAP)
        - sc(ENT_DETAIL_LINE);
    CREATURE_SIZES
        .iter()
        .map(|&p| sc(p))
        .find(|&p| p + creature_chrome_h() <= room)
        .unwrap_or(0)
}

/// The surface's height excluding the creature block, for `detail_lines` of detail.
fn entity_layout_h(detail_lines: i32, n_measures: usize, n_controls: usize) -> i32 {
    let detail_h = if detail_lines > 0 {
        sc(ENT_HD_GAP) + detail_lines * sc(ENT_DETAIL_LINE)
    } else {
        0
    };
    sc(ENT_HD_PAD_TOP)
        + sc(ENT_STATEMENT_H)
        + detail_h
        + sc(ENT_HD_PAD_BOTTOM)
        + sc(ENT_M_PAD_Y) * 2
        + n_measures as i32 * sc(ENT_MROW_H)
        + sc(ENT_ID_PAD_TOP)
        + identity_rows_h()
        + sc(ENT_ID_PAD_BOTTOM)
        + sc(ENT_C_PAD_TOP)
        + n_controls as i32 * sc(ENT_CROW_H)
        + sc(ENT_C_PAD_BOTTOM)
        + sc(ENT_F_PAD_TOP)
        + sc(ENT_F_LINE_H)
        + sc(ENT_F_PAD_BOTTOM)
}

impl EntityLayout {
    /// The creature's drawn size in real pixels. 96 wherever it fits; see [`CREATURE_SIZES`].
    pub fn creature_px(&self) -> i32 {
        self.creature_px
    }

    /// The square the creature is blitted into — `place-items: center`, horizontally centred and
    /// sitting on the block's own top padding.
    pub fn creature_box(&self) -> Rect {
        Rect::new(
            self.creature.x + (self.creature.w - self.creature_px) / 2,
            self.creature.y + sc(ENT_CR_PAD_TOP),
            self.creature_px,
            self.creature_px,
        )
    }

    /// The hairline under the creature block — `.ent-cr { border-bottom: 1px solid var(--line) }`.
    pub fn creature_rule(&self) -> Rect {
        Rect::new(self.creature.x, self.creature.bottom() - 1, self.creature.w, 1)
    }

    /// `(x, y)` for the "NYX ENTITY" tag — `.tag { left: 20px; top: 16px }`, drawn in `lb`.
    pub fn creature_tag(&self) -> (i32, i32) {
        (self.creature.x + sc(ENT_CR_LABEL_INSET), self.creature.y + sc(ENT_CR_LABEL_TOP))
    }

    /// The right edge the "ONLINE" label is aligned to, and the accent pip that precedes it.
    ///
    /// `label_w` is the measured width of the word, because the pip sits `7px` to its left and the
    /// pair is right-aligned as a unit — measuring outside and placing inside is the only way the
    /// two cannot drift apart.
    pub fn creature_live(&self, label_w: i32, label_h: i32) -> (Rect, i32, i32) {
        let right = self.creature.right() - sc(ENT_CR_LABEL_INSET);
        let text_x = right - label_w;
        let top = self.creature.y + sc(ENT_CR_LABEL_TOP);
        let pip = sc(ENT_CR_PIP).max(1);
        let pip_rect = Rect::new(
            text_x - sc(ENT_CR_PIP_GAP) - pip,
            top + (label_h - pip) / 2,
            pip,
            pip,
        );
        (pip_rect, text_x, top)
    }

    /// Identity row `i` — a key on the left and a value right-aligned in the same rect, exactly like
    /// [`measure_row`](Self::measure_row). See the note above [`ENT_ID_ROW_H`]'s constants.
    pub fn identity_row(&self, i: usize) -> Option<Rect> {
        if i >= ENT_IDENTITY_ROWS {
            return None;
        }
        Some(Rect::new(
            self.identity.x + sc(ENT_HD_PAD_X),
            self.identity.y + sc(ENT_ID_PAD_TOP) + i as i32 * sc(ENT_ID_ROW_H),
            self.identity.w - sc(ENT_HD_PAD_X) * 2,
            sc(ENT_ID_ROW_H),
        ))
    }

    /// Measurement row `i` — a key on the left, a value right-aligned in the same rect.
    pub fn measure_row(&self, i: usize) -> Option<Rect> {
        if i as i32 >= self.n_measures {
            return None;
        }
        Some(Rect::new(
            self.measures.x + sc(ENT_HD_PAD_X),
            self.measures.y + sc(ENT_M_PAD_Y) + i as i32 * sc(ENT_MROW_H),
            self.measures.w - sc(ENT_HD_PAD_X) * 2,
            sc(ENT_MROW_H),
        ))
    }

    /// Control row `i` — the full-width hover plate, inset by `.ent-c`'s own padding.
    pub fn control_row(&self, i: usize) -> Option<Rect> {
        if i as i32 >= self.n_controls {
            return None;
        }
        Some(Rect::new(
            self.controls.x + sc(ENT_C_PAD_X),
            self.controls.y + sc(ENT_C_PAD_TOP) + i as i32 * sc(ENT_CROW_H),
            self.controls.w - sc(ENT_C_PAD_X) * 2,
            sc(ENT_CROW_H),
        ))
    }

    /// Which control row is under `(mx, my)`.
    pub fn control_hit(&self, mx: i32, my: i32) -> Option<usize> {
        (0..self.n_controls as usize).find(|&i| {
            self.control_row(i).map(|r| r.contains(mx, my)).unwrap_or(false)
        })
    }

    /// The three hairlines: under the head, under the measurements, above the footer.
    pub fn rules(&self) -> [i32; 4] {
        // Not the creature block's own bottom rule — that one belongs to `.ent-cr` and is drawn with
        // it, because it separates the sunken ground from the chrome above the head rather than
        // dividing two sections of the same ground.
        [self.measures.y, self.identity.y, self.controls.y, self.footer.y]
    }

    /// The level track for a control row that has one — today only Brightness.
    ///
    /// It sits along the BOTTOM edge of the row rather than taking horizontal space beside the
    /// value, which is what lets a row carry a level without the label and the reading having to
    /// share the width with a slider. Same 2px hairline as the OSD's track, deliberately: they are
    /// the same quantity and a second visual language for it would be a second thing to learn.
    pub fn control_track(&self, i: usize) -> Option<Rect> {
        let row = self.control_row(i)?;
        Some(Rect::new(
            row.x + sc(ENT_CROW_PAD_X),
            row.bottom() - sc(ENT_CROW_TRACK_BOTTOM) - sc(OSD_TRACK_H),
            (row.w - sc(ENT_CROW_PAD_X) * 2).max(1),
            sc(OSD_TRACK_H),
        ))
    }
}

/// `.ent-c .track { bottom: 7px }` — clearance from the row's lower edge to the track.
const ENT_CROW_TRACK_BOTTOM: i32 = 7;

/// Where along a track `mx` falls, as 0..=100.
///
/// Clamped rather than rejected outside the track: a drag that runs off the end of the slider should
/// pin at the end, not stop responding, which is the difference between a control that feels
/// attached to the pointer and one that feels broken.
pub fn track_percent(track: Rect, mx: i32) -> i32 {
    if track.w <= 1 {
        return 0;
    }
    let rel = (mx - track.x).clamp(0, track.w);
    // Round to nearest rather than truncating, so the far right of the track reads 100 and the exact
    // middle reads 50 instead of 49.
    (rel * 100 + track.w / 2) / track.w
}

// ─────────────────────────────────────────────────────────────────────────────
// WINDOWS  —  `.w`, `.w-cap`, `.w-ctl`, `.w-body`, `.w-folded`
// ─────────────────────────────────────────────────────────────────────────────

/// `.w-cap { height: 22px }` — the caption line sits ABOVE the surface, not on it.
const CAPTION_H: i32 = 22;
/// `.w-cap { padding: 0 2px }`
const CAPTION_PAD_X: i32 = 2;
/// `.w-cap { gap: 9px }` between name, separator dot and context.
const CAPTION_GAP: i32 = 9;
/// `.w-cap .sep { width: 2px; height: 2px }`
const CAPTION_SEP: i32 = 2;
/// `.w-ctl div { width: 18px; height: 18px }`
const CTL_SIZE: i32 = 18;
/// `.w-ctl { gap: 3px }`
const CTL_GAP: i32 = 3;
/// The icon drawn inside a control plate — `.i-12`.
const CTL_ICON: i32 = 12;

/// The two window controls. **Two, not three**: maximise is a double-click on the caption line, a
/// gesture the surface already affords, so it needs no glyph.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Control {
    /// Collapse to the caption line, in place. Replaces minimise.
    Fold,
    Close,
}

/// Controls in draw order, left to right.
pub const CONTROLS: [Control; 2] = [Control::Fold, Control::Close];

/// The caption line for a window whose SURFACE is at `(x, y, w, _)`.
///
/// Note `y` here is the top of the *surface*; the caption sits above it, so this returns a negative
/// offset from that point. A window at y=0 would have its caption off-screen, which is why the shell
/// clamps window placement to leave room.
pub fn caption(surface: Rect) -> Rect {
    Rect::new(surface.x, surface.y - sc(CAPTION_H), surface.w, sc(CAPTION_H))
}

/// The full footprint of a window: caption line plus surface. This is what the shell dirties, and
/// what a shadow and an occlusion test have to cover.
pub fn window_footprint(surface: Rect) -> Rect {
    Rect::new(
        surface.x,
        surface.y - sc(CAPTION_H),
        surface.w,
        surface.h + sc(CAPTION_H),
    )
}

/// Rect of control `i` (0 = fold, 1 = close), right-aligned on the caption line.
///
/// The controls have `opacity: 0` until the window is hovered or focused — but they are still
/// *there*, and this returns their rect regardless. Hit-testing an invisible control is correct:
/// the pointer being on the caption is what makes them visible in the first place.
pub fn control(surface: Rect, i: usize) -> Option<Rect> {
    if i >= CONTROLS.len() {
        return None;
    }
    let cap = caption(surface);
    // Right-aligned: the last control ends at the caption's right padding.
    let from_right = (CONTROLS.len() - 1 - i) as i32 * (sc(CTL_SIZE) + sc(CTL_GAP));
    let x = cap.right() - sc(CAPTION_PAD_X) - sc(CTL_SIZE) - from_right;
    let y = cap.y + (sc(CAPTION_H) - sc(CTL_SIZE)) / 2;
    Some(Rect::new(x, y, sc(CTL_SIZE), sc(CTL_SIZE)))
}

/// Which control is under `(mx, my)`, if any.
pub fn control_hit(surface: Rect, mx: i32, my: i32) -> Option<Control> {
    for (i, c) in CONTROLS.iter().enumerate() {
        if control(surface, i)?.contains(mx, my) {
            return Some(*c);
        }
    }
    None
}

/// The draggable part of the caption line: everything left of the controls.
///
/// Excluding the controls matters — a drag that starts on the close button and moves one pixel would
/// otherwise both close the window and start dragging it.
pub fn caption_drag_region(surface: Rect) -> Rect {
    let cap = caption(surface);
    let controls_w = CONTROLS.len() as i32 * (sc(CTL_SIZE) + sc(CTL_GAP)) + sc(CAPTION_PAD_X);
    Rect::new(cap.x, cap.y, (cap.w - controls_w).max(0), cap.h)
}

/// A folded window: the surface is gone, the caption line stays where it was.
///
/// `.w-folded .w-cap { padding-bottom: 7px; border-bottom: 1px solid }` — the caption plus its
/// padding and the hairline under it. Zero quads: one hairline and a text run.
const FOLD_PAD_BOTTOM: i32 = 7;
/// `.w-folded .tick { width: 3px; height: 3px }` — the accent tick that marks a folded window.
const FOLD_TICK: i32 = 3;

/// The footprint a folded window occupies. It keeps its position, so the window is still *somewhere*
/// rather than nowhere — which is the whole argument for fold over minimise, and why no taskbar is
/// needed.
pub fn folded_footprint(surface: Rect) -> Rect {
    Rect::new(
        surface.x,
        surface.y - sc(CAPTION_H),
        surface.w,
        sc(CAPTION_H) + sc(FOLD_PAD_BOTTOM) + 1,
    )
}

/// The hairline under a folded caption.
pub fn folded_rule(surface: Rect) -> Rect {
    let f = folded_footprint(surface);
    Rect::new(f.x, f.bottom() - 1, f.w, 1)
}

/// Window surface corner radius.
pub const fn window_radius() -> usize {
    RADIUS_WINDOW
}

/// Floating surface corner radius (the Command, the Entity surface).
pub const fn surface_radius() -> usize {
    RADIUS_SURFACE
}

// ─────────────────────────────────────────────────────────────────────────────
// THE COMMAND  —  `.cmd`
// ─────────────────────────────────────────────────────────────────────────────

/// `.cmd { width: 620px }`
const CMD_W: i32 = 620;
/// `.cmd { top: 132px }`
const CMD_TOP: i32 = 132;
/// `.cmd-q { padding: 22px 26px 20px }`
const CMD_Q_PAD_TOP: i32 = 22;
const CMD_Q_PAD_X: i32 = 26;
const CMD_Q_PAD_BOTTOM: i32 = 20;
/// `.cmd-q { gap: 14px }` between the search icon, the query and the caret.
const CMD_Q_GAP: i32 = 14;
/// `.cmd-b { padding: 10px 12px 12px }`
const CMD_B_PAD_TOP: i32 = 10;
const CMD_B_PAD_X: i32 = 12;
const CMD_B_PAD_BOTTOM: i32 = 12;
/// `.cmd-g { padding: 14px 14px 8px }` — a group heading.
const CMD_GROUP_PAD_TOP: i32 = 14;
const CMD_GROUP_PAD_BOTTOM: i32 = 8;
/// `.cmd-g:first-child { padding-top: 6px }`
const CMD_GROUP_PAD_TOP_FIRST: i32 = 6;
/// `.cr { height: 42px; padding: 0 14px; gap: 14px }`
const CMD_ROW_H: i32 = 42;
const CMD_ROW_PAD_X: i32 = 14;
const CMD_ROW_GAP: i32 = 14;
/// `.cr.on::before { top: 11px; bottom: 11px; width: 2px }` — the accent selection bar.
const CMD_SEL_BAR_W: i32 = 2;
const CMD_SEL_BAR_INSET: i32 = 11;
/// `.cmd-f { padding: 12px 26px }`
const CMD_FOOTER_PAD_Y: i32 = 12;
/// The footer's own height: two 12px padding bands around a 14px line.
const CMD_FOOTER_H: i32 = CMD_FOOTER_PAD_Y * 2 + 14;
/// The query line's height: 20px type on a 27px line box, plus its padding.
const CMD_QUERY_H: i32 = CMD_Q_PAD_TOP + 27 + CMD_Q_PAD_BOTTOM;
/// The icon in a result row — `.i-16`.
const CMD_ROW_ICON: i32 = 16;
/// The search glyph on the query line — `.i`, the base 20px.
const CMD_Q_ICON: i32 = 20;
/// The Nyx mark in the footer, at 12px.
const CMD_FOOTER_MARK: i32 = 12;

/// One entry in the Command's result list: either a group heading or a result row.
///
/// The list is modelled as a flat sequence because that is how it is both drawn and navigated —
/// arrow keys step over headings, but the heading still occupies vertical space, and computing that
/// twice is how a selection highlight ends up on the wrong row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmdEntry {
    /// An 11px uppercase label. Results are grouped by these and separated by space, not rules.
    Group,
    Row,
}

/// The Command panel itself, centred horizontally. `body_h` is the measured height of the result
/// list; the panel is the query line, the list and the footer.
pub fn command_panel(screen_w: i32, body_h: i32) -> Rect {
    Rect::new(
        (screen_w - sc(CMD_W)) / 2,
        sc(CMD_TOP),
        sc(CMD_W),
        sc(CMD_QUERY_H) + body_h + sc(CMD_FOOTER_H),
    )
}

/// The query line — the top band of the panel, above the hairline.
///
/// Typing is the interaction, so this is the one region of the shell that is genuinely editable
/// text, and therefore the one place the shell can put the text cursor without an app telling it
/// where its fields are. Derived from `command_panel` rather than recomputed, for the usual reason:
/// a hit region and the thing it is a region *of* drift apart the moment they are two expressions.
pub fn command_query(screen_w: i32, body_h: i32) -> Rect {
    let p = command_panel(screen_w, body_h);
    Rect::new(p.x, p.y, p.w, sc(CMD_QUERY_H))
}

/// Height of a result list containing `entries`.
pub fn command_body_height(entries: &[CmdEntry]) -> i32 {
    let mut h = sc(CMD_B_PAD_TOP);
    for (i, e) in entries.iter().enumerate() {
        h += entry_height(*e, i == 0);
    }
    h + sc(CMD_B_PAD_BOTTOM)
}

fn entry_height(e: CmdEntry, first: bool) -> i32 {
    match e {
        CmdEntry::Group => {
            let top = if first { sc(CMD_GROUP_PAD_TOP_FIRST) } else { sc(CMD_GROUP_PAD_TOP) };
            top + 14 + sc(CMD_GROUP_PAD_BOTTOM)
        }
        CmdEntry::Row => sc(CMD_ROW_H),
    }
}

/// Rect of entry `i` within the Command, in screen coordinates.
pub fn command_entry(screen_w: i32, entries: &[CmdEntry], i: usize) -> Option<Rect> {
    if i >= entries.len() {
        return None;
    }
    let panel = command_panel(screen_w, command_body_height(entries));
    let mut y = panel.y + sc(CMD_QUERY_H) + sc(CMD_B_PAD_TOP);
    for k in 0..i {
        y += entry_height(entries[k], k == 0);
    }
    Some(Rect::new(
        panel.x + sc(CMD_B_PAD_X),
        y,
        panel.w - sc(CMD_B_PAD_X) * 2,
        entry_height(entries[i], i == 0),
    ))
}

/// Clearance kept between the bottom of the Command and the bottom of the screen, so the panel
/// never reaches the dock or the Entity.
const CMD_BOTTOM_MARGIN: i32 = 48;

/// How many leading entries of `entries` fit on a `screen_h`-tall screen.
///
/// The Command's result list is **capped, not scrolled**. Its footer reads `6 of 41 results` in the
/// design, which says exactly this: a few results are shown and the total is reported. A scrolling
/// launcher would need a scrollbar, a thumb and a wheel path, and would undermine the premise that
/// you refine by typing rather than by hunting.
///
/// The returned count is trimmed so the list never *ends* on a group heading — a label with no
/// results under it reads as a rendering fault.
pub fn command_visible(screen_h: i32, entries: &[CmdEntry]) -> usize {
    let budget = screen_h - sc(CMD_TOP) - sc(CMD_BOTTOM_MARGIN) - sc(CMD_QUERY_H) - sc(CMD_FOOTER_H)
        - sc(CMD_B_PAD_TOP) - sc(CMD_B_PAD_BOTTOM);
    let mut used = 0;
    let mut n = 0usize;
    for (i, e) in entries.iter().enumerate() {
        let h = entry_height(*e, i == 0);
        if used + h > budget {
            break;
        }
        used += h;
        n += 1;
    }
    // Never end on a heading.
    while n > 0 && entries[n - 1] == CmdEntry::Group {
        n -= 1;
    }
    n
}

/// Which entry is under `(mx, my)` — headings excluded, since they are not selectable.
pub fn command_hit(screen_w: i32, entries: &[CmdEntry], mx: i32, my: i32) -> Option<usize> {
    for (i, e) in entries.iter().enumerate() {
        if *e == CmdEntry::Group {
            continue;
        }
        if command_entry(screen_w, entries, i)?.contains(mx, my) {
            return Some(i);
        }
    }
    None
}

/// The next selectable row after `from`, stepping by `delta` (+1 down, -1 up).
///
/// Skips group headings, which is what makes ↑↓ feel right: the selection moves between results, not
/// between rows of a list that happens to contain labels. Stops at the ends rather than wrapping.
pub fn command_step(entries: &[CmdEntry], from: Option<usize>, delta: i32) -> Option<usize> {
    let n = entries.len() as i32;
    if n == 0 {
        return None;
    }
    let mut i = match from {
        Some(f) => f as i32 + delta,
        None if delta > 0 => 0,
        None => n - 1,
    };
    while i >= 0 && i < n {
        if entries[i as usize] == CmdEntry::Row {
            return Some(i as usize);
        }
        i += delta;
    }
    from
}

/// The first selectable row, or None if the result list is all headings or empty.
pub fn command_first(entries: &[CmdEntry]) -> Option<usize> {
    command_step(entries, None, 1)
}

// ─────────────────────────────────────────────────────────────────────────────
// THE SCROLLBAR
// ─────────────────────────────────────────────────────────────────────────────
//
// ★ NOT IN THE DESIGN DOCUMENT. `ver3.0` never draws a scrollbar — every scene fits — so this is
// an extension, written in the design's idiom the way `u-bright` was. The rules it follows:
//
//   * No container. A track box would be a card, and Meridian has no cards; the track is one
//     translucent hairline token and the thumb is a rounded bar, so what you see is a mark on the
//     ground rather than a widget stuck to the edge.
//   * It is INSET, not flush. The design's windows are rounded surfaces with a hairline edge, and
//     something touching that edge reads as breaking out of the window.
//   * The tones are the standard tiers: `fg_4` at rest, `fg_3` under the pointer.
//
// The shipping bar before this was `nyx_gui::ui::draw_scrollbar` — 12px wide, square, flush against
// the surface's right edge, drawn in the app palette. It was left alone deliberately while the app
// interiors were unported (steps 10-16), and step 10 is where it stops being someone else's problem.

/// The thumb's width. Narrow: it is a position indicator first and a grab target second, and
/// [`scrollbar_hit`] widens the grab region well past the ink.
const SB_THUMB_W: i32 = 4;
/// How far the thumb's OUTER edge sits from the surface's right edge.
///
/// Now a taste value, which it was not before: it used to be stacked on top of `resize_band()` to
/// keep the bar out of the resizer's way, which put it ~14px from the wall at this laptop's scale
/// and made the gap read as a layout error. The scrollbar owns the edge now — see
/// [`scrollbar_track`] — so this is just the breathing room between the bar and the window wall.
const SB_EDGE_GAP: i32 = 3;
/// Top and bottom inset, so the track clears the window's 6px rounded corners instead of running
/// into them.
const SB_PAD_Y: i32 = 8;
/// Shortest the thumb may get. A 4px-wide, 6px-tall thumb on a very long list is not a target.
const SB_MIN_THUMB: i32 = 28;
/// How far to the LEFT of the thumb still counts as grabbing it.
///
/// Asymmetric, because the thumb is now near the wall and there is nothing to the right of it to
/// claim. 8 + the 4px thumb gives a 12px target, the same as the old flush bar.
const SB_GRAB_PAD: i32 = 8;

/// Grab band just inside a surface's edge where a drag starts a resize.
///
/// Mirrors `apps/shell::resize_band`. Duplicated here on purpose: the scrollbar's relationship to it
/// (it wins the middle of the right edge, and yields the corners) has to be checkable from the crate
/// that owns the geometry rather than living only in a comment in the binary.
pub fn resize_band() -> i32 {
    sc(8)
}

/// The scrollbar's track inside a window's SURFACE rect.
///
/// ⚠️⚠️ **`apps/shell` tests the scrollbar BEFORE the resize edge**, and that ordering is what lets
/// the bar sit here. It was originally the other way round, which meant anything drawn inside
/// `resize_band()` belonged to the resizer — the pointer turned into a resize arrow and a drag
/// resized the window instead of scrolling it. The first fix was to inset the bar past the band
/// entirely, but that left the thumb floating `resize_band() + 3` from the wall: at this laptop's
/// scale roughly fourteen pixels of dead air beside a four-pixel bar, which reads as a mistake.
///
/// So the bar now takes the right edge and the resizer yields to it — the standard arrangement, and
/// the one a user expects when a window is visibly scrollable. The price is paid in
/// [`scrollbar_hit`], not here: the top and bottom [`scrollbar_corner`] of the edge stay with the
/// resizer so a window can always be resized diagonally, and a window with nothing to scroll never
/// claims the edge at all.
pub fn scrollbar_track(surface: Rect) -> Rect {
    let w = sc(SB_THUMB_W);
    let right = surface.right() - sc(SB_EDGE_GAP);
    let pad = sc(SB_PAD_Y);
    Rect::new(right - w, surface.y + pad, w, (surface.h - 2 * pad).max(0))
}

/// How much of a surface's right edge the scrollbar occupies, measured from the edge inward.
///
/// What an app must keep a right-aligned column clear of, so a metadata column does not run under
/// the bar.
pub fn scrollbar_reserve() -> i32 {
    sc(SB_EDGE_GAP) + sc(SB_THUMB_W) + sc(SB_GRAB_PAD)
}

/// How much of the top and bottom of the right edge stays with the **resizer** rather than the
/// scrollbar.
///
/// The scrollbar wins the right edge (see [`scrollbar_track`]), but it must not win all of it: with
/// no exception the top-right and bottom-right corners would stop resizing, and the corners are the
/// only place a window can be resized in two axes at once. Generous on purpose — a corner is aimed
/// at, and this is the affordance being protected.
pub fn scrollbar_corner() -> i32 {
    sc(24)
}

/// The thumb inside `track`, for a viewport of `viewport` px showing `content` px at `scroll`.
///
/// Returns None when everything fits — there is no bar to draw and nothing to grab. One function
/// for the draw and the hit-test, the `ctl_btn_rect` rule: a thumb you can see one pixel away from
/// where you can grab it is the most irritating class of bug this module exists to prevent.
pub fn scrollbar_thumb(track: Rect, content: i32, viewport: i32, scroll: i32) -> Option<Rect> {
    if content <= viewport || track.h <= 0 || viewport <= 0 {
        return None;
    }
    let min = sc(SB_MIN_THUMB).min(track.h);
    let h = ((viewport * track.h) / content).clamp(min, track.h);
    let max_scroll = content - viewport;
    let off = scroll.clamp(0, max_scroll);
    let y = if max_scroll == 0 { 0 } else { (track.h - h) * off / max_scroll };
    Some(Rect::new(track.x, track.y + y, track.w, h))
}

/// True if `(mx, my)` is on the scrollbar — anywhere down the track, not just on the thumb, so a
/// click above or below it jumps there.
///
/// This is tested **before** the resize edge, so returning true here takes the pixel away from the
/// resizer. Three things keep that honest:
///
/// - It is false when there is nothing to scroll, so an unscrollable window keeps its whole edge.
/// - It reserves [`scrollbar_corner`] at the top and bottom, so both right-hand corners still
///   resize.
/// - The grab region is padded to the LEFT of the 4px ink only. A 4px pointer target is a dare;
///   padding right would run off the surface, and there is nothing out there to claim.
pub fn scrollbar_hit(surface: Rect, content: i32, viewport: i32, mx: i32, my: i32) -> bool {
    if content <= viewport {
        return false;
    }
    let t = scrollbar_track(surface);
    let corner = scrollbar_corner();
    // A window too short to hold two corners plus a usable strip between them is all corner: the
    // resizer keeps the edge and the bar is drawn but not grabbable. Scrolling still works from the
    // wheel, and a 40px-tall window is not something anyone drags a thumb inside.
    let (top, bottom) = (surface.y + corner, surface.bottom() - corner);
    if bottom <= top {
        return false;
    }
    mx >= t.x - sc(SB_GRAB_PAD) && mx <= surface.right() && my >= top && my < bottom
}

/// The scroll offset a click/drag at `my` means: centre the thumb on the pointer.
pub fn scroll_from_mouse(surface: Rect, content: i32, viewport: i32, my: i32) -> i32 {
    let t = scrollbar_track(surface);
    let Some(thumb) = scrollbar_thumb(t, content, viewport, 0) else {
        return 0;
    };
    let range = t.h - thumb.h;
    if range <= 0 {
        return 0;
    }
    let rel = (my - t.y - thumb.h / 2).clamp(0, range);
    rel * (content - viewport) / range
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::Icons;

    const SW: i32 = 1440;
    const SH: i32 = 900;

    // ---- the Command footer's power glyphs ----

    #[test]
    fn the_power_glyphs_sit_in_the_footer_and_hit_where_they_draw() {
        let body = 300;
        let panel = command_panel(SW, body);
        let f_y = panel.bottom() - cmd_footer_h();
        let mut seen: alloc::vec::Vec<Rect> = alloc::vec::Vec::new();
        for i in 0..CMD_POWER_SLOTS {
            let slot = cmd_power_slot(SW, body, i).unwrap();
            let glyph = cmd_power_glyph(SW, body, i).unwrap();
            // Inside the footer band, inside the panel.
            assert!(slot.y >= f_y && slot.bottom() <= panel.bottom(), "slot {i} left the footer");
            assert!(slot.right() <= panel.right(), "slot {i} ran off the panel");
            // The glyph is inside its own hit box. A target that does not contain what it looks
            // like is precisely the `ctl_btn_rect` drift this module exists to prevent.
            assert!(glyph.x >= slot.x && glyph.right() <= slot.right(), "glyph {i} escaped its box");
            assert!(glyph.y >= slot.y && glyph.bottom() <= slot.bottom(), "glyph {i} escaped its box");
            // Hit-testing agrees, at both corners inside the box.
            assert_eq!(cmd_power_hit(SW, body, slot.x + 1, slot.y + 1), Some(i));
            assert_eq!(cmd_power_hit(SW, body, slot.right() - 1, slot.bottom() - 1), Some(i));
            for other in &seen {
                assert!(slot.right() <= other.x || slot.x >= other.right(), "slots overlap");
            }
            seen.push(slot);
        }
        assert_eq!(cmd_power_slot(SW, body, CMD_POWER_SLOTS), None);
        // Slot 0 is the RIGHTMOST — it is counted from the right edge inward.
        assert!(seen[0].x > seen[1].x, "slot 0 should be the rightmost");
        // And nothing outside them is a hit.
        assert_eq!(cmd_power_hit(SW, body, panel.x + 4, f_y + 4), None);
        assert_eq!(cmd_power_hit(SW, body, seen[0].right() + 8, f_y + 4), None);
        assert_eq!(cmd_power_hit(SW, body, seen[0].x + 2, f_y - 20), None);
    }

    /// The result count shares the footer with the glyphs and must not run under them. It is
    /// right-aligned, so without a hard limit it grows straight into that corner.
    #[test]
    fn the_footer_count_stops_clear_of_the_power_glyphs() {
        for w in [1280, 1440, 1920, 2560] {
            let right = cmd_footer_text_right(w, 300);
            let last = cmd_power_slot(w, 300, CMD_POWER_SLOTS - 1).unwrap();
            assert!(right < last.x, "{w}px: the count would overlap the glyphs");
            assert!(right > command_panel(w, 300).x, "{w}px: no room left for the count");
        }
    }

    // ---- dock ----

    /// The design's central claim about the dock is a count: "eight glyphs and one mark". The
    /// geometry has to agree with the icon table about which slot is the divider.
    #[test]
    fn the_dock_geometry_matches_the_icon_table() {
        assert_eq!(DOCK_SLOTS, Icons::DOCK.len());
        for i in 0..DOCK_SLOTS {
            assert_eq!(
                is_divider(i),
                Icons::DOCK[i].is_none(),
                "slot {} disagrees about being the divider",
                i
            );
        }
    }

    #[test]
    fn the_dock_starts_at_the_design_offsets() {
        let first = dock_slot(SH, 0).unwrap();
        assert_eq!(first.x, DOCK_LEFT);
        assert_eq!(first.bottom(), SH - DOCK_BOTTOM);
        assert_eq!(first.w, DOCK_ICON);
    }

    /// Slots must not overlap and must run left to right.
    #[test]
    fn dock_slots_are_ordered_and_disjoint() {
        let mut prev = dock_slot(SH, 0).unwrap();
        for i in 1..DOCK_SLOTS {
            let s = dock_slot(SH, i).unwrap();
            assert!(s.x >= prev.right(), "slot {} overlaps slot {}", i, i - 1);
            prev = s;
        }
        assert!(dock_slot(SH, DOCK_SLOTS).is_none());
    }

    /// The divider is narrower than a glyph, so uniform spacing would put Settings in the wrong
    /// place. This pins the fact that the advance is per-slot.
    #[test]
    fn the_divider_is_narrower_than_a_glyph_slot() {
        let d = dock_slot(SH, 7).unwrap();
        assert_eq!(d.w, DOCK_DIVIDER_W);
        assert_eq!(d.h, DOCK_DIVIDER_H);
        let settings = dock_slot(SH, 8).unwrap();
        let entity = dock_slot(SH, 6).unwrap();
        let uniform = entity.x + DOCK_ICON + DOCK_GAP + DOCK_ICON + DOCK_GAP;
        assert_ne!(settings.x, uniform, "settings placed as if the divider were a full slot");
    }

    /// Every glyph slot must be clickable, the divider must not be, and no point may be claimed by
    /// two slots.
    #[test]
    fn dock_hit_covers_every_glyph_and_never_two_at_once() {
        for i in 0..DOCK_SLOTS {
            let s = dock_slot(SH, i).unwrap();
            let (cx, cy) = (s.x + s.w / 2, s.y + s.h / 2);
            if is_divider(i) {
                assert_ne!(dock_hit(SH, cx, cy), Some(i), "the divider must not be clickable");
            } else {
                assert_eq!(dock_hit(SH, cx, cy), Some(i), "slot {} centre missed", i);
            }
        }
        // Sweep the dock's whole span; every point resolves to at most one slot, which `dock_hit`
        // guarantees by returning the first match — so check the boxes themselves are disjoint.
        let pad = DOCK_GAP / 2;
        let boxes: alloc::vec::Vec<Rect> = (0..DOCK_SLOTS)
            .filter(|i| !is_divider(*i))
            .map(|i| {
                let s = dock_slot(SH, i).unwrap();
                Rect::new(s.x - pad, s.y - pad, s.w + pad * 2, s.h + pad * 2)
            })
            .collect();
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                assert!(a.right() <= b.x || b.right() <= a.x, "hit boxes overlap: {:?} {:?}", a, b);
            }
        }
    }

    #[test]
    fn the_dock_is_clear_of_the_screen_edges() {
        let last = dock_slot(SH, DOCK_SLOTS - 1).unwrap();
        assert!(last.right() < SW / 2, "the dock should not reach the middle of the screen");
        assert!(dock_dot(SH, 0).unwrap().bottom() < SH, "the running dot falls off the screen");
    }

    // ---- entity ----

    #[test]
    fn the_entity_mark_sits_bottom_right() {
        let m = entity_mark(SW, SH);
        assert_eq!(m.right(), SW - ENTITY_RIGHT);
        assert_eq!(m.bottom(), SH - ENTITY_BOTTOM);
        assert_eq!(m.w, ENTITY_MARK);
    }

    /// The phrase sits to the LEFT of the mark. Getting this backwards would put it off-screen.
    #[test]
    fn the_entity_phrase_is_left_of_the_mark() {
        let m = entity_mark(SW, SH);
        let p = entity_phrase(SW, SH, 96);
        assert_eq!(p.right() + ENTITY_GAP, m.x);
        assert!(p.x > 0);
    }

    /// The dock and the Entity are the only permanent chrome, at opposite corners. They must never
    /// collide, even on a narrow screen.
    #[test]
    fn the_dock_and_the_entity_never_overlap() {
        for w in [1024, 1280, 1440, 1920] {
            let dock_end = dock_slot(SH, DOCK_SLOTS - 1).unwrap().right();
            let ent = entity_phrase(w, SH, 140);
            assert!(dock_end < ent.x, "dock reaches the Entity at {}px wide", w);
        }
    }

    #[test]
    fn the_entity_surface_is_anchored_above_the_mark() {
        let s = entity_surface(SW, SH, 420);
        let m = entity_mark(SW, SH);
        assert_eq!(s.right(), m.right(), "surface should align to the mark's right edge");
        assert!(s.bottom() < m.y, "the surface must sit above the mark it grew from");
        assert_eq!(s.w, ENTITY_SURFACE_W);
    }

    // ---- the Entity surface's sections ----

    /// Every section must tile the surface exactly — no gap, no overlap. A one-pixel error here
    /// puts a hairline on top of a row of text.
    #[test]
    fn the_sections_tile_the_surface_exactly() {
        for lines in 0..4 {
            let l = entity_layout(SW, SH, lines, 4, 3);
            // The creature block is FIRST, at the very top of the surface — "a new first section".
            assert_eq!(l.creature.y, l.surface.y);
            let head_y = l.creature.bottom();
            assert!(l.measures.y > head_y, "head has no height at {} detail lines", lines);
            assert_eq!(l.identity.y, l.measures.bottom(), "gap between measures and identity");
            assert_eq!(l.controls.y, l.identity.bottom(), "gap between identity and controls");
            assert_eq!(l.footer.y, l.controls.bottom(), "gap between controls and footer");
            assert_eq!(
                l.footer.bottom(),
                l.surface.bottom(),
                "the footer must end exactly at the surface's bottom edge"
            );
        }
    }

    /// The creature block's own furniture: the creature centred on the block's top padding, the
    /// hairline on its last row, and the two labels inset 20px at 16px down.
    #[test]
    fn the_creature_block_matches_the_design_metrics() {
        scale::force_for_test(1000);
        let l = entity_layout(SW, SH, 0, 4, 3);
        assert!(l.creature_px() > 0, "the fixture assumes the creature fits on {}", SH);

        let b = l.creature_box();
        assert_eq!((b.w, b.h), (l.creature_px(), l.creature_px()), "the box is not square");
        assert_eq!(b.y, l.creature.y + 26, ".ent-cr padding-top");
        // `place-items: center`, to the odd pixel integer centring cannot split.
        assert!(((b.x - l.creature.x) - (l.creature.right() - b.right())).abs() <= 1);
        // 20px of padding below it, then the rule on the block's last row.
        assert_eq!(l.creature.bottom() - b.bottom(), 20 + 1);
        assert_eq!(l.creature_rule().y, l.creature.bottom() - 1);
        assert_eq!(l.creature_rule().h, 1);

        assert_eq!(l.creature_tag(), (l.creature.x + 20, l.creature.y + 16));
        let (pip, text_x, top) = l.creature_live(40, 14);
        assert_eq!(text_x + 40, l.creature.right() - 20, "the label is not inset 20px");
        assert_eq!(pip.right() + 7, text_x, ".live gap");
        assert_eq!((pip.w, pip.h), (4, 4));
        assert_eq!(top, l.creature.y + 16);
    }

    /// ★ The creature is sized from the screen ALONE, not from how much the Entity currently has to
    /// say. Otherwise the picture visibly changes size the moment a sentence appears underneath it.
    #[test]
    fn the_creature_does_not_resize_when_the_entity_speaks() {
        for &f in &[1000, 1250, 1500, 2000] {
            scale::force_for_test(f);
            let quiet = entity_layout(SW, SH, 0, 4, 3).creature_px();
            for lines in 1..4 {
                assert_eq!(
                    entity_layout(SW, SH, lines, 4, 3).creature_px(),
                    quiet,
                    "the creature resized at {} detail lines, scale {}",
                    lines, f
                );
            }
        }
        scale::force_for_test(1000);
    }

    /// ★ On a screen with no room for even the smallest creature the block is DROPPED, and the
    /// surface is exactly what it was before this step. Growing off the top of the screen — where
    /// nothing clips it — would cost the statement, the measurements and the controls to show a
    /// picture.
    #[test]
    fn a_screen_with_no_room_drops_the_creature_block_rather_than_overflowing() {
        scale::force_for_test(1000);
        // Deliberately absurd, to reach the case a real panel might one day reach at a large scale.
        let cramped = entity_layout(SW, 420, 0, 4, 3);
        assert_eq!(cramped.creature_px(), 0, "a 420px screen cannot hold a creature");
        assert_eq!(cramped.creature.h, 0, "a dropped block must take no space");
        assert_eq!(cramped.creature.y, cramped.surface.y);
        // ...and everything else still tiles.
        assert_eq!(cramped.identity.y, cramped.measures.bottom());
        assert_eq!(cramped.footer.bottom(), cramped.surface.bottom());
    }

    /// The identity rows stack inside their block without escaping it, in the same 31px rhythm as
    /// the measurements they sit under.
    #[test]
    fn identity_rows_stack_inside_their_block() {
        scale::force_for_test(1000);
        let l = entity_layout(SW, SH, 1, 4, 3);
        for i in 0..ENT_IDENTITY_ROWS {
            let r = l.identity_row(i).expect("identity row");
            assert!(r.y >= l.identity.y && r.bottom() <= l.identity.bottom(), "row {} escapes", i);
            assert_eq!(r.h, ENT_MROW_H, "identity and measurement rows must share a rhythm");
            if i > 0 {
                assert_eq!(r.y, l.identity_row(i - 1).unwrap().bottom(), "row {} overlaps", i);
            }
            // Same inset as the measurement rows, or the two lists visibly step in and out.
            assert_eq!(r.x, l.measure_row(0).unwrap().x);
            assert_eq!(r.w, l.measure_row(0).unwrap().w);
        }
        assert_eq!(l.identity_row(ENT_IDENTITY_ROWS), None);
    }

    /// The surface grows UPWARD: its bottom edge is fixed relative to the mark, so more detail text
    /// must raise the top rather than push the footer off the screen.
    #[test]
    fn more_detail_grows_the_surface_upward() {
        let short = entity_layout(SW, SH, 1, 4, 3);
        let tall = entity_layout(SW, SH, 3, 4, 3);
        assert_eq!(
            short.surface.bottom(),
            tall.surface.bottom(),
            "the bottom edge is anchored and must not move"
        );
        assert!(tall.surface.y < short.surface.y, "the extra lines must extend upward");
        assert_eq!(tall.surface.h - short.surface.h, 2 * ENT_DETAIL_LINE);
    }

    /// The nominal state has a statement and no detail. That must cost no vertical space at all —
    /// not an empty 18px line, and certainly not the 6px gap that would have preceded it.
    #[test]
    fn no_detail_costs_no_space() {
        let none = entity_layout(SW, SH, 0, 4, 3);
        let one = entity_layout(SW, SH, 1, 4, 3);
        assert_eq!(none.detail.h, 0);
        assert_eq!(one.surface.h - none.surface.h, ENT_HD_GAP + ENT_DETAIL_LINE);
    }

    #[test]
    fn rows_stack_inside_their_sections_without_overlapping() {
        let l = entity_layout(SW, SH, 2, 4, 3);
        for i in 0..4 {
            let r = l.measure_row(i).expect("measure row");
            assert!(r.y >= l.measures.y && r.bottom() <= l.measures.bottom(), "row {} escapes", i);
            if i > 0 {
                assert_eq!(r.y, l.measure_row(i - 1).unwrap().bottom(), "row {} overlaps", i);
            }
        }
        for i in 0..3 {
            let r = l.control_row(i).expect("control row");
            assert!(r.y >= l.controls.y && r.bottom() <= l.controls.bottom(), "crow {} escapes", i);
        }
        assert!(l.measure_row(4).is_none());
        assert!(l.control_row(3).is_none());
    }

    /// Draw and hit-test must come from the same rect — the `ctl_btn_rect` lesson, again.
    #[test]
    fn control_hit_matches_the_drawn_rows() {
        let l = entity_layout(SW, SH, 2, 4, 3);
        for i in 0..3 {
            let r = l.control_row(i).unwrap();
            assert_eq!(l.control_hit(r.x + 4, r.y + r.h / 2), Some(i));
        }
        assert_eq!(l.control_hit(l.surface.x + 2, l.controls.y + 4), None, "outside the inset");
        assert_eq!(l.control_hit(l.statement.x, l.statement.y), None, "the head is not a control");
    }

    /// The track has to stay inside the row it belongs to, at every interface scale. A track that
    /// overhangs the row bleeds into the neighbouring control and reads as that row's level.
    #[test]
    fn the_control_track_stays_inside_its_row() {
        for s in [1000, 1250, 1500, 2000, 3000] {
            crate::scale::force_for_test(s);
            let l = entity_layout(SW, SH, 2, 4, 4);
            for i in 0..4 {
                let row = l.control_row(i).unwrap();
                let t = l.control_track(i).expect("track");
                assert!(t.y >= row.y && t.bottom() <= row.bottom(), "scale {} row {} overhangs", s, i);
                assert!(t.x >= row.x && t.right() <= row.right(), "scale {} row {} too wide", s, i);
                assert!(t.w > 0 && t.h > 0, "scale {} row {} collapsed", s, i);
            }
            assert!(l.control_track(4).is_none(), "a track for a row that does not exist");
        }
        crate::scale::force_for_test(1000);
    }

    /// The two ends and the middle must be exactly 0, 50 and 100 — a slider you cannot drag to
    /// either extreme is the classic off-by-one, and it is invisible until someone wants full
    /// brightness and gets 98.
    #[test]
    fn a_drag_reaches_both_ends_of_the_track() {
        let t = Rect::new(40, 10, 200, 2);
        assert_eq!(track_percent(t, t.x), 0);
        assert_eq!(track_percent(t, t.right()), 100);
        assert_eq!(track_percent(t, t.x + t.w / 2), 50);
        // Off either end pins rather than wrapping or going out of range.
        assert_eq!(track_percent(t, t.x - 500), 0);
        assert_eq!(track_percent(t, t.right() + 500), 100);
        for mx in (t.x - 20)..=(t.right() + 20) {
            let p = track_percent(t, mx);
            assert!((0..=100).contains(&p), "mx {} gave {}", mx, p);
        }
    }

    /// The hairlines mark section boundaries; a rule inside a section would cut a row in half.
    #[test]
    fn the_rules_sit_on_the_section_boundaries() {
        let l = entity_layout(SW, SH, 2, 4, 3);
        assert_eq!(l.rules(), [l.measures.y, l.identity.y, l.controls.y, l.footer.y]);
        for y in l.rules() {
            assert!(y > l.surface.y && y < l.surface.bottom(), "rule at {} escapes the surface", y);
        }
    }

    /// It has to fit on the smallest panel this machine drives. The surface grows upward from a
    /// fixed bottom, so an over-tall one runs off the TOP of the screen, where nothing clips it.
    #[test]
    fn the_surface_fits_a_720p_screen() {
        let l = entity_layout(1280, 720, 3, 4, 3);
        assert!(l.surface.y > 0, "the Entity surface overflows 720p by {}px", -l.surface.y);
    }

    // ---- windows ----

    #[test]
    fn the_caption_sits_above_the_surface() {
        let surf = Rect::new(336, 128, 768, 512);
        let cap = caption(surf);
        assert_eq!(cap.bottom(), surf.y, "caption must end where the surface begins");
        assert_eq!(cap.x, surf.x, "caption is left-aligned to the surface, like a print caption");
        assert_eq!(cap.h, CAPTION_H);
    }

    #[test]
    fn the_footprint_covers_caption_and_surface() {
        let surf = Rect::new(336, 128, 768, 512);
        let f = window_footprint(surf);
        assert_eq!(f.y, surf.y - CAPTION_H);
        assert_eq!(f.bottom(), surf.bottom());
        assert_eq!(f.h, surf.h + CAPTION_H);
    }

    /// Two controls, right-aligned, fold before close — and both inside the caption line.
    #[test]
    fn the_controls_are_two_and_right_aligned() {
        let surf = Rect::new(336, 128, 768, 512);
        let cap = caption(surf);
        assert_eq!(CONTROLS.len(), 2);
        assert_eq!(CONTROLS[0], Control::Fold);
        assert_eq!(CONTROLS[1], Control::Close);

        let fold = control(surf, 0).unwrap();
        let close = control(surf, 1).unwrap();
        assert!(fold.x < close.x, "fold should come before close");
        assert_eq!(close.right(), cap.right() - CAPTION_PAD_X);
        assert_eq!(close.x - fold.right(), CTL_GAP);
        for c in [fold, close] {
            assert!(c.y >= cap.y && c.bottom() <= cap.bottom(), "control escapes the caption line");
        }
        assert!(control(surf, 2).is_none());
    }

    /// Draw and hit-test must agree — the discipline this module exists for.
    #[test]
    fn control_hit_matches_the_drawn_rects() {
        let surf = Rect::new(336, 128, 768, 512);
        for (i, expect) in CONTROLS.iter().enumerate() {
            let r = control(surf, i).unwrap();
            for (px, py) in [
                (r.x, r.y),
                (r.right() - 1, r.bottom() - 1),
                (r.x + r.w / 2, r.y + r.h / 2),
            ] {
                assert_eq!(control_hit(surf, px, py), Some(*expect), "missed {:?} at ({},{})", expect, px, py);
            }
            // Just outside must not hit.
            assert_ne!(control_hit(surf, r.x - 1, r.y - 1), Some(*expect));
        }
        assert_eq!(control_hit(surf, surf.x, surf.y + 10), None, "the surface is not a control");
    }

    /// A drag must not start on a control, or clicking close would also begin a drag.
    #[test]
    fn the_drag_region_excludes_the_controls() {
        let surf = Rect::new(336, 128, 768, 512);
        let drag = caption_drag_region(surf);
        for i in 0..CONTROLS.len() {
            let c = control(surf, i).unwrap();
            assert!(!drag.contains(c.x + c.w / 2, c.y + c.h / 2), "control {} is inside the drag region", i);
        }
        // The name end of the caption must still be draggable.
        assert!(drag.contains(surf.x + 10, surf.y - CAPTION_H / 2));
    }

    /// A folded window keeps its position — that is the entire argument for fold over minimise, and
    /// why the design needs no taskbar.
    #[test]
    fn folding_keeps_the_caption_where_it_was() {
        let surf = Rect::new(336, 702, 560, 400);
        let open = caption(surf);
        let folded = folded_footprint(surf);
        assert_eq!(folded.x, open.x, "a folded window must not move");
        assert_eq!(folded.y, open.y);
        assert!(folded.h > CAPTION_H, "the fold rule needs room below the caption");
        assert!(folded.h < window_footprint(surf).h, "folding must shrink the footprint");
        assert_eq!(folded_rule(surf).h, 1, "the fold marker is a hairline");
        assert_eq!(folded_rule(surf).bottom(), folded.bottom());
    }

    // ---- the command ----

    #[test]
    fn the_command_is_centred_at_the_design_size() {
        let p = command_panel(SW, 300);
        assert_eq!(p.w, CMD_W);
        assert_eq!(p.x, (SW - CMD_W) / 2);
        assert_eq!(p.x + p.w / 2, SW / 2, "must be centred");
        assert_eq!(p.y, CMD_TOP);
    }

    /// Entries must stack without gaps or overlaps, and stay inside the panel.
    #[test]
    fn command_entries_stack_exactly() {
        let entries = [
            CmdEntry::Group, CmdEntry::Row, CmdEntry::Row,
            CmdEntry::Group, CmdEntry::Row,
            CmdEntry::Group, CmdEntry::Row, CmdEntry::Row,
        ];
        let panel = command_panel(SW, command_body_height(&entries));
        let mut prev_bottom = panel.y + CMD_QUERY_H + CMD_B_PAD_TOP;
        for i in 0..entries.len() {
            let r = command_entry(SW, &entries, i).unwrap();
            assert_eq!(r.y, prev_bottom, "entry {} does not follow the previous one", i);
            prev_bottom = r.bottom();
        }
        assert_eq!(
            prev_bottom + CMD_B_PAD_BOTTOM + CMD_FOOTER_H,
            panel.bottom(),
            "the body height and the stacked entries disagree"
        );
        assert!(command_entry(SW, &entries, entries.len()).is_none());
    }

    /// Clicking must select the row the pointer is actually over — and never a heading.
    #[test]
    fn command_hit_skips_headings() {
        let entries = [CmdEntry::Group, CmdEntry::Row, CmdEntry::Row, CmdEntry::Group, CmdEntry::Row];
        for (i, e) in entries.iter().enumerate() {
            let r = command_entry(SW, &entries, i).unwrap();
            let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
            match e {
                CmdEntry::Group => assert_eq!(command_hit(SW, &entries, cx, cy), None, "heading {} was selectable", i),
                CmdEntry::Row => assert_eq!(command_hit(SW, &entries, cx, cy), Some(i), "row {} missed", i),
            }
        }
    }

    /// ↑↓ steps between RESULTS, not between list rows — the selection must never land on a label.
    #[test]
    fn arrow_navigation_steps_over_headings() {
        let entries = [CmdEntry::Group, CmdEntry::Row, CmdEntry::Row, CmdEntry::Group, CmdEntry::Row];
        assert_eq!(command_first(&entries), Some(1), "first selection should be the first result");
        assert_eq!(command_step(&entries, Some(1), 1), Some(2));
        assert_eq!(command_step(&entries, Some(2), 1), Some(4), "should skip the heading at 3");
        assert_eq!(command_step(&entries, Some(4), 1), Some(4), "should stop at the end, not wrap");
        assert_eq!(command_step(&entries, Some(4), -1), Some(2));
        assert_eq!(command_step(&entries, Some(1), -1), Some(1), "should stop at the start");
    }

    #[test]
    fn navigation_handles_an_empty_or_headings_only_list() {
        assert_eq!(command_first(&[]), None);
        assert_eq!(command_step(&[], None, 1), None);
        let only_labels = [CmdEntry::Group, CmdEntry::Group];
        assert_eq!(command_first(&only_labels), None);
    }

    /// The Command must fit on screen at every resolution the machine might run — which is what
    /// `command_visible` is for. An uncapped 14-entry list is 823px tall and overflows 720p, so this
    /// is a real constraint rather than a hypothetical one.
    #[test]
    fn the_command_fits_on_screen_once_capped() {
        let entries: alloc::vec::Vec<CmdEntry> = (0..14)
            .map(|i| if i % 5 == 0 { CmdEntry::Group } else { CmdEntry::Row })
            .collect();
        for (w, h) in [(1280, 720), (1440, 900), (1920, 1080)] {
            let n = command_visible(h, &entries);
            assert!(n > 0, "nothing fits on a {}x{} screen", w, h);
            let shown = &entries[..n];
            let p = command_panel(w, command_body_height(shown));
            assert!(p.x > 0 && p.right() < w, "the Command overflows a {}px screen", w);
            assert!(
                p.bottom() + CMD_BOTTOM_MARGIN <= h,
                "the Command reaches the bottom of a {}px screen ({} entries, bottom {})",
                h, n, p.bottom()
            );
        }
    }

    /// The uncapped list really does overflow — so the cap is load-bearing, not decoration.
    #[test]
    fn an_uncapped_list_would_overflow() {
        let entries: alloc::vec::Vec<CmdEntry> = (0..14)
            .map(|i| if i % 5 == 0 { CmdEntry::Group } else { CmdEntry::Row })
            .collect();
        let p = command_panel(1280, command_body_height(&entries));
        assert!(p.bottom() > 720, "this test is pointless if 14 entries already fit");
        assert!(command_visible(720, &entries) < entries.len());
    }

    /// A capped list must never end on a group heading — a label with nothing under it looks like a
    /// rendering fault.
    #[test]
    fn the_capped_list_never_ends_on_a_heading() {
        // Craft a list where the naive cut lands exactly on a heading.
        for count in 1..24usize {
            let entries: alloc::vec::Vec<CmdEntry> = (0..count)
                .map(|i| if i % 3 == 0 { CmdEntry::Group } else { CmdEntry::Row })
                .collect();
            for h in [600, 720, 900, 1080] {
                let n = command_visible(h, &entries);
                if n > 0 {
                    assert_eq!(
                        entries[n - 1],
                        CmdEntry::Row,
                        "cap of {} on a {}px screen ends on a heading",
                        n, h
                    );
                }
            }
        }
    }

    /// More screen must never show fewer results.
    #[test]
    fn a_taller_screen_shows_at_least_as_much() {
        let entries: alloc::vec::Vec<CmdEntry> = (0..30)
            .map(|i| if i % 4 == 0 { CmdEntry::Group } else { CmdEntry::Row })
            .collect();
        let mut prev = 0;
        for h in [600, 720, 900, 1080, 1440] {
            let n = command_visible(h, &entries);
            assert!(n >= prev, "a {}px screen showed {} after {}", h, n, prev);
            prev = n;
        }
    }

    // ─────────────────────────────────────────────────────────────────────────
    // SCALED LAYOUT
    //
    // Every test above runs at scale 1.0, where `sc()` is the identity — which is why they read as
    // the design's own numbers. These are the ones that prove the scaling actually reached the
    // geometry. `crate::scale` stores the factor thread-locally under `cfg(test)`, so setting it
    // here cannot disturb the tests running in parallel alongside.
    // ─────────────────────────────────────────────────────────────────────────

    use crate::scale;

    /// The failure this whole change exists to prevent: geometry that scaled and geometry that did
    /// not, sitting next to each other. If a constant were missed, the caption would stay 22px tall
    /// while the text inside it grew.
    #[test]
    fn window_geometry_scales_as_a_whole() {
        scale::force_for_test(1250);
        let surf = Rect::new(200, 200, 640, 400);
        let cap = caption(surf);
        assert_eq!(cap.h, scale::px_at(22, 1250), "the caption line must scale");
        assert_eq!(caption_h(), cap.h, "the accessor and the rect must agree");
        assert_eq!(cap.bottom(), surf.y, "caption still ends where the surface begins");

        // Controls stay inside the taller caption, and stay two.
        for i in 0..CONTROLS.len() {
            let b = control(surf, i).expect("control");
            assert!(b.y >= cap.y && b.bottom() <= cap.bottom(), "control {} escapes the caption", i);
            assert_eq!(b.w, scale::px_at(18, 1250));
        }
        // And hit-testing follows the drawn plates, which is the whole point.
        let close = control(surf, 1).unwrap();
        assert_eq!(control_hit(surf, close.x + 2, close.y + 2), Some(Control::Close));
    }

    /// The dock must stay coherent: bigger glyphs, proportionally bigger gaps, still disjoint, still
    /// clear of the screen edge.
    #[test]
    fn the_dock_scales_without_overlapping() {
        for &f in &[1000, 1250, 1500, 2000] {
            scale::force_for_test(f);
            let sh = 1080;
            assert_eq!(dock_icon(), scale::px_at(20, f));
            let mut prev_right = 0;
            for i in 0..DOCK_SLOTS {
                let r = dock_slot(sh, i).expect("slot");
                assert!(r.x >= prev_right, "slot {} overlaps its neighbour at scale {}", i, f);
                assert!(r.bottom() <= sh, "slot {} runs off the bottom at scale {}", i, f);
                prev_right = r.right();
            }
            // Hit-testing must still land on the glyph it drew.
            let r = dock_slot(sh, 0).unwrap();
            assert_eq!(dock_hit(sh, r.x + r.w / 2, r.y + r.h / 2), Some(0));
        }
    }

    /// The Command grows with the interface, stays centred, and still fits — `command_visible` caps
    /// by vertical space, so a larger scale must show FEWER results, not overflow.
    #[test]
    fn the_command_scales_and_still_fits() {
        let entries = alloc::vec![
            CmdEntry::Group, CmdEntry::Row, CmdEntry::Row, CmdEntry::Row,
            CmdEntry::Group, CmdEntry::Row, CmdEntry::Row, CmdEntry::Row,
            CmdEntry::Group, CmdEntry::Row, CmdEntry::Row, CmdEntry::Row,
        ];
        let (sw, sh) = (1920, 1080);
        let mut prev_visible = usize::MAX;
        for &f in &[1000, 1250, 1500, 2000] {
            scale::force_for_test(f);
            let n = command_visible(sh, &entries);
            let shown: alloc::vec::Vec<CmdEntry> = entries[..n].to_vec();
            let panel = command_panel(sw, command_body_height(&shown));
            assert_eq!(panel.w, scale::px_at(620, f), "the panel must scale");
            // Within a pixel: a scaled width can be odd, and 1px off centre on a 1920px screen is
            // not a thing anyone can see.
            assert!(
                (panel.x + panel.w / 2 - sw / 2).abs() <= 1,
                "off centre at scale {}: {} vs {}", f, panel.x + panel.w / 2, sw / 2
            );
            assert!(
                panel.bottom() <= sh - scale::px_at(48, f),
                "at scale {} the Command overflows: bottom {} on a {}px screen",
                f, panel.bottom(), sh
            );
            assert!(n <= prev_visible, "scale {} showed MORE results than a smaller scale", f);
            prev_visible = n;
        }
    }

    /// The Entity surface grows upward from a fixed bottom, so scaling is where it would run off the
    /// TOP of the screen — and nothing clips it there.
    ///
    /// Each scale is paired with a panel that would actually select it (`scale::for_screen`), since
    /// 2.0 on a 1080p screen is not a configuration this can be in.
    #[test]
    fn the_entity_surface_scales_within_the_screen() {
        for &(f, sw, sh) in &[(1000, 1280, 720), (1250, 1920, 1080), (1500, 2560, 1440), (2000, 3840, 2160)] {
            scale::force_for_test(f);
            let cap = entity_max_detail_lines(sh, 4, 3);
            assert!(cap >= 2, "only {} detail lines fit at scale {} on {}x{}", cap, f, sw, sh);
            let l = entity_layout(sw, sh, cap.min(3), 4, 3);
            assert_eq!(l.surface.w, scale::px_at(372, f));
            assert!(l.surface.y > 0, "the Entity overflows the top at scale {} by {}px", f, -l.surface.y);
            assert_eq!(l.footer.bottom(), l.surface.bottom(), "sections must still tile exactly");
            let row = l.control_row(1).expect("control row");
            assert_eq!(l.control_hit(row.x + 4, row.y + row.h / 2), Some(1), "hit-test drifted");
        }
    }

    /// The cap has to be honoured by the layout it feeds, and the backstop has to catch anyone who
    /// ignores it — an unclipped surface off the top of the screen is a statement nobody can read.
    #[test]
    fn the_detail_cap_keeps_the_entity_on_screen() {
        for &f in &[1000, 1250, 1500, 2000] {
            scale::force_for_test(f);
            for sh in [720, 900, 1080, 1440] {
                let cap = entity_max_detail_lines(sh, 4, 3);
                let at_cap = entity_layout(400, sh, cap, 4, 3);
                assert!(
                    at_cap.surface.y >= scale::px_at(24, f),
                    "at scale {} on {}px, {} lines still overflows (y={})", f, sh, cap, at_cap.surface.y
                );
                // And one line past it must be clamped rather than negative.
                let over = entity_layout(400, sh, cap + 8, 4, 3);
                assert!(over.surface.y > 0, "the backstop let the surface off the top");
                assert_eq!(over.footer.bottom(), over.surface.bottom(), "clamping broke the tiling");
            }
        }
    }

    /// Hairlines and the fold tick are 1-3px by design. Scaling must not round any of them away —
    /// Meridian carries its structure on hairlines, and a border that vanished at 1.25x would look
    /// like the window had lost its frame.
    #[test]
    fn thin_details_survive_every_scale() {
        for &f in &[1000, 1250, 1500, 2000] {
            scale::force_for_test(f);
            assert!(fold_tick() >= 1, "the fold tick vanished at scale {}", f);
            assert!(cmd_sel_bar_w() >= 1, "the selection bar vanished at scale {}", f);
            let d = dock_slot(1080, 7).expect("divider");
            assert!(d.w >= 1 && d.h >= 1, "the dock divider vanished at scale {}", f);
            assert!(folded_rule(Rect::new(100, 100, 400, 300)).h >= 1);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// THE OSD
// ─────────────────────────────────────────────────────────────────────────────
// A system popup — brightness, volume — grows out of the Entity mark, in the same corner, with the
// same geometry as a notification. System state appears where system state always appears.
//
// It does NOT dim or blur the screen. Every OSD in every other desktop that does is spending a
// full-screen pass to announce a volume change; this one is a single damage rectangle of about
// 21,000 pixels, roughly 1.6% of a 1080p screen.

const OSD_W: i32 = 232;
const OSD_H: i32 = 92;
/// Gap between the OSD's bottom edge and the Entity mark it grew from.
const OSD_GAP: i32 = 12;
const OSD_PAD_X: i32 = 16;
const OSD_PAD_TOP: i32 = 14;
/// The hairline track under the numeral.
const OSD_TRACK_H: i32 = 2;
const OSD_TRACK_BOTTOM: i32 = 18;
const OSD_ICON: i32 = 16;

pub fn osd_pad_x() -> i32 { sc(OSD_PAD_X) }
pub fn osd_pad_top() -> i32 { sc(OSD_PAD_TOP) }
pub fn osd_icon() -> i32 { sc(OSD_ICON) }

/// The OSD panel, sitting directly above the Entity mark and right-aligned with it.
///
/// Anchored to `entity_mark` rather than to the screen edge so the two cannot drift apart: the
/// popup is supposed to look like it grew out of the mark, and a popup that is two pixels off the
/// mark's right edge looks like a mistake rather than like a relationship.
pub fn osd_panel(screen_w: i32, screen_h: i32) -> Rect {
    let m = entity_mark(screen_w, screen_h);
    let w = sc(OSD_W);
    Rect::new(m.right() - w, m.y - sc(OSD_GAP) - sc(OSD_H), w, sc(OSD_H))
}

/// The value track inside the panel — full width less the padding, near the bottom.
///
/// Returns `None` for a state with no value to show, which is the volume case today: there is no
/// audio device, so there is no percentage, and drawing an empty track would imply one exists at
/// zero rather than that the subsystem is absent.
pub fn osd_track(panel: Rect, has_value: bool) -> Option<Rect> {
    if !has_value {
        return None;
    }
    Some(Rect::new(
        panel.x + sc(OSD_PAD_X),
        panel.bottom() - sc(OSD_TRACK_BOTTOM),
        panel.w - sc(OSD_PAD_X) * 2,
        sc(OSD_TRACK_H),
    ))
}

/// How much of the track is filled, for `pct` of 100.
pub fn osd_fill_w(track: Rect, pct: i32) -> i32 {
    (track.w * pct.clamp(0, 100)) / 100
}

#[cfg(test)]
mod scrollbar_tests {
    use super::*;

    /// A typical app surface: the caption is outside it, so this is pure content.
    fn surface() -> Rect {
        Rect::new(300, 120, 880, 512)
    }

    /// ★★ THE POINT OF THE WHOLE SECTION.
    ///
    /// `apps/shell` now tests the scrollbar BEFORE `resize_edges`, so the bar can sit near the wall
    /// — but it must not swallow the whole edge. Both right-hand corners have to keep resizing,
    /// because a corner is the only place a window resizes in two axes at once. Swept at every scale
    /// step, since the corner and the grab region scale independently of each other.
    #[test]
    fn the_scrollbar_never_claims_a_resize_corner() {
        for step in crate::scale::STEPS {
            crate::scale::force_for_test(step);
            let s = surface();
            let corner = scrollbar_corner();
            for mx in (s.right() - resize_band())..=s.right() {
                for my in s.y..(s.y + corner) {
                    assert!(
                        !scrollbar_hit(s, 4000, s.h, mx, my),
                        "scale {}: the bar took the TOP-right corner at ({}, {})",
                        step, mx, my
                    );
                }
                for my in (s.bottom() - corner)..s.bottom() {
                    assert!(
                        !scrollbar_hit(s, 4000, s.h, mx, my),
                        "scale {}: the bar took the BOTTOM-right corner at ({}, {})",
                        step, mx, my
                    );
                }
            }
        }
        crate::scale::force_for_test(crate::scale::UNIT);
    }

    /// The reason the ordering was inverted at all: the user could see the gap. The thumb has to end
    /// up genuinely near the wall, not merely nearer than it was.
    #[test]
    fn the_thumb_sits_against_the_wall_at_every_scale() {
        for step in crate::scale::STEPS {
            crate::scale::force_for_test(step);
            let s = surface();
            let gap = s.right() - scrollbar_track(s).right();
            assert!(
                gap <= sc(4),
                "scale {}: {}px of dead air between a {}px bar and the window wall",
                step, gap, sc(SB_THUMB_W)
            );
        }
        crate::scale::force_for_test(crate::scale::UNIT);
    }

    /// A window with nothing to scroll must not take the edge away from the resizer at all.
    #[test]
    fn an_unscrollable_window_keeps_its_whole_right_edge() {
        let s = surface();
        for my in s.y..s.bottom() {
            for mx in (s.right() - resize_band())..=s.right() {
                assert!(!scrollbar_hit(s, s.h, s.h, mx, my), "claimed ({}, {}) with nothing to scroll", mx, my);
            }
        }
    }

    /// Between the corners the bar DOES own the edge — that is the trade being made, and it should
    /// fail loudly if someone reinstates the old ordering.
    #[test]
    fn between_the_corners_the_bar_owns_the_edge() {
        let s = surface();
        let mid = s.y + s.h / 2;
        assert!(scrollbar_hit(s, 4000, s.h, s.right(), mid), "the outermost column must scroll");
        assert!(scrollbar_hit(s, 4000, s.h, s.right() - resize_band(), mid));
        // ...and the app's content is still reachable well inside it.
        assert!(!scrollbar_hit(s, 4000, s.h, s.right() - scrollbar_reserve() - 1, mid));
    }

    /// A window shorter than two corners is all corner. The bar must decline rather than hand back a
    /// grab region that overlaps both of them.
    #[test]
    fn a_very_short_window_leaves_the_edge_to_the_resizer() {
        let s = Rect::new(0, 0, 400, scrollbar_corner() * 2);
        assert!(!scrollbar_hit(s, 4000, s.h, s.right(), s.y + s.h / 2));
    }

    /// It is a mark on the surface, not something stuck to the outside of it.
    #[test]
    fn the_track_sits_inside_the_window_on_every_edge() {
        let s = surface();
        let t = scrollbar_track(s);
        assert!(t.x > s.x && t.right() < s.right(), "{:?} is not inside {:?}", t, s);
        assert!(t.y > s.y && t.bottom() < s.bottom(), "the track runs into the rounded corners");
        assert_eq!(t.w, 4, "the design's idiom is a hairline-weight mark, not a 12px widget");
    }

    #[test]
    fn there_is_no_thumb_when_everything_fits() {
        let t = scrollbar_track(surface());
        assert!(scrollbar_thumb(t, 100, 512, 0).is_none());
        assert!(scrollbar_thumb(t, 512, 512, 0).is_none());
        assert!(scrollbar_thumb(t, 513, 512, 0).is_some());
        assert!(!scrollbar_hit(surface(), 512, 512, 1000, 300), "nothing to grab");
    }

    /// The thumb has to stay a target however long the list gets, and must never leave its track.
    #[test]
    fn the_thumb_stays_grabbable_and_inside_its_track() {
        let s = surface();
        let t = scrollbar_track(s);
        for content in [600, 1000, 5000, 200_000] {
            for scroll in [0, 1, content / 3, content - s.h] {
                let th = scrollbar_thumb(t, content, s.h, scroll).unwrap();
                assert!(th.h >= 28.min(t.h), "content {}: thumb is {}px tall", content, th.h);
                assert!(th.y >= t.y, "thumb above its track");
                assert!(th.bottom() <= t.bottom(), "thumb below its track at scroll {}", scroll);
                assert_eq!(th.x, t.x);
            }
        }
    }

    /// Both ends of the track must be reachable, and must mean the two ends of the content.
    #[test]
    fn the_ends_of_the_track_are_the_ends_of_the_content() {
        let s = surface();
        let t = scrollbar_track(s);
        let content = 4000;
        let max = content - s.h;

        let top = scrollbar_thumb(t, content, s.h, 0).unwrap();
        assert_eq!(top.y, t.y, "at scroll 0 the thumb is at the top");
        let bottom = scrollbar_thumb(t, content, s.h, max).unwrap();
        assert_eq!(bottom.bottom(), t.bottom(), "at max scroll the thumb is at the bottom");

        assert_eq!(scroll_from_mouse(s, content, s.h, t.y - 100), 0, "above the track clamps to 0");
        assert_eq!(scroll_from_mouse(s, content, s.h, t.bottom() + 100), max, "below clamps to max");
    }

    /// Dragging must never produce an offset outside the legal range, at any pointer position —
    /// the app clamps too, but an out-of-range `MSG_SCROLL` would make the thumb and the content
    /// disagree about where the list is.
    #[test]
    fn a_drag_anywhere_produces_a_legal_offset() {
        let s = surface();
        for content in [513, 900, 4000, 100_000] {
            let max = content - s.h;
            for my in (s.y - 50)..(s.bottom() + 50) {
                let off = scroll_from_mouse(s, content, s.h, my);
                assert!(off >= 0 && off <= max, "content {} my {} gave {}", content, my, off);
            }
        }
    }

    /// Clicking the track anywhere down its length is a jump, so the hit region is the whole column
    /// between the corners rather than only the thumb.
    ///
    /// It used to be the full height of the surface. That changed deliberately when the bar took the
    /// right edge from the resizer: the two [`scrollbar_corner`] bands are the price, and they are
    /// the only place this returns false down the column.
    #[test]
    fn the_whole_column_between_the_corners_is_clickable_not_just_the_thumb() {
        let s = surface();
        let t = scrollbar_track(s);
        let corner = scrollbar_corner();
        for my in (s.y + corner)..(s.bottom() - corner) {
            assert!(scrollbar_hit(s, 4000, s.h, t.x + 1, my), "dead at y={}", my);
        }
        let mid = s.y + s.h / 2;
        assert!(scrollbar_hit(s, 4000, s.h, t.x - 5, mid), "the grab region is padded");
        assert!(!scrollbar_hit(s, 4000, s.h, t.x - 40, mid), "but not unboundedly");
    }

    /// A window squeezed to nothing must not produce a negative track or panic the desktop.
    #[test]
    fn a_degenerate_surface_is_survivable() {
        for (w, h) in [(0, 0), (1, 1), (208, 10), (208, 100)] {
            let s = Rect::new(0, 0, w, h);
            let t = scrollbar_track(s);
            assert!(t.h >= 0, "{}x{} gave a negative track", w, h);
            let _ = scrollbar_thumb(t, 10_000, h, 5);
            let _ = scroll_from_mouse(s, 10_000, h, 50);
            let _ = scrollbar_hit(s, 10_000, h, 5, 5);
        }
    }
}

#[cfg(test)]
mod osd_tests {
    use super::*;

    const SW: i32 = 1440;
    const SH: i32 = 900;

    #[test]
    fn the_osd_grows_out_of_the_entity_mark() {
        let m = entity_mark(SW, SH);
        let p = osd_panel(SW, SH);
        assert_eq!(p.right(), m.right(), "the OSD must be flush with the mark's right edge");
        assert!(p.bottom() < m.y, "the OSD sits ABOVE the mark, not over it");
        assert_eq!(p.w, OSD_W);
        assert_eq!(p.h, OSD_H);
        assert!(p.x > 0 && p.y > 0, "the OSD fell off the screen: {:?}", p);
    }

    #[test]
    fn the_osd_costs_about_what_the_design_says() {
        // "a single damage rectangle of about 21,000 pixels". If this grows by an order of
        // magnitude someone has quietly turned it into a full-screen surface.
        let p = osd_panel(SW, SH);
        let px = p.w * p.h;
        assert!((18_000..24_000).contains(&px), "the OSD is {} pixels", px);
    }

    #[test]
    fn a_valueless_state_has_no_track() {
        // Volume today: no audio device, so no percentage. An empty track would say "zero", which
        // is a different and untrue statement from "this subsystem does not exist".
        let p = osd_panel(SW, SH);
        assert!(osd_track(p, false).is_none());
        let t = osd_track(p, true).expect("a value state has a track");
        assert!(t.x >= p.x && t.right() <= p.right(), "track escapes the panel: {:?}", t);
        assert!(t.bottom() <= p.bottom());
    }

    #[test]
    fn the_fill_spans_the_track_exactly() {
        let t = osd_track(osd_panel(SW, SH), true).unwrap();
        assert_eq!(osd_fill_w(t, 0), 0);
        assert_eq!(osd_fill_w(t, 100), t.w);
        assert!(osd_fill_w(t, 50) > 0 && osd_fill_w(t, 50) < t.w);
        // Out of range must clamp rather than draw past the track's end.
        assert_eq!(osd_fill_w(t, 150), t.w);
        assert_eq!(osd_fill_w(t, -20), 0);
    }

    #[test]
    fn the_osd_stays_on_screen_at_every_interface_scale() {
        for step in crate::scale::STEPS {
            crate::scale::force_for_test(step);
            for (w, h) in [(1366, 768), (1920, 1080), (2560, 1440), (3840, 2160)] {
                let p = osd_panel(w, h);
                assert!(p.x >= 0 && p.y >= 0 && p.right() <= w && p.bottom() <= h,
                        "scale {} on {}x{}: {:?}", step, w, h, p);
            }
        }
        crate::scale::force_for_test(crate::scale::UNIT);
    }
}
