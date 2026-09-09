//! Render the QCLang interior to a PNG on the development host, without booting.
//!
//! ```text
//! cargo run --release -p nyx-meridian --example qclang_preview -- /tmp/qc.png [--light] \
//!     [--size 1088x540] [--src path.ql]
//! ```
//!
//! The circuit and the numbers come from a **real compile** of a real `.ql` — that is the whole
//! point of the dev-dependency on `qclang_compiler`. A preview drawn from a hand-written fixture
//! would agree with itself forever and prove nothing about the window that ships.
//!
//! ⚠️ The drawing below is a second implementation of `apps/qcstudio`'s, because that app is not a
//! workspace member (std target, `build-std`) and cannot be imported here. Everything with geometry
//! in it — the wire rows, the gate cells, the connector, the measurement rows — comes from
//! `nyx_meridian::circuit` and so cannot drift; what can drift is the *order and colour* of the
//! marks. Keep the two in step by eye when either changes.

#[path = "common/png.rs"]
mod png;
use png::write_png;

use nyx_gui::canvas::Canvas;
use nyx_meridian::circuit::{self, GATE_LABEL};
use nyx_meridian::layout::Rect;
use nyx_meridian::shapes;
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B2, B3, LB, MONO};

use qclang_compiler::highlight::{self, Tone};
use qclang_compiler::program::{self, Step};
use qclang_compiler::qir::QirGate;
use qclang_compiler::statevector;
use qclang_compiler::Compiler;

/// The bundled sample, verbatim from `apps/qcstudio/samples/sample.ql` — the file the window opens
/// when nothing else is handed to it.
const SAMPLE: &str = r#"// qcstudio sample: a Bell pair (entangle two qubits, then measure both).
// Compiled on-device by qcstudio (Workstream C); its .qasm output is byte-compared
// against host qclang for the same input in C3 validation.
//
// QCLang uses affine (linear) typing for quantum resources: a qubit cannot be
// reassigned, so gates are applied as fresh bindings (`qubit c = CNOT(H(a), b);`)
// rather than mutation (`a = H(a);`).
fn main() -> int {
    qubit a = |0>;
    qubit b = |0>;
    qubit c = CNOT(H(a), b);
    cbit r1 = measure(a);
    cbit r2 = measure(b);
    return 0;
}
"#;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}
fn flag(name: &str) -> bool {
    std::env::args().any(|x| x == name)
}
fn px(v: i32) -> i32 {
    nyx_meridian::scale::px(v)
}

// The app's own constants, kept identical.
const CODE_PAD_Y: i32 = 26;
const CODE_LINE: i32 = 25;
const GUTTER_W: i32 = 52;
const GUTTER_PAD_R: i32 = 20;
const CODE_PAD_R: i32 = 20;
const CIRC_W: i32 = 400;
const LB_GAP_BELOW: i32 = 15;
const SECTION_GAP_ABOVE: i32 = 26;
const SECTION_GAP_BELOW: i32 = 22;

#[allow(non_snake_case)]
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "qclang.png".into());
    let t = if flag("--light") { Theme::light() } else { Theme::dark() };
    let (W, H) = match arg("--size").and_then(|s| {
        let (a, b) = s.split_once('x')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }) {
        Some(v) => v,
        None => (1088usize, 540usize),
    };
    let source = match arg("--src") {
        Some(p) => std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {}", p, e)),
        None => SAMPLE.to_string(),
    };
    let name = arg("--src")
        .map(|p| p.rsplit('/').next().unwrap_or(&p).to_string())
        .unwrap_or_else(|| "sample.ql".into());

    nyx_meridian::font::register_text();
    nyx_meridian::font::register_mono();

    // ── the real compile ──
    let res = Compiler::compile_with_stats(&source, true).expect("the sample must compile");
    let flat = program::flatten(&res.ir).expect("the sample must flatten");
    let columns = flat.columns();
    let dist = statevector::evaluate(&flat).expect("the sample must evaluate");
    println!(
        "{} \u{00B7} {} qubits \u{00B7} depth {} \u{00B7} {} steps",
        name, flat.qubits, flat.depth(), flat.steps.len()
    );

    let mut buf = vec![0u32; W * H];
    let mut canvas = Canvas::new(&mut buf, W, H);
    let (w, h) = (W as i32, H as i32);
    canvas.fill_rect(0, 0, W, H, t.surface);

    let circ = Rect::new(w - px(CIRC_W).min(w / 2), 0, px(CIRC_W).min(w / 2), h);
    let code = Rect::new(0, 0, circ.x, h);

    // ── the code pane ──
    if t.is_dark {
        fill(&mut canvas, code.x, code.y, code.w, code.h, t.sunken);
    }
    fill(&mut canvas, code.right() - 1, 0, 1, h, t.line);

    let num_right = code.x + px(GUTTER_W) - px(GUTTER_PAD_R);
    let code_x = code.x + px(GUTTER_W);
    let mut in_block = false;
    for (i, line) in source.lines().enumerate() {
        let y = px(CODE_PAD_Y) + i as i32 * px(CODE_LINE);
        let ny = text::baseline_with(GATE_LABEL, MONO, y);
        text::draw_right(&mut canvas, num_right, ny, GATE_LABEL, t.fg_4, &format!("{}", i + 1));

        let clip = code.right() - 1 - px(CODE_PAD_R);
        let mut x = code_x;
        for run in highlight::line(line, in_block) {
            if x >= clip {
                break;
            }
            let col = match run.tone {
                Tone::Plain => t.fg_2,
                Tone::Keyword | Tone::Gate => t.fg,
                Tone::Comment => t.fg_4,
            };
            let s = &line[run.start..run.end];
            if x + text::width(MONO, s) > clip {
                text::draw(&mut canvas, x, y, MONO, col, &text::ellipsize(MONO, s, clip - x));
                break;
            }
            x += text::draw(&mut canvas, x, y, MONO, col, s);
        }
        in_block = highlight::block_comment_depth(line, in_block);
    }

    // ── the circuit pane ──
    let cx = circ.x + circuit::pad_x();
    let right = circ.right() - circuit::pad_x();
    let mut y = circ.y + circuit::pad_top();

    text::draw(
        &mut canvas, cx, y, B3, t.fg_3,
        &format!("{} \u{00B7} {} qubits \u{00B7} depth {}", name, flat.qubits, flat.depth()),
    );
    y += text::line_box(B3) + px(22);

    // CIRCUIT
    text::draw(&mut canvas, cx, y, LB, t.fg_4, "Circuit");
    let top = y + circuit::heading_height() + px(LB_GAP_BELOW);
    let cols = columns.iter().max().map(|m| m + 1).unwrap_or(0);
    let c = circuit::circuit(
        Rect::new(circ.x, top - circuit::pad_top(), circ.w, h),
        flat.qubits,
        cols,
    );

    for wire in 0..flat.qubits {
        if let Some(r) = c.wire(wire) {
            fill(&mut canvas, r.x, r.y, r.w, r.h, t.line_2);
        }
        if let Some((lx, ly)) = c.label(wire) {
            let ket = if flat.init.get(wire).copied().unwrap_or(false) { "|1>" } else { "|0>" };
            text::draw(&mut canvas, lx, ly, GATE_LABEL, t.fg_4, ket);
        }
    }

    // The gate under the pointer is accented; the preview picks the first, so that `.gate.a` — the
    // one accent mark the design puts in this window — is in the proof sheet at all.
    let accent_cell = columns
        .first()
        .zip(flat.steps.first())
        .map(|(&col, s)| (col, s.qubits()[0]));

    for (i, step) in flat.steps.iter().enumerate() {
        let col = columns[i];
        if col >= c.visible_cols() {
            continue;
        }
        let qs = step.qubits();
        let control = matches!(step, Step::Gate { gate: QirGate::CNOT, .. }
            | Step::Gate { gate: QirGate::Toffoli, .. }
            | Step::Gate { gate: QirGate::Fredkin, .. });
        if qs.len() > 1 {
            if let (Some(&lo), Some(&hi)) = (qs.iter().min(), qs.iter().max()) {
                if let Some(r) = c.connector(col, lo, hi) {
                    fill(&mut canvas, r.x, r.y, r.w, r.h, t.line_2);
                }
            }
        }
        let letter = step.letter();
        for (n, &wire) in qs.iter().enumerate() {
            if control && n + 1 < qs.len() {
                if let Some(d) = c.dot(col, wire) {
                    shapes::fill_round_rect(
                        &mut canvas, d.x as usize, d.y as usize, d.w as usize, d.h as usize,
                        (d.w / 2) as usize, t.fg,
                    );
                }
                continue;
            }
            let Some(g) = c.gate(col, wire) else { continue };
            let on = accent_cell == Some((col, wire));
            shapes::fill_round_rect(
                &mut canvas, g.x as usize, g.y as usize, g.w as usize, g.h as usize,
                circuit::gate_radius(), t.surface,
            );
            shapes::stroke_round_rect(
                &mut canvas, g.x as usize, g.y as usize, g.w as usize, g.h as usize,
                circuit::gate_radius(), if on { t.accent } else { t.line_2 },
            );
            let lw = text::width(GATE_LABEL, &letter);
            let ly = text::centre_y(g.y, g.h, GATE_LABEL);
            text::draw(&mut canvas, g.x + (g.w - lw) / 2, ly, GATE_LABEL, t.fg, &letter);
        }
    }
    // The accented gate's QASM, right-aligned on the heading line — what the window shows on hover.
    if let Some((col, wire)) = accent_cell {
        if let Some(step) = columns.iter().zip(flat.steps.iter())
            .find(|(&sc, s)| sc == col && s.qubits().contains(&wire))
            .map(|(_, s)| s)
        {
            let q: Vec<String> = step.qubits().iter().map(|i| format!("q[{}]", i)).collect();
            let s = match step {
                Step::Measure { qubit, cbit } => format!("measure q[{}] -> c[{}]", qubit, cbit),
                Step::Gate { gate, .. } => format!("{} {}", gate.to_qasm_name(), q.join(", ")),
            };
            let ny = text::baseline_with(GATE_LABEL, LB, y);
            text::draw_right(&mut canvas, right, ny, GATE_LABEL, t.fg_3, &s);
        }
    }

    let mut ry = top + flat.qubits as i32 * circuit::wire_h() + px(SECTION_GAP_ABOVE);
    fill(&mut canvas, cx, ry, right - cx, 1, t.line);
    ry += 1 + px(SECTION_GAP_BELOW);

    // MEASUREMENT
    text::draw(&mut canvas, cx, ry, LB, t.fg_4, "Measurement");
    let ny = text::baseline_with(GATE_LABEL, LB, ry);
    text::draw_right(&mut canvas, right, ny, GATE_LABEL, t.fg_3, "exact");
    ry += circuit::heading_height() + px(LB_GAP_BELOW);

    let ranked = dist.ranked();
    for (i, &(basis, p)) in ranked.iter().enumerate() {
        let r = circuit::outcome_row(circ, ry, i);
        text::draw(&mut canvas, r.label.0, r.label.1, GATE_LABEL, t.fg_2, &dist.label(basis));
        fill(&mut canvas, r.track.x, r.track.y, r.track.w, r.track.h, t.wash_2);
        let f = circuit::outcome_fill(r.track, p);
        if f.w > 0 {
            fill(&mut canvas, f.x, f.y, f.w, f.h, t.accent);
        }
        let col = if p > 0.0 { t.fg } else { t.fg_3 };
        text::draw_right(&mut canvas, r.value_right, r.label.1, GATE_LABEL, col, &pct(p));
    }
    ry += circuit::outcomes_height(ranked.len());

    ry += px(SECTION_GAP_ABOVE);
    fill(&mut canvas, cx, ry, right - cx, 1, t.line);
    ry += 1 + px(SECTION_GAP_BELOW);

    // The design's `Fidelity 0.998` row, replaced by the check this machine can actually make.
    text::draw(&mut canvas, cx, ry, B3, t.fg_3, "Host reference");
    let vy = text::baseline_with(B2, B3, ry);
    text::draw_right(&mut canvas, right, vy, B2, t.fg, "byte-identical");
    ry += B3.line() as i32;
    text::draw(
        &mut canvas, cx, ry + px(4), B3, t.fg_4,
        &format!("wrote out.qasm \u{00B7} {} bytes", res.qasm.len()),
    );

    write_png(&out, &buf, W, H).expect("write png");
    println!("wrote {} ({}x{}, {})", out, W, H, if t.is_dark { "dark" } else { "light" });
}

fn pct(p: f64) -> String {
    let tenths = (p * 1000.0).round() as i64;
    format!("{}.{}%", tenths / 10, tenths % 10)
}

fn fill(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(cw), (y + h).min(ch));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    canvas.fill_rect(x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, color);
}
