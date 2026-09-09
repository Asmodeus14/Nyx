//! # Files — Meridian build-order step 10
//!
//! The first of the 10–16 application-interior ports. The design's own table says what this one is:
//! *"Files · `apps/explorer` · Loses its title bar and its toolbar; keeps its navigation."*
//!
//! So the Up button, the Refresh button, the path field, the raised header bar and the footer strip
//! are gone. What replaces them is not another bar — it is the design's `.nav` pane on the left, a
//! `.main-h` heading block, and rows of `.fr` separated by nothing but space. The window has no
//! chrome at all: `apps/shell` draws the caption on a line *outside* the surface, so every pixel
//! from (0,0) belongs to this app.
//!
//! ## Where the geometry lives
//!
//! In `nyx_meridian::panes`, not here. This is a `no_std` binary with a `_start` and its tests
//! cannot run on the host, and the machine has no QEMU — a hit-test that disagrees with a draw by
//! four pixels would be found by booting and squinting. Everything positional comes from one
//! function that `cargo test` exercises. See that module's header.
//!
//! ## Two things the design draws that this cannot honestly draw
//!
//! Both were checked against the kernel rather than assumed, and both are omissions rather than
//! inventions — the Entity's "no battery driver" rule applied to a file manager.
//!
//! - **The modification-date column.** `.fr .mt.b` shows "31 Aug". Nyx has a real `stat` (syscall 4,
//!   `vfs::FileStat` carries `mtime`) but **nothing ever writes a nonzero one**: `ext4_fs.c` sets
//!   every new inode's times to 0, the timestamp updates in `ext4.c` are commented out pending a
//!   wall clock, and `installer.rs::extract_tar_to_ext4` discards the tar's mtime field. Every row
//!   would read `1 Jan 1970`. A column of identical wrong dates is worse than no column: it makes
//!   the surface look finished and it is a lie.
//! - **The `initrd` device row.** The mock lists "nvme0 64 G" and "initrd 12 M". There is no initrd
//!   *mount* — `main.rs` unpacks a tar embedded in the kernel image onto ext4 at boot, so the
//!   initrd is a build artifact and not a filesystem with a capacity. Only `/mnt/nvme` and `/tmp`
//!   are mounted, and `sys_statfs` ignores its argument and always reports the NVMe, so `/tmp`'s
//!   capacity is not reachable either.
//!
//! ## And one it extends
//!
//! The heading is a **breadcrumb** rather than a bare folder name. The design's scene never
//! navigates anywhere, so its `.main-h .t` is just "Applications"; but the build order says to keep
//! the navigation while deleting the toolbar, and the toolbar held the only way up. Folding the
//! ancestors into the heading is cheaper than keeping the surface the design removed. `Backspace`
//! does the same thing from the keyboard.

#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
use nyx_meridian::panes::{self, CrumbKind, NavKind};
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B1, B2, B3, D2, HINT, LB};

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// The shell's theme, kept in step by `MSG_THEME_CHANGED` (see `NyxApp::on_theme` below).
///
/// A static rather than a field because `theme()` is called from every draw helper in this file and
/// threading a parameter through all of them would say nothing the name does not. Safe as a plain
/// atomic: an app is single-threaded, and this is one bool written by the message pump and read by
/// the painter.
static DARK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

fn theme() -> Theme {
    if DARK.load(core::sync::atomic::Ordering::Relaxed) { Theme::dark() } else { Theme::light() }
}

// --- NATIVE SYSCALL WRAPPERS FOR VFS ---
#[inline]
unsafe fn syscall_2(id: u64, a: u64, b: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

#[inline]
unsafe fn syscall_3(id: u64, a: u64, b: u64, c: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, in("rdx") c, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

#[inline]
unsafe fn syscall_4(id: u64, a: u64, b: u64, c: u64, d: u64) -> u64 {
    let ret: u64;
    core::arch::asm!("syscall", inlateout("rax") id => ret, in("rdi") a, in("rsi") b, in("rdx") c, in("r10") d, out("rcx") _, out("r11") _, options(nostack, preserves_flags));
    ret
}

fn dir_count(path: &str) -> usize {
    unsafe { syscall_2(510, path.as_ptr() as u64, path.len() as u64) as usize }
}

fn get_directory_names(path: &str) -> Vec<String> {
    let mut files = Vec::new();
    let count = dir_count(path);
    for i in 0..count {
        let mut buf = [0u8; 256];
        let len = unsafe { syscall_4(511, i as u64, buf.as_mut_ptr() as u64, path.as_ptr() as u64, path.len() as u64) };
        if len > 0 {
            if let Ok(s) = core::str::from_utf8(&buf[..len as usize]) { files.push(String::from(s)); }
        }
    }
    files
}

fn read_file(path: &str) -> String {
    let fd = unsafe { syscall_2(2, path.as_ptr() as u64, path.len() as u64) } as i64;
    if fd < 0 { return String::from("This is a directory, or it could not be opened."); }

    let mut buf = vec![0u8; 8192];
    let bytes_read = unsafe { syscall_3(0, fd as u64, buf.as_mut_ptr() as u64, buf.len() as u64) } as i64;
    unsafe { syscall_2(3, fd as u64, 0); }

    if bytes_read > 0 {
        String::from_utf8_lossy(&buf[..bytes_read as usize]).into_owned()
    } else {
        String::from("This file is empty.")
    }
}

fn join_path(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        alloc::format!("{}{}", dir, name)
    } else {
        alloc::format!("{}/{}", dir, name)
    }
}

/// Human-readable byte size (one decimal for KB/MB).
fn human_size(b: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if b < KB {
        alloc::format!("{} B", b)
    } else if b < MB {
        alloc::format!("{}.{} KB", b / KB, (b % KB) * 10 / KB)
    } else if b < GB {
        alloc::format!("{}.{} MB", b / MB, (b % MB) * 10 / MB)
    } else {
        alloc::format!("{}.{} GB", b / GB, (b % GB) * 10 / GB)
    }
}

/// The short form for the nav's count column, where 92px of `.nv .c` has to hold a whole device.
fn short_size(b: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if b >= GB {
        alloc::format!("{} G", b / GB)
    } else {
        alloc::format!("{} M", b / MB)
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// THE NAVIGATION PANE
// ─────────────────────────────────────────────────────────────────────────────

/// What a nav row's `.nv .c` column counts. Named rather than inferred, so refreshing the pane
/// cannot get it wrong by testing a label string.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Count {
    /// Nothing — a group heading.
    None,
    /// How many entries the directory holds.
    Entries,
    /// The device's total size, from `sys_statfs`.
    Capacity,
}

/// One entry in the nav pane. A heading has no path; a destination has one that exists.
///
/// ⚠️ Every path here was checked against what the running system actually mounts. The design's
/// "Recent / Home / Documents / Downloads" have nothing behind them — there is no `/home` anywhere
/// in the kernel, the image or `libs`, and a nav row that opens an empty or failing directory is
/// the same mistake as a dock glyph whose exec path is not on the image.
struct Nav {
    label: &'static str,
    /// `None` for a group heading.
    path: Option<&'static str>,
    counts: Count,
    /// The rendered `.nv .c` text, resolved once per navigation rather than per frame.
    count: String,
}

impl Nav {
    fn kind(&self) -> NavKind {
        if self.path.is_some() { NavKind::Item } else { NavKind::Group }
    }

    fn refresh(&mut self) {
        self.count = match (self.counts, self.path) {
            (Count::Entries, Some(p)) => match dir_count(p) {
                0 => String::new(),
                n => alloc::format!("{}", n),
            },
            // `sys_statfs` takes no path and always answers for the NVMe, so this is the only row
            // that can carry a capacity — and it is a real one.
            (Count::Capacity, _) => match sys_statfs() {
                Some(sf) => short_size(sf.total_bytes),
                None => String::new(),
            },
            _ => String::new(),
        };
    }
}

/// The pane, in the design's two-group shape. Four destinations, all real.
fn build_nav() -> Vec<Nav> {
    let head = |label| Nav { label, path: None, counts: Count::None, count: String::new() };
    let place = |label, path: &'static str, counts| Nav {
        label,
        path: Some(path),
        counts,
        count: String::new(),
    };

    let mut v = vec![
        head("Locations"),
        place("Applications", "/mnt/nvme/apps", Count::Entries),
        place("Temp", "/tmp", Count::Entries),
        place("Root", "/", Count::Entries),
        head("Devices"),
        place("nvme0", "/mnt/nvme", Count::Capacity),
    ];
    for n in v.iter_mut() {
        n.refresh();
    }
    v
}

// ─────────────────────────────────────────────────────────────────────────────
// THE FILE LIST
// ─────────────────────────────────────────────────────────────────────────────

/// A directory entry with its metadata resolved once per navigation, not per frame.
struct Entry {
    name: String,
    is_dir: bool,
    size: u64,
}

/// List a directory into entries with type + size resolved. A path that can't be opened as a file
/// (`sys_file_size < 0`) is a directory — a reliable detector that also covers synthetic mount
/// folders, complementing the trailing `/` the kernel appends to ext4 dirs.
fn load_dir(path: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for raw in get_directory_names(path) {
        let trimmed = raw.trim_end_matches('/');
        let full = join_path(path, trimmed);
        let sz = sys_file_size(&full);
        let is_dir = raw.ends_with('/') || sz < 0;
        out.push(Entry {
            name: trimmed.to_string(),
            is_dir,
            size: if sz >= 0 { sz as u64 } else { 0 },
        });
    }
    // Directories first, then files; each group alphabetical.
    out.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => core::cmp::Ordering::Less,
        (false, true) => core::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// OPENING A FILE
// ─────────────────────────────────────────────────────────────────────────────

/// An application that can be handed a file, and the extensions it claims.
///
/// A table rather than a chain of `if ends_with`, for the reason the shell's `APPS` table exists:
/// adding a handler is one row, and the extension list and the binary it maps to cannot drift apart
/// because they are the same row.
struct Handler {
    /// Lowercase, without the dot.
    exts: &'static [&'static str],
    exec: &'static str,
}

/// ⚠️ Every `exec` here must exist on the image. A row pointing at a binary that was never built
/// launches nothing and reads as a hang — the same failure as a dock glyph with no app behind it,
/// which is why the shell's dock deliberately differs from the design's.
const HANDLERS: &[Handler] = &[
    Handler {
        // What `libs/image` can actually decode: hand-written PNG + zune-jpeg + BMP/TGA. Both
        // decoders are 8-bit non-interlaced only, but a file they refuse still opens — the viewer
        // reports the error by name, which is more useful than Files silently declining to launch.
        exts: &["png", "jpg", "jpeg", "bmp", "tga"],
        exec: "/mnt/nvme/apps/ImageViewer.nyx/run.bin",
    },
    Handler {
        // Meridian step 14. A `.ql` opens in the window built to read one rather than in the text
        // editor: QCLang compiles it, draws its circuit and evaluates it exactly.
        exts: &["ql"],
        exec: "/mnt/nvme/apps/QcStudio.nyx/run.bin",
    },
    Handler {
        // Meridian step 15. Text understands Markdown's headings, which is why `md` leads; the rest
        // are the plain-text kinds that actually exist on this image. Deliberately NOT a catch-all —
        // handing Text a binary would fill its column with replacement characters, and Files
        // declining to launch is the more honest outcome.
        exts: &["md", "txt", "log", "toml", "json", "qasm"],
        exec: "/mnt/nvme/apps/Notepad.nyx/run.bin",
    },
];

/// The handler for `name`, by extension. None means Files opens it itself.
fn handler_for(name: &str) -> Option<&'static Handler> {
    let ext = name.rsplit_once('.')?.1;
    HANDLERS.iter().find(|h| {
        h.exts.iter().any(|e| {
            e.len() == ext.len()
                && e.bytes().zip(ext.bytes()).all(|(a, b)| a == b.to_ascii_lowercase())
        })
    })
}

/// Fork and hand `path` to `exec`.
///
/// Bracketed with the same breadcrumbs `apps/shell::launch` uses, and for the same reason: Files
/// going quiet after a double-click is indistinguishable from a freeze from the outside, and
/// `parent alive` is what tells the two apart. These are `write(2)`s, so each one proves the
/// process is still able to enter the kernel.
fn launch_with(exec: &str, path: &str) -> bool {
    sys_write(1, b"[FILES] pre-fork\n");
    if sys_fork() == 0 {
        sys_write(1, b"[FILES] child: execve\n");
        sys_execve_arg(exec, path);
        // Only reached if execve failed — otherwise the image is gone. The child MUST exit rather
        // than return: it is a fork of Files, and letting it fall back into the event loop would
        // put a second process on this window's SHM.
        sys_write(1, b"[FILES] child: execve FAILED\n");
        sys_exit(1);
    }
    sys_write(1, b"[FILES] parent alive\n");
    true
}

#[derive(PartialEq)]
enum AppState {
    Browsing,
    /// Reading a file in place.
    ///
    /// The design's table maps Text to `apps/notepad`, and in the end that is where this belongs —
    /// but nothing in `libs/api` can launch an application *with an argument*, so handing a path to
    /// Notepad is not currently expressible. Deleting the reader before its replacement can be
    /// reached would remove the only way to look at a file on this machine, which is the "deleting
    /// an application is the last step, never the first" rule from the build order.
    Reading,
}

struct FilesApp {
    state: AppState,
    nav: Vec<Nav>,
    /// The nav's shape, cached. `panes::nav_entry` needs it on every draw and on every pointer
    /// move, and it only changes if the pane itself does — which it does not.
    nav_kinds: Vec<NavKind>,
    current_path: String,
    entries: Vec<Entry>,
    scroll_px: i32,
    active_file: String,
    file_body: String,
    width: i32,
    height: i32,
    /// The row under the pointer, from `MSG_MOUSE_MOVE`. `.fr:hover`.
    hover_row: Option<usize>,
    /// The nav entry under the pointer. `.nv:hover`.
    hover_nav: Option<usize>,
    /// The selected row. `.fr.on` — driven by the arrow keys, since a click opens immediately.
    selected: Option<usize>,
    /// Capacity, re-read on each navigation for the `.main-h .s` line.
    statfs: Option<StatFs>,
}

impl FilesApp {
    fn new() -> Self {
        // Meridian step 16: "Reveal in Files". The Image window forks us with the directory the
        // image it is showing lives in, over the same `SYS_LAUNCH_ARG` route Files itself uses to
        // hand a file to another app — so the traffic goes both ways now.
        //
        // A path that lists to nothing falls back to the default rather than opening an empty pane:
        // an empty window is indistinguishable from a broken one, and the argument is the thing most
        // likely to be wrong. (An argument naming a genuinely empty directory falls back too. That
        // is a real, accepted miss — there is no syscall that separates "empty" from "not a
        // directory", and being wrong about an empty folder is cheaper than being wrong about a
        // typo.)
        let mut buf = [0u8; 512];
        let n = sys_launch_arg(&mut buf);
        let arg = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();
        let (initial, entries) = match (!arg.is_empty()).then(|| load_dir(arg)) {
            Some(e) if !e.is_empty() => (String::from(arg), e),
            _ => {
                let d = String::from("/mnt/nvme/apps");
                let e = load_dir(&d);
                (d, e)
            }
        };
        let nav = build_nav();
        let nav_kinds = nav.iter().map(|n| n.kind()).collect();
        Self {
            state: AppState::Browsing,
            nav,
            nav_kinds,
            entries,
            current_path: initial,
            scroll_px: 0,
            active_file: String::new(),
            file_body: String::new(),
            width: 880,
            height: 512,
            hover_row: None,
            hover_nav: None,
            selected: None,
            statfs: sys_statfs(),
        }
    }

    /// The content column, told whether the window server is painting a scrollbar over this app's
    /// right edge — without which the size text ends up underneath it.
    fn list(&self) -> panes::List {
        let n = self.entries.len();
        panes::list(self.width, self.height, panes::needs_scrollbar(self.width, self.height, n))
    }

    fn go(&mut self, path: String) {
        self.current_path = path;
        self.entries = load_dir(&self.current_path);
        self.scroll_px = 0;
        self.selected = None;
        // The rows under the pointer are now different rows.
        self.hover_row = None;
        self.statfs = sys_statfs();
        for n in self.nav.iter_mut() {
            n.refresh();
        }
    }

    fn go_up(&mut self) -> bool {
        if self.current_path == "/" {
            return false;
        }
        let mut parts: Vec<&str> = self.current_path.split('/').filter(|s| !s.is_empty()).collect();
        parts.pop();
        let up = if parts.is_empty() { String::from("/") } else { alloc::format!("/{}", parts.join("/")) };
        self.go(up);
        true
    }

    fn open(&mut self, i: usize) -> bool {
        let Some(e) = self.entries.get(i) else { return false };
        let target = join_path(&self.current_path, &e.name);
        if e.is_dir {
            self.go(target);
            return true;
        }
        // A file type someone else owns goes to its application, with the path as its argument.
        if let Some(h) = handler_for(&e.name) {
            let name = e.name.clone();
            if launch_with(h.exec, &target) {
                // Files stays where it is. The viewer is a separate window, and navigating away
                // from the folder you just opened something out of is not what was asked for.
                self.active_file = name;
                return true;
            }
        }
        self.active_file = e.name.clone();
        self.file_body = read_file(&target);
        self.state = AppState::Reading;
        true
    }

    /// The text a crumb is drawn with — and therefore measured with; see `panes::breadcrumb`.
    ///
    /// Only the **final** crumb takes the nav's friendly name, so the heading for
    /// `/mnt/nvme/apps` reads "Applications" and agrees with the highlighted nav row. The
    /// ancestors stay as the path spells them: renaming a middle crumb would produce
    /// "/ mnt / nvme0 / Applications", which is neither a path nor a place anyone recognises.
    fn crumb_label(&self, target: usize, raw: &str) -> String {
        if target >= self.current_path.len() {
            let path = &self.current_path[..target.min(self.current_path.len())];
            for n in self.nav.iter() {
                if n.path == Some(path) {
                    return String::from(n.label);
                }
            }
        }
        String::from(raw)
    }

    fn clamp_scroll(&mut self) {
        let max = panes::max_scroll(&self.list(), self.entries.len());
        self.scroll_px = self.scroll_px.clamp(0, max);
    }

    /// Scroll the selected row into view after an arrow key. Without this the selection walks off
    /// the bottom of the band and the list appears to stop responding.
    fn reveal(&mut self, i: usize) {
        let l = self.list();
        let top = i as i32 * l.row_h;
        if top < self.scroll_px {
            self.scroll_px = top;
        } else if top + l.row_h > self.scroll_px + l.band.h {
            self.scroll_px = top + l.row_h - l.band.h;
        }
        self.clamp_scroll();
    }

    fn step_selection(&mut self, delta: i32) -> bool {
        if self.entries.is_empty() {
            return false;
        }
        let last = self.entries.len() as i32 - 1;
        let next = match self.selected {
            Some(i) => (i as i32 + delta).clamp(0, last),
            None if delta > 0 => 0,
            None => last,
        } as usize;
        self.selected = Some(next);
        self.reveal(next);
        true
    }

    // ── Painting ────────────────────────────────────────────────────────────

    fn draw_nav(&self, canvas: &mut Canvas, t: &Theme) {
        for (i, n) in self.nav.iter().enumerate() {
            let Some(r) = panes::nav_entry(&self.nav_kinds, i) else { continue };
            match n.path {
                // `.nav .lb` — an uppercase, tracked group heading at the faintest tier.
                None => {
                    text::draw(canvas, r.x, text::centre_y(r.y, r.h, LB), LB, t.fg_4, n.label);
                }
                Some(p) => {
                    let active = p == self.current_path;
                    // `.nv.on { color: var(--fg) }` / `.nv:hover { color: var(--fg-2) }`.
                    let fg = if active {
                        t.fg
                    } else if self.hover_nav == Some(i) {
                        t.fg_2
                    } else {
                        t.fg_3
                    };
                    if active {
                        // `.nv.on::before` — a 2px accent mark, the design's only marker here.
                        // There is no highlighted ground: the row says it is current by being
                        // brighter and by carrying the one accent on this side of the window.
                        let b = panes::nav_bar(r);
                        canvas.fill_rect(b.x as usize, b.y as usize, b.w as usize, b.h as usize, t.accent);
                    }
                    let y = text::centre_y(r.y, r.h, B2);
                    text::draw(canvas, panes::nav_label_x(r), y, B2, fg, n.label);
                    if !n.count.is_empty() {
                        let cy = text::centre_y(r.y, r.h, HINT);
                        text::draw_right(canvas, panes::nav_count_right(r), cy, HINT, t.fg_4, &n.count);
                    }
                }
            }
        }
    }

    /// The heading block: the breadcrumb, then the one line that counts what is here and what is
    /// left on the disk.
    fn draw_head(&self, canvas: &mut Canvas, t: &Theme, l: &panes::List) {
        let y = text::centre_y(l.title.y, l.title.h, D2);
        let path = self.current_path.clone();
        let last = path.len();
        // The label a crumb is DRAWN with is also the one it is MEASURED with — see
        // `panes::breadcrumb`. Renaming "apps" to "Applications" without this overlaps the crumb
        // beside it, because the two strings are nowhere near the same width.
        let label = |target: usize, raw: &str| self.crumb_label(target, raw);
        panes::breadcrumb(&path, D2, label, l.title.w, |kind, s, x, _| {
            // Only the final crumb is the heading; its ancestors are context, toned back so the
            // heading still reads as one word rather than as a path.
            let color = match kind {
                CrumbKind::Segment(target) if target >= last => t.fg,
                CrumbKind::Segment(_) => t.fg_3,
                CrumbKind::Separator => t.fg_4,
            };
            text::draw(canvas, l.title.x + x, y, D2, color, s);
        });

        let n = self.entries.len();
        let items = alloc::format!("{} item{}", n, if n == 1 { "" } else { "s" });
        let sub = match self.statfs {
            Some(sf) => alloc::format!(
                "{} · {} free of {}",
                items,
                human_size(sf.free_bytes),
                human_size(sf.total_bytes)
            ),
            // No capacity is a fact about the mount, not a number to invent.
            None => items,
        };
        let sy = text::centre_y(l.sub.y, l.sub.h, B3);
        text::draw(canvas, l.sub.x, sy, B3, t.fg_3, &sub);
    }

    fn draw_rows(&self, canvas: &mut Canvas, t: &Theme, l: &panes::List) {
        if self.entries.is_empty() {
            let y = l.band.y + l.row_h / 2;
            text::draw(canvas, l.content.x, y, B2, t.fg_4, "This folder is empty.");
            return;
        }

        let icon_px = panes::fr_icon_px();
        let (name_x, name_w) = panes::row_name(l);
        let meta_right = panes::row_meta_right(l);
        let (first, last) = panes::visible_rows(l, self.scroll_px, self.entries.len());

        for i in first..last {
            let e = &self.entries[i];
            let plate = panes::row_plate(l, i, self.scroll_px);
            if plate.bottom() <= 0 || plate.y >= l.band.bottom() {
                continue;
            }

            let selected = self.selected == Some(i);
            let hovered = self.hover_row == Some(i);
            // `.fr.on { background: var(--wash-2) }`, `.fr:hover { background: var(--wash) }`.
            // Both are translucent tokens, so this blends rather than writes — which is the whole
            // reason Meridian can skip a blur pass over a low-frequency ground.
            //
            // Drawn at FULL height even when the row is half out of the band: the header mask
            // repaints the overhang afterwards, which cuts it square at the band edge. Clipping the
            // rect here instead would round the corners at the cut, so the wash would look like it
            // ends there rather than continuing under the heading.
            if selected || hovered {
                let wash = if selected { t.wash_2 } else { t.wash };
                let top = plate.y.max(0);
                nyx_meridian::shapes::fill_round_rect(
                    canvas,
                    plate.x.max(0) as usize,
                    top as usize,
                    plate.w as usize,
                    (plate.bottom().min(l.band.bottom()) - top).max(0) as usize,
                    panes::fr_radius(),
                    wash,
                );
            }

            let ic = panes::row_icon(l, plate);
            let id = if e.is_dir { nyx_meridian::Icons::FOLDER } else { nyx_meridian::Icons::DOC };
            // `.fr .ic { color: var(--fg-4) }`, `.fr.on .ic { color: var(--fg-3) }` — the icon is
            // the quietest thing in the row, and it lifts one tier when the row is active.
            let icon_fg = if selected || hovered { t.fg_3 } else { t.fg_4 };
            text::icon(canvas, ic.x, ic.y, id, icon_px, icon_fg);

            let ty = text::centre_y(plate.y, plate.h, B1);
            let name = text::ellipsize(B1, &e.name, name_w);
            text::draw(canvas, name_x, ty, B1, t.fg, &name);

            // Directories carry no size. A recursive walk is the only way to get one, and this is
            // the paint path; the design shows sizes for folders and this machine cannot supply
            // them without making every navigation walk the whole tree.
            if !e.is_dir {
                let my = text::centre_y(plate.y, plate.h, B3);
                text::draw_right(canvas, meta_right, my, B3, t.fg_3, &human_size(e.size));
            }
        }
    }

    /// The file reader. One column on the sunken ground, no toolbar — `Esc` or `Backspace` returns.
    fn draw_reader(&self, canvas: &mut Canvas, t: &Theme) {
        let l = panes::list(self.width, self.height, false);
        canvas.fill_rect(0, 0, self.width as usize, self.height as usize, t.sunken);

        let y = text::centre_y(l.title.y, l.title.h, D2);
        let w = text::draw(canvas, l.title.x, y, D2, t.fg, &self.active_file);
        let hy = text::centre_y(l.title.y, l.title.h, HINT);
        text::draw(canvas, l.title.x + w + 14, hy, HINT, t.fg_4, "esc to close");

        let line_h = B2.line() as i32;
        let mut cy = l.band.y;
        let mut cx = l.content.x;
        let right = l.content.right();
        // ★ Encoded into a stack buffer, not `format!`. This loop runs over the whole 8 KB read
        // buffer on every frame the reader is up; one `String` allocation per character is ~8000
        // trips through a 1 MB heap per repaint, which is how an app that draws fine becomes an app
        // that stutters and then runs out of memory.
        let mut buf = [0u8; 4];
        for c in self.file_body.chars() {
            if cy > self.height - line_h {
                break;
            }
            match c {
                '\n' => {
                    cx = l.content.x;
                    cy += line_h;
                    continue;
                }
                // A file written on another system carries these, and the proportional face has no
                // glyph for them — they would rasterize as a notdef box in the middle of the text.
                '\r' => continue,
                '\t' => {
                    cx += 4 * text::width(B2, " ");
                    continue;
                }
                c if (c as u32) < 0x20 => continue,
                _ => {}
            }
            cx += text::draw(canvas, cx, cy, B2, t.fg_2, c.encode_utf8(&mut buf));
            if cx >= right {
                cx = l.content.x;
                cy += line_h;
            }
        }
    }
}

impl NyxApp for FilesApp {
    fn icon_path(&self) -> &str { "/mnt/nvme/apps/Explorer.nyx/icon.png" }
    fn title(&self) -> &str { "Files" }
    fn initial_width(&self) -> usize { 880 }
    fn initial_height(&self) -> usize { 512 }

    /// ★ The rows PLUS the header block above them — see `panes::reported_content_height`. The
    /// window server drives the scrollbar against the whole window, not against the band, so
    /// reporting the bare row height makes a full-length drag stop short of the last row.
    fn content_height(&self) -> usize {
        if self.state == AppState::Browsing {
            panes::reported_content_height(&self.list(), self.entries.len()) as usize
        } else {
            0
        }
    }
    fn scroll_offset(&self) -> usize { self.scroll_px.max(0) as usize }
    fn on_scroll(&mut self, new_offset: usize) -> bool {
        self.scroll_px = new_offset as i32;
        self.clamp_scroll();
        true
    }

    fn on_theme(&mut self, dark: bool) -> bool {
        use core::sync::atomic::Ordering;
        if DARK.swap(dark, Ordering::Relaxed) == dark {
            return false;
        }
        true
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        self.width = canvas.width as i32;
        self.height = canvas.height as i32;
        let t = theme();

        if self.state == AppState::Reading {
            self.draw_reader(canvas, &t);
            return;
        }

        // A resize can leave the scroll past the end of a now-taller band.
        self.clamp_scroll();
        let l = self.list();

        // No cards, no panels, no rules: the nav and the list share one ground and are separated by
        // space alone. The buffer is never cleared between frames, so this full fill is also what
        // erases the last one.
        canvas.fill_rect(0, 0, self.width as usize, self.height as usize, t.surface);
        self.draw_nav(canvas, &t);

        // ★ Order matters: rows, then the mask, then the heading. The band does not clip — nothing
        // in `Canvas` does — so a half-scrolled row paints straight through the subtitle above it.
        // See `panes::header_mask` for why this is painter's order rather than a scissor.
        self.draw_rows(canvas, &t, &l);
        let m = panes::header_mask(&l, self.width);
        canvas.fill_rect(m.x as usize, m.y as usize, m.w as usize, m.h as usize, t.surface);
        self.draw_head(canvas, &t, &l);
    }

    fn on_mouse_move(&mut self, mx: usize, my: usize) -> bool {
        if self.state != AppState::Browsing {
            return false;
        }
        let (mx, my) = (mx as i32, my as i32);
        let l = self.list();
        let row = panes::row_at(&l, mx, my, self.scroll_px, self.entries.len());
        let nav = panes::nav_hit(&self.nav_kinds, mx, my);
        // Redraw ONLY on a change. Returning true for every pointer movement would repaint the
        // whole window at the mouse's sample rate for no visible difference.
        let changed = row != self.hover_row || nav != self.hover_nav;
        self.hover_row = row;
        self.hover_nav = nav;
        changed
    }

    fn on_mouse_leave(&mut self) -> bool {
        self.hover_row.take().is_some() | self.hover_nav.take().is_some()
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        let (mx, my) = (mx as i32, my as i32);
        if self.state == AppState::Reading {
            self.state = AppState::Browsing;
            return true;
        }

        let l = self.list();

        // The nav pane.
        if let Some(i) = panes::nav_hit(&self.nav_kinds, mx, my) {
            if let Some(p) = self.nav[i].path {
                if p != self.current_path {
                    self.go(String::from(p));
                }
                return true;
            }
        }

        // An ancestor in the heading. Walked through the same function that drew it, so a crumb is
        // clickable exactly where it looks clickable.
        if my >= l.title.y && my < l.title.bottom() {
            let path = self.current_path.clone();
            let label = |target: usize, raw: &str| self.crumb_label(target, raw);
            if let Some(target) = panes::crumb_at(&path, D2, label, l.title.w, l.title.x, mx) {
                let to = &path[..target.min(path.len()).max(1)];
                if to != self.current_path {
                    let to = String::from(to);
                    self.go(to);
                    return true;
                }
            }
        }

        // A row.
        if let Some(i) = panes::row_at(&l, mx, my, self.scroll_px, self.entries.len()) {
            self.selected = Some(i);
            return self.open(i);
        }
        false
    }

    fn on_key(&mut self, key: char) -> bool {
        if self.state == AppState::Reading {
            return match key {
                '\x1b' | '\x08' => {
                    self.state = AppState::Browsing;
                    true
                }
                _ => false,
            };
        }
        match key {
            keys::UP => self.step_selection(-1),
            keys::DOWN => self.step_selection(1),
            keys::HOME => self.step_selection(-(self.entries.len() as i32)),
            keys::END => self.step_selection(self.entries.len() as i32),
            keys::PAGE_UP | keys::PAGE_DOWN => {
                let rows = (self.list().band.h / self.list().row_h.max(1)).max(1);
                self.step_selection(if key == keys::PAGE_UP { -rows } else { rows })
            }
            '\n' | '\r' => match self.selected {
                Some(i) => self.open(i),
                None => false,
            },
            // The toolbar's Up button, without the toolbar.
            '\x08' | keys::LEFT => self.go_up(),
            keys::RIGHT => match self.selected {
                Some(i) => self.open(i),
                None => false,
            },
            _ => false,
        }
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // ★ 2026-09-08: the typefaces are no longer `include_bytes!`d into this binary — they are
    // read out of the shared bundle at /mnt/nvme/fonts/ (see `nyx_meridian::font`). That took
    // 1.24 MB off the boot image and put it on THIS HEAP instead, so the heap had to grow to
    // hold it. A heap too small to take the fonts is not a crash: the read fails, every face
    // falls back to DejaVu, and the window renders in the wrong typeface at the right size.
    let heap_start = sys_alloc_pages(768);
    if heap_start == 0 { sys_exit(1); }
    unsafe { ALLOCATOR.lock().init(heap_start as *mut u8, 768 * 4096); }

    // Install Inter before the first frame. Only the three faces this interior sets type in —
    // `register_all` would add Inter ExtraLight and JetBrains Mono, about 690 KB of font this app
    // never draws a glyph of. A face that fails to parse falls back to DejaVu, so the failure mode
    // is the wrong typeface rather than an empty window.
    nyx_meridian::font::register_text();

    nyx_gui::app::run(FilesApp::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { sys_exit(111); }
