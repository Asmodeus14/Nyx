#![cfg_attr(not(test), no_std)]

//! # `nyx-json` — just enough JSON to talk to a quantum provider
//!
//! A [`Value`] tree, a recursive-descent parser, and a writer. No derive macros, no reflection, no
//! borrowed-lifetime zero-copy mode. ~350 lines, zero dependencies.
//!
//! ## Why this is not `serde_json`
//!
//! `serde_json` is excellent and would have been one line in a `Cargo.toml`. It is not used because
//! of what this crate is *for*: parsing HTTP responses from a cloud service. That is untrusted input
//! arriving over the network, and the properties that matter are
//!
//! * the recursion bound can be pointed at — it is [`MAX_DEPTH`], enforced in one place;
//! * the whole parser fits in one reading;
//! * nothing is pulled into the dependency graph of a crate the kernel-adjacent side of the tree
//!   links against.
//!
//! Nyx's engineering rule is *small, modular, understandable, auditable*, and the terminal is a
//! `no_std`-adjacent binary on a machine with no serial console. A 350-line parser with a stated
//! depth cap is a smaller thing to trust than a derive framework.
//!
//! ## ⚠️ What it deliberately does not do
//!
//! * **No arbitrary-precision numbers.** Everything numeric becomes `f64`, which is what JSON's
//!   spec effectively assumes and what a probability histogram needs. A 64-bit integer id beyond
//!   2⁵³ would lose precision — providers use string ids, and [`Value::as_str`] is how you read them.
//! * **No duplicate-key merging.** The last key wins, which is what every mainstream parser does.
//! * **No trailing commas, comments, or `NaN`/`Infinity`.** Strict RFC 8259. A provider sending
//!   those is a provider whose response should be rejected loudly.
//!
//! ## Depth, and why it is a security control
//!
//! `[[[[[[...]]]]]]` is a few hundred bytes of input and an unbounded amount of stack. The kernel
//! hands userspace a fixed stack and there is no guard-page story for a deep recursion in an app —
//! it faults, and `pf_handler` kills the process. That is survivable but it is a remote party
//! choosing when your terminal dies, so nesting past [`MAX_DEPTH`] is a parse error instead.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// `|x|`. `f64::abs` lives in `std` (it is backed by `libm`), and this crate is `no_std` with no
/// dependencies — so the handful of float helpers it needs are here, as three lines of comparison.
fn fabs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else {
        x
    }
}

/// Truncate toward zero. ⚠️ Only valid for `|x| < 2⁶³`; every caller checks the magnitude first.
fn trunc(x: f64) -> f64 {
    (x as i64) as f64
}

/// Whether `x` holds an exact integer value. Replaces `x.fract() == 0.0`.
fn is_integral(x: f64) -> bool {
    x.is_finite() && fabs(x) < 9.223_372_036_854_776e18 && x == trunc(x)
}

/// The largest `f64` that represents every integer below it exactly: 2⁵³.
const MAX_EXACT_INT: f64 = 9_007_199_254_740_992.0;

/// Maximum nesting depth.
///
/// 64 is far past anything a real API returns (IonQ's deepest response is about four levels) and far
/// short of anything that threatens a userspace stack. See the module docs — this is a security
/// control, not a convenience limit.
pub const MAX_DEPTH: usize = 64;

/// A JSON value.
///
/// Objects are a `Vec` of pairs rather than a map: provider responses have a handful of keys, a
/// linear scan beats hashing at that size, and it keeps this crate free of a `BTreeMap` import and
/// of any question about key ordering.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The value at `key`, if this is an object containing it.
    ///
    /// Last occurrence wins, matching every mainstream parser.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(pairs) => pairs.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Follow a path of object keys. `v.path(&["data", "histogram"])`.
    pub fn path(&self, keys: &[&str]) -> Option<&Value> {
        let mut cur = self;
        for k in keys {
            cur = cur.get(k)?;
        }
        Some(cur)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    /// The value as a `u64`, if it is a number that is actually a non-negative integer.
    ///
    /// Rejects fractions rather than truncating: a provider returning `3.7` where a count belongs is
    /// a provider whose response should not be silently rounded.
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(v) if *v >= 0.0 && *v <= MAX_EXACT_INT && is_integral(*v) => {
                Some(*v as u64)
            }
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Object(v) => Some(v.as_slice()),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    /// Serialise. Compact, no whitespace.
    pub fn to_string(&self) -> String {
        let mut s = String::new();
        self.write(&mut s);
        s
    }

    fn write(&self, out: &mut String) {
        match self {
            Value::Null => out.push_str("null"),
            Value::Bool(true) => out.push_str("true"),
            Value::Bool(false) => out.push_str("false"),
            Value::Number(n) => write_number(*n, out),
            Value::String(s) => write_string(s, out),
            Value::Array(items) => {
                out.push('[');
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    v.write(out);
                }
                out.push(']');
            }
            Value::Object(pairs) => {
                out.push('{');
                for (i, (k, v)) in pairs.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    write_string(k, out);
                    out.push(':');
                    v.write(out);
                }
                out.push('}');
            }
        }
    }
}

/// Write a number the way JSON wants it.
///
/// ⚠️ Integers are written without a decimal point. `{"shots": 1024.0}` is legal JSON but several
/// APIs reject it when they expect an integer field, and `1024` is what every other client sends.
fn write_number(n: f64, out: &mut String) {
    if !n.is_finite() {
        // JSON has no NaN or Infinity. Emitting `null` is what serde_json does and is the only legal
        // option; silently writing `NaN` would produce a document no parser accepts.
        out.push_str("null");
        return;
    }
    if is_integral(n) && fabs(n) < MAX_EXACT_INT {
        out.push_str(&itoa(n as i64));
    } else {
        out.push_str(&ftoa(n));
    }
}

/// `i64` to decimal, without `format!` (which needs `alloc::fmt` machinery this avoids in hot paths).
fn itoa(mut v: i64) -> String {
    if v == 0 {
        return String::from("0");
    }
    let neg = v < 0;
    let mut digits = [0u8; 20];
    let mut n = 0;
    // Work in the negative domain so i64::MIN does not overflow on negation.
    if !neg {
        v = -v;
    }
    while v != 0 {
        digits[n] = b'0' + ((-(v % 10)) as u8);
        v /= 10;
        n += 1;
    }
    let mut s = String::with_capacity(n + 1);
    if neg {
        s.push('-');
    }
    for i in (0..n).rev() {
        s.push(digits[i] as char);
    }
    s
}

/// `f64` to decimal with enough digits to round-trip a probability.
///
/// Not a shortest-representation algorithm (Ryū/Grisu); 17 significant digits via repeated scaling
/// is sufficient and is a tenth of the code. The only floats this crate writes are gate angles and
/// probabilities.
fn ftoa(v: f64) -> String {
    let mut s = String::new();
    let mut x = v;
    if x < 0.0 {
        s.push('-');
        x = -x;
    }
    let int_part = x as u64;
    s.push_str(&itoa(int_part as i64));
    let mut frac = x - int_part as f64;
    if frac == 0.0 {
        s.push_str(".0");
        return s;
    }
    s.push('.');
    // 17 digits round-trips any f64.
    for _ in 0..17 {
        frac *= 10.0;
        let d = frac as u32;
        s.push((b'0' + d as u8) as char);
        frac -= d as f64;
        if frac <= 0.0 {
            break;
        }
    }
    s
}

/// Write a JSON string literal with the escapes RFC 8259 requires.
fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            // Everything below 0x20 must be escaped; \u is the only general form.
            c if (c as u32) < 0x20 => {
                out.push_str("\\u00");
                let b = c as u32;
                out.push(hex_digit((b >> 4) as u8));
                out.push(hex_digit((b & 0xF) as u8));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn hex_digit(n: u8) -> char {
    match n {
        0..=9 => (b'0' + n) as char,
        _ => (b'a' + (n - 10)) as char,
    }
}

/// Why a document was rejected. Carries a byte offset, because "invalid JSON" is not diagnosable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    pub at: usize,
    pub what: &'static str,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at byte {}", self.what, self.at)
    }
}

/// Parse a JSON document.
///
/// Rejects trailing content: `{"a":1} garbage` is an error, not a success with the tail ignored.
pub fn parse(input: &str) -> Result<Value, Error> {
    let mut p = Parser { b: input.as_bytes(), i: 0, depth: 0 };
    p.skip_ws();
    let v = p.value()?;
    p.skip_ws();
    if p.i != p.b.len() {
        return Err(p.err("unexpected trailing content"));
    }
    Ok(v)
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn err(&self, what: &'static str) -> Error {
        Error { at: self.i, what }
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(c) = self.peek() {
            match c {
                b' ' | b'\t' | b'\n' | b'\r' => self.i += 1,
                _ => break,
            }
        }
    }

    fn eat(&mut self, c: u8) -> bool {
        if self.peek() == Some(c) {
            self.i += 1;
            true
        } else {
            false
        }
    }

    fn lit(&mut self, s: &[u8]) -> bool {
        if self.b[self.i..].starts_with(s) {
            self.i += s.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self) -> Result<Value, Error> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') => {
                if self.lit(b"null") {
                    Ok(Value::Null)
                } else {
                    Err(self.err("invalid literal"))
                }
            }
            Some(b't') => {
                if self.lit(b"true") {
                    Ok(Value::Bool(true))
                } else {
                    Err(self.err("invalid literal"))
                }
            }
            Some(b'f') => {
                if self.lit(b"false") {
                    Ok(Value::Bool(false))
                } else {
                    Err(self.err("invalid literal"))
                }
            }
            Some(b'"') => self.string().map(Value::String),
            Some(b'[') => self.array(),
            Some(b'{') => self.object(),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.number(),
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    /// Enter a nested container, enforcing [`MAX_DEPTH`].
    ///
    /// ★ The one place recursion is bounded. Both `array` and `object` go through it, so there is no
    /// second path that could grow without a check.
    fn enter(&mut self) -> Result<(), Error> {
        if self.depth >= MAX_DEPTH {
            return Err(self.err("nesting too deep"));
        }
        self.depth += 1;
        Ok(())
    }

    fn array(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.i += 1; // '['
        let mut out = Vec::new();
        self.skip_ws();
        if self.eat(b']') {
            self.depth -= 1;
            return Ok(Value::Array(out));
        }
        loop {
            self.skip_ws();
            out.push(self.value()?);
            self.skip_ws();
            if self.eat(b',') {
                continue;
            }
            if self.eat(b']') {
                break;
            }
            return Err(self.err("expected ',' or ']'"));
        }
        self.depth -= 1;
        Ok(Value::Array(out))
    }

    fn object(&mut self) -> Result<Value, Error> {
        self.enter()?;
        self.i += 1; // '{'
        let mut out: Vec<(String, Value)> = Vec::new();
        self.skip_ws();
        if self.eat(b'}') {
            self.depth -= 1;
            return Ok(Value::Object(out));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a string key"));
            }
            let k = self.string()?;
            self.skip_ws();
            if !self.eat(b':') {
                return Err(self.err("expected ':'"));
            }
            self.skip_ws();
            let v = self.value()?;
            out.push((k, v));
            self.skip_ws();
            if self.eat(b',') {
                continue;
            }
            if self.eat(b'}') {
                break;
            }
            return Err(self.err("expected ',' or '}'"));
        }
        self.depth -= 1;
        Ok(Value::Object(out))
    }

    fn string(&mut self) -> Result<String, Error> {
        self.i += 1; // opening quote
        let mut s = String::new();
        loop {
            let c = match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(c) => c,
            };
            self.i += 1;
            match c {
                b'"' => return Ok(s),
                b'\\' => {
                    let e = match self.peek() {
                        None => return Err(self.err("unterminated escape")),
                        Some(e) => e,
                    };
                    self.i += 1;
                    match e {
                        b'"' => s.push('"'),
                        b'\\' => s.push('\\'),
                        b'/' => s.push('/'),
                        b'b' => s.push('\u{08}'),
                        b'f' => s.push('\u{0c}'),
                        b'n' => s.push('\n'),
                        b'r' => s.push('\r'),
                        b't' => s.push('\t'),
                        b'u' => {
                            let hi = self.hex4()?;
                            // Surrogate pair: a high surrogate must be followed by \uDC00-\uDFFF.
                            let ch = if (0xD800..0xDC00).contains(&hi) {
                                if !self.lit(b"\\u") {
                                    return Err(self.err("lone high surrogate"));
                                }
                                let lo = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&lo) {
                                    return Err(self.err("invalid low surrogate"));
                                }
                                let c = 0x1_0000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                                char::from_u32(c)
                            } else if (0xDC00..0xE000).contains(&hi) {
                                return Err(self.err("lone low surrogate"));
                            } else {
                                char::from_u32(hi)
                            };
                            match ch {
                                Some(c) => s.push(c),
                                None => return Err(self.err("invalid code point")),
                            }
                        }
                        _ => return Err(self.err("invalid escape")),
                    }
                }
                // Raw control characters are not legal inside a JSON string.
                c if c < 0x20 => return Err(self.err("control character in string")),
                c if c < 0x80 => s.push(c as char),
                _ => {
                    // Multi-byte UTF-8: find the whole sequence and validate it.
                    let start = self.i - 1;
                    let len = utf8_len(c);
                    if len == 0 || start + len > self.b.len() {
                        return Err(self.err("invalid UTF-8"));
                    }
                    match core::str::from_utf8(&self.b[start..start + len]) {
                        Ok(t) => {
                            s.push_str(t);
                            self.i = start + len;
                        }
                        Err(_) => return Err(self.err("invalid UTF-8")),
                    }
                }
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        if self.i + 4 > self.b.len() {
            return Err(self.err("truncated \\u escape"));
        }
        let mut v = 0u32;
        for k in 0..4 {
            let c = self.b[self.i + k];
            let d = match c {
                b'0'..=b'9' => (c - b'0') as u32,
                b'a'..=b'f' => (c - b'a' + 10) as u32,
                b'A'..=b'F' => (c - b'A' + 10) as u32,
                _ => return Err(self.err("bad hex digit")),
            };
            v = (v << 4) | d;
        }
        self.i += 4;
        Ok(v)
    }

    fn number(&mut self) -> Result<Value, Error> {
        let start = self.i;
        if self.eat(b'-') {}
        // Integer part: `0` alone, or a nonzero digit followed by digits. Leading zeros are illegal.
        if self.eat(b'0') {
        } else {
            let mut any = false;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
                any = true;
            }
            if !any {
                return Err(self.err("expected a digit"));
            }
        }
        if self.peek() == Some(b'.') {
            self.i += 1;
            let mut any = false;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
                any = true;
            }
            if !any {
                return Err(self.err("expected a digit after '.'"));
            }
        }
        if matches!(self.peek(), Some(b'e') | Some(b'E')) {
            self.i += 1;
            if matches!(self.peek(), Some(b'+') | Some(b'-')) {
                self.i += 1;
            }
            let mut any = false;
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.i += 1;
                any = true;
            }
            if !any {
                return Err(self.err("expected an exponent digit"));
            }
        }
        let text = core::str::from_utf8(&self.b[start..self.i]).map_err(|_| self.err("bad number"))?;
        // `f64::from_str` is in core, not just std.
        text.parse::<f64>().map(Value::Number).map_err(|_| self.err("number out of range"))
    }
}

/// Length in bytes of a UTF-8 sequence from its lead byte. `0` for an invalid lead.
fn utf8_len(b: u8) -> usize {
    match b {
        0x00..=0x7F => 1,
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => 0,
    }
}

// ── Building ────────────────────────────────────────────────────────────────────────────────────

/// Build an object from pairs. `obj(&[("a", Value::Number(1.0))])`.
pub fn obj(pairs: &[(&str, Value)]) -> Value {
    Value::Object(pairs.iter().map(|(k, v)| (String::from(*k), v.clone())).collect())
}

/// A string value, from anything stringy.
pub fn s(v: &str) -> Value {
    Value::String(String::from(v))
}

/// A number value from an integer, written without a decimal point.
pub fn n(v: i64) -> Value {
    Value::Number(v as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn parses_the_scalars() {
        assert_eq!(parse("null").unwrap(), Value::Null);
        assert_eq!(parse("true").unwrap(), Value::Bool(true));
        assert_eq!(parse("false").unwrap(), Value::Bool(false));
        assert_eq!(parse("0").unwrap(), Value::Number(0.0));
        assert_eq!(parse("-3").unwrap(), Value::Number(-3.0));
        assert_eq!(parse("1.5e3").unwrap(), Value::Number(1500.0));
        assert_eq!(parse("\"hi\"").unwrap(), Value::String("hi".into()));
    }

    #[test]
    fn parses_nested_structures() {
        let v = parse(r#"{"a":[1,2,{"b":null}],"c":true}"#).unwrap();
        assert_eq!(v.get("c").unwrap().as_bool(), Some(true));
        let a = v.get("a").unwrap().as_array().unwrap();
        assert_eq!(a.len(), 3);
        assert!(a[2].get("b").unwrap().is_null());
        assert_eq!(v.path(&["a"]).unwrap().as_array().unwrap().len(), 3);
    }

    #[test]
    fn handles_whitespace_and_empty_containers() {
        assert_eq!(parse("  {  }  ").unwrap(), Value::Object(vec![]));
        assert_eq!(parse("[\n\t]").unwrap(), Value::Array(vec![]));
        assert_eq!(
            parse(" { \"a\" : [ 1 , 2 ] } ").unwrap().get("a").unwrap().as_array().unwrap().len(),
            2
        );
    }

    /// ★ The security control. A few hundred bytes of input must not be able to choose when the
    /// terminal's stack runs out.
    #[test]
    fn nesting_past_the_cap_is_an_error_not_a_crash() {
        let ok: String = "[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH);
        assert!(parse(&ok).is_ok(), "exactly at the cap must be accepted");

        let deep: String = "[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1);
        let e = parse(&deep).unwrap_err();
        assert_eq!(e.what, "nesting too deep");

        // Objects go through the same gate, not a second unchecked path.
        let deep_obj: String =
            "{\"a\":".repeat(MAX_DEPTH + 1) + "1" + &"}".repeat(MAX_DEPTH + 1);
        assert_eq!(parse(&deep_obj).unwrap_err().what, "nesting too deep");
    }

    #[test]
    fn depth_is_per_branch_not_cumulative() {
        // Many shallow siblings must not accumulate toward the cap — only nesting counts.
        let wide: String = alloc::format!("[{}]", vec!["[1]"; 500].join(","));
        assert!(parse(&wide).is_ok());
    }

    #[test]
    fn malformed_documents_are_rejected_with_an_offset() {
        for bad in [
            "", "{", "[", "{\"a\"}", "{\"a\":}", "[1,]", "{,}", "tru", "01", "1.", "1e",
            "\"unterminated", "{\"a\":1,}", "nul",
        ] {
            let e = parse(bad);
            assert!(e.is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn trailing_content_is_rejected_rather_than_ignored() {
        let e = parse(r#"{"a":1} junk"#).unwrap_err();
        assert_eq!(e.what, "unexpected trailing content");
    }

    #[test]
    fn string_escapes_round_trip() {
        let v = parse(r#""a\"b\\c\nd\teAé""#).unwrap();
        assert_eq!(v.as_str().unwrap(), "a\"b\\c\nd\te\u{41}\u{e9}");

        // And back out again.
        let out = v.to_string();
        assert_eq!(parse(&out).unwrap(), v);
    }

    #[test]
    fn surrogate_pairs_become_one_character() {
        // U+1F600 GRINNING FACE
        let v = parse(r#""😀""#).unwrap();
        assert_eq!(v.as_str().unwrap(), "\u{1F600}");
        assert!(parse(r#""\ud83d""#).is_err(), "lone high surrogate");
        assert!(parse(r#""\ude00""#).is_err(), "lone low surrogate");
    }

    #[test]
    fn raw_control_characters_in_strings_are_rejected() {
        assert!(parse("\"a\nb\"").is_err());
        assert!(parse("\"a\tb\"").is_err());
    }

    #[test]
    fn multibyte_utf8_survives_a_round_trip() {
        let v = parse("\"héllo — 日本\"").unwrap();
        assert_eq!(v.as_str().unwrap(), "héllo — 日本");
        assert_eq!(parse(&v.to_string()).unwrap(), v);
    }

    /// ⚠️ `{"shots": 1024.0}` is legal JSON that several APIs reject when they want an integer.
    #[test]
    fn integers_are_written_without_a_decimal_point() {
        assert_eq!(n(1024).to_string(), "1024");
        assert_eq!(n(0).to_string(), "0");
        assert_eq!(n(-7).to_string(), "-7");
        assert_eq!(Value::Number(2.0).to_string(), "2");
    }

    #[test]
    fn fractions_round_trip_to_full_precision() {
        for x in [0.5, 0.497, 1.0 / 3.0, 0.123456789012345, 2.718281828459045] {
            let out = Value::Number(x).to_string();
            let back = parse(&out).unwrap().as_f64().unwrap();
            assert!((back - x).abs() < 1e-15, "{x} -> {out} -> {back}");
        }
    }

    #[test]
    fn as_u64_refuses_to_round_a_fraction() {
        assert_eq!(Value::Number(7.0).as_u64(), Some(7));
        assert_eq!(Value::Number(7.9).as_u64(), None);
        assert_eq!(Value::Number(-1.0).as_u64(), None);
    }

    #[test]
    fn non_finite_numbers_are_written_as_null_because_json_has_no_nan() {
        assert_eq!(Value::Number(f64::NAN).to_string(), "null");
        assert_eq!(Value::Number(f64::INFINITY).to_string(), "null");
    }

    #[test]
    fn the_last_duplicate_key_wins() {
        let v = parse(r#"{"a":1,"a":2}"#).unwrap();
        assert_eq!(v.get("a").unwrap().as_f64(), Some(2.0));
    }

    #[test]
    fn path_returns_none_rather_than_panicking_on_a_wrong_shape() {
        let v = parse(r#"{"a":{"b":1}}"#).unwrap();
        assert_eq!(v.path(&["a", "b"]).unwrap().as_f64(), Some(1.0));
        assert!(v.path(&["a", "z"]).is_none());
        assert!(v.path(&["z", "b"]).is_none());
        // Indexing into a scalar is a miss, not a crash.
        assert!(v.path(&["a", "b", "c"]).is_none());
    }

    #[test]
    fn builders_produce_what_a_provider_expects() {
        let body = obj(&[
            ("target", s("simulator")),
            ("shots", n(1024)),
            (
                "input",
                obj(&[
                    ("format", s("ionq.circuit.v0")),
                    ("qubits", n(2)),
                    (
                        "circuit",
                        Value::Array(vec![
                            obj(&[("gate", s("h")), ("target", n(0))]),
                            obj(&[("gate", s("cnot")), ("control", n(0)), ("target", n(1))]),
                        ]),
                    ),
                ]),
            ),
        ]);
        let text = body.to_string();
        assert_eq!(
            text,
            r#"{"target":"simulator","shots":1024,"input":{"format":"ionq.circuit.v0","qubits":2,"circuit":[{"gate":"h","target":0},{"gate":"cnot","control":0,"target":1}]}}"#
        );
        // And it parses back to the same tree.
        assert_eq!(parse(&text).unwrap(), body);
    }

    /// A recorded IonQ job-results response. See `docs/quantum/remote.md`.
    #[test]
    fn parses_a_recorded_ionq_histogram() {
        // ⚠️ The keys are DECIMAL BASIS-STATE INDICES and the values are PROBABILITIES, not counts.
        let doc = r#"{"0":0.4970703125,"3":0.5029296875}"#;
        let v = parse(doc).unwrap();
        let pairs = v.as_object().unwrap();
        assert_eq!(pairs.len(), 2);
        let total: f64 = pairs.iter().map(|(_, p)| p.as_f64().unwrap()).sum();
        assert!((total - 1.0).abs() < 1e-9, "a histogram should sum to 1, got {total}");
        assert_eq!(v.get("0").unwrap().as_f64(), Some(0.4970703125));
    }

    #[test]
    fn parses_a_recorded_ionq_job_envelope() {
        let doc = r#"{
            "id": "617a1f8b-59d4-435d-aa33-695433d7155e",
            "status": "completed",
            "target": "qpu.aria-1",
            "qubits": 2,
            "shots": 1024,
            "request": 1631894413,
            "children": []
        }"#;
        let v = parse(doc).unwrap();
        assert_eq!(v.get("status").unwrap().as_str(), Some("completed"));
        assert_eq!(v.get("target").unwrap().as_str(), Some("qpu.aria-1"));
        // ★ The id is a STRING. Parsing it as a number would lose it — which is why providers use
        // strings and why `as_str` is the accessor for ids.
        assert!(v.get("id").unwrap().as_str().unwrap().len() > 20);
        assert_eq!(v.get("shots").unwrap().as_u64(), Some(1024));
    }
}
