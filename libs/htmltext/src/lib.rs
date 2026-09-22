//! HTML → readable plain text, plus the links found along the way.
//!
//! This is the render half of the terminal's text browser. It is a scanner, not a parser: it walks
//! the byte stream, throws away markup, keeps text, and remembers `href`s. That is a deliberate
//! choice, not a shortcut taken for lack of a parser — `libs/web` had a real html5ever tree builder,
//! and using it here would pull the whole cascade and layout stack into the terminal to do a job
//! that is fundamentally "strip tags and lay the text out for a fixed-pitch screen".
//!
//! What it does get right, because getting these wrong makes the output unreadable rather than
//! merely imperfect:
//!
//! * `<script>` and `<style>` bodies are dropped. They are text nodes, so a naive stripper prints
//!   the whole of jQuery at you.
//! * Block-level tags become line breaks; inline tags do not. Without this the entire page arrives
//!   as one paragraph.
//! * Runs of whitespace collapse to one space, exactly as a real renderer does, because HTML source
//!   is full of newlines and indentation that are not content.
//! * `<` inside a quoted attribute value does not end the tag.
//!
//! ## Structure, not just line breaks
//!
//! The first version turned every block tag into a bare newline, which meant a heading, a list item
//! and a paragraph were all the same thing on screen: a line. That is readable in the sense that the
//! words are present, and unreadable in the sense that you cannot skim it — and skimming is most of
//! what reading a web page is. So the emitter keeps a small amount of state (list nesting, quote
//! depth, preformatted depth) and renders:
//!
//! ```text
//! # Heading                  <- level is in the marker, so it survives wrapping
//!   • a list item            <- nesting indents; <ol> counts instead of bulleting
//!   > a quotation
//!   code stays verbatim      <- inside <pre>, whitespace is content
//!   cell | cell | cell       <- table rows keep their column boundaries
//! ```
//!
//! ## Allocation
//!
//! A page has tens of thousands of tags, and the terminal renders on a machine with no swap and a
//! small heap. The scanner therefore does no allocation per tag: tag names are lowercased into a
//! 16-byte stack buffer, attributes stay as a byte range into the source, and text is entity-decoded
//! straight into the output rather than through a temporary `String` per text node.

use core::ops::Range;

/// A link found in the document, numbered as it appeared.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    /// 1-based, matching the `[n]` marker left in the text.
    pub index: usize,
    /// The raw `href`, which may be relative — resolving it is the caller's job, since only the
    /// caller knows the base URL the document came from.
    pub href: String,
    /// The anchor's visible text, trimmed. Often empty (image links), which is worth showing as
    /// such rather than hiding the link.
    pub text: String,
}

/// The result of rendering a document.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub title: String,
    pub text: String,
    pub links: Vec<Link>,
    /// `<base href>`, if the document declared one. Every relative link on the page resolves against
    /// this instead of against the URL the document came from — ignoring it sends `open <n>` to the
    /// wrong place on any site that uses it, and enough of them do that it is not an edge case.
    pub base: Option<String>,
}

/// How many links one document may contribute.
///
/// Each link is three heap allocations (index, href, text) and the terminal renders the whole list
/// on `links`. A page with a million anchors is not a page anyone is reading; it is a way to make
/// a small download cost a large amount of memory. Text past the cap still renders — only the
/// numbering stops, so the document is truncated in its navigation, not in its content.
pub const MAX_LINKS: usize = 4096;

/// Knobs the caller may want per page.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Drop `<nav>`, `<aside>` and `<footer>` bodies. These are the same forty links on every page
    /// of a site, and in a text browser they arrive *before* the article — so the content you asked
    /// for starts a screen and a half down. Off by default: it is a judgement about the document,
    /// and a judgement that is sometimes wrong should be something the reader turns on.
    pub reader: bool,
}

/// Render `html` to text and collect its links.
pub fn render(html: &str) -> Page {
    render_with(html, &Options::default())
}

/// Render with [`Options`].
pub fn render_with(html: &str, opts: &Options) -> Page {
    let b = html.as_bytes();
    let mut page = Page::default();
    let mut emit = Emit::new(html.len());

    let mut i = 0usize;
    // Set while inside an <a>. The anchor's visible text is accumulated separately rather than
    // sliced back out of the output, because the output may have broken a line (and applied an
    // indent prefix) in the middle of the anchor.
    let mut anchor: Option<(usize, String)> = None;
    let mut anchor_text = String::new();
    let mut in_title = false;
    // Reader mode: the tag we are skipping and how deep we are nested in it. A scanner has no tree,
    // so matching the right close tag means counting opens of the same name.
    let mut skip: Option<(&'static str, usize)> = None;

    while i < b.len() {
        if b[i] != b'<' {
            // Text node. Runs to the next '<' or to the end.
            let start = i;
            while i < b.len() && b[i] != b'<' {
                i += 1;
            }
            let raw = &html[start..i];
            if skip.is_some() {
                // dropped
            } else if in_title {
                push_decoded_into(&mut page.title, raw, true);
            } else {
                // A block break inside an anchor is a word boundary in its label. Noticed here,
                // before the text is emitted, because afterwards the line is no longer fresh —
                // without it `<a><div>one</div><div>two</div></a>` is labelled "onetwo".
                let broke = emit.at_line_start();
                emit.text(raw);
                if anchor.is_some() {
                    if broke && !anchor_text.is_empty() && !anchor_text.ends_with(' ') {
                        anchor_text.push(' ');
                    }
                    push_decoded_into(&mut anchor_text, raw, !emit.in_pre());
                }
            }
            continue;
        }

        let Some(tag) = read_tag(b, i) else {
            // A stray '<' that never closes is literal text, not a tag.
            emit.raw("<");
            i += 1;
            continue;
        };
        let name = tag.name.as_str();
        i = tag.end;

        // Raw-text elements are skipped wholesale: their contents are code, not prose.
        if !tag.closing && (name == "script" || name == "style") {
            i = skip_raw_text(b, tag.end, name);
            continue;
        }

        // Reader mode's skip window. Checked before anything else so a heading inside <nav> does not
        // still emit its marker.
        if let Some((skipped, depth)) = skip.as_mut() {
            if name == *skipped {
                if tag.closing {
                    *depth -= 1;
                    if *depth == 0 {
                        skip = None;
                        emit.break_line();
                    }
                } else {
                    *depth += 1;
                }
            }
            continue;
        }
        if opts.reader && !tag.closing && matches!(name, "nav" | "aside" | "footer") {
            skip = Some((leak_name(name), 1));
            emit.break_line();
            continue;
        }

        match name {
            "title" => in_title = !tag.closing,

            // `<base href>` wins over the document's own URL for relative links. First one only,
            // which is what the HTML spec says and also what stops a stray second one from
            // redirecting the whole page.
            "base" if !tag.closing => {
                if page.base.is_none() {
                    if let Some(href) = attr(b, tag.attrs.clone(), "href") {
                        let h = href.trim();
                        if !h.is_empty() {
                            page.base = Some(h.to_string());
                        }
                    }
                }
            }

            "a" if !tag.closing && page.links.len() < MAX_LINKS => {
                if let Some(href) = attr(b, tag.attrs.clone(), "href") {
                    // Fragments and javascript: URLs are not navigations worth numbering.
                    let h = href.trim();
                    if !h.is_empty()
                        && !h.starts_with('#')
                        && !starts_with_ci(h, "javascript:")
                    {
                        anchor = Some((page.links.len() + 1, h.to_string()));
                        anchor_text.clear();
                    }
                }
            }
            "a" if tag.closing => {
                if let Some((index, href)) = anchor.take() {
                    // The marker goes AFTER the anchor text so the sentence still reads.
                    emit.raw(&format!("[{index}]"));
                    page.links.push(Link {
                        index,
                        href,
                        text: anchor_text.trim().to_string(),
                    });
                    anchor_text.clear();
                }
            }

            // An image with alt text is content; without it, it is noise.
            "img" if !tag.closing => {
                if let Some(alt) = attr(b, tag.attrs.clone(), "alt") {
                    let alt = alt.trim();
                    if !alt.is_empty() {
                        emit.text(&format!("[image: {alt}]"));
                        if anchor.is_some() {
                            anchor_text.push_str(alt);
                        }
                    }
                }
            }

            // ── Structure ───────────────────────────────────────────────────────────────────────
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                emit.break_line();
                if tag.closing {
                    emit.blank_line();
                } else {
                    emit.blank_line();
                    let level = (name.as_bytes()[1] - b'0') as usize;
                    emit.marker(&format!("{} ", "#".repeat(level)));
                }
            }
            "ul" | "ol" if !tag.closing => {
                emit.break_line();
                emit.push_list(name == "ol");
            }
            "ul" | "ol" if tag.closing => {
                emit.break_line();
                emit.pop_list();
                if emit.list_depth() == 0 {
                    emit.blank_line();
                }
            }
            "li" if !tag.closing => {
                emit.break_line();
                let m = emit.next_bullet();
                emit.marker(&m);
            }
            "blockquote" => {
                emit.break_line();
                emit.blank_line();
                if tag.closing {
                    emit.pop_quote();
                } else {
                    emit.push_quote();
                }
            }
            "pre" => {
                emit.break_line();
                emit.blank_line();
                if tag.closing {
                    emit.pop_pre();
                } else {
                    emit.push_pre();
                }
            }
            "hr" => {
                emit.break_line();
                emit.raw("────────────────────────────────────────");
                emit.break_line();
            }
            // A cell boundary is the only thing that makes a table row readable as a row. Emitted
            // before the cell's text, and suppressed at the start of a line so a row does not begin
            // with a stray separator.
            "td" | "th" if !tag.closing => emit.cell(),
            "tr" | "table" | "thead" | "tbody" | "tfoot" | "caption" => emit.break_line(),
            "dt" => {
                emit.break_line();
                if !tag.closing {
                    emit.marker("");
                }
            }
            "dd" if !tag.closing => {
                emit.break_line();
                emit.marker("    ");
            }
            // `<br>` is a line break and nothing more — no paragraph spacing.
            "br" => emit.break_line(),
            "p" | "div" | "section" | "article" | "header" | "footer" | "nav" | "aside" | "main"
            | "form" | "figure" | "figcaption" | "dl" | "details" | "summary" | "fieldset"
            | "address" | "video" | "audio" | "canvas" | "noscript" => {
                emit.break_line();
                // A blank line between paragraphs is the difference between prose and a wall. Cheap
                // here because `blank_line` is idempotent and runs of blanks are squeezed anyway.
                if matches!(name, "p" | "section" | "article" | "figure" | "form" | "address") {
                    emit.blank_line();
                }
            }
            _ => {}
        }
    }

    page.text = emit.finish();
    page
}

/// Sniff a `<meta charset>` declaration out of the head of a document.
///
/// Takes bytes, not `&str`, and that is the whole point: the charset is what tells you how to turn
/// the bytes into text, so it has to be readable *before* any decoding has happened. Only the first
/// 2 KB is scanned, which is where the spec requires it to be and where every real document puts it.
///
/// Returns the label as written (`"utf-8"`, `"windows-1252"`, …), lowercased. Interpreting it is the
/// caller's job — this module knows about HTML, not about code pages.
pub fn sniff_charset(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(2048)];
    let mut i = 0usize;
    while i < head.len() {
        if head[i] != b'<' {
            i += 1;
            continue;
        }
        let Some(tag) = read_tag(head, i) else {
            i += 1;
            continue;
        };
        i = tag.end;
        if tag.closing || tag.name.as_str() != "meta" {
            continue;
        }
        // <meta charset="utf-8">
        if let Some(cs) = attr(head, tag.attrs.clone(), "charset") {
            let cs = cs.trim().to_ascii_lowercase();
            if !cs.is_empty() {
                return Some(cs);
            }
        }
        // <meta http-equiv="content-type" content="text/html; charset=utf-8">
        if let Some(equiv) = attr(head, tag.attrs.clone(), "http-equiv") {
            if equiv.trim().eq_ignore_ascii_case("content-type") {
                if let Some(content) = attr(head, tag.attrs.clone(), "content") {
                    if let Some(cs) = charset_from_content_type(&content) {
                        return Some(cs);
                    }
                }
            }
        }
    }
    None
}

/// Pull `charset=x` out of a MIME type. Shared by the `<meta http-equiv>` path above and by callers
/// holding an HTTP `Content-Type` header, which uses exactly the same syntax.
pub fn charset_from_content_type(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let at = lower.find("charset")?;
    let rest = lower[at + "charset".len()..].trim_start();
    let rest = rest.strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest
        .find(|c: char| c == '"' || c == ';' || c.is_ascii_whitespace())
        .unwrap_or(rest.len());
    let cs = rest[..end].trim();
    if cs.is_empty() {
        None
    } else {
        Some(cs.to_string())
    }
}

// ── The emitter ─────────────────────────────────────────────────────────────────────────────────

/// One entry on the list stack. `ordered` picks between a bullet and a running number.
struct ListLevel {
    ordered: bool,
    counter: usize,
}

/// Builds the output a line at a time.
///
/// Line-at-a-time rather than one flat `String` because every structural feature — indentation,
/// quote markers, bullets — is a property of a *line*, and can only be applied once you know the
/// line has content. A prefix written eagerly at the break would leave trailing `> ` and stray
/// indentation on every empty line a document's `<div>` soup generates.
struct Emit {
    out: String,
    /// The current line's content, without its prefix.
    cur: String,
    /// A one-shot marker (bullet, heading hashes) written after the prefix on the next flush.
    marker: String,
    marker_set: bool,
    lists: Vec<ListLevel>,
    quote: usize,
    pre: usize,
    /// True when the last thing written was whitespace, for HTML's collapsing rule.
    space: bool,
    /// Consecutive blank lines already emitted, so runs squeeze to one without a second pass over
    /// the whole document.
    blanks: usize,
    /// Whether anything non-blank has been emitted yet — leading blank lines are dropped.
    started: bool,
}

impl Emit {
    fn new(hint: usize) -> Emit {
        Emit {
            out: String::with_capacity(hint / 2),
            cur: String::new(),
            marker: String::new(),
            marker_set: false,
            lists: Vec::new(),
            quote: 0,
            pre: 0,
            space: true,
            blanks: 0,
            started: false,
        }
    }

    fn in_pre(&self) -> bool {
        self.pre > 0
    }

    /// Nothing has been written to the line being built yet.
    fn at_line_start(&self) -> bool {
        self.cur.is_empty()
    }

    fn list_depth(&self) -> usize {
        self.lists.len()
    }

    /// Append a text node, decoding entities and applying HTML's whitespace rules — except inside
    /// `<pre>`, where whitespace *is* the content and a newline ends the line.
    fn text(&mut self, s: &str) {
        if self.pre > 0 {
            let decoded = decode_entities(s);
            for ch in decoded.chars() {
                if ch == '\n' {
                    self.flush(false);
                } else {
                    self.cur.push(ch);
                }
            }
            self.space = false;
            return;
        }
        let mut space = self.space || self.cur.is_empty();
        push_decoded(&mut self.cur, s, &mut space);
        self.space = space;
    }

    /// Append literal characters with no entity decoding and no collapsing — link markers, rules.
    fn raw(&mut self, s: &str) {
        self.cur.push_str(s);
        self.space = s.ends_with(' ');
    }

    /// Set the one-shot prefix for the line currently being built.
    fn marker(&mut self, m: &str) {
        self.marker.clear();
        self.marker.push_str(m);
        self.marker_set = true;
    }

    /// A table cell boundary. Suppressed at the start of a line so a row never opens with ` | `.
    fn cell(&mut self) {
        if !self.cur.trim().is_empty() {
            self.cur.push_str(" | ");
            self.space = true;
        }
    }

    fn push_list(&mut self, ordered: bool) {
        self.lists.push(ListLevel { ordered, counter: 0 });
    }

    fn pop_list(&mut self) {
        self.lists.pop();
    }

    /// The marker for the next `<li>`, and the bookkeeping that makes `<ol>` count.
    ///
    /// An `<li>` outside any list is not an error worth dropping content over — plenty of documents
    /// have one — so it gets a bullet at depth zero.
    fn next_bullet(&mut self) -> String {
        let indent = "  ".repeat(self.lists.len().saturating_sub(1));
        match self.lists.last_mut() {
            Some(l) if l.ordered => {
                l.counter += 1;
                format!("{indent}{}. ", l.counter)
            }
            Some(_) => format!("{indent}• "),
            None => "• ".to_string(),
        }
    }

    fn push_quote(&mut self) {
        self.quote += 1;
    }

    fn pop_quote(&mut self) {
        self.quote = self.quote.saturating_sub(1);
    }

    fn push_pre(&mut self) {
        self.pre += 1;
    }

    fn pop_pre(&mut self) {
        self.pre = self.pre.saturating_sub(1);
    }

    /// End the current line if it has anything on it.
    fn break_line(&mut self) {
        self.flush(false);
    }

    /// Ask for one blank line here. Idempotent, and a no-op before any content.
    fn blank_line(&mut self) {
        self.flush(false);
        if self.started && self.blanks == 0 {
            self.out.push('\n');
            self.blanks = 1;
        }
    }

    /// Write `prefix + marker + cur` out as a line. `force` emits even an empty line, which only the
    /// `<pre>` path wants (a blank line inside a code block is content).
    fn flush(&mut self, force: bool) {
        let has_content = !self.cur.trim_end().is_empty() || self.marker_set;
        if !has_content && !force {
            self.marker_set = false;
            self.marker.clear();
            return;
        }

        // Quote markers, then list indentation. That order matches how the two nest in practice: a
        // list inside a quotation is indented *within* the quote, not beside it.
        for _ in 0..self.quote {
            self.out.push_str("> ");
        }
        if !self.marker_set && !self.lists.is_empty() {
            // A continuation line inside a list item lines up under its bullet.
            self.out.push_str(&"  ".repeat(self.lists.len()));
        }
        self.out.push_str(&self.marker);

        let body = if self.pre > 0 { self.cur.as_str() } else { self.cur.trim_end() };
        self.out.push_str(body);
        self.out.push('\n');

        self.started = true;
        self.blanks = 0;
        self.cur.clear();
        self.marker.clear();
        self.marker_set = false;
        self.space = true;
    }

    fn finish(mut self) -> String {
        self.flush(false);
        // Trailing blank lines serve nothing; a leading one was never emitted (see `blank_line`).
        while self.out.ends_with("\n\n") {
            self.out.pop();
        }
        self.out
    }
}

// ── The scanner ─────────────────────────────────────────────────────────────────────────────────

/// A tag name, lowercased into a fixed buffer.
///
/// No allocation: a page has tens of thousands of tags, and a `String` per tag was two heap
/// operations each for a value that lives for three lines. Sixteen bytes is comfortably more than
/// the longest real HTML tag name (`blockquote`, `figcaption` — ten), and anything longer is not a
/// tag we handle, so truncating to "unknown" loses nothing.
struct TagName {
    buf: [u8; 16],
    len: usize,
}

impl TagName {
    fn as_str(&self) -> &str {
        // Always ASCII: `read_tag` only accepts alphanumerics and '-' into the buffer.
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

struct Tag {
    name: TagName,
    /// Byte range of the attribute text within the source, so no copy is made for the (many) tags
    /// whose attributes are never looked at.
    attrs: Range<usize>,
    /// Index just past the '>'.
    end: usize,
    closing: bool,
}

/// Parse the tag starting at `b[i] == b'<'`.
fn read_tag(b: &[u8], i: usize) -> Option<Tag> {
    let mut j = i + 1;
    if j >= b.len() {
        return None;
    }

    // Comments, doctypes and CDATA: skip to the appropriate terminator rather than treating them as
    // tags. A comment containing '>' would otherwise leak its tail as text.
    if b[j] == b'!' {
        let end = if b[j..].starts_with(b"!--") {
            find(b, j + 3, b"-->").map(|k| k + 3).unwrap_or(b.len())
        } else {
            memchr(b, j, b'>').map(|k| k + 1).unwrap_or(b.len())
        };
        return Some(Tag { name: TagName { buf: [0; 16], len: 0 }, attrs: end..end, end, closing: false });
    }

    let closing = b[j] == b'/';
    if closing {
        j += 1;
    }

    let name_start = j;
    let mut name = TagName { buf: [0; 16], len: 0 };
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
        if name.len < name.buf.len() {
            name.buf[name.len] = b[j].to_ascii_lowercase();
            name.len += 1;
        }
        j += 1;
    }
    if j == name_start {
        return None; // '<' followed by something that is not a tag name
    }
    // Overlong = not a tag we know. Report it as unnamed rather than as a truncated match, or
    // `<sectionxxxxxxxxxxx>` would be handled as `<sectionxxxxxxxx>`.
    if j - name_start > name.buf.len() {
        name.len = 0;
    }

    // Scan to '>', honouring quotes so `<a title="a > b">` does not end early.
    let attr_start = j;
    let mut quote = 0u8;
    while j < b.len() {
        let c = b[j];
        if quote != 0 {
            if c == quote {
                quote = 0;
            }
        } else if c == b'"' || c == b'\'' {
            quote = c;
        } else if c == b'>' {
            break;
        }
        j += 1;
    }
    let attrs = attr_start..j.min(b.len());
    Some(Tag { name, attrs, end: (j + 1).min(b.len()), closing })
}

/// Everything up to the matching close tag of a raw-text element.
fn skip_raw_text(b: &[u8], from: usize, name: &str) -> usize {
    let needle = format!("</{name}");
    let n = needle.as_bytes();
    let mut i = from;
    while i + n.len() <= b.len() {
        if b[i..i + n.len()].eq_ignore_ascii_case(n) {
            return memchr(b, i, b'>').map(|k| k + 1).unwrap_or(b.len());
        }
        i += 1;
    }
    b.len()
}

/// Pull one attribute's value out of a tag's attribute range.
///
/// Walks the attribute list properly — name, optional `=`, optionally quoted value — rather than
/// searching the text for the name. Two bugs live in the searching version, and both are the kind
/// that silently navigate somewhere the page never linked to:
///
/// * `href` matches inside `data-href`. A leading-boundary check fixes that one.
/// * `charset` matches inside `content="text/html; charset=ISO-8859-1"`, because the space before it
///   satisfies the boundary check. No amount of boundary checking fixes that: the match is inside a
///   quoted value, and only something that tracks quoting knows it.
///
/// It also allocates nothing per lookup. The first version lowercased the whole attribute text into
/// a fresh `String` every time, which is an allocation per `<a>` on a page that may have thousands.
fn attr(b: &[u8], range: Range<usize>, want: &str) -> Option<String> {
    let (lo, hi) = (range.start, range.end);
    let w = want.as_bytes();
    let mut k = lo;

    loop {
        // Leading whitespace, and the '/' of a self-closing tag, separate attributes.
        while k < hi && (b[k].is_ascii_whitespace() || b[k] == b'/') {
            k += 1;
        }
        if k >= hi {
            return None;
        }

        let name_start = k;
        while k < hi && !b[k].is_ascii_whitespace() && b[k] != b'=' && b[k] != b'/' {
            k += 1;
        }
        let name_end = k;
        if name_end == name_start {
            k += 1; // not a name; step over it rather than spinning
            continue;
        }

        // The value is optional — `<input disabled>` is a valueless attribute, and treating it as
        // one is what keeps the scan aligned with the attributes that follow it.
        let (mut value_start, mut value_end) = (name_end, name_end);
        let mut probe = k;
        while probe < hi && b[probe].is_ascii_whitespace() {
            probe += 1;
        }
        if probe < hi && b[probe] == b'=' {
            k = probe + 1;
            while k < hi && b[k].is_ascii_whitespace() {
                k += 1;
            }
            if k < hi && (b[k] == b'"' || b[k] == b'\'') {
                let q = b[k];
                k += 1;
                value_start = k;
                while k < hi && b[k] != q {
                    k += 1;
                }
                value_end = k;
                if k < hi {
                    k += 1; // past the closing quote
                }
            } else {
                value_start = k;
                while k < hi && !b[k].is_ascii_whitespace() {
                    k += 1;
                }
                value_end = k;
            }
        }

        if b[name_start..name_end].eq_ignore_ascii_case(w) {
            // Attribute values are almost always ASCII, but `alt` text is prose and can be anything.
            return Some(decode_entities(&String::from_utf8_lossy(&b[value_start..value_end])));
        }
    }
}

// ── Entities ────────────────────────────────────────────────────────────────────────────────────

/// Decode entities straight into `out`, applying HTML's whitespace collapsing as it goes.
///
/// One pass, no temporary. The previous shape built a decoded `String` per text node and then walked
/// it again to collapse — two allocations and two passes over every scrap of text on the page.
fn push_decoded(out: &mut String, s: &str, space: &mut bool) {
    let mut rest = s;
    loop {
        let cut = rest.find('&').unwrap_or(rest.len());
        push_collapsed(out, &rest[..cut], space);
        if cut == rest.len() {
            return;
        }
        let tail = &rest[cut..];
        match take_entity(tail) {
            Some((ch, used)) => {
                // A decoded &nbsp; is whitespace and must collapse with its neighbours, or a run of
                // them (which is how pages fake indentation) becomes a wall of spaces.
                if ch.is_whitespace() {
                    if !*space {
                        out.push(' ');
                        *space = true;
                    }
                } else {
                    out.push(ch);
                    *space = false;
                }
                rest = &tail[used..];
            }
            None => {
                out.push('&');
                *space = false;
                rest = &tail[1..];
            }
        }
    }
}

/// The same, without collapsing, for the title and for anchor text captured alongside the body.
fn push_decoded_into(out: &mut String, s: &str, collapse: bool) {
    if collapse {
        let mut space = out.is_empty() || out.ends_with([' ', '\n']);
        push_decoded(out, s, &mut space);
    } else {
        out.push_str(&decode_entities(s));
    }
}

fn push_collapsed(out: &mut String, s: &str, space: &mut bool) {
    for c in s.chars() {
        if c.is_whitespace() {
            if !*space {
                out.push(' ');
                *space = true;
            }
        } else {
            out.push(c);
            *space = false;
        }
    }
}

/// Decode a whole string's entities into a new `String`. Used where an owned value is wanted anyway
/// (attribute values), not on the hot text path.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        match take_entity(tail) {
            Some((ch, used)) => {
                out.push(ch);
                rest = &tail[used..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Decode the entity at the start of `tail` (which begins with '&'). Returns the character and how
/// many bytes it consumed, or `None` if this is a bare ampersand.
///
/// The named table covers the entities that actually appear in prose. A full table is not the point;
/// leaving `&amp;` on screen is, because it makes text look broken.
fn take_entity(tail: &str) -> Option<(char, usize)> {
    // A `;` more than a dozen bytes away is not an entity, it is a bare '&' followed by text.
    let semi = tail[..tail.len().min(12)].find(';')?;
    let name = &tail[1..semi];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#39" => '\'',
        "nbsp" | "#160" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "#8217" | "rsquo" => '\'',
        "#8216" | "lsquo" => '\'',
        "#8220" | "ldquo" => '"',
        "#8221" | "rdquo" => '"',
        "middot" => '·',
        "bull" => '•',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "times" => '×',
        "laquo" => '«',
        "raquo" => '»',
        _ => latin1_entity(name).or_else(|| numeric_entity(name))?,
    };
    Some((ch, semi + 1))
}

/// The HTML Latin-1 named entities, which are exactly the characters U+00A0–U+00FF in order.
///
/// Worth having as a table rather than as a handful of hand-picked cases: these are how every page
/// in a European language writes its accented letters, and an undecoded one does not degrade
/// gracefully — it puts a literal `na&iuml;ve` on screen, which reads as the renderer being broken
/// rather than as one missing character.
///
/// The index into this array is the codepoint minus 0xA0. `nbsp` is listed for completeness but is
/// matched earlier, where it becomes a plain space so that runs of them collapse.
const LATIN1_ENTITIES: [&str; 96] = [
    "nbsp", "iexcl", "cent", "pound", "curren", "yen", "brvbar", "sect", "uml", "copy", "ordf",
    "laquo", "not", "shy", "reg", "macr", "deg", "plusmn", "sup2", "sup3", "acute", "micro", "para",
    "middot", "cedil", "sup1", "ordm", "raquo", "frac14", "frac12", "frac34", "iquest", "Agrave",
    "Aacute", "Acirc", "Atilde", "Auml", "Aring", "AElig", "Ccedil", "Egrave", "Eacute", "Ecirc",
    "Euml", "Igrave", "Iacute", "Icirc", "Iuml", "ETH", "Ntilde", "Ograve", "Oacute", "Ocirc",
    "Otilde", "Ouml", "times", "Oslash", "Ugrave", "Uacute", "Ucirc", "Uuml", "Yacute", "THORN",
    "szlig", "agrave", "aacute", "acirc", "atilde", "auml", "aring", "aelig", "ccedil", "egrave",
    "eacute", "ecirc", "euml", "igrave", "iacute", "icirc", "iuml", "eth", "ntilde", "ograve",
    "oacute", "ocirc", "otilde", "ouml", "divide", "oslash", "ugrave", "uacute", "ucirc", "uuml",
    "yacute", "thorn", "yuml",
];

fn latin1_entity(name: &str) -> Option<char> {
    // Case-sensitive on purpose: `&Auml;` and `&auml;` are different characters, and treating them
    // as the same would turn German nouns into their own lowercase.
    let i = LATIN1_ENTITIES.iter().position(|&e| e == name)?;
    char::from_u32(0xA0 + i as u32)
}

fn numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

// ── Small helpers ───────────────────────────────────────────────────────────────────────────────

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

/// Reader mode stores which tag it is skipping. The names come from a fixed set, so borrowing a
/// `'static` one keeps the skip state `Copy`-cheap instead of allocating per skipped section.
fn leak_name(name: &str) -> &'static str {
    match name {
        "nav" => "nav",
        "aside" => "aside",
        "footer" => "footer",
        _ => "",
    }
}

fn memchr(b: &[u8], from: usize, needle: u8) -> Option<usize> {
    b.get(from..)?.iter().position(|&c| c == needle).map(|p| p + from)
}

fn find(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= b.len() {
        return None;
    }
    (from..=b.len().saturating_sub(needle.len())).find(|&i| &b[i..i + needle.len()] == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Body lines with blank lines removed — most structural assertions care about the sequence of
    /// non-empty lines, not about how much air is between them.
    fn lines(p: &Page) -> Vec<&str> {
        p.text.lines().filter(|l| !l.trim().is_empty()).collect()
    }

    #[test]
    fn plain_text_survives() {
        assert_eq!(render("<p>hello world</p>").text.trim(), "hello world");
    }

    #[test]
    fn script_and_style_bodies_are_dropped() {
        // The failure this guards: a naive stripper prints the whole of a page's JavaScript,
        // because script contents are a text node like any other.
        let p = render("<p>before</p><script>var x = 1 < 2;</script><style>p{color:red}</style><p>after</p>");
        assert!(!p.text.contains("var x"), "script body leaked: {:?}", p.text);
        assert!(!p.text.contains("color"), "style body leaked: {:?}", p.text);
        assert!(p.text.contains("before") && p.text.contains("after"));
    }

    #[test]
    fn block_tags_break_lines_and_inline_tags_do_not() {
        // Paragraphs are separated by a blank line, which is what makes a page skimmable rather
        // than a wall — so the assertion is on the non-blank lines.
        let p = render("<p>one</p><p>two</p>");
        assert_eq!(lines(&p), vec!["one", "two"]);
        assert!(p.text.contains("one\n\ntwo"), "no paragraph spacing: {:?}", p.text);
        let inline = render("<p>a <b>bold</b> word</p>");
        assert_eq!(inline.text.trim(), "a bold word");
    }

    #[test]
    fn whitespace_collapses_like_a_real_renderer() {
        let p = render("<p>a\n\n   lot    of\tspace</p>");
        assert_eq!(p.text.trim(), "a lot of space");
    }

    #[test]
    fn entities_are_decoded() {
        let p = render("<p>a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#39; &#x41;</p>");
        assert_eq!(p.text.trim(), "a & b <c> \"d\" 'e' A");
    }

    #[test]
    fn the_latin1_named_entities_all_decode() {
        // The table is positional, so an insertion anywhere shifts every entity after it — this
        // checks both ends and the boundaries rather than trusting the transcription.
        let p = render("<p>na&iuml;ve caf&eacute; Stra&szlig;e &Auml;pfel &frac12; &yuml; &iexcl;</p>");
        assert_eq!(p.text.trim(), "naïve café Straße Äpfel ½ ÿ ¡");

        // Case matters: &Auml; and &auml; are different letters.
        assert_eq!(render("<p>&Ouml;&ouml;</p>").text.trim(), "Öö");

        // Every entry must land on its own codepoint, in order.
        for (i, name) in LATIN1_ENTITIES.iter().enumerate() {
            let want = char::from_u32(0xA0 + i as u32).unwrap();
            assert_eq!(latin1_entity(name), Some(want), "{name} is at the wrong index");
        }
    }

    #[test]
    fn an_unknown_entity_is_left_alone_rather_than_eaten() {
        let p = render("<p>50 &widget; of it</p>");
        assert!(p.text.contains("&widget;"), "{:?}", p.text);
    }

    #[test]
    fn a_run_of_nbsp_collapses_instead_of_becoming_a_wall_of_spaces() {
        // Pages fake indentation with these. Decoded without collapsing, one line of a menu can
        // arrive forty columns to the right of the others.
        let p = render("<p>a&nbsp;&nbsp;&nbsp;&nbsp;b</p>");
        assert_eq!(p.text.trim(), "a b");
    }

    #[test]
    fn links_are_numbered_and_collected() {
        let p = render(r#"<p>see <a href="/a">first</a> and <a href="/b">second</a></p>"#);
        assert_eq!(p.links.len(), 2);
        assert_eq!(p.links[0], Link { index: 1, href: "/a".into(), text: "first".into() });
        assert_eq!(p.links[1].href, "/b");
        assert!(p.text.contains("first[1]"), "{:?}", p.text);
        assert!(p.text.contains("second[2]"), "{:?}", p.text);
    }

    #[test]
    fn fragment_and_javascript_links_are_not_numbered() {
        // r##..##: the href `"#top"` contains `"#`, which closes a plain r#".."# early.
        let p = render(r##"<a href="#top">top</a><a href="javascript:void(0)">x</a><a href="/real">r</a>"##);
        assert_eq!(p.links.len(), 1);
        assert_eq!(p.links[0].href, "/real");
    }

    #[test]
    fn a_quoted_angle_bracket_does_not_end_the_tag() {
        let p = render(r#"<a href="/x" title="a > b">link</a>"#);
        assert_eq!(p.links.len(), 1);
        assert_eq!(p.links[0].href, "/x");
        assert!(!p.text.contains("b\">"), "tag tail leaked as text: {:?}", p.text);
    }

    #[test]
    fn data_href_is_not_mistaken_for_href() {
        // `attrs.find("href")` without a boundary check matches inside `data-href`, which silently
        // navigates somewhere the page never linked to.
        let p = render(r#"<a data-href="/wrong" href="/right">x</a>"#);
        assert_eq!(p.links[0].href, "/right");
    }

    #[test]
    fn unquoted_attribute_values_work() {
        let p = render("<a href=/bare>x</a>");
        assert_eq!(p.links[0].href, "/bare");
    }

    #[test]
    fn an_anchor_broken_across_a_block_still_captures_its_text() {
        // The old renderer sliced the anchor's text back out of the output buffer by byte offset,
        // so a block break inside the anchor swept the newline and any indent into the link label.
        let p = render(r#"<a href="/x"><div>one</div><div>two</div></a>"#);
        assert_eq!(p.links.len(), 1);
        assert_eq!(p.links[0].text, "one two");
    }

    #[test]
    fn comments_are_dropped_including_any_angle_brackets_inside() {
        let p = render("<p>a</p><!-- a > b, and <p>fake</p> --><p>b</p>");
        assert!(!p.text.contains("fake"), "{:?}", p.text);
        assert_eq!(lines(&p), vec!["a", "b"]);
    }

    #[test]
    fn the_title_is_extracted_and_kept_out_of_the_body() {
        let p = render("<html><head><title>My Page</title></head><body><p>hi</p></body></html>");
        assert_eq!(p.title, "My Page");
        assert_eq!(p.text.trim(), "hi");
    }

    #[test]
    fn image_alt_text_is_shown_and_empty_alt_is_not() {
        assert!(render(r#"<img alt="a cat">"#).text.contains("[image: a cat]"));
        assert_eq!(render(r#"<img alt="" src="x.png">"#).text.trim(), "");
    }

    #[test]
    fn a_stray_less_than_is_literal_text() {
        let p = render("<p>5 < 6 is true</p>");
        assert!(p.text.contains("5 < 6"), "{:?}", p.text);
    }

    #[test]
    fn runs_of_blank_lines_are_squeezed() {
        let p = render("<p>a</p><div></div><div></div><div></div><p>b</p>");
        assert!(!p.text.contains("\n\n\n"), "{:?}", p.text);
    }

    #[test]
    fn an_unterminated_tag_does_not_hang_or_panic() {
        let p = render("<p>text<a href=\"/x\"");
        assert!(p.text.contains("text"));
    }

    #[test]
    fn empty_input_is_empty_output() {
        let p = render("");
        assert!(p.text.is_empty() && p.links.is_empty() && p.title.is_empty());
    }

    #[test]
    fn multibyte_text_is_not_split() {
        let p = render("<p>héllo — wörld … 日本語</p>");
        assert_eq!(p.text.trim(), "héllo — wörld … 日本語");
    }

    // ── Structure ───────────────────────────────────────────────────────────────────────────────

    #[test]
    fn headings_carry_their_level() {
        // Without this a heading and a paragraph are the same thing on screen, and the page cannot
        // be skimmed at all.
        let p = render("<h1>Title</h1><p>body</p><h3>Sub</h3>");
        assert_eq!(lines(&p), vec!["# Title", "body", "### Sub"]);
    }

    #[test]
    fn unordered_lists_get_bullets_and_ordered_lists_count() {
        let p = render("<ul><li>a</li><li>b</li></ul>");
        assert_eq!(lines(&p), vec!["• a", "• b"]);
        let o = render("<ol><li>first</li><li>second</li><li>third</li></ol>");
        assert_eq!(lines(&o), vec!["1. first", "2. second", "3. third"]);
    }

    #[test]
    fn nested_lists_indent() {
        let p = render("<ul><li>outer</li><ul><li>inner</li></ul></ul>");
        assert_eq!(lines(&p), vec!["• outer", "  • inner"]);
    }

    #[test]
    fn a_second_ordered_list_starts_from_one_again() {
        let p = render("<ol><li>a</li></ol><ol><li>b</li></ol>");
        assert_eq!(lines(&p), vec!["1. a", "1. b"]);
    }

    #[test]
    fn pre_keeps_its_whitespace() {
        // Everywhere else runs of spaces collapse. Inside <pre> they are the content — collapsing
        // them turns every code sample on the web into one unreadable line.
        let p = render("<pre>fn main() {\n    let x = 1;\n}</pre>");
        assert!(p.text.contains("    let x = 1;"), "indentation lost: {:?}", p.text);
        assert_eq!(lines(&p), vec!["fn main() {", "    let x = 1;", "}"]);
    }

    #[test]
    fn blockquotes_are_marked() {
        let p = render("<p>said</p><blockquote><p>the quote</p></blockquote><p>after</p>");
        assert_eq!(lines(&p), vec!["said", "> the quote", "after"]);
    }

    #[test]
    fn table_cells_keep_their_boundaries() {
        let p = render("<table><tr><td>a</td><td>b</td></tr><tr><td>c</td><td>d</td></tr></table>");
        assert_eq!(lines(&p), vec!["a | b", "c | d"]);
    }

    #[test]
    fn a_row_does_not_open_with_a_stray_separator() {
        let p = render("<table><tr><td>only</td></tr></table>");
        assert_eq!(lines(&p), vec!["only"]);
    }

    #[test]
    fn an_hr_draws_a_rule() {
        let p = render("<p>a</p><hr><p>b</p>");
        assert!(p.text.contains("────"), "{:?}", p.text);
    }

    #[test]
    fn br_breaks_without_adding_a_blank_line() {
        let p = render("a<br>b");
        assert_eq!(p.text, "a\nb\n");
    }

    // ── base, charset, reader ───────────────────────────────────────────────────────────────────

    #[test]
    fn base_href_is_captured() {
        let p = render(r#"<head><base href="https://cdn.example/app/"></head><a href="x">y</a>"#);
        assert_eq!(p.base.as_deref(), Some("https://cdn.example/app/"));
    }

    #[test]
    fn only_the_first_base_counts() {
        let p = render(r#"<base href="/one/"><base href="/two/">"#);
        assert_eq!(p.base.as_deref(), Some("/one/"));
    }

    #[test]
    fn a_document_without_a_base_reports_none() {
        assert!(render("<p>hi</p>").base.is_none());
    }

    #[test]
    fn charset_is_sniffed_from_bytes_before_decoding() {
        // Has to work on bytes: the charset is what tells you how to make text out of them.
        assert_eq!(
            sniff_charset(br#"<html><head><meta charset="windows-1252"></head>"#).as_deref(),
            Some("windows-1252")
        );
        assert_eq!(
            sniff_charset(br#"<meta http-equiv="Content-Type" content="text/html; charset=ISO-8859-1">"#)
                .as_deref(),
            Some("iso-8859-1")
        );
        assert_eq!(sniff_charset(b"<html><body>no declaration</body>"), None);
    }

    #[test]
    fn charset_is_pulled_out_of_a_content_type_header() {
        assert_eq!(
            charset_from_content_type("text/html; charset=utf-8").as_deref(),
            Some("utf-8")
        );
        assert_eq!(
            charset_from_content_type("text/html;charset=\"Windows-1251\"").as_deref(),
            Some("windows-1251")
        );
        assert_eq!(charset_from_content_type("text/html"), None);
    }

    #[test]
    fn reader_mode_drops_navigation_chrome() {
        let html = "<nav><a href=\"/a\">nav link</a></nav><p>the article</p><footer>small print</footer>";
        let plain = render(html);
        assert!(plain.text.contains("nav link") && plain.text.contains("small print"));

        let read = render_with(html, &Options { reader: true });
        assert!(!read.text.contains("nav link"), "{:?}", read.text);
        assert!(!read.text.contains("small print"), "{:?}", read.text);
        assert!(read.text.contains("the article"));
    }

    #[test]
    fn reader_mode_counts_nesting_so_it_stops_at_the_right_close_tag() {
        // A <nav> inside a <nav> must not end the skip early and spill the outer one's links.
        let read = render_with(
            "<nav>outer<nav>inner</nav>still outer</nav><p>body</p>",
            &Options { reader: true },
        );
        assert_eq!(read.text.trim(), "body");
    }

    #[test]
    fn an_overlong_tag_name_is_not_truncated_into_a_match() {
        // `<sectionxxxxxxxxxx>` must not be handled as `<section>`.
        let p = render("<sectionxxxxxxxxxxxxx>a</sectionxxxxxxxxxxxxx>b");
        assert_eq!(p.text.trim(), "ab");
    }
}
