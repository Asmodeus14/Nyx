//! Generates `libs/meridian/src/icons_gen.rs` from the Meridian design document.
//!
//! ```text
//! cargo run -p nyx-icons-gen -- [design-doc] [output.rs]
//! ```
//!
//! Nyx has no vector renderer and no room for one on the drawing path: icons ride the same coverage
//! atlas as text, because the GPU text shader samples one channel and that is all an icon needs.
//! So the vectors are resolved here, at build time, into the same 8-bit coverage bitmaps the font
//! rasterizer produces — after which an icon is indistinguishable from a glyph all the way to the
//! screen.
//!
//! The generated file is checked in. Regenerating it is a deliberate act (this command), not a
//! `build.rs` that runs on every compile: the design document lives in a *different repository
//! checkout* (`Nyx-ui`), and a build that silently fails when that directory is absent would be a
//! bad trade for a table that changes a few times a year.

mod brand;
mod cursors;
mod raster;
mod svg;

use std::path::{Path, PathBuf};

/// The five sizes the design renders icons at, and the stroke width it uses at each.
///
/// Straight from `01-head.html`: `.i-12` is 1.6, `.i-14` is 1.5, `.i-16` is 1.4, and `.i` / `.i-22`
/// inherit 1.25. The widths get *thinner* as the icon gets bigger, which looks backwards written
/// down and is correct on screen — the width is in 24-grid units, so it is divided by the scale.
/// In pixels they all land between 0.80 and 1.15, which is the hairline the design is after.
const DESIGN_SIZES: [(usize, f64); 5] = [(12, 1.6), (14, 1.5), (16, 1.4), (20, 1.25), (22, 1.25)];

/// Every size the shell can ask for: each design size at each interface-scale step.
///
/// ★ Must stay identical to `nyx_meridian::scale::required_icon_sizes()`, which has a test pinning
/// it. Icons are rasterized here, at build time, with a stroke tuned per size — `icons::get` refuses
/// a size it was not drawn at rather than scaling a bitmap, so a size missing from this list is a
/// piece of chrome that silently draws nothing at one interface scale and fine at every other.
const SIZES: [usize; 16] = [12, 14, 15, 16, 18, 20, 21, 22, 24, 25, 28, 30, 32, 33, 40, 44];

/// Stroke width in 24-grid units for any size.
///
/// The design's own five are pinned exactly, so nothing about a 1:1 panel changes. Beyond them the
/// same line is continued in PIXEL width — which is the quantity that actually has to stay a
/// hairline — rather than in grid units, which would make a 44px icon's stroke twice as heavy as a
/// 22px one's and turn the set from line art into something drawn with a marker.
fn stroke_for(px: usize) -> f64 {
    for &(p, s) in &DESIGN_SIZES {
        if p == px {
            return s;
        }
    }
    // Fitted through the design's endpoints: 0.800px of ink at 12px, 1.146px at 22px.
    let ink = 0.800 + 0.03462 * (px as f64 - 12.0);
    ink * 24.0 / px as f64
}

/// The design parts that define symbols Meridian actually draws, relative to the `Nyx-ui` root.
///
/// `02-icons` is the interface set. `14-brand` is pulled in for one symbol — `nyx-mark`, which the
/// Command's footer draws. The brand file also defines the wordmark and the lockup; those are for
/// the boot screen and the About surface (build-order steps 7b and 8) and are skipped here rather
/// than carried as dead coverage in every build.
const SOURCES: [(&str, &[&str]); 2] = [
    ("design/ver3.0/parts/02-icons.html", &[]),
    ("design/ver3.0/parts/14-brand.html", &["nyx-mark"]),
];

/// Print an icon as ASCII art instead of generating the table.
///
/// The unit tests prove the rasterizer handles synthetic geometry correctly, which is not the same
/// as proving `#a-browser` looks like a globe. A mis-parsed arc flag or a dropped contour produces
/// a perfectly valid bitmap of the wrong picture, and no assertion is going to notice. Looking at
/// it is the check — and on a machine where the alternative is a power cycle to find out, being
/// able to look without booting is the entire point of doing this at build time.
fn preview(root: &Path, id: &str) {
    for (rel, only) in SOURCES {
        let src = match std::fs::read_to_string(root.join(rel)) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let filter = if only.is_empty() { vec![id] } else { only.to_vec() };
        let found = match svg::parse_document(&src, &filter) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {}", e);
                std::process::exit(1);
            }
        };
        for sym in found.iter().filter(|s| s.id == id) {
            for &px in &SIZES {
            let stroke = stroke_for(px);
                let bm = raster::rasterize(sym, px, stroke);
                println!("\n{} @ {}px  ({}x{})", sym.id, px, bm.w, bm.h);
                for y in 0..bm.h {
                    let row: String = (0..bm.w)
                        .map(|x| match bm.cov[y * bm.w + x] {
                            0 => ' ',
                            1..=63 => '.',
                            64..=127 => ':',
                            128..=191 => '*',
                            _ => '#',
                        })
                        .collect();
                    println!("|{}|", row);
                }
            }
            return;
        }
    }
    eprintln!("error: no symbol '{}' in any configured source", id);
    std::process::exit(1);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--preview") {
        let id = args.get(pos + 1).map(|s| s.as_str()).unwrap_or("");
        let root = args.get(1).filter(|a| *a != "--preview").map(PathBuf::from)
            .unwrap_or_else(default_ui_root);
        preview(&root, id);
        return;
    }
    let root = args.get(1).map(PathBuf::from).unwrap_or_else(default_ui_root);
    if args.iter().any(|a| a == "--preview-cursors") {
        for c in load_cursors(&root) {
            cursors::preview(&c);
        }
        return;
    }
    if args.iter().any(|a| a == "--preview-brand") {
        for a in load_brand(&root) {
            brand::preview(&a);
        }
        return;
    }
    let out = args
        .get(2)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("libs/meridian/src/icons_gen.rs"));

    // Cursors first: they are the cheaper of the two and share the same parser, so a design document
    // this tool cannot read fails in a second rather than after rasterizing 576 icons.
    let set = load_cursors(&root);
    let cur_out = out.with_file_name("cursors_gen.rs");
    write_out(&cur_out, &cursors::emit(&set));
    println!("wrote {} ({} cursors)", cur_out.display(), set.len());

    // The identity goes to the KERNEL, not to `libs/meridian` — it is the one asset the boot screen
    // needs before userspace exists. Absolute path from the workspace root rather than relative to
    // `out`, because the two land in different crates.
    let assets = load_brand(&root);
    let bytes: usize = assets.iter().map(|a| a.cov.len()).sum();
    let text = brand::emit(&assets);
    // Written to BOTH crates. The kernel needs it for cold-start stages 1 and 2; `libs/meridian`
    // needs the identical bitmap for stage 3, where the shell picks the mark up from exactly where
    // the kernel left it and carries it to the corner. Two copies of ~14 KB is the cost of not
    // inventing a way to hand a static table across the ring boundary — and they cannot drift,
    // because one command writes both from one source.
    for out in [brand::out_path(), brand::meridian_out_path()] {
        write_out(out, &text);
        println!("wrote {} ({} brand assets, {} bytes of coverage)", out.display(), assets.len(), bytes);
    }

    let mut symbols = Vec::new();
    for (rel, only) in SOURCES {
        let doc = root.join(rel);
        let src = match std::fs::read_to_string(&doc) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read design document {}: {}", doc.display(), e);
                eprintln!(
                    "hint: pass the Nyx-ui checkout root explicitly:\n\
                     \tcargo run -p nyx-icons-gen -- ../Nyx-ui"
                );
                std::process::exit(1);
            }
        };
        let found = match svg::parse_document(&src, only) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: {}: {}", rel, e);
                std::process::exit(1);
            }
        };
        if found.is_empty() {
            eprintln!("error: no matching <symbol> definitions found in {}", doc.display());
            std::process::exit(1);
        }
        // A filter entry that matches nothing means the design was reorganised and this build is
        // quietly dropping an icon it believes it has.
        for want in only {
            if !found.iter().any(|s| s.id == *want) {
                eprintln!("error: {} does not define a symbol '{}'", rel, want);
                std::process::exit(1);
            }
        }
        symbols.extend(found);
    }

    let mut body = String::new();
    let mut entries = String::new();
    let mut total = 0usize;

    for sym in &symbols {
        let ident = sanitize(&sym.id);
        let mut per_size = Vec::new();
        for &px in &SIZES {
            let stroke = stroke_for(px);
            let bm = raster::rasterize(sym, px, stroke);
            let (c, ox, oy) = raster::crop(&bm);
            if c.w == 0 {
                eprintln!(
                    "error: icon '{}' rasterized to nothing at {}px. Its geometry is outside the \
                     24 grid, or every shape was skipped.",
                    sym.id, px
                );
                std::process::exit(1);
            }
            total += c.cov.len();
            let name = format!("{}_{}", ident, px);
            body.push_str(&emit_bytes(&name, &c.cov));
            per_size.push(format!(
                "        Icon {{ w: {}, h: {}, off_x: {}, off_y: {}, cov: &{} }},",
                c.w, c.h, ox, oy, name
            ));
        }
        entries.push_str(&format!(
            "    IconSet {{\n        id: {:?},\n        sizes: [\n{}\n        ],\n    }},\n",
            sym.id,
            per_size.join("\n")
        ));
    }

    let file = format!(
        "{}\n\n{}\n\
         /// Every icon in the Meridian set, in the order the design document defines them.\n\
         pub static ICONS: [IconSet; {}] = [\n{}];\n\
         \n\
         /// Look an icon up by its design-document id, e.g. `\"a-files\"`.\n\
         ///\n\
         /// Linear over ~3 dozen entries. The shell resolves an icon once when it builds the atlas,\n\
         /// not per frame, so a map would be machinery for a cost nothing pays.\n\
         pub fn find(id: &str) -> Option<&'static IconSet> {{\n\
         \x20   ICONS.iter().find(|i| i.id == id)\n\
         }}\n\
         \n\
         /// The index into [`SIZES`] for `px`, or None if the set is not rasterized at that size.\n\
         pub fn size_index(px: usize) -> Option<usize> {{\n\
         \x20   SIZES.iter().position(|&s| s == px)\n\
         }}\n",
        header(symbols.len(), total),
        body,
        symbols.len(),
        entries
    );

    write_out(&out, &file);
    println!(
        "wrote {} ({} icons x {} sizes, {} bytes of coverage)",
        out.display(),
        symbols.len(),
        SIZES.len(),
        total
    );
}

fn write_out(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(path, contents) {
        eprintln!("error: cannot write {}: {}", path.display(), e);
        std::process::exit(1);
    }
}

fn load_brand(root: &Path) -> Vec<brand::Asset> {
    let doc = root.join(brand::SOURCE);
    let src = match std::fs::read_to_string(&doc) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read design document {}: {}", doc.display(), e);
            std::process::exit(1);
        }
    };
    match brand::build(&src) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {}: {}", brand::SOURCE, e);
            std::process::exit(1);
        }
    }
}

fn load_cursors(root: &Path) -> Vec<cursors::Composed> {
    let doc = root.join(cursors::SOURCE);
    let src = match std::fs::read_to_string(&doc) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: cannot read design document {}: {}", doc.display(), e);
            std::process::exit(1);
        }
    };
    match cursors::build(&src) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {}: {}", cursors::SOURCE, e);
            std::process::exit(1);
        }
    }
}

/// The design lives in a sibling checkout. Guessing it saves typing the path in the common case
/// without pretending the location is fixed.
pub fn default_ui_root() -> PathBuf {
    for base in ["../Nyx-ui", "../../Nyx-ui", "/mnt/c/Code/Nyx-ui", "C:/Code/Nyx-ui"] {
        let p = Path::new(base);
        if p.join("design/ver3.0/parts/02-icons.html").exists() {
            return p.to_path_buf();
        }
    }
    PathBuf::from("../Nyx-ui")
}

/// `a-files` → `A_FILES`, usable as a Rust const name.
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' })
        .collect()
}

fn emit_bytes(name: &str, data: &[u8]) -> String {
    let mut s = format!("static {}: [u8; {}] = [\n", name, data.len());
    for chunk in data.chunks(24) {
        s.push_str("    ");
        for b in chunk {
            s.push_str(&format!("{},", b));
        }
        s.push('\n');
    }
    s.push_str("];\n");
    s
}

fn header(count: usize, bytes: usize) -> String {
    let srcs: Vec<&str> = SOURCES.iter().map(|(p, _)| *p).collect();
    format!(
        "// @generated by `cargo run -p nyx-icons-gen` — DO NOT EDIT BY HAND.\n\
         //\n\
         // Source: Nyx-ui/{}\n\
         // {} icons, rasterized at {:?} px.\n\
         // {} bytes of 8-bit coverage.\n\
         //\n\
         // The design document is the single source of truth for this geometry. To change an icon,\n\
         // change it there and re-run the generator — editing the coverage here would put the two\n\
         // out of sync with nothing to detect it.\n\
         //\n\
         // Coverage is the same 8-bit format the TTF rasterizer emits, so icons and text share one\n\
         // atlas and one GPU path. `off_x`/`off_y` are where the cropped bitmap sits inside the\n\
         // icon's box, so a caller positions by the box and never has to know an icon was trimmed.\n\
         \n\
         /// One icon at one size. `cov` is `w * h` bytes, row-major, 0..=255 coverage.\n\
         pub struct Icon {{\n\
         \x20   pub w: u8,\n\
         \x20   pub h: u8,\n\
         \x20   pub off_x: u8,\n\
         \x20   pub off_y: u8,\n\
         \x20   pub cov: &'static [u8],\n\
         }}\n\
         \n\
         /// One icon at every size the design renders it at.\n\
         pub struct IconSet {{\n\
         \x20   /// The `<symbol>` id from the design document, e.g. `\"a-files\"`.\n\
         \x20   pub id: &'static str,\n\
         \x20   /// Parallel to [`SIZES`].\n\
         \x20   pub sizes: [Icon; {}],\n\
         }}\n\
         \n\
         /// Pixel sizes, in the order [`IconSet::sizes`] stores them.\n\
         pub static SIZES: [usize; {}] = {:?};\n",
        srcs.join(", Nyx-ui/"),
        count,
        SIZES,
        bytes,
        SIZES.len(),
        SIZES.len(),
        SIZES,
    )
}
