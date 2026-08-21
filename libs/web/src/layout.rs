//! Block and inline layout, producing a display list.
//!
//! Two formatting contexts, which is what the vast majority of real pages actually use:
//! * **Block** — children stack vertically, each filling the content width unless `width` says
//!   otherwise.
//! * **Inline** — a run of text and inline elements broken into line boxes at the content width.
//!
//! Text measurement is injected through [`FontMetrics`] rather than hard-wired. That keeps this
//! module free of any Nyx or GUI dependency, so line breaking — the part most likely to be wrong —
//! is testable on the host with a predictable stub font instead of through a serial log.
//!
//! Deliberately absent for now, and each would be visible if you look for it: margin collapsing
//! (adjacent vertical margins add instead of collapsing, so spacing runs generous), floats,
//! positioning, and tables laid out as anything other than stacked blocks.

use crate::dom::{Dom, NodeId, NodeKind};
use crate::style::{Color, ComputedStyle, Display, StyleTree, TextAlign, WhiteSpace, TRANSPARENT};

/// How the embedder measures text. Implemented against `nyx_gui`'s font in the browser app.
pub trait FontMetrics {
    fn text_width(&self, text: &str, style: &ComputedStyle) -> f32;
    fn line_height(&self, style: &ComputedStyle) -> f32;
}

/// How the embedder answers "how big is this image?".
///
/// Layout has to know an image's size *before* it can place it, so the embedder is expected to have
/// already fetched and decoded whatever it could. Returning `None` means "not available" — the image
/// is then laid out from its `width`/`height` attributes if it has them, and falls back to alt text
/// if it does not, so a failed fetch degrades to the same thing as no image support at all.
pub trait ImageSource {
    fn intrinsic_size(&self, src: &str) -> Option<(f32, f32)>;
}

/// An embedder with no image support. Used by the layout tests.
pub struct NoImages;
impl ImageSource for NoImages {
    fn intrinsic_size(&self, _src: &str) -> Option<(f32, f32)> {
        None
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisplayItem {
    Rect { x: f32, y: f32, w: f32, h: f32, color: Color },
    Text {
        x: f32,
        y: f32,
        text: String,
        color: Color,
        font_size: f32,
        bold: bool,
        italic: bool,
        monospace: bool,
        underline: bool,
    },
    /// A replaced element. `src` is the attribute **as written in the document** — resolving it
    /// against the page URL is the embedder's job, and it must resolve it the same way here as it
    /// did when filling its cache or the lookup silently misses.
    Image { x: f32, y: f32, w: f32, h: f32, src: String },
}

/// A clickable region, for hit testing in the browser chrome.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub href: String,
}

#[derive(Debug, Default)]
pub struct LayoutResult {
    pub items: Vec<DisplayItem>,
    pub links: Vec<Link>,
    /// Total content height — what a scrollbar needs.
    pub height: f32,
}

/// One atomic piece of inline content. Words are the unit of line breaking.
struct Word {
    text: String,
    /// Set for a replaced element, which occupies a box instead of a run of glyphs. Its presence is
    /// what makes this word measure by pixels rather than by font metrics.
    image: Option<ImageBox>,
    style: ComputedStyle,
    href: Option<String>,
    /// Whether a collapsible space separated this word from the previous one.
    space_before: bool,
    /// `<br>`, or a newline inside `white-space: pre`.
    forced_break: bool,
}

struct ImageBox {
    src: String,
    w: f32,
    h: f32,
}

impl Word {
    fn text(text: String, style: ComputedStyle, href: Option<&str>, space_before: bool) -> Word {
        Word {
            text,
            image: None,
            style,
            href: href.map(str::to_string),
            space_before,
            forced_break: false,
        }
    }

    fn brk(style: ComputedStyle, href: Option<&str>) -> Word {
        Word {
            text: String::new(),
            image: None,
            style,
            href: href.map(str::to_string),
            space_before: false,
            forced_break: true,
        }
    }
}

struct Ctx<'a> {
    dom: &'a Dom,
    styles: &'a StyleTree,
    fonts: &'a dyn FontMetrics,
    images: &'a dyn ImageSource,
    items: Vec<DisplayItem>,
    links: Vec<Link>,
}

/// Lay out `dom` into a display list `viewport_width` wide.
pub fn layout(
    dom: &Dom,
    styles: &StyleTree,
    fonts: &dyn FontMetrics,
    images: &dyn ImageSource,
    viewport_width: f32,
) -> LayoutResult {
    let mut ctx = Ctx { dom, styles, fonts, images, items: Vec::new(), links: Vec::new() };

    // Start at <body> when there is one: the UA sheet puts the page margin there, and starting at
    // <html> would also try to lay out <head>.
    let start = dom.find_tag("body").unwrap_or(dom.root);
    let height = layout_node(&mut ctx, start, 0.0, 0.0, viewport_width);

    LayoutResult { items: ctx.items, links: ctx.links, height }
}

/// Lay out one node at `(x, y)` in a container `avail_width` wide.
/// Returns the total vertical space consumed, **including** this node's own vertical margins.
fn layout_node(ctx: &mut Ctx, id: NodeId, x: f32, y: f32, avail_width: f32) -> f32 {
    let s = ctx.styles.get(id).clone();
    if s.display == Display::None {
        return 0.0;
    }

    let border_x = x + s.margin.left;
    let border_y = y + s.margin.top;

    // Width: `width` sets the CONTENT box; otherwise fill the container minus our own margins.
    let horizontal_extra = s.padding.left + s.padding.right + s.border.left + s.border.right;
    let content_w = match s.width.resolve(avail_width) {
        Some(w) => w.max(0.0),
        None => (avail_width - s.margin.left - s.margin.right - horizontal_extra).max(0.0),
    };
    let border_w = content_w + horizontal_extra;

    let content_x = border_x + s.border.left + s.padding.left;
    let content_top = border_y + s.border.top + s.padding.top;

    // Background and borders paint UNDER the children, but their height is only known after the
    // children are laid out. Reserve the slots now and patch them once the height is known —
    // emitting them afterwards would paint the background over the text.
    let bg_slot = ctx.items.len();
    if s.background_color != TRANSPARENT {
        ctx.items.push(DisplayItem::Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0, color: TRANSPARENT });
    }
    let border_slot = ctx.items.len();
    let has_border = s.border.top > 0.0 || s.border.right > 0.0 || s.border.bottom > 0.0 || s.border.left > 0.0;
    if has_border {
        for _ in 0..4 {
            ctx.items.push(DisplayItem::Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0, color: TRANSPARENT });
        }
    }

    // A list item gets its marker just outside the content box.
    if s.display == Display::ListItem {
        ctx.items.push(DisplayItem::Text {
            x: content_x - 14.0,
            y: content_top,
            text: "\u{2022}".to_string(),
            color: s.color,
            font_size: s.font_size,
            bold: false,
            italic: false,
            monospace: false,
            underline: false,
        });
    }

    let mut cy = content_top;

    // Children split into runs of inline content and individual block children. A block child ends
    // the current inline run — that is what an anonymous block box does, without the extra tree.
    let children = ctx.dom.node(id).children.clone();
    let mut inline_run: Vec<NodeId> = Vec::new();

    for child in children {
        if is_block_level(ctx, child) {
            if !inline_run.is_empty() {
                cy += layout_inline_run(ctx, &inline_run, content_x, cy, content_w, &s);
                inline_run.clear();
            }
            cy += layout_node(ctx, child, content_x, cy, content_w);
        } else {
            inline_run.push(child);
        }
    }
    if !inline_run.is_empty() {
        cy += layout_inline_run(ctx, &inline_run, content_x, cy, content_w, &s);
    }

    let content_h = match s.height.resolve(avail_width) {
        Some(h) => h,
        None => cy - content_top,
    };
    let border_h = content_h + s.padding.top + s.padding.bottom + s.border.top + s.border.bottom;

    if s.background_color != TRANSPARENT {
        ctx.items[bg_slot] = DisplayItem::Rect {
            x: border_x,
            y: border_y,
            w: border_w,
            h: border_h,
            color: s.background_color,
        };
    }
    if has_border {
        let c = s.border_color;
        ctx.items[border_slot] =
            DisplayItem::Rect { x: border_x, y: border_y, w: border_w, h: s.border.top, color: c };
        ctx.items[border_slot + 1] = DisplayItem::Rect {
            x: border_x,
            y: border_y + border_h - s.border.bottom,
            w: border_w,
            h: s.border.bottom,
            color: c,
        };
        ctx.items[border_slot + 2] =
            DisplayItem::Rect { x: border_x, y: border_y, w: s.border.left, h: border_h, color: c };
        ctx.items[border_slot + 3] = DisplayItem::Rect {
            x: border_x + border_w - s.border.right,
            y: border_y,
            w: s.border.right,
            h: border_h,
            color: c,
        };
    }

    s.margin.top + border_h + s.margin.bottom
}

fn is_block_level(ctx: &Ctx, id: NodeId) -> bool {
    matches!(
        ctx.styles.get(id).display,
        Display::Block | Display::ListItem
    ) && matches!(ctx.dom.node(id).kind, NodeKind::Element { .. })
}

/// Lay out a run of inline content into line boxes. Returns the height used.
// `flush!` resets `line_w` for the next line; on the final expansion after the loop that reset is
// dead, which is exactly what the lint sees. Keeping the reset inside the macro is what makes every
// other expansion correct.
#[allow(unused_assignments)]
fn layout_inline_run(
    ctx: &mut Ctx,
    roots: &[NodeId],
    x: f32,
    y: f32,
    width: f32,
    parent: &ComputedStyle,
) -> f32 {
    let mut words: Vec<Word> = Vec::new();
    for &r in roots {
        collect_words(ctx, r, None, width, &mut words);
    }
    if words.is_empty() {
        return 0.0;
    }

    // A word's box: an image measures in pixels, text through the injected font.
    macro_rules! word_w {
        ($w:expr) => {
            match &$w.image {
                Some(img) => img.w,
                None => ctx.fonts.text_width(&$w.text, &$w.style),
            }
        };
    }
    macro_rules! word_h {
        ($w:expr) => {
            match &$w.image {
                Some(img) => img.h,
                None => ctx.fonts.line_height(&$w.style),
            }
        };
    }

    let mut cy = y;
    let mut line: Vec<(usize, f32, f32)> = Vec::new(); // (word index, x offset, width)
    let mut line_w = 0.0f32;

    // Closure-free flush so `ctx` stays borrowable.
    macro_rules! flush {
        () => {
            if !line.is_empty() {
                let line_h = line
                    .iter()
                    .map(|&(i, _, _)| word_h!(words[i]))
                    .fold(0.0f32, f32::max);
                let slack = (width - line_w).max(0.0);
                let shift = match parent.text_align {
                    TextAlign::Left => 0.0,
                    TextAlign::Center => slack / 2.0,
                    TextAlign::Right => slack,
                };
                for &(i, ox, w) in &line {
                    let word = &words[i];
                    // Baseline-ish: bottom-align smaller text within the line box, which is what
                    // mixed font sizes on one line should look like.
                    let wh = word_h!(*word);
                    let wy = cy + (line_h - wh);
                    match &word.image {
                        Some(img) => ctx.items.push(DisplayItem::Image {
                            x: x + shift + ox,
                            y: wy,
                            w: img.w,
                            h: img.h,
                            src: img.src.clone(),
                        }),
                        None => ctx.items.push(DisplayItem::Text {
                            x: x + shift + ox,
                            y: wy,
                            text: word.text.clone(),
                            color: word.style.color,
                            font_size: word.style.font_size,
                            bold: word.style.bold,
                            italic: word.style.italic,
                            monospace: word.style.monospace,
                            underline: word.style.underline,
                        }),
                    }
                    if let Some(href) = &word.href {
                        ctx.links.push(Link {
                            x: x + shift + ox,
                            y: wy,
                            w,
                            h: wh,
                            href: href.clone(),
                        });
                    }
                }
                cy += line_h;
                line.clear();
                line_w = 0.0;
            }
        };
    }

    for i in 0..words.len() {
        if words[i].forced_break {
            // An empty line still advances by one line height, which is what <br><br> must do.
            if line.is_empty() {
                cy += ctx.fonts.line_height(&words[i].style);
            } else {
                flush!();
            }
            continue;
        }

        let w = word_w!(words[i]);
        let space = if words[i].space_before && !line.is_empty() {
            ctx.fonts.text_width(" ", &words[i].style)
        } else {
            0.0
        };

        let wraps = words[i].style.white_space != WhiteSpace::NoWrap
            && !line.is_empty()
            && line_w + space + w > width;
        if wraps {
            flush!();
        }

        let space = if line.is_empty() { 0.0 } else { space };
        line.push((i, line_w + space, w));
        line_w += space + w;
    }
    flush!();

    cy - y
}

/// Walk an inline subtree, flattening it into words that carry their own computed style.
/// `avail` is the width of the line box, needed to size replaced elements.
fn collect_words(ctx: &Ctx, id: NodeId, href: Option<&str>, avail: f32, out: &mut Vec<Word>) {
    let style = ctx.styles.get(id).clone();
    if style.display == Display::None {
        return;
    }

    match &ctx.dom.node(id).kind {
        NodeKind::Text(text) => {
            if style.white_space == WhiteSpace::Pre {
                // Preserve the text as written; only newlines break.
                for (i, seg) in text.split('\n').enumerate() {
                    if i > 0 {
                        out.push(Word::brk(style.clone(), href));
                    }
                    if !seg.is_empty() {
                        out.push(Word::text(seg.to_string(), style.clone(), href, false));
                    }
                }
            } else {
                // Collapsed whitespace: split into words, remembering where separators were so the
                // renderer can re-insert exactly one space.
                let leading_space = text.starts_with(|c: char| c.is_ascii_whitespace());
                for (n, w) in text.split_ascii_whitespace().enumerate() {
                    // A space before the first word matters only if something precedes it in
                    // the run — otherwise a stray newline in the source would indent the line.
                    let space = n > 0 || (leading_space && !out.is_empty());
                    out.push(Word::text(w.to_string(), style.clone(), href, space));
                }
            }
        }
        NodeKind::Element { name, .. } => {
            if name == "br" {
                out.push(Word::brk(style, href));
                return;
            }
            // An <a> ancestor makes every word inside it clickable, however deeply nested.
            let own_href = ctx.dom.node(id).attr("href");
            let href = if name == "a" && own_href.is_some() { own_href } else { href };

            if name == "img" {
                match image_box(ctx, id, &style, avail) {
                    Some(img) => out.push(Word {
                        text: String::new(),
                        image: Some(img),
                        style,
                        href: href.map(str::to_string),
                        space_before: true,
                        forced_break: false,
                    }),
                    // No usable image: show the alt text so the page still reads. This is also the
                    // path a failed fetch takes, which is why it is not an error case.
                    None => {
                        if let Some(alt) = ctx.dom.node(id).attr("alt").filter(|a| !a.is_empty()) {
                            let alt = alt.to_string();
                            for (n, w) in alt.split_ascii_whitespace().enumerate() {
                                out.push(Word::text(w.to_string(), style.clone(), href, n > 0));
                            }
                        }
                    }
                }
                return;
            }

            for &c in &ctx.dom.node(id).children {
                collect_words(ctx, c, href, avail, out);
            }
        }
        _ => {}
    }
}

/// Work out the box an `<img>` occupies, or `None` if it has no size we can justify inventing.
fn image_box(ctx: &Ctx, id: NodeId, style: &ComputedStyle, avail: f32) -> Option<ImageBox> {
    let node = ctx.dom.node(id);
    let src = node.attr("src").filter(|s| !s.is_empty())?;
    let intrinsic = ctx.images.intrinsic_size(src);

    // CSS beats the presentational `width`/`height` attributes — that is the whole point of the
    // cascade. (A percentage `height` should resolve against the containing block's height, not its
    // width; we only have the width here, and a percentage height on an image is vanishingly rare.)
    let attr = |n: &str| {
        node.attr(n)
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f32>().ok())
            .filter(|v| *v > 0.0)
    };
    let mut w = style.width.resolve(avail).or_else(|| attr("width"));
    let mut h = style.height.resolve(avail).or_else(|| attr("height"));

    match (w, h) {
        (Some(_), Some(_)) => {}
        // With one axis given, keep the intrinsic aspect ratio rather than guessing a square.
        (Some(w0), None) => {
            h = Some(match intrinsic {
                Some((iw, ih)) if iw > 0.0 => w0 * ih / iw,
                _ => w0,
            })
        }
        (None, Some(h0)) => {
            w = Some(match intrinsic {
                Some((iw, ih)) if ih > 0.0 => h0 * iw / ih,
                _ => h0,
            })
        }
        // Nothing to go on. Falling back to alt text beats reserving a blank box of a made-up size.
        (None, None) => {
            let (iw, ih) = intrinsic?;
            w = Some(iw);
            h = Some(ih);
        }
    }

    let (mut w, mut h) = (w?, h?);
    if !w.is_finite() || !h.is_finite() || w <= 0.0 || h <= 0.0 {
        return None;
    }
    // An image wider than the line would otherwise push the whole line off-screen.
    if w > avail && avail > 0.0 {
        h *= avail / w;
        w = avail;
    }
    Some(ImageBox { src: src.to_string(), w, h })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::css::Stylesheet;
    use crate::style::compute;

    /// A predictable stub font: every glyph is `0.5 * font_size` wide, lines are `1.25 * font_size`.
    /// Fixed metrics are the point — line-break assertions have to be exact to mean anything.
    struct Stub;
    impl FontMetrics for Stub {
        fn text_width(&self, text: &str, style: &ComputedStyle) -> f32 {
            text.chars().count() as f32 * style.font_size * 0.5
        }
        fn line_height(&self, style: &ComputedStyle) -> f32 {
            style.font_size * 1.25
        }
    }

    /// Every image is 100x50, so aspect-ratio maths is checkable by eye.
    struct Pics;
    impl ImageSource for Pics {
        fn intrinsic_size(&self, src: &str) -> Option<(f32, f32)> {
            if src == "missing.png" { None } else { Some((100.0, 50.0)) }
        }
    }

    fn run(html: &str, css: &str, width: f32) -> LayoutResult {
        let dom = Dom::parse(html);
        let styles = compute(&dom, &Stylesheet::parse(css));
        layout(&dom, &styles, &Stub, &NoImages, width)
    }

    fn run_img(html: &str, css: &str, width: f32) -> LayoutResult {
        let dom = Dom::parse(html);
        let styles = compute(&dom, &Stylesheet::parse(css));
        layout(&dom, &styles, &Stub, &Pics, width)
    }

    fn images(r: &LayoutResult) -> Vec<(f32, f32, f32, f32, String)> {
        r.items
            .iter()
            .filter_map(|i| match i {
                DisplayItem::Image { x, y, w, h, src } => {
                    Some((*x, *y, *w, *h, src.clone()))
                }
                _ => None,
            })
            .collect()
    }

    fn texts(r: &LayoutResult) -> Vec<(f32, f32, String)> {
        r.items
            .iter()
            .filter_map(|i| match i {
                DisplayItem::Text { x, y, text, .. } => Some((*x, *y, text.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn blocks_stack_vertically() {
        let r = run("<body><p>one</p><p>two</p></body>", "p { margin: 0 }", 800.0);
        let t = texts(&r);
        assert_eq!(t.len(), 2);
        assert!(t[1].1 > t[0].1, "second paragraph must be below the first");
        assert_eq!(t[0].0, t[1].0, "and start at the same x");
    }

    #[test]
    fn text_wraps_at_the_content_width() {
        // 8px font -> 4px per char. "aaaa" = 16px. Width 40 fits two words + a space (16+4+16=36),
        // and a third would need 56.
        let r = run(
            "<body><p>aaaa aaaa aaaa</p></body>",
            "body { margin: 0 } p { margin: 0; font-size: 8px }",
            40.0,
        );
        let t = texts(&r);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].1, t[1].1, "first two words share a line");
        assert!(t[2].1 > t[1].1, "third word wraps to the next line");
    }

    #[test]
    fn a_long_word_does_not_loop_forever() {
        // A word wider than the line must still be placed, not retried.
        let r = run(
            "<body><p>aaaaaaaaaaaaaaaaaaaa</p></body>",
            "body { margin: 0 } p { margin: 0; font-size: 8px }",
            10.0,
        );
        assert_eq!(texts(&r).len(), 1);
    }

    #[test]
    fn br_forces_a_line_break() {
        let r = run(
            "<body><p>a<br>b</p></body>",
            "body { margin: 0 } p { margin: 0; font-size: 10px }",
            800.0,
        );
        let t = texts(&r);
        assert_eq!(t.len(), 2);
        assert!(t[1].1 > t[0].1);
    }

    #[test]
    fn margins_offset_the_content() {
        let r = run("<body><p>x</p></body>", "body { margin: 0 } p { margin: 10px 0 0 20px }", 800.0);
        let t = texts(&r);
        assert_eq!(t[0].0, 20.0);
        assert_eq!(t[0].1, 10.0);
    }

    #[test]
    fn display_none_produces_nothing() {
        let r = run("<body><p>gone</p><p>here</p></body>", "p:first-child { display: none }", 800.0);
        let t = texts(&r);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].2, "here");
    }

    #[test]
    fn head_content_is_never_laid_out() {
        let r = run("<html><head><title>T</title></head><body>body text</body></html>", "", 800.0);
        let joined: String = texts(&r).iter().map(|t| t.2.clone()).collect::<Vec<_>>().join(" ");
        assert!(!joined.contains('T'), "title must not appear in the page");
        assert!(joined.contains("body"));
    }

    #[test]
    fn links_get_hit_rects_even_when_nested() {
        let r = run(
            r#"<body><a href="/x">click <b>here</b></a></body>"#,
            "body { margin: 0 }",
            800.0,
        );
        assert_eq!(r.links.len(), 2, "both words inside the <a> are clickable");
        assert!(r.links.iter().all(|l| l.href == "/x"));
        assert!(r.links[0].w > 0.0 && r.links[0].h > 0.0);
    }

    #[test]
    fn background_paints_before_the_text_it_sits_behind() {
        let r = run("<body><p>x</p></body>", "p { background: red }", 800.0);
        let bg = r.items.iter().position(|i| matches!(i, DisplayItem::Rect { color, .. } if *color == 0xFF_FF0000));
        let tx = r.items.iter().position(|i| matches!(i, DisplayItem::Text { .. }));
        assert!(bg.is_some() && tx.is_some());
        assert!(bg < tx, "background must be emitted before the text");
    }

    #[test]
    fn background_rect_covers_the_laid_out_height() {
        let r = run(
            "<body><p>x</p></body>",
            "body { margin: 0 } p { margin: 0; font-size: 10px; background: red }",
            800.0,
        );
        let rect = r
            .items
            .iter()
            .find_map(|i| match i {
                DisplayItem::Rect { h, w, color, .. } if *color == 0xFF_FF0000 => Some((*w, *h)),
                _ => None,
            })
            .expect("background rect");
        assert_eq!(rect.0, 800.0, "block fills the container width");
        assert_eq!(rect.1, 12.5, "and is one line tall");
    }

    #[test]
    fn text_align_center_and_right() {
        let css = "body { margin: 0 } p { margin: 0; font-size: 10px; text-align: center }";
        let r = run("<body><p>ab</p></body>", css, 100.0);
        // "ab" = 10px wide, centred in 100 -> x = 45.
        assert_eq!(texts(&r)[0].0, 45.0);

        let css = "body { margin: 0 } p { margin: 0; font-size: 10px; text-align: right }";
        let r = run("<body><p>ab</p></body>", css, 100.0);
        assert_eq!(texts(&r)[0].0, 90.0);
    }

    #[test]
    fn pre_preserves_newlines() {
        let r = run("<body><pre>a\nb</pre></body>", "body { margin: 0 } pre { margin: 0 }", 800.0);
        let t = texts(&r);
        assert_eq!(t.len(), 2);
        assert!(t[1].1 > t[0].1, "the newline inside <pre> must break the line");
    }

    #[test]
    fn inline_elements_stay_on_one_line() {
        let r = run(
            "<body><p>a <b>b</b> c</p></body>",
            "body { margin: 0 } p { margin: 0; font-size: 10px }",
            800.0,
        );
        let t = texts(&r);
        assert_eq!(t.len(), 3);
        assert!(t.iter().all(|w| w.1 == t[0].1), "no wrapping at this width");
        assert!(t[1].0 > t[0].0 && t[2].0 > t[1].0, "laid out left to right");
    }

    #[test]
    fn a_block_child_splits_the_surrounding_inline_run() {
        // The text before and after the <div> must not end up on the same line.
        let r = run(
            "<body>before<div>mid</div>after</body>",
            "body { margin: 0 } div { margin: 0 }",
            800.0,
        );
        let t = texts(&r);
        assert_eq!(t.len(), 3);
        assert!(t[0].1 < t[1].1 && t[1].1 < t[2].1);
    }

    #[test]
    fn an_image_is_laid_out_at_its_intrinsic_size() {
        let r = run_img(r#"<body><img src="a.png"></body>"#, "body { margin: 0 }", 800.0);
        let i = images(&r);
        assert_eq!(i.len(), 1);
        assert_eq!((i[0].2, i[0].3), (100.0, 50.0));
        assert_eq!(i[0].4, "a.png", "src is passed through verbatim for the embedder to resolve");
    }

    #[test]
    fn one_given_axis_preserves_the_aspect_ratio() {
        // 100x50 intrinsic, asked for width 200 -> height must follow to 100, not stay at 50.
        let r = run_img(r#"<body><img src="a.png" width="200"></body>"#, "body { margin: 0 }", 800.0);
        assert_eq!(images(&r)[0].3, 100.0);

        let r = run_img(r#"<body><img src="a.png" height="100"></body>"#, "body { margin: 0 }", 800.0);
        assert_eq!(images(&r)[0].2, 200.0);
    }

    #[test]
    fn css_width_beats_the_html_attribute() {
        let r = run_img(
            r#"<body><img src="a.png" width="200"></body>"#,
            "body { margin: 0 } img { width: 40px }",
            800.0,
        );
        assert_eq!(images(&r)[0].2, 40.0);
    }

    #[test]
    fn an_oversized_image_is_scaled_down_to_the_line() {
        // 400 wide in a 100 wide viewport -> clamped to 100, and the height comes down with it.
        let r = run_img(
            r#"<body><img src="a.png" width="400"></body>"#,
            "body { margin: 0 }",
            100.0,
        );
        let i = images(&r);
        assert_eq!((i[0].2, i[0].3), (100.0, 50.0));
    }

    #[test]
    fn an_unavailable_image_falls_back_to_alt_text() {
        let r = run_img(
            r#"<body><img src="missing.png" alt="a cat"></body>"#,
            "body { margin: 0 }",
            800.0,
        );
        assert!(images(&r).is_empty(), "no box is reserved for an image we do not have");
        let t = texts(&r);
        assert_eq!(t.len(), 2, "the alt text is laid out as words instead");
        assert_eq!(t[0].2, "a");
    }

    #[test]
    fn an_image_sets_the_line_height_it_sits_on() {
        // The image is 50 tall and the text 12.5; the line must be tall enough for the image, and
        // the next line must clear it.
        let r = run_img(
            r#"<body><p><img src="a.png">x</p><p>y</p></body>"#,
            "body { margin: 0 } p { margin: 0; font-size: 10px }",
            800.0,
        );
        let t = texts(&r);
        assert_eq!(t.len(), 2);
        assert!(t[1].1 >= 50.0, "second paragraph starts below the 50px image, got {}", t[1].1);
    }

    #[test]
    fn an_image_inside_a_link_is_clickable() {
        let r = run_img(
            r#"<body><a href="/x"><img src="a.png"></a></body>"#,
            "body { margin: 0 }",
            800.0,
        );
        assert_eq!(r.links.len(), 1);
        assert_eq!(r.links[0].href, "/x");
        assert_eq!((r.links[0].w, r.links[0].h), (100.0, 50.0), "the hit rect is the image box");
    }

    #[test]
    fn reported_height_covers_the_content() {
        let r = run(
            "<body><p>a</p><p>b</p></body>",
            "body { margin: 0 } p { margin: 0; font-size: 10px }",
            800.0,
        );
        assert_eq!(r.height, 25.0, "two 12.5px lines");
    }
}
