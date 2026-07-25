// Nyx Terminal — Workstream D (D3): ported from no_std to a `target_os = "nyx"` **std** binary.
//
// This is the same terminal UI as before (matrix-green shell over nyx-gui), now built on real std so
// it can host the pluggable compiler registry in D4 (which needs std::fs + the std-only
// `nyx_toolchains` crate). The port is the exact 4-item swap D2 proved out: the no_std app's own
// `_start`/.text.entry + #[global_allocator] + #[panic_handler] are gone — std owns the heap, the
// entry, and panics — and `alloc::` becomes `std::`. nyx-gui/nyx-api link in UNCHANGED.
//
// Behaviour is intentionally identical to the no_std terminal for this step; the `compile`/`toolchains`
// commands land in D4 so this boot test cleanly answers "did the std port work".
//
// `restricted_std` is rustc's required opt-in for a std built for an out-of-tree target it doesn't
// recognise; the nyx PAL is injected into rust-src by vendor/nyx-std/apply.py.
#![feature(restricted_std)]

// ELF entry — identical contract to apps/stdgui, apps/qcstudio and tests/stdhello: the kernel hands
// us a SysV stack [argc][argv..][NULL][envp..][NULL][auxv..] with RSP % 16 == 8. Install the main
// thread's native (fs:-relative) TLS block BEFORE anything touches a #[thread_local], then forward
// argc/argv to the rustc-generated C `main` and exit_group(231) with its return value.
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

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
// D4: the pluggable compiler registry — dispatches a source file to a language backend by extension.
use nyx_toolchains::{Artifact, Registry};

// D4: bundled by Build.sh into the Terminal app dir, so `compile` with no argument has something to
// chew on out of the box (a Bell-pair .ql — same sample qcstudio uses).
const DEFAULT_SAMPLE: &str = "/mnt/nvme/apps/Terminal.nyx/sample.ql";

const BG_COLOR: u32 = 0xFF0D0D0D;
const FG_COLOR: u32 = 0xFF00FF66;
const FONT_W: usize = 8;
const FONT_H: usize = 8;

struct TerminalApp {
    input_buffer: String,
    output_history: String,
    blink_timer: usize,
    cursor_visible: bool,
}

impl TerminalApp {
    fn new() -> Self {
        Self {
            input_buffer: String::new(),
            output_history: String::from("NyxOS v0.1 Shell\nType 'help' for commands.\n"),
            blink_timer: 0,
            cursor_visible: true,
        }
    }

    // D4: `toolchains` — enumerate the registry's installed language backends. This is the single
    // place the terminal learns what languages it can build; adding one is a one-line change in
    // Registry::with_defaults(), no terminal edit needed.
    fn list_toolchains(&mut self) {
        let reg = Registry::with_defaults();
        self.output_history.push_str("Installed toolchains:\n");
        for b in reg.backends() {
            let exts = b.extensions().join(", .");
            self.output_history.push_str(&format!(
                "  {:<8} .{}  - {}\n",
                b.name(),
                exts,
                b.describe()
            ));
        }
    }

    // D4: `compile <file>` — read a source file via std::fs, hand it to the registry (which picks the
    // backend by extension), and report the result. This is the whole point of Workstream D: the
    // terminal compiles any registered language without knowing which one it is.
    fn do_compile(&mut self, path: &str) {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                self.output_history.push_str(&format!("compile: cannot read '{path}': {e}\n"));
                return;
            }
        };

        let reg = Registry::with_defaults();
        match reg.compile(path, &source) {
            Ok(Artifact::Text { label, suggested_ext, content }) => {
                let out = replace_ext(path, &suggested_ext);
                match std::fs::write(&out, content.as_bytes()) {
                    Ok(()) => self.output_history.push_str(&format!(
                        "compile: {} -> {} ({} bytes of {})\n",
                        path, out, content.len(), label
                    )),
                    Err(e) => self.output_history.push_str(&format!(
                        "compile: built {} but could not write {out}: {e}\n",
                        label
                    )),
                }
                // Echo the produced text so the result is visible in the terminal itself.
                for line in content.lines() {
                    self.output_history.push_str("  ");
                    self.output_history.push_str(line);
                    self.output_history.push('\n');
                }
            }
            Ok(Artifact::Ran { output }) => {
                self.output_history.push_str("compile: program output:\n");
                for line in output.lines() {
                    self.output_history.push_str("  ");
                    self.output_history.push_str(line);
                    self.output_history.push('\n');
                }
            }
            Ok(Artifact::Executable { bytes }) => {
                self.output_history.push_str(&format!(
                    "compile: produced a {}-byte executable (run support TBD)\n",
                    bytes.len()
                ));
            }
            Err(diags) => {
                self.output_history.push_str(&format!("compile: FAILED ({} diagnostic(s)):\n", diags.len()));
                for d in &diags {
                    self.output_history.push_str(&format!("  {}\n", d.message));
                }
            }
        }
    }
}

// Swap a path's extension for `new_ext` (no dot). If there's no extension, append one. Kept simple:
// operates on the final path component so a dotted directory earlier in the path is left alone.
fn replace_ext(path: &str, new_ext: &str) -> String {
    let slash = path.rfind(|c| c == '/' || c == '\\').map(|i| i + 1).unwrap_or(0);
    match path[slash..].rfind('.') {
        Some(rel_dot) => format!("{}.{}", &path[..slash + rel_dot], new_ext),
        None => format!("{}.{}", path, new_ext),
    }
}

impl NyxApp for TerminalApp {
    fn icon_path(&self) -> &str { "/mnt/nvme/apps/Terminal.nyx/icon.png" }
    fn title(&self) -> &str { "Nyx Matrix Terminal" }
    fn initial_width(&self) -> usize { 640 }
    fn initial_height(&self) -> usize { 400 }

    fn update(&mut self) -> bool {
        self.blink_timer += 1;
        if self.blink_timer > 30 {
            self.blink_timer = 0;
            self.cursor_visible = !self.cursor_visible;
            return true; // Force redraw to show/hide cursor
        }
        false
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        canvas.fill_rect(0, 0, canvas.width, canvas.height, BG_COLOR);

        // F1: proportional TTF metrics — advance per glyph, line height from the font. FONT_W/FONT_H
        // remain only as a fallback caret size.
        let line_h = nyx_gui::font::line_height(1);
        let mut cx = 10;
        let mut cy = 10;

        // Draw History
        for c in self.output_history.chars() {
            if c == '\n' { cx = 10; cy += line_h; continue; }
            canvas.draw_char(cx, cy, c, FG_COLOR, 1);
            cx += nyx_gui::font::advance(c, 1);
            if cx >= canvas.width - 15 { cx = 10; cy += line_h; }
        }

        // Draw Prompt
        let prompt = "N> ";
        for c in prompt.chars() {
            canvas.draw_char(cx, cy, c, FG_COLOR, 1);
            cx += nyx_gui::font::advance(c, 1);
        }

        // Draw Input Buffer
        for c in self.input_buffer.chars() {
            canvas.draw_char(cx, cy, c, FG_COLOR, 1);
            cx += nyx_gui::font::advance(c, 1);
            if cx >= canvas.width - 15 { cx = 10; cy += line_h; }
        }

        // Draw Cursor — a caret sized to a space advance × line height, at the current pen.
        if self.cursor_visible {
            let cw = nyx_gui::font::advance(' ', 1).max(FONT_W);
            canvas.fill_rect(cx, cy, cw, line_h.min(FONT_H * 2), FG_COLOR);
        }
    }

    fn on_key(&mut self, key: char) -> bool {
        self.cursor_visible = true;
        self.blink_timer = 0;

        if key == '\n' || key == '\r' {
            // Own the command string so the dispatch below can take `&mut self` (do_compile /
            // list_toolchains) without colliding with a borrow of self.input_buffer.
            let cmd = self.input_buffer.trim().to_string();
            let cmd = cmd.as_str();
            self.output_history.push_str("N> ");
            self.output_history.push_str(cmd);
            self.output_history.push('\n');

            if cmd == "help" {
                self.output_history.push_str("Commands: help, clear, echo <text>, settings, explorer, sysmon, network\n");
                self.output_history.push_str("  toolchains        - list installed language compilers\n");
                self.output_history.push_str("  compile [file]    - compile a source file (defaults to the bundled sample.ql)\n");
            } else if cmd == "clear" {
                self.output_history.clear();
            } else if cmd == "toolchains" {
                self.list_toolchains();
            } else if cmd == "compile" {
                self.do_compile(DEFAULT_SAMPLE);
            } else if let Some(arg) = cmd.strip_prefix("compile ") {
                self.do_compile(arg.trim());
            } else if cmd == "settings" {
                self.output_history.push_str("Launching Settings...\n");
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Settings.nyx/run.bin\0"); sys_exit(1); }
            } else if cmd == "explorer" {
                self.output_history.push_str("Launching Explorer...\n");
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Explorer.nyx/run.bin\0"); sys_exit(1); }
            } else if cmd == "sysmon" {
                self.output_history.push_str("Launching System Monitor...\n");
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/SystemMonitor.nyx/run.bin\0"); sys_exit(1); }
            } else if cmd == "network" {
                self.output_history.push_str("Launching Network Suite...\n");
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Network.nyx/run.bin\0"); sys_exit(1); }
            } else if cmd.starts_with("echo ") {
                self.output_history.push_str(&cmd[5..]);
                self.output_history.push('\n');
            } else if !cmd.is_empty() {
                self.output_history.push_str("Unknown command. Type 'help'.\n");
            }
            self.input_buffer.clear();
        } else if key == '\x08' {
            self.input_buffer.pop();
        } else {
            self.input_buffer.push(key);
        }
        true // Redraw instantly on keypress
    }
}

fn main() {
    // Headless breadcrumb in the serial log so the boot test is legible even before the window paints.
    println!("nyx-terminal: D3 std port — shell over nyx-gui on target_os=nyx");
    // run() takes over the process (event loop, never returns).
    nyx_gui::app::run(TerminalApp::new());
}
