//! Splitting a line of QCLang source into the three tones the design's code pane sets it in.
//!
//! ```text
//! .ql .c        { color: var(--fg-2) }   /* the code                */
//! .ql .c .kw    { color: var(--fg)   }   /* reserved words          */
//! .ql .c .dim   { color: var(--fg-4) }   /* comments                */
//! ```
//!
//! Three tones, no colour. That is not a stylistic choice the window is free to revise: Nyx's text
//! shader carries **one luminance** per quad (`build_ps_text` interpolates attribute-0 channel 2),
//! so a syntax highlighter with red strings and green numbers is not something this machine can
//! draw. Meridian answers that by highlighting *structurally* — what is reserved, what is a
//! comment, what is neither — which is most of what colour was carrying anyway.
//!
//! ## Why it lives beside the lexer
//!
//! Because it asks the lexer what a word is. [`crate::lexer::is_keyword`] answers from `logos`'s own
//! token table and [`crate::lexer::is_gate_name`] from the list the semantic analyser uses, so the
//! pane cannot highlight a word the compiler does not recognise, or miss one it does. The usual bug
//! in a code viewer — a keyword list that has drifted a release behind the grammar — has nowhere to
//! live here.
//!
//! Portable, allocation-light, and host-tested: the QCLang window runs this over every visible line
//! on every frame, on a machine with no QEMU to check it on.

use crate::lexer::{is_gate_name, is_keyword};

/// The tone a run of characters is set in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// `--fg-2`. Punctuation, identifiers, numbers — the body of the code.
    Plain,
    /// `--fg`. A reserved word: `fn`, `qubit`, `return`, `measure`'s siblings.
    Keyword,
    /// `--fg`. A gate name. Set at the same weight as a keyword because in a quantum program the
    /// gates *are* the instructions — dropping them to `Plain` leaves the one line that matters
    /// (`CNOT(H(a), b)`) looking like punctuation.
    Gate,
    /// `--fg-4`. A comment, to the end of the line.
    Comment,
}

/// A half-open byte range of the line, and the tone to draw it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub start: usize,
    pub end: usize,
    pub tone: Tone,
}

/// Split `line` into consecutive runs covering it exactly, left to right.
///
/// Line-at-a-time, because the pane draws and scrolls by line and because the language's only
/// multi-line construct is the `/* */` comment — handled by [`block_comment_depth`] rather than by
/// making every caller carry a lexer across the whole file.
///
/// `in_block` is whether the line *starts* inside a `/* */`.
pub fn line(src: &str, in_block: bool) -> Vec<Run> {
    let b = src.as_bytes();
    let mut runs: Vec<Run> = Vec::new();
    let mut i = 0usize;
    let mut block = in_block;

    let mut push = |runs: &mut Vec<Run>, start: usize, end: usize, tone: Tone| {
        if end <= start {
            return;
        }
        // Merge with the previous run when the tone matches, so a line of punctuation is one run
        // rather than forty. The pane draws a run with one `text::draw` call.
        match runs.last_mut() {
            Some(last) if last.tone == tone && last.end == start => last.end = end,
            _ => runs.push(Run { start, end, tone }),
        }
    };

    while i < b.len() {
        if block {
            // Inside a block comment: everything is comment until `*/`.
            let close = find(b, i, b"*/");
            let end = close.map(|c| c + 2).unwrap_or(b.len());
            push(&mut runs, i, end, Tone::Comment);
            i = end;
            block = close.is_none();
            continue;
        }
        if b[i..].starts_with(b"//") {
            push(&mut runs, i, b.len(), Tone::Comment);
            i = b.len();
            continue;
        }
        if b[i..].starts_with(b"/*") {
            // Search for the terminator PAST the opener, or `/*/` closes itself.
            let close = find(b, i + 2, b"*/");
            let end = close.map(|c| c + 2).unwrap_or(b.len());
            push(&mut runs, i, end, Tone::Comment);
            i = end;
            block = close.is_none();
            continue;
        }
        if b[i].is_ascii_alphabetic() || b[i] == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &src[start..i];
            let tone = if is_keyword(word) {
                Tone::Keyword
            } else if is_gate_name(word) && next_nonspace(b, i) == Some(b'(') {
                // A gate only where it is being *applied*. `h` is a perfectly ordinary variable
                // name, and setting every stray `x` at full weight would light up half the file.
                Tone::Gate
            } else {
                Tone::Plain
            };
            push(&mut runs, start, i, tone);
            continue;
        }
        let start = i;
        i += 1;
        push(&mut runs, start, i, Tone::Plain);
    }

    runs
}

/// True if a line that started with `in_block` still leaves the reader inside a `/* */`.
///
/// The one piece of cross-line state the pane has to carry. QCLang's block comments do not nest
/// (the lexer's regex matches the first `*/`), so this is a flag and not a depth.
pub fn block_comment_depth(src: &str, in_block: bool) -> bool {
    let b = src.as_bytes();
    let mut i = 0usize;
    let mut block = in_block;
    while i < b.len() {
        if block {
            match find(b, i, b"*/") {
                Some(c) => {
                    i = c + 2;
                    block = false;
                }
                None => return true,
            }
        } else if b[i..].starts_with(b"//") {
            return false;
        } else if b[i..].starts_with(b"/*") {
            i += 2;
            block = true;
        } else {
            i += 1;
        }
    }
    block
}

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    (from..hay.len().saturating_sub(needle.len() - 1)).find(|&i| &hay[i..i + needle.len()] == needle)
}

fn next_nonspace(b: &[u8], from: usize) -> Option<u8> {
    b[from..].iter().copied().find(|c| !c.is_ascii_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tones(src: &str) -> Vec<(&str, Tone)> {
        line(src, false).into_iter().map(|r| (&src[r.start..r.end], r.tone)).collect()
    }

    /// The runs must tile the line exactly. A gap drops characters from the pane and an overlap
    /// draws them twice at different weights — both look like a font bug rather than a layout one.
    #[test]
    fn runs_cover_the_line_exactly_and_in_order() {
        for src in [
            "fn main() -> int {",
            "    qubit c = CNOT(H(a), b);   // entangle",
            "",
            "}",
            "/* a block */ qubit a = |0>;",
        ] {
            let runs = line(src, false);
            let mut at = 0;
            for r in &runs {
                assert_eq!(r.start, at, "gap or overlap before {:?} in {:?}", r, src);
                assert!(r.end > r.start, "empty run in {:?}", src);
                at = r.end;
            }
            assert_eq!(at, src.len(), "runs stop short of the end of {:?}", src);
        }
    }

    /// The line QCLang's own sample actually opens with, tone by tone. `qubit` is reserved; the
    /// name it declares and the ket literal are not.
    #[test]
    fn a_declaration_sets_its_keywords_apart_from_its_names() {
        assert_eq!(
            tones("qubit a = |0>;"),
            vec![("qubit", Tone::Keyword), (" a = |0>;", Tone::Plain)]
        );
    }

    /// Same-toned neighbours are one run, so a line of punctuation costs one draw call and not one
    /// per character.
    #[test]
    fn adjacent_runs_of_the_same_tone_are_merged() {
        let runs = line("    return 0;", false);
        assert_eq!(runs.len(), 3, "{:?}", tones("    return 0;"));
        assert_eq!(tones("    return 0;"), vec![
            ("    ", Tone::Plain),
            ("return", Tone::Keyword),
            (" 0;", Tone::Plain),
        ]);
    }

    /// ★ The keyword set comes from the lexer, not from a copy of it. If someone adds a token to
    /// `Token` this passes without anyone remembering to update the highlighter — and if the two
    /// ever do drift, this is what fails.
    #[test]
    fn every_reserved_word_the_lexer_knows_is_highlighted_as_one() {
        for kw in [
            "fn", "let", "mut", "return", "if", "else", "while", "for", "in", "break", "continue",
            "qubit", "qreg", "cbit", "int", "float", "bool", "string", "qif", "qelse", "qfor",
            "range", "type", "struct", "tuple",
        ] {
            assert!(is_keyword(kw), "the lexer no longer reserves `{}`", kw);
            assert_eq!(tones(kw), vec![(kw, Tone::Keyword)], "`{}` was not set as a keyword", kw);
        }
    }

    /// ...and ordinary names are not. `main` is an identifier; so is a variable called `q0`.
    #[test]
    fn identifiers_and_literals_are_not_keywords() {
        for word in ["main", "q0", "a", "_tmp", "measure2", "123", "qubits"] {
            assert!(!is_keyword(word), "`{}` was mistaken for a reserved word", word);
        }
    }

    /// A gate is set at full weight only where it is applied. `h` as a bare variable is a name, and
    /// lighting up every `x` in the file would make the highlighting noise.
    #[test]
    fn a_gate_is_highlighted_only_where_it_is_called() {
        assert_eq!(tones("H(a)"), vec![("H", Tone::Gate), ("(a)", Tone::Plain)]);
        assert_eq!(tones("CNOT (a, b)"), vec![("CNOT", Tone::Gate), (" (a, b)", Tone::Plain)]);
        // Not a call: a variable that happens to share a gate's name stays plain.
        assert_eq!(tones("x = 1;"), vec![("x = 1;", Tone::Plain)]);
    }

    /// A line comment swallows the rest of the line, including anything that looks like code.
    #[test]
    fn a_line_comment_runs_to_the_end_of_the_line() {
        assert_eq!(
            tones("    h(q0);  // prepare the Bell state, fn qubit"),
            vec![
                ("    ", Tone::Plain),
                ("h", Tone::Gate),
                ("(q0);  ", Tone::Plain),
                ("// prepare the Bell state, fn qubit", Tone::Comment),
            ]
        );
    }

    /// Block comments span lines, which is the one thing a line-at-a-time pane needs help with.
    #[test]
    fn a_block_comment_carries_across_lines_and_ends_where_it_closes() {
        let src = ["/* the header", " * spans lines", " */ qubit a = |0>;"];
        let mut in_block = false;
        let mut got = Vec::new();
        for l in src {
            got.push(line(l, in_block));
            in_block = block_comment_depth(l, in_block);
        }
        assert!(!in_block, "the block comment never closed");
        assert!(got[0].iter().all(|r| r.tone == Tone::Comment), "{:?}", got[0]);
        assert!(got[1].iter().all(|r| r.tone == Tone::Comment), "{:?}", got[1]);
        // The third line closes the comment and then holds real code.
        // The last line is dim up to and including the closer, then live code.
        assert_eq!(got[2][0].tone, Tone::Comment);
        assert_eq!(&src[2][got[2][0].start..got[2][0].end], " */");
        assert!(got[2].iter().any(|r| r.tone == Tone::Keyword), "the code after `*/` stayed dim");
    }

    /// A `//` inside a block comment is not a line comment, and a `/*` inside a line comment does
    /// not open a block. Getting either wrong dims the rest of the file.
    #[test]
    fn comment_openers_inside_comments_are_just_text() {
        assert!(!block_comment_depth("// /* not an opener", false), "a line comment opened a block");
        assert!(block_comment_depth("/* // still open", false), "a line comment closed a block");
        assert!(!block_comment_depth("closes */ then code", true));
    }

    /// UTF-8 must survive: the runs are byte ranges, and slicing one through a codepoint panics.
    #[test]
    fn multibyte_source_is_split_on_character_boundaries() {
        let src = "let π = 3; // трижды";
        for r in line(src, false) {
            assert!(src.is_char_boundary(r.start) && src.is_char_boundary(r.end), "{:?}", r);
            let _ = &src[r.start..r.end];
        }
    }
}
