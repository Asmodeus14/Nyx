//! Selector matching, the cascade, and computed values.
//!
//! Three origins, in increasing priority: the built-in UA stylesheet, author stylesheets (`<style>`
//! and, later, linked ones), and the inline `style=""` attribute. Within an origin, declarations sort
//! by specificity then source order, and `!important` flips an origin above the normal ones. That is
//! the real cascade, not an approximation of it.
//!
//! The UA stylesheet is written as CSS text and run through the same parser as everything else —
//! which is how real engines do it, and means a bug in the parser shows up in the defaults rather
//! than hiding behind a hand-built table.

use std::collections::HashMap;

use crate::css::{
    AttrOp, Combinator, Compound, MediaContext, PseudoClass, Selector, StyleRule, Stylesheet,
};
use crate::dom::{Dom, NodeId, NodeKind};

/// 0xAARRGGBB, matching `nyx_gui::Canvas`.
pub type Color = u32;

pub const BLACK: Color = 0xFF_000000;
pub const TRANSPARENT: Color = 0x00_000000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Display {
    Block,
    Inline,
    InlineBlock,
    ListItem,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    /// Collapse runs of whitespace and wrap at spaces.
    Normal,
    /// Preserve whitespace and newlines (`<pre>`).
    Pre,
    /// Collapse, but never wrap.
    NoWrap,
}

/// A CSS length that may still depend on the containing block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LengthOrAuto {
    Auto,
    Px(f32),
    Percent(f32),
}

impl LengthOrAuto {
    /// Resolve against a containing-block size. `Auto` stays `None` so callers can apply their own
    /// rule (fill for width, shrink for height).
    pub fn resolve(self, containing: f32) -> Option<f32> {
        match self {
            LengthOrAuto::Auto => None,
            LengthOrAuto::Px(v) => Some(v),
            LengthOrAuto::Percent(p) => Some(containing * p / 100.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Edges {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    pub display: Display,
    pub color: Color,
    pub background_color: Color,
    pub font_size: f32,
    pub bold: bool,
    pub italic: bool,
    pub monospace: bool,
    pub underline: bool,
    pub text_align: TextAlign,
    pub white_space: WhiteSpace,
    pub margin: Edges,
    pub padding: Edges,
    pub border: Edges,
    pub border_color: Color,
    pub width: LengthOrAuto,
    pub height: LengthOrAuto,
}

impl Default for ComputedStyle {
    fn default() -> Self {
        ComputedStyle {
            display: Display::Inline,
            color: BLACK,
            background_color: TRANSPARENT,
            font_size: 16.0,
            bold: false,
            italic: false,
            monospace: false,
            underline: false,
            text_align: TextAlign::Left,
            white_space: WhiteSpace::Normal,
            margin: Edges::default(),
            padding: Edges::default(),
            border: Edges::default(),
            border_color: BLACK,
            width: LengthOrAuto::Auto,
            height: LengthOrAuto::Auto,
        }
    }
}

impl ComputedStyle {
    /// Start a child's style from its parent: only inherited properties carry over.
    ///
    /// Getting this list wrong is quietly catastrophic — inheriting `margin` would compound spacing
    /// at every level of nesting, and *not* inheriting `color` would make every nested element black.
    fn inherit_from(parent: &ComputedStyle) -> ComputedStyle {
        ComputedStyle {
            color: parent.color,
            font_size: parent.font_size,
            bold: parent.bold,
            italic: parent.italic,
            monospace: parent.monospace,
            // Not strictly inherited in CSS, but decorations propagate to descendants visually,
            // and inheriting is the cheap way to get `<a><span>text</span></a>` underlined.
            underline: parent.underline,
            text_align: parent.text_align,
            white_space: parent.white_space,
            ..ComputedStyle::default()
        }
    }
}

/// The built-in stylesheet. Everything a page does not say is decided here.
pub const UA_CSS: &str = r#"
html, body, div, p, section, article, header, footer, nav, aside, main, figure,
figcaption, blockquote, form, fieldset, table, tbody, thead, tfoot, tr, hr,
h1, h2, h3, h4, h5, h6, ul, ol, dl, dt, dd, pre, address, details, summary { display: block }

li { display: list-item }

head, script, style, title, meta, link, base, noscript, template { display: none }

body { margin: 8px; color: #000000 }
p, blockquote, dl, figure { margin: 16px 0 }
h1 { font-size: 32px; font-weight: bold; margin: 21px 0 }
h2 { font-size: 24px; font-weight: bold; margin: 20px 0 }
h3 { font-size: 19px; font-weight: bold; margin: 18px 0 }
h4 { font-size: 16px; font-weight: bold; margin: 21px 0 }
h5 { font-size: 13px; font-weight: bold; margin: 22px 0 }
h6 { font-size: 11px; font-weight: bold; margin: 24px 0 }
ul, ol { margin: 16px 0; padding: 0 0 0 40px }
b, strong { font-weight: bold }
i, em, cite, var, address { font-style: italic }
a { color: #0000ee; text-decoration: underline }
pre { white-space: pre; margin: 13px 0 }
pre, code, kbd, samp, tt { font-family: monospace }
small { font-size: 13px }
big { font-size: 19px }
hr { margin: 8px 0; border-top: 1px solid #808080 }
th { font-weight: bold; text-align: center }
td, th { padding: 1px }
center { text-align: center }
"#;

/// Computed style for every node, indexed by `NodeId`.
pub struct StyleTree {
    pub styles: Vec<ComputedStyle>,
}

impl StyleTree {
    pub fn get(&self, id: NodeId) -> &ComputedStyle {
        &self.styles[id]
    }
}

/// One declaration that won its way into an element, with the keys it was sorted by.
struct Winner<'a> {
    origin: u8,
    specificity: (u32, u32, u32),
    order: usize,
    name: &'a str,
    value: &'a str,
}

/// Run the cascade over the whole document.
///
/// `author` is the page's CSS (concatenated `<style>` blocks). The UA sheet is parsed once here.
pub fn compute(dom: &Dom, author: &Stylesheet) -> StyleTree {
    compute_media(dom, author, &MediaContext::default())
}

/// The cascade, with a real viewport for `@media` to resolve against.
///
/// Separate from `compute` so the existing callers and tests keep a stable signature; the browser
/// uses this one and re-runs it on resize, since a width change can flip which rules apply.
pub fn compute_media(dom: &Dom, author: &Stylesheet, ctx: &MediaContext) -> StyleTree {
    let ua = Stylesheet::parse(UA_CSS);
    let ua_index = RuleIndex::build(&ua, ctx);
    let author_index = RuleIndex::build(author, ctx);
    let mut styles = vec![ComputedStyle::default(); dom.nodes.len()];

    // Document order matters: a child's inherited values must come from an already-computed parent.
    for id in dom.descendants(dom.root) {
        let parent_style = match dom.node(id).parent {
            Some(p) => styles[p].clone(),
            None => ComputedStyle::default(),
        };

        // Text nodes take their parent's style unchanged — they have no selectors of their own, and
        // the inline formatter reads font/colour off them directly.
        if !matches!(dom.node(id).kind, NodeKind::Element { .. }) {
            let mut s = ComputedStyle::inherit_from(&parent_style);
            s.display = Display::Inline;
            styles[id] = s;
            continue;
        }

        let mut style = ComputedStyle::inherit_from(&parent_style);

        let mut winners: Vec<Winner> = Vec::new();
        ua_index.collect(dom, id, 0, &mut winners);
        author_index.collect(dom, id, 1, &mut winners);

        // The inline `style` attribute behaves like an author rule that beats every selector.
        // Wrap it in a dummy rule so the same declaration parser can be reused.
        let inline_sheet = match dom.node(id).attr("style").filter(|s| !s.is_empty()) {
            Some(css) => Stylesheet::parse(&format!("x {{ {css} }}")),
            None => Stylesheet::default(),
        };
        for rule in &inline_sheet.rules {
            for (i, d) in rule.declarations.iter().enumerate() {
                winners.push(Winner {
                    origin: if d.important { 5 } else { 2 },
                    specificity: (u32::MAX, u32::MAX, u32::MAX),
                    order: i,
                    name: &d.name,
                    value: &d.value,
                });
            }
        }

        // Sort ascending so later writes win, which makes application a simple forward pass.
        winners.sort_by(|a, b| {
            a.origin
                .cmp(&b.origin)
                .then(a.specificity.cmp(&b.specificity))
                .then(a.order.cmp(&b.order))
        });
        for w in &winners {
            apply(&mut style, w.name, w.value, &parent_style);
        }

        styles[id] = style;
    }

    StyleTree { styles }
}

/// One selector, and which rule it belongs to.
struct Candidate<'a> {
    rule: usize,
    selector: &'a Selector,
}

/// A stylesheet indexed for matching.
///
/// Testing every rule against every element is O(nodes × rules): on a real page that is 8500 × 500 =
/// four million selector matches, and it is the single slowest thing the engine does. Almost all of
/// those fail on the rightmost compound's tag, class or id — so bucket the selectors by exactly that
/// and a node only ever tries the handful that could plausibly match. This is what every real engine
/// does, and it is the difference between a page appearing and a page seeming to hang.
struct RuleIndex<'a> {
    by_id: HashMap<&'a str, Vec<Candidate<'a>>>,
    by_class: HashMap<&'a str, Vec<Candidate<'a>>>,
    by_tag: HashMap<&'a str, Vec<Candidate<'a>>>,
    /// Selectors whose rightmost compound has no tag, class or id (`*`, `[attr]`, `:root`). Nothing
    /// narrows these, so every element pays for them — which is why they are worth keeping rare.
    universal: Vec<Candidate<'a>>,
    /// Declaration index each rule starts at, in whole-sheet order. Source order has to be counted
    /// over the *whole* sheet, not over the matches, or bucketing would silently reorder the cascade.
    base: Vec<usize>,
    rules: &'a [StyleRule],
}

impl<'a> RuleIndex<'a> {
    fn build(sheet: &'a Stylesheet, ctx: &MediaContext) -> RuleIndex<'a> {
        let mut index = RuleIndex {
            by_id: HashMap::new(),
            by_class: HashMap::new(),
            by_tag: HashMap::new(),
            universal: Vec::new(),
            base: Vec::with_capacity(sheet.rules.len()),
            rules: &sheet.rules,
        };

        let mut declarations = 0usize;
        for (i, rule) in sheet.rules.iter().enumerate() {
            index.base.push(declarations);
            // ★ `declarations` advances for EVERY rule, including ones filtered out by media.
            // Source order breaks cascade ties and is defined over the whole sheet, so skipping
            // the count for a non-matching rule would shift every later rule's order and change
            // which declaration wins — a bug that only appears on pages that use @media.
            declarations += rule.declarations.len();

            // A rule inside a non-matching @media is simply not bucketed, so the matcher never
            // sees it. Filtering here rather than at match time keeps it off the hot path.
            if !rule.applies(ctx) {
                continue;
            }

            for selector in &rule.selectors {
                let candidate = Candidate { rule: i, selector };
                // The rightmost compound is what the matcher tests first, so it is what decides the
                // bucket. Most selective key wins: an id narrows harder than a class, a class harder
                // than a tag.
                let Some((_, key)) = selector.parts.first() else {
                    index.universal.push(candidate);
                    continue;
                };
                if let Some(id) = &key.id {
                    index.by_id.entry(id.as_str()).or_default().push(candidate);
                } else if let Some(class) = key.classes.first() {
                    index.by_class.entry(class.as_str()).or_default().push(candidate);
                } else if let Some(tag) = &key.tag {
                    index.by_tag.entry(tag.as_str()).or_default().push(candidate);
                } else {
                    index.universal.push(candidate);
                }
            }
        }
        index
    }

    /// Every declaration from this sheet that applies to `id`, appended to `out`.
    fn collect(&self, dom: &Dom, id: NodeId, origin: u8, out: &mut Vec<Winner<'a>>) {
        let node = dom.node(id);

        // A rule applies with the specificity of its BEST-matching selector, not the first one, so
        // hits are accumulated per rule before anything is emitted. The list is short in practice —
        // linear scanning beats a map here.
        let mut hits: Vec<(usize, (u32, u32, u32))> = Vec::new();
        let test = |candidates: &[Candidate<'a>], hits: &mut Vec<(usize, (u32, u32, u32))>| {
            for candidate in candidates {
                if !matches(dom, id, candidate.selector) {
                    continue;
                }
                let spec = candidate.selector.specificity();
                match hits.iter_mut().find(|(r, _)| *r == candidate.rule) {
                    Some((_, best)) if *best >= spec => {}
                    Some((_, best)) => *best = spec,
                    None => hits.push((candidate.rule, spec)),
                }
            }
        };

        test(&self.universal, &mut hits);
        if let Some(tag) = node.tag() {
            if let Some(bucket) = self.by_tag.get(tag) {
                test(bucket, &mut hits);
            }
        }
        if let Some(elem_id) = node.attr("id") {
            if let Some(bucket) = self.by_id.get(elem_id) {
                test(bucket, &mut hits);
            }
        }
        for class in node.classes() {
            if let Some(bucket) = self.by_class.get(class) {
                test(bucket, &mut hits);
            }
        }

        // Emit in source order. `Winner.order` is a whole-sheet declaration index, so the sort in
        // `compute` resolves ties exactly as an unbucketed pass would.
        hits.sort_unstable_by_key(|(rule, _)| *rule);
        for (rule_idx, spec) in hits {
            let rule = &self.rules[rule_idx];
            for (i, d) in rule.declarations.iter().enumerate() {
                out.push(Winner {
                    // `!important` lifts a declaration above every normal-origin one. UA-important
                    // outranking author-important is a real CSS rule, but nothing in our UA sheet
                    // uses !important, so the simple ordering is enough.
                    origin: if d.important { origin + 3 } else { origin },
                    specificity: spec,
                    order: self.base[rule_idx] + i,
                    name: &d.name,
                    value: &d.value,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------------------------

pub fn matches(dom: &Dom, id: NodeId, sel: &Selector) -> bool {
    matches_from(dom, id, sel, 0)
}

/// Match `sel.parts[idx..]` ending at `id`.
///
/// Selectors are stored rightmost-first, so this walks leftward through ancestors and previous
/// siblings. Descendant and later-sibling combinators **backtrack** — taking the first matching
/// ancestor greedily is wrong for selectors like `a b c`, where the nearest `b` may not be the one
/// with an `a` above it.
fn matches_from(dom: &Dom, id: NodeId, sel: &Selector, idx: usize) -> bool {
    if !compound_matches(dom, id, &sel.parts[idx].1) {
        return false;
    }
    if idx + 1 == sel.parts.len() {
        return true;
    }
    match sel.parts[idx].0 {
        Combinator::None => true, // no combinator but more parts: malformed, treat as matched
        Combinator::Child => match element_parent(dom, id) {
            Some(p) => matches_from(dom, p, sel, idx + 1),
            None => false,
        },
        Combinator::Descendant => {
            let mut cur = element_parent(dom, id);
            while let Some(p) = cur {
                if matches_from(dom, p, sel, idx + 1) {
                    return true;
                }
                cur = element_parent(dom, p);
            }
            false
        }
        Combinator::NextSibling => match prev_element_sibling(dom, id) {
            Some(s) => matches_from(dom, s, sel, idx + 1),
            None => false,
        },
        Combinator::LaterSibling => {
            let mut cur = prev_element_sibling(dom, id);
            while let Some(s) = cur {
                if matches_from(dom, s, sel, idx + 1) {
                    return true;
                }
                cur = prev_element_sibling(dom, s);
            }
            false
        }
    }
}

fn compound_matches(dom: &Dom, id: NodeId, c: &Compound) -> bool {
    let node = dom.node(id);
    let Some(tag) = node.tag() else { return false };

    if let Some(want) = &c.tag {
        if want != tag {
            return false;
        }
    }
    if let Some(want) = &c.id {
        if node.id() != Some(want.as_str()) {
            return false;
        }
    }
    for class in &c.classes {
        if !node.classes().any(|k| k == class) {
            return false;
        }
    }
    for a in &c.attrs {
        let Some(v) = node.attr(&a.name) else { return false };
        let ok = match a.op {
            AttrOp::Exists => true,
            AttrOp::Equals => v == a.value,
            AttrOp::Includes => v.split_ascii_whitespace().any(|w| w == a.value),
            AttrOp::DashMatch => v == a.value || v.starts_with(&format!("{}-", a.value)),
            AttrOp::Prefix => !a.value.is_empty() && v.starts_with(&a.value),
            AttrOp::Suffix => !a.value.is_empty() && v.ends_with(&a.value),
            AttrOp::Substring => !a.value.is_empty() && v.contains(&a.value),
        };
        if !ok {
            return false;
        }
    }
    for p in &c.pseudos {
        let ok = match p {
            PseudoClass::FirstChild => element_index(dom, id).map_or(false, |(i, _)| i == 0),
            PseudoClass::LastChild => element_index(dom, id).map_or(false, |(i, n)| i + 1 == n),
            PseudoClass::OnlyChild => element_index(dom, id).map_or(false, |(_, n)| n == 1),
            PseudoClass::Root => tag == "html",
            PseudoClass::Link => tag == "a" && node.attr("href").is_some(),
            // Never match what we do not implement. Matching would apply styles the page meant for
            // a state we cannot detect (`:hover`, `:nth-child(...)`), which is worse than ignoring.
            PseudoClass::Unsupported(_) => false,
        };
        if !ok {
            return false;
        }
    }
    true
}

fn element_parent(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let p = dom.node(id).parent?;
    matches!(dom.node(p).kind, NodeKind::Element { .. }).then_some(p)
}

fn prev_element_sibling(dom: &Dom, id: NodeId) -> Option<NodeId> {
    let p = dom.node(id).parent?;
    let sibs = &dom.node(p).children;
    let pos = sibs.iter().position(|&c| c == id)?;
    sibs[..pos]
        .iter()
        .rev()
        .copied()
        .find(|&c| matches!(dom.node(c).kind, NodeKind::Element { .. }))
}

/// `(index among element siblings, total element siblings)`.
fn element_index(dom: &Dom, id: NodeId) -> Option<(usize, usize)> {
    let p = dom.node(id).parent?;
    let elems: Vec<NodeId> = dom
        .node(p)
        .children
        .iter()
        .copied()
        .filter(|&c| matches!(dom.node(c).kind, NodeKind::Element { .. }))
        .collect();
    let i = elems.iter().position(|&c| c == id)?;
    Some((i, elems.len()))
}

// ---------------------------------------------------------------------------------------------
// Applying declarations
// ---------------------------------------------------------------------------------------------

fn apply(style: &mut ComputedStyle, name: &str, value: &str, parent: &ComputedStyle) {
    let v = value.trim();
    let lower = v.to_ascii_lowercase();
    match name {
        "display" => {
            style.display = match lower.as_str() {
                "block" | "flex" | "grid" | "table" | "flow-root" => Display::Block,
                "inline" => Display::Inline,
                "inline-block" | "inline-flex" => Display::InlineBlock,
                "list-item" => Display::ListItem,
                "none" => Display::None,
                _ => style.display,
            }
        }
        "color" => {
            if let Some(c) = parse_color(&lower) {
                style.color = c;
            }
        }
        "background-color" | "background" => {
            // `background` is a shorthand; take a bare colour out of it and ignore images.
            if let Some(c) = lower.split_whitespace().find_map(parse_color) {
                style.background_color = c;
            }
        }
        "font-size" => {
            if let Some(px) = parse_font_size(&lower, parent.font_size) {
                style.font_size = px;
            }
        }
        "font-weight" => {
            style.bold = match lower.as_str() {
                "bold" | "bolder" => true,
                "normal" | "lighter" => false,
                other => other.parse::<u32>().map(|n| n >= 600).unwrap_or(style.bold),
            }
        }
        "font-style" => style.italic = lower == "italic" || lower == "oblique",
        "font-family" => {
            // We have exactly two faces: the UI font and a monospace one. Anything that names a
            // monospace family gets the latter; everything else is the default.
            style.monospace = lower.contains("monospace") || lower.contains("courier");
        }
        "text-decoration" | "text-decoration-line" => {
            style.underline = lower.contains("underline");
        }
        "text-align" => {
            style.text_align = match lower.as_str() {
                "center" => TextAlign::Center,
                "right" | "end" => TextAlign::Right,
                _ => TextAlign::Left,
            }
        }
        "white-space" => {
            style.white_space = match lower.as_str() {
                "pre" | "pre-wrap" | "break-spaces" => WhiteSpace::Pre,
                "nowrap" | "pre-line" => WhiteSpace::NoWrap,
                _ => WhiteSpace::Normal,
            }
        }
        "margin" => style.margin = parse_edges(&lower, style.font_size),
        "margin-top" => style.margin.top = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "margin-right" => style.margin.right = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "margin-bottom" => style.margin.bottom = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "margin-left" => style.margin.left = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "padding" => style.padding = parse_edges(&lower, style.font_size),
        "padding-top" => style.padding.top = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "padding-right" => style.padding.right = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "padding-bottom" => style.padding.bottom = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "padding-left" => style.padding.left = parse_px(&lower, style.font_size).unwrap_or(0.0),
        "border" | "border-top" | "border-bottom" | "border-left" | "border-right" => {
            // Shorthand: <width> <style> <color> in any order. `none`/`0` clears it.
            let width = lower
                .split_whitespace()
                .find_map(|t| parse_px(t, style.font_size))
                .unwrap_or(if lower.contains("none") { 0.0 } else { 1.0 });
            let width = if lower.contains("none") || lower.contains("hidden") { 0.0 } else { width };
            match name {
                "border-top" => style.border.top = width,
                "border-bottom" => style.border.bottom = width,
                "border-left" => style.border.left = width,
                "border-right" => style.border.right = width,
                _ => style.border = Edges { top: width, right: width, bottom: width, left: width },
            }
            if let Some(c) = lower.split_whitespace().find_map(parse_color) {
                style.border_color = c;
            }
        }
        "border-width" => style.border = parse_edges(&lower, style.font_size),
        "border-color" => {
            if let Some(c) = parse_color(&lower) {
                style.border_color = c;
            }
        }
        "width" => style.width = parse_length_or_auto(&lower, style.font_size),
        "height" => style.height = parse_length_or_auto(&lower, style.font_size),
        _ => {}
    }
}

fn parse_edges(v: &str, font_size: f32) -> Edges {
    let parts: Vec<f32> = v
        .split_whitespace()
        .map(|t| parse_px(t, font_size).unwrap_or(0.0))
        .collect();
    match parts.len() {
        1 => Edges { top: parts[0], right: parts[0], bottom: parts[0], left: parts[0] },
        2 => Edges { top: parts[0], right: parts[1], bottom: parts[0], left: parts[1] },
        3 => Edges { top: parts[0], right: parts[1], bottom: parts[2], left: parts[1] },
        4 => Edges { top: parts[0], right: parts[1], bottom: parts[2], left: parts[3] },
        _ => Edges::default(),
    }
}

fn parse_px(v: &str, font_size: f32) -> Option<f32> {
    let v = v.trim();
    if v == "0" || v == "auto" {
        return Some(0.0);
    }
    if let Some(n) = v.strip_suffix("px") {
        return n.trim().parse().ok();
    }
    if let Some(n) = v.strip_suffix("em") {
        return n.trim().parse::<f32>().ok().map(|e| e * font_size);
    }
    if let Some(n) = v.strip_suffix("rem") {
        return n.trim().parse::<f32>().ok().map(|e| e * 16.0);
    }
    if let Some(n) = v.strip_suffix("pt") {
        // 1pt = 1/72in, 1px = 1/96in.
        return n.trim().parse::<f32>().ok().map(|p| p * 96.0 / 72.0);
    }
    // A bare number is invalid CSS for a length (except 0) but appears constantly in the wild.
    v.parse().ok()
}

fn parse_length_or_auto(v: &str, font_size: f32) -> LengthOrAuto {
    let v = v.trim();
    if v == "auto" || v.is_empty() {
        return LengthOrAuto::Auto;
    }
    if let Some(p) = v.strip_suffix('%') {
        if let Ok(n) = p.trim().parse::<f32>() {
            return LengthOrAuto::Percent(n);
        }
    }
    parse_px(v, font_size).map(LengthOrAuto::Px).unwrap_or(LengthOrAuto::Auto)
}

fn parse_font_size(v: &str, parent: f32) -> Option<f32> {
    match v {
        "xx-small" => return Some(9.0),
        "x-small" => return Some(10.0),
        "small" => return Some(13.0),
        "medium" => return Some(16.0),
        "large" => return Some(18.0),
        "x-large" => return Some(24.0),
        "xx-large" => return Some(32.0),
        "smaller" => return Some(parent * 0.833),
        "larger" => return Some(parent * 1.2),
        _ => {}
    }
    if let Some(p) = v.strip_suffix('%') {
        return p.trim().parse::<f32>().ok().map(|n| parent * n / 100.0);
    }
    // `em` on font-size resolves against the PARENT's size, not this element's.
    if let Some(n) = v.strip_suffix("em") {
        return n.trim().parse::<f32>().ok().map(|e| e * parent);
    }
    parse_px(v, parent)
}

pub fn parse_color(v: &str) -> Option<Color> {
    let v = v.trim();
    if let Some(hex) = v.strip_prefix('#') {
        return match hex.len() {
            3 => {
                let d = |c: char| c.to_digit(16).map(|n| (n * 17) as u32);
                let mut it = hex.chars();
                let r = d(it.next()?)?;
                let g = d(it.next()?)?;
                let b = d(it.next()?)?;
                Some(0xFF00_0000 | (r << 16) | (g << 8) | b)
            }
            6 => u32::from_str_radix(hex, 16).ok().map(|n| 0xFF00_0000 | n),
            8 => u32::from_str_radix(hex, 16).ok().map(|n| n.rotate_right(8)),
            _ => None,
        };
    }
    if let Some(rest) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")) {
        let rest = rest.trim_end_matches(')');
        let nums: Vec<f32> = rest
            .split(|c| c == ',' || c == ' ' || c == '/')
            .filter(|s| !s.trim().is_empty())
            .filter_map(|s| s.trim().parse::<f32>().ok())
            .collect();
        if nums.len() >= 3 {
            let a = if nums.len() >= 4 { (nums[3] * 255.0) as u32 } else { 255 };
            return Some(
                (a << 24) | ((nums[0] as u32) << 16) | ((nums[1] as u32) << 8) | nums[2] as u32,
            );
        }
        return None;
    }
    named_color(v)
}

fn named_color(name: &str) -> Option<Color> {
    // The colours that actually appear in hand-written HTML. Not the full 148-name list.
    Some(match name {
        "black" => 0xFF_000000,
        "silver" => 0xFF_C0C0C0,
        "gray" | "grey" => 0xFF_808080,
        "white" => 0xFF_FFFFFF,
        "maroon" => 0xFF_800000,
        "red" => 0xFF_FF0000,
        "purple" => 0xFF_800080,
        "fuchsia" | "magenta" => 0xFF_FF00FF,
        "green" => 0xFF_008000,
        "lime" => 0xFF_00FF00,
        "olive" => 0xFF_808000,
        "yellow" => 0xFF_FFFF00,
        "navy" => 0xFF_000080,
        "blue" => 0xFF_0000FF,
        "teal" => 0xFF_008080,
        "aqua" | "cyan" => 0xFF_00FFFF,
        "orange" => 0xFF_FFA500,
        "pink" => 0xFF_FFC0CB,
        "brown" => 0xFF_A52A2A,
        "gold" => 0xFF_FFD700,
        "darkgray" | "darkgrey" => 0xFF_A9A9A9,
        "lightgray" | "lightgrey" => 0xFF_D3D3D3,
        "darkblue" => 0xFF_00008B,
        "darkgreen" => 0xFF_006400,
        "darkred" => 0xFF_8B0000,
        "transparent" => TRANSPARENT,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn style_of(html: &str, css: &str, tag: &str) -> ComputedStyle {
        let dom = Dom::parse(html);
        let sheet = Stylesheet::parse(css);
        let tree = compute(&dom, &sheet);
        let id = dom.find_tag(tag).expect("tag present");
        tree.get(id).clone()
    }

    #[test]
    fn ua_stylesheet_supplies_defaults() {
        let s = style_of("<p>x</p>", "", "p");
        assert_eq!(s.display, Display::Block);
        assert_eq!(s.margin.top, 16.0);
        let h1 = style_of("<h1>x</h1>", "", "h1");
        assert_eq!(h1.font_size, 32.0);
        assert!(h1.bold);
        // head content must not be rendered
        assert_eq!(style_of("<html><head><title>t</title></head><body></body></html>", "", "head").display, Display::None);
    }

    #[test]
    fn author_css_beats_the_ua_sheet() {
        let s = style_of("<p>x</p>", "p { color: red }", "p");
        assert_eq!(s.color, 0xFF_FF0000);
    }

    #[test]
    fn specificity_decides_between_author_rules() {
        let s = style_of(
            "<p class='c' id='i'>x</p>",
            "p { color: red } .c { color: green } #i { color: blue }",
            "p",
        );
        assert_eq!(s.color, 0xFF_0000FF, "#id must win");
    }

    #[test]
    fn source_order_breaks_specificity_ties() {
        let s = style_of("<p>x</p>", "p { color: red } p { color: lime }", "p");
        assert_eq!(s.color, 0xFF_00FF00);
    }

    #[test]
    fn inline_style_beats_selectors_and_important_beats_inline() {
        let s = style_of("<p id='i' style='color: red'>x</p>", "#i { color: blue }", "p");
        assert_eq!(s.color, 0xFF_FF0000, "inline style wins");

        let s = style_of("<p id='i' style='color: red'>x</p>", "#i { color: blue !important }", "p");
        assert_eq!(s.color, 0xFF_0000FF, "!important outranks inline");
    }

    #[test]
    fn inheritance_carries_only_inherited_properties() {
        let dom = Dom::parse("<div style='color: red; margin: 50px'><span>x</span></div>");
        let tree = compute(&dom, &Stylesheet::default());
        let span = dom.find_tag("span").unwrap();
        assert_eq!(tree.get(span).color, 0xFF_FF0000, "color inherits");
        assert_eq!(tree.get(span).margin.top, 0.0, "margin must NOT inherit");
    }

    #[test]
    fn descendant_matching_backtracks() {
        // The nearest `div` ancestor of `<i>` is the inner one, which has no `section` above it
        // other than through the outer div. A greedy walk that stopped at the first `div` and then
        // demanded `section` immediately above it would fail here.
        let dom = Dom::parse("<section><div><div><i>x</i></div></div></section>");
        let sheet = Stylesheet::parse("section div i { color: red }");
        let tree = compute(&dom, &sheet);
        let i = dom.find_tag("i").unwrap();
        assert_eq!(tree.get(i).color, 0xFF_FF0000);
    }

    #[test]
    fn child_and_sibling_combinators() {
        let dom = Dom::parse("<div><p>a</p><span>b</span></div>");
        let tree = compute(&dom, &Stylesheet::parse("div > span { color: red } p + span { color: lime }"));
        let span = dom.find_tag("span").unwrap();
        // Both match; equal specificity (0,0,2) so source order wins -> lime.
        assert_eq!(tree.get(span).color, 0xFF_00FF00);

        // `>` must not match a grandchild.
        let dom = Dom::parse("<div><section><span>b</span></section></div>");
        let tree = compute(&dom, &Stylesheet::parse("div > span { color: red }"));
        let span = dom.find_tag("span").unwrap();
        assert_ne!(tree.get(span).color, 0xFF_FF0000);
    }

    #[test]
    fn attribute_and_pseudo_class_matching() {
        let dom = Dom::parse("<ul><li>a</li><li>b</li></ul>");
        let tree = compute(&dom, &Stylesheet::parse("li:first-child { color: red }"));
        let lis: Vec<_> = dom
            .descendants(dom.root)
            .into_iter()
            .filter(|&n| dom.node(n).tag() == Some("li"))
            .collect();
        assert_eq!(tree.get(lis[0]).color, 0xFF_FF0000);
        assert_ne!(tree.get(lis[1]).color, 0xFF_FF0000);

        let s = style_of(r#"<a href="http://x">l</a>"#, r#"a[href^="http"] { color: lime }"#, "a");
        assert_eq!(s.color, 0xFF_00FF00);
    }

    #[test]
    fn unsupported_pseudo_classes_never_match() {
        // Matching :hover would apply hover styles permanently.
        let s = style_of("<p>x</p>", "p:hover { color: red }", "p");
        assert_ne!(s.color, 0xFF_FF0000);
    }

    #[test]
    fn colors_parse_in_every_common_form() {
        assert_eq!(parse_color("#f00"), Some(0xFF_FF0000));
        assert_eq!(parse_color("#ff0000"), Some(0xFF_FF0000));
        assert_eq!(parse_color("rgb(255, 0, 0)"), Some(0xFF_FF0000));
        assert_eq!(parse_color("red"), Some(0xFF_FF0000));
        assert_eq!(parse_color("nonsense"), None);
    }

    #[test]
    fn lengths_and_font_size_units() {
        let s = style_of("<p style='font-size: 200%'>x</p>", "", "p");
        assert_eq!(s.font_size, 32.0, "% font-size resolves against the parent");
        let s = style_of("<p style='margin: 1em 2em'>x</p>", "", "p");
        assert_eq!(s.margin.top, 16.0);
        assert_eq!(s.margin.left, 32.0);
    }

    #[test]
    fn edges_shorthand_follows_the_1_2_3_4_rule() {
        let e = parse_edges("1px 2px 3px 4px", 16.0);
        assert_eq!((e.top, e.right, e.bottom, e.left), (1.0, 2.0, 3.0, 4.0));
        let e = parse_edges("1px 2px 3px", 16.0);
        assert_eq!((e.top, e.right, e.bottom, e.left), (1.0, 2.0, 3.0, 2.0));
        let e = parse_edges("5px", 16.0);
        assert_eq!((e.top, e.right, e.bottom, e.left), (5.0, 5.0, 5.0, 5.0));
    }
}
