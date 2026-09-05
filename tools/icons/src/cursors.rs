//! Generates `libs/meridian/src/cursors_gen.rs` from `10-cursors.html`.
//!
//! ## Why this is ARGB and the icons are coverage
//!
//! Icons are coverage bitmaps because they ride the GPU text shader, which carries one luminance
//! and can therefore only be greyscale. The cursor is the exact opposite case: the hardware cursor
//! plane is a 64×64 ARGB surface alpha-composited by the SCANOUT engine after everything else, so it
//! never meets that shader, never costs a quad, and colour is free. Two of the twelve shapes use
//! that — the grabbing hand and the working ring are accent-tinted — and every shape uses it for the
//! thing that actually makes a cursor work: a dark contrast edge under a white body, so that one
//! bitmap is legible on a black terminal and on a white document without swapping per theme.
//!
//! So the compositing happens here, at build time, and the runtime gets finished pixels.
//!
//! ## Why compositing here rather than at run time
//!
//! The three layers could be shipped as separate coverage planes and combined on Nyx, which would
//! be about 40% smaller. It would also silently reorder `#c-unavailable`: its slashed circle is
//! drawn *over* the pointer glyph in the document, so the circle's dark edge cuts into the pointer's
//! white body where the two overlap. Compositing plane-by-plane instead of part-by-part puts the
//! body back on top and loses that. Doing it here keeps document order, which is the only order the
//! design ever specified.
//!
//! ## One coordinate rule
//!
//! Every shape is drawn on the same 24 grid as the icon set, inside a `-2 -2 26 26` viewBox — the
//! bleed is room for the 3.4-unit dark edge. **One grid unit is one plane pixel**, which is what
//! makes the design's own numbers ("a chevron reads correctly at 1.5px stroke with a 3.4px dark edge
//! behind it") true rather than approximate. The art therefore occupies 26×26 of the 64×64 plane at
//! its top-left, and grid (0,0) lands at plane (2,2) — so every hotspot below, which the design
//! states in GRID coordinates, is emitted with the bleed added.

use crate::raster::{self, Paint};
use crate::svg::{self, Part, Symbol};

/// The design part that defines the cursor set, relative to the `Nyx-ui` root.
pub const SOURCE: &str = "design/ver3.0/parts/10-cursors.html";

/// Height of the rendered viewBox in pixels. Equal to the viewBox height, i.e. scale 1:1 — see the
/// coordinate rule in the module note. Not a knob: changing it silently changes every stroke width.
const ART_PX: usize = 26;

/// The viewBox origin, negated. Grid (0,0) is at plane (BLEED, BLEED).
const BLEED: u32 = 2;

/// The three colours in the set, from the `.cur` rules in the design's own stylesheet.
const EDGE_RGB: u32 = 0x0A0B0C;
const BODY_RGB: u32 = 0xFFFFFF;
const ACCENT_RGB: u32 = 0x4676F0;

/// `.cur .fe` and `.cur .e` carry `opacity: .9`. It matters: a fully opaque black edge under a white
/// body reads as a sticker, and 0.9 is what keeps the pointer looking drawn rather than pasted.
const EDGE_OPACITY: f64 = 0.9;

/// Stroke widths, in grid units, from the stylesheet.
const W_EDGE_FILL: f64 = 2.4; // .fe — a stroke *around* a filled shape, to grow the edge outward
const W_EDGE: f64 = 3.4; // .e
const W_BODY: f64 = 1.5; // .b

/// Every cursor, in the order the design's own specimen grid lists them, with the hotspot printed
/// under each specimen — in GRID coordinates.
///
/// This is the one piece of the cursor set that is not derived from the document, because it is not
/// *in* the document as data: it is prose under each picture. Listed here so that a shape added to
/// the design without a hotspot stops the build rather than defaulting to the top-left corner and
/// pointing 12px away from where it looks like it points.
const HOTSPOTS: [(&str, u32, u32); 12] = [
    ("c-pointer", 0, 0),
    ("c-text", 10, 12),
    ("c-precision", 12, 12),
    ("c-move", 12, 12),
    ("c-resize-h", 12, 12),
    ("c-resize-v", 12, 12),
    ("c-resize-nwse", 12, 12),
    ("c-resize-nesw", 12, 12),
    ("c-grab", 12, 12),
    ("c-grabbing", 12, 12),
    ("c-working", 12, 12),
    ("c-unavailable", 0, 0),
];

/// One resolved paint: what colour, how opaque, and the geometry rule.
#[derive(Clone, Copy)]
struct Layer {
    rgb: u32,
    opacity: f64,
    paint: Paint,
}

/// Resolve an element's `class` against the design's stylesheet.
///
/// Errors rather than defaulting. A class this does not know is a shape the design draws and this
/// tool does not, and an invisible cursor layer is not something a glance at the result would catch
/// — the picture stays recognisable, it just loses its contrast edge on one ground.
fn layer_for(sym: &str, part: &Part) -> Result<Layer, String> {
    let classes: Vec<&str> = part.class.split_whitespace().collect();
    let butt_caps = classes.contains(&"sq");
    let accent = classes.contains(&"ba") || classes.contains(&"da");
    let base = classes.first().copied().unwrap_or("");
    let (rgb, opacity, paint) = match base {
        // Filled pointer silhouette, dark, plus a stroke that grows it outward into an edge.
        "fe" => (
            EDGE_RGB,
            EDGE_OPACITY,
            Paint { fill: true, stroke: Some(W_EDGE_FILL), butt_caps },
        ),
        // The same silhouette in white, unstroked, sitting inside the edge.
        "fb" => (BODY_RGB, 1.0, Paint { fill: true, stroke: None, butt_caps }),
        // A stroked contrast edge. `fill: none` in the stylesheet, so a <circle class="e"> is a ring.
        "e" => (EDGE_RGB, EDGE_OPACITY, Paint { fill: false, stroke: Some(W_EDGE), butt_caps }),
        // The visible line on top. `ba` recolours it without changing its geometry.
        "b" => (
            if accent { ACCENT_RGB } else { BODY_RGB },
            1.0,
            Paint { fill: false, stroke: Some(W_BODY), butt_caps },
        ),
        // A solid accent dot — the working cursor's pupil.
        "da" => (ACCENT_RGB, 1.0, Paint { fill: true, stroke: None, butt_caps }),
        other => {
            return Err(format!(
                "{}: element has class '{}', which the cursor stylesheet does not define. Add it \
                 here rather than letting the layer render blank.",
                sym, other
            ))
        }
    };
    Ok(Layer { rgb, opacity, paint })
}

/// A composited cursor: straight-alpha ARGB at `w * h`, plus where it sits in the 64×64 plane.
pub struct Composed {
    pub id: String,
    pub w: usize,
    pub h: usize,
    pub off_x: usize,
    pub off_y: usize,
    pub hot_x: u32,
    pub hot_y: u32,
    pub argb: Vec<u32>,
}

/// Rasterize each part in document order and composite it over what is already there.
pub fn compose(sym: &Symbol, hot_x: u32, hot_y: u32) -> Result<Composed, String> {
    let n = ART_PX * ART_PX;
    // Accumulate in unpremultiplied float so the src-over below is the textbook formula and not an
    // approximation that happens to look right for opaque interiors.
    let mut acc = vec![(0.0f64, 0.0f64, 0.0f64, 0.0f64); n]; // r, g, b, a

    for (i, part) in sym.parts.iter().enumerate() {
        let layer = layer_for(&sym.id, part)?;
        // One part at a time: `rasterize_parts` selects by identity so a symbol that draws the same
        // path twice (which every cursor does) gets two independent coverage passes.
        let bm = raster::rasterize_parts(sym, ART_PX, |p| {
            (core::ptr::eq(p, part)).then_some(layer.paint)
        });
        if bm.w != ART_PX || bm.h != ART_PX {
            return Err(format!(
                "{}: part {} rasterized to {}x{}, expected {}x{} — the viewBox is not square",
                sym.id, i, bm.w, bm.h, ART_PX, ART_PX
            ));
        }
        let (sr, sg, sb) = unpack(layer.rgb);
        for p in 0..n {
            let sa = (bm.cov[p] as f64 / 255.0) * layer.opacity;
            if sa <= 0.0 {
                continue;
            }
            let (dr, dg, db, da) = acc[p];
            let out_a = sa + da * (1.0 - sa);
            let mix = |s: f64, d: f64| (s * sa + d * da * (1.0 - sa)) / out_a;
            acc[p] = (mix(sr, dr), mix(sg, dg), mix(sb, db), out_a);
        }
    }

    let argb: Vec<u32> = acc
        .iter()
        .map(|&(r, g, b, a)| {
            if a <= 0.0 {
                return 0;
            }
            (q(a) << 24) | (q(r) << 16) | (q(g) << 8) | q(b)
        })
        .collect();

    let (argb, w, h, off_x, off_y) = crop(&argb, ART_PX, ART_PX);
    if w == 0 {
        return Err(format!("{}: composited to nothing", sym.id));
    }
    Ok(Composed {
        id: sym.id.clone(),
        w,
        h,
        off_x,
        off_y,
        hot_x: hot_x + BLEED,
        hot_y: hot_y + BLEED,
        argb,
    })
}

fn unpack(rgb: u32) -> (f64, f64, f64) {
    (
        ((rgb >> 16) & 0xFF) as f64 / 255.0,
        ((rgb >> 8) & 0xFF) as f64 / 255.0,
        (rgb & 0xFF) as f64 / 255.0,
    )
}

fn q(v: f64) -> u32 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u32
}

/// Trim fully transparent rows and columns. The pointer is tall and narrow and the resize chevrons
/// sit well inside their box; keeping the full square would put roughly a third more nothing in the
/// binary than there are pixels.
fn crop(argb: &[u32], w: usize, h: usize) -> (Vec<u32>, usize, usize, usize, usize) {
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
    for y in 0..h {
        for x in 0..w {
            if argb[y * w + x] >> 24 != 0 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return (Vec::new(), 0, 0, 0, 0);
    }
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut out = vec![0u32; cw * ch];
    for y in 0..ch {
        out[y * cw..(y + 1) * cw].copy_from_slice(&argb[(y + y0) * w + x0..(y + y0) * w + x0 + cw]);
    }
    (out, cw, ch, x0, y0)
}

/// Read the design document and compose every cursor in `HOTSPOTS`, in that order.
pub fn build(src: &str) -> Result<Vec<Composed>, String> {
    let symbols = svg::parse_document(src, &HOTSPOTS.iter().map(|h| h.0).collect::<Vec<_>>())?;
    let mut out = Vec::new();
    for &(id, hx, hy) in &HOTSPOTS {
        let sym = symbols
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(|| format!("{} does not define a symbol '{}'", SOURCE, id))?;
        out.push(compose(sym, hx, hy)?);
    }
    // The reverse direction: a shape added to the design but not given a hotspot here.
    for sym in &symbols {
        if !HOTSPOTS.iter().any(|h| h.0 == sym.id) {
            return Err(format!(
                "{} defines '{}', which has no hotspot in HOTSPOTS. A cursor without a declared \
                 hotspot points at its top-left corner.",
                SOURCE, sym.id
            ));
        }
    }
    Ok(out)
}

/// Draw a composed cursor as text, the way `--preview` does for icons. A cursor whose contrast edge
/// has vanished still rasterizes to a valid picture; the only way to notice is to look.
pub fn preview(c: &Composed) {
    println!(
        "\n{}  {}x{} at ({},{})  hot ({},{}) in plane coords",
        c.id, c.w, c.h, c.off_x, c.off_y, c.hot_x, c.hot_y
    );
    for y in 0..c.h {
        let mut row = String::new();
        for x in 0..c.w {
            let p = c.argb[y * c.w + x];
            let a = p >> 24;
            let (r, b) = ((p >> 16) & 0xFF, p & 0xFF);
            row.push(if a == 0 {
                ' '
            } else if b > r + 40 {
                // Accent is the only colour where blue leads.
                if a > 160 { 'A' } else { 'a' }
            } else if r > 128 {
                if a > 160 { '#' } else { '+' }
            } else if a > 160 {
                'O'
            } else {
                '.'
            });
        }
        println!("|{}|", row);
    }
    println!("  ' ' clear  · '.'/'O' dark edge  · '+'/'#' white body  · 'a'/'A' accent");
}

/// Emit the generated Rust.
pub fn emit(cursors: &[Composed]) -> String {
    let mut body = String::new();
    let mut entries = String::new();
    let mut total = 0usize;
    for c in cursors {
        let name = c
            .id
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch.to_ascii_uppercase() } else { '_' })
            .collect::<String>();
        total += c.argb.len() * 4;
        body.push_str(&format!("static {}: [u32; {}] = [\n", name, c.argb.len()));
        for chunk in c.argb.chunks(8) {
            body.push_str("    ");
            for p in chunk {
                body.push_str(&format!("{:#010x},", p));
            }
            body.push('\n');
        }
        body.push_str("];\n");
        entries.push_str(&format!(
            "    Cursor {{ id: {:?}, w: {}, h: {}, off_x: {}, off_y: {}, hot_x: {}, hot_y: {}, \
             argb: &{} }},\n",
            c.id, c.w, c.h, c.off_x, c.off_y, c.hot_x, c.hot_y, name
        ));
    }

    format!(
        "// @generated by `cargo run -p nyx-icons-gen` — DO NOT EDIT BY HAND.\n\
         //\n\
         // Source: Nyx-ui/{}\n\
         // {} cursors, composited at 1 grid unit per pixel inside a 26x26 art box.\n\
         // {} bytes of straight-alpha ARGB.\n\
         //\n\
         // Straight (non-premultiplied) alpha: the pixel is the colour it looks like, and `a` is how\n\
         // much of it lands. The interior of every shape is fully opaque, so this only describes the\n\
         // antialiased rim — which is why picking the wrong convention here would be a soft halo\n\
         // rather than an obviously broken cursor, and why it is worth writing down which one it is.\n\
         \n\
         /// One cursor shape, ready to blit into the 64x64 hardware cursor plane.\n\
         pub struct Cursor {{\n\
         \x20   /// The `<symbol>` id from the design document, e.g. `\"c-pointer\"`.\n\
         \x20   pub id: &'static str,\n\
         \x20   pub w: u8,\n\
         \x20   pub h: u8,\n\
         \x20   /// Where the cropped bitmap sits inside the plane.\n\
         \x20   pub off_x: u8,\n\
         \x20   pub off_y: u8,\n\
         \x20   /// The pixel that IS the pointer, in PLANE coordinates — pass it to\n\
         \x20   /// `sys_cursor_set_image`, which subtracts it before writing CURPOS.\n\
         \x20   pub hot_x: u8,\n\
         \x20   pub hot_y: u8,\n\
         \x20   /// `w * h` pixels, row-major, 0xAARRGGBB.\n\
         \x20   pub argb: &'static [u32],\n\
         }}\n\
         \n{}\n\
         /// Every cursor in the Meridian set, in the order the design's specimen grid lists them.\n\
         pub static CURSORS: [Cursor; {}] = [\n{}];\n\
         \n\
         /// Look a cursor up by its design-document id.\n\
         pub fn find(id: &str) -> Option<&'static Cursor> {{\n\
         \x20   CURSORS.iter().find(|c| c.id == id)\n\
         }}\n",
        SOURCE,
        cursors.len(),
        total,
        body,
        cursors.len(),
        entries
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole set, parsed from the checked-in design document if it is present.
    ///
    /// The tools tests run on a host that may not have the `Nyx-ui` checkout, so these skip rather
    /// than fail when it is absent — the generator itself already errors loudly in that case, and a
    /// test that fails on a fresh clone teaches people to ignore it.
    fn set() -> Option<Vec<Composed>> {
        let root = crate::default_ui_root();
        let src = std::fs::read_to_string(root.join(SOURCE)).ok()?;
        Some(build(&src).expect("the checked-in cursor document must compose"))
    }

    fn opaque(p: u32) -> bool {
        p >> 24 > 128
    }
    fn is_dark(p: u32) -> bool {
        opaque(p) && (p >> 16) & 0xFF < 64
    }
    fn is_accent(p: u32) -> bool {
        opaque(p) && (p & 0xFF) > ((p >> 16) & 0xFF) + 40
    }

    #[test]
    fn every_shape_has_a_dark_edge_under_a_visible_body() {
        // This is the property that makes ONE bitmap work on a black terminal and a white document.
        // A shape that lost its edge is still a perfectly good picture — and invisible on white.
        //
        // The body is "light OR accent" rather than "light": the grabbing hand has no white in it
        // at all, only the accent, which is the design's way of saying the drag is live.
        let Some(set) = set() else { return };
        assert_eq!(set.len(), 12);
        for c in &set {
            let dark = c.argb.iter().filter(|&&p| is_dark(p)).count();
            let body = c
                .argb
                .iter()
                .filter(|&&p| opaque(p) && (((p >> 16) & 0xFF) > 192 || is_accent(p)))
                .count();
            assert!(dark > 20, "{} has {} dark edge pixels", c.id, dark);
            assert!(body > 10, "{} has {} body pixels", c.id, body);
        }
    }

    #[test]
    fn the_hotspot_lands_on_the_shape() {
        // A hotspot adrift from the picture means the pointer points at nothing — the symptom of a
        // hotspot written in the wrong coordinate system, which is the risk of adding the viewBox
        // bleed by hand.
        //
        // "On the shape" and not "on ink": the precision crosshair has a deliberate gap at its
        // centre and the grab bracket is hollow, and in both cases that hole is exactly where the
        // cursor points. So for the ten centred shapes the assertion is that the hotspot is the
        // centre of the shape's own inked bounding box — which is the property that actually breaks
        // if the bleed is added twice or the crop offset is forgotten. The two pointer shapes are
        // the exception the design names, and there the tip must carry ink.
        let Some(set) = set() else { return };
        for c in &set {
            let (hx, hy) = (c.hot_x as i32 - c.off_x as i32, c.hot_y as i32 - c.off_y as i32);
            assert!(
                hx >= 0 && hy >= 0 && hx < c.w as i32 && hy < c.h as i32,
                "{}: hotspot ({},{}) is outside its {}x{} bitmap at ({},{})",
                c.id, c.hot_x, c.hot_y, c.w, c.h, c.off_x, c.off_y
            );
            if c.id == "c-pointer" || c.id == "c-unavailable" {
                let p = c.argb[hy as usize * c.w + hx as usize];
                assert!(p >> 24 > 0, "{}: the tip pixel is transparent", c.id);
                continue;
            }
            let (cx, cy) = ((c.w as i32 - 1) / 2, (c.h as i32 - 1) / 2);
            assert!(
                (hx - cx).abs() <= 1 && (hy - cy).abs() <= 1,
                "{}: hotspot is at ({},{}) of a {}x{} bitmap; centre is ({},{})",
                c.id, hx, hy, c.w, c.h, cx, cy
            );
        }
    }

    #[test]
    fn only_the_two_accent_shapes_carry_colour() {
        // The design allows accent on exactly two cursors. Colour leaking into a third would mean a
        // class was misread — `ba` applied where only `b` was meant — and the working cursor's
        // pupil is a 3px disc, so "any accent at all" is the only threshold that is not arbitrary.
        let Some(set) = set() else { return };
        for c in &set {
            let blue = c.argb.iter().filter(|&&p| is_accent(p)).count();
            let expect = c.id == "c-grabbing" || c.id == "c-working";
            assert_eq!(blue > 0, expect, "{} has {} accent pixels", c.id, blue);
        }
    }

    #[test]
    fn the_text_cursor_has_flat_ends() {
        // `stroke-linecap: butt`. With round caps the 3.4-wide edge bulges 1.7px past each end and
        // the I-beam reads as a capsule rather than a bar. The tell is the ink profile down the
        // stem: butt caps make every row identical, round caps taper the first and last few.
        let Some(set) = set() else { return };
        let t = set.iter().find(|c| c.id == "c-text").unwrap();
        let row_ink = |y: usize| -> u32 {
            (0..t.w).map(|x| t.argb[y * t.w + x] >> 24).sum()
        };
        let mid = row_ink(t.h / 2);
        assert!(mid > 0);
        assert_eq!(row_ink(0), mid, "the top row should be as wide as the middle");
        assert_eq!(row_ink(t.h - 1), mid, "and so should the bottom");
    }

    #[test]
    fn the_pointer_tip_is_at_the_top_left() {
        // The one shape whose hotspot is not its centre. If the bleed were added twice, or the
        // viewBox origin applied twice, the tip would drift away from (2,2).
        let Some(set) = set() else { return };
        let p = set.iter().find(|c| c.id == "c-pointer").unwrap();
        assert_eq!((p.hot_x, p.hot_y), (2, 2));
        assert_eq!((p.off_x, p.off_y), (0, 0), "the dark edge should reach the box corner");
    }

    #[test]
    fn an_unknown_class_stops_the_build() {
        let sym = Symbol {
            id: "c-test".into(),
            vb_x: -2.0,
            vb_y: -2.0,
            vb_w: 26.0,
            vb_h: 26.0,
            parts: vec![Part {
                ink: svg::Ink::Stroke,
                class: "wat".into(),
                polys: svg::parse_path("M4 12H20").unwrap(),
            }],
        };
        assert!(compose(&sym, 12, 12).is_err());
    }
}
