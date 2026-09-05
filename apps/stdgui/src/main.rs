// Workstream D (D2): std-GUI foundation spike.
//
// Goal: prove that a `target_os = "nyx"` **std** binary can link the existing **no_std** GUI stack
// (nyx-gui + nyx-api) UNCHANGED and open a real window. This is the keystone check before porting
// the terminal to std (D3) and wiring in the compiler registry (D4): if the font pipeline
// (ttf-parser + ab_glyph_rasterizer + libm) builds for nyx under build-std and a std app can drive
// the compositor, the whole multi-language-terminal plan is unblocked.
//
// What makes this a genuine test and not just "a window":
//   * It draws with `nyx_gui::canvas` — exercising the antialiased TTF font engine from std.
//   * `draw()` formats live text with `format!` (std alloc) and reads `std::time::Instant`
//     (the std time PAL) — so a correct boot proves std + GUI coexisting, not merely linking.
//
// `restricted_std` is rustc's required opt-in for a std built for an out-of-tree target it doesn't
// recognise; the nyx PAL is injected into rust-src by vendor/nyx-std/apply.py.
#![feature(restricted_std)]

// ELF entry — identical contract to apps/qcstudio and tests/stdhello: the kernel hands us a SysV
// stack [argc][argv..][NULL][envp..][NULL][auxv..] with RSP % 16 == 8. Install the main thread's
// native (fs:-relative) TLS block BEFORE anything touches a #[thread_local] (rt::init reads
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
    "call main",            // rustc-generated `int main(int, char**)` -> std lang_start
    "mov edi, eax",         // propagate exit code
    "mov eax, 231",         // SYS_exit_group
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};
use std::time::Instant;

struct StdGuiApp {
    /// Set on the first draw so the on-screen uptime is measured from window-open, using the std
    /// monotonic clock (proves the std `time` PAL works inside the GUI event loop).
    started: Option<Instant>,
    /// Repaint counter — proves `update()` is being pumped and `format!` (std alloc) works per frame.
    frames: u64,
}

impl StdGuiApp {
    fn new() -> Self {
        Self { started: None, frames: 0 }
    }
}

impl NyxApp for StdGuiApp {
    fn icon_path(&self) -> &str { "/mnt/nvme/apps/StdGui.nyx/icon.png" }
    fn title(&self) -> &str {
        "Std GUI Spike (D2)"
    }

    fn initial_width(&self) -> usize {
        520
    }
    fn initial_height(&self) -> usize {
        300
    }

    // Repaint on a timer so the uptime readout ticks — returning true asks the loop to redraw.
    fn update(&mut self) -> bool {
        true
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let now = Instant::now();
        let start = *self.started.get_or_insert(now);
        self.frames += 1;

        let w = canvas.width;
        let h = canvas.height;

        canvas.fill_rect(0, 0, w, h, Color::SURFACE);
        canvas.fill_rect(0, 0, w, 40, Color::RAISED);
        canvas.fill_rect(0, 40, w, 1, Color::LINE);

        canvas.print_str(16, 12, "Rust std, running on Nyx", Color::FG, 2);

        canvas.print_str(16, 64, "This window is drawn by a target_os=nyx", Color::FG, 1);
        canvas.print_str(16, 84, "binary built with -Z build-std=std.", Color::FG, 1);
        canvas.print_str(16, 112, "It links nyx-gui + nyx-api UNCHANGED.", Color::FG, 1);

        // Live std proof: format! (alloc) + std::time (monotonic clock) inside the GUI loop.
        let uptime_ms = now.duration_since(start).as_millis();
        let line = format!("std::time uptime: {} ms   frames: {}", uptime_ms, self.frames);
        canvas.print_str(16, 152, &line, Color::FG, 1);

        canvas.print_str(16, 200, "D2 milestone: std + GUI coexist.", Color::FG, 1);
    }
}

fn main() {
    // Headless breadcrumb in the serial log so the boot test is legible even before the window paints.
    println!("stdgui: D2 spike — std binary linking no_std nyx-gui, opening a window");
    // run() takes over the process (event loop, never returns).
    nyx_gui::app::run(StdGuiApp::new());
}
