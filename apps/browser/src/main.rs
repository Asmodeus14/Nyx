// Nyx Browser — fetch, parse, style, lay out and paint a real web page.
//
// This is the top of the stack every earlier phase was built for: `nyx_net` for HTTPS, `nyx_web` for
// the DOM/cascade/layout, and `nyx_gui` for the window. Nothing here does any engine work itself —
// it owns the chrome (toolbar, loading UI, error dialog, scrolling, link hit-testing) and turns the
// engine's display list into `Canvas` calls.
//
// The single most important property of this file is that **it never blocks**. Nyx has no threads,
// so a blocking `nyx_net::get` of a 500 KB page reads to the user as a frozen machine, with no way
// to tell a slow server from a hung one. Every request is driven through `nyx_net::Fetch` a slice at
// a time, one poll per frame, which is what makes the progress bar, the spinner and the Stop button
// possible at all.
//
// A `target_os = "nyx"` std binary; built standalone via Build-std.sh like notepad/terminal.
#![feature(restricted_std)]

core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, rsp",
    "mov rdi, rsp",
    "and rsp, -16",
    "call __nyx_setup_main_tls",
    "mov rdi, [rbx]",
    "lea rsi, [rbx + 8]",
    "and rsp, -16",
    "call main",
    "mov edi, eax",
    "mov eax, 231",
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

use std::collections::HashMap;

use nyx_gui::app::NyxApp;
use nyx_gui::canvas::{Canvas, Color};
use nyx_net::{Fetch, Progress, Url};
use nyx_web::layout::{DisplayItem, FontMetrics, ImageSource, LayoutResult};
use nyx_web::style::ComputedStyle;
use nyx_web::{Dom, Stylesheet};

const BG: u32 = Color::WARM_BG;
const SURFACE: u32 = Color::WARM_SURFACE;
const BORDER: u32 = Color::WARM_BORDER;
const TEXT: u32 = Color::TEXT_DARK;
const MUTED: u32 = Color::TEXT_MUTED;
const ACCENT: u32 = Color::ACCENT_PRIMARY;
const PAGE_BG: u32 = 0xFF_FFFFFF;
const DANGER: u32 = 0xFF_C0392B;

const TOOLBAR_H: usize = 38;
const PROGRESS_H: usize = 3;
const STATUS_H: usize = 20;
const PAD: usize = 8;
const BTN: usize = 28;

// Subresource budgets. Every one of these is another round trip on a link that took real work to
// bring up, so the ceiling is set by patience, not by correctness.
const MAX_SHEETS: usize = 4;
const MAX_SHEET_BYTES: usize = 512 * 1024;
const MAX_IMAGES: usize = 12;
/// Total decoded pixels per page. A decoded image is 4 bytes a pixel, so this is an 8 MiB cap on
/// everything the page can make us hold at once — per-image limits alone would not bound the sum.
const MAX_TOTAL_PIXELS: usize = 2 * 1024 * 1024;

/// Text metrics from the real UI font. This is the bridge `nyx_web::layout` is written against —
/// the engine never learns what a font is, which is what keeps line breaking host-testable.
struct GuiFont;

/// CSS gives fractional pixel sizes; the rasterizer wants an integer height.
fn font_px(style: &ComputedStyle) -> usize {
    (style.font_size.round() as isize).max(6) as usize
}

impl FontMetrics for GuiFont {
    fn text_width(&self, text: &str, style: &ComputedStyle) -> f32 {
        let mut w = nyx_gui::font::text_width_px(text, font_px(style)) as f32;
        // Bold is faked by over-drawing one pixel to the right (there is only one face), so it
        // really is one pixel wider — measuring without that makes bold text overlap what follows.
        if style.bold {
            w += 1.0;
        }
        w
    }
    fn line_height(&self, style: &ComputedStyle) -> f32 {
        nyx_gui::font::line_height_px(font_px(style)) as f32
    }
}

/// Decoded images for the current page, keyed by **absolute** URL.
///
/// The display list carries `src` exactly as the document wrote it, so every lookup has to go
/// through the same `join` against the same base that filled the cache — resolving one way here and
/// another way at paint time is a miss that looks like a decode failure.
#[derive(Default)]
struct ImageCache {
    base: Option<Url>,
    map: HashMap<String, nyx_image::Image>,
}

impl ImageCache {
    fn key(&self, src: &str) -> Option<String> {
        Some(self.base.as_ref()?.join(src).ok()?.to_string())
    }
    fn get(&self, src: &str) -> Option<&nyx_image::Image> {
        self.map.get(&self.key(src)?)
    }
    fn pixels_used(&self) -> usize {
        self.map.values().map(|i| i.w * i.h).sum()
    }
}

impl ImageSource for ImageCache {
    fn intrinsic_size(&self, src: &str) -> Option<(f32, f32)> {
        let img = self.get(src)?;
        Some((img.w as f32, img.h as f32))
    }
}

/// One slot in the page's stylesheet list. Linked sheets start as `Link` and become `Text` when they
/// arrive, so the CSS can be concatenated in **document order** whatever order it lands in — source
/// order breaks specificity ties in the cascade, so appending as-they-arrive would be wrong.
enum CssSource {
    Text(String),
    Link(String),
}

/// Why a page is being loaded, which decides what happens to the history stacks.
#[derive(Clone, Copy, PartialEq)]
enum Nav {
    New,
    Back,
    Forward,
    Reload,
}

/// Which part of the page load is in flight.
enum Stage {
    Idle,
    Document,
    Sheets,
    Images,
}

/// The whole loading state machine. One `Fetch` is in flight at a time; `poll` advances it by one
/// frame's worth of work and everything else here exists so the toolbar can describe what is
/// happening while it does.
struct Loader {
    stage: Stage,
    fetch: Option<Fetch>,
    nav: Nav,
    /// What to show next to the spinner.
    label: String,
    got: usize,
    total: Option<usize>,
    /// Frame counter, so an indeterminate bar can still move.
    tick: usize,

    base: Option<Url>,
    sources: Vec<CssSource>,
    next_source: usize,
    images: Vec<String>,
    images_total: usize,
    /// Subresource failures in a row. If the link has gone away there is nothing to be gained by
    /// opening another eleven connections to prove it, and plenty to lose: every attempt is a DNS
    /// lookup and a TCP connect that the kernel has to unwind.
    failures: usize,
}

/// How many subresources may fail back-to-back before the rest of the page's are abandoned.
const MAX_CONSECUTIVE_FAILURES: usize = 3;

impl Default for Loader {
    fn default() -> Self {
        Loader {
            stage: Stage::Idle,
            fetch: None,
            nav: Nav::New,
            label: String::new(),
            got: 0,
            total: None,
            tick: 0,
            base: None,
            sources: Vec::new(),
            next_source: 0,
            images: Vec::new(),
            images_total: 0,
            failures: 0,
        }
    }
}

impl Loader {
    fn busy(&self) -> bool {
        !matches!(self.stage, Stage::Idle)
    }
}

/// A modal the user has to acknowledge. Errors that stop a page from rendering deserve more than a
/// line of grey text in the status bar, which is easy to miss and easy to mistake for a slow load.
struct Dialog {
    title: String,
    detail: String,
    /// The URL to retry, when retrying makes sense.
    retry: Option<String>,
}

struct Browser {
    /// What the address bar shows / what we will load.
    address: String,
    /// The URL the current page actually came from, after redirects. Relative links resolve here.
    current: String,
    html: String,
    /// Parsed once per load and kept. Re-parsing on every resize would run html5ever — by far the
    /// most expensive step here — for a change that cannot affect the tree.
    dom: Dom,
    /// The cascade result, likewise load-scoped: computed styles depend on the document and the
    /// stylesheets, never on the viewport width. Only layout does.
    styles: nyx_web::style::StyleTree,
    images: ImageCache,
    page: LayoutResult,
    title: String,
    status: String,
    scroll: usize,
    history: Vec<String>,
    forward: Vec<String>,
    loader: Loader,
    dialog: Option<Dialog>,
    /// Width the current layout was computed at; a resize forces a re-layout.
    laid_out_at: usize,
    viewport_w: usize,
    viewport_h: usize,
    address_focused: bool,
}

impl Browser {
    fn new() -> Self {
        // A real (if empty) document, so `styles` is always index-compatible with `dom` — a layout
        // against a mismatched pair would panic rather than draw nothing.
        let dom = Dom::parse("");
        let styles = nyx_web::style::compute(&dom, &Stylesheet::parse(""));
        Browser {
            address: String::from("http://example.com"),
            current: String::new(),
            html: String::new(),
            dom,
            styles,
            images: ImageCache::default(),
            page: LayoutResult::default(),
            title: String::from("Nyx Browser"),
            status: String::from("Type a URL and press Enter."),
            scroll: 0,
            history: Vec::new(),
            forward: Vec::new(),
            loader: Loader::default(),
            dialog: None,
            laid_out_at: 0,
            viewport_w: 900,
            viewport_h: 650,
            address_focused: true,
        }
    }

    fn content_w(&self) -> usize {
        self.viewport_w.saturating_sub(PAD * 2)
    }

    /// The progress strip is always reserved, even when idle. Growing the chrome only while loading
    /// would shift the whole page down three pixels the moment a load starts and back up when it
    /// ends — a visible twitch on every navigation, for nothing.
    fn content_top(&self) -> usize {
        TOOLBAR_H + PROGRESS_H
    }

    fn content_h(&self) -> usize {
        self.viewport_h.saturating_sub(self.content_top() + STATUS_H)
    }

    // -----------------------------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------------------------

    /// Begin a navigation. Opens nothing — the first `poll` does that, one frame later, so the
    /// "Connecting…" state is on screen before any blocking work happens.
    fn navigate(&mut self, url: &str, nav: Nav) {
        let url = url.trim().to_string();
        if url.is_empty() {
            return;
        }
        let url = if url.contains("://") { url } else { format!("http://{url}") };

        match Fetch::new(&url) {
            Ok(fetch) => {
                // `label` is the *subject* — the phase supplies the verb.
                self.loader = Loader {
                    stage: Stage::Document,
                    fetch: Some(fetch),
                    nav,
                    label: host_of(&url),
                    ..Loader::default()
                };
                self.address = url;
                self.status = format!("Looking up {}…", self.loader.label);
                self.dialog = None;
            }
            Err(e) => self.fail(&url, "That is not a URL", &e.to_string(), false),
        }
    }

    fn stop(&mut self) {
        if self.loader.busy() {
            self.loader = Loader::default();
            self.status = String::from("Stopped.");
        }
    }

    /// Give up on a page and say so in a way that cannot be missed.
    fn fail(&mut self, url: &str, title: &str, detail: &str, retryable: bool) {
        self.status = format!("{title}: {detail}");
        self.dialog = Some(Dialog {
            title: title.to_string(),
            detail: detail.to_string(),
            retry: retryable.then(|| url.to_string()),
        });
        // Also render it as a page, so the window does not keep showing whatever was there before —
        // stale content under a dismissed dialog is how a user ends up trusting the wrong thing.
        self.show_document(
            &format!(
                "<html><head><title>{}</title></head><body><div class=e>\
                 <h1>{}</h1><p class=d>{}</p><p class=u>{}</p></div></body></html>",
                escape(title),
                escape(title),
                escape(detail),
                escape(url)
            ),
            ERROR_CSS,
            None,
        );
        self.loader = Loader::default();
    }

    /// Install a document that needs no network: parse, cascade, lay out.
    fn show_document(&mut self, html: &str, css: &str, base: Option<Url>) {
        self.html = html.to_string();
        self.dom = Dom::parse(&self.html);
        self.styles = nyx_web::style::compute(&self.dom, &Stylesheet::parse(css));
        self.images = ImageCache { base, map: HashMap::new() };
        self.scroll = 0;
        self.relayout();
    }

    // -----------------------------------------------------------------------------------------
    // The load state machine — one step per frame
    // -----------------------------------------------------------------------------------------

    fn poll_load(&mut self) -> bool {
        if !self.loader.busy() {
            return false;
        }
        self.loader.tick += 1;

        let Some(fetch) = self.loader.fetch.as_mut() else {
            // No request in flight for this stage: pick the next thing to ask for.
            return self.advance();
        };

        match fetch.poll() {
            // Name the exact step. "Connecting…" covering DNS, TCP, TLS and the wait for headers
            // meant a stall in any of four unrelated places looked identical from the outside.
            Progress::Connecting(phase) => {
                self.status = format!("{} {}…", phase.label(), self.loader.label);
                true
            }
            Progress::Receiving { got, total } => {
                self.loader.got = got;
                self.loader.total = total;
                self.status = match total {
                    Some(t) => format!("{} {} of {}", self.loader.label, kb(got), kb(t)),
                    None => format!("{} {}", self.loader.label, kb(got)),
                };
                true
            }
            Progress::Done(resp) => {
                self.loader.fetch = None;
                self.finished(*resp);
                true
            }
            Progress::Failed(e) => {
                self.loader.fetch = None;
                let message = e.to_string();
                match self.loader.stage {
                    // Only the document failing stops the page. A stylesheet or an image that will
                    // not load leaves a worse-looking page, not a broken one.
                    Stage::Document => {
                        let url = self.address.clone();
                        self.fail(&url, "Could not load this page", &message, true);
                    }
                    // Blank the slot and step past it. Leaving it as a `Link` would make `advance`
                    // pick the same sheet again on the next frame, forever.
                    Stage::Sheets => {
                        let i = self.loader.next_source;
                        if let Some(slot) = self.loader.sources.get_mut(i) {
                            *slot = CssSource::Text(String::new());
                        }
                        self.loader.next_source += 1;
                        self.give_up_on_subresources(&message);
                    }
                    // The image was already popped off the queue, so there is nothing to skip.
                    Stage::Images => self.give_up_on_subresources(&message),
                    Stage::Idle => {}
                }
                true
            }
        }
    }

    /// Count a subresource failure, and stop trying once the link has clearly gone.
    ///
    /// The document is already on screen at this point, so abandoning the extras costs styling and
    /// pictures, not the page. Continuing costs another connect per item and gets the same answer.
    fn give_up_on_subresources(&mut self, why: &str) {
        self.loader.failures += 1;
        if self.loader.failures < MAX_CONSECUTIVE_FAILURES {
            return;
        }
        // Apply whatever CSS did arrive before dropping the queue — `Loader::default()` takes the
        // sources with it, and a page styled by two of its four sheets beats one styled by none.
        if matches!(self.loader.stage, Stage::Sheets) {
            self.recompute_styles();
        }
        self.loader = Loader::default();
        self.status = format!("{} · stopped loading extras: {why}", self.title);
    }

    /// A request completed. Fold it into the page and decide what to ask for next.
    fn finished(&mut self, resp: nyx_net::Response) {
        // Anything arriving means the link is alive; a later failure starts the count afresh.
        self.loader.failures = 0;
        match self.loader.stage {
            Stage::Idle => {}
            Stage::Document => {
                if resp.status >= 400 {
                    // The body of an error page is often useful, so render it — but say plainly
                    // that the server refused, rather than letting a styled 404 look like success.
                    self.status = format!("Server returned {}", resp.status);
                }
                let body = resp.text();
                // A non-HTML document still deserves to be readable rather than rendered as if its
                // markup meant something.
                let is_html = resp
                    .content_type()
                    .map(|c| c.to_ascii_lowercase().contains("html"))
                    .unwrap_or(true);
                self.html = if is_html { body } else { format!("<pre>{}</pre>", escape(&body)) };

                let final_url = resp.url.to_string();
                match self.loader.nav {
                    Nav::New => {
                        if !self.current.is_empty() {
                            self.history.push(self.current.clone());
                        }
                        self.forward.clear();
                    }
                    Nav::Back | Nav::Forward | Nav::Reload => {}
                }
                self.current = final_url.clone();
                self.address = final_url;
                self.scroll = 0;

                self.dom = Dom::parse(&self.html);
                self.title = self.dom.title().unwrap_or_else(|| self.current.clone());
                self.loader.base = Some(resp.url.clone());

                self.loader.sources = self.css_sources();
                self.loader.next_source = 0;
                self.loader.stage = Stage::Sheets;
                // Show the page immediately, unstyled by anything linked. Waiting for the sheets
                // before the first paint is what makes a browser feel slow even when it is not.
                self.recompute_styles();
            }
            Stage::Sheets => {
                let i = self.loader.next_source;
                let ok = resp.status == 200 && resp.body.len() <= MAX_SHEET_BYTES;
                if let Some(slot) = self.loader.sources.get_mut(i) {
                    *slot = CssSource::Text(if ok { resp.text() } else { String::new() });
                }
                self.loader.next_source += 1;
            }
            Stage::Images => {
                if resp.status == 200 {
                    // Decoders are BMP/TGA/PNG/JPEG only; anything else — SVG, WebP, or an error
                    // page served as HTML — fails here and the element keeps its alt text.
                    if let Ok(img) = nyx_image::decode(&resp.body) {
                        if self.images.pixels_used() + img.w * img.h <= MAX_TOTAL_PIXELS {
                            self.images.map.insert(resp.url.to_string(), img);
                            // The image was standing in as alt text; its real box changes the line
                            // breaks around it, so the page has to be laid out again.
                            self.relayout();
                            self.scroll = self.scroll.min(self.max_scroll());
                        }
                    }
                }
            }
        }
    }

    /// Start the next request for the current stage, or move to the stage after it.
    fn advance(&mut self) -> bool {
        match self.loader.stage {
            Stage::Idle => false,
            Stage::Document => {
                // Only reached if a document fetch was dropped; nothing left to do.
                self.loader = Loader::default();
                true
            }
            Stage::Sheets => {
                while self.loader.next_source < self.loader.sources.len() {
                    let i = self.loader.next_source;
                    if let CssSource::Link(url) = &self.loader.sources[i] {
                        let url = url.clone();
                        match Fetch::new(&url) {
                            Ok(f) => {
                                self.loader.fetch = Some(f);
                                self.loader.label = String::from("stylesheet");
                                return true;
                            }
                            // Unfetchable: blank the slot and carry on down the list.
                            Err(_) => {
                                self.loader.sources[i] = CssSource::Text(String::new());
                                self.loader.next_source += 1;
                                continue;
                            }
                        }
                    }
                    self.loader.next_source += 1;
                }
                // Every sheet accounted for: re-cascade with the full set, then go for images.
                self.recompute_styles();
                self.loader.images = self.image_queue();
                self.loader.images_total = self.loader.images.len();
                self.loader.stage = Stage::Images;
                true
            }
            Stage::Images => {
                while let Some(url) = self.loader.images.pop() {
                    if let Ok(f) = Fetch::new(&url) {
                        self.loader.fetch = Some(f);
                        let done = self.loader.images_total - self.loader.images.len();
                        self.loader.label = format!("image {done} of {}", self.loader.images_total);
                        return true;
                    }
                }
                self.loader = Loader::default();
                self.status = format!(
                    "{} · {} images · {} links",
                    self.title,
                    self.images.map.len(),
                    self.page.links.len()
                );
                true
            }
        }
    }

    /// The page's CSS slots in document order: every inline `<style>`, and a placeholder for every
    /// `<link rel=stylesheet>` worth fetching.
    fn css_sources(&self) -> Vec<CssSource> {
        let mut sources = Vec::new();
        let mut links = 0usize;
        for id in self.dom.descendants(self.dom.root) {
            let node = self.dom.node(id);
            match node.tag() {
                Some("style") => sources.push(CssSource::Text(self.dom.text_content(id))),
                Some("link") if links < MAX_SHEETS => {
                    // `rel` is a space-separated token list, so "alternate stylesheet" and friends
                    // have to be matched by token, not by whole-string equality.
                    let is_sheet = node
                        .attr("rel")
                        .map(|r| {
                            r.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet"))
                        })
                        .unwrap_or(false);
                    let href = node.attr("href").filter(|h| !h.is_empty());
                    if let (true, Some(href), Some(base)) = (is_sheet, href, self.loader.base.as_ref())
                    {
                        if let Ok(url) = base.join(href) {
                            sources.push(CssSource::Link(url.to_string()));
                            links += 1;
                        }
                    }
                }
                _ => {}
            }
        }
        sources
    }

    /// Re-run the cascade over whatever CSS has arrived so far.
    fn recompute_styles(&mut self) {
        let mut css = String::new();
        for source in &self.loader.sources {
            if let CssSource::Text(text) = source {
                css.push_str(text);
                css.push('\n');
            }
        }
        self.styles = nyx_web::style::compute(&self.dom, &Stylesheet::parse(&css));
        self.relayout();
    }

    /// Reset the image cache and list the absolute URLs worth fetching, ordered so that `pop()`
    /// yields them in document order — what is at the top of the page appears first.
    fn image_queue(&mut self) -> Vec<String> {
        self.images = ImageCache { base: self.loader.base.clone(), map: HashMap::new() };

        let mut queue: Vec<String> = Vec::new();
        for id in self.dom.descendants(self.dom.root) {
            if queue.len() >= MAX_IMAGES {
                break;
            }
            if self.dom.node(id).tag() != Some("img") {
                continue;
            }
            let Some(src) = self.dom.node(id).attr("src").filter(|s| !s.is_empty()) else {
                continue;
            };
            let Some(key) = self.images.key(src) else { continue };
            // The same picture used ten times is one fetch.
            if !queue.contains(&key) {
                queue.push(key);
            }
        }
        queue.reverse();
        queue
    }

    /// Lay the current document out at the current viewport width. The DOM, the cascade and the
    /// images are all already in hand, so this is cheap enough to run on every resize.
    fn relayout(&mut self) {
        let width = self.content_w() as f32;
        self.page = nyx_web::layout::layout(&self.dom, &self.styles, &GuiFont, &self.images, width);
        self.laid_out_at = self.viewport_w;
    }

    fn max_scroll(&self) -> usize {
        (self.page.height as usize).saturating_sub(self.content_h())
    }

    // --- chrome geometry, in one place so hit-testing and drawing cannot disagree ---
    fn back_rect(&self) -> Rect {
        (PAD, 5, BTN, BTN)
    }
    fn forward_rect(&self) -> Rect {
        (PAD + BTN + 4, 5, BTN, BTN)
    }
    fn reload_rect(&self) -> Rect {
        (PAD + (BTN + 4) * 2, 5, BTN, BTN)
    }
    fn address_rect(&self) -> Rect {
        let x = PAD + (BTN + 4) * 3 + 4;
        (x, 5, self.viewport_w.saturating_sub(x + 56), BTN)
    }
    fn go_rect(&self) -> Rect {
        (self.viewport_w.saturating_sub(48), 5, 40, BTN)
    }
    /// Centred, and sized so a long URL still fits on two lines.
    fn dialog_rect(&self) -> Rect {
        let w = 460.min(self.viewport_w.saturating_sub(40));
        let h = 168;
        ((self.viewport_w.saturating_sub(w)) / 2, (self.viewport_h.saturating_sub(h)) / 3, w, h)
    }
    fn dialog_retry_rect(&self) -> Rect {
        let (x, y, w, h) = self.dialog_rect();
        (x + w - 190, y + h - 44, 84, 30)
    }
    fn dialog_close_rect(&self) -> Rect {
        let (x, y, w, h) = self.dialog_rect();
        (x + w - 98, y + h - 44, 84, 30)
    }
}

type Rect = (usize, usize, usize, usize);

impl NyxApp for Browser {
    fn title(&self) -> &str {
        &self.title
    }
    fn initial_width(&self) -> usize {
        900
    }
    fn initial_height(&self) -> usize {
        650
    }
    fn icon_path(&self) -> &str {
        "/mnt/nvme/apps/Browser.nyx/icon.png"
    }

    fn update(&mut self) -> bool {
        self.poll_load()
    }

    fn content_height(&self) -> usize {
        self.page.height as usize + self.content_top() + STATUS_H
    }
    fn scroll_offset(&self) -> usize {
        self.scroll
    }
    fn on_scroll(&mut self, new_offset: usize) -> bool {
        self.scroll = new_offset.min(self.max_scroll());
        true
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let w = canvas.width;
        let h = canvas.height;
        if w != self.viewport_w || h != self.viewport_h {
            self.viewport_w = w;
            self.viewport_h = h;
        }
        // A resize changes the line breaks, so the page has to be laid out again.
        if !self.html.is_empty() && self.laid_out_at != self.viewport_w {
            self.relayout();
            self.scroll = self.scroll.min(self.max_scroll());
        }

        // Page first, chrome on top: that is what keeps content from painting over the toolbar
        // without needing a clip rect the Canvas does not have.
        let top = self.content_top();
        canvas.fill_rect(0, top, w, h.saturating_sub(top + STATUS_H), PAGE_BG);
        self.paint_page(canvas);
        self.paint_chrome(canvas);
        if self.dialog.is_some() {
            self.paint_dialog(canvas);
        }
    }

    fn on_key(&mut self, key: char) -> bool {
        // Escape is the universal "get me out of this": dismiss a dialog, else stop loading.
        if key == '\u{1b}' {
            if self.dialog.take().is_some() {
                return true;
            }
            self.stop();
            return true;
        }
        if !self.address_focused {
            return false;
        }
        match key {
            '\n' | '\r' => {
                let url = self.address.clone();
                self.navigate(&url, Nav::New);
            }
            '\u{8}' | '\u{7f}' => {
                self.address.pop();
            }
            c if (c as u32) >= 0xE000 && (c as u32) <= 0xF8FF => return false, // PUA nav keys
            c if !c.is_control() => self.address.push(c),
            _ => return false,
        }
        true
    }

    fn on_mouse(&mut self, mx: usize, my: usize, clicked: bool) -> bool {
        if !clicked {
            return false;
        }

        // The dialog is modal, so it is hit-tested before anything else — and it swallows clicks
        // that miss its buttons, or the page underneath would react to a click aimed at the modal.
        if self.dialog.is_some() {
            if hit(self.dialog_close_rect(), mx, my) {
                self.dialog = None;
            } else if hit(self.dialog_retry_rect(), mx, my) {
                if let Some(url) = self.dialog.as_ref().and_then(|d| d.retry.clone()) {
                    self.dialog = None;
                    self.navigate(&url, Nav::Reload);
                }
            }
            return true;
        }

        if hit(self.back_rect(), mx, my) {
            if let Some(prev) = self.history.pop() {
                if !self.current.is_empty() {
                    self.forward.push(self.current.clone());
                }
                self.navigate(&prev, Nav::Back);
            }
            return true;
        }
        if hit(self.forward_rect(), mx, my) {
            if let Some(next) = self.forward.pop() {
                if !self.current.is_empty() {
                    self.history.push(self.current.clone());
                }
                self.navigate(&next, Nav::Forward);
            }
            return true;
        }
        if hit(self.reload_rect(), mx, my) {
            // The same button is Stop while a load is running — that is the only moment a user
            // wants it, and a separate control would sit dead the rest of the time.
            if self.loader.busy() {
                self.stop();
            } else if !self.current.is_empty() {
                let url = self.current.clone();
                self.navigate(&url, Nav::Reload);
            }
            return true;
        }
        if hit(self.go_rect(), mx, my) {
            let url = self.address.clone();
            self.navigate(&url, Nav::New);
            return true;
        }
        if hit(self.address_rect(), mx, my) {
            self.address_focused = true;
            return true;
        }

        // Link hit-testing, in page coordinates.
        let top = self.content_top();
        if my >= top && my < self.viewport_h.saturating_sub(STATUS_H) {
            self.address_focused = false;
            let px = mx.saturating_sub(PAD) as f32;
            let py = (my - top + self.scroll) as f32;
            let hit_href = self
                .page
                .links
                .iter()
                .find(|l| px >= l.x && px < l.x + l.w && py >= l.y && py < l.y + l.h)
                .map(|l| l.href.clone());

            if let Some(href) = hit_href {
                // Resolve against the page we are ON, not what is typed in the address bar.
                let base = if self.current.is_empty() { &self.address } else { &self.current };
                match Url::parse(base).and_then(|b| b.join(&href)) {
                    Ok(target) => {
                        let t = target.to_string();
                        self.navigate(&t, Nav::New);
                    }
                    Err(e) => self.status = format!("Bad link {href:?}: {e}"),
                }
                return true;
            }
            return true;
        }
        false
    }
}

impl Browser {
    fn paint_page(&self, canvas: &mut Canvas) {
        let top = self.content_top() as i32;
        let bottom = self.viewport_h.saturating_sub(STATUS_H) as i32;

        for item in &self.page.items {
            match item {
                DisplayItem::Rect { x, y, w, h, color } => {
                    let sy = *y as i32 - self.scroll as i32 + top;
                    let sh = *h as i32;
                    if sy + sh <= top || sy >= bottom || *color >> 24 == 0 {
                        continue;
                    }
                    // Clamp into the content area instead of relying on a clip rect.
                    let cy = sy.max(top);
                    let ch = (sy + sh).min(bottom) - cy;
                    if ch <= 0 {
                        continue;
                    }
                    canvas.fill_rect(
                        (*x as i32 + PAD as i32).max(0) as usize,
                        cy as usize,
                        *w as usize,
                        ch as usize,
                        *color,
                    );
                }
                DisplayItem::Text { x, y, text, color, font_size, bold, underline, .. } => {
                    let px = (font_size.round() as isize).max(6) as usize;
                    let lh = nyx_gui::font::line_height_px(px) as i32;
                    let sy = *y as i32 - self.scroll as i32 + top;
                    // Whole-line cull. Partially visible lines are dropped rather than clipped —
                    // the alternative is glyphs bleeding into the toolbar.
                    if sy < top || sy + lh > bottom {
                        continue;
                    }
                    let mut cx = *x as i32 + PAD as i32;
                    for ch in text.chars() {
                        if cx >= self.viewport_w as i32 {
                            break;
                        }
                        if cx >= 0 {
                            canvas.draw_char_px(cx as usize, sy as usize, ch, *color, px);
                            if *bold {
                                // No bold face exists; over-draw one pixel across to thicken.
                                canvas.draw_char_px(cx as usize + 1, sy as usize, ch, *color, px);
                            }
                        }
                        cx += nyx_gui::font::advance_px(ch, px) as i32 + *bold as i32;
                    }
                    if *underline {
                        let uy = sy + nyx_gui::font::ascent_px(px) as i32 + 1;
                        if uy < bottom {
                            let start = (*x as i32 + PAD as i32).max(0) as usize;
                            let width = (cx - *x as i32 - PAD as i32).max(0) as usize;
                            canvas.fill_rect(start, uy as usize, width, 1, *color);
                        }
                    }
                }
                DisplayItem::Image { x, y, w, h, src } => {
                    let (dw, dh) = (w.round() as i32, h.round() as i32);
                    let sy = *y as i32 - self.scroll as i32 + top;
                    if dw <= 0 || dh <= 0 || sy + dh <= top || sy >= bottom {
                        continue;
                    }
                    let Some(img) = self.images.get(src) else { continue };

                    // Row-level clipping. Unlike text, an image is usually taller than the viewport
                    // is spare, so culling a partly visible one would mean never seeing it at all.
                    let skip = (top - sy).max(0);
                    let visible = (sy + dh).min(bottom) - (sy + skip);
                    if visible <= 0 {
                        continue;
                    }

                    let (dw, dh) = (dw as usize, dh as usize);
                    let scaled;
                    let px: &[u32] = if img.w == dw && img.h == dh {
                        &img.pixels
                    } else {
                        scaled = Canvas::scale_rgba(&img.pixels, img.w, img.h, dw, dh);
                        &scaled
                    };
                    canvas.blit_rgba(
                        (*x as i32 + PAD as i32).max(0) as usize,
                        (sy + skip) as usize,
                        &px[skip as usize * dw..],
                        dw,
                        visible as usize,
                        None,
                    );
                }
            }
        }
    }

    fn paint_chrome(&self, canvas: &mut Canvas) {
        let w = self.viewport_w;
        let h = self.viewport_h;

        canvas.fill_rect(0, 0, w, TOOLBAR_H, BG);
        canvas.fill_rect(0, TOOLBAR_H - 1, w, 1, BORDER);

        let button = |canvas: &mut Canvas, r: Rect, glyph: char, enabled: bool| {
            canvas.fill_rect(r.0, r.1, r.2, r.3, SURFACE);
            canvas.draw_char_centered(r.0, r.1, r.2, r.3, glyph, if enabled { TEXT } else { MUTED }, 1);
        };
        button(canvas, self.back_rect(), '\u{2190}', !self.history.is_empty());
        button(canvas, self.forward_rect(), '\u{2192}', !self.forward.is_empty());
        // Reload doubles as Stop while a load is in flight.
        let (glyph, on) = if self.loader.busy() {
            ('\u{2715}', true)
        } else {
            ('\u{21bb}', !self.current.is_empty())
        };
        button(canvas, self.reload_rect(), glyph, on);

        let (ax, ay, aw, ah) = self.address_rect();
        canvas.fill_rect(ax, ay, aw, ah, SURFACE);
        if self.address_focused {
            canvas.fill_rect(ax, ay + ah - 1, aw, 1, ACCENT);
        }
        // A spinner inside the address bar, where the site icon would be: it is the one place the
        // eye is already looking while waiting for a page.
        let mut text_x = ax + 6;
        if self.loader.busy() {
            const SPINNER: [char; 4] = ['\u{2596}', '\u{2598}', '\u{259D}', '\u{2597}'];
            let frame = SPINNER[(self.loader.tick / 3) % SPINNER.len()];
            canvas.draw_char_centered(ax + 2, ay, 16, ah, frame, ACCENT, 1);
            text_x = ax + 20;
        }
        // Show the tail of a long URL — the end is what changes between pages.
        let room = aw.saturating_sub(text_x - ax + 6);
        let shown = fit_tail(&self.address, room);
        canvas.print_str(text_x, ay + 6, &shown, TEXT, 1);

        let (gx, gy, gw, gh) = self.go_rect();
        canvas.fill_rect(gx, gy, gw, gh, ACCENT);
        canvas.draw_char_centered(gx, gy, gw, gh, '\u{2192}', 0xFF_FFFFFF, 1);

        // Progress bar, immediately under the toolbar. A determinate bar when the server told us how
        // big the body is; otherwise a segment that sweeps, which says "working" without lying about
        // how far along it is.
        let y = TOOLBAR_H;
        if !self.loader.busy() {
            canvas.fill_rect(0, y, w, PROGRESS_H, BG);
        } else {
            canvas.fill_rect(0, y, w, PROGRESS_H, BORDER);
            match self.loader.total {
                Some(total) if total > 0 => {
                    let filled = (self.loader.got.min(total) * w) / total;
                    canvas.fill_rect(0, y, filled, PROGRESS_H, ACCENT);
                }
                _ => {
                    let span = w / 5;
                    let travel = w + span;
                    let pos = (self.loader.tick * 6) % travel;
                    let x = pos.saturating_sub(span);
                    let end = (pos).min(w);
                    canvas.fill_rect(x, y, end.saturating_sub(x), PROGRESS_H, ACCENT);
                }
            }
        }

        let sy = h.saturating_sub(STATUS_H);
        canvas.fill_rect(0, sy, w, STATUS_H, BG);
        canvas.fill_rect(0, sy, w, 1, BORDER);
        let status = fit_tail(&self.status, w.saturating_sub(PAD * 2));
        canvas.print_str(PAD, sy + 3, &status, MUTED, 1);
    }

    fn paint_dialog(&self, canvas: &mut Canvas) {
        let Some(dialog) = &self.dialog else { return };
        let (x, y, w, h) = self.dialog_rect();

        // Dim the page behind it, so it reads as modal rather than as another panel.
        canvas.fill_rect(0, self.content_top(), self.viewport_w,
            self.viewport_h.saturating_sub(self.content_top() + STATUS_H), 0x60_000000);

        canvas.fill_rect(x - 1, y - 1, w + 2, h + 2, BORDER);
        canvas.fill_rect(x, y, w, h, BG);
        // A red spine, so the nature of the box is legible before a single word is read.
        canvas.fill_rect(x, y, 4, h, DANGER);

        canvas.print_str(x + 18, y + 16, &dialog.title, DANGER, 1);
        let inner = w.saturating_sub(36);
        let mut ty = y + 44;
        for line in wrap(&dialog.detail, inner, 3) {
            canvas.print_str(x + 18, ty, &line, TEXT, 1);
            ty += 18;
        }

        if dialog.retry.is_some() {
            let r = self.dialog_retry_rect();
            canvas.fill_rect(r.0, r.1, r.2, r.3, SURFACE);
            canvas.draw_char_centered(r.0, r.1, r.2, r.3, ' ', TEXT, 1);
            center_text(canvas, r, "Retry", TEXT);
        }
        let c = self.dialog_close_rect();
        canvas.fill_rect(c.0, c.1, c.2, c.3, ACCENT);
        center_text(canvas, c, "Dismiss", 0xFF_FFFFFF);
    }
}

fn center_text(canvas: &mut Canvas, r: Rect, text: &str, color: u32) {
    let tw = nyx_gui::font::text_width_px(text, nyx_gui::font::BASE_PX);
    let x = r.0 + r.2.saturating_sub(tw) / 2;
    let y = r.1 + r.3.saturating_sub(nyx_gui::font::line_height_px(nyx_gui::font::BASE_PX)) / 2;
    canvas.print_str(x, y, text, color, 1);
}

fn hit(r: Rect, mx: usize, my: usize) -> bool {
    mx >= r.0 && mx < r.0 + r.2 && my >= r.1 && my < r.1 + r.3
}

/// Trim `text` from the LEFT until it fits `width` pixels. URLs and error messages both put the
/// part that changes at the end, so the tail is what is worth keeping.
fn fit_tail(text: &str, width: usize) -> String {
    let px = nyx_gui::font::BASE_PX;
    if nyx_gui::font::text_width_px(text, px) <= width {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        let candidate: String = chars.iter().collect();
        if nyx_gui::font::text_width_px(&candidate, px) + 12 <= width {
            return format!("…{candidate}");
        }
        chars.remove(0);
    }
    String::new()
}

/// Greedy word wrap to at most `max_lines`, for dialog text.
fn wrap(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let px = nyx_gui::font::BASE_PX;
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate =
            if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if nyx_gui::font::text_width_px(&candidate, px) <= width {
            line = candidate;
        } else {
            if !line.is_empty() {
                lines.push(core::mem::take(&mut line));
            }
            line = word.to_string();
            if lines.len() == max_lines {
                break;
            }
        }
    }
    if !line.is_empty() && lines.len() < max_lines {
        lines.push(line);
    }
    lines
}

fn host_of(url: &str) -> String {
    Url::parse(url).map(|u| u.host).unwrap_or_else(|_| url.to_string())
}

fn kb(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else {
        format!("{} KB", n / 1024)
    }
}

/// Make arbitrary text safe to drop into markup.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Styling for the built-in error page. Kept separate from the UA sheet so a site can never
/// accidentally restyle an error into looking like content.
const ERROR_CSS: &str = "
body { margin: 48px 40px; color: #333333 }
.e { border: 1px solid #dddddd; padding: 28px }
h1 { font-size: 24px; color: #c0392b; margin: 0 0 12px 0 }
.d { font-size: 16px; margin: 0 0 18px 0 }
.u { font-size: 13px; color: #888888; margin: 0 }
";

fn main() {
    println!("browser: nyx_net (stepped HTTP/TLS) + nyx_web (html5ever/cssparser/layout)");
    nyx_gui::app::run(Browser::new());
}
