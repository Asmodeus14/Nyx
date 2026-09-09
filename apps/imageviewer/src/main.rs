//! # Image — Meridian build-order step 16
//!
//! The design's lede is the specification: *"Image is the image: its controls are plain text on a
//! hairline at the foot of the surface, and they appear on hover, exactly as the browser's address
//! does."* And: *"No filmstrip, no chrome, no checkerboard."*
//!
//! What that deletes from the old viewer is the permanent 28px footer strip and the flat `#1E1E1E`
//! backdrop that was neither of the theme's grounds. What replaces them is `--sunken` (or `--surface`
//! in the light theme), a hairline frame around the image with 4px corners, and a bar that is not
//! there until the pointer is.
//!
//! ## The bar floats
//!
//! `.iv-b` is `position: absolute`, so the image is centred in the **whole** surface and the bar is
//! drawn over it on the translucent `--chrome` ground. That is what makes "they appear on hover"
//! work at all: a bar that reserved 42px of height would make the image jump every time the pointer
//! crossed the window edge.
//!
//! ## Where the filename went
//!
//! Onto the caption line, with the dimensions and the file size —
//! `apps / sample.png · 1280×720 · 214 KB` — which is what step 15 added `WindowHeader::context` for.
//! The old viewer spent its footer saying that; the design spends the footer on controls instead,
//! and the context belongs outside the surface with every other piece of window context.
//!
//! ## What Previous / Next actually walk
//!
//! The directory the image is in, listed with `sys_fs_count`/`sys_fs_get_name` and filtered to the
//! extensions `libs/image` can decode. Not the three bundled samples — those were a stand-in from
//! before Files could hand this app a path.
//!
//! ⚠️ This revisits an earlier decision deliberately. When the launch-argument path landed, a launch
//! argument became the **only** candidate, because back then *any click anywhere* cycled the image
//! and quietly walking away from the file someone asked for was answering a different question. The
//! objection was to the unlabelled click, not to having neighbours: `Previous` and `Next` are named
//! controls that say what they do, so the reason no longer applies. The click-to-cycle behaviour is
//! gone.
//!
//! ## Two things this cannot do, and one it will not
//!
//! - **Drag to pan.** The window protocol delivers `MSG_MOUSE_EVENT` (a click) and `MSG_MOUSE_MOVE`
//!   (a move with no button held). There is no button-held-move event, so a drag is not expressible.
//!   At 1:1 the **arrow keys** pan instead, and the window's own scrollbar drives the vertical axis.
//! - **Enlarge on Fit.** Fit shrinks a large image and leaves a small one alone. Nearest-neighbour
//!   up-scaling a 32px icon across a 1400px window produces a mosaic, and "fit" is a request to see
//!   all of an image, not to magnify it.
//! - **Colour management.** The design's own note: *"there is no ICC path, no display profile and no
//!   gamma correction anywhere in the stack — which is worth stating plainly, because an image
//!   viewer is precisely the application where someone will eventually assume otherwise."* A decoded
//!   PNG arrives on screen exactly as its bytes describe it.

#![no_std]
#![no_main]
#![allow(warnings)]

extern crate alloc;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use linked_list_allocator::LockedHeap;

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
use nyx_image::{decode, Image};
use nyx_meridian::layout::Rect;
use nyx_meridian::shapes;
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B3};
use nyx_meridian::viewer;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Where the viewer looks when nothing hands it a file — its own bundle, which ships one sample per
/// decoder so every format gets exercised on a machine with no other images on it.
const BUNDLE: &str = "/mnt/nvme/apps/ImageViewer.nyx";

/// What `libs/image` can actually decode. Both the PNG and JPEG decoders are 8-bit non-interlaced
/// only, but a file they refuse still opens — the window reports the error by name, which is more
/// useful than the file silently not appearing in Previous/Next.
const EXTS: [&str; 5] = ["png", "jpg", "jpeg", "bmp", "tga"];

static DARK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

fn theme() -> Theme {
    if DARK.load(core::sync::atomic::Ordering::Relaxed) { Theme::dark() } else { Theme::light() }
}

fn sc(v: i32) -> i32 {
    nyx_meridian::scale::px(v)
}

// ── files ────────────────────────────────────────────────────────────────────────────────────────

fn read_file_bytes(path: &str) -> Vec<u8> {
    let fd = sys_open(path);
    if fd < 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut chunk = vec![0u8; 65536];
    loop {
        let n = sys_read(fd, &mut chunk);
        if n <= 0 {
            break;
        }
        out.extend_from_slice(&chunk[..n as usize]);
    }
    sys_close(fd);
    out
}

fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => ".",
    }
}

fn short_name(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

fn is_image(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else { return false };
    EXTS.iter().any(|e| {
        e.len() == ext.len() && e.bytes().zip(ext.bytes()).all(|(a, b)| a == b.to_ascii_lowercase())
    })
}

/// Every decodable image in `dir`, in the order the filesystem reports them.
///
/// Not sorted. `sys_fs_get_name` walks ext4's directory entries, and Files presents that same order
/// — so Previous/Next steps through the images in the order they appear in the window someone just
/// came from, which is the only ordering that will not surprise them.
fn images_in(dir: &str) -> Vec<String> {
    let mut out = Vec::new();
    let n = sys_fs_count(dir);
    let mut buf = [0u8; 256];
    for i in 0..n {
        let len = sys_fs_get_name(dir, i, &mut buf);
        let Ok(name) = core::str::from_utf8(&buf[..len]) else { continue };
        if is_image(name) {
            out.push(alloc::format!("{}/{}", dir.trim_end_matches('/'), name));
        }
    }
    out
}

fn human_size(b: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = 1024 * 1024;
    if b < KB {
        alloc::format!("{} B", b)
    } else if b < MB {
        alloc::format!("{}.{} KB", b / KB, (b % KB) * 10 / KB)
    } else {
        alloc::format!("{}.{} MB", b / MB, (b % MB) * 10 / MB)
    }
}

// ── the app ──────────────────────────────────────────────────────────────────────────────────────

/// The bar's items, in the order they are laid out. `Zoom` is a readout and not a control; it is in
/// the list so it takes its place in the flex row, and the app simply does nothing when it is
/// clicked.
const ZOOM: usize = 0;
const SCALE_TOGGLE: usize = 1;
/// Everything from here is the `.r` group, right-aligned.
const RIGHT_FROM: usize = 2;
const PREVIOUS: usize = 2;
const NEXT: usize = 3;
const REVEAL: usize = 4;

struct Viewer {
    width: i32,
    height: i32,

    dir: String,
    candidates: Vec<String>,
    cur: usize,

    image: Option<Image>,
    bytes: usize,
    error: Option<String>,

    /// Fit the image to the window, or show it at 1:1.
    fit: bool,
    pan: (i32, i32),

    /// The scaled frame, cached so a repaint on hover does not resample the image.
    scaled: Vec<u32>,
    scaled_w: i32,
    scaled_h: i32,

    /// The bar is not drawn until the pointer is in the window, and `hover` is which control it is
    /// over. `None` for both is the resting state, which is the design's whole point.
    pointer_in: bool,
    hover: Option<usize>,

    ctx: String,
}

impl Viewer {
    fn new() -> Self {
        // Launched with a file — from Files, which execve's us with the path it was asked to open.
        let mut buf = [0u8; 512];
        let n = sys_launch_arg(&mut buf);
        let arg = core::str::from_utf8(&buf[..n]).unwrap_or("").trim();

        let (dir, want) = if arg.is_empty() {
            (String::from(BUNDLE), None)
        } else {
            (String::from(parent_of(arg)), Some(String::from(arg)))
        };

        let mut candidates = images_in(&dir);
        // The file we were handed must be in the list even if the directory walk missed it — a
        // viewer that opens something other than the file it was asked for is worse than one that
        // cannot list a directory.
        let cur = match &want {
            Some(w) => match candidates.iter().position(|c| c == w) {
                Some(i) => i,
                None => {
                    candidates.insert(0, w.clone());
                    0
                }
            },
            None => 0,
        };

        let mut app = Self {
            width: 672,
            height: 544,
            dir,
            candidates,
            cur,
            image: None,
            bytes: 0,
            error: None,
            fit: true,
            pan: (0, 0),
            scaled: Vec::new(),
            scaled_w: 0,
            scaled_h: 0,
            pointer_in: false,
            hover: None,
            ctx: String::new(),
        };
        app.load(cur);
        app
    }

    fn load(&mut self, i: usize) {
        if self.candidates.is_empty() {
            self.error = Some(String::from("no images here"));
            self.refresh_context();
            return;
        }
        self.cur = i % self.candidates.len();
        let path = self.candidates[self.cur].clone();
        let bytes = read_file_bytes(&path);
        self.bytes = bytes.len();
        if bytes.is_empty() {
            self.image = None;
            self.error = Some(alloc::format!("could not open {}", short_name(&path)));
        } else {
            match decode(&bytes) {
                Ok(img) => {
                    self.image = Some(img);
                    self.error = None;
                }
                Err(e) => {
                    self.image = None;
                    self.error = Some(alloc::format!("{}: {}", short_name(&path), e));
                }
            }
        }
        // A new image is a new fit: keeping the previous pan would drop the next picture at an
        // offset nobody asked for.
        self.pan = (0, 0);
        self.scaled.clear();
        self.scaled_w = 0;
        self.scaled_h = 0;
        self.refresh_context();
    }

    fn path(&self) -> &str {
        self.candidates.get(self.cur).map(|s| s.as_str()).unwrap_or("")
    }

    /// `apps / sample.png · 1280×720 · 214 KB` — the design's caption context.
    ///
    /// The parent directory's name leads, as the design's `captures / shelf-pack.png` does: in a
    /// viewer walking a folder, which folder is the thing the filename alone does not say.
    fn refresh_context(&mut self) {
        let path = String::from(self.path());
        let folder = short_name(parent_of(&path));
        let name = short_name(&path);
        self.ctx = match &self.image {
            Some(img) => alloc::format!(
                "{} / {} \u{00B7} {}\u{00D7}{} \u{00B7} {}",
                folder, name, img.w, img.h, human_size(self.bytes)
            ),
            None => alloc::format!("{} / {}", folder, name),
        };
    }

    fn surface(&self) -> Rect {
        Rect::new(0, 0, self.width, self.height)
    }

    /// The size the image is drawn at, and the frame it lands in.
    fn placement(&self) -> Option<(i32, i32, Rect)> {
        let img = self.image.as_ref()?;
        let (dw, dh) = viewer::drawn_size(self.surface(), img.w as i32, img.h as i32, self.fit);
        if dw <= 0 || dh <= 0 {
            return None;
        }
        Some((dw, dh, viewer::frame(self.surface(), dw, dh, self.pan)))
    }

    /// Resample into `scaled` if the target size changed. 1:1 needs no resample at all, and copying
    /// a 12-megapixel image through a nearest-neighbour loop to produce itself is the kind of work
    /// that shows up as the window taking a second to open.
    fn ensure_scaled(&mut self, dw: i32, dh: i32) {
        if dw == self.scaled_w && dh == self.scaled_h && !self.scaled.is_empty() {
            return;
        }
        let Some(img) = self.image.as_ref() else { return };
        self.scaled_w = dw;
        self.scaled_h = dh;
        if dw as usize == img.w && dh as usize == img.h {
            self.scaled = img.pixels.clone();
            return;
        }
        let (dw, dh) = (dw as usize, dh as usize);
        let mut out = vec![0u32; dw * dh];
        for y in 0..dh {
            let sy = y * img.h / dh;
            let (src, dst) = (sy * img.w, y * dw);
            for x in 0..dw {
                out[dst + x] = img.pixels[src + x * img.w / dw];
            }
        }
        self.scaled = out;
    }

    /// The bar's labels. The scale toggle names what clicking it **does**, not what state you are
    /// in — the design's static "Fit" beside a "100%" readout means exactly that, and saying it
    /// generalises to the fitted state where "Fit" would do nothing.
    fn labels(&self) -> [String; 5] {
        let zoom = match (&self.image, self.placement()) {
            (Some(img), Some((dw, _, _))) => {
                alloc::format!("{}%", viewer::zoom_pct(img.w as i32, dw))
            }
            _ => String::from("\u{2014}"),
        };
        [
            zoom,
            String::from(if self.fit { "Actual size" } else { "Fit" }),
            String::from("Previous"),
            String::from("Next"),
            String::from("Reveal in Files"),
        ]
    }

    fn widths(&self, labels: &[String; 5]) -> Vec<i32> {
        labels.iter().map(|s| text::width(B3, s)).collect()
    }

    fn clamp_pan(&mut self) {
        let Some((dw, dh, _)) = self.placement() else { return };
        let (lx, ly) = viewer::pan_limit(self.surface(), dw, dh);
        self.pan.0 = self.pan.0.clamp(-lx, lx);
        self.pan.1 = self.pan.1.clamp(-ly, ly);
    }

    fn activate(&mut self, item: usize) -> bool {
        match item {
            SCALE_TOGGLE => {
                self.fit = !self.fit;
                self.pan = (0, 0);
                true
            }
            PREVIOUS => {
                let n = self.candidates.len();
                if n > 1 {
                    self.load((self.cur + n - 1) % n);
                    return true;
                }
                false
            }
            NEXT => {
                let n = self.candidates.len();
                if n > 1 {
                    self.load((self.cur + 1) % n);
                    return true;
                }
                false
            }
            REVEAL => {
                // Fork and hand Files this image's directory. Bracketed exactly like Files' own
                // launch path: going quiet after a click is indistinguishable from a freeze from the
                // outside, and these writes prove the process can still enter the kernel.
                sys_print("[IMAGE] pre-fork\n");
                let pid = sys_fork();
                if pid == 0 {
                    sys_execve_arg("/mnt/nvme/apps/Explorer.nyx/run.bin", &self.dir);
                    sys_print("[IMAGE] child: execve returned\n");
                    sys_exit(1);
                }
                sys_print("[IMAGE] parent alive\n");
                false
            }
            _ => false,
        }
    }
}

impl NyxApp for Viewer {
    fn title(&self) -> &str {
        "Image"
    }
    fn icon_path(&self) -> &str {
        "/mnt/nvme/apps/ImageViewer.nyx/icon.png"
    }
    /// The design's own window: 672 wide, quantised to 16 like every width in the system.
    fn initial_width(&self) -> usize {
        672
    }
    fn initial_height(&self) -> usize {
        544
    }
    fn context(&self) -> &str {
        &self.ctx
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let t = theme();
        self.width = canvas.width as i32;
        self.height = canvas.height as i32;
        self.clamp_pan();

        // `.dark .iv { background: var(--sunken) }`. In the light theme the design leaves it on the
        // surface — an image sits better on a ground that does not compete with it, and there is no
        // checkerboard here because the design says so and because alpha is composited by the
        // window server, not faked underneath.
        let ground = if t.is_dark { t.sunken } else { t.surface };
        canvas.fill_rect(0, 0, canvas.width, canvas.height, ground);

        if let Some((dw, dh, f)) = self.placement() {
            self.ensure_scaled(dw, dh);
            // `composite_buffer` has no notion of a negative origin, so a panned image is blitted
            // from the visible sub-rectangle rather than from its top-left.
            let (sx, sy) = ((-f.x).max(0), (-f.y).max(0));
            let vis_w = (f.w - sx).min(self.width - f.x.max(0)).max(0);
            let vis_h = (f.h - sy).min(self.height - f.y.max(0)).max(0);
            if vis_w > 0 && vis_h > 0 && !self.scaled.is_empty() {
                let mut rows: Vec<u32> = Vec::with_capacity((vis_w * vis_h) as usize);
                for y in 0..vis_h {
                    let start = ((sy + y) * f.w + sx) as usize;
                    rows.extend_from_slice(&self.scaled[start..start + vis_w as usize]);
                }
                canvas.composite_buffer(
                    f.x.max(0) as usize, f.y.max(0) as usize,
                    &rows, vis_w as usize, vis_h as usize, 255,
                );
                // `.iv .frame { border: 1px solid var(--line); border-radius: 4px; overflow: hidden }`
                // — carve the corners back to the ground FIRST, then stroke the outline over the
                // result, or the border's own arc is drawn on top of square image pixels.
                if f.x >= 0 && f.y >= 0 && f.right() <= self.width && f.bottom() <= self.height {
                    shapes::carve_round_corners(
                        canvas, f.x as usize, f.y as usize, f.w as usize, f.h as usize,
                        viewer::frame_radius(), |_, _| ground,
                    );
                    shapes::stroke_round_rect(
                        canvas, f.x as usize, f.y as usize, f.w as usize, f.h as usize,
                        viewer::frame_radius(), t.line,
                    );
                }
            }
        } else if let Some(e) = &self.error {
            let w = text::width(B3, e);
            let y = text::centre_y(0, self.height, B3);
            text::draw(canvas, (self.width - w) / 2, y, B3, t.fg_3, e);
        }

        // The bar, and only while the pointer is here.
        if !self.pointer_in {
            return;
        }
        let bar = viewer::bar(self.surface());
        canvas.fill_rect(bar.x as usize, bar.y as usize, bar.w as usize, bar.h as usize, t.chrome);
        shapes::hairline_h(canvas, bar.x as usize, bar.y as usize, bar.w as usize, t.line);

        let labels = self.labels();
        let widths = self.widths(&labels);
        for (i, label) in labels.iter().enumerate() {
            let Some(r) = viewer::bar_item(self.surface(), &widths, RIGHT_FROM, i) else { continue };
            // `.z` is a readout, not a control: it is set a tier brighter and never highlights.
            let col = if i == ZOOM {
                t.fg_2
            } else if self.hover == Some(i) {
                t.fg
            } else {
                t.fg_3
            };
            text::draw(canvas, r.x, text::centre_y(r.y, r.h, B3), B3, col, label);
        }
    }

    /// Vertical panning through the window server's scrollbar. There is no horizontal equivalent —
    /// the arrow keys cover that axis.
    fn content_height(&self) -> usize {
        match self.placement() {
            Some((_, dh, _)) if dh > self.height => dh as usize,
            _ => 0,
        }
    }
    fn scroll_offset(&self) -> usize {
        let Some((_, dh, _)) = self.placement() else { return 0 };
        let (_, ly) = viewer::pan_limit(self.surface(), 0, dh);
        (ly - self.pan.1).max(0) as usize
    }
    fn on_scroll(&mut self, off: usize) -> bool {
        let Some((_, dh, _)) = self.placement() else { return false };
        let (_, ly) = viewer::pan_limit(self.surface(), 0, dh);
        let want = ly - off as i32;
        if want != self.pan.1 {
            self.pan.1 = want;
            self.clamp_pan();
            return true;
        }
        false
    }

    fn on_theme(&mut self, dark: bool) -> bool {
        DARK.swap(dark, core::sync::atomic::Ordering::Relaxed) != dark
    }

    fn on_mouse_move(&mut self, mx: usize, my: usize) -> bool {
        let labels = self.labels();
        let widths = self.widths(&labels);
        let new = viewer::bar_hit(self.surface(), &widths, RIGHT_FROM, mx as i32, my as i32);
        // The bar is revealed by the pointer being in the window at all, not by it being over the
        // bar — the design's "they appear on hover" is about the window, and a strip that only
        // appeared once you had already found it would be unusable.
        let changed = !self.pointer_in || new != self.hover;
        self.pointer_in = true;
        self.hover = new;
        changed
    }

    fn on_mouse_leave(&mut self) -> bool {
        let changed = self.pointer_in || self.hover.is_some();
        self.pointer_in = false;
        self.hover = None;
        changed
    }

    fn on_mouse(&mut self, mx: usize, my: usize, _clicked: bool) -> bool {
        let labels = self.labels();
        let widths = self.widths(&labels);
        // Clicking the image itself does NOTHING. The old viewer cycled on any click; a picture that
        // silently becomes a different picture when you click it is the interaction this design
        // replaced with two named controls.
        match viewer::bar_hit(self.surface(), &widths, RIGHT_FROM, mx as i32, my as i32) {
            Some(i) => self.activate(i),
            None => false,
        }
    }

    fn on_key(&mut self, key: char) -> bool {
        // Panning by key, because the protocol has no drag: `MSG_MOUSE_EVENT` is a click and
        // `MSG_MOUSE_MOVE` carries no button state, so a button-held move does not exist.
        let step = sc(64);
        match key {
            keys::LEFT if !self.fit => self.pan.0 += step,
            keys::RIGHT if !self.fit => self.pan.0 -= step,
            keys::UP if !self.fit => self.pan.1 += step,
            keys::DOWN if !self.fit => self.pan.1 -= step,
            // Fitted, there is nothing to pan, so the arrows walk the folder instead.
            keys::LEFT => return self.activate(PREVIOUS),
            keys::RIGHT => return self.activate(NEXT),
            ' ' => return self.activate(SCALE_TOGGLE),
            _ => return false,
        }
        self.clamp_pan();
        true
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // 4096 pages * 4 KiB = 16 MiB heap. A decode holds several full-frame buffers at once: the
    // decoder's own output, our packed u32 copy, and the scaled-to-window copy.
    let heap_start = sys_alloc_pages(4096);
    if heap_start == 0 {
        sys_exit(1);
    }
    unsafe {
        ALLOCATOR.lock().init(heap_start as *mut u8, 4096 * 4096);
    }

    // The three faces a ported interior sets type in. This window draws one 12px run per control and
    // nothing else, so it needs no `d1` and no monospace.
    nyx_meridian::font::register_text();
    nyx_gui::app::run(Viewer::new());
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(111);
}
