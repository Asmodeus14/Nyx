//! Stage-by-stage timing for the engine, run on the host against a real page.
//!
//! `cargo run --release --example bench -- page.html page.css`
//!
//! This exists because the alternative is guessing which stage made a laptop appear to freeze. Every
//! stage here is the same code the browser app runs; only the font is a stub.

use std::time::Instant;

use nyx_web::layout::{FontMetrics, NoImages};
use nyx_web::style::ComputedStyle;
use nyx_web::{Dom, Stylesheet};

/// Roughly DejaVu Sans proportions, without needing the real rasterizer.
struct Stub;
impl FontMetrics for Stub {
    fn text_width(&self, text: &str, style: &ComputedStyle) -> f32 {
        text.chars().count() as f32 * style.font_size * 0.52
    }
    fn line_height(&self, style: &ComputedStyle) -> f32 {
        style.font_size * 1.16
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let html = std::fs::read_to_string(&args[1]).expect("html");
    let css = args.get(2).map(|p| std::fs::read_to_string(p).expect("css")).unwrap_or_default();

    println!("input: {} KiB html, {} KiB css", html.len() / 1024, css.len() / 1024);

    let t = Instant::now();
    let dom = Dom::parse(&html);
    println!("{:>8.0} ms  Dom::parse          ({} nodes)", t.elapsed().as_secs_f64() * 1000.0, dom.nodes.len());

    let t = Instant::now();
    let sheet = Stylesheet::parse(&css);
    let selectors: usize = sheet.rules.len();
    println!("{:>8.0} ms  Stylesheet::parse   ({selectors} rules)", t.elapsed().as_secs_f64() * 1000.0);

    let t = Instant::now();
    let styles = nyx_web::style::compute(&dom, &sheet);
    println!("{:>8.0} ms  style::compute      ({} nodes x {selectors} rules)", t.elapsed().as_secs_f64() * 1000.0, dom.nodes.len());

    let t = Instant::now();
    let page = nyx_web::layout::layout(&dom, &styles, &Stub, &NoImages, 884.0);
    println!("{:>8.0} ms  layout              ({} items, {} links, {:.0}px tall)",
        t.elapsed().as_secs_f64() * 1000.0, page.items.len(), page.links.len(), page.height);

    // Relayout is what a resize and every arriving image pay for.
    let t = Instant::now();
    let _ = nyx_web::layout::layout(&dom, &styles, &Stub, &NoImages, 700.0);
    println!("{:>8.0} ms  layout (relayout)", t.elapsed().as_secs_f64() * 1000.0);
}
