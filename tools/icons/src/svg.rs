//! Reads the Meridian icon set out of the design document and flattens every shape to polylines in
//! the 24-unit viewBox space.
//!
//! The design file is the single source of truth for the icons — they are `<symbol>` definitions in
//! `Nyx-ui/design/ver3.0/parts/02-icons.html`, and there is deliberately no second copy of the path
//! data anywhere. This scanner is what keeps it that way.
//!
//! Everything is flattened to polylines rather than kept as curves, because the rasterizer works by
//! distance-to-segment (see `raster.rs`). That choice is what removes the need for a stroker: the
//! design draws icons with `stroke-linecap: round` and `stroke-linejoin: round`, and the distance
//! field to a set of segments *is* a round-capped, round-joined stroke. Building an actual outline
//! stroker — offsetting curves, mitring joins, handling self-intersection — would be several
//! hundred lines to arrive at the same picture.

use std::collections::HashMap;
use std::f64::consts::PI;

/// How a shape is painted. The design strokes almost everything at a hairline on the 24 grid; the
/// handful of solid dots (an Entity pupil, the `#u-warn` full stop) carry
/// `fill="currentColor" stroke="none"` and are filled instead.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ink {
    Stroke,
    Fill,
}

/// A flattened contour in viewBox units.
#[derive(Clone, Debug)]
pub struct Poly {
    pub pts: Vec<(f64, f64)>,
    pub closed: bool,
}

/// One drawing element from a symbol, already flattened.
#[derive(Clone, Debug)]
pub struct Part {
    pub ink: Ink,
    /// The element's `class` attribute, verbatim and possibly empty.
    ///
    /// The icon set does not use classes — it declares `fill`/`stroke` per element, which is what
    /// [`Ink`] above is read from. The cursor set does the opposite: every shape is classed and the
    /// paint lives in one stylesheet, because each cursor is drawn twice (dark edge, light body)
    /// from the *same* path data and repeating the geometry per colour would be the one thing
    /// guaranteed to drift. So the class is carried through unparsed and `cursors.rs` resolves it.
    pub class: String,
    pub polys: Vec<Poly>,
}

/// One `<symbol>`: its id, its coordinate grid, and everything inside it in document order.
///
/// The grid is almost always 24×24 — the design draws every icon there, and the brand mark too, so
/// that the mark drops into a dock row or a caption line without refitting. It is carried per
/// symbol rather than assumed because the wordmark and lockup are not square (36.83×16 and
/// 63.83×19), and squashing those into a square box would be a silent distortion.
#[derive(Clone, Debug)]
pub struct Symbol {
    pub id: String,
    /// viewBox origin. Zero for every icon; the cursors use `-2 -2 26 26` so that a 3.4-unit dark
    /// edge stroke drawn on the 24 grid has somewhere to go instead of being clipped at the box.
    pub vb_x: f64,
    pub vb_y: f64,
    pub vb_w: f64,
    pub vb_h: f64,
    pub parts: Vec<Part>,
}

/// Curve flattening resolution. The whole coordinate space is 24 units wide and the largest render
/// is 22px, so one unit is at most one pixel — 24 segments per curve puts the flattening error far
/// below the rasterizer's supersampling grid and costs nothing at build time.
const CURVE_STEPS: usize = 24;
/// Segments in a full circle or ellipse.
const ARC_STEPS: usize = 96;

// ─────────────────────────────────────────────────────────────────────────────
// SCANNING
// ─────────────────────────────────────────────────────────────────────────────

/// Pull `<symbol>` definitions out of a design document, in document order.
///
/// `only` restricts the result to those ids; empty means everything. The filter is applied *before*
/// the symbol's children are parsed, and that ordering matters: `14-brand.html` also defines the
/// wordmark and lockup, which use elements (`<g>`) that no icon needs. Parsing them anyway would
/// force this tool to grow support for shapes nothing draws, just to reach the one symbol it wants.
pub fn parse_document(src: &str, only: &[&str]) -> Result<Vec<Symbol>, String> {
    let mut out = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = src[cursor..].find("<symbol") {
        let start = cursor + rel;
        let end = match src[start..].find("</symbol>") {
            Some(e) => start + e,
            None => return Err(format!("unterminated <symbol> at byte {}", start)),
        };
        let block = &src[start..end];
        let attrs = attrs_of(block)?;
        let id = attrs
            .get("id")
            .ok_or_else(|| format!("<symbol> with no id at byte {}", start))?
            .clone();

        if !only.is_empty() && !only.contains(&id.as_str()) {
            cursor = end + "</symbol>".len();
            continue;
        }

        let vb = attrs
            .get("viewBox")
            .ok_or_else(|| format!("symbol '{}' has no viewBox", id))?;
        let n: Vec<f64> = vb
            .split_whitespace()
            .map(|s| s.parse::<f64>().unwrap_or(f64::NAN))
            .collect();
        if n.len() != 4 || n.iter().any(|v| v.is_nan()) || n[2] <= 0.0 || n[3] <= 0.0 {
            return Err(format!(
                "symbol '{}' has viewBox '{}'; expected four numbers with a positive width and height",
                id, vb
            ));
        }
        let (vb_x, vb_y, vb_w, vb_h) = (n[0], n[1], n[2], n[3]);

        // Skip past the opening tag before looking for children, or the symbol's own attributes
        // would be read as a child element.
        let body_start = block
            .find('>')
            .ok_or_else(|| format!("symbol '{}' has no closing '>'", id))?;
        let parts = parse_children(&block[body_start + 1..], &id)?;
        if parts.is_empty() {
            return Err(format!("symbol '{}' contains no drawable shapes", id));
        }
        out.push(Symbol { id, vb_x, vb_y, vb_w, vb_h, parts });
        cursor = end + "</symbol>".len();
    }
    Ok(out)
}

/// What a `<g>` contributes to the elements inside it.
///
/// Only what the design actually uses: presentation attributes that inherit, and a translation. The
/// brand assets need both — the wordmark puts `fill="currentColor"` on a group rather than on each
/// of its three paths, and the lockup nests `translate()` inside that group to sit the mark and the
/// word beside each other. Without this the scanner rejected `<g>` outright, which is why the
/// wordmark was skipped and the boot screen had nothing to draw.
#[derive(Clone, Default)]
struct Group {
    fill: Option<String>,
    class: Option<String>,
    /// Accumulated translation, already composed with every enclosing group.
    tx: f64,
    ty: f64,
}

fn parse_children(body: &str, sym: &str) -> Result<Vec<Part>, String> {
    let mut parts = Vec::new();
    let mut cursor = 0usize;
    let mut stack: Vec<Group> = Vec::new();
    while let Some(rel) = body[cursor..].find('<') {
        let start = cursor + rel;
        let end = match body[start..].find('>') {
            Some(e) => start + e + 1,
            None => break,
        };
        let tag = &body[start..end];
        let closing = tag.as_bytes().get(1) == Some(&b'/');
        let name: String = tag[if closing { 2 } else { 1 }..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();

        if closing {
            if name == "g" {
                // A stray `</g>` would mean the document nests differently than it reads, and
                // silently popping an empty stack would apply the wrong transform to what follows.
                stack.pop().ok_or_else(|| format!("{}: </g> with no matching <g>", sym))?;
            }
            cursor = end;
            continue;
        }

        let attrs = attrs_of(tag)?;
        let cur = stack.last().cloned().unwrap_or_default();

        if name == "g" {
            let mut g = Group {
                fill: attrs.get("fill").cloned().or(cur.fill),
                class: attrs.get("class").cloned().or(cur.class),
                tx: cur.tx,
                ty: cur.ty,
            };
            if let Some(t) = attrs.get("transform") {
                let (dx, dy) = parse_translate(t).map_err(|e| format!("{}: {}", sym, e))?;
                g.tx += dx;
                g.ty += dy;
            }
            // A self-closing `<g/>` never gets a `</g>`, so it must not be pushed.
            if !tag.trim_end().ends_with("/>") {
                stack.push(g);
            }
            cursor = end;
            continue;
        }

        // `fill="currentColor" stroke="none"` marks a solid shape. Everything else is stroked.
        // Inherited from the enclosing group when the element does not say for itself, which is how
        // SVG presentation attributes work and how the wordmark declares its fill exactly once.
        let fill = attrs.get("fill").or(cur.fill.as_ref()).map(|s| s.as_str());
        let ink = if fill == Some("currentColor") { Ink::Fill } else { Ink::Stroke };

        let polys = match name.as_str() {
            "path" => {
                let d = attrs
                    .get("d")
                    .ok_or_else(|| format!("{}: <path> with no d", sym))?;
                parse_path(d).map_err(|e| format!("{}: {}", sym, e))?
            }
            "circle" => {
                let cx = num_attr(&attrs, "cx", sym)?;
                let cy = num_attr(&attrs, "cy", sym)?;
                let r = num_attr(&attrs, "r", sym)?;
                vec![ellipse_poly(cx, cy, r, r, 0.0, cx, cy)]
            }
            "ellipse" => {
                let cx = num_attr(&attrs, "cx", sym)?;
                let cy = num_attr(&attrs, "cy", sym)?;
                let rx = num_attr(&attrs, "rx", sym)?;
                let ry = num_attr(&attrs, "ry", sym)?;
                // `#a-qclang` is three ellipses at 0/60/120 degrees — the only transform in the set.
                let (deg, ox, oy) = match attrs.get("transform") {
                    Some(t) => parse_rotate(t).map_err(|e| format!("{}: {}", sym, e))?,
                    None => (0.0, cx, cy),
                };
                vec![ellipse_poly(cx, cy, rx, ry, deg, ox, oy)]
            }
            "rect" => {
                let x = num_attr(&attrs, "x", sym)?;
                let y = num_attr(&attrs, "y", sym)?;
                let w = num_attr(&attrs, "width", sym)?;
                let h = num_attr(&attrs, "height", sym)?;
                let r = attrs
                    .get("rx")
                    .map(|v| v.parse::<f64>().unwrap_or(0.0))
                    .unwrap_or(0.0);
                vec![rect_poly(x, y, w, h, r)]
            }
            // Comments and stray text between elements.
            "" | "!" => {
                cursor = end;
                continue;
            }
            other => {
                return Err(format!(
                    "{}: unhandled element <{}>. Add it here rather than letting the icon render \
                     blank — a missing shape is invisible, not obvious.",
                    sym, other
                ))
            }
        };
        // Group translation is applied here, once, in viewBox units — so everything downstream
        // (rasterizing, the viewBox origin, cropping) sees plain coordinates and never has to know
        // a transform was in play.
        let polys = if cur.tx != 0.0 || cur.ty != 0.0 {
            polys
                .into_iter()
                .map(|p| Poly {
                    pts: p.pts.iter().map(|&(x, y)| (x + cur.tx, y + cur.ty)).collect(),
                    closed: p.closed,
                })
                .collect()
        } else {
            polys
        };

        parts.push(Part {
            ink,
            class: attrs
                .get("class")
                .cloned()
                .or(cur.class)
                .unwrap_or_default(),
            polys,
        });
        cursor = end;
    }
    if !stack.is_empty() {
        return Err(format!("{}: {} unclosed <g>", sym, stack.len()));
    }
    Ok(parts)
}

/// `translate(x)` or `translate(x, y)` → the offset. Anything else is an error rather than a
/// silently-ignored transform, which would draw the shape in the wrong place at full confidence.
fn parse_translate(t: &str) -> Result<(f64, f64), String> {
    let inner = t
        .trim()
        .strip_prefix("translate(")
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| format!("unsupported group transform '{}'; only translate() is handled", t))?;
    let n: Vec<f64> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<f64>().unwrap_or(f64::NAN))
        .collect();
    match n.len() {
        1 if !n[0].is_nan() => Ok((n[0], 0.0)),
        2 if !n.iter().any(|v| v.is_nan()) => Ok((n[0], n[1])),
        _ => Err(format!("translate() needs one or two numbers, got '{}'", inner)),
    }
}

/// `key="value"` pairs out of one tag.
fn attrs_of(tag: &str) -> Result<HashMap<String, String>, String> {
    let mut map = HashMap::new();
    let bytes = tag.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 1 < bytes.len() && bytes[i + 1] == b'"' {
            // Walk back over the key.
            let mut k = i;
            while k > 0 && (bytes[k - 1].is_ascii_alphanumeric() || bytes[k - 1] == b'-') {
                k -= 1;
            }
            let key = &tag[k..i];
            let vstart = i + 2;
            let vend = match tag[vstart..].find('"') {
                Some(e) => vstart + e,
                None => return Err(format!("unterminated attribute value in: {}", tag)),
            };
            map.insert(key.to_string(), tag[vstart..vend].to_string());
            i = vend + 1;
        } else {
            i += 1;
        }
    }
    Ok(map)
}

fn num_attr(attrs: &HashMap<String, String>, key: &str, sym: &str) -> Result<f64, String> {
    attrs
        .get(key)
        .ok_or_else(|| format!("{}: missing attribute '{}'", sym, key))?
        .parse::<f64>()
        .map_err(|_| format!("{}: attribute '{}' is not a number", sym, key))
}

/// `rotate(60 12 12)` → (degrees, origin x, origin y).
fn parse_rotate(t: &str) -> Result<(f64, f64, f64), String> {
    let inner = t
        .trim()
        .strip_prefix("rotate(")
        .and_then(|s| s.strip_suffix(')'))
        .ok_or_else(|| format!("unsupported transform '{}'; only rotate() is handled", t))?;
    let n: Vec<f64> = inner
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<f64>().unwrap_or(0.0))
        .collect();
    match n.len() {
        1 => Ok((n[0], 0.0, 0.0)),
        3 => Ok((n[0], n[1], n[2])),
        _ => Err(format!("rotate() needs 1 or 3 numbers, got {}", n.len())),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PRIMITIVES
// ─────────────────────────────────────────────────────────────────────────────

fn ellipse_poly(cx: f64, cy: f64, rx: f64, ry: f64, deg: f64, ox: f64, oy: f64) -> Poly {
    let rot = deg.to_radians();
    let (sr, cr) = rot.sin_cos();
    let mut pts = Vec::with_capacity(ARC_STEPS);
    for i in 0..ARC_STEPS {
        let t = (i as f64) * 2.0 * PI / (ARC_STEPS as f64);
        let (x, y) = (cx + rx * t.cos(), cy + ry * t.sin());
        // Rotate about (ox, oy).
        let (dx, dy) = (x - ox, y - oy);
        pts.push((ox + dx * cr - dy * sr, oy + dx * sr + dy * cr));
    }
    Poly { pts, closed: true }
}

fn rect_poly(x: f64, y: f64, w: f64, h: f64, r: f64) -> Poly {
    let r = r.min(w / 2.0).min(h / 2.0);
    let mut pts = Vec::new();
    if r <= 0.0 {
        pts.push((x, y));
        pts.push((x + w, y));
        pts.push((x + w, y + h));
        pts.push((x, y + h));
        return Poly { pts, closed: true };
    }
    // Four quarter-arcs, corner by corner, clockwise from top-left.
    let corners = [
        (x + w - r, y + r, -PI / 2.0), // top-right
        (x + w - r, y + h - r, 0.0),   // bottom-right
        (x + r, y + h - r, PI / 2.0),  // bottom-left
        (x + r, y + r, PI),            // top-left
    ];
    let per = ARC_STEPS / 4;
    for &(ccx, ccy, start) in &corners {
        for i in 0..=per {
            let t = start + (i as f64) * (PI / 2.0) / (per as f64);
            pts.push((ccx + r * t.cos(), ccy + r * t.sin()));
        }
    }
    Poly { pts, closed: true }
}

// ─────────────────────────────────────────────────────────────────────────────
// PATH DATA
// ─────────────────────────────────────────────────────────────────────────────

/// Tokenizer for SVG path numbers.
///
/// Numbers in path data are not reliably separated. `a1.9 1.9 0 0 1 1.9-1.9` packs a sign as a
/// separator, and `.5.5` is two numbers. So a number ends at the next sign or at a second decimal
/// point, not merely at whitespace — which is the classic way a hand-rolled path parser silently
/// reads one shape as another.
struct Lexer<'a> {
    s: &'a [u8],
    i: usize,
}

impl<'a> Lexer<'a> {
    fn new(s: &'a str) -> Self {
        Lexer { s: s.as_bytes(), i: 0 }
    }

    fn skip_sep(&mut self) {
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c == b',' || c.is_ascii_whitespace() {
                self.i += 1;
            } else {
                break;
            }
        }
    }

    fn peek_cmd(&mut self) -> Option<u8> {
        self.skip_sep();
        let c = *self.s.get(self.i)?;
        if c.is_ascii_alphabetic() {
            Some(c)
        } else {
            None
        }
    }

    fn take_cmd(&mut self) -> Option<u8> {
        let c = self.peek_cmd()?;
        self.i += 1;
        Some(c)
    }

    fn number(&mut self) -> Result<f64, String> {
        self.skip_sep();
        let start = self.i;
        if self.i < self.s.len() && (self.s[self.i] == b'-' || self.s[self.i] == b'+') {
            self.i += 1;
        }
        let mut seen_dot = false;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_digit() {
                self.i += 1;
            } else if c == b'.' && !seen_dot {
                seen_dot = true;
                self.i += 1;
            } else {
                break;
            }
        }
        if self.i == start {
            return Err(format!(
                "expected a number at byte {} of path data",
                start
            ));
        }
        std::str::from_utf8(&self.s[start..self.i])
            .unwrap()
            .parse::<f64>()
            .map_err(|_| format!("bad number '{}'", &String::from_utf8_lossy(&self.s[start..self.i])))
    }

    /// Arc flags are single characters — `0` or `1` — and are NOT separated from what follows.
    /// `0 0 1 1.9-1.9` is flag, flag, then a coordinate pair. Reading a flag with `number()` would
    /// swallow the following digits.
    fn flag(&mut self) -> Result<bool, String> {
        self.skip_sep();
        match self.s.get(self.i) {
            Some(b'0') => {
                self.i += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.i += 1;
                Ok(true)
            }
            _ => Err(format!("expected an arc flag at byte {}", self.i)),
        }
    }

    fn more_numbers(&mut self) -> bool {
        self.skip_sep();
        match self.s.get(self.i) {
            Some(c) => c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.',
            None => false,
        }
    }
}

/// Flatten one `d` attribute into contours.
pub fn parse_path(d: &str) -> Result<Vec<Poly>, String> {
    let mut lex = Lexer::new(d);
    let mut polys: Vec<Poly> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let mut closed = false;
    let (mut x, mut y) = (0.0f64, 0.0f64);
    let (mut sx, mut sy) = (0.0f64, 0.0f64);
    let mut last_cmd = 0u8;
    // The previous curve's trailing control point, for the smooth variants. `S` and `T` reflect it
    // about the current point, which is the whole reason they exist — and if the previous command
    // was not a curve of the matching kind, the spec says the reflection is the current point
    // itself. Getting that fallback wrong produces a kink instead of a smooth join.
    let mut last_ctrl: Option<(f64, f64)> = None;
    let mut last_qctrl: Option<(f64, f64)> = None;

    macro_rules! flush {
        () => {
            if cur.len() > 1 {
                polys.push(Poly { pts: std::mem::take(&mut cur), closed });
            } else {
                cur.clear();
            }
            #[allow(unused_assignments)]
            {
                closed = false;
            }
        };
    }

    loop {
        let cmd = match lex.take_cmd() {
            Some(c) => c,
            None => {
                if lex.more_numbers() {
                    // An implicit repeat: after `M x y`, further pairs are `L`; after any other
                    // command, the command repeats.
                    match last_cmd {
                        b'M' => b'L',
                        b'm' => b'l',
                        0 => return Err("path data starts with a number".into()),
                        c => c,
                    }
                } else {
                    break;
                }
            }
        };
        last_cmd = cmd;

        // A smooth curve only reflects when the PREVIOUS command was a curve of the matching kind;
        // after anything else the spec says the reflected control point is the current point. This
        // runs before the arms below set their own, so each arm only has to record what it produced.
        if !matches!(cmd, b'C' | b'c' | b'S' | b's') {
            last_ctrl = None;
        }
        if !matches!(cmd, b'Q' | b'q' | b'T' | b't') {
            last_qctrl = None;
        }

        match cmd {
            b'M' | b'm' => {
                flush!();
                let (nx, ny) = (lex.number()?, lex.number()?);
                if cmd == b'm' {
                    x += nx;
                    y += ny;
                } else {
                    x = nx;
                    y = ny;
                }
                sx = x;
                sy = y;
                cur.push((x, y));
            }
            b'L' | b'l' => {
                let (nx, ny) = (lex.number()?, lex.number()?);
                if cmd == b'l' {
                    x += nx;
                    y += ny;
                } else {
                    x = nx;
                    y = ny;
                }
                cur.push((x, y));
            }
            b'H' | b'h' => {
                let nx = lex.number()?;
                x = if cmd == b'h' { x + nx } else { nx };
                cur.push((x, y));
            }
            b'V' | b'v' => {
                let ny = lex.number()?;
                y = if cmd == b'v' { y + ny } else { ny };
                cur.push((x, y));
            }
            b'C' | b'c' => {
                let (mut x1, mut y1) = (lex.number()?, lex.number()?);
                let (mut x2, mut y2) = (lex.number()?, lex.number()?);
                let (mut ex, mut ey) = (lex.number()?, lex.number()?);
                if cmd == b'c' {
                    x1 += x;
                    y1 += y;
                    x2 += x;
                    y2 += y;
                    ex += x;
                    ey += y;
                }
                flatten_cubic(&mut cur, (x, y), (x1, y1), (x2, y2), (ex, ey));
                last_ctrl = Some((x2, y2));
                x = ex;
                y = ey;
            }
            // Smooth cubic. `#a-browser`'s globe meridians are drawn with these.
            b'S' | b's' => {
                let (mut x2, mut y2) = (lex.number()?, lex.number()?);
                let (mut ex, mut ey) = (lex.number()?, lex.number()?);
                if cmd == b's' {
                    x2 += x;
                    y2 += y;
                    ex += x;
                    ey += y;
                }
                let (x1, y1) = match last_ctrl {
                    Some((cx, cy)) => (2.0 * x - cx, 2.0 * y - cy),
                    None => (x, y),
                };
                flatten_cubic(&mut cur, (x, y), (x1, y1), (x2, y2), (ex, ey));
                last_ctrl = Some((x2, y2));
                x = ex;
                y = ey;
            }
            b'Q' | b'q' => {
                let (mut x1, mut y1) = (lex.number()?, lex.number()?);
                let (mut ex, mut ey) = (lex.number()?, lex.number()?);
                if cmd == b'q' {
                    x1 += x;
                    y1 += y;
                    ex += x;
                    ey += y;
                }
                flatten_quad(&mut cur, (x, y), (x1, y1), (ex, ey));
                last_qctrl = Some((x1, y1));
                x = ex;
                y = ey;
            }
            b'T' | b't' => {
                let (mut ex, mut ey) = (lex.number()?, lex.number()?);
                if cmd == b't' {
                    ex += x;
                    ey += y;
                }
                let (x1, y1) = match last_qctrl {
                    Some((cx, cy)) => (2.0 * x - cx, 2.0 * y - cy),
                    None => (x, y),
                };
                flatten_quad(&mut cur, (x, y), (x1, y1), (ex, ey));
                last_qctrl = Some((x1, y1));
                x = ex;
                y = ey;
            }
            b'A' | b'a' => {
                let rx = lex.number()?;
                let ry = lex.number()?;
                let rot = lex.number()?;
                let large = lex.flag()?;
                let sweep = lex.flag()?;
                let (mut ex, mut ey) = (lex.number()?, lex.number()?);
                if cmd == b'a' {
                    ex += x;
                    ey += y;
                }
                flatten_arc(&mut cur, (x, y), rx, ry, rot, large, sweep, (ex, ey));
                x = ex;
                y = ey;
            }
            b'Z' | b'z' => {
                closed = true;
                x = sx;
                y = sy;
                flush!();
                cur.push((x, y));
            }
            other => {
                return Err(format!(
                    "unsupported path command '{}'. Implement it rather than ignoring it.",
                    other as char
                ))
            }
        }
    }
    flush!();
    Ok(polys)
}

fn flatten_cubic(
    out: &mut Vec<(f64, f64)>,
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) {
    for i in 1..=CURVE_STEPS {
        let t = i as f64 / CURVE_STEPS as f64;
        let u = 1.0 - t;
        let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((
            a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
            a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
        ));
    }
}

fn flatten_quad(out: &mut Vec<(f64, f64)>, p0: (f64, f64), p1: (f64, f64), p2: (f64, f64)) {
    for i in 1..=CURVE_STEPS {
        let t = i as f64 / CURVE_STEPS as f64;
        let u = 1.0 - t;
        let (a, b, c) = (u * u, 2.0 * u * t, t * t);
        out.push((
            a * p0.0 + b * p1.0 + c * p2.0,
            a * p0.1 + b * p1.1 + c * p2.1,
        ));
    }
}

/// SVG elliptical arc, endpoint parameterization → centre parameterization, then sampled.
///
/// This is the implementation of SVG 1.1 appendix F.6.5. It is fiddly and worth having in one
/// place: `#e-working`, `#e-attention`, `#a-files`, `#u-reload` and `#u-power` are all arcs, and an
/// arc that silently degrades to a straight line looks like a *different icon* rather than a broken
/// one — which is precisely the failure that would survive review.
#[allow(clippy::too_many_arguments)]
fn flatten_arc(
    out: &mut Vec<(f64, f64)>,
    p0: (f64, f64),
    mut rx: f64,
    mut ry: f64,
    rot_deg: f64,
    large: bool,
    sweep: bool,
    p1: (f64, f64),
) {
    // Degenerate radii mean a straight line, per spec — not a dropped segment.
    if rx == 0.0 || ry == 0.0 {
        out.push(p1);
        return;
    }
    rx = rx.abs();
    ry = ry.abs();
    let phi = rot_deg.to_radians();
    let (sp, cp) = phi.sin_cos();

    let dx2 = (p0.0 - p1.0) / 2.0;
    let dy2 = (p0.1 - p1.1) / 2.0;
    let x1p = cp * dx2 + sp * dy2;
    let y1p = -sp * dx2 + cp * dy2;

    // Scale the radii up if they are too small to span the endpoints (F.6.6).
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }

    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;

    let cx = cp * cxp - sp * cyp + (p0.0 + p1.0) / 2.0;
    let cy = sp * cxp + cp * cyp + (p0.1 + p1.1) / 2.0;

    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };

    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = ang(1.0, 0.0, ux, uy);
    let mut dtheta = ang(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * PI;
    }

    // Step count proportional to the swept angle, so a quarter turn is not sampled as finely as a
    // full one and a near-full circle is not sampled too coarsely.
    let steps = ((dtheta.abs() / (2.0 * PI)) * ARC_STEPS as f64).ceil().max(4.0) as usize;
    for i in 1..=steps {
        let t = theta1 + dtheta * (i as f64 / steps as f64);
        let (st, ct) = t.sin_cos();
        out.push((
            cp * rx * ct - sp * ry * st + cx,
            sp * rx * ct + cp * ry * st + cy,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexer_splits_numbers_glued_by_a_sign() {
        // Straight out of `#a-files`: the `-1.9` is a separate number, not part of `1.9`.
        let mut lex = Lexer::new("1.9 1.9 0 0 1 1.9-1.9");
        assert_eq!(lex.number().unwrap(), 1.9);
        assert_eq!(lex.number().unwrap(), 1.9);
        assert_eq!(lex.number().unwrap(), 0.0);
        assert!(!lex.flag().unwrap());
        assert!(lex.flag().unwrap());
        assert_eq!(lex.number().unwrap(), 1.9);
        assert_eq!(lex.number().unwrap(), -1.9);
    }

    #[test]
    fn implicit_lineto_after_moveto() {
        // `#a-terminal`: `m5.5 8.5 3.75 3.5L5.5 15.5` — the second pair is an implicit relative L.
        let polys = parse_path("m5.5 8.5 3.75 3.5L5.5 15.5").unwrap();
        assert_eq!(polys.len(), 1);
        assert_eq!(polys[0].pts.len(), 3);
        assert_eq!(polys[0].pts[0], (5.5, 8.5));
        assert_eq!(polys[0].pts[1], (9.25, 12.0));
        assert_eq!(polys[0].pts[2], (5.5, 15.5));
    }

    #[test]
    fn arc_reaches_its_endpoint() {
        // If the F.6.5 conversion is wrong the arc usually still draws — just not to where it was
        // asked to go. Checking the endpoint is what catches that.
        let polys = parse_path("M12 3a9 9 0 0 1 8.4 5.8").unwrap();
        let last = *polys[0].pts.last().unwrap();
        assert!((last.0 - 20.4).abs() < 1e-6, "x was {}", last.0);
        assert!((last.1 - 8.8).abs() < 1e-6, "y was {}", last.1);
        assert!(polys[0].pts.len() > 4, "arc was not subdivided");
    }

    #[test]
    fn arc_bulges_away_from_the_chord() {
        // An arc that degenerates to a line still passes an endpoint test, so check that the middle
        // actually leaves the chord.
        let polys = parse_path("M12 3a9 9 0 0 1 8.4 5.8").unwrap();
        let pts = &polys[0].pts;
        let mid = pts[pts.len() / 2];
        let (x0, y0) = (12.0, 3.0);
        let (x1, y1) = (20.4, 8.8);
        let num = ((y1 - y0) * mid.0 - (x1 - x0) * mid.1 + x1 * y0 - y1 * x0).abs();
        let den = ((y1 - y0).powi(2) + (x1 - x0).powi(2)).sqrt();
        assert!(num / den > 0.5, "arc is nearly straight: {}", num / den);
    }

    #[test]
    fn closepath_starts_a_new_contour() {
        let polys = parse_path("M4 3h3.6v14.4H4z").unwrap();
        assert_eq!(polys.len(), 1);
        assert!(polys[0].closed);
    }

    #[test]
    fn unknown_command_is_an_error_not_a_silent_skip() {
        // This is the property that caught `#a-browser` using `S` before it could ship as a blank
        // globe: an unimplemented command must stop the build, not draw nothing.
        assert!(parse_path("M0 0 B 1 1").is_err());
    }

    #[test]
    fn smooth_cubic_reflects_the_previous_control_point() {
        // From `#a-browser`. After `c` ends at (15.45,12) with its second control at (15.45,8.75)
        // relative-resolved, the following `S` must start with the reflection of that point —
        // (15.45, 15.25). Getting the fallback path instead puts a visible kink in the globe.
        let polys = parse_path("M12 3.25c2.3 2.45 3.45 5.4 3.45 8.75S14.3 18.3 12 20.75").unwrap();
        let pts = &polys[0].pts;
        assert_eq!(*pts.last().unwrap(), (12.0, 20.75));
        // The join must be smooth: the direction entering the join and leaving it should agree.
        let n = CURVE_STEPS;
        let join = pts[n];
        let before = pts[n - 1];
        let after = pts[n + 1];
        let d1 = (join.0 - before.0, join.1 - before.1);
        let d2 = (after.0 - join.0, after.1 - join.1);
        let cos = (d1.0 * d2.0 + d1.1 * d2.1)
            / ((d1.0.hypot(d1.1)) * (d2.0.hypot(d2.1)));
        assert!(cos > 0.99, "tangent should be continuous across the smooth join, cos={}", cos);
    }

    #[test]
    fn smooth_cubic_without_a_preceding_curve_uses_the_current_point() {
        // Spec fallback. If this silently reflected a stale control point from two commands ago the
        // curve would bulge the wrong way, which is not something a glance would catch.
        let polys = parse_path("M0 0 L10 0 S 20 10 20 0").unwrap();
        assert_eq!(*polys[0].pts.last().unwrap(), (20.0, 0.0));
    }
}
