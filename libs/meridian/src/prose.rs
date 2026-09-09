//! The Text window's one column of type — `.tx`, `.tx .col`, `.tx h1`, `.tx h2`, `.tx p`.
//!
//! ```text
//! .tx      { padding: 46px 0 0; overflow: hidden }
//! .tx .col { max-width: 62ch; margin: 0 auto; padding: 0 32px }
//! .tx h1   { 28px/34, weight 300, margin 0 0 22px }
//! .tx h2   { 15px/22, weight 500, margin 26px 0 8px }
//! .tx p    { 14px/25, --fg-2,     margin 0 0 16px }
//! .tx .car { width: 1px; height: 17px; background: var(--accent) }
//! ```
//!
//! Geometry only — this module never sees a character of the document. It answers "where does line
//! *i* sit and how wide may it be", and the application decides what a line *is*.
//!
//! ## The measure is the whole design
//!
//! Everything else in Meridian is a component; this window is one column of prose with no chrome at
//! all, so the only decisions left are the measure and the leading. `62ch` is a real typographic
//! cap — text set wider than about 65 characters loses the reader on the line return — and it is why
//! the column **centres** rather than filling: a window dragged to 1400px must not produce a
//! 200-character line.
//!
//! ⚠️ The design document contradicts itself here. Its prose says the measure is capped at **68ch**
//! and its CSS says **62ch**. The CSS is what renders the picture everything else was matched
//! against, so 62 is what this uses; the disagreement is recorded rather than silently resolved.
//!
//! ## Units
//!
//! Same rule as `panes.rs` and `layout.rs`: the `const`s are design units against the 1440x900
//! scene and are private; everything public returns real pixels through [`crate::scale::px`].

use crate::layout::Rect;
use crate::scale::px as sc;
use crate::tokens::{Face, Style, B1, D2};

/// `.tx { padding-top: 46px }`
const PAD_TOP: i32 = 46;
/// `.tx .col { padding: 0 32px }`
const COL_PAD_X: i32 = 32;
/// `.tx .col { max-width: 62ch }`. In `ch` — the advance of `0` in the body face — not in pixels,
/// because that is what the unit means and it is what keeps the measure right at every scale.
const MEASURE_CH: i32 = 62;
/// `.tx h1 { margin-bottom: 22px }`
const H1_BELOW: i32 = 22;
/// `.tx h2 { margin: 26px 0 8px }`
const H2_ABOVE: i32 = 26;
const H2_BELOW: i32 = 8;
/// `.tx p { margin-bottom: 16px }`
const P_BELOW: i32 = 16;
/// `.tx p { line-height: 25px }` — the document's leading, looser than `b1`'s 22px because this is
/// prose to be read rather than a row in a list.
const P_LINE: i32 = 25;
/// `.tx .car { width: 1px; height: 17px }`
const CARET_W: i32 = 1;
const CARET_H: i32 = 17;

/// `.tx h2` — 15px at weight 500.
///
/// Not a step on the design's published scale (`b1` is 15px Regular, `lb` is 11px Medium), but a
/// real style in its component sheet, exactly as `CAPTION` is: `b1`'s size at `lb`'s weight, without
/// the uppercasing. Named here rather than in `tokens` because only this window sets it, and adding
/// it to `ALL_STYLES` would cost the shell an atlas shelf for a style the shell never draws.
pub const H2: Style = Style { px: 15, line_h: 22, tracking: -5, face: Face::Medium, upper: false };

/// `.tx h1` — the design's 28px/300 is `d2` exactly.
pub const H1: Style = D2;

/// `.tx p` — the body.
///
/// ⚠️ The design sets 14px, which is **not on its own type scale** (`b1` is 15, `b2` is 13). Set at
/// `b1`, the same call made for Settings' `.sr .k`: following the scale is the more faithful reading
/// of a design whose entire argument is that the scale *is* the system. The design's 25px leading is
/// kept — that is a prose decision, not a scale step.
pub const BODY: Style = B1;

/// What a line of the document is. The application classifies; this module only stacks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Block {
    /// `.tx h1`
    H1,
    /// `.tx h2`
    H2,
    /// `.tx p` — and also a blank line, which is a paragraph of nothing and still takes a line.
    Body,
}

impl Block {
    pub fn style(self) -> Style {
        match self {
            Block::H1 => H1,
            Block::H2 => H2,
            Block::Body => BODY,
        }
    }

    /// One line of this block.
    ///
    /// ⚠️ Line height, **not** `text::line_box`. A paragraph stacks on the design's leading (25px for
    /// a 15px face, where the face's own box is about 18px); using the ink box would set the document
    /// solid. This is exactly the case `text::line_box`'s own warning carves out — that function is
    /// for centring one line in a box, and this is stacking lines of a paragraph.
    pub fn line_h(self) -> i32 {
        match self {
            Block::Body => sc(P_LINE),
            other => other.style().line() as i32,
        }
    }

    fn margin_below(self) -> i32 {
        match self {
            Block::H1 => sc(H1_BELOW),
            Block::H2 => sc(H2_BELOW),
            Block::Body => 0,
        }
    }

    /// The air above this block. Only `h2` has any — a heading needs separating from the paragraph
    /// it follows, and `.tx h1` is always first.
    fn margin_above(self) -> i32 {
        match self {
            Block::H2 => sc(H2_ABOVE),
            _ => 0,
        }
    }
}

/// One **visual** row of the document: a block, and whether it is the first and/or last row of the
/// source line it came from.
///
/// A paragraph wider than the measure occupies several rows, and only the outer two carry the
/// block's margins — otherwise every wrapped line of a heading would add another 26px of air, and a
/// paragraph's 16px bottom margin would appear between its own wrapped lines.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Row {
    pub block: Block,
    pub first: bool,
    pub last: bool,
}

/// One source line, as [`rows`] needs to see it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Source {
    pub block: Block,
    /// How many visual rows it wraps to. Never zero — an empty line is one row.
    pub wrapped: usize,
    /// True when the line has no non-whitespace content.
    pub blank: bool,
}

/// Expand source lines into visual rows, **collapsing margins against blank lines**.
///
/// ## The problem this solves
///
/// The design renders a document; this edits its source, and the two disagree about where spacing
/// comes from. In `.tx`'s CSS an `h1` is followed by 22px of margin. In Markdown a heading is
/// followed by a *blank line*, which is a real line the caret can sit on and therefore takes a full
/// 25px row. Apply both and every gap in the document is nearly double what the design draws.
///
/// So they collapse, the way CSS margins do: a block's margin-below is dropped when the next line is
/// blank, and a block's margin-above is dropped when the previous line is blank. A document written
/// with blank lines gets the blank lines' rhythm; one written without gets the design's margins.
/// Neither gets both.
pub fn rows(lines: &[Source]) -> alloc::vec::Vec<Row> {
    let mut out = alloc::vec::Vec::new();
    for (i, s) in lines.iter().enumerate() {
        let prev_blank = i > 0 && lines[i - 1].blank;
        let next_blank = lines.get(i + 1).is_some_and(|n| n.blank);
        let n = s.wrapped.max(1);
        for j in 0..n {
            out.push(Row {
                block: s.block,
                first: j == 0 && !prev_blank,
                last: j == n - 1 && !next_blank,
            });
        }
    }
    out
}

impl Row {
    /// A source line that fits on one row and carries both its margins.
    pub fn whole(block: Block) -> Row {
        Row { block, first: true, last: true }
    }

    /// Height including whichever of its margins this row carries.
    pub fn height(self) -> i32 {
        self.block.line_h() + if self.last { self.block.margin_below() } else { 0 }
    }

    fn air_above(self, is_document_start: bool) -> i32 {
        if self.first && !is_document_start { self.block.margin_above() } else { 0 }
    }
}

/// The measure — the column's content width, before its own padding.
///
/// `ch` is the advance of `0` in the body face, so this needs the font registry to be loaded.
pub fn measure_w() -> i32 {
    let ch = nyx_gui::font::advance_px_face(crate::font::slot_of(BODY.face), '0', BODY.size()) as i32;
    MEASURE_CH * ch.max(1)
}

/// The centred column inside a surface `w` x `h`.
///
/// Centres on the **surface**, not on the remaining space after some sidebar, because there is no
/// sidebar: this window is one column and nothing else. A window narrower than the measure gets the
/// window, minus the padding — the cap is a maximum, never a minimum.
pub fn column(w: i32, h: i32) -> Rect {
    let pad = sc(COL_PAD_X);
    let content = (w - 2 * pad).min(measure_w()).max(0);
    Rect::new((w - content) / 2, sc(PAD_TOP), content, (h - sc(PAD_TOP)).max(0))
}

/// The top of row `i`, relative to the column's own top.
///
/// Walked from the start rather than cached, for the same reason `panes::nav_entry` is: a row's
/// offset depends on every row before it, the list changes on every keystroke, and a stale cache in
/// a text editor is a caret drawn on the wrong line.
pub fn row_top(rows: &[Row], i: usize) -> Option<i32> {
    if i >= rows.len() {
        return None;
    }
    let mut y = 0;
    for (j, r) in rows.iter().enumerate() {
        y += r.air_above(j == 0);
        if j == i {
            return Some(y);
        }
        y += r.height();
    }
    None
}

/// The rect of row `i` inside `col` — its text line, without whatever margin follows it.
pub fn row(col: Rect, rows: &[Row], i: usize) -> Option<Rect> {
    let top = row_top(rows, i)?;
    Some(Rect::new(col.x, col.y + top, col.w, rows[i].block.line_h()))
}

/// Total height of the document, so the window can report it for the scrollbar.
///
/// Includes the column's own top padding, because the window server's scroll viewport is the WHOLE
/// window — see `panes::reported_content_height` for what happens when an app reports only its
/// content and not the air above it.
pub fn height(rows: &[Row]) -> i32 {
    let mut y = 0;
    for (j, r) in rows.iter().enumerate() {
        y += r.air_above(j == 0) + r.height();
    }
    y + sc(PAD_TOP)
}

/// The row containing `y` (relative to the column's top), for placing the caret with the mouse.
///
/// Returns the **nearest** row rather than None for a click in a margin: the gap under a heading
/// belongs to the heading as far as a reader is concerned, and a click that does nothing in a text
/// editor reads as the window being dead.
pub fn row_at(rows: &[Row], y: i32) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    if y < 0 {
        return Some(0);
    }
    let mut top = 0;
    for (j, r) in rows.iter().enumerate() {
        top += r.air_above(j == 0);
        let next = top + r.height();
        if y < next {
            return Some(j);
        }
        top = next;
    }
    Some(rows.len() - 1)
}

/// Break `s` into the byte ranges that each fit `max_w` when set in `style`.
///
/// Breaks at whitespace, keeping the space at the **end** of the row it followed, so a caret placed
/// after the last word of a wrapped row has somewhere to sit. A single word wider than the measure
/// is broken mid-word rather than allowed to overhang — a URL in a 62ch column is a real case, and
/// text running off the edge of the only column in the window is worse than an ugly break.
///
/// Always returns at least one range, so a caller can index `[0]` for an empty line.
pub fn wrap(style: Style, s: &str, max_w: i32) -> alloc::vec::Vec<core::ops::Range<usize>> {
    let mut out = alloc::vec::Vec::new();
    if s.is_empty() || max_w <= 0 {
        out.push(0..s.len());
        return out;
    }
    let mut start = 0usize;
    while start < s.len() {
        let rest = &s[start..];
        if crate::text::width(style, rest) <= max_w {
            out.push(start..s.len());
            return out;
        }
        // The last byte offset in `rest` whose prefix still fits.
        let mut fit = 0usize;
        for (i, _) in rest.char_indices().skip(1) {
            if crate::text::width(style, &rest[..i]) > max_w {
                break;
            }
            fit = i;
        }
        if fit == 0 {
            // Not even one character fits. Take one anyway or this loops forever.
            fit = rest.char_indices().nth(1).map(|(i, _)| i).unwrap_or(rest.len());
        }
        // Prefer the last whitespace at or before the fit point, and keep it on this row.
        let brk = rest[..fit]
            .char_indices()
            .filter(|&(_, c)| c.is_whitespace())
            .map(|(i, c)| i + c.len_utf8())
            .next_back()
            .unwrap_or(fit);
        out.push(start..start + brk);
        start += brk;
    }
    if out.is_empty() {
        out.push(0..s.len());
    }
    out
}

/// The caret: a 1px accent rule, `x` at the pen position and centred on `line`'s vertical middle.
///
/// ⚠️ Deliberately **not** the full line height. `.tx .car` is 17px against a 25px line, so the
/// caret does not touch the line above or below it — in a paragraph set at 25px leading a
/// full-height caret merges with its neighbours into a continuous vertical rule.
pub fn caret(line: Rect, x: i32) -> Rect {
    let h = sc(CARET_H).min(line.h.max(1));
    Rect::new(x, line.y + (line.h - h) / 2, sc(CARET_W).max(1), h)
}

/// Words in `s`, by the only definition a word count can honestly use here: runs separated by
/// whitespace.
///
/// The design's caption says "412 words", and a count is the sort of number that looks authoritative
/// however it was reached. This one is `split_whitespace`, which means `don't` is one word and
/// `--` is one word. Stated so nobody has to guess which convention it follows.
pub fn word_count(s: &str) -> usize {
    s.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::register_text;
    use alloc::vec;
    use alloc::vec::Vec;

    fn doc() -> Vec<Row> {
        [Block::H1, Block::Body, Block::H2, Block::Body, Block::Body]
            .into_iter()
            .map(Row::whole)
            .collect()
    }

    /// The design's own metrics, read straight off the CSS.
    ///
    /// ⚠️ Note what this pins about the design's **own** 592px window: at `b1`'s 15px the measure
    /// (62ch ≈ 558px) is wider than the window minus its 32px padding (528px), so that window is
    /// padding-limited and sets about 58 characters, not 62. That is a consequence of setting the
    /// body at 15px where the design says 14 — see [`BODY`]. The cap is a maximum, so the result is
    /// a slightly shorter line, never a clipped one.
    #[test]
    fn the_column_matches_the_design_metrics() {
        register_text();
        let col = column(592, 544);
        assert_eq!(col.y, 46, ".tx padding-top");
        assert_eq!(col.w, 592 - 2 * 32, "the design's own window is padding-limited");
        assert!(measure_w() > col.w, "the fixture assumes 62ch does not fit a 592px window at b1");
        // Centred, and the two margins are equal.
        assert_eq!(col.x, 32);
        assert_eq!(col.x + col.w + col.x, 592);
    }

    /// ★ The cap is a maximum, not a fixed width. A window dragged narrower than the measure gets
    /// the window; one dragged wider keeps the measure and gains margins.
    #[test]
    fn the_measure_caps_a_wide_window_and_yields_to_a_narrow_one() {
        register_text();
        let wide = column(2000, 600);
        assert_eq!(wide.w, measure_w(), "a wide window must not set a 200-character line");
        assert!(wide.x > 32, "the column did not centre in a wide window");

        let narrow = column(300, 600);
        assert_eq!(narrow.w, 300 - 2 * 32, "a narrow window should give up the measure, not clip");
        assert_eq!(narrow.x, 32);

        // Absurdly narrow must not produce a negative width.
        assert_eq!(column(10, 600).w, 0);
        assert!(column(0, 0).w >= 0);
    }

    /// Rows stack in order with no overlap, and `height` agrees with the walk.
    #[test]
    fn rows_stack_without_overlap_and_the_total_agrees() {
        register_text();
        let d = doc();
        let col = column(592, 544);
        let mut prev_bottom = 0;
        for i in 0..d.len() {
            let r = row(col, &d, i).unwrap();
            let top = row_top(&d, i).unwrap();
            assert!(top >= prev_bottom, "row {} starts above row {}'s bottom", i, i - 1);
            assert!(r.h > 0);
            assert_eq!(r.x, col.x);
            assert_eq!(r.w, col.w);
            prev_bottom = top + r.h;
        }
        assert!(height(&d) >= prev_bottom + col.y);
        assert_eq!(row(col, &d, d.len()), None);
        assert_eq!(row_top(&d, d.len()), None);
    }

    /// `.tx h2 { margin-top: 26px }` — a heading needs air above it, but not when it is the first
    /// thing in the document, or the column starts 26px too low.
    #[test]
    fn a_heading_carries_air_above_it_unless_it_is_first() {
        register_text();
        let leading = vec![Row::whole(Block::H2), Row::whole(Block::Body)];
        assert_eq!(row_top(&leading, 0), Some(0), "a leading heading pushed the column down");

        let d = doc();
        let h2_top = row_top(&d, 2).unwrap();
        let prev = row_top(&d, 1).unwrap() + d[1].height();
        assert_eq!(h2_top - prev, 26, ".tx h2 margin-top");
    }

    /// ★ A wrapped block carries its margins ONCE, on its outer rows. Otherwise every wrapped line
    /// of a paragraph would be pushed apart by that paragraph's own bottom margin, and a two-line
    /// heading would gain 26px of air in its middle.
    #[test]
    fn a_wrapped_block_carries_its_margins_only_on_its_outer_rows() {
        register_text();
        let wrapped = vec![
            Row { block: Block::H2, first: true, last: false },
            Row { block: Block::H2, first: false, last: true },
            Row { block: Block::Body, first: true, last: true },
        ];
        // No air injected between the heading's own two lines.
        assert_eq!(row_top(&wrapped, 1).unwrap() - row_top(&wrapped, 0).unwrap(), Block::H2.line_h());
        // ...and the 8px below the heading appears once, after its last row.
        let gap = row_top(&wrapped, 2).unwrap() - row_top(&wrapped, 1).unwrap();
        assert_eq!(gap, Block::H2.line_h() + 8);
    }

    /// ⚠️ Paragraphs stack on the design's LEADING, not on the face's ink box. Set on `line_box` the
    /// document would come out solid — this is the case `text::line_box`'s own warning carves out.
    #[test]
    fn paragraphs_stack_on_the_designs_leading_not_the_ink_box() {
        register_text();
        assert_eq!(Block::Body.line_h(), 25, ".tx p line-height");
        assert!(
            Block::Body.line_h() > crate::text::line_box(BODY),
            "the leading collapsed onto the face's own box ({} vs {})",
            Block::Body.line_h(),
            crate::text::line_box(BODY)
        );
    }

    /// Hit-testing must agree with drawing on every row, and a click in a margin must land
    /// somewhere rather than nowhere.
    #[test]
    fn every_row_is_hit_within_itself_and_a_margin_click_lands_on_a_neighbour() {
        register_text();
        let d = doc();
        for i in 0..d.len() {
            let top = row_top(&d, i).unwrap();
            assert_eq!(row_at(&d, top), Some(i), "row {} does not hit-test to itself", i);
            assert_eq!(row_at(&d, top + 1), Some(i));
        }
        // Above the first row and below the last both clamp — a click in a text editor always
        // places the caret.
        assert_eq!(row_at(&d, -500), Some(0));
        assert_eq!(row_at(&d, 100_000), Some(d.len() - 1));
        assert_eq!(row_at(&[], 0), None);
    }

    /// ★ Wrapping is what makes this an editor rather than a viewer: without it a paragraph typed
    /// past the measure runs off the only column in the window. Every produced row must fit, the
    /// ranges must tile the line exactly, and they must land on char boundaries.
    #[test]
    fn wrapped_rows_fit_the_measure_and_tile_the_line_exactly() {
        register_text();
        let s = "Seven shelves, not seven atlases. A shelf is a horizontal band of the same \
                 512-wide texture, so adding a size costs vertical space and nothing else — no \
                 second binding, no second draw call, no second texture upload.";
        for max_w in [80, 160, 320, 528] {
            let rows = wrap(BODY, s, max_w);
            assert!(rows.len() > 1, "{}px did not wrap at all", max_w);
            let mut at = 0;
            for r in &rows {
                assert_eq!(r.start, at, "gap or overlap at {:?} for {}px", r, max_w);
                assert!(s.is_char_boundary(r.start) && s.is_char_boundary(r.end), "{:?}", r);
                assert!(
                    crate::text::width(BODY, &s[r.clone()]) <= max_w,
                    "row {:?} is {}px in a {}px measure",
                    &s[r.clone()],
                    crate::text::width(BODY, &s[r.clone()]),
                    max_w
                );
                at = r.end;
            }
            assert_eq!(at, s.len(), "the wrap stopped short at {}px", max_w);
        }
    }

    /// A word longer than the whole measure has to break mid-word. The alternative is text running
    /// off the edge of the only column in the window, which is worse than an ugly break — and a
    /// wrapper that refuses to break would loop forever looking for a whitespace that is not there.
    #[test]
    fn an_unbreakable_word_is_broken_rather_than_allowed_to_overhang() {
        register_text();
        let s = "https://example.invalid/a/very/long/path/that/has/no/spaces/in/it/at/all";
        let rows = wrap(BODY, s, 100);
        assert!(rows.len() > 1);
        let mut at = 0;
        for r in &rows {
            assert_eq!(r.start, at);
            assert!(r.end > r.start, "empty row — the wrapper made no progress");
            assert!(crate::text::width(BODY, &s[r.clone()]) <= 100);
            at = r.end;
        }
        assert_eq!(at, s.len());
    }

    /// ★ A blank source line and a CSS margin are two ways of saying the same thing, and applying
    /// both nearly doubles every gap in the document. The one the author wrote wins.
    #[test]
    fn a_blank_line_collapses_the_margins_around_it() {
        register_text();
        let blank = Source { block: Block::Body, wrapped: 1, blank: true };
        let para = Source { block: Block::Body, wrapped: 1, blank: false };
        let h2 = Source { block: Block::H2, wrapped: 1, blank: false };

        // Written WITH blank lines: no margins, so the gap is exactly the blank row.
        let with = rows(&[para, blank, h2, blank, para]);
        assert!(!with[0].last, "margin-below survived a following blank line");
        assert!(!with[2].first, "margin-above survived a preceding blank line");
        assert!(!with[2].last);
        let h2_top = row_top(&with, 2).unwrap();
        assert_eq!(h2_top - row_top(&with, 1).unwrap(), Block::Body.line_h(), "gap is one blank row");

        // Written WITHOUT them: the design's own margins do the work instead.
        let without = rows(&[para, h2, para]);
        assert!(without[1].first && without[1].last);
        assert_eq!(
            row_top(&without, 1).unwrap() - row_top(&without, 0).unwrap(),
            Block::Body.line_h() + 26,
            ".tx h2 margin-top"
        );
    }

    /// A wrapped line contributes N rows, and only its outer two may carry margins — combined with
    /// the collapse above, that is two rules interacting and worth pinning together.
    #[test]
    fn wrapping_and_margin_collapse_compose() {
        register_text();
        let r = rows(&[
            Source { block: Block::H1, wrapped: 2, blank: false },
            Source { block: Block::Body, wrapped: 3, blank: false },
        ]);
        assert_eq!(r.len(), 5);
        assert!(r[0].first && !r[0].last, "the heading's first row must not also be its last");
        assert!(!r[1].first && r[1].last, "the heading's margin-below belongs to its last row");
        assert!(r[2].first && !r[2].last);
        assert!(!r[3].first && !r[3].last, "a middle row carries neither margin");
        assert!(!r[4].first && r[4].last);
        // A zero wrap count would silently drop a line; it is clamped instead.
        assert_eq!(rows(&[Source { block: Block::Body, wrapped: 0, blank: true }]).len(), 1);
    }

    /// Degenerate inputs must still yield exactly one row, because the caller indexes `[0]` to place
    /// a caret on an empty line.
    #[test]
    fn wrapping_always_yields_at_least_one_row() {
        register_text();
        assert_eq!(wrap(BODY, "", 400), vec![0..0]);
        assert_eq!(wrap(BODY, "hi", 0), vec![0..2], "a zero measure must not loop");
        assert_eq!(wrap(BODY, "hi", -5), vec![0..2]);
        assert_eq!(wrap(BODY, "hi", 4000), vec![0..2]);
    }

    /// UTF-8 must survive the break: a range cut through a codepoint panics the moment it is sliced.
    #[test]
    fn wrapping_multibyte_text_lands_on_character_boundaries() {
        register_text();
        let s = "рабочий стол — документы и заметки, набранные по-русски для проверки переносов";
        for max_w in [40, 90, 200] {
            let mut at = 0;
            for r in wrap(BODY, s, max_w) {
                assert!(s.is_char_boundary(r.start) && s.is_char_boundary(r.end), "{:?}", r);
                let _ = &s[r.clone()];
                assert_eq!(r.start, at);
                at = r.end;
            }
            assert_eq!(at, s.len());
        }
    }

    /// ★ The caret must not span the full line. At 25px leading a full-height caret touches the
    /// lines above and below and reads as a continuous rule rather than as a caret.
    #[test]
    fn the_caret_is_shorter_than_its_line_and_centred_in_it() {
        register_text();
        let col = column(592, 544);
        let line = row(col, &doc(), 1).unwrap();
        let c = caret(line, col.x + 40);
        assert_eq!((c.w, c.h), (1, 17));
        assert!(c.h < line.h, "the caret is as tall as the line");
        assert_eq!(line.bottom() - c.bottom(), c.y - line.y, "the caret is not centred");
        assert_eq!(c.x, col.x + 40);

        // On a line SHORTER than the caret (a heading squeezed at some scale) it must clamp rather
        // than overhang into the block above.
        let tiny = Rect::new(0, 0, 100, 8);
        assert!(caret(tiny, 0).h <= tiny.h);
    }

    /// The count is `split_whitespace`, and it is worth pinning because a word count is exactly the
    /// kind of number that looks authoritative however it was reached.
    #[test]
    fn the_word_count_is_whitespace_separated_runs() {
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("   \n\t "), 0);
        assert_eq!(word_count("one"), 1);
        assert_eq!(word_count("Seven shelves, not seven atlases."), 5);
        assert_eq!(word_count("don't  count   double"), 3);
    }
}
