//! The design's two list components: the **navigation pane** (`.nav` / `.nv`) and the **file row**
//! (`.fr`), plus the content column they sit in (`.pane` / `.main` / `.main-h`).
//!
//! These are components, not one application's furniture. `03-components.html` defines them in the
//! shared component sheet, the hero scene builds Files out of them, and Settings (step 13, "rows on
//! hairlines") is the next surface that wants them.
//!
//! ## Why an application's geometry lives in this crate
//!
//! Exactly the argument `layout.rs` makes for the shell, for exactly the same two reasons.
//! `apps/explorer` is a `no_std` binary with a `_start`; its tests cannot run on the host, and this
//! machine has no QEMU, so a hit-test that disagrees with a draw by four pixels is found by booting
//! and squinting. Moved in here, both the plate and the pointer come from one function and it is
//! proved by `cargo test`.
//!
//! ## Units
//!
//! Same rule as `layout.rs`: the `const`s are **design units** against the 1440x900 scene and they
//! are private. Everything public returns real pixels, having passed through [`crate::scale::px`].
//! At scale 1.0 that is the identity, which is why the tests below read as the design's own CSS.

use alloc::string::String;
use alloc::vec::Vec;

use crate::layout::Rect;
use crate::scale::px as sc;
use crate::tokens::{B1, B2, B3, D1, D2, LB, Style};

// ─────────────────────────────────────────────────────────────────────────────
// THE NAVIGATION PANE  —  `.nav`, `.nav .lb`, `.nv`, `.nv .c`
// ─────────────────────────────────────────────────────────────────────────────

/// `.nav { width: 208px }`
const NAV_W: i32 = 208;
/// `.nav { padding: 30px 0 24px 30px }`
const NAV_PAD_TOP: i32 = 30;
const NAV_PAD_LEFT: i32 = 30;
/// `.nav .lb { margin-bottom: 16px }`
const NAV_LB_GAP_BELOW: i32 = 16;
/// `.nav .lb:not(:first-child) { margin-top: 32px }` — the air that separates one group from the
/// previous group's last row. The design uses space, not a rule, to divide the two lists.
const NAV_LB_GAP_ABOVE: i32 = 32;
/// `.nv { height: 30px }`
const NV_H: i32 = 30;
/// `.nv { padding-left: 12px }` — the indent that distinguishes a destination from its heading.
const NV_PAD_LEFT: i32 = 12;
/// `.nv.on::before { width: 2px; top: 9px; bottom: 9px }` — the accent mark on the active row.
const NV_BAR_W: i32 = 2;
const NV_BAR_INSET: i32 = 9;
/// `.nv .c { padding-right: 22px }`
const NV_COUNT_PAD_RIGHT: i32 = 22;

/// What a nav entry is. The pane is one flat list so that a heading and a row cannot get out of
/// order between the code that lays it out and the code that draws it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NavKind {
    /// `.nav .lb` — an uppercase group heading. Not a destination and never highlighted.
    Group,
    /// `.nv` — a place you can go.
    Item,
}

/// The nav column's rect inside a surface `h` tall.
pub fn nav(h: i32) -> Rect {
    Rect::new(0, 0, sc(NAV_W), h)
}

/// The rect of nav entry `i`, given the kinds of every entry in the pane.
///
/// The pane is laid out by walking from the top, because a group heading's height depends on
/// whether anything precedes it (`:not(:first-child)`). `kinds` is short — under ten entries — so
/// the walk is cheaper than caching it would be, and a cache is one more thing that can be stale
/// when the device list changes.
pub fn nav_entry(kinds: &[NavKind], i: usize) -> Option<Rect> {
    if i >= kinds.len() {
        return None;
    }
    let x = sc(NAV_PAD_LEFT);
    let w = sc(NAV_W) - x;
    let mut y = sc(NAV_PAD_TOP);
    for (j, k) in kinds.iter().enumerate() {
        let h = match k {
            NavKind::Group => {
                if j > 0 {
                    y += sc(NAV_LB_GAP_ABOVE);
                }
                LB.line() as i32
            }
            NavKind::Item => sc(NV_H),
        };
        if j == i {
            return Some(Rect::new(x, y, w, h));
        }
        y += h;
        if matches!(k, NavKind::Group) {
            y += sc(NAV_LB_GAP_BELOW);
        }
    }
    None
}

/// The nav entry under `(mx, my)`, or None. Group headings are never hit — they are not places.
///
/// ⚠️ The hit band is the full width of the pane, not the entry's visual rect, which starts 30px in
/// at the pane's padding. The *vertical* extent is the thing that can drift between drawing and
/// hit-testing, and it comes from [`nav_entry`] so it cannot. Widening horizontally is deliberate:
/// the 30px gutter left of a row is dead space that looks like part of the row, and a click there
/// doing nothing reads as an unresponsive list.
pub fn nav_hit(kinds: &[NavKind], mx: i32, my: i32) -> Option<usize> {
    if mx < 0 || mx >= sc(NAV_W) {
        return None;
    }
    (0..kinds.len()).find(|&i| {
        kinds[i] == NavKind::Item
            && nav_entry(kinds, i).is_some_and(|r| my >= r.y && my < r.y + r.h)
    })
}

/// The 2px accent mark on the active row, from that row's rect.
pub fn nav_bar(entry: Rect) -> Rect {
    Rect::new(entry.x, entry.y + sc(NV_BAR_INSET), sc(NV_BAR_W), entry.h - 2 * sc(NV_BAR_INSET))
}

/// Where a row's label starts. A group heading draws at `entry.x`; a row is indented past it.
pub fn nav_label_x(entry: Rect) -> i32 {
    entry.x + sc(NV_PAD_LEFT)
}

/// The right edge a row's count (`15`, `64 G`) is right-aligned to.
pub fn nav_count_right(entry: Rect) -> i32 {
    entry.right() - sc(NV_COUNT_PAD_RIGHT)
}

// ─────────────────────────────────────────────────────────────────────────────
// THE CONTENT COLUMN  —  `.main`, `.main-h`, `.fr`
// ─────────────────────────────────────────────────────────────────────────────

/// `.main { padding: 30px 32px 0 }` — note the zero at the bottom: the list runs to the edge of the
/// surface and is clipped there rather than stopping short of it.
const MAIN_PAD_TOP: i32 = 30;
const MAIN_PAD_X: i32 = 32;
/// `.main-h { margin-bottom: 26px }`
const HEAD_GAP_BELOW: i32 = 26;
/// `.main-h .s { margin-top: 5px }`
const SUB_GAP_ABOVE: i32 = 5;
/// `.fr { height: 44px }`
const FR_H: i32 = 44;
/// `.fr { padding: 0 12px; margin: 0 -12px }` — the row's hover plate hangs 12px outside the
/// content column on both sides, so the wash has air around the text instead of clipping it.
const FR_PAD_X: i32 = 12;
/// `.fr .ic` — 16px glyph, `margin-right: 15px`.
const FR_ICON: i32 = 16;
const FR_ICON_GAP: i32 = 15;
/// `.fr .mt.a { width: 92px }` — the size column.
const FR_META_W: i32 = 92;

/// `.fr { border-radius: 5px }`
const FR_RADIUS: i32 = 5;

/// The corner radius of a row's hover plate, in real pixels.
pub fn fr_radius() -> usize {
    crate::scale::upx(FR_RADIUS as usize)
}

/// The icon size a file row draws at, in real pixels — 16 at scale 1.0, and one of the sizes the
/// generator emits at every scale step.
pub fn fr_icon_px() -> usize {
    crate::scale::upx(FR_ICON as usize)
}

/// Everything the content column needs, resolved once per frame from the surface size.
///
/// One struct rather than eight functions taking `(w, h, scrollbar)` because every field is derived
/// from the same three numbers, and eight chances to pass a stale width is eight chances to draw a
/// column somewhere other than where the pointer thinks it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct List {
    /// The content column: where a row's icon, name and metadata sit.
    pub content: Rect,
    /// `.main-h .t` — the heading's line box.
    pub title: Rect,
    /// `.main-h .s` — the line under it.
    pub sub: Rect,
    /// The scrolling band. Rows are laid out from its top and clipped to it.
    pub band: Rect,
    /// One row's height, in real pixels.
    pub row_h: i32,
}

/// Lay out the content column for a surface `w` x `h`.
///
/// ⚠️ `scrollbar` is not cosmetic. The window server draws the scrollbar *over the right edge of
/// the app's own surface*, so a right-aligned size column laid out against the full width would sit
/// under it the moment the list got long enough to scroll — which is exactly when anyone would
/// notice. [`crate::layout::scrollbar_reserve`] is the width to keep clear, and it also covers the
/// resize band, where a click resizes the window rather than reaching the app.
pub fn list(w: i32, h: i32, scrollbar: bool) -> List {
    let bar = if scrollbar { crate::layout::scrollbar_reserve() } else { 0 };
    let x = sc(NAV_W) + sc(MAIN_PAD_X);
    let right = (w - sc(MAIN_PAD_X) - bar).max(x);

    let title_y = sc(MAIN_PAD_TOP);
    let title_h = D2.line() as i32;
    let sub_y = title_y + title_h + sc(SUB_GAP_ABOVE);
    let sub_h = B3.line() as i32;
    let band_y = sub_y + sub_h + sc(HEAD_GAP_BELOW);

    List {
        content: Rect::new(x, band_y, right - x, (h - band_y).max(0)),
        title: Rect::new(x, title_y, right - x, title_h),
        sub: Rect::new(x, sub_y, right - x, sub_h),
        band: Rect::new(x, band_y, right - x, (h - band_y).max(0)),
        row_h: sc(FR_H),
    }
}

/// The hover/selected plate for row `i`. Hangs `FR_PAD_X` outside the content column on both sides.
pub fn row_plate(l: &List, i: usize, scroll: i32) -> Rect {
    let pad = sc(FR_PAD_X);
    Rect::new(
        l.content.x - pad,
        l.band.y + i as i32 * l.row_h - scroll,
        l.content.w + 2 * pad,
        l.row_h,
    )
}

/// The row under `(mx, my)` in a list of `n` rows scrolled by `scroll`, or None.
///
/// Derived from [`row_plate`] — the plate the pointer is over is the plate that lit up.
pub fn row_at(l: &List, mx: i32, my: i32, scroll: i32, n: usize) -> Option<usize> {
    if my < l.band.y || my >= l.band.bottom() {
        return None;
    }
    let idx = (my - l.band.y + scroll) / l.row_h.max(1);
    if idx < 0 || idx as usize >= n {
        return None;
    }
    let i = idx as usize;
    row_plate(l, i, scroll).contains(mx, my).then_some(i)
}

/// The 16px icon box at the head of a row.
pub fn row_icon(l: &List, row: Rect) -> Rect {
    let s = fr_icon_px() as i32;
    Rect::new(l.content.x, row.y + (row.h - s) / 2, s, s)
}

/// Where a row's name starts, and how wide it may be before it collides with the size column.
/// Feed the width to [`crate::text::ellipsize`].
pub fn row_name(l: &List) -> (i32, i32) {
    let x = l.content.x + fr_icon_px() as i32 + sc(FR_ICON_GAP);
    (x, (l.content.right() - sc(FR_META_W) - x).max(0))
}

/// The right edge the size column is aligned to.
pub fn row_meta_right(l: &List) -> i32 {
    l.content.right()
}

/// The height `n` rows occupy. The real scrollable content.
pub fn rows_height(l: &List, n: usize) -> i32 {
    n as i32 * l.row_h
}

/// The largest useful scroll offset. Zero when everything fits.
pub fn max_scroll(l: &List, n: usize) -> i32 {
    (rows_height(l, n) - l.band.h).max(0)
}

/// ★ What an app must report as `NyxApp::content_height` — **the rows plus the header block above
/// them**, not [`rows_height`].
///
/// This is not a rounding choice, it is what makes the window server's scrollbar agree with the
/// app. The shell drives the bar against the surface, not against the band: it shows one when
/// `content_h > r.h`, and `mouse_to_scroll` maps a drag onto `0 ..= content_h - r.h`, where `r.h`
/// is the **whole window height**. The app's viewport is the band, which is `band.y` shorter.
///
/// Reporting the bare row height therefore gets both wrong in the same direction: the bar appears
/// late, and a full-length drag stops `band.y` pixels short of the last row — a list whose bottom
/// entries cannot be reached by the scrollbar, only by the keyboard. Adding the header back in
/// makes `content_h - r.h` come out at exactly [`max_scroll`], so the two ends of the track are the
/// two ends of the list, and `content_h > r.h` becomes exactly "the rows do not fit in the band".
///
/// The thumb ends up slightly shorter than the true viewport fraction, because the header is
/// counted as content that never scrolls. That is the cost of the range being exact, and it is the
/// cheaper of the two errors: a thumb a few pixels short is invisible, and a list you cannot scroll
/// to the end of is not.
pub fn reported_content_height(l: &List, n: usize) -> i32 {
    l.band.y + rows_height(l, n)
}

/// True once `n` rows overflow the band — i.e. once the window server will paint a scrollbar over
/// this app's right edge. Feed it to [`list`]'s `scrollbar` argument.
///
/// Derived from [`reported_content_height`] against the same comparison the shell makes, rather
/// than from a second inequality that happens to agree today.
pub fn needs_scrollbar(w: i32, h: i32, n: usize) -> bool {
    let bare = list(w, h, false);
    reported_content_height(&bare, n) > h
}

/// The strip above the band that a scrolled row overhangs into, and that must be repainted after
/// the rows are drawn.
///
/// ⚠️ **The band does not clip.** `Canvas` has no scissor, so a row scrolled half out of view draws
/// its wash, its icon and its name at negative offsets into the band — straight through the
/// subtitle and the heading above it. Every geometry test still passes, because the rows are
/// exactly where they should be; they are just also somewhere they should not be. Found by
/// rendering the frame (`examples/files_preview.rs`), not by asserting on it.
///
/// The fix is painter's order, not clipping: draw the rows, repaint this strip in the surface
/// colour, then draw the heading over it. That costs one `fill_rect` and gives a row a hard, square
/// cut at the top of the band, which is what a list scrolling under a header should look like.
/// Adding a scissor argument to `text::draw`, `text::icon` and `shapes::fill_round_rect` — three
/// functions the shell also draws every surface with — would be a much wider change for the same
/// picture.
///
/// Spans from the nav pane's edge to the surface's, so it also covers the row plate's 12px
/// overhang and any scrollbar the window server paints.
pub fn header_mask(l: &List, surface_w: i32) -> Rect {
    let x = sc(NAV_W);
    Rect::new(x, 0, (surface_w - x).max(0), l.band.y)
}

/// The rows that can contribute ink at `scroll`, as a half-open range. Drawing the other few
/// thousand costs a `with_glyph_face` lock and a measure each, per frame.
pub fn visible_rows(l: &List, scroll: i32, n: usize) -> (usize, usize) {
    if l.row_h <= 0 || n == 0 {
        return (0, 0);
    }
    let first = (scroll / l.row_h).max(0) as usize;
    let count = (l.band.h / l.row_h + 2) as usize;
    (first.min(n), (first + count).min(n))
}

// ─────────────────────────────────────────────────────────────────────────────
// THE BREADCRUMB
// ─────────────────────────────────────────────────────────────────────────────

/// The air on each side of a `/` between two crumbs. Without it a breadcrumb is indistinguishable
/// from the path written out, and nothing suggests the pieces are separately reachable.
const CRUMB_GAP: i32 = 7;

/// One piece of a breadcrumb.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CrumbKind {
    /// A reachable ancestor. The payload is the **byte length of the prefix** of the original path
    /// that this crumb navigates to, so a caller slices rather than rebuilding a string it would
    /// then have to keep in step with the one it measured.
    Segment(usize),
    /// The rule between two segments. Not a target.
    Separator,
}

/// Walk `path` as a breadcrumb, emitting `(kind, text, x, w)` for each piece left to right.
///
/// ★ This is an **extension to the design**, and a deliberate one. The mock's `.main-h .t` is the
/// folder's name — "Applications" — because the scene never navigates anywhere. The build order
/// says Files "loses its title bar and its toolbar; **keeps its navigation**", and deleting the
/// toolbar deletes the only way up. A heading whose ancestors are reachable is the same heading
/// with the navigation folded into it, which is cheaper than the surface the design removed.
///
/// ⚠️ `label(target, raw)` supplies the text a segment is **drawn** with, and it exists because a
/// file manager does not want to call `/mnt/nvme/apps` "apps" — the nav pane calls it
/// "Applications", and the heading has to agree with the nav. That means the displayed string is
/// not the string in the path, so it is resolved *here*, where the widths are measured. A caller
/// that measured the raw segment and drew a longer label would overlap the crumb beside it and
/// hit-test against a box that is not the one on screen. Pass `|_, raw| String::from(raw)` for a
/// literal path.
///
/// Both the painter and the hit-test walk this one function, for the reason the module header
/// gives: a crumb you can read but not click is not a crash, it is a thing that quietly does not
/// work.
/// `max_w` is the width the crumb trail must fit in — the heading box. When it does not fit,
/// **leading** crumbs are dropped until it does, and the final one is always kept: an ancestor you
/// cannot see is a navigation shortcut you lose, but a heading you cannot read is the name of the
/// folder you are looking at. Pass `i32::MAX` for an unbounded trail.
pub fn breadcrumb(
    path: &str,
    style: Style,
    label: impl Fn(usize, &str) -> String,
    max_w: i32,
    mut emit: impl FnMut(CrumbKind, &str, i32, i32),
) {
    // Build the trail first, because dropping from the front needs to know the total.
    let mut trail: Vec<(CrumbKind, String, i32)> = Vec::new();
    let mut push = |trail: &mut Vec<_>, k: CrumbKind, s: String, pad: i32| {
        let w = crate::text::width(style, &s) + 2 * pad;
        trail.push((k, s, w));
    };

    // The root is always reachable, and is drawn as whatever the caller calls it.
    push(&mut trail, CrumbKind::Segment(1), label(1, "/"), 0);

    let mut end = 0usize;
    let mut first = true;
    for part in path.split('/').filter(|p| !p.is_empty()) {
        // `end` walks the ORIGINAL string so the payload is a real prefix of it — separators in the
        // path may be doubled or trailing, and rejoining the parts would not reproduce them.
        end = match path[end..].find(part) {
            Some(off) => end + off + part.len(),
            None => break,
        };
        // ⚠️ No separator before the FIRST component. The root crumb is itself a "/", so emitting
        // one after it spells the heading "/ / mnt / nvme" — which the geometry tests cannot see,
        // because a doubled separator is perfectly laid out. Found by rendering it.
        if first {
            first = false;
            // The root still needs the air a separator would have carried, or it reads as "/mnt".
            if let Some(root) = trail.last_mut() {
                root.2 += sc(CRUMB_GAP);
            }
        } else {
            push(&mut trail, CrumbKind::Separator, String::from("/"), sc(CRUMB_GAP));
        }
        push(&mut trail, CrumbKind::Segment(end), label(end, part), 0);
    }

    // Drop from the front, in separator+segment pairs, never touching the last entry.
    let mut start = 0usize;
    while trail.len() - start > 1 && trail[start..].iter().map(|c| c.2).sum::<i32>() > max_w {
        start += 1;
    }

    let mut x = 0i32;
    for (k, s, w) in &trail[start..] {
        let pad = match k {
            CrumbKind::Separator => sc(CRUMB_GAP),
            CrumbKind::Segment(_) => 0,
        };
        emit(*k, s, x + pad, crate::text::width(style, s));
        x += w;
    }
}

/// The crumb whose target is under `mx`, given the same arguments it was drawn with. Separators are
/// not targets.
pub fn crumb_at(
    path: &str,
    style: Style,
    label: impl Fn(usize, &str) -> String,
    max_w: i32,
    origin_x: i32,
    mx: i32,
) -> Option<usize> {
    let mut hit = None;
    breadcrumb(path, style, label, max_w, |k, _, x, w| {
        if let CrumbKind::Segment(target) = k {
            if mx >= origin_x + x && mx < origin_x + x + w {
                hit = Some(target);
            }
        }
    });
    hit
}

/// The `label` argument for a breadcrumb that spells the path literally.
pub fn raw_label(_target: usize, raw: &str) -> String {
    String::from(raw)
}

// ─────────────────────────────────────────────────────────────────────────────
// THE METRIC GRID  —  `.mg`, `.mc`, `.mc .k`, `.mc .v`, `.mc .sub`, `.spark`
// ─────────────────────────────────────────────────────────────────────────────
//
// System Monitor's whole interior: a 2x2 grid of cells, each a label, one large number and either a
// sub-line or a sparkline. The design's note calls it "the cheapest window in the system" — under 40
// glyph quads and 18 fill_rects for the entire frame — and that is a constraint worth keeping, not a
// boast. Nothing here allocates or loops over pixels.

/// `.mg { grid-template-columns: 1fr 1fr }` — two columns, and therefore two rows for four metrics.
const MG_COLS: i32 = 2;
/// `.mc { padding: 34px 34px 32px }`
const MC_PAD_X: i32 = 34;
const MC_PAD_TOP: i32 = 34;
const MC_PAD_BOTTOM: i32 = 32;
/// `.mc .k { margin-bottom: 14px }`
const MC_K_GAP: i32 = 14;
/// `.mc .sub { margin-top: 12px }`
const MC_SUB_GAP: i32 = 12;
/// `.spark { height: 26px; gap: 2px }`, `.spark i { width: 2px }`
const SPARK_H: i32 = 26;
const SPARK_BAR_W: i32 = 2;
const SPARK_BAR_GAP: i32 = 2;
/// The design draws 18 bars. That is the number, not a maximum — it is what makes the sparkline the
/// width it is, and a metric with fewer samples so far should show fewer bars, not rescale.
pub const SPARK_BARS: usize = 18;

/// One cell of the metric grid: the rects a card is drawn from, in surface coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    /// The whole cell, including its padding. This is the hover target.
    pub rect: Rect,
    /// `.mc .k` — the metric's name, at the top left of the content box.
    pub label: Rect,
    /// `.mc .v` — the large number's box. `text::centre_y` is wrong here: the design aligns these on
    /// a *baseline*, and the unit suffix shares it.
    pub value: Rect,
    /// `.mc .sub` / `.spark` — the third line, whichever the metric has.
    ///
    /// One origin for both, which is a deliberate 4px departure: the design gives `.sub` a 12px top
    /// margin and `.spark` a 16px one, but those sit on *different* cards there. Here the same card
    /// swaps its sub-line for a sparkline when the pointer enters it, and honouring both margins
    /// would make the line jump 4px on hover — trading a static 4px against a moving one.
    pub detail: Rect,
    /// True when this cell owes a right-hand hairline: `.mc { border-right }` with
    /// `.mc:nth-child(2n) { border-right: 0 }`, i.e. every cell that is not in the last column.
    pub rule_right: bool,
    /// True for cells in the bottom row. `.mc { border-bottom }` applies to all of them, but the
    /// last row's rule is shared with whatever sits under the grid, so the app decides who draws it.
    pub last_row: bool,
}

/// The metric grid's cell `i` (row-major) in a surface `w` wide, for `n` metrics.
///
/// Column widths come from integer division, and the last column absorbs the remainder rather than
/// every column being `w / 2`. On an odd width the alternative leaves a 1px column of surface down
/// the right edge of the window — which looks exactly like a rendering bug, and is the sort of thing
/// that is invisible at 1440 and obvious at 1441.
pub fn metric_cell(w: i32, n: usize, i: usize) -> Option<Cell> {
    if i >= n || n == 0 || w <= 0 {
        return None;
    }
    let rows = (n as i32 + MG_COLS - 1) / MG_COLS;
    let col = i as i32 % MG_COLS;
    let row = i as i32 / MG_COLS;

    let col_w = w / MG_COLS;
    let x = col * col_w;
    let cw = if col == MG_COLS - 1 { w - x } else { col_w };
    let ch = metric_cell_height();
    let y = row * ch;

    let pad_x = sc(MC_PAD_X);
    let inner_x = x + pad_x;
    let inner_w = (cw - pad_x * 2).max(0);

    let mut cy = y + sc(MC_PAD_TOP);
    let label = Rect::new(inner_x, cy, inner_w, crate::text::line_box(B3));
    cy += label.h + sc(MC_K_GAP);
    let value = Rect::new(inner_x, cy, inner_w, D1.line() as i32);
    cy += value.h + sc(MC_SUB_GAP);
    let detail = Rect::new(inner_x, cy, inner_w, sc(SPARK_H).max(crate::text::line_box(B3)));

    Some(Cell {
        rect: Rect::new(x, y, cw, ch),
        label,
        value,
        detail,
        // `:nth-child(2n)` is 1-based over the whole grid, which for a 2-column grid is exactly
        // "the cell in the right-hand column".
        rule_right: col != MG_COLS - 1,
        last_row: row == rows - 1,
    })
}

/// The height of every cell: padding, label, number, detail, padding. Uniform by construction —
/// a grid row is as tall as its tallest cell, and giving a metric with a sparkline more room than
/// one with a sub-line would break the alignment of the four numbers, which is the whole composition.
pub fn metric_cell_height() -> i32 {
    sc(MC_PAD_TOP)
        + crate::text::line_box(B3)
        + sc(MC_K_GAP)
        + D1.line() as i32
        + sc(MC_SUB_GAP)
        + sc(SPARK_H).max(crate::text::line_box(B3))
        + sc(MC_PAD_BOTTOM)
}

/// The full height of a grid of `n` metrics.
pub fn metric_grid_height(n: usize) -> i32 {
    if n == 0 {
        return 0;
    }
    let rows = (n as i32 + MG_COLS - 1) / MG_COLS;
    rows * metric_cell_height()
}

/// The metric cell under `(mx, my)`, or None. Drives the sparkline: the design shows one "on the
/// metric under the pointer and nowhere else".
pub fn metric_hit(w: i32, n: usize, mx: i32, my: i32) -> Option<usize> {
    (0..n).find(|&i| metric_cell(w, n, i).is_some_and(|c| c.rect.contains(mx, my)))
}

/// Bar `i` of a sparkline in `detail`, given `v` in 0.0..=1.0. `i` is the sample's **age** — 0 is
/// the newest.
///
/// The run is **left-aligned in `detail`**, under the number, exactly where `.spark` sits in the
/// design: it is a plain flex row inside `.mc`, not something floated to the cell's edge. Within
/// that fixed 18-slot run the newest sample is the RIGHTMOST, because a sparkline reads
/// oldest-to-newest left to right.
///
/// So age `i` occupies slot `SPARK_BARS - 1 - i`. A series that has not filled yet leaves its gap
/// on the LEFT, which is the honest place for it — the missing bars are the samples not taken yet,
/// and the current value stays put at the right instead of sliding across the box as history
/// accumulates.
///
/// A bar is never shorter than 1px. A zero-height rect draws nothing, and a flat-zero metric would
/// render as an empty box that looks like a broken sparkline rather than a quiet one.
pub fn spark_bar(detail: Rect, i: usize, v: f32) -> Option<Rect> {
    if i >= SPARK_BARS {
        return None;
    }
    let bw = sc(SPARK_BAR_W);
    let step = bw + sc(SPARK_BAR_GAP);
    let slot = SPARK_BARS as i32 - 1 - i as i32;
    let x = detail.x + slot * step;
    if x + bw > detail.right() {
        return None;
    }
    let full = sc(SPARK_H);
    let h = ((v.clamp(0.0, 1.0) * full as f32) as i32).clamp(1, full);
    // `align-items: flex-end` — bars hang from the bottom of the 26px box.
    Some(Rect::new(x, detail.y + full - h, bw, h))
}

/// The width a full sparkline occupies, for a caller that wants to know whether one fits.
pub fn spark_width() -> i32 {
    (sc(SPARK_BAR_W) + sc(SPARK_BAR_GAP)) * SPARK_BARS as i32 - sc(SPARK_BAR_GAP)
}

// ─────────────────────────────────────────────────────────────────────────────
// THE SETTINGS LIST  —  `.sh`, `.sr`, `.sr .k`, `.sr .k .d`, `.sr .v`, `.tog`, `.seg`
// ─────────────────────────────────────────────────────────────────────────────
//
// "Rows are label, description and control, separated by hairlines and space." Reuses the `.nav`
// pane above unchanged — Settings and Files are the same two-column component with different
// contents, which is the argument for `panes` existing at all.

/// `.sh { font: lb; margin: 38px 0 6px }`, and `:first-child { margin-top: 0 }`.
const SH_GAP_ABOVE: i32 = 38;
const SH_GAP_BELOW: i32 = 6;
/// `.sr { min-height: 62px; border-bottom: 1px solid var(--line) }`
const SR_MIN_H: i32 = 62;
// Note: `.sr .k` is `font-size: 14px` in the sheet, which is not a step on the design's own type
// scale — `b1` is 15 and `b2` is 13. Rows are set in `b1`. Following the scale is the more faithful
// reading of a design whose whole argument is that the scale is the system.
/// `.sr .k .d { margin-top: 3px; line-height: 17px }` — the description under the label.
const SR_DESC_GAP: i32 = 3;
/// `.sr { gap: 24px }` — the least air between the label block and the control.
const SR_GAP: i32 = 24;
/// `.tog { width: 32px; height: 18px; border-radius: 9px }` and its 14px knob at 2px inset.
const TOG_W: i32 = 32;
const TOG_H: i32 = 18;
const TOG_KNOB: i32 = 14;
const TOG_INSET: i32 = 2;
/// `.seg > div { padding: 0 13px; height: 26px; border-radius: 4px }`, `.seg { gap: 2px }`
const SEG_H: i32 = 26;
const SEG_PAD_X: i32 = 13;
const SEG_GAP: i32 = 2;
const SEG_RADIUS: i32 = 4;

/// One entry in the settings column. Same flat-list argument as [`NavKind`]: a heading and a row
/// cannot get out of order between layout and drawing if there is only one list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKind {
    /// `.sh` — an uppercase section heading. Not interactive.
    Heading,
    /// `.sr` with a label only.
    Row,
    /// `.sr` whose label carries a `.d` description under it, making the row taller.
    RowWithDesc,
}

/// The rects a settings row is drawn from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SettingRow {
    /// The whole row, hairline included. The hover/click target.
    pub rect: Rect,
    /// `.sr .k` — the label's line box.
    pub label: Rect,
    /// `.sr .k .d` — the description, or a zero-height rect when there is none.
    pub desc: Rect,
    /// Where a right-aligned control ends. The control itself is laid out from here leftward.
    pub value_right: i32,
    /// `.sr { border-bottom }` — false on the last row of a section, per `:last-child`.
    pub rule: bool,
}

/// Height of one entry, without walking the list.
fn setting_entry_h(kind: RowKind, first: bool) -> i32 {
    match kind {
        RowKind::Heading => {
            let above = if first { 0 } else { sc(SH_GAP_ABOVE) };
            above + crate::text::line_box(LB) + sc(SH_GAP_BELOW)
        }
        // The design's 62px minimum already clears a label; a description pushes past it.
        RowKind::Row => sc(SR_MIN_H),
        RowKind::RowWithDesc => sc(SR_MIN_H).max(
            crate::text::line_box(B1) + sc(SR_DESC_GAP) + crate::text::line_box(B3) + sc(SR_GAP),
        ),
    }
}

/// Entry `i` of the settings column, in a content area `w` wide starting at `x`.
///
/// Walked from the top like [`nav_entry`], and for the same reason: a heading's height depends on
/// whether anything precedes it, so no entry's position is a closed-form function of its index.
pub fn setting_row(kinds: &[RowKind], w: i32, x: i32, i: usize) -> Option<SettingRow> {
    if i >= kinds.len() {
        return None;
    }
    let mut y = 0;
    for (j, &k) in kinds.iter().enumerate() {
        let h = setting_entry_h(k, j == 0);
        if j == i {
            let rect = Rect::new(x, y, w, h);
            let (label, desc) = match k {
                RowKind::Heading => (
                    Rect::new(x, y, w, crate::text::line_box(LB)),
                    Rect::new(x, y, 0, 0),
                ),
                RowKind::Row => {
                    let lb = crate::text::line_box(B1);
                    (Rect::new(x, y + (h - lb) / 2, w, lb), Rect::new(x, y, 0, 0))
                }
                RowKind::RowWithDesc => {
                    // Label and description are centred as one block, so a row with a description
                    // and one without still read as the same rhythm down the column.
                    let lb = crate::text::line_box(B1);
                    let db = crate::text::line_box(B3);
                    let block = lb + sc(SR_DESC_GAP) + db;
                    let top = y + (h - block) / 2;
                    (
                        Rect::new(x, top, w, lb),
                        Rect::new(x, top + lb + sc(SR_DESC_GAP), w, db),
                    )
                }
            };
            // `:last-child` — and a heading never draws one.
            let last_in_section = kinds
                .get(j + 1)
                .map_or(true, |n| *n == RowKind::Heading);
            return Some(SettingRow {
                rect,
                label,
                desc,
                value_right: x + w,
                rule: k != RowKind::Heading && !last_in_section,
            });
        }
        y += h;
    }
    None
}

/// The settings entry under `(mx, my)`, or None. Headings are never hit — they are not controls.
pub fn setting_hit(kinds: &[RowKind], w: i32, x: i32, mx: i32, my: i32) -> Option<usize> {
    (0..kinds.len()).find(|&i| {
        kinds[i] != RowKind::Heading
            && setting_row(kinds, w, x, i).is_some_and(|r| r.rect.contains(mx, my))
    })
}

/// Total height of the settings column.
pub fn settings_height(kinds: &[RowKind]) -> i32 {
    kinds
        .iter()
        .enumerate()
        .map(|(j, &k)| setting_entry_h(k, j == 0))
        .sum()
}

/// The `.tog` track, right-aligned to `right` and centred in `row`.
pub fn toggle(row: Rect, right: i32) -> Rect {
    let (w, h) = (sc(TOG_W), sc(TOG_H));
    Rect::new(right - w, row.y + (row.h - h) / 2, w, h)
}

/// The `.tog` knob, for a toggle drawn at `track`.
///
/// `.tog::after { left: 2px }` and `.tog.on::after { left: 16px }` — the ON position is derived from
/// the track rather than written as 16, so the two cannot disagree if the toggle is ever resized.
pub fn toggle_knob(track: Rect, on: bool) -> Rect {
    let k = sc(TOG_KNOB);
    let inset = sc(TOG_INSET);
    let x = if on { track.right() - inset - k } else { track.x + inset };
    Rect::new(x, track.y + (track.h - k) / 2, k, k)
}

/// The `.seg` segmented control: rect of segment `i`, right-aligned to `right` and centred in `row`.
///
/// Each segment is sized to its own label, which is what the design's `padding: 0 13px` means. A
/// fixed segment width would make "Auto" and "Light" the same size and the control would stop
/// looking like text with a mark behind it.
pub fn segment(row: Rect, right: i32, widths: &[i32], i: usize) -> Option<Rect> {
    if i >= widths.len() {
        return None;
    }
    let pad = sc(SEG_PAD_X) * 2;
    let gap = sc(SEG_GAP);
    let total: i32 =
        widths.iter().map(|w| w + pad).sum::<i32>() + gap * (widths.len() as i32 - 1).max(0);
    let h = sc(SEG_H);
    let mut x = right - total;
    for (j, w) in widths.iter().enumerate() {
        let seg_w = w + pad;
        if j == i {
            return Some(Rect::new(x, row.y + (row.h - h) / 2, seg_w, h));
        }
        x += seg_w + gap;
    }
    None
}

/// The segment under `mx`, given the same inputs [`segment`] was laid out with.
pub fn segment_hit(row: Rect, right: i32, widths: &[i32], mx: i32, my: i32) -> Option<usize> {
    (0..widths.len()).find(|&i| segment(row, right, widths, i).is_some_and(|r| r.contains(mx, my)))
}

/// `.seg > div { border-radius: 4px }`
pub fn segment_radius() -> usize {
    sc(SEG_RADIUS) as usize
}

/// `.trk { height: 2px; border-radius: 1px }` — the measurement track, used here for brightness.
const TRK_H: i32 = 2;
/// How wide a track is drawn in a settings row. The design's own `.trk` instances are 130px.
const TRK_W: i32 = 130;
/// How far above and below the 2px ink still counts as grabbing it. A 2px drag target is not one.
const TRK_GRAB_PAD: i32 = 11;

/// The `.trk` track, right-aligned to `right` and centred in `row`.
pub fn track(row: Rect, right: i32) -> Rect {
    let (w, h) = (sc(TRK_W), sc(TRK_H));
    Rect::new(right - w, row.y + (row.h - h) / 2, w, h)
}

/// `.trk .f` — the filled portion, for `pct` of 100.
pub fn track_fill(track: Rect, pct: i32) -> Rect {
    Rect::new(track.x, track.y, track.w * pct.clamp(0, 100) / 100, track.h)
}

/// True if `(mx, my)` is close enough to `track` to start dragging it.
///
/// Padded generously in y — the ink is two pixels tall, and a control you have to hit exactly is a
/// control nobody uses. Padded in x too, so the two ends are reachable rather than being a
/// one-pixel-wide chance to set 0% or 100%.
pub fn track_hit(track: Rect, mx: i32, my: i32) -> bool {
    let pad = sc(TRK_GRAB_PAD);
    mx >= track.x - pad && mx <= track.right() + pad && my >= track.y - pad && my <= track.bottom() + pad
}

/// The percentage a press/drag at `mx` means. Clamped, so a drag that runs off either end pins
/// rather than wrapping.
pub fn track_pct(track: Rect, mx: i32) -> i32 {
    if track.w <= 0 {
        return 0;
    }
    ((mx - track.x) * 100 / track.w).clamp(0, 100)
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    use crate::font::register_text;
    use crate::text::width;

    const W: i32 = 596; // the design's 896px window minus its 198px nav and padding
    const X: i32 = 236;

    fn kinds() -> alloc::vec::Vec<RowKind> {
        alloc::vec![
            RowKind::Heading,
            RowKind::RowWithDesc,
            RowKind::RowWithDesc,
            RowKind::Heading,
            RowKind::RowWithDesc,
            RowKind::Row,
        ]
    }

    #[test]
    fn entries_stack_without_gaps_or_overlap() {
        register_text();
        let k = kinds();
        let mut y = 0;
        for i in 0..k.len() {
            let r = setting_row(&k, W, X, i).unwrap();
            assert_eq!(r.rect.y, y, "entry {} does not start where {} ended", i, i - 1);
            assert!(r.rect.h > 0);
            y = r.rect.bottom();
        }
        assert_eq!(settings_height(&k), y);
        assert_eq!(setting_row(&k, W, X, k.len()), None);
    }

    /// `.sh:first-child { margin-top: 0 }` — a leading heading must not push the whole column down.
    #[test]
    fn the_first_heading_has_no_space_above_it() {
        register_text();
        let k = kinds();
        assert_eq!(setting_row(&k, W, X, 0).unwrap().rect.y, 0);
        let second = setting_row(&k, W, X, 3).unwrap();
        let prev = setting_row(&k, W, X, 2).unwrap();
        assert!(
            second.rect.y == prev.rect.bottom() && second.rect.h > setting_entry_h(RowKind::Heading, true),
            "a later heading must carry its 38px of air"
        );
    }

    /// `.sr:last-child { border-bottom: 0 }`. The rule under the final row of a section would double
    /// with the space before the next heading and read as a boxed group.
    #[test]
    fn the_last_row_of_a_section_draws_no_rule() {
        register_text();
        let k = kinds();
        assert!(!setting_row(&k, W, X, 0).unwrap().rule, "a heading never rules");
        assert!(setting_row(&k, W, X, 1).unwrap().rule);
        assert!(!setting_row(&k, W, X, 2).unwrap().rule, "last before a heading");
        assert!(setting_row(&k, W, X, 4).unwrap().rule);
        assert!(!setting_row(&k, W, X, 5).unwrap().rule, "last in the whole column");
    }

    #[test]
    fn a_label_and_its_description_sit_inside_their_row() {
        register_text();
        let k = kinds();
        for i in 0..k.len() {
            let r = setting_row(&k, W, X, i).unwrap();
            assert!(r.label.y >= r.rect.y, "entry {}: label above its row", i);
            assert!(r.label.bottom() <= r.rect.bottom(), "entry {}: label below its row", i);
            if k[i] == RowKind::RowWithDesc {
                assert!(r.desc.y >= r.label.bottom(), "entry {}: description overlaps the label", i);
                assert!(r.desc.bottom() <= r.rect.bottom(), "entry {}: description below its row", i);
            } else {
                assert_eq!(r.desc.h, 0, "entry {} has no description", i);
            }
        }
    }

    #[test]
    fn only_rows_are_clickable_and_the_click_lands_on_the_row_under_the_pointer() {
        register_text();
        let k = kinds();
        for i in 0..k.len() {
            let r = setting_row(&k, W, X, i).unwrap();
            let mid = (X + W / 2, r.rect.y + r.rect.h / 2);
            let got = setting_hit(&k, W, X, mid.0, mid.1);
            if k[i] == RowKind::Heading {
                assert_ne!(got, Some(i), "heading {} is not a control", i);
            } else {
                assert_eq!(got, Some(i), "row {} missed", i);
            }
        }
        assert_eq!(setting_hit(&k, W, X, X - 1, 30), None, "left of the column");
        assert_eq!(setting_hit(&k, W, X, X + 10, settings_height(&k)), None, "below it");
    }

    /// The knob has to be inside the track in BOTH states, or the control reads as broken at one end.
    #[test]
    fn the_toggle_knob_stays_inside_its_track_both_ways() {
        register_text();
        let row = setting_row(&kinds(), W, X, 1).unwrap();
        let t = toggle(row.rect, row.value_right);
        assert_eq!(t.right(), row.value_right, "the control is right-aligned");
        for on in [false, true] {
            let k = toggle_knob(t, on);
            assert!(k.x >= t.x && k.right() <= t.right(), "knob escapes horizontally when on={}", on);
            assert!(k.y >= t.y && k.bottom() <= t.bottom(), "knob escapes vertically when on={}", on);
        }
        assert!(toggle_knob(t, true).x > toggle_knob(t, false).x, "the knob must actually move");
        // Symmetric: the same 2px margin at each end, or the ON state looks like a mistake.
        let (off, on) = (toggle_knob(t, false), toggle_knob(t, true));
        assert_eq!(off.x - t.x, t.right() - on.right());
    }

    #[test]
    fn segments_tile_right_aligned_without_overlapping() {
        register_text();
        let row = setting_row(&kinds(), W, X, 1).unwrap();
        let labels = ["Light", "Dark", "Auto"];
        let widths: alloc::vec::Vec<i32> = labels.iter().map(|s| width(B2, s)).collect();
        let mut prev_right = None;
        for i in 0..labels.len() {
            let s = segment(row.rect, row.value_right, &widths, i).unwrap();
            assert!(s.w > widths[i], "segment {} is narrower than its label", i);
            if let Some(p) = prev_right {
                assert!(s.x >= p, "segment {} overlaps the one before it", i);
            }
            prev_right = Some(s.right());
            assert!(s.y >= row.rect.y && s.bottom() <= row.rect.bottom());
        }
        assert_eq!(prev_right, Some(row.value_right), "the control is right-aligned");
        assert_eq!(segment(row.rect, row.value_right, &widths, 3), None);
    }

    /// The fill has to agree with the number beside it at both ends and in the middle, and a drag
    /// has to give back the percentage it was drawn at — otherwise the bar creeps under the pointer.
    #[test]
    fn a_track_reads_back_the_percentage_it_was_drawn_at() {
        register_text();
        let row = setting_row(&kinds(), W, X, 1).unwrap();
        let t = track(row.rect, row.value_right);
        assert_eq!(t.right(), row.value_right, "the control is right-aligned");
        assert_eq!(track_fill(t, 0).w, 0);
        assert_eq!(track_fill(t, 100).w, t.w);
        assert!(track_fill(t, 50).w > 0 && track_fill(t, 50).w < t.w);
        // Clamped, never wrapped: a drag past either end pins.
        assert_eq!(track_fill(t, -20).w, 0);
        assert_eq!(track_fill(t, 400).w, t.w);
        assert_eq!(track_pct(t, t.x - 50), 0);
        assert_eq!(track_pct(t, t.right() + 50), 100);
        // Round-tripping: where the fill ends is where that percentage reads back.
        for pct in [0, 1, 25, 50, 99, 100] {
            let end = track_fill(t, pct).right();
            let back = track_pct(t, end);
            assert!((back - pct).abs() <= 1, "{}% drew to x={} which reads back as {}%", pct, end, back);
        }
    }

    #[test]
    fn a_two_pixel_track_is_still_grabbable() {
        register_text();
        let row = setting_row(&kinds(), W, X, 1).unwrap();
        let t = track(row.rect, row.value_right);
        assert_eq!(t.h, sc(TRK_H), "the ink is a hairline");
        assert!(track_hit(t, t.x + t.w / 2, t.y), "dead centre");
        assert!(track_hit(t, t.x + t.w / 2, t.y - sc(8)), "a few px above the 2px ink");
        assert!(track_hit(t, t.x + t.w / 2, t.bottom() + sc(8)), "and below");
        assert!(track_hit(t, t.x - sc(4), t.y), "the 0% end is reachable");
        assert!(track_hit(t, t.right() + sc(4), t.y), "and the 100% end");
        assert!(!track_hit(t, t.x - sc(40), t.y), "but not unboundedly");
    }

    /// Draw and hit-test share one function, so the segment that lights up is the one that acts.
    #[test]
    fn clicking_a_segment_selects_the_one_it_is_drawn_over() {
        register_text();
        let row = setting_row(&kinds(), W, X, 1).unwrap();
        let widths = [40, 36, 38];
        for i in 0..widths.len() {
            let s = segment(row.rect, row.value_right, &widths, i).unwrap();
            for mx in s.x..s.right() {
                assert_eq!(
                    segment_hit(row.rect, row.value_right, &widths, mx, s.y + s.h / 2),
                    Some(i),
                    "x={} should be segment {}",
                    mx, i
                );
            }
        }
    }
}

#[cfg(test)]
mod metric_tests {
    use super::*;
    use crate::font::register_text;

    const W: i32 = 640; // the design's System Monitor window

    #[test]
    fn the_four_cells_tile_the_width_exactly_and_never_overlap() {
        register_text();
        for w in [321, 400, 640, 641, 700, 1088, 1441] {
            let mut covered = alloc::vec![false; w as usize];
            for i in 0..4 {
                let c = metric_cell(w, 4, i).expect("four cells for four metrics");
                for x in c.rect.x..c.rect.right() {
                    // Only the top row needs to tile; the bottom row tiles the same columns.
                    if i < 2 {
                        assert!(!covered[x as usize], "w={}: column {} covered twice", w, x);
                        covered[x as usize] = true;
                    }
                }
            }
            assert!(covered.iter().all(|&c| c), "w={}: the top row leaves a gap", w);
        }
    }

    /// The composition is four numbers on two shared baselines. If a cell's rows were derived from
    /// its own content the two cards in a row would drift apart the moment one had a longer label.
    #[test]
    fn every_cell_puts_its_number_on_the_same_line_as_its_neighbour() {
        register_text();
        let rows: alloc::vec::Vec<i32> = (0..4)
            .map(|i| {
                let c = metric_cell(W, 4, i).unwrap();
                c.value.y - c.rect.y
            })
            .collect();
        assert!(rows.iter().all(|&y| y == rows[0]), "value baselines drift: {:?}", rows);
    }

    #[test]
    fn only_the_left_column_carries_a_rule_and_only_the_bottom_row_is_last() {
        register_text();
        let c: alloc::vec::Vec<Cell> = (0..4).map(|i| metric_cell(W, 4, i).unwrap()).collect();
        assert!(c[0].rule_right && !c[1].rule_right, "the right-hand cell must not draw a rule");
        assert!(c[2].rule_right && !c[3].rule_right);
        assert!(!c[0].last_row && !c[1].last_row);
        assert!(c[2].last_row && c[3].last_row, "the bottom row owns the shared rule");
    }

    #[test]
    fn a_cells_content_stays_inside_its_own_padding() {
        register_text();
        for w in [400, 640, 1088] {
            for i in 0..4 {
                let c = metric_cell(w, 4, i).unwrap();
                for (name, r) in [("label", c.label), ("value", c.value), ("detail", c.detail)] {
                    assert!(r.x >= c.rect.x, "w={} cell {}: {} starts left of the cell", w, i, name);
                    assert!(r.right() <= c.rect.right(), "w={} cell {}: {} overflows right", w, i, name);
                    assert!(r.y >= c.rect.y, "w={} cell {}: {} above the cell", w, i, name);
                    assert!(r.y + r.h <= c.rect.bottom(), "w={} cell {}: {} below the cell", w, i, name);
                }
            }
        }
    }

    #[test]
    fn the_grid_height_is_what_the_cells_actually_occupy() {
        register_text();
        for n in 1..=4 {
            let last = metric_cell(W, n, n - 1).unwrap();
            assert_eq!(metric_grid_height(n), last.rect.bottom(), "n={}", n);
        }
        assert_eq!(metric_grid_height(0), 0);
    }

    /// The sparkline reads oldest-to-newest left to right, so the *newest* sample (age 0) is the
    /// rightmost bar and increasing age walks leftward. The run itself starts at the left edge of
    /// the detail box, under the number — not floated to the cell's edge.
    #[test]
    fn sparkline_bars_march_left_from_the_newest_sample() {
        register_text();
        let d = metric_cell(W, 4, 0).unwrap().detail;
        let mut prev_x = i32::MAX;
        for i in 0..SPARK_BARS {
            let b = spark_bar(d, i, 0.5).unwrap_or_else(|| panic!("bar {} does not fit", i));
            assert!(b.x < prev_x, "bar {} is not left of the one before it", i);
            assert!(b.right() <= d.right(), "bar {} overflows the detail box", i);
            assert!(b.x >= d.x, "bar {} starts left of the detail box", i);
            prev_x = b.x;
        }
        assert_eq!(spark_bar(d, SPARK_BARS, 0.5), None, "there is no bar past the series");

        // ★ The oldest bar sits ON the left edge — the run is left-aligned in the cell, sharing the
        // number's x. Getting this backwards puts the whole sparkline over at the cell's right
        // margin, detached from the metric it belongs to. Every geometry assertion above still
        // passes when it is: the bars tile perfectly, just in the wrong place.
        assert_eq!(spark_bar(d, SPARK_BARS - 1, 0.5).unwrap().x, d.x);
    }

    /// A half-filled series must leave its gap on the LEFT and keep the current value where it will
    /// still be once the history fills, rather than sliding across the box for eighteen seconds.
    #[test]
    fn a_partial_series_keeps_the_newest_sample_in_its_final_place() {
        register_text();
        let d = metric_cell(W, 4, 0).unwrap().detail;
        let newest = spark_bar(d, 0, 0.5).unwrap();
        assert_eq!(newest.right(), d.x + spark_width(), "the newest bar ends the run");
        // Drawing only three samples must not move it.
        for samples in [1usize, 3, SPARK_BARS] {
            assert_eq!(spark_bar(d, 0, 0.5).unwrap(), newest, "{} samples moved bar 0", samples);
        }
    }

    /// A quiet metric must still look like a sparkline, not like a broken one.
    #[test]
    fn a_zero_sample_still_draws_one_pixel_and_a_full_one_fills_the_box() {
        register_text();
        let d = metric_cell(W, 4, 0).unwrap().detail;
        let zero = spark_bar(d, 0, 0.0).unwrap();
        assert_eq!(zero.h, 1, "a zero bar must still be visible");
        let full = spark_bar(d, 0, 1.0).unwrap();
        assert_eq!(full.h, sc(SPARK_H));
        assert_eq!(full.y, d.y, "a full bar reaches the top of the 26px box");
        // Clamped, not wrapped: a metric that briefly reports >100% must not draw upward out of
        // its cell and through the number above it.
        assert_eq!(spark_bar(d, 0, 9.0).unwrap(), full);
        assert_eq!(spark_bar(d, 0, -1.0).unwrap(), zero);
        // Every bar hangs from the same floor.
        assert_eq!(zero.bottom(), full.bottom());
    }

    #[test]
    fn a_full_sparkline_fits_the_cell_it_is_drawn_in() {
        register_text();
        assert!(
            spark_width() <= metric_cell(W, 4, 0).unwrap().detail.w,
            "18 bars do not fit the design's own 640px window"
        );
    }

    /// Draw and hit-test come from one function, so the card that lights up is the card under the
    /// pointer — the sparkline appears on "the one metric under the pointer and nowhere else".
    #[test]
    fn hovering_anywhere_in_a_cell_selects_that_cell_and_no_other() {
        register_text();
        for i in 0..4 {
            let c = metric_cell(W, 4, i).unwrap();
            for &(mx, my) in &[
                (c.rect.x, c.rect.y),
                (c.rect.right() - 1, c.rect.bottom() - 1),
                (c.rect.x + c.rect.w / 2, c.rect.y + c.rect.h / 2),
            ] {
                assert_eq!(metric_hit(W, 4, mx, my), Some(i), "cell {} at ({},{})", i, mx, my);
            }
        }
        assert_eq!(metric_hit(W, 4, W / 2, metric_grid_height(4)), None, "below the grid is nothing");
        assert_eq!(metric_hit(W, 4, -1, 10), None);
    }

    #[test]
    fn an_odd_number_of_metrics_still_lays_out() {
        register_text();
        let c = metric_cell(W, 3, 2).expect("the third of three sits alone on row two");
        assert_eq!(c.rect.x, 0, "an orphan cell starts a new row rather than centring");
        assert!(c.rule_right, "it is still a left-column cell, so it still owes its rule");
        assert_eq!(metric_cell(W, 3, 3), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::register_text;
    use crate::text::width;
    use alloc::vec::Vec;

    /// The pane from the hero scene: two groups, five places, two devices.
    fn hero() -> Vec<NavKind> {
        use NavKind::*;
        alloc::vec![Group, Item, Item, Item, Item, Item, Group, Item, Item]
    }

    /// Transcribed straight off `04-hero.html` + the `.nav` rules. If these move, the pane no
    /// longer matches the document.
    #[test]
    fn the_nav_column_stacks_exactly_as_the_css_says() {
        let k = hero();
        let y = |i: usize| nav_entry(&k, i).unwrap().y;
        assert_eq!(y(0), 30, "first .lb sits at the pane's padding-top, with no margin-top");
        assert_eq!(y(1), 60, "30 + 14 (lb line box) + 16 (margin-bottom)");
        assert_eq!(y(2), 90);
        assert_eq!(y(5), 180, "the last row of Locations");
        assert_eq!(y(6), 242, "210 + 32, the :not(:first-child) margin-top");
        assert_eq!(y(7), 272, "242 + 14 + 16");
        assert_eq!(y(8), 302);
    }

    #[test]
    fn nav_entries_are_indented_inside_the_pane() {
        let k = hero();
        let heading = nav_entry(&k, 0).unwrap();
        let row = nav_entry(&k, 1).unwrap();
        assert_eq!(heading.x, 30, ".nav padding-left");
        assert_eq!(row.x, 30);
        assert_eq!(nav_label_x(row), 42, ".nv padding-left, so a row is indented past its heading");
        assert_eq!(row.right(), 208, "rows run to the pane's edge — padding-right is 0");
        assert_eq!(nav_count_right(row), 186);
    }

    #[test]
    fn the_active_bar_is_two_pixels_inset_top_and_bottom() {
        let row = nav_entry(&hero(), 1).unwrap();
        let bar = nav_bar(row);
        assert_eq!((bar.x, bar.w), (30, 2));
        assert_eq!(bar.y, row.y + 9);
        assert_eq!(bar.h, 12, "30 - 9 - 9");
    }

    /// ★ The property this module exists for: what lights up is what you clicked.
    #[test]
    fn every_nav_row_is_hittable_across_its_whole_height() {
        let k = hero();
        for (i, kind) in k.iter().enumerate() {
            let r = nav_entry(&k, i).unwrap();
            for y in r.y..r.y + r.h {
                let hit = nav_hit(&k, 100, y);
                match kind {
                    NavKind::Item => assert_eq!(hit, Some(i), "row {} missed at y={}", i, y),
                    NavKind::Group => assert_ne!(hit, Some(i), "heading {} is not a target", i),
                }
            }
        }
    }

    #[test]
    fn the_nav_gutter_is_clickable_but_the_content_column_is_not() {
        let k = hero();
        let y = nav_entry(&k, 1).unwrap().y + 4;
        assert_eq!(nav_hit(&k, 0, y), Some(1), "the 30px gutter belongs to the row beside it");
        assert_eq!(nav_hit(&k, 207, y), Some(1));
        assert_eq!(nav_hit(&k, 208, y), None, "the content column is not the nav");
        assert_eq!(nav_hit(&k, -1, y), None);
    }

    #[test]
    fn nav_entry_refuses_an_index_it_does_not_have() {
        assert!(nav_entry(&hero(), 9).is_none());
        assert!(nav_entry(&[], 0).is_none());
        assert_eq!(nav_hit(&[], 10, 10), None);
    }

    /// Against the hero scene: a 1440-wide window with a 512-tall body.
    #[test]
    fn the_content_column_matches_the_hero_scene() {
        let l = list(880, 512, false);
        assert_eq!(l.content.x, 240, "208 nav + 32 padding");
        assert_eq!(l.content.right(), 848, "880 - 32");
        assert_eq!(l.title.y, 30);
        assert_eq!(l.title.h, 34, "d2's line box");
        assert_eq!(l.sub.y, 69, "30 + 34 + 5");
        assert_eq!(l.sub.h, 17, "b3's line box");
        assert_eq!(l.band.y, 112, "69 + 17 + 26");
        assert_eq!(l.row_h, 44);
    }

    /// ⚠️ The scrollbar the window server paints over the app's own right edge. A size column laid
    /// out under it is invisible exactly when the list is long enough to need scrolling.
    #[test]
    fn the_size_column_clears_the_window_servers_scrollbar() {
        let plain = list(880, 512, false);
        let scrolled = list(880, 512, true);
        assert_eq!(
            plain.content.right() - scrolled.content.right(),
            crate::layout::scrollbar_reserve()
        );
        assert_eq!(scrolled.content.x, plain.content.x, "only the right edge moves");
        // The size column must end left of the scrollbar TRACK, not merely inside the window.
        let track = crate::layout::scrollbar_track(Rect::new(0, 0, 880, 512));
        assert!(
            row_meta_right(&scrolled) <= track.x,
            "the size column at {} runs under the scrollbar at {}",
            row_meta_right(&scrolled), track.x
        );
    }

    /// A window narrower than its own chrome must produce a degenerate column, not a negative one —
    /// widths feed `fill_rect` and `ellipsize`, and a negative budget there is an underflow.
    #[test]
    fn an_impossibly_narrow_surface_degrades_instead_of_underflowing() {
        for w in [0, 1, 120, 240, 260] {
            let l = list(w, 200, true);
            assert!(l.content.w >= 0, "w={} gave a negative column", w);
            let (_, name_w) = row_name(&l);
            assert!(name_w >= 0);
            assert!(max_scroll(&l, 40) >= 0);
        }
        let short = list(880, 10, false);
        assert_eq!(short.band.h, 0, "a surface shorter than its own header has no band");
        assert_eq!(visible_rows(&short, 0, 50), (0, 2));
    }

    #[test]
    fn a_rows_plate_hangs_outside_the_content_column() {
        let l = list(880, 512, false);
        let r = row_plate(&l, 0, 0);
        assert_eq!(r.x, 228, "240 - 12");
        assert_eq!(r.right(), 860, "848 + 12");
        assert_eq!((r.y, r.h), (112, 44));
        assert_eq!(row_plate(&l, 3, 0).y, 112 + 3 * 44);
        assert_eq!(row_plate(&l, 3, 30).y, 112 + 3 * 44 - 30, "scroll moves rows, not the band");
    }

    #[test]
    fn a_rows_columns_do_not_overlap() {
        let l = list(880, 512, false);
        let r = row_plate(&l, 0, 0);
        let ic = row_icon(&l, r);
        let (name_x, name_w) = row_name(&l);
        assert_eq!((ic.x, ic.w, ic.h), (240, 16, 16));
        assert_eq!(ic.y, r.y + 14, "16px glyph centred in a 44px row");
        assert_eq!(name_x, 271, "240 + 16 + 15");
        assert_eq!(name_x + name_w, row_meta_right(&l) - 92, "the name stops at the size column");
        assert!(name_x >= ic.right());
    }

    /// ★ The same property as the nav, for the list: the plate under the pointer is the row that
    /// gets opened. Swept across every pixel of a scrolled band.
    #[test]
    fn the_row_under_the_pointer_is_the_row_that_lights_up() {
        let l = list(880, 512, false);
        let (n, scroll) = (40usize, 57i32);
        let mut seen = 0;
        for y in l.band.y..l.band.bottom() {
            if let Some(i) = row_at(&l, 400, y, scroll, n) {
                assert!(row_plate(&l, i, scroll).contains(400, y), "row {} does not contain y={}", i, y);
                seen += 1;
            }
        }
        assert_eq!(seen, l.band.h, "every pixel of the band belongs to some row");
    }

    /// ★ The mask has to cover **everything** a scrolled row can paint above the band: the plate's
    /// 12px overhang on both sides, and the full width out to the scrollbar. A gap anywhere in it
    /// leaves a sliver of the row showing through the heading.
    #[test]
    fn the_header_mask_covers_every_pixel_a_scrolled_row_can_reach() {
        for (w, h) in [(880, 512), (640, 400), (1200, 900)] {
            for scrollbar in [false, true] {
                let l = list(w, h, scrollbar);
                let m = header_mask(&l, w);
                assert_eq!(m.y, 0);
                assert_eq!(m.bottom(), l.band.y, "the mask must stop exactly at the band");
                assert_eq!(m.right(), w, "and run to the surface edge, under the scrollbar");
                for i in 0..4 {
                    for scroll in [1, 17, 43, 44, 100] {
                        let p = row_plate(&l, i, scroll);
                        if p.y < l.band.y {
                            assert!(p.x >= m.x, "plate {:?} starts left of the mask {:?}", p, m);
                            assert!(p.right() <= m.right(), "plate {:?} runs past the mask", p);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn nothing_outside_the_band_or_the_list_is_a_row() {
        let l = list(880, 512, false);
        assert_eq!(row_at(&l, 400, l.band.y - 1, 0, 40), None, "the header block is not a row");
        assert_eq!(row_at(&l, 400, l.band.bottom(), 0, 40), None);
        assert_eq!(row_at(&l, 400, l.band.y + 10, 0, 0), None, "an empty folder has no rows");
        assert_eq!(row_at(&l, 100, l.band.y + 10, 0, 40), None, "the nav pane is not a row");
        assert_eq!(row_at(&l, 400, l.band.y + 3 * 44, 0, 3), None, "past the last row");
    }

    #[test]
    fn scrolling_bounds_are_never_negative() {
        let l = list(880, 512, false);
        assert_eq!(max_scroll(&l, 0), 0);
        assert_eq!(max_scroll(&l, 3), 0, "three rows fit in a 400px band");
        assert_eq!(rows_height(&l, 10), 440);
        assert_eq!(max_scroll(&l, 10), 440 - l.band.h);
    }

    /// ★★ The reported content height has to make the WINDOW SERVER's arithmetic land on this
    /// module's. The shell owns the scrollbar — it draws it over the app's right edge and maps a
    /// drag with `mouse_to_scroll(my, bar_y, viewport = r.h, content = header.content_h)`, giving
    /// an offset in `0 ..= content - viewport`. This asserts that range IS `0 ..= max_scroll`, and
    /// that the shell's visibility test is exactly "the rows overflow the band".
    ///
    /// Reproduced here rather than trusted, because the two live in different crates and the
    /// symptom of disagreement is the last few rows of a long folder being unreachable by the
    /// scrollbar — which looks like a stuck list, not like an off-by-`band.y`.
    #[test]
    fn the_window_servers_scroll_range_is_exactly_this_modules() {
        for (w, h) in [(880, 512), (640, 400), (1200, 900), (400, 240)] {
            for n in [0usize, 1, 3, 9, 40, 500] {
                let scrollbar = needs_scrollbar(w, h, n);
                let l = list(w, h, scrollbar);
                let content = reported_content_height(&l, n);

                // The shell: `if content_h > r.h { draw_scrollbar(..) }`.
                assert_eq!(
                    content > h, scrollbar,
                    "{}x{} n={}: the shell and needs_scrollbar disagree", w, h, n
                );

                // The shell: `rel * (content - viewport) / track_range`, maximum at rel == range.
                let shell_max = (content - h).max(0);
                assert_eq!(
                    shell_max, max_scroll(&l, n),
                    "{}x{} n={}: a full drag lands {}px from the end of the list",
                    w, h, n, max_scroll(&l, n) - shell_max
                );

                // ...and at that offset the last row is fully inside the band.
                if n > 0 && scrollbar {
                    let last = row_plate(&l, n - 1, shell_max);
                    assert!(
                        last.bottom() <= l.band.bottom() && last.y >= l.band.y,
                        "{}x{} n={}: the last row is not reachable ({:?} vs band {:?})",
                        w, h, n, last, l.band
                    );
                }
            }
        }
    }

    /// `needs_scrollbar` must not oscillate: reserving the 12px changes the content column's width,
    /// and if that fed back into the overflow test the bar would appear and disappear every frame.
    #[test]
    fn reserving_the_scrollbar_does_not_change_whether_one_is_needed() {
        for n in [0usize, 8, 9, 10, 200] {
            let with = list(880, 512, true);
            let without = list(880, 512, false);
            assert_eq!(with.band.h, without.band.h, "the bar takes width, never height");
            assert_eq!(rows_height(&with, n), rows_height(&without, n));
        }
    }

    /// The culling window has to cover the band plus the partial row at each end, or the top and
    /// bottom rows of a scrolled list are missing.
    #[test]
    fn the_visible_window_covers_every_row_that_can_show_ink() {
        let l = list(880, 512, false);
        let n = 60;
        for scroll in [0, 1, 43, 44, 200, max_scroll(&l, n)] {
            let (first, last) = visible_rows(&l, scroll, n);
            for i in 0..n {
                let r = row_plate(&l, i, scroll);
                let shows = r.bottom() > l.band.y && r.y < l.band.bottom();
                if shows {
                    assert!(i >= first && i < last, "row {} shows at scroll {} but was culled", i, scroll);
                }
            }
            assert!(last <= n);
        }
        assert_eq!(visible_rows(&l, 0, 0), (0, 0));
    }

    /// Everything a breadcrumb emits, in order, as `"text"` pieces — so a test can assert on what
    /// the heading actually SPELLS rather than only on where its boxes are.
    fn spell(path: &str, max_w: i32) -> alloc::string::String {
        let mut s = String::new();
        breadcrumb(path, D2, raw_label, max_w, |_, txt, _, _| {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(txt);
        });
        s
    }

    #[test]
    fn a_breadcrumb_names_every_ancestor_of_a_path() {
        register_text();
        let mut segs = Vec::new();
        breadcrumb("/mnt/nvme/apps", D2, raw_label, i32::MAX, |k, s, _, _| {
            if let CrumbKind::Segment(t) = k {
                segs.push((String::from(s), t));
            }
        });
        assert_eq!(segs.len(), 4);
        assert_eq!(segs[0], (String::from("/"), 1));
        assert_eq!(segs[1].0, "mnt");
        assert_eq!(&"/mnt/nvme/apps"[..segs[1].1], "/mnt");
        assert_eq!(&"/mnt/nvme/apps"[..segs[2].1], "/mnt/nvme");
        assert_eq!(&"/mnt/nvme/apps"[..segs[3].1], "/mnt/nvme/apps");
    }

    /// ★ The root crumb IS a separator, so it must not be followed by another one.
    ///
    /// The first version emitted one unconditionally and the heading read "/ / mnt / nvme". Every
    /// geometry assertion passed — a doubled separator is laid out perfectly — and it was caught by
    /// rendering the frame on the host (`examples/files_preview.rs`), not by a test. This is that
    /// test.
    #[test]
    fn the_root_crumb_is_not_followed_by_a_second_slash() {
        register_text();
        assert_eq!(spell("/mnt/nvme/apps", i32::MAX), "/ mnt / nvme / apps");
        assert_eq!(spell("/mnt", i32::MAX), "/ mnt");
        assert_eq!(spell("/", i32::MAX), "/");
        assert!(!spell("/mnt/nvme", i32::MAX).contains("/ /"));
    }

    #[test]
    fn the_root_path_is_one_crumb() {
        register_text();
        let mut n = 0;
        breadcrumb("/", D2, raw_label, i32::MAX, |_, _, _, _| n += 1);
        assert_eq!(n, 1);
        assert_eq!(crumb_at("/", D2, raw_label, i32::MAX, 0, 2), Some(1));
    }

    /// A heading wider than its box drops ancestors from the FRONT, and never drops the folder's
    /// own name — that is the one piece of the heading that is not optional.
    #[test]
    fn an_overlong_breadcrumb_sheds_ancestors_and_keeps_the_folder() {
        register_text();
        let path = "/mnt/nvme/apps";
        let full = width(D2, "/") ; // just to force the font to be loaded
        let _ = full;

        assert_eq!(spell(path, i32::MAX), "/ mnt / nvme / apps");
        // Squeezing it down sheds from the left, one piece at a time, until only "apps" is left.
        let squeezed = spell(path, 120);
        assert!(squeezed.ends_with("apps"), "{:?} lost the folder name", squeezed);
        assert!(squeezed.len() < "/ mnt / nvme / apps".len(), "{:?} did not shed", squeezed);
        assert_eq!(spell(path, 0), "apps", "with no room at all, only the folder survives");

        // Whatever survives must actually fit, unless it is the irreducible last crumb.
        for max in [0, 20, 60, 90, 140, 200, 400] {
            let mut total = 0;
            let mut n = 0;
            breadcrumb(path, D2, raw_label, max, |_, s, _, _| {
                total += width(D2, s);
                n += 1;
            });
            assert!(n >= 1, "a breadcrumb is never empty");
            assert!(total <= max.max(width(D2, "apps")), "max={} kept {}px", max, total);
        }
    }

    /// Shedding must not break the hit-test: the crumbs still on screen have to stay clickable,
    /// and a target that was dropped must not be reachable at a stale coordinate.
    #[test]
    fn a_shed_breadcrumb_is_still_correctly_clickable() {
        register_text();
        let path = "/mnt/nvme/apps";
        for max in [40, 90, 140, i32::MAX] {
            let mut shown = Vec::new();
            breadcrumb(path, D2, raw_label, max, |k, _, x, w| {
                if let CrumbKind::Segment(t) = k {
                    shown.push((t, x, w));
                }
            });
            for (t, x, w) in &shown {
                for mx in *x..(*x + *w) {
                    assert_eq!(crumb_at(path, D2, raw_label, max, 0, mx), Some(*t), "max={}", max);
                }
            }
            // Past the end of the trail there is nothing to click.
            let end = shown.last().map(|c| c.1 + c.2).unwrap();
            assert_eq!(crumb_at(path, D2, raw_label, max, 0, end + 8), None);
        }
    }

    /// ★ A crumb is laid out from the string it is DRAWN with, not the one in the path.
    ///
    /// Files renames its ancestors — `/mnt/nvme/apps` is "Applications" in the nav pane and the
    /// heading has to agree with it. The first version measured the raw segment and drew the label,
    /// which is a silent overlap: "Applications" is roughly four times the width of "apps", so it
    /// would run straight through the crumb beside it and every hit-test after it would be wrong by
    /// the difference. Renaming must move the boxes.
    #[test]
    fn a_renamed_crumb_is_measured_by_its_label_not_by_its_path() {
        register_text();
        let rename = |t: usize, raw: &str| {
            if t == 14 { String::from("Applications") } else { String::from(raw) }
        };
        let path = "/mnt/nvme/apps";

        let mut boxes = alloc::vec::Vec::new();
        breadcrumb(path, D2, rename, i32::MAX, |k, s, x, w| boxes.push((k, String::from(s), x, w)));

        let last = boxes.last().unwrap();
        assert_eq!(last.1, "Applications", "the label reached the emit");
        assert_eq!(last.3, crate::text::width(D2, "Applications"), "measured as drawn");

        // Nothing may overlap its neighbour, at any label length.
        for pair in boxes.windows(2) {
            assert!(
                pair[0].2 + pair[0].3 <= pair[1].2,
                "{:?} runs into {:?}",
                pair[0], pair[1]
            );
        }
        // ...and the hit-test agrees with the widened box.
        assert_eq!(crumb_at(path, D2, rename, i32::MAX, 0, last.2 + last.3 - 1), Some(14));
        assert_eq!(crumb_at(path, D2, rename, i32::MAX, 0, last.2 + last.3 + 4), None);
    }

    /// Every crumb payload must be a valid slice of the path it came from, including when the path
    /// is malformed — a doubled or trailing separator is what `join_path` produces on a bad day,
    /// and slicing on a byte that is not a boundary panics inside the file manager.
    #[test]
    fn crumb_targets_are_always_valid_prefixes() {
        register_text();
        for path in ["/", "/mnt", "/mnt/nvme/apps", "//mnt//nvme/", "/mnt/nvme/apps/", "", "mnt/nvme"] {
            breadcrumb(path, D2, raw_label, i32::MAX, |k, s, x, w| {
                assert!(w > 0, "{:?}: crumb {:?} has no width", path, s);
                assert!(x >= 0);
                if let CrumbKind::Segment(t) = k {
                    assert!(t <= path.len().max(1), "{:?}: target {} is past the end", path, t);
                    if t <= path.len() {
                        assert!(path.is_char_boundary(t), "{:?}: target {} splits a codepoint", path, t);
                    }
                }
            });
        }
    }

    /// Drawing and hit-testing walk one function, so a crumb reads as clickable exactly where it
    /// is clickable.
    #[test]
    fn clicking_a_crumb_lands_on_the_path_it_spells() {
        register_text();
        let path = "/mnt/nvme/apps";
        let origin = 240;
        let mut checked = 0;
        breadcrumb(path, D2, raw_label, i32::MAX, |k, _, x, w| {
            if let CrumbKind::Segment(t) = k {
                for mx in (origin + x)..(origin + x + w) {
                    assert_eq!(crumb_at(path, D2, raw_label, i32::MAX, origin, mx), Some(t));
                }
                checked += 1;
            }
        });
        assert_eq!(checked, 4);
    }

    /// The separators carry the air that makes it read as a breadcrumb rather than as a path, and
    /// they must not be reachable — clicking the rule between two folders has no meaning.
    #[test]
    fn separators_have_air_and_are_not_targets() {
        register_text();
        let path = "/mnt/nvme";
        breadcrumb(path, D2, raw_label, i32::MAX, |k, _, x, w| {
            if k == CrumbKind::Separator {
                // The gap sits outside the emitted rect, so the hit-test skips it either way.
                assert_eq!(crumb_at(path, D2, raw_label, i32::MAX, 0, x + w / 2), None);
                assert_eq!(crumb_at(path, D2, raw_label, i32::MAX, 0, x - 1), None, "the leading gap is dead space");
            }
        });
    }
}
