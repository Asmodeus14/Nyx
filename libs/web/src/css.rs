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

#[derive(Debug, Clone)]
pub struct StyleRule {
    pub selectors: Vec<Selector>,
    pub declarations: Vec<Declaration>,
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

        loop {
            parser.skip_whitespace();
            if parser.is_exhausted() {
                break;
            }

            // Look at the first token to decide rule vs at-rule, then rewind so the prelude parse
            // sees the whole thing.
            let state = parser.state();
            let is_at_rule = matches!(parser.next(), Ok(Token::AtKeyword(_)));
            parser.reset(&state);

            if is_at_rule {
                skip_at_rule(&mut parser);
                continue;
            }

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
                    sheet.rules.push(StyleRule { selectors, declarations });
                }
            }
        }
        sheet
    }
}

/// Consume an at-rule without interpreting it: prelude up to `;` or `{`, then the block if present.
///
/// `@media` etc. are not evaluated yet, but they must be *skipped correctly* — mis-consuming one
/// would drop every rule that follows.
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
        // The rule AFTER the @media block must survive — that is the whole point.
        let s = Stylesheet::parse("@media screen { p { color: red } } h1 { color: blue }");
        assert_eq!(s.rules.len(), 1);
        assert_eq!(s.rules[0].selectors[0].parts[0].1.tag.as_deref(), Some("h1"));

        let s = Stylesheet::parse("@import url(x.css); h1 { color: blue }");
        assert_eq!(s.rules.len(), 1, "a ;-terminated at-rule must not swallow the next rule");
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
}
