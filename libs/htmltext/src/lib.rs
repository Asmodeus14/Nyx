//! HTML → readable plain text, plus the links found along the way.
//!
//! This is the render half of the terminal's text browser. It is a scanner, not a parser: it walks
//! the byte stream, throws away markup, keeps text, and remembers `href`s. That is a deliberate
//! choice, not a shortcut taken for lack of a parser — `libs/web` has a real html5ever tree builder,
//! and using it here would pull the whole cascade and layout stack into the terminal to do a job
//! that is fundamentally "strip tags and collapse whitespace".
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
}

/// Tags after which a line break belongs. Everything else is treated as inline.
const BLOCK: &[&str] = &[
    "p", "div", "br", "hr", "li", "ul", "ol", "tr", "table", "section", "article", "header",
    "footer", "nav", "aside", "main", "blockquote", "pre", "form", "figure", "figcaption",
    "h1", "h2", "h3", "h4", "h5", "h6", "dt", "dd", "dl",
];

/// Render `html` to text and collect its links.
pub fn render(html: &str) -> Page {
    let b = html.as_bytes();
    let mut out = String::with_capacity(html.len() / 2);
    let mut page = Page::default();

    let mut i = 0usize;
    // Set while inside an <a>, so the anchor's text can be captured for the link list.
    let mut anchor: Option<(usize, String, usize)> = None; // (index, href, text start in `out`)
    let mut in_title = false;

    while i < b.len() {
        if b[i] == b'<' {
            let Some((name, attrs, end, closing)) = read_tag(b, i) else {
                // A stray '<' that never closes is literal text, not a tag.
                push_char(&mut out, '<');
                i += 1;
                continue;
            };

            match name.as_str() {
                // Skip the raw-text elements wholesale. Their contents are code, not prose.
                "script" | "style" if !closing => {
                    i = skip_raw_text(b, end, &name);
                    continue;
                }
                "title" => {
                    in_title = !closing;
                }
                "a" if !closing => {
                    if let Some(href) = attr(&attrs, "href") {
                        // Fragments and javascript: URLs are not navigations worth numbering.
                        let h = href.trim();
                        if !h.is_empty()
                            && !h.starts_with('#')
                            && !h.to_ascii_lowercase().starts_with("javascript:")
                        {
                            let idx = page.links.len() + 1;
                            anchor = Some((idx, h.to_string(), out.len()));
                        }
                    }
                }
                "a" if closing => {
                    if let Some((index, href, start)) = anchor.take() {
                        let text = out[start.min(out.len())..].trim().to_string();
                        // The marker goes AFTER the anchor text so the sentence still reads.
                        out.push_str(&format!("[{index}]"));
                        page.links.push(Link { index, href, text });
                    }
                }
                // An image with alt text is content; without it, it is noise.
                "img" if !closing => {
                    if let Some(alt) = attr(&attrs, "alt") {
                        if !alt.trim().is_empty() {
                            push_str_collapsed(&mut out, &format!("[image: {}]", alt.trim()));
                        }
                    }
                }
                _ => {}
            }

            if BLOCK.contains(&name.as_str()) {
                push_break(&mut out);
            }
            i = end;
            continue;
        }

        // Text node.
        let start = i;
        while i < b.len() && b[i] != b'<' {
            i += 1;
        }
        let raw = &html[start..i];
        let decoded = decode_entities(raw);
        if in_title {
            page.title.push_str(decoded.trim());
        } else {
            push_str_collapsed(&mut out, &decoded);
        }
    }

    page.text = tidy(&out);
    page
}

/// Parse the tag starting at `b[i] == b'<'`. Returns `(lowercase name, attr text, index after '>',
/// is_closing)`.
fn read_tag(b: &[u8], i: usize) -> Option<(String, String, usize, bool)> {
    let mut j = i + 1;
    if j >= b.len() {
        return None;
    }

    // Comments, doctypes and CDATA: skip to the appropriate terminator rather than treating them
    // as tags. A comment containing '>' would otherwise leak its tail as text.
    if b[j] == b'!' {
        if b[j..].starts_with(b"!--") {
            let end = find(b, j + 3, b"-->").map(|k| k + 3).unwrap_or(b.len());
            return Some((String::new(), String::new(), end, false));
        }
        let end = memchr(b, j, b'>').map(|k| k + 1).unwrap_or(b.len());
        return Some((String::new(), String::new(), end, false));
    }

    let closing = b[j] == b'/';
    if closing {
        j += 1;
    }

    let name_start = j;
    while j < b.len() && (b[j].is_ascii_alphanumeric() || b[j] == b'-') {
        j += 1;
    }
    if j == name_start {
        return None; // '<' followed by something that is not a tag name
    }
    let name = String::from_utf8_lossy(&b[name_start..j]).to_ascii_lowercase();

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
    let attrs = String::from_utf8_lossy(&b[attr_start..j.min(b.len())]).to_string();
    Some((name, attrs, (j + 1).min(b.len()), closing))
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

/// Pull one attribute's value out of a tag's attribute text.
fn attr(attrs: &str, want: &str) -> Option<String> {
    let lower = attrs.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(pos) = lower[from..].find(want) {
        let at = from + pos;
        // Must be preceded by whitespace or start-of-attrs, or `href` matches inside `data-href`.
        let boundary = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        let mut k = at + want.len();
        while k < lower.len() && lower.as_bytes()[k].is_ascii_whitespace() {
            k += 1;
        }
        if boundary && k < lower.len() && lower.as_bytes()[k] == b'=' {
            k += 1;
            while k < lower.len() && lower.as_bytes()[k].is_ascii_whitespace() {
                k += 1;
            }
            if k >= attrs.len() {
                return None;
            }
            let q = attrs.as_bytes()[k];
            let value = if q == b'"' || q == b'\'' {
                let start = k + 1;
                let end = attrs[start..].find(q as char).map(|d| start + d).unwrap_or(attrs.len());
                &attrs[start..end]
            } else {
                let start = k;
                let end = attrs[start..]
                    .find(|c: char| c.is_ascii_whitespace())
                    .map(|d| start + d)
                    .unwrap_or(attrs.len());
                &attrs[start..end]
            };
            return Some(decode_entities(value));
        }
        from = at + want.len();
    }
    None
}

/// The entities that actually appear in prose. A full table is not the point; leaving `&amp;` on
/// screen is, because it makes text look broken.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let Some(semi) = tail[..tail.len().min(12)].find(';') else {
            out.push('&');
            rest = &tail[1..];
            continue;
        };
        let name = &tail[1..semi];
        let replacement = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" | "#160" => Some(' '),
            "mdash" => Some('—'),
            "ndash" => Some('–'),
            "hellip" => Some('…'),
            "#8217" | "rsquo" => Some('\''),
            "#8220" | "ldquo" => Some('"'),
            "#8221" | "rdquo" => Some('"'),
            _ => numeric_entity(name),
        };
        match replacement {
            Some(c) => {
                out.push(c);
                rest = &tail[semi + 1..];
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

fn numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let code = match digits.strip_prefix(['x', 'X']) {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse::<u32>().ok()?,
    };
    char::from_u32(code)
}

fn push_char(out: &mut String, c: char) {
    out.push(c);
}

/// Append text with HTML whitespace rules: any run of whitespace becomes a single space, and a
/// leading space is dropped when the output already ends in whitespace.
fn push_str_collapsed(out: &mut String, s: &str) {
    let mut space = out.is_empty() || out.ends_with([' ', '\n']);
    for c in s.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    }
}

fn push_break(out: &mut String) {
    while out.ends_with(' ') {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

/// Trim trailing spaces and squeeze more than one blank line, which block tags generate in bulk.
fn tidy(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut blanks = 0usize;
    for line in s.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blanks += 1;
            if blanks > 1 {
                continue;
            }
        } else {
            blanks = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    while out.starts_with('\n') {
        out.remove(0);
    }
    out
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
        let p = render("<p>one</p><p>two</p>");
        assert_eq!(p.text.lines().collect::<Vec<_>>(), vec!["one", "two"]);
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
    fn an_unknown_entity_is_left_alone_rather_than_eaten() {
        let p = render("<p>50 &widget; of it</p>");
        assert!(p.text.contains("&widget;"), "{:?}", p.text);
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
    fn comments_are_dropped_including_any_angle_brackets_inside() {
        let p = render("<p>a</p><!-- a > b, and <p>fake</p> --><p>b</p>");
        assert!(!p.text.contains("fake"), "{:?}", p.text);
        assert_eq!(p.text.lines().collect::<Vec<_>>(), vec!["a", "b"]);
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
}
