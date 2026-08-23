//! Stylesheet parsing: selectors, declarations, specificity.
//!
//! `cssparser` does the tokenizing — strings, escapes, comments, numeric forms and block nesting are
//! its job, and getting them subtly wrong by hand is how a parser ends up mangling one page in
//! twenty. What is built on top here is the selector grammar and the rule structure.
//!
//! Supported today: type / universal / `#id` / `.class` / `[attr]` with all six matchers /
//! pseudo-classes, combined into compounds and joined by descendant, `>`, `+` and `~`. At-rules
//! (`@media`, `@font-face`, …) are parsed far enough to be skipped correctly rather than
//! desynchronising the parser — an at-rule that swallowed the rest of the file would silently drop
//! every style after it.

// `Delimiter` (singular) is the module holding the constants; `Delimiters` is the set type.
use cssparser::{Delimiter, ParseError, Parser, ParserInput, Token};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combinator {
    /// First compound in a selector — nothing to its left.
    None,
    Descendant,
    Child,
    NextSibling,
    LaterSibling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrOp {
    /// `[attr]`
    Exists,
    /// `[attr=v]`
    Equals,
    /// `[attr~=v]` — whitespace-separated word list contains v
    Includes,
    /// `[attr|=v]` — v, or v followed by '-'
    DashMatch,
    /// `[attr^=v]`
    Prefix,
    /// `[attr$=v]`
    Suffix,
    /// `[attr*=v]`
    Substring,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttrSelector {
    pub name: String,
    pub op: AttrOp,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PseudoClass {
    FirstChild,
    LastChild,
    OnlyChild,
    Root,
    Link,
    /// Anything we recognise syntactically but do not implement. Kept (rather than dropped) so it
    /// still counts toward specificity and so a rule guarded by it never matches by accident.
    Unsupported(String),
}

/// A sequence of simple selectors with no combinator between them, e.g. `a.btn[href]:link`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Compound {
    /// `None` means universal (`*`) or an implied universal like `.cls`.
    pub tag: Option<String>,
    pub id: Option<String>,
    pub classes: Vec<String>,
    pub attrs: Vec<AttrSelector>,
    pub pseudos: Vec<PseudoClass>,
}

impl Compound {
    fn is_empty(&self) -> bool {
        self.tag.is_none()
            && self.id.is_none()
            && self.classes.is_empty()
            && self.attrs.is_empty()
            && self.pseudos.is_empty()
    }
}

/// A full selector, stored **rightmost-first**.
///
/// Matching starts at the element under test and walks outward, so storing it in match order keeps
/// the matcher straightforward and lets it bail on the first failure — which is the common case,
/// since most rules do not apply to most elements.
#[derive(Debug, Clone, PartialEq)]
pub struct Selector {
    /// `(combinator-to-my-left, compound)`, rightmost first. The last entry's combinator is `None`.
    pub parts: Vec<(Combinator, Compound)>,
}

impl Selector {
    /// CSS specificity as (ids, classes+attrs+pseudo-classes, types), compared lexicographically.
    pub fn specificity(&self) -> (u32, u32, u32) {
        let mut spec = (0, 0, 0);
        for (_, c) in &self.parts {
            if c.id.is_some() {
                spec.0 += 1;
            }
            spec.1 += (c.classes.len() + c.attrs.len() + c.pseudos.len()) as u32;
            if c.tag.is_some() {
                spec.2 += 1;
            }
        }
        spec
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// What a media query is evaluated against.
///
/// Kept separate from the stylesheet so queries are resolved at CASCADE time, not parse time: the
/// viewport changes on every resize, and re-parsing 200 KB of CSS to answer "is it still wider than
/// 600px" would make resizing cost more than loading.
#[derive(Debug, Clone, Copy)]
pub struct MediaContext {
    pub width_px: f32,
}

impl Default for MediaContext {
    /// A desktop-ish viewport. Used by callers that do not care about media queries (most tests);
    /// the browser passes the real window width.
    fn default() -> Self {
        Self { width_px: 1280.0 }
    }
}

/// One `(feature: value)` test.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaCondition {
    MinWidth(f32),
    MaxWidth(f32),
}

impl MediaCondition {
    fn matches(&self, ctx: &MediaContext) -> bool {
        match *self {
            MediaCondition::MinWidth(px) => ctx.width_px >= px,
            MediaCondition::MaxWidth(px) => ctx.width_px <= px,
        }
    }
}

/// One query in a comma-separated list: an optional media type plus `and`-joined conditions.
#[derive(Debug, Clone)]
pub struct MediaQuery {
    /// `screen`, `print`, `all`, … `None` means none was written, which implies `all`.
    pub media_type: Option<String>,
    pub conditions: Vec<MediaCondition>,
    pub negated: bool,
    /// ★ False when the query contained something we do not understand. The spec says an unknown
    /// feature makes the query FALSE, and that is also the safe direction here: treating an
    /// unrecognised query as a match would apply a print or narrow-screen sheet to the window.
    pub understood: bool,
}

impl MediaQuery {
    fn matches(&self, ctx: &MediaContext) -> bool {
        if !self.understood {
            return false;
        }
        let type_ok = match self.media_type.as_deref() {
            // Nyx paints to a screen. `print` and the rest never match.
            None | Some("all") | Some("screen") => true,
            _ => false,
        };
        let r = type_ok && self.conditions.iter().all(|c| c.matches(ctx));
        if self.negated { !r } else { r }
    }
}

/// A comma-separated media list. Commas are OR.
#[derive(Debug, Clone, Default)]
pub struct MediaList {
    pub queries: Vec<MediaQuery>,
}

impl MediaList {
    /// An empty list matches — that is a bare `@media { }` and, more usefully, the "no media
    /// constraints at all" case for top-level rules.
    pub fn matches(&self, ctx: &MediaContext) -> bool {
        self.queries.is_empty() || self.queries.iter().any(|q| q.matches(ctx))
    }
}

#[derive(Debug, Clone)]
pub struct StyleRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
    /// Every enclosing `@media` list. ALL must match, which is what nesting means; the common case
    /// is empty. A `Vec` rather than an `Option` so nested `@media` composes without the list
    /// having to be flattened into one condition (comma-OR does not distribute over AND).
    pub media: Vec<MediaList>,
}

impl StyleRule {
    pub fn applies(&self, ctx: &MediaContext) -> bool {
        self.media.iter().all(|m| m.matches(ctx))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Stylesheet {
    pub rules: Vec<StyleRule>,
}

impl Stylesheet {
    /// Parse a stylesheet. Never fails: a malformed rule is dropped and parsing resumes at the next
    /// one, which is what CSS error recovery requires and what real pages depend on.
    pub fn parse(css: &str) -> Stylesheet {
        let mut input = ParserInput::new(css);
        let mut parser = Parser::new(&mut input);
        let mut sheet = Stylesheet::default();
        parse_rules_into(&mut parser, &mut sheet, &[]);
        sheet
    }
}

/// Parse rules from `parser` into `sheet`, tagging each with the enclosing `@media` lists.
///
/// Recursive so a nested `@media` composes rather than being flattened.
fn parse_rules_into(parser: &mut Parser, sheet: &mut Stylesheet, media: &[MediaList]) {
    loop {
        parser.skip_whitespace();
        if parser.is_exhausted() {
            break;
        }

        // Look at the first token to decide rule vs at-rule, then rewind so the prelude parse
        // sees the whole thing.
        let state = parser.state();
        let at_name = match parser.next() {
            Ok(Token::AtKeyword(name)) => Some(name.to_string()),
            _ => None,
        };
        parser.reset(&state);

        if let Some(name) = at_name {
            if name.eq_ignore_ascii_case("media") {
                // Consume the `@media` token itself, then the prelude up to the block.
                let _ = parser.next();
                let list = parser
                    .parse_until_before(Delimiter::CurlyBracketBlock, |p| {
                        Ok::<MediaList, ParseError<()>>(parse_media_list(p))
                    })
                    .unwrap_or_default();

                match parser.next() {
                    Ok(Token::CurlyBracketBlock) => {
                        let mut nested = media.to_vec();
                        nested.push(list);
                        // The block is parsed as a rule list. Errors inside must not abort the
                        // outer sheet, which is why this returns Ok unconditionally.
                        let _: Result<(), ParseError<()>> = parser.parse_nested_block(|p| {
                            parse_rules_into(p, sheet, &nested);
                            Ok(())
                        });
                    }
                    // `@media` with no block: malformed, and already consumed.
                    _ => {}
                }
                continue;
            }
            skip_at_rule(parser);
            continue;
        }
        {
            // Prelude = everything up to the `{`.
            let selectors: Result<Vec<Selector>, ParseError<()>> = parser
                .parse_until_before(Delimiter::CurlyBracketBlock, |p| Ok(parse_selector_list(p)));

            // The block must be consumed either way, or the parser desynchronises and the rest of
            // the sheet is read as garbage.
            let block: Result<Vec<Declaration>, ParseError<()>> = match parser.next() {
                Ok(Token::CurlyBracketBlock) => {
                    parser.parse_nested_block(|p| Ok(parse_declarations(p)))
                }
                // No block at all: the sheet ended mid-rule.
                _ => break,
            };

            if let (Ok(selectors), Ok(declarations)) = (selectors, block) {
                if !selectors.is_empty() && !declarations.is_empty() {
                    sheet.rules.push(StyleRule {
                        selectors,
                        declarations,
                        media: media.to_vec(),
                    });
                }
            }
        }
    }
}

/// Parse a media prelude: `screen and (min-width: 600px), print`.
///
/// Token-driven rather than grammar-driven because the prelude is small and the failure mode we
/// care about is "did not understand it", which every branch can set directly.
fn parse_media_list(parser: &mut Parser) -> MediaList {
    let mut list = MediaList::default();
    let mut cur = MediaQuery {
        media_type: None,
        conditions: Vec::new(),
        negated: false,
        understood: true,
    };
    let mut any_token = false;

    loop {
        parser.skip_whitespace();
        let token = match parser.next() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };
        any_token = true;
        match token {
            Token::Comma => {
                list.queries.push(core::mem::replace(
                    &mut cur,
                    MediaQuery {
                        media_type: None,
                        conditions: Vec::new(),
                        negated: false,
                        understood: true,
                    },
                ));
            }
            Token::Ident(name) => {
                let n = name.to_ascii_lowercase();
                match n.as_str() {
                    // `only` exists purely to hide the query from CSS2 parsers; it has no effect.
                    "only" | "and" => {}
                    "not" => cur.negated = true,
                    _ => {
                        if cur.media_type.is_none() {
                            cur.media_type = Some(n);
                        } else {
                            cur.understood = false;
                        }
                    }
                }
            }
            Token::ParenthesisBlock => {
                let parsed = parser.parse_nested_block(|p| {
                    Ok::<Option<MediaCondition>, ParseError<()>>(parse_media_feature(p))
                });
                match parsed {
                    Ok(Some(c)) => cur.conditions.push(c),
                    // An unparsable or unsupported feature makes the whole query false, per spec.
                    _ => cur.understood = false,
                }
            }
            // Anything else in a prelude is something we do not model.
            _ => cur.understood = false,
        }
    }

    if any_token {
        list.queries.push(cur);
    }
    list
}

/// Parse one `(feature: value)`, already inside the parentheses.
fn parse_media_feature(parser: &mut Parser) -> Option<MediaCondition> {
    parser.skip_whitespace();
    let name = match parser.next() {
        Ok(Token::Ident(n)) => n.to_ascii_lowercase(),
        _ => return None,
    };
    parser.skip_whitespace();
    if !matches!(parser.next(), Ok(Token::Colon)) {
        // A boolean feature like `(color)`. We model none of them, so it is not understood.
        return None;
    }
    parser.skip_whitespace();
    let px = match parser.next() {
        Ok(Token::Dimension { value, unit, .. }) => {
            let v = *value;
            match unit.to_ascii_lowercase().as_str() {
                "px" => v,
                // 1em/1rem is the initial 16px here: media queries resolve em against the INITIAL
                // font size, never the element's, so there is no cascade dependency to chase.
                "em" | "rem" => v * 16.0,
                _ => return None,
            }
        }
        // A bare `0` is legal and unitless.
        Ok(Token::Number { value, .. }) if *value == 0.0 => 0.0,
        _ => return None,
    };
    match name.as_str() {
        // `device-width` is the legacy spelling; on a single-window system it is the same number.
        "min-width" | "min-device-width" => Some(MediaCondition::MinWidth(px)),
        "max-width" | "max-device-width" => Some(MediaCondition::MaxWidth(px)),
        _ => None,
    }
}

/// Consume an at-rule without interpreting it: prelude up to `;` or `{`, then the block if present.
///
/// `@media` is handled properly above; `@import`, `@font-face`, `@supports` and the rest still land
/// here and must be *skipped correctly* — mis-consuming one would drop every rule that follows.
fn skip_at_rule(parser: &mut Parser) {
    let _: Result<(), ParseError<()>> = parser.parse_until_before(
        Delimiter::Semicolon | Delimiter::CurlyBracketBlock,
        |p| {
            while p.next().is_ok() {}
            Ok(())
        },
    );
    match parser.next() {
        Ok(Token::CurlyBracketBlock) => {
            let _: Result<(), ParseError<()>> = parser.parse_nested_block(|p| {
                while p.next().is_ok() {}
                Ok(())
            });
        }
        // A `;`-terminated at-rule (@import, @charset) has already been consumed.
        _ => {}
    }
}

fn parse_selector_list(parser: &mut Parser) -> Vec<Selector> {
    let mut out = Vec::new();
    loop {
        match parse_selector(parser) {
            Some(sel) => out.push(sel),
            // Per the Selectors spec, ONE invalid selector invalidates the entire group — a rule
            // must never be applied through only the selectors we happened to understand.
            None => return Vec::new(),
        }
        parser.skip_whitespace();
        match parser.next() {
            Ok(Token::Comma) => continue,
            // Anything else left in the prelude is junk; drop the rule.
            Ok(_) => return Vec::new(),
            Err(_) => break,
        }
    }
    out
}

/// Parse one complex selector. Returns `None` if nothing usable was found.
fn parse_selector(parser: &mut Parser) -> Option<Selector> {
    // Built left-to-right, then reversed — the struct stores rightmost-first for matching.
    let mut parts: Vec<(Combinator, Compound)> = Vec::new();
    let mut pending = Combinator::None;

    loop {
        let compound = parse_compound(parser);
        match compound {
            Some(c) => parts.push((pending, c)),
            None => break,
        }

        // Whitespace here may or may not be a descendant combinator: `a b` is, but the space in
        // `a > b` and the trailing space before `{` are not. So note it and let the next token decide.
        let mut saw_space = false;
        let combinator = loop {
            let state = parser.state();
            match parser.next_including_whitespace() {
                Ok(Token::WhiteSpace(_)) => {
                    saw_space = true;
                    continue;
                }
                Ok(Token::Delim('>')) => break Some(Combinator::Child),
                Ok(Token::Delim('+')) => break Some(Combinator::NextSibling),
                Ok(Token::Delim('~')) => break Some(Combinator::LaterSibling),
                Ok(_) => {
                    parser.reset(&state);
                    break if saw_space { Some(Combinator::Descendant) } else { None };
                }
                Err(_) => break None,
            }
        };

        match combinator {
            Some(c) => pending = c,
            None => break,
        }
    }

    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    // After reversing, each entry's combinator must be the one to ITS left in source order, which
    // is the combinator recorded on the entry that followed it. Shift them along.
    let mut shifted: Vec<(Combinator, Compound)> = Vec::with_capacity(parts.len());
    for i in 0..parts.len() {
        let comb = if i + 1 < parts.len() { parts[i].0 } else { Combinator::None };
        shifted.push((comb, parts[i].1.clone()));
    }
    // The rightmost compound has nothing to its left in match order; the leftmost source compound
    // ends up last and correctly carries `None`.
    Some(Selector { parts: shifted })
}

fn parse_compound(parser: &mut Parser) -> Option<Compound> {
    // Leading whitespace is never part of a compound. It matters only to the caller's combinator
    // detection, which has already run by the time we get here — so skipping it is safe, and NOT
    // skipping it makes `div > p` and `h1, h2` fail: the compound after the combinator starts on a
    // space, bails immediately, and leaves the prelude unconsumed.
    parser.skip_whitespace();

    let mut c = Compound::default();
    let mut first = true;

    loop {
        let state = parser.state();
        let token = match parser.next_including_whitespace() {
            Ok(t) => t.clone(),
            Err(_) => break,
        };

        match token {
            Token::Ident(name) if first => c.tag = Some(name.to_ascii_lowercase()),
            Token::Delim('*') if first => c.tag = None,
            Token::Delim('.') => match parser.next_including_whitespace() {
                Ok(Token::Ident(n)) => c.classes.push(n.to_string()),
                _ => {
                    parser.reset(&state);
                    break;
                }
            },
            Token::IDHash(name) => c.id = Some(name.to_string()),
            Token::Hash(name) => c.id = Some(name.to_string()),
            Token::SquareBracketBlock => {
                let attr: Result<Option<AttrSelector>, ParseError<()>> =
                    parser.parse_nested_block(|p| Ok(parse_attr_selector(p)));
                match attr {
                    Ok(Some(a)) => c.attrs.push(a),
                    _ => {
                        // An attribute selector we cannot read must invalidate the whole selector,
                        // not silently widen it to match everything.
                        return None;
                    }
                }
            }
            Token::Colon => {
                // `::before` is a pseudo-ELEMENT. We do not generate boxes for them, and treating
                // one as a pseudo-class would make the rule apply to the originating element —
                // visibly wrong. Consume and poison the selector.
                let state2 = parser.state();
                let double = matches!(parser.next_including_whitespace(), Ok(Token::Colon));
                if !double {
                    parser.reset(&state2);
                }
                match parser.next_including_whitespace() {
                    Ok(Token::Ident(n)) => {
                        let n = n.to_ascii_lowercase();
                        if double {
                            return None;
                        }
                        c.pseudos.push(match n.as_str() {
                            "first-child" => PseudoClass::FirstChild,
                            "last-child" => PseudoClass::LastChild,
                            "only-child" => PseudoClass::OnlyChild,
                            "root" => PseudoClass::Root,
                            "link" | "any-link" => PseudoClass::Link,
                            _ => PseudoClass::Unsupported(n),
                        });
                    }
                    // Functional pseudo like :nth-child(2n) — consume its block and mark unsupported.
                    Ok(Token::Function(n)) => {
                        let n = n.to_string();
                        let _: Result<(), ParseError<()>> = parser.parse_nested_block(|p| {
                            while p.next().is_ok() {}
                            Ok(())
                        });
                        c.pseudos.push(PseudoClass::Unsupported(n.to_ascii_lowercase()));
                    }
                    _ => return None,
                }
            }
            _ => {
                // Whitespace, combinator, comma, `{` — not part of this compound.
                parser.reset(&state);
                break;
            }
        }
        first = false;
    }

    if c.is_empty() && c.tag.is_none() {
        // Distinguish "nothing at all" from a bare `*`, which is a legitimate empty-but-present
        // compound. `first` is false only if we consumed something.
        if first {
            return None;
        }
    }
    Some(c)
}

fn parse_attr_selector(parser: &mut Parser) -> Option<AttrSelector> {
    parser.skip_whitespace();
    let name = parser.expect_ident().ok()?.to_ascii_lowercase();
    parser.skip_whitespace();

    let op = match parser.next() {
        Err(_) => return Some(AttrSelector { name, op: AttrOp::Exists, value: String::new() }),
        Ok(Token::Delim('=')) => AttrOp::Equals,
        Ok(Token::IncludeMatch) => AttrOp::Includes,
        Ok(Token::DashMatch) => AttrOp::DashMatch,
        Ok(Token::PrefixMatch) => AttrOp::Prefix,
        Ok(Token::SuffixMatch) => AttrOp::Suffix,
        Ok(Token::SubstringMatch) => AttrOp::Substring,
        Ok(_) => return None,
    };

    parser.skip_whitespace();
    let value = match parser.next() {
        Ok(Token::Ident(v)) => v.to_string(),
        Ok(Token::QuotedString(v)) => v.to_string(),
        _ => return None,
    };
    Some(AttrSelector { name, op, value })
}

fn parse_declarations(parser: &mut Parser) -> Vec<Declaration> {
    let mut out = Vec::new();
    loop {
        parser.skip_whitespace();
        if parser.is_exhausted() {
            break;
        }

        let decl: Result<Option<Declaration>, ParseError<()>> =
            parser.parse_until_before(Delimiter::Semicolon, |p| Ok(parse_one_declaration(p)));

        if let Ok(Some(d)) = decl {
            out.push(d);
        }
        // Consume the `;`. Absent at the end of a block, which is legal.
        if parser.next().is_err() {
            break;
        }
    }
    out
}

fn parse_one_declaration(parser: &mut Parser) -> Option<Declaration> {
    parser.skip_whitespace();
    let name = parser.expect_ident().ok()?.to_ascii_lowercase();
    parser.expect_colon().ok()?;

    // Re-serialise the value tokens. Keeping the text (rather than a typed value) lets `style`
    // decide how to interpret each property, and keeps unknown properties round-trippable.
    //
    // `Delimiter::Bang` stops at the `!` of `!important`, so the flag can never end up inside the
    // value text — which is what makes `color: red !important` and `color: red` compare equal.
    let value: Result<String, ParseError<()>> =
        parser.parse_until_before(Delimiter::Bang, |p| Ok(collect_value(p)));
    let value = value.ok()?.trim().to_string();

    let mut important = false;
    if parser.next().is_ok() {
        // Consumed the `!`; the only thing that may legally follow is `important`.
        if let Ok(id) = parser.expect_ident() {
            important = id.eq_ignore_ascii_case("important");
        }
    }

    if value.is_empty() {
        return None;
    }
    Some(Declaration { name, value, important })
}

/// Flatten a value's tokens back to text, collapsing runs of whitespace to a single space.
fn collect_value(parser: &mut Parser) -> String {
    let mut out = String::new();
    while let Ok(t) = parser.next_including_whitespace() {
        match t {
            Token::WhiteSpace(_) => {
                if !out.is_empty() && !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            other => out.push_str(&token_to_string(other)),
        }
    }
    out
}

fn token_to_string(t: &Token) -> String {
    match t {
        Token::Ident(s) => s.to_string(),
        Token::AtKeyword(s) => format!("@{s}"),
        Token::Hash(s) | Token::IDHash(s) => format!("#{s}"),
        Token::QuotedString(s) => format!("\"{s}\""),
        Token::UnquotedUrl(s) => format!("url({s})"),
        Token::Number { int_value, value, .. } => match int_value {
            Some(i) => i.to_string(),
            None => value.to_string(),
        },
        Token::Percentage { unit_value, .. } => format!("{}%", unit_value * 100.0),
        Token::Dimension { value, unit, .. } => format!("{value}{unit}"),
        Token::Delim(c) => c.to_string(),
        Token::Colon => ":".to_string(),
        Token::Comma => ",".to_string(),
        Token::Function(name) => format!("{name}("),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(css: &str) -> StyleRule {
        let s = Stylesheet::parse(css);
        assert_eq!(s.rules.len(), 1, "expected exactly one rule from {css:?}");
        s.rules.into_iter().next().unwrap()
    }

    #[test]
    fn parses_a_simple_rule() {
        let r = one("p { color: red; margin: 0 }");
        assert_eq!(r.selectors.len(), 1);
        assert_eq!(r.selectors[0].parts[0].1.tag.as_deref(), Some("p"));
        assert_eq!(r.declarations.len(), 2);
        assert_eq!(r.declarations[0].name, "color");
        assert_eq!(r.declarations[0].value, "red");
    }

    #[test]
    fn selector_groups_become_separate_selectors() {
        let r = one("h1, h2 , h3 { font-weight: bold }");
        assert_eq!(r.selectors.len(), 3);
    }

    #[test]
    fn parses_classes_ids_and_attrs() {
        let r = one(r#"a#main.btn.big[href^="http"] { color: blue }"#);
        let c = &r.selectors[0].parts[0].1;
        assert_eq!(c.tag.as_deref(), Some("a"));
        assert_eq!(c.id.as_deref(), Some("main"));
        assert_eq!(c.classes, vec!["btn", "big"]);
        assert_eq!(c.attrs.len(), 1);
        assert_eq!(c.attrs[0].op, AttrOp::Prefix);
        assert_eq!(c.attrs[0].value, "http");
    }

    #[test]
    fn stores_combinators_rightmost_first() {
        let r = one("div > p a { color: red }");
        let parts = &r.selectors[0].parts;
        assert_eq!(parts.len(), 3);
        // Rightmost is `a`, and the combinator to its left is the descendant space.
        assert_eq!(parts[0].1.tag.as_deref(), Some("a"));
        assert_eq!(parts[0].0, Combinator::Descendant);
        assert_eq!(parts[1].1.tag.as_deref(), Some("p"));
        assert_eq!(parts[1].0, Combinator::Child);
        assert_eq!(parts[2].1.tag.as_deref(), Some("div"));
        assert_eq!(parts[2].0, Combinator::None);
    }

    #[test]
    fn specificity_counts_the_three_buckets() {
        let s = |css: &str| one(css).selectors[0].specificity();
        assert_eq!(s("* { color: red }"), (0, 0, 0));
        assert_eq!(s("p { color: red }"), (0, 0, 1));
        assert_eq!(s(".c { color: red }"), (0, 1, 0));
        assert_eq!(s("#i { color: red }"), (1, 0, 0));
        assert_eq!(s("div p.c#i[href] { color: red }"), (1, 2, 2));
    }

    #[test]
    fn important_is_flagged_and_stripped_from_the_value() {
        let r = one("p { color: red !important }");
        assert!(r.declarations[0].important);
        assert_eq!(r.declarations[0].value, "red");
    }

    #[test]
    fn at_rules_are_skipped_without_eating_what_follows() {
        // Updated when @media stopped being skipped and started being EVALUATED: this used to
        // assert the `p` inside the block was dropped. The property still being tested — and the
        // one that mattered all along — is that the rule AFTER an at-rule survives it.
        let s = Stylesheet::parse("@media screen { p { color: red } } h1 { color: blue }");
        assert_eq!(s.rules.len(), 2);
        assert_eq!(s.rules[1].selectors[0].parts[0].1.tag.as_deref(), Some("h1"));

        let s = Stylesheet::parse("@import url(x.css); h1 { color: blue }");
        assert_eq!(s.rules.len(), 1, "a ;-terminated at-rule must not swallow the next rule");

        // An at-rule we still skip wholesale must behave as it always did.
        let s = Stylesheet::parse("@supports (display: grid) { p { color: red } } h1 { color: blue }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].parts[0].1.tag.as_deref(), Some("h1"));
    }

    #[test]
    fn comments_and_odd_whitespace_are_handled() {
        let s = Stylesheet::parse("/* c */ p /* c */ { /* c */ color : red /* c */ }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].declarations[0].value, "red");
    }

    #[test]
    fn a_malformed_rule_does_not_kill_the_sheet() {
        let s = Stylesheet::parse("p { color: } h1 { color: blue }");
        // The empty declaration is dropped; h1 still parses.
        assert!(s.rules.iter().any(|r| r.selectors[0].parts[0].1.tag.as_deref() == Some("h1")));
    }

    #[test]
    fn pseudo_elements_poison_their_selector() {
        // We generate no boxes for ::before, so this rule must not apply to the element itself.
        let s = Stylesheet::parse("p::before { color: red }");
        assert!(s.rules.is_empty() || s.rules[0].selectors.is_empty());
    }

    #[test]
    fn multi_value_declarations_keep_their_text() {
        let r = one("p { margin: 10px 20px; font-family: \"Times New Roman\", serif }");
        assert_eq!(r.declarations[0].value, "10px 20px");
        assert!(r.declarations[1].value.contains("Times New Roman"));
    }

    // ---- @media ---------------------------------------------------------------------------

    fn wide() -> MediaContext { MediaContext { width_px: 1200.0 } }
    fn narrow() -> MediaContext { MediaContext { width_px: 400.0 } }

    /// The regression this whole feature exists for: rules inside `@media` used to be DROPPED, so
    /// a page whose layout lives in media blocks rendered unstyled.
    #[test]
    fn media_rules_are_kept_not_skipped() {
        let s = Stylesheet::parse("@media screen { p { color: red } }");
        assert_eq!(s.rules.len(), 1);
        assert!(s.rules[0].applies(&wide()));
    }

    #[test]
    fn min_width_gates_on_the_viewport() {
        let s = Stylesheet::parse("@media (min-width: 600px) { p { color: red } }");
        assert!(s.rules[0].applies(&wide()));
        assert!(!s.rules[0].applies(&narrow()));
    }

    #[test]
    fn max_width_gates_the_other_way() {
        let s = Stylesheet::parse("@media (max-width: 600px) { p { color: red } }");
        assert!(!s.rules[0].applies(&wide()));
        assert!(s.rules[0].applies(&narrow()));
    }

    #[test]
    fn and_requires_both() {
        let s = Stylesheet::parse(
            "@media screen and (min-width: 300px) and (max-width: 500px) { p { color: red } }",
        );
        assert!(s.rules[0].applies(&narrow()));
        assert!(!s.rules[0].applies(&wide()));
    }

    /// Commas are OR, and getting this backwards silently halves what a page applies.
    #[test]
    fn comma_is_or() {
        let s = Stylesheet::parse("@media (max-width: 100px), (min-width: 1000px) { p { color: red } }");
        assert!(s.rules[0].applies(&wide()));
        assert!(!s.rules[0].applies(&narrow()));
    }

    #[test]
    fn print_never_matches_a_screen() {
        let s = Stylesheet::parse("@media print { p { color: red } }");
        assert!(!s.rules[0].applies(&wide()));
    }

    #[test]
    fn only_screen_is_the_same_as_screen() {
        let s = Stylesheet::parse("@media only screen and (min-width: 600px) { p { color: red } }");
        assert!(s.rules[0].applies(&wide()));
    }

    #[test]
    fn not_inverts() {
        let s = Stylesheet::parse("@media not screen { p { color: red } }");
        assert!(!s.rules[0].applies(&wide()));
    }

    /// Per spec an unknown feature makes the query false. The safe direction too: matching it would
    /// apply a sheet meant for hardware we cannot detect.
    #[test]
    fn unknown_features_never_match() {
        let s = Stylesheet::parse("@media (orientation: landscape) { p { color: red } }");
        assert_eq!(s.rules.len(), 1);
        assert!(!s.rules[0].applies(&wide()));
    }

    #[test]
    fn em_resolves_against_the_initial_font_size() {
        // 40em = 640px.
        let s = Stylesheet::parse("@media (min-width: 40em) { p { color: red } }");
        assert!(s.rules[0].applies(&MediaContext { width_px: 700.0 }));
        assert!(!s.rules[0].applies(&MediaContext { width_px: 600.0 }));
    }

    #[test]
    fn nested_media_requires_both() {
        let s = Stylesheet::parse(
            "@media (min-width: 300px) { @media (max-width: 500px) { p { color: red } } }",
        );
        assert_eq!(s.rules[0].media.len(), 2);
        assert!(s.rules[0].applies(&narrow()));
        assert!(!s.rules[0].applies(&wide()));
    }

    /// The block must be consumed exactly, or everything after it is read as garbage — the same
    /// failure the old `skip_at_rule` was written to avoid.
    #[test]
    fn rules_after_a_media_block_still_parse() {
        let s = Stylesheet::parse("@media screen { p { color: red } } h1 { color: blue }");
        assert_eq!(s.rules.len(), 2);
        assert!(s.rules[1].media.is_empty());
    }

    #[test]
    fn a_top_level_rule_has_no_media_and_always_applies() {
        let s = Stylesheet::parse("p { color: red }");
        assert!(s.rules[0].media.is_empty());
        assert!(s.rules[0].applies(&narrow()));
    }

    /// Other at-rules must still be skipped whole, not misread as media.
    #[test]
    fn font_face_is_still_skipped() {
        let s = Stylesheet::parse("@font-face { font-family: x } p { color: red }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].parts[0].1.tag.as_deref(), Some("p"));
    }
}
