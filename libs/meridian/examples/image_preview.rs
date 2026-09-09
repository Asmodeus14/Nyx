//! Render the Image interior to a PNG on the development host, without booting.
//!
//! ```text
//! cargo run --release -p nyx-meridian --example image_preview -- /tmp/iv.png [--light] \
//!     [--size 672x544] [--img 1280x720] [--bar] [--hover N] [--actual] [--pan 40,-20]
//! ```
//!
//! The picture inside the frame is generated, not decoded — it is the design's own two radial
//! washes over a dark linear gradient. That is deliberate: this preview exists to check the
//! **chrome** (the ground, the frame's carved corners, the floating bar, the hover tone), and a real
//! decode would only prove `libs/image` still works, which its own tests already do.
//!
//! ⚠️ The drawing is a second implementation of `apps/imageviewer`'s, because that app is a `no_std`
//! binary with a `_start` and cannot be imported here. All the *geometry* comes from
//! `nyx_meridian::viewer` so it cannot drift; the order and colour of marks can.
//!
//! The caption line is not drawn — it belongs to the window server. The context the app would hand
//! it is printed to stdout instead.

#[path = "common/png.rs"]
mod png;
use png::write_png;

use nyx_gui::canvas::Canvas;
use nyx_meridian::layout::Rect;
use nyx_meridian::shapes;
use nyx_meridian::text;
use nyx_meridian::tokens::{Theme, B3};
use nyx_meridian::viewer;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}
fn flag(name: &str) -> bool {
    std::env::args().any(|x| x == name)
}
fn pair(name: &str, sep: char) -> Option<(i32, i32)> {
    arg(name).and_then(|s| {
        let (a, b) = s.split_once(sep)?;
        Some((a.parse().ok()?, b.parse().ok()?))
    })
}

/// The design's own frame content: two radial washes over a dark diagonal gradient.
fn test_image(w: i32, h: i32) -> Vec<u32> {
    let mut px = vec![0u32; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            // linear-gradient(152deg, #101215, #0B0C0E)
            let t = (fx * 0.47 + fy * 0.88).clamp(0.0, 1.0);
            let mut r = 0x10 as f32 + (0x0B as f32 - 0x10 as f32) * t;
            let mut g = 0x12 as f32 + (0x0C as f32 - 0x12 as f32) * t;
            let mut b = 0x15 as f32 + (0x0E as f32 - 0x15 as f32) * t;
            // radial-gradient(72% 62% at 24% 20%, rgba(70,118,240,.17), transparent 62%)
            let d = (((fx - 0.24) / 0.72).powi(2) + ((fy - 0.20) / 0.62).powi(2)).sqrt();
            let a = (1.0 - d / 0.62).clamp(0.0, 1.0) * 0.17;
            r += (70.0 - r) * a;
            g += (118.0 - g) * a;
            b += (240.0 - b) * a;
            // radial-gradient(58% 58% at 80% 78%, rgba(102,114,132,.20), transparent 62%)
            let d = (((fx - 0.80) / 0.58).powi(2) + ((fy - 0.78) / 0.58).powi(2)).sqrt();
            let a = (1.0 - d / 0.62).clamp(0.0, 1.0) * 0.20;
            r += (102.0 - r) * a;
            g += (114.0 - g) * a;
            b += (132.0 - b) * a;
            px[(y * w + x) as usize] =
                0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32;
        }
    }
    px
}

/// Nearest-neighbour, exactly as the app resamples.
fn scale(src: &[u32], sw: i32, sh: i32, dw: i32, dh: i32) -> Vec<u32> {
    let mut out = vec![0u32; (dw * dh) as usize];
    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            out[(y * dw + x) as usize] = src[(sy * sw + x * sw / dw) as usize];
        }
    }
    out
}

#[allow(non_snake_case)]
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "image.png".into());
    let t = if flag("--light") { Theme::light() } else { Theme::dark() };
    let (W, H) = pair("--size", 'x').unwrap_or((672, 544));
    let (iw, ih) = pair("--img", 'x').unwrap_or((1280, 720));
    let fit = !flag("--actual");
    let pan = pair("--pan", ',').unwrap_or((0, 0));
    let hover: Option<usize> = arg("--hover").and_then(|s| s.parse().ok());
    let show_bar = flag("--bar") || hover.is_some();

    nyx_meridian::font::register_text();

    let surface = Rect::new(0, 0, W, H);
    let mut buf = vec![0u32; (W * H) as usize];
    let mut canvas = Canvas::new(&mut buf, W as usize, H as usize);

    let ground = if t.is_dark { t.sunken } else { t.surface };
    canvas.fill_rect(0, 0, W as usize, H as usize, ground);

    let (dw, dh) = viewer::drawn_size(surface, iw, ih, fit);
    let f = viewer::frame(surface, dw, dh, pan);
    println!(
        "caption context would read:  apps / sample.png \u{00B7} {}\u{00D7}{} \u{00B7} 214.0 KB   \
         (drawn {}\u{00D7}{} at {}%)",
        iw, ih, dw, dh, viewer::zoom_pct(iw, dw)
    );

    let src = test_image(iw, ih);
    let scaled = scale(&src, iw, ih, dw, dh);

    // Blit only the visible sub-rectangle, exactly as the app does for a panned image.
    let (sx, sy) = ((-f.x).max(0), (-f.y).max(0));
    let vis_w = (f.w - sx).min(W - f.x.max(0)).max(0);
    let vis_h = (f.h - sy).min(H - f.y.max(0)).max(0);
    if vis_w > 0 && vis_h > 0 {
        let mut rows: Vec<u32> = Vec::with_capacity((vis_w * vis_h) as usize);
        for y in 0..vis_h {
            let start = ((sy + y) * f.w + sx) as usize;
            rows.extend_from_slice(&scaled[start..start + vis_w as usize]);
        }
        canvas.composite_buffer(
            f.x.max(0) as usize, f.y.max(0) as usize,
            &rows, vis_w as usize, vis_h as usize, 255,
        );
        if f.x >= 0 && f.y >= 0 && f.right() <= W && f.bottom() <= H {
            shapes::carve_round_corners(
                &mut canvas, f.x as usize, f.y as usize, f.w as usize, f.h as usize,
                viewer::frame_radius(), |_, _| ground,
            );
            shapes::stroke_round_rect(
                &mut canvas, f.x as usize, f.y as usize, f.w as usize, f.h as usize,
                viewer::frame_radius(), t.line,
            );
        }
    }

    if show_bar {
        let bar = viewer::bar(surface);
        canvas.fill_rect(bar.x as usize, bar.y as usize, bar.w as usize, bar.h as usize, t.chrome);
        shapes::hairline_h(&mut canvas, bar.x as usize, bar.y as usize, bar.w as usize, t.line);

        let labels = [
            format!("{}%", viewer::zoom_pct(iw, dw)),
            String::from(if fit { "Actual size" } else { "Fit" }),
            String::from("Previous"),
            String::from("Next"),
            String::from("Reveal in Files"),
        ];
        let widths: Vec<i32> = labels.iter().map(|s| text::width(B3, s)).collect();
        for (i, label) in labels.iter().enumerate() {
            let Some(r) = viewer::bar_item(surface, &widths, 2, i) else { continue };
            let col = if i == 0 {
                t.fg_2
            } else if hover == Some(i) {
                t.fg
            } else {
                t.fg_3
            };
            text::draw(&mut canvas, r.x, text::centre_y(r.y, r.h, B3), B3, col, label);
        }
    }

    write_png(&out, &buf, W as usize, H as usize).expect("write png");
    println!("wrote {} ({}x{}, {})", out, W, H, if t.is_dark { "dark" } else { "light" });
}
