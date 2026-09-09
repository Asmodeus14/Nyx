//! The quantum circuit diagram and its measurement table — `.qcirc`, `.qw`, `.gate`, `.mrow`.
//!
//! Geometry only. Nothing here knows what a qubit is: it is handed a wire count, a column count and
//! which cells are occupied, and it answers with rectangles. The circuit's *meaning* — which gate
//! goes in which column — is `qclang_compiler::program`'s job, computed from the compiled QIR, and
//! keeping the two apart is what lets this be tested on the host while that is tested against the
//! compiler.
//!
//! From `03-components.html`:
//!
//! ```text
//! .qcirc  { width: 400px; padding: 30px 30px 0 }
//! .qw     { display: flex; align-items: center; height: 54px }
//! .qw .lb { width: 40px; font: 12px mono; color: var(--fg-4) }
//! .qw .wire { flex: 1; height: 1px; background: var(--line-2) }
//! .gate   { width: 32px; height: 32px; border: 1px solid var(--line-2); border-radius: 4px;
//!           font: 12px mono; color: var(--fg); background: var(--surface) }
//! .gate.a { border-color: var(--accent) }
//! .mrow   { display: flex; align-items: baseline; padding: 9px 0 }
//! ```
//!
//! ## One departure from the document, and it is a correctness one
//!
//! The design's scene draws a CNOT as a control dot on one wire and a boxed `X` on another, **with
//! no line between them**. On two wires that reads by proximity; on four it is ambiguous, and on a
//! circuit where two two-qubit gates share a column it is unreadable. Standard circuit notation
//! draws the vertical connector, and it costs one 1px `fill_rect` — the same primitive the wires
//! already are. So [`Circuit::connector`] exists and the window draws it.
//!
//! The related consequence is in the packer, not here: a gate may not be scheduled in a column on a
//! wire another gate's connector crosses, or it would be drawn sitting on that connector. See
//! `qclang_compiler::program::Flattened::columns`.
//!
//! ## Units
//!
//! Same rule as `panes.rs` and `layout.rs`: the `const`s are design units against the 1440x900
//! scene and are private; everything public returns real pixels through [`crate::scale::px`].

use crate::layout::Rect;
use crate::scale::px as sc;
use crate::tokens::{Style, B3, LB, MONO};

// ─────────────────────────────────────────────────────────────────────────────
// THE CIRCUIT  —  `.qcirc`, `.qw`, `.gate`
// ─────────────────────────────────────────────────────────────────────────────

/// `.qcirc { padding: 30px 30px 0 }`
const PAD_X: i32 = 30;
const PAD_TOP: i32 = 30;
/// `.qw { height: 54px }`
const WIRE_H: i32 = 54;
/// `.qw .lb { width: 40px }` — the `|0>` column.
const LB_W: i32 = 40;
/// `.gate { width: 32px; height: 32px }`
const GATE: i32 = 32;
/// `.gate { border-radius: 4px }`
const GATE_RADIUS: i32 = 4;
/// The control dot: `width: 9px; border-radius: 5px`.
const DOT: i32 = 9;
/// The narrowest a `.wire` segment may be squeezed before the diagram is declared too wide for its
/// pane. Below this the gates run together and the picture stops reading as a circuit.
const WIRE_MIN: i32 = 12;

/// The style a gate's letter is set in — `.gate { font-size: 12px; font-family: mono }`. One step
/// below [`MONO`], which is the 13px the code pane uses.
pub const GATE_LABEL: Style = Style { px: 12, line_h: 16, tracking: 0, face: MONO.face, upper: false };

/// A resolved circuit diagram: fixed wire rows, evenly-spread gate columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Circuit {
    /// The pane the diagram was laid out in.
    pane: Rect,
    wires: usize,
    cols: usize,
    /// Left edge of the first gate column.
    x0: i32,
    /// Column pitch: one gate plus one wire segment.
    pitch: i32,
    /// True when the columns did not fit and the diagram is clipped at the pane's right edge.
    overflow: bool,
}

/// Lay out `cols` gate columns across `wires` wires inside `pane`.
///
/// The wire segments take the slack, which is what the design's `flex: 1` does — a two-column
/// circuit spreads across the pane rather than huddling at the left. When the slack runs out the
/// segments stop at [`WIRE_MIN`] and [`Circuit::overflow`] goes true; the diagram is then wider than
/// the pane and the caller must say so rather than let a gate half-disappear off the edge.
///
/// ## The row ends at the last gate, with no trailing wire
///
/// `.qw` is `lb · wire · gate · wire · gate · …` — a wire segment before each gate and none after
/// the last, so there are `cols` segments and the diagram finishes flush with the pane's right
/// padding. That is also the correct notation for the circuits this draws: the last column is almost
/// always the measurements, and a wire does not continue past the point where it is measured.
pub fn circuit(pane: Rect, wires: usize, cols: usize) -> Circuit {
    let track_x = pane.x + sc(PAD_X) + sc(LB_W);
    let avail = (pane.right() - sc(PAD_X) - track_x).max(0);
    let gates_w = cols as i32 * sc(GATE);
    let segs = cols as i32;
    let slack = avail - gates_w;
    let (gap, overflow) = if segs > 0 && slack >= segs * sc(WIRE_MIN) {
        (slack / segs, false)
    } else {
        (sc(WIRE_MIN), true)
    };
    Circuit {
        pane,
        wires,
        cols,
        x0: track_x + gap,
        pitch: sc(GATE) + gap,
        overflow: overflow && cols > 0,
    }
}

impl Circuit {
    /// Total height of the wire rows.
    pub fn height(&self) -> i32 {
        sc(PAD_TOP) + self.wires as i32 * sc(WIRE_H)
    }

    /// True when the circuit is wider than its pane. The caller must mark the clip — a diagram
    /// silently missing its last two gates is a diagram of a different program.
    pub fn overflow(&self) -> bool {
        self.overflow
    }

    /// The number of columns that fit whole. Everything from here on is clipped.
    pub fn visible_cols(&self) -> usize {
        if !self.overflow {
            return self.cols;
        }
        let limit = self.pane.right() - sc(PAD_X);
        (0..self.cols).take_while(|&c| self.x0 + c as i32 * self.pitch + sc(GATE) <= limit).count()
    }

    /// The vertical centre of wire `w` — the line the hairline is drawn on and everything on that
    /// wire is centred against.
    pub fn wire_y(&self, w: usize) -> i32 {
        self.pane.y + sc(PAD_TOP) + w as i32 * sc(WIRE_H) + sc(WIRE_H) / 2
    }

    /// The 1px hairline for wire `w`, running the full width of the diagram.
    ///
    /// Drawn edge to edge and the gates painted **over** it, which is how the design's flex row
    /// resolves: the wire is a background the boxes sit on. Painter's order, not clipping — the same
    /// rule `panes::header_mask` documents for the file list.
    pub fn wire(&self, w: usize) -> Option<Rect> {
        if w >= self.wires {
            return None;
        }
        let x = self.pane.x + sc(PAD_X) + sc(LB_W);
        // Up to the right edge of the LAST gate, not past it — see the note on `circuit`. A circuit
        // with no gates at all keeps a full-width wire, because a bare wire is what an empty circuit
        // looks like and a zero-width one looks like a drawing failure.
        let limit = self.pane.right() - sc(PAD_X);
        let right = if self.cols == 0 {
            limit
        } else {
            (self.x0 + (self.cols as i32 - 1) * self.pitch + sc(GATE)).min(limit)
        };
        Some(Rect::new(x, self.wire_y(w), (right - x).max(0), sc(1).max(1)))
    }

    /// Where wire `w`'s `|0>` label is drawn: `(x, top-of-line-box)` for [`GATE_LABEL`].
    pub fn label(&self, w: usize) -> Option<(i32, i32)> {
        if w >= self.wires {
            return None;
        }
        let h = crate::text::line_box(GATE_LABEL);
        Some((self.pane.x + sc(PAD_X), self.wire_y(w) - h / 2))
    }

    /// The 32x32 box for the gate in `(col, wire)`.
    pub fn gate(&self, col: usize, wire: usize) -> Option<Rect> {
        if col >= self.cols || wire >= self.wires {
            return None;
        }
        let x = self.x0 + col as i32 * self.pitch;
        Some(Rect::new(x, self.wire_y(wire) - sc(GATE) / 2, sc(GATE), sc(GATE)))
    }

    /// The control dot in `(col, wire)`.
    ///
    /// Centred on the **connector's** line and the wire's line, not inside the gate box. Those are
    /// not the same pixel: a 9px dot cannot sit exactly in the middle of a 32px box, and centring it
    /// in the box instead would leave the 1px connector emerging half a pixel off the dot's middle —
    /// which at a 1px stroke reads as the vertical line missing the dot entirely.
    pub fn dot(&self, col: usize, wire: usize) -> Option<Rect> {
        let g = self.gate(col, wire)?;
        let d = sc(DOT);
        Some(Rect::new(g.x + g.w / 2 - d / 2, self.wire_y(wire) - d / 2, d, d))
    }

    /// The 1px vertical connector joining wires `a` and `b` in `col`.
    ///
    /// Runs centre to centre, so a gate box drawn afterwards covers the part inside it and the line
    /// emerges cleanly from the edge of the box rather than stopping short of it.
    pub fn connector(&self, col: usize, a: usize, b: usize) -> Option<Rect> {
        if col >= self.cols || a >= self.wires || b >= self.wires {
            return None;
        }
        let g = self.gate(col, a.min(b))?;
        let (top, bottom) = (self.wire_y(a.min(b)), self.wire_y(a.max(b)));
        Some(Rect::new(g.x + g.w / 2, top, sc(1).max(1), (bottom - top).max(1)))
    }

    /// The cell under `(mx, my)`, using the **whole column pitch and wire row** rather than the 32px
    /// box.
    ///
    /// Widened for the same reason `panes::nav_hit` is: the space around a gate reads as belonging
    /// to it, and a pointer that only responds inside a 32px square feels broken on a diagram whose
    /// columns are 90px apart. A hit is still only reported where the caller says something is.
    pub fn cell_at(&self, mx: i32, my: i32) -> Option<(usize, usize)> {
        if self.cols == 0 || self.wires == 0 || self.pitch <= 0 {
            return None;
        }
        let rel_x = mx - (self.x0 - (self.pitch - sc(GATE)) / 2);
        if rel_x < 0 {
            return None;
        }
        let col = (rel_x / self.pitch) as usize;
        if col >= self.cols {
            return None;
        }
        let rel_y = my - (self.pane.y + sc(PAD_TOP));
        if rel_y < 0 {
            return None;
        }
        let wire = (rel_y / sc(WIRE_H)) as usize;
        if wire >= self.wires {
            return None;
        }
        Some((col, wire))
    }
}

/// `.gate { border-radius: 4px }`, in real pixels, for `shapes::stroke_round_rect`.
pub fn gate_radius() -> usize {
    sc(GATE_RADIUS).max(1) as usize
}

/// `.qcirc { padding: 30px }` — the pane's horizontal inset.
pub fn pad_x() -> i32 {
    sc(PAD_X)
}

/// `.qcirc { padding-top: 30px }` — the gap between the pane's top and the first wire row.
///
/// A caller that places the diagram somewhere other than the top of a pane (under a heading, say)
/// subtracts this from the origin it wants, so that [`Circuit::wire_y`] lands where it meant. Both
/// the drawing and the hit-testing must use the same expression, which is why it is here and not a
/// literal at each call site.
pub fn pad_top() -> i32 {
    sc(PAD_TOP)
}

/// `.qw { height: 54px }` — one wire row.
pub fn wire_h() -> i32 {
    sc(WIRE_H)
}

// ─────────────────────────────────────────────────────────────────────────────
// THE MEASUREMENT TABLE  —  `.mrow`, `.trk`
// ─────────────────────────────────────────────────────────────────────────────

/// `.mrow { padding: 9px 0 }` in the QCLang scene, against the component sheet's default 7px.
const MROW_PAD_Y: i32 = 9;
/// `.trk { width: 130px; height: 2px }`
const TRK_W: i32 = 130;
const TRK_H: i32 = 2;
/// The gap between the bar and the number, and the number's own column.
///
/// The design reserves 34px for a shot count (`512`). This shows an exact probability, so the widest
/// string is `100.0%` — 46px of 12px mono, not 34 — and a column sized for the design's number would
/// push the longest value out of alignment with the rest.
const VALUE_W: i32 = 46;
const VALUE_GAP: i32 = 16;

/// One row of the measurement table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The whole row, for hit-testing and for the hairline if one is drawn.
    pub rect: Rect,
    /// `(x, y)` to draw the basis-state label at, in [`GATE_LABEL`].
    pub label: (i32, i32),
    /// The 2px bar. Empty width when the probability is zero — see [`outcome_fill`].
    pub track: Rect,
    /// The right edge the probability is right-aligned to.
    pub value_right: i32,
}

/// Row `i` of the measurement table, laid out from `top` inside `pane`.
pub fn outcome_row(pane: Rect, top: i32, i: usize) -> Outcome {
    let h = outcome_row_height();
    let y = top + i as i32 * h;
    let x = pane.x + sc(PAD_X);
    let right = pane.right() - sc(PAD_X);
    let rect = Rect::new(x, y, (right - x).max(0), h);

    // `align-items: baseline`: the label, the number and the bar all hang off one baseline rather
    // than being independently centred. The bar has no baseline of its own, so it is placed on the
    // one the text sits on — which is where a reader's eye already is.
    let base = y + sc(MROW_PAD_Y) + crate::text::ascent(GATE_LABEL);
    let value_right = right;
    let trk_right = value_right - sc(VALUE_W) - sc(VALUE_GAP);
    Outcome {
        rect,
        label: (x, base - crate::text::ascent(GATE_LABEL)),
        track: Rect::new(trk_right - sc(TRK_W), base - sc(TRK_H), sc(TRK_W), sc(TRK_H)),
        value_right,
    }
}

/// The height of one measurement row: `9px` of padding either side of a line of [`GATE_LABEL`].
pub fn outcome_row_height() -> i32 {
    2 * sc(MROW_PAD_Y) + crate::text::line_box(GATE_LABEL)
}

/// The height of `n` rows.
pub fn outcomes_height(n: usize) -> i32 {
    n as i32 * outcome_row_height()
}

/// The accent-filled part of a track for a probability in 0.0..=1.0.
///
/// ⚠️ A **non-zero** probability always gets at least one pixel. Rounding 0.004 to an empty bar makes
/// an outcome that can happen look exactly like one that cannot, and "cannot happen" is the single
/// most informative thing this table says — it is the whole content of the Bell pair sitting beside
/// it. The design encodes that distinction by tinting the impossible rows' track `--fg-3` instead of
/// the accent (`.trk.n`); this makes sure the geometry agrees with the colour.
pub fn outcome_fill(track: Rect, p: f64) -> Rect {
    let p = p.clamp(0.0, 1.0);
    let w = (track.w as f64 * p) as i32;
    let w = if p > 0.0 { w.max(1) } else { 0 };
    Rect::new(track.x, track.y, w.min(track.w), track.h)
}

/// The `.lb` section heading's own line height, for stacking a labelled block.
pub fn heading_height() -> i32 {
    crate::text::line_box(LB)
}

/// The style the caption-line context is set in — `b3`, matching the design's `.sub`.
pub const NOTE: Style = B3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::{register_mono, register_text};
    use alloc::vec::Vec;

    fn pane() -> Rect {
        // `.qcirc` at the design's own width, inside a window body 540 tall.
        Rect::new(688, 0, 400, 540)
    }

    /// The scale is 1.0 until a shell calls `scale::init`, which no test does — so `sc()` is the
    /// identity here and the assertions below read as the design's own CSS.
    fn setup() {
        register_text();
        register_mono();
    }

    /// The design's own scene: two wires, three columns, in a 400px pane. Every metric below is read
    /// straight off the CSS.
    #[test]
    fn a_two_wire_three_column_circuit_matches_the_design_metrics() {
        setup();
        let c = circuit(pane(), 2, 3);
        assert!(!c.overflow(), "the design's own circuit does not fit its own pane");

        // Rows are 54px tall and start 30px down.
        assert_eq!(c.wire_y(0), 0 + 30 + 27);
        assert_eq!(c.wire_y(1) - c.wire_y(0), 54);

        // Gates are 32x32 and centred on the wire.
        let g = c.gate(0, 0).unwrap();
        assert_eq!((g.w, g.h), (32, 32));
        assert_eq!(g.y + g.h / 2, c.wire_y(0));

        // The label column is 40px wide and the track starts after it.
        let (lx, _) = c.label(0).unwrap();
        assert_eq!(lx, 688 + 30);
        assert_eq!(c.wire(0).unwrap().x, 688 + 30 + 40);
    }

    /// `.wire { flex: 1 }` — the slack goes into the wire segments, so a short circuit spreads
    /// across the pane instead of bunching at the left. Columns stay evenly pitched.
    #[test]
    fn wire_segments_take_up_the_slack_and_columns_stay_evenly_pitched() {
        setup();
        let c = circuit(pane(), 2, 3);
        let xs: Vec<i32> = (0..3).map(|i| c.gate(i, 0).unwrap().x).collect();
        assert_eq!(xs[1] - xs[0], xs[2] - xs[1], "columns are not evenly pitched");
        assert!(xs[1] - xs[0] > 32, "the wire segments collapsed to nothing");
        // The last gate stays inside the pane's right padding.
        assert!(c.gate(2, 0).unwrap().right() <= pane().right() - 30);
    }

    /// ★ A circuit too wide for its pane must SAY so. Silently clipping the last gates draws a
    /// picture of a different program, and on a diagram there is no scrollbar to hint at it.
    #[test]
    fn a_circuit_too_wide_for_its_pane_reports_overflow_and_a_visible_count() {
        setup();
        let c = circuit(pane(), 2, 40);
        assert!(c.overflow(), "40 columns cannot fit 400px");
        let vis = c.visible_cols();
        assert!(vis > 0 && vis < 40, "visible_cols reported {}", vis);
        // Every column it claims is visible really is inside the pane.
        for col in 0..vis {
            assert!(
                c.gate(col, 0).unwrap().right() <= pane().right() - 30,
                "column {} was reported visible but overhangs",
                col
            );
        }
        // ...and the first one it excludes really does overhang.
        assert!(c.gate(vis, 0).unwrap().right() > pane().right() - 30);
    }

    /// The row ends AT the last gate: `cols` wire segments, none trailing. So the diagram finishes
    /// flush with the pane's right padding and the wire stops where the measurement does.
    #[test]
    fn the_row_ends_at_the_last_gate_with_no_trailing_wire() {
        setup();
        let c = circuit(pane(), 2, 3);
        let last = c.gate(2, 0).unwrap();
        assert_eq!(last.right(), pane().right() - 30, "the last gate is not flush right");
        assert_eq!(c.wire(0).unwrap().right(), last.right(), "the wire runs past the last gate");
    }

    /// A one-column circuit is the degenerate case the flex maths divides by. It must not panic, and
    /// its single wire segment must still separate the `|0>` label from the gate.
    #[test]
    fn a_single_column_circuit_is_laid_out_without_dividing_by_zero() {
        setup();
        let c = circuit(pane(), 1, 1);
        assert!(!c.overflow());
        let g = c.gate(0, 0).unwrap();
        let track = c.wire(0).unwrap();
        assert!(g.x > track.x, "the gate sits on top of the label");
        assert_eq!(track.right(), g.right());
    }

    /// A circuit with no gates at all still draws its wires — otherwise `qubit a = |0>;` with no
    /// operations renders as a blank pane, which looks like a failure rather than like an empty
    /// circuit.
    #[test]
    fn a_circuit_with_wires_but_no_gates_still_draws_full_width_wires() {
        setup();
        let c = circuit(pane(), 2, 0);
        let w = c.wire(0).unwrap();
        assert_eq!(w.x, pane().x + 30 + 40);
        assert_eq!(w.right(), pane().right() - 30);
        assert!(c.wire(1).is_some());
    }

    /// An empty circuit — a program with no gates — must not panic and must offer nothing.
    #[test]
    fn an_empty_circuit_answers_nothing_rather_than_panicking() {
        setup();
        let c = circuit(pane(), 0, 0);
        assert!(!c.overflow());
        assert_eq!(c.visible_cols(), 0);
        assert!(c.gate(0, 0).is_none());
        assert!(c.wire(0).is_none());
        assert!(c.label(0).is_none());
        assert!(c.dot(0, 0).is_none());
        assert!(c.connector(0, 0, 1).is_none());
        assert!(c.cell_at(700, 60).is_none());
    }

    /// ★ The connector runs between the two wires' centres, so the gate box drawn over it hides
    /// exactly the part that would otherwise cross the letter.
    #[test]
    fn the_connector_spans_centre_to_centre_and_is_covered_by_its_gates() {
        setup();
        let c = circuit(pane(), 3, 2);
        let conn = c.connector(1, 0, 2).unwrap();
        assert_eq!(conn.y, c.wire_y(0));
        assert_eq!(conn.bottom(), c.wire_y(2));
        assert_eq!(conn.w, 1);
        // Centred on the column, so it emerges from the middle of each box.
        let g = c.gate(1, 0).unwrap();
        assert_eq!(conn.x, g.x + g.w / 2);
        // Order of the two wires must not matter.
        assert_eq!(c.connector(1, 2, 0), Some(conn));
    }

    /// ★ The dot's middle pixel must be the connector's pixel and the wire's pixel. An odd-width dot
    /// cannot also be centred in an even-width box, and choosing the box would put the 1px connector
    /// half a pixel off the dot — visibly, at a 1px stroke.
    #[test]
    fn the_control_dot_is_centred_on_the_connector_and_the_wire() {
        setup();
        let c = circuit(pane(), 2, 2);
        let d = c.dot(1, 0).unwrap();
        assert_eq!((d.w, d.h), (9, 9));
        let conn = c.connector(1, 0, 1).unwrap();
        assert_eq!(d.x + d.w / 2, conn.x, "the connector does not run through the dot's middle");
        assert_eq!(d.y + d.h / 2, c.wire_y(0), "the dot does not sit on its wire");
        // And it stays inside the cell it belongs to.
        let g = c.gate(1, 0).unwrap();
        assert!(d.x >= g.x && d.right() <= g.right() && d.y >= g.y && d.bottom() <= g.bottom());
    }

    /// Hit-testing must agree with drawing on every cell, and must decline outside the diagram.
    #[test]
    fn every_gate_is_hit_at_its_own_centre_and_nowhere_outside_the_diagram() {
        setup();
        let c = circuit(pane(), 3, 4);
        for col in 0..4 {
            for wire in 0..3 {
                let g = c.gate(col, wire).unwrap();
                assert_eq!(
                    c.cell_at(g.x + g.w / 2, g.y + g.h / 2),
                    Some((col, wire)),
                    "cell ({}, {}) does not hit-test to itself",
                    col, wire
                );
            }
        }
        assert_eq!(c.cell_at(0, 0), None, "hit far left of the pane");
        assert_eq!(c.cell_at(700, -50), None, "hit above the diagram");
        assert_eq!(c.cell_at(700, 10_000), None, "hit below the last wire");
        assert_eq!(c.cell_at(10_000, 60), None, "hit past the last column");
    }

    /// ★ An outcome that CAN happen must never draw as an empty bar. That is the difference between
    /// "0.4% of the time" and "never", and never is what the table is mostly saying.
    #[test]
    fn a_possible_outcome_always_gets_at_least_one_pixel() {
        setup();
        let t = outcome_row(pane(), 0, 0).track;
        assert_eq!(outcome_fill(t, 0.0).w, 0, "an impossible outcome drew a bar");
        assert_eq!(outcome_fill(t, 0.000_1).w, 1, "a possible outcome drew nothing");
        assert_eq!(outcome_fill(t, 1.0).w, t.w);
        assert_eq!(outcome_fill(t, 0.5).w, t.w / 2);
        // Out-of-range input clamps rather than overflowing the track.
        assert_eq!(outcome_fill(t, 2.0).w, t.w);
        assert_eq!(outcome_fill(t, -1.0).w, 0);
    }

    /// The rows stack without overlapping and each one's track is the design's 130x2.
    #[test]
    fn measurement_rows_stack_evenly_with_a_130px_track() {
        setup();
        let top = 300;
        let rows: Vec<Outcome> = (0..4).map(|i| outcome_row(pane(), top, i)).collect();
        for i in 1..4 {
            assert_eq!(rows[i].rect.y, rows[i - 1].rect.bottom(), "row {} overlaps", i);
        }
        assert_eq!(outcomes_height(4), rows[3].rect.bottom() - top);
        for r in &rows {
            assert_eq!((r.track.w, r.track.h), (130, 2));
            assert!(r.track.right() < r.value_right, "the bar runs into the number");
            assert!(r.rect.contains(r.track.x, r.track.y), "the bar escaped its row");
        }
    }

    /// `align-items: baseline` — the label and the bar share one baseline, so the bar sits under the
    /// text rather than floating at the row's vertical centre.
    #[test]
    fn the_bar_sits_on_the_labels_baseline() {
        setup();
        let r = outcome_row(pane(), 0, 0);
        let baseline = r.label.1 + crate::text::ascent(GATE_LABEL);
        assert_eq!(r.track.bottom(), baseline, "the bar is off the baseline");
    }
}
