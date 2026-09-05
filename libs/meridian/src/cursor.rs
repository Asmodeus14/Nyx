//! The twelve Meridian cursors, ready for the hardware cursor plane.
//!
//! The pointer is the only element of Nyx that is on screen literally always, and it is also the
//! cheapest thing in the whole design: the cursor plane is composited by the SCANOUT engine after
//! everything else, so a shape costs no quad, no frame and no compositor work. Moving it is one
//! 32-bit register write from the mouse IRQ; changing it is one 16 KB copy. There is no per-frame
//! cost at all.
//!
//! The pixels are composited at build time — see `tools/icons/src/cursors.rs`. All this module does
//! is place a finished bitmap into the 64×64 plane and hand back its hotspot.
//!
//! ## Why the plane is built on demand and not cached
//!
//! The design costs the set at "twelve shapes × 16 KB = 192 KB, generated once at shell start into
//! a static table". Held to literally that is 192 KB of RAM permanently resident to save composing
//! about 500 pixels on the handful of occasions per session that the shape changes. The bitmaps
//! themselves are already static (46 KB, cropped, in `.rodata`); this expands one of them into a
//! caller-owned buffer at the moment of upload instead.

use crate::cursors_gen::{self, Cursor};

/// The width and height of the hardware cursor plane. Fixed by the display controller: CURCNTR mode
/// `MCURSOR_MODE_64_ARGB_AX` is 64×64 ARGB and there is no other mode Nyx uses.
pub const PLANE: usize = 64;

/// Every shape in the set, in the order the design's specimen grid lists them.
///
/// Seven of the twelve are reachable from the shell today. The rest wait on machinery that does not
/// exist yet — `Precision` needs an app to be able to request a cursor, `Grab`/`Grabbing` need
/// draggable objects (Overview thumbnails, dock reordering), `Unavailable` needs a drop target that
/// can refuse. They are generated and tested anyway because they are one table entry each and
/// because the design is the thing being implemented, not a subset of it that happens to be wired.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// Everything not listed below — the wallpaper, the dock, the Entity, caption lines.
    Pointer,
    /// Terminal, the Command query, any editable field.
    Text,
    /// Image at 1:1 or greater, QCLang's circuit canvas. Application request.
    Precision,
    /// A caption line being dragged, a folded window being repositioned.
    Move,
    ResizeH,
    ResizeV,
    ResizeNwse,
    ResizeNesw,
    /// Hovering something draggable.
    Grab,
    /// Held mid-drag. Accent-tinted.
    Grabbing,
    /// The Entity is working and the pointer is over the responsible window. Accent-tinted.
    Working,
    /// A drop target that will refuse, or a control disabled by system state.
    Unavailable,
}

impl Shape {
    /// The `<symbol>` id in the design document.
    pub const fn id(self) -> &'static str {
        match self {
            Shape::Pointer => "c-pointer",
            Shape::Text => "c-text",
            Shape::Precision => "c-precision",
            Shape::Move => "c-move",
            Shape::ResizeH => "c-resize-h",
            Shape::ResizeV => "c-resize-v",
            Shape::ResizeNwse => "c-resize-nwse",
            Shape::ResizeNesw => "c-resize-nesw",
            Shape::Grab => "c-grab",
            Shape::Grabbing => "c-grabbing",
            Shape::Working => "c-working",
            Shape::Unavailable => "c-unavailable",
        }
    }

    /// Every shape, for tests and for anything that wants to walk the set.
    pub const ALL: [Shape; 12] = [
        Shape::Pointer,
        Shape::Text,
        Shape::Precision,
        Shape::Move,
        Shape::ResizeH,
        Shape::ResizeV,
        Shape::ResizeNwse,
        Shape::ResizeNesw,
        Shape::Grab,
        Shape::Grabbing,
        Shape::Working,
        Shape::Unavailable,
    ];

    fn entry(self) -> &'static Cursor {
        // Positional, so it cannot miss: `ALL` and `CURSORS` are both in the design's order, and
        // `the_two_orders_agree` below pins that. A name lookup would be a runtime failure mode for
        // something that is decided at build time.
        &cursors_gen::CURSORS[self as usize]
    }

    /// Which resize shape an edge bitmask calls for, or `None` for "not on an edge".
    ///
    /// The bits are the shell's `window_resize_edges` mask: 1 = left, 2 = right, 4 = top, 8 =
    /// bottom. Living here rather than in the shell keeps the diagonal pairings in one place — the
    /// mapping that is easy to get subtly wrong is 5/10 (both NW–SE) versus 6/9 (both NE–SW).
    pub const fn for_edges(mask: u8) -> Option<Shape> {
        match mask {
            1 | 2 => Some(Shape::ResizeH),
            4 | 8 => Some(Shape::ResizeV),
            5 | 10 => Some(Shape::ResizeNwse),
            6 | 9 => Some(Shape::ResizeNesw),
            _ => None,
        }
    }
}

/// Paint `shape` into a 64×64 ARGB plane and return its hotspot as `(x, y)`.
///
/// `out` is cleared first — the plane is persistent hardware state, and a smaller shape replacing a
/// larger one would otherwise leave the difference on screen forever.
pub fn render(shape: Shape, out: &mut [u32; PLANE * PLANE]) -> (u32, u32) {
    let c = shape.entry();
    out.fill(0);
    for y in 0..c.h as usize {
        let dy = y + c.off_y as usize;
        if dy >= PLANE {
            break;
        }
        for x in 0..c.w as usize {
            let dx = x + c.off_x as usize;
            if dx >= PLANE {
                break;
            }
            out[dy * PLANE + dx] = c.argb[y * c.w as usize + x];
        }
    }
    (c.hot_x as u32, c.hot_y as u32)
}

/// Draw `shape` into a framebuffer with its hotspot at `(px, py)` — the software fallback for when
/// the cursor plane could not be brought up.
///
/// Same bitmap as the hardware path, so a machine that falls back looks the same rather than looking
/// like a different, older system. Clipped on all four sides: a centred shape at the top-left corner
/// is *supposed* to hang off, which is the whole point of carrying a hotspot.
pub fn blit(
    shape: Shape,
    buf: &mut [u32],
    stride: usize,
    screen_h: usize,
    px: i32,
    py: i32,
) {
    let c = shape.entry();
    let ox = px - c.hot_x as i32 + c.off_x as i32;
    let oy = py - c.hot_y as i32 + c.off_y as i32;
    for y in 0..c.h as i32 {
        let dy = oy + y;
        if dy < 0 || dy as usize >= screen_h {
            continue;
        }
        for x in 0..c.w as i32 {
            let dx = ox + x;
            if dx < 0 || dx as usize >= stride {
                continue;
            }
            let s = c.argb[y as usize * c.w as usize + x as usize];
            let a = s >> 24;
            if a == 0 {
                continue;
            }
            let i = dy as usize * stride + dx as usize;
            buf[i] = if a == 255 { s | 0xFF00_0000 } else { blend(buf[i], s, a) };
        }
    }
}

/// Straight-alpha source-over of one pixel. Integer throughout: `/ 255` rather than `>> 8`, because
/// the shifted form never quite reaches the source colour at full alpha and leaves the white body
/// looking faintly grey.
fn blend(dst: u32, src: u32, a: u32) -> u32 {
    let inv = 255 - a;
    let ch = |sh: u32| {
        let s = (src >> sh) & 0xFF;
        let d = (dst >> sh) & 0xFF;
        ((s * a + d * inv) / 255) << sh
    };
    0xFF00_0000 | ch(16) | ch(8) | ch(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_orders_agree() {
        // `entry()` indexes the generated table by the enum's discriminant. If the design ever
        // reorders its specimen grid, or a shape is inserted, every cursor after the insertion point
        // silently becomes a different picture — with the right hotspot for the wrong shape.
        for (i, s) in Shape::ALL.iter().enumerate() {
            assert_eq!(*s as usize, i);
            assert_eq!(s.entry().id, s.id(), "index {}", i);
        }
        assert_eq!(cursors_gen::CURSORS.len(), Shape::ALL.len());
    }

    #[test]
    fn every_shape_fits_the_plane_and_leaves_no_residue() {
        // The clear is the point: switching from the 25x25 Unavailable to the 4x16 Text without it
        // leaves most of a cursor on screen, permanently, because nothing else ever writes this
        // buffer.
        let mut plane = [0u32; PLANE * PLANE];
        let big = render(Shape::Unavailable, &mut plane);
        let painted = plane.iter().filter(|&&p| p != 0).count();
        assert!(painted > 200, "Unavailable painted {} pixels", painted);

        for s in Shape::ALL {
            let (hx, hy) = render(s, &mut plane);
            let c = s.entry();
            assert!(
                c.off_x as usize + c.w as usize <= PLANE && c.off_y as usize + c.h as usize <= PLANE,
                "{} does not fit the plane",
                s.id()
            );
            assert!(hx < PLANE as u32 && hy < PLANE as u32, "{} hotspot off-plane", s.id());
            // Nothing may survive outside this shape's own box.
            for y in 0..PLANE {
                for x in 0..PLANE {
                    let inside = x >= c.off_x as usize
                        && x < c.off_x as usize + c.w as usize
                        && y >= c.off_y as usize
                        && y < c.off_y as usize + c.h as usize;
                    if !inside {
                        assert_eq!(plane[y * PLANE + x], 0, "{} left residue at {},{}", s.id(), x, y);
                    }
                }
            }
        }
        assert_ne!(big, (0, 0), "the pointer shapes still carry the bleed offset");
    }

    #[test]
    fn the_pointer_is_the_default_and_points_at_its_tip() {
        let mut plane = [0u32; PLANE * PLANE];
        let (hx, hy) = render(Shape::Pointer, &mut plane);
        // Grid (0,0) plus the design's 2-unit viewBox bleed.
        assert_eq!((hx, hy), (2, 2));
        assert!(plane[hy as usize * PLANE + hx as usize] >> 24 > 0, "the tip must be opaque");
    }

    #[test]
    fn the_centred_shapes_hang_off_the_top_left_corner() {
        // The whole reason the kernel needed a hotspot. A shape centred at (14,14) put at pointer
        // (0,0) has to place its plane at (-14,-14) — which is what CURPOS's sign bits are for and
        // what the driver used to mask away.
        let mut plane = [0u32; PLANE * PLANE];
        for s in [Shape::ResizeH, Shape::Move, Shape::Grab, Shape::Working] {
            let (hx, hy) = render(s, &mut plane);
            assert_eq!((hx, hy), (14, 14), "{}", s.id());
        }
    }

    #[test]
    fn the_software_blit_lands_the_hotspot_where_it_was_asked_to() {
        // A 40x40 patch of white. Pointing at (20,20) must put the pointer's dark tip there, not
        // its top-left corner — the software path has to honour the hotspot too or the fallback
        // clicks somewhere the hardware path does not.
        const W: usize = 40;
        let mut buf = [0xFFFF_FFFFu32; W * W];
        blit(Shape::Pointer, &mut buf, W, W, 20, 20);
        assert!(buf[20 * W + 20] != 0xFFFF_FFFF, "the tip should be inked at the hotspot");
        assert_eq!(buf[18 * W + 18], 0xFFFF_FFFF, "two pixels up-left of the tip is still clear");
        // The arrow extends down and right from its tip. The only thing above the tip is the dark
        // contrast edge, which wraps it — and that reaches exactly the 2px of viewBox bleed, never
        // further. More than that would mean the hotspot was applied in the wrong direction.
        let above: usize = (0..18)
            .map(|y| (0..W).filter(|&x| buf[y * W + x] != 0xFFFF_FFFF).count())
            .sum();
        assert_eq!(above, 0, "{} pixels painted more than 2px above the tip", above);
    }

    #[test]
    fn a_centred_shape_clips_off_the_corner_instead_of_wrapping() {
        // Pointing at (2,2) with a shape whose hotspot is (14,14) puts most of it off-screen. The
        // failure this guards is not a crash — it is the row wrapping around and painting a stripe
        // down the right-hand edge of the screen.
        const W: usize = 48;
        let mut buf = [0u32; W * W];
        blit(Shape::Move, &mut buf, W, W, 2, 2);
        for y in 0..W {
            assert_eq!(buf[y * W + (W - 1)], 0, "row {} wrapped to the right edge", y);
        }
        assert!(buf.iter().any(|&p| p != 0), "the visible corner of the shape should still draw");
    }

    #[test]
    fn full_alpha_reproduces_the_source_colour_exactly() {
        // `>> 8` instead of `/ 255` is the classic version of this, and it turns the white body into
        // 254/254/254 — invisible in isolation, and a visible grey cast next to a real white.
        assert_eq!(blend(0x0000_0000, 0x00FF_FFFF, 255), 0xFFFF_FFFF);
        assert_eq!(blend(0xFFFF_FFFF, 0x0000_0000, 0), 0xFFFF_FFFF);
    }

    #[test]
    fn the_edge_mask_maps_to_the_right_diagonal() {
        assert_eq!(Shape::for_edges(0), None);
        assert_eq!(Shape::for_edges(1), Some(Shape::ResizeH));
        assert_eq!(Shape::for_edges(8), Some(Shape::ResizeV));
        // top-left (4|1) and bottom-right (8|2) share one diagonal…
        assert_eq!(Shape::for_edges(5), Some(Shape::ResizeNwse));
        assert_eq!(Shape::for_edges(10), Some(Shape::ResizeNwse));
        // …top-right (4|2) and bottom-left (8|1) the other.
        assert_eq!(Shape::for_edges(6), Some(Shape::ResizeNesw));
        assert_eq!(Shape::for_edges(9), Some(Shape::ResizeNesw));
        // Opposite edges at once is not a corner; it happens on a window narrower than two bands.
        assert_eq!(Shape::for_edges(3), None);
        assert_eq!(Shape::for_edges(12), None);
    }
}
