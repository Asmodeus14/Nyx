//! Render the Text interior to a PNG on the development host, without booting.
//!
//! ```text
//! cargo run --release -p nyx-meridian --example text_preview -- /tmp/tx.png [--light] \
//!     [--size 592x544] [--src doc.md] [--scroll 120] [--caret 4,20]
//! ```
//!
//! The default document is `apps/notepad/samples/atlas-sizes.md`, which is the design's own scene
//! transcribed — so the picture this writes and the picture in `13-apps.html` are of the same words
//! and can be laid side by side.
//!
//! ⚠️ The drawing is a second implementation of `apps/notepad`'s, because that app is not a
//! workspace member (std target, `build-std`) and cannot be imported. All the *geometry* comes from
//! `nyx_meridian::prose` so it cannot drift; the order and colour of marks can. Same caveat as the
//! other previews.
//!
//! The caption line is not drawn here — it belongs to the window server, and the whole point of
//! step 15 is that the filename and the word count are **outside** the surface. The count is printed
//! to stdout instead, which is what the shell would be handed.

#[path = "common/png.rs"]
mod png;
use png::write_png;

use nyx_gui::canvas::Canvas;
use nyx_meridian::layout::Rect;
use nyx_meridian::prose::{self, Block, Row};
use nyx_meridian::text;
use nyx_meridian::tokens::{Style, Theme};

const SAMPLE: &str = include_str!("../../../apps/notepad/samples/atlas-sizes.md");

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}
fn flag(name: &str) -> bool {
    std::env::args().any(|x| x == name)
}

/// The app's own classifier, kept identical.
fn classify(line: &str) -> (Block, usize) {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if hashes == 0 || line.as_bytes().get(hashes) != Some(&b' ') {
        return (Block::Body, 0);
    }
    (if hashes == 1 { Block::H1 } else { Block::H2 }, hashes + 1)
}

fn emphasis_at(src: &str, at: usize) -> bool {
    let mut count = 0;
    let mut i = 0;
    while i + 2 <= at {
        if src[i..].starts_with("**") {
            count += 1;
            i += 2;
        } else {
            i += src[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    count % 2 == 1
}

fn draw_runs(canvas: &mut Canvas, x: i32, y: i32, t: &Theme, style: Style, src: &str,
             range: core::ops::Range<usize>, marker: usize) {
    let mut pen = x;
    let mut emph = emphasis_at(src, range.start);
    let mut i = range.start;
    while i < range.end {
        if i < marker {
            let end = marker.min(range.end);
            pen += text::draw(canvas, pen, y, style, t.fg_4, &src[i..end]);
            i = end;
            continue;
        }
        if src[i..].starts_with("**") && i + 2 <= range.end {
            pen += text::draw(canvas, pen, y, style, t.fg_4, "**");
            emph = !emph;
            i += 2;
            continue;
        }
        let next = src[i..range.end].find("**").map(|o| i + o).unwrap_or(range.end);
        let next = if next == i { range.end } else { next };
        pen += text::draw(canvas, pen, y, style, if emph { t.fg } else { t.fg_2 }, &src[i..next]);
        i = next;
    }
}

#[allow(non_snake_case)]
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "text.png".into());
    let t = if flag("--light") { Theme::light() } else { Theme::dark() };
    let (W, H) = match arg("--size").and_then(|s| {
        let (a, b) = s.split_once('x')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }) {
        Some(v) => v,
        None => (592usize, 544usize),
    };
    let doc = match arg("--src") {
        Some(p) => std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {}", p, e)),
        None => SAMPLE.to_string(),
    };
    let scroll: i32 = arg("--scroll").and_then(|s| s.parse().ok()).unwrap_or(0);
    // `--caret ROW,BYTE` places the caret on a visual row, so the 1px accent rule is in the sheet.
    let caret: (usize, usize) = arg("--caret")
        .and_then(|s| {
            let (a, b) = s.split_once(',')?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((0, usize::MAX));

    nyx_meridian::font::register_text();

    let lines: Vec<&str> = doc.split('\n').collect();
    let mut buf = vec![0u32; W * H];
    let mut canvas = Canvas::new(&mut buf, W, H);
    canvas.fill_rect(0, 0, W, H, t.surface);

    let col = prose::column(W as i32, H as i32);

    // Lay the document out exactly as the app does: wrap, then let `prose::rows` decide which rows
    // carry margins.
    let mut ranges: Vec<Vec<core::ops::Range<usize>>> = Vec::new();
    let mut src: Vec<prose::Source> = Vec::new();
    for line in &lines {
        let (block, _) = classify(line);
        let r = prose::wrap(block.style(), line, col.w);
        src.push(prose::Source { block, wrapped: r.len(), blank: line.trim().is_empty() });
        ranges.push(r);
    }
    let kinds = prose::rows(&src);
    let mut rows: Vec<(usize, core::ops::Range<usize>, Row)> = Vec::new();
    let mut flat = kinds.iter().copied();
    for (i, rs) in ranges.into_iter().enumerate() {
        for r in rs {
            rows.push((i, r, flat.next().unwrap()));
        }
    }

    let words: usize = lines.iter().map(|l| prose::word_count(l)).sum();
    println!(
        "caption context would read:  atlas-sizes.md \u{00B7} {} words   ({} rows, {}px tall, measure {}px)",
        words,
        rows.len(),
        prose::height(&kinds),
        col.w
    );

    for i in 0..rows.len() {
        let Some(r) = prose::row(col, &kinds, i) else { continue };
        let y = r.y - scroll;
        if y + r.h < 0 || y >= H as i32 {
            continue;
        }
        let (src, range, row) = (&rows[i].0, rows[i].1.clone(), rows[i].2);
        let line = lines[*src];
        let (_, marker) = classify(line);
        draw_runs(
            &mut canvas, col.x, y, &t, row.block.style(), line, range.clone(),
            if row.first { marker } else { 0 },
        );
        if i == caret.0 {
            let at = caret.1.clamp(range.start, range.end);
            let x = col.x + text::width(row.block.style(), &line[range.start..at]);
            let c = prose::caret(Rect::new(r.x, y, r.w, r.h), x);
            canvas.fill_rect(c.x as usize, c.y as usize, c.w as usize, c.h as usize, t.accent);
        }
    }

    write_png(&out, &buf, W, H).expect("write png");
    println!("wrote {} ({}x{}, {})", out, W, H, if t.is_dark { "dark" } else { "light" });
}
