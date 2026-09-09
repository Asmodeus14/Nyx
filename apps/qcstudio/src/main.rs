//! # QCLang — Meridian build-order step 14
//!
//! The design's lede is the specification: *"Code on the left, circuit on the right, measurement
//! underneath. The circuit is drawn the way a physicist draws it — wires as hairlines, gates as
//! outlined squares, nothing shaded or glowing. Scientific rather than science-fiction."*
//!
//! ## What this replaces
//!
//! Workstream C's `qcstudio`, which was the same program with no window: it read a `.ql`, ran it
//! through the portable compiler core, wrote the `.qasm` back to the VFS, byte-compared the result
//! against host `qclang`, and `println!`d all of it into the boot log. Every one of those steps still
//! happens — this is the same binary and the same bundle. What changed is that the answers are now
//! on screen instead of in a log that **this machine has no serial console to read**.
//!
//! That last point is why the C3 byte-match did not get deleted along with the log output. It is a
//! real verification — "the compiler on the device agrees with the compiler on the host, byte for
//! byte" — and it was previously visible only to someone who thought to open the kernel log in
//! System Monitor. It is now a row in the window.
//!
//! ## The design's scene is not valid QCLang, so this does not draw it
//!
//! `06-monitor-qclang.html` sets its code pane to
//!
//! ```text
//! fn main() { qubit q0, q1;  h(q0);  cx(q0, q1);  measure(q0); }
//! ```
//!
//! which this compiler rejects. QCLang uses **affine typing** for quantum resources: a qubit cannot
//! be reassigned or aliased, so gates are applied as fresh bindings —
//! `qubit c = CNOT(H(a), b);` — rather than as statements mutating a register. Drawing the mockup's
//! source verbatim would put a program on screen that the compiler three inches to its right cannot
//! compile. So the window shows the real file, and the language it shows is the one that exists.
//!
//! ## Where the numbers come from, and the one the design shows that is not drawn
//!
//! Everything on the right is computed from the compiled QIR:
//!
//! - **the circuit** — `qclang_compiler::program::flatten`, the same walk the evaluator uses, so the
//!   picture and the numbers cannot describe different programs;
//! - **the measurement table** — `qclang_compiler::statevector::evaluate`, which computes the
//!   **exact** distribution rather than sampling it.
//!
//! **Fidelity, "0.998", is not drawn.** There is no noise model anywhere in this compiler — no
//! error rates, no decoherence, no channel of any kind. An ideal statevector has fidelity 1 against
//! itself by construction, so any other number would have to be invented, and this window sits two
//! inches from a table of probabilities it would then cast doubt on. The design's slot is taken by
//! the host-reference check instead: a fact about this machine that is true, checkable, and was
//! previously invisible.
//!
//! The design's *"Measurement · 1024 shots"* becomes *"Measurement · exact"* for the same reason.
//! Printing `512` next to a bar would claim a thousand samples were drawn. None were: the number is
//! the amplitude, and it is better than a sample.
//!
//! ## Where the geometry lives
//!
//! `nyx_meridian::circuit`. Same argument as every other port — the wire rows, the gate cells, the
//! connector and the measurement rows are proved by `cargo test` on the host, because this machine
//! has no QEMU and a hit-test that disagrees with a draw by four pixels would otherwise be found by
//! booting and squinting.
//!
//! `restricted_std` is rustc's required opt-in for a std built for an out-of-tree target it doesn't
//! recognise — the nyx PAL is injected into rust-src by vendor/nyx-std/apply.py.
#![feature(restricted_std)]

// ELF entry — identical contract to apps/terminal, apps/stdgui and tests/stdhello: the kernel hands
// us a SysV stack [argc][argv..][NULL][envp..][NULL][auxv..] with RSP % 16 == 8. Install the main
// thread's native (fs:-relative) TLS block BEFORE anything touches a #[thread_local] (rt::init reads
// thread::current_id() before sys::init), then forward argc/argv to the rustc-generated C `main`
// and exit_group(231) with its return value.
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, rsp",         // stash entry RSP (callee-saved across the call)
    "mov rdi, rsp",         // arg0 = &argc (the SysV info block) for TLS auxv walk
    "and rsp, -16",
    "call __nyx_setup_main_tls",
    "mov rdi, [rbx]",       // argc
    "lea rsi, [rbx + 8]",   // argv
    "and rsp, -16",
    "call main",            // rustc-generated `int main(int, char**)` → std lang_start
    "mov edi, eax",         // propagate exit code
    "mov eax, 231",         // SYS_exit_group
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
use nyx_meridian::circuit::{self, Circuit, GATE_LABEL};
use nyx_meridian::layout::Rect;
use nyx_meridian::shapes;
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B2, B3, LB};

use qclang_compiler::highlight::{self, Tone};
use qclang_compiler::program::{self, Flattened, Step};
use qclang_compiler::qir::QirGate;
use qclang_compiler::statevector::{self, Distribution};
use qclang_compiler::Compiler;

/// The bundle this app ships in. `Build.sh` puts the sample, the host reference and the icon here.
const BUNDLE: &str = "/mnt/nvme/apps/QcStudio.nyx/";

fn bundled(name: &str) -> String {
    format!("{}{}", BUNDLE, name)
}

/// The shell's theme, kept in step by `MSG_THEME_CHANGED`. Same static-plus-atomic shape as Files
/// and System Monitor, and for the same reason: every draw helper here reads it.
static DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

fn theme() -> Theme {
    if DARK.load(std::sync::atomic::Ordering::Relaxed) { Theme::dark() } else { Theme::light() }
}

fn sc(v: i32) -> i32 {
    nyx_meridian::scale::px(v)
}

// ── the design's own metrics, as design units ───────────────────────────────────────────────────
//
//   .qcode { flex: 1; padding: 26px 0; border-right: 1px solid var(--line) }
//   .dark .qcode { background: #0B0C0E }              <- exactly `Theme::dark().sunken`
//   .ql    { font: 13px mono; line-height: 25px }
//   .ql .n { width: 52px; text-align: right; padding-right: 20px; font-size: 12px; color: fg-4 }
//   .qcirc { width: 400px; flex: none }

/// `.qcode { padding: 26px 0 }`
const CODE_PAD_Y: i32 = 26;
/// `.ql { line-height: 25px }` — looser than [`nyx_meridian::tokens::MONO`]'s own 22px leading,
/// because the design sets the code pane wider than the terminal.
const CODE_LINE: i32 = 25;
/// `.ql .n { width: 52px }` — the gutter, including its 20px right padding.
const GUTTER_W: i32 = 52;
const GUTTER_PAD_R: i32 = 20;
/// The gutter's padding again, mirrored on the right.
///
/// The design's `.qcode` has **no** right padding — its lines simply fit. A real file's do not, and
/// a comment running under the 1px divider and out the other side into the circuit reads as memory
/// corruption rather than as a long line. So the pane ellipsizes at this inset.
const CODE_PAD_R: i32 = 20;
/// `.qcirc { width: 400px }`
const CIRC_W: i32 = 400;
/// The rule between the circuit and the measurement table: `margin: 26px 0 22px`.
const SECTION_GAP_ABOVE: i32 = 26;
const SECTION_GAP_BELOW: i32 = 22;
/// `.lb { margin-bottom: 14px }` under CIRCUIT, `16px` under MEASUREMENT. One value; the two-pixel
/// difference in the mockup is not a rule either heading states.
const LB_GAP_BELOW: i32 = 15;

/// What the compile produced, or why it did not.
enum Outcome {
    /// The program compiled. Everything the right pane draws hangs off this.
    Ok(Box<Compiled>),
    /// It did not. The diagnostics are the compiler's own, verbatim.
    Failed(Vec<String>),
}

struct Compiled {
    /// Size of the emitted QASM. The old headless app logged "compiled OK, N bytes of QASM"; the
    /// number is still the cheapest evidence that the artifact beside the window is this compile's.
    qasm_len: usize,
    flat: Flattened,
    columns: Vec<usize>,
    /// The exact distribution, or the sentence explaining why this evaluator will not produce one.
    dist: Result<Distribution, String>,
}

/// How the on-device QASM compared with the reference host `qclang` emitted for the same input.
///
/// This is Workstream C's C3 milestone. It used to be a line in the boot log; it is a row in the
/// window now, because a verification nobody can see is a verification that quietly stops holding.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reference {
    /// Byte-identical to the host's output.
    Match,
    /// The two differ — the on-device compiler has drifted from the host one.
    Differs,
    /// No reference is bundled next to this source. Not a failure; most files will not have one.
    Absent,
}

struct QcLang {
    width: i32,
    height: i32,

    /// The file being shown, and its lines. Held split because the pane draws, measures and
    /// highlights by line, and re-splitting a source on every frame is work for nothing.
    name: String,
    lines: Vec<String>,
    /// Whether each line *starts* inside a `/* */`. Computed once at load: it is the only
    /// cross-line state the highlighter needs, and recomputing it per frame would make the cost of
    /// drawing line 400 depend on all 399 above it.
    in_block: Vec<bool>,

    outcome: Outcome,
    reference: Reference,
    /// Where the emitted QASM was written, if it was. Reported so the window says what it did to
    /// the disk rather than leaving it to be discovered.
    wrote: Option<String>,

    /// The gate cell under the pointer. The design accents exactly one gate (`.gate.a`); this is
    /// what decides which, and it is also what puts that gate's QASM on the heading line.
    hover: Option<(usize, usize)>,
    scroll: i32,

    /// The caption context, drawn by the shell beside this window's name. Held as a field because
    /// `NyxApp::context` returns a borrow.
    ctx: String,
}

impl QcLang {
    fn new() -> Self {
        // Files can hand this app a path (the same `SYS_LAUNCH_ARG` route that opens a .png in the
        // Image Viewer). With nothing to open, fall back to the bundled Bell pair, which is also
        // the file the host reference was generated from.
        let mut buf = [0u8; 256];
        let n = sys_launch_arg(&mut buf);
        let arg = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();
        let path = if arg.is_empty() { bundled("sample.ql") } else { arg.to_string() };

        let source = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            // A source that cannot be read is shown as a comment in the pane rather than as an
            // empty window: the file name and the errno are the whole diagnosis.
            format!("// cannot read {}\n// {}\n", path, e)
        });

        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let lines: Vec<String> = source.lines().map(String::from).collect();
        let mut in_block = Vec::with_capacity(lines.len());
        let mut block = false;
        for l in &lines {
            in_block.push(block);
            block = highlight::block_comment_depth(l, block);
        }

        // ★ ONE compile. `compile_with_stats` returns the QASM *and* the QIR, so the artifact
        // written to disk, the byte-compare against the host, the circuit and the probabilities all
        // come from the same run of the compiler. Calling `compile` for the text and
        // `compile_with_stats` for the IR would be two runs, and two runs is how a window ends up
        // drawing one program while having written another to the disk beside it.
        let (outcome, reference, wrote) = match Compiler::compile_with_stats(&source, true) {
            Ok(res) => {
                // Keep doing what the headless app did: write the artifact back to the VFS. This is
                // the std::fs write path the original round-trip was built to exercise, and an app
                // that only ever displayed the QASM would quietly stop exercising it.
                let out = bundled("out.qasm");
                let wrote = std::fs::write(&out, res.qasm.as_bytes()).ok().map(|_| out);

                // C3: byte-compare against the QASM host qclang emits for the same input.
                let reference = match std::fs::read_to_string(bundled("expected.qasm")) {
                    Ok(r) if r == res.qasm => Reference::Match,
                    Ok(_) => Reference::Differs,
                    Err(_) => Reference::Absent,
                };

                let compiled = match program::flatten(&res.ir) {
                    Ok(flat) => {
                        let columns = flat.columns();
                        let dist = statevector::evaluate(&flat);
                        Outcome::Ok(Box::new(Compiled { qasm_len: res.qasm.len(), flat, columns, dist }))
                    }
                    Err(e) => Outcome::Failed(vec![e]),
                };
                (compiled, reference, wrote)
            }
            Err(errors) => (Outcome::Failed(errors), Reference::Absent, None),
        };

        let mut app = Self {
            width: 1088,
            height: 540,
            name,
            lines,
            in_block,
            outcome,
            reference,
            wrote,
            hover: None,
            scroll: 0,
            ctx: String::new(),
        };
        app.ctx = app.build_context();
        app
    }

    // ── layout ───────────────────────────────────────────────────────────────────────────────────

    /// The right-hand pane. Fixed at the design's 400px, but never more than half a narrow window —
    /// `flex: none` against a 1088px scene is a fixed width; against a window dragged to 500px it
    /// would leave the code pane with nothing.
    fn circ_pane(&self) -> Rect {
        let w = sc(CIRC_W).min(self.width / 2).max(0);
        Rect::new(self.width - w, 0, w, self.height)
    }

    fn code_pane(&self) -> Rect {
        Rect::new(0, 0, self.circ_pane().x, self.height)
    }

    /// Total height of the source listing, which is what the window's scrollbar drives.
    ///
    /// ⚠️ Only the **code** scrolls. The shell's scroll viewport is the whole window, so reporting
    /// the taller of the two panes would slide the circuit off the top as well — and the circuit is
    /// a fixed-size diagram with nothing below it to reach. The bar therefore reads as "the source
    /// scrolls", which is the only thing here that has more content than room.
    fn code_h(&self) -> i32 {
        2 * sc(CODE_PAD_Y) + self.lines.len() as i32 * sc(CODE_LINE)
    }

    fn max_scroll(&self) -> i32 {
        (self.code_h() - self.height).max(0)
    }

    /// The y the diagram's first wire row block starts at: below the CIRCUIT heading.
    fn circuit_top(&self) -> i32 {
        self.circ_pane().y + circuit::pad_top() + circuit::heading_height() + sc(LB_GAP_BELOW)
    }

    /// The circuit **as it is actually drawn**, so that drawing and hit-testing cannot disagree.
    ///
    /// `circuit::circuit` measures its wire rows down from `pane.y + pad_top()`, so placing the
    /// diagram under a heading means handing it a pane whose top is that much higher. Deriving that
    /// in one place is the whole point: two copies of the expression is exactly how the accent ends
    /// up on a different gate from the pointer, in an app whose tests cannot run on the host.
    fn laid_out(&self) -> Option<(Circuit, &Compiled)> {
        let Outcome::Ok(c) = &self.outcome else { return None };
        let pane = self.circ_pane();
        let p = Rect::new(pane.x, self.circuit_top() - circuit::pad_top(), pane.w, self.height);
        let cols = c.columns.iter().max().map(|m| m + 1).unwrap_or(0);
        Some((circuit::circuit(p, c.flat.qubits, cols), c.as_ref()))
    }

    /// The step drawn in `(col, wire)`, if any. Linear over a list that is a few dozen entries at
    /// most, and it runs once per hover rather than per frame.
    fn step_at(&self, col: usize, wire: usize) -> Option<&Step> {
        let Outcome::Ok(c) = &self.outcome else { return None };
        c.columns
            .iter()
            .zip(c.flat.steps.iter())
            .find(|(&sc_, s)| sc_ == col && s.qubits().contains(&wire))
            .map(|(_, s)| s)
    }

    /// The QASM line for one step — what the compiler will actually emit for it.
    ///
    /// Shown on hover so the diagram and the artifact can be checked against each other by eye. It
    /// is built from `to_qasm_name`, the same function the code generator uses, so it cannot say
    /// something the emitted file does not.
    fn step_qasm(&self, step: &Step) -> String {
        match step {
            Step::Measure { qubit, cbit } => format!("measure q[{}] -> c[{}]", qubit, cbit),
            Step::Gate { gate, qubits } => {
                let args: Vec<String> = qubits.iter().map(|q| format!("q[{}]", q)).collect();
                format!("{} {}", gate.to_qasm_name(), args.join(", "))
            }
        }
    }

    // ── drawing ──────────────────────────────────────────────────────────────────────────────────

    fn draw_code(&self, canvas: &mut Canvas, t: &Theme) {
        let pane = self.code_pane();
        if pane.w <= 0 {
            return;
        }
        // `.dark .qcode { background: #0B0C0E }` — which is `sunken` exactly. In the light theme the
        // design leaves the pane on the surface, so there is no plate to draw.
        if t.is_dark {
            fill(canvas, pane.x, pane.y, pane.w, pane.h, t.sunken);
        }
        // `.qcode { border-right: 1px solid var(--line) }`
        fill(canvas, pane.right() - 1, 0, 1, self.height, t.line);

        let num_right = pane.x + sc(GUTTER_W) - sc(GUTTER_PAD_R);
        let code_x = pane.x + sc(GUTTER_W);
        let clip = pane.right() - 1 - sc(CODE_PAD_R);

        // Only the lines that can contribute ink. A source file is short, but each line costs a
        // highlight pass and a glyph walk per run, and this repaints on every scroll tick.
        let first = ((self.scroll - sc(CODE_PAD_Y)) / sc(CODE_LINE)).max(0) as usize;
        let count = (self.height / sc(CODE_LINE) + 2) as usize;
        for i in first..(first + count).min(self.lines.len()) {
            let y = sc(CODE_PAD_Y) + i as i32 * sc(CODE_LINE) - self.scroll;
            let line = &self.lines[i];

            // `.ql .n` — 12px mono, right-aligned in the gutter, on the code's own baseline so the
            // number does not float above the text it numbers.
            let ny = text::baseline_with(GATE_LABEL, MONO_CODE, y);
            text::draw_right(canvas, num_right, ny, GATE_LABEL, t.fg_4, &format!("{}", i + 1));

            let mut x = code_x;
            for run in highlight::line(line, self.in_block[i]) {
                if x >= clip {
                    break;
                }
                let col = match run.tone {
                    Tone::Plain => t.fg_2,
                    Tone::Keyword | Tone::Gate => t.fg,
                    Tone::Comment => t.fg_4,
                };
                // Ellipsize the run that crosses the edge rather than letting it overhang. Breaking
                // between runs is not enough on its own: a long comment is ONE run, so the check
                // above would pass and the whole line would be drawn straight through the divider.
                let s = &line[run.start..run.end];
                if x + text::width(MONO_CODE, s) > clip {
                    text::draw(canvas, x, y, MONO_CODE, col, &text::ellipsize(MONO_CODE, s, clip - x));
                    break;
                }
                x += text::draw(canvas, x, y, MONO_CODE, col, s);
            }
        }
    }

    fn draw_circuit_pane(&self, canvas: &mut Canvas, t: &Theme) {
        let pane = self.circ_pane();
        if pane.w <= 0 {
            return;
        }
        let x = pane.x + circuit::pad_x();
        let right = pane.right() - circuit::pad_x();
        let mut y = pane.y + circuit::pad_top();

        // `sample.ql · 2 qubits · depth 3` is NOT drawn here. It is the window's caption context and
        // the design puts it on the caption line beside the name — which step 15 made possible by
        // adding `WindowHeader::context`. See `NyxApp::context` below.
        match &self.outcome {
            Outcome::Ok(_) => {
                y = self.draw_circuit(canvas, t, x, right, y);
                y = self.rule(canvas, t, x, right, y);
                y = self.draw_measurement(canvas, t, x, right, y);
                self.rule(canvas, t, x, right, y);
            }
            Outcome::Failed(errors) => {
                self.heading(canvas, t, x, right, y, "Diagnostics", "");
                y += circuit::heading_height() + sc(LB_GAP_BELOW);
                for e in errors {
                    for chunk in wrap(e, B2, right - x) {
                        text::draw(canvas, x, y, B2, t.fg_2, &chunk);
                        y += B2.line() as i32;
                    }
                    y += sc(8);
                }
            }
        }
    }

    /// `sample.ql · 2 qubits · depth 3` — the design's caption context, computed rather than
    /// written. Nothing here changes after startup, so it is built once in `new`.
    fn build_context(&self) -> String {
        match &self.outcome {
            Outcome::Ok(c) => format!(
                "{} \u{00B7} {} qubit{} \u{00B7} depth {}",
                self.name,
                c.flat.qubits,
                if c.flat.qubits == 1 { "" } else { "s" },
                c.flat.depth()
            ),
            Outcome::Failed(e) => {
                format!("{} \u{00B7} {} error{}", self.name, e.len(), if e.len() == 1 { "" } else { "s" })
            }
        }
    }

    /// A `.lb` heading with an optional right-aligned note beside it.
    fn heading(&self, canvas: &mut Canvas, t: &Theme, x: i32, right: i32, y: i32, s: &str, note: &str) {
        text::draw(canvas, x, y, LB, t.fg_4, s);
        if !note.is_empty() {
            // On the heading's own baseline: `lb` is 11px Medium and the note is 12px mono, and two
            // runs drawn at the same `y` do not share a baseline (see `text::baseline_with`).
            let ny = text::baseline_with(GATE_LABEL, LB, y);
            text::draw_right(canvas, right, ny, GATE_LABEL, t.fg_3, note);
        }
    }

    fn rule(&self, canvas: &mut Canvas, t: &Theme, x: i32, right: i32, y: i32) -> i32 {
        let y = y + sc(SECTION_GAP_ABOVE);
        fill(canvas, x, y, right - x, 1, t.line);
        y + 1 + sc(SECTION_GAP_BELOW)
    }

    fn draw_circuit(&self, canvas: &mut Canvas, t: &Theme, x: i32, right: i32, y: i32) -> i32 {
        let Some((c, compiled)) = self.laid_out() else { return y };

        // The hovered gate's QASM, beside the heading. One accent gate, one line of context — the
        // design's `.gate.a` given something to mean.
        let note = self
            .hover
            .and_then(|(col, wire)| self.step_at(col, wire))
            .map(|s| self.step_qasm(s))
            .unwrap_or_default();
        self.heading(canvas, t, x, right, y, "Circuit", &note);

        // Wires first: the design's flex row resolves to a hairline the gates sit ON, so this is
        // painter's order and not clipping — the same rule `panes::header_mask` documents.
        for w in 0..compiled.flat.qubits {
            if let Some(rect) = c.wire(w) {
                fill(canvas, rect.x, rect.y, rect.w, rect.h, t.line_2);
            }
            if let Some((lx, ly)) = c.label(w) {
                // `|0>` or `|1>`: the state the program PREPARES, which is a real instruction —
                // the QASM backend emits an `x q[k];` for a `|1>`. Drawing every wire as `|0>`
                // would show a different circuit from the one that runs.
                let ket = if compiled.flat.init.get(w).copied().unwrap_or(false) { "|1>" } else { "|0>" };
                text::draw(canvas, lx, ly, GATE_LABEL, t.fg_4, ket);
            }
        }

        let visible = c.visible_cols();
        for (i, step) in compiled.flat.steps.iter().enumerate() {
            let col = compiled.columns[i];
            if col >= visible {
                continue;
            }
            let qs = step.qubits();
            let control = matches!(step, Step::Gate { gate: QirGate::CNOT, .. }
                | Step::Gate { gate: QirGate::Toffoli, .. }
                | Step::Gate { gate: QirGate::Fredkin, .. });

            // ★ The connector. The design's scene draws a CNOT as a dot and a box with NOTHING
            // between them; on two wires that reads by proximity, and on four it is ambiguous.
            // Standard notation joins them, and it costs one 1px fill.
            if qs.len() > 1 {
                if let (Some(&lo), Some(&hi)) = (qs.iter().min(), qs.iter().max()) {
                    if let Some(r) = c.connector(col, lo, hi) {
                        fill(canvas, r.x, r.y, r.w, r.h, t.line_2);
                    }
                }
            }

            let letter = step.letter();
            for (n, &wire) in qs.iter().enumerate() {
                // A controlled gate's leading arguments are controls: a 9px dot, not a box.
                if control && n + 1 < qs.len() {
                    if let Some(d) = c.dot(col, wire) {
                        shapes::fill_round_rect(
                            canvas, d.x.max(0) as usize, d.y.max(0) as usize,
                            d.w as usize, d.h as usize, (d.w / 2) as usize, t.fg,
                        );
                    }
                    continue;
                }
                let Some(g) = c.gate(col, wire) else { continue };
                let accent = self.hover == Some((col, wire));
                // `.gate { background: var(--surface) }` — the box is opaque so the wire and any
                // connector stop cleanly at its edge instead of running through the letter.
                shapes::fill_round_rect(
                    canvas, g.x.max(0) as usize, g.y.max(0) as usize,
                    g.w as usize, g.h as usize, circuit::gate_radius(), t.surface,
                );
                shapes::stroke_round_rect(
                    canvas, g.x.max(0) as usize, g.y.max(0) as usize,
                    g.w as usize, g.h as usize, circuit::gate_radius(),
                    if accent { t.accent } else { t.line_2 },
                );
                let lw = text::width(GATE_LABEL, &letter);
                let ly = text::centre_y(g.y, g.h, GATE_LABEL);
                text::draw(canvas, g.x + (g.w - lw) / 2, ly, GATE_LABEL, t.fg, &letter);
            }
        }

        let mut bottom = self.circuit_top() + compiled.flat.qubits as i32 * circuit::wire_h();

        // A diagram wider than its pane must SAY so. Silently dropping the last gates would draw a
        // picture of a different program, and unlike a list there is no scrollbar to hint at it.
        if c.overflow() {
            let cols = compiled.columns.iter().max().map(|m| m + 1).unwrap_or(0);
            let hidden = cols.saturating_sub(visible);
            text::draw(
                canvas, x, bottom, B3, t.fg_4,
                &format!("+{} more column{} \u{2014} widen the window", hidden, if hidden == 1 { "" } else { "s" }),
            );
            bottom += text::line_box(B3);
        }
        bottom
    }

    fn draw_measurement(&self, canvas: &mut Canvas, t: &Theme, x: i32, right: i32, y: i32) -> i32 {
        let Outcome::Ok(c) = &self.outcome else { return y };

        // "Measurement · exact", not "· 1024 shots". Nothing was sampled: these are the amplitudes.
        self.heading(canvas, t, x, right, y, "Measurement", "exact");
        let mut ty = y + circuit::heading_height() + sc(LB_GAP_BELOW);

        let dist = match &c.dist {
            Ok(d) => d,
            Err(why) => {
                for chunk in wrap(why, B3, right - x) {
                    text::draw(canvas, x, ty, B3, t.fg_3, &chunk);
                    ty += B3.line() as i32;
                }
                return ty;
            }
        };
        if dist.measured.is_empty() {
            text::draw(canvas, x, ty, B3, t.fg_3, "nothing is measured, so there is no outcome");
            return ty + B3.line() as i32;
        }

        // Rows fitting the pane, most likely first. A 4-qubit circuit has sixteen outcomes and
        // usually two that can happen; the rest are the point, but they are not worth the space.
        let room = ((self.height - ty - sc(80)) / circuit::outcome_row_height()).max(1) as usize;
        let ranked = dist.ranked();
        let shown = ranked.len().min(room);
        for (i, &(basis, p)) in ranked.iter().take(shown).enumerate() {
            let r = circuit::outcome_row(self.circ_pane(), ty, i);
            text::draw(canvas, r.label.0, r.label.1, GATE_LABEL, t.fg_2, &dist.label(basis));

            fill(canvas, r.track.x, r.track.y, r.track.w, r.track.h, t.wash_2);
            let f = circuit::outcome_fill(r.track, p);
            if f.w > 0 {
                fill(canvas, f.x, f.y, f.w, f.h, t.accent);
            }
            // `.trk.n` — an impossible outcome's number drops to `fg-3`, so the row reads as a zero
            // rather than as a missing bar.
            let col = if p > 0.0 { t.fg } else { t.fg_3 };
            let vy = text::baseline_with(GATE_LABEL, GATE_LABEL, r.label.1);
            text::draw_right(canvas, r.value_right, vy, GATE_LABEL, col, &pct(p));
        }
        ty += circuit::outcomes_height(shown);

        if shown < ranked.len() {
            let rest: f64 = ranked[shown..].iter().map(|&(_, p)| p).sum();
            text::draw(
                canvas, x, ty + sc(6), B3, t.fg_4,
                &format!("+{} more outcomes, {} between them", ranked.len() - shown, pct(rest)),
            );
            ty += sc(6) + text::line_box(B3);
        }

        // The design's `Fidelity 0.998` row, replaced by the one verification this machine can
        // actually make about this compile. See the module docs.
        let base = ty + sc(24);
        text::draw(canvas, x, base, B3, t.fg_3, "Host reference");
        let (verdict, colour) = match self.reference {
            Reference::Match => ("byte-identical", t.fg),
            Reference::Differs => ("differs", t.accent),
            Reference::Absent => ("none bundled", t.fg_4),
        };
        let vy = text::baseline_with(B2, B3, base);
        text::draw_right(canvas, right, vy, B2, colour, verdict);
        let mut out = base + B3.line() as i32;

        if let Some(p) = &self.wrote {
            let name = p.rsplit('/').next().unwrap_or(p);
            let note = format!("wrote {} \u{00B7} {} bytes", name, c.qasm_len);
            text::draw(canvas, x, out + sc(4), B3, t.fg_4, &note);
            out += sc(4) + text::line_box(B3);
        }
        out
    }
}

/// `.ql { font-size: 13px }` — [`nyx_meridian::tokens::MONO`] exactly, named locally so the code
/// pane's leading (25px, not MONO's 22px) is applied from one place.
const MONO_CODE: nyx_meridian::tokens::Style = nyx_meridian::tokens::MONO;

/// A probability as the design's tabular number. One decimal: the table's job is to separate "half
/// the time" from "never", and three decimals of an exact value is precision nobody reads.
fn pct(p: f64) -> String {
    let tenths = (p * 1000.0).round() as i64;
    format!("{}.{}%", tenths / 10, tenths % 10)
}

/// Break `s` into lines that fit `max_w`. Only diagnostics and refusals go through this, and both
/// are sentences that must not be silently cut in half.
fn wrap(s: &str, style: nyx_meridian::tokens::Style, max_w: i32) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in s.split_whitespace() {
        let probe = if line.is_empty() { word.to_string() } else { format!("{} {}", line, word) };
        if text::width(style, &probe) > max_w && !line.is_empty() {
            out.push(core::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = probe;
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// `Canvas::fill_rect` takes `usize` and has no notion of a negative origin, so every fill in this
/// file goes through here. A scrolled line has a negative `y` on the way out of view, and passing
/// that in as `usize` wraps it to an enormous coordinate.
fn fill(canvas: &mut Canvas, x: i32, y: i32, w: i32, h: i32, color: u32) {
    let (cw, ch) = (canvas.width as i32, canvas.height as i32);
    let (x0, y0) = (x.max(0), y.max(0));
    let (x1, y1) = ((x + w).min(cw), (y + h).min(ch));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    canvas.fill_rect(x0 as usize, y0 as usize, (x1 - x0) as usize, (y1 - y0) as usize, color);
}

impl NyxApp for QcLang {
    fn title(&self) -> &str {
        "QCLang"
    }
    fn icon_path(&self) -> &str {
        "/mnt/nvme/apps/QcStudio.nyx/icon.png"
    }
    /// `sample.ql · 2 qubits · depth 3`, drawn by the shell on the caption line beside the name —
    /// which is where the design has always had it. See `apps/notepad` for why the protocol only
    /// gained the ability to say this at step 15.
    fn context(&self) -> &str {
        &self.ctx
    }
    /// The design's own window: 1088 wide, quantised to 16 like every width in the system.
    fn initial_width(&self) -> usize {
        1088
    }
    fn initial_height(&self) -> usize {
        540
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let t = theme();
        self.width = canvas.width as i32;
        self.height = canvas.height as i32;
        self.scroll = self.scroll.clamp(0, self.max_scroll());

        canvas.fill_rect(0, 0, canvas.width, canvas.height, t.surface);
        self.draw_code(canvas, &t);
        self.draw_circuit_pane(canvas, &t);
    }

    fn content_height(&self) -> usize {
        let h = self.code_h();
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

    fn on_theme(&mut self, dark: bool) -> bool {
        DARK.swap(dark, std::sync::atomic::Ordering::Relaxed) != dark
    }

    fn on_mouse_move(&mut self, mx: usize, my: usize) -> bool {
        // The circuit does not scroll, so this is a straight window-space hit test. A cell is only
        // reported as hovered when something is actually drawn in it.
        let new = self
            .laid_out()
            .and_then(|(c, _)| c.cell_at(mx as i32, my as i32))
            .filter(|&(col, wire)| self.step_at(col, wire).is_some());
        if new != self.hover {
            self.hover = new;
            return true;
        }
        false
    }

    fn on_mouse_leave(&mut self) -> bool {
        self.hover.take().is_some()
    }
}

fn main() {
    // Both families: Inter for the headings and labels, JetBrains Mono for the source, the gate
    // letters and the basis-state labels. Not `register_all` — this window sets no `d1`, and the
    // ExtraLight face is 415 KB in a binary that would never draw a glyph of it.
    nyx_meridian::font::register_text();
    nyx_meridian::font::register_mono();
    nyx_gui::app::run(QcLang::new());
}
