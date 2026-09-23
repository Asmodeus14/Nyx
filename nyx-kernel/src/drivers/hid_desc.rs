//! HID report descriptor parser — just enough to find a pointer in it.
//!
//! A report descriptor is a byte-coded program describing the reports a device sends: which report
//! ID, which bits, what each bit range means (usage page + usage), and whether it is relative or
//! absolute. This walks it and flattens every INPUT field into a [`Field`] with its absolute bit
//! position inside its report, plus the application collection it belongs to.
//!
//! ★ Deliberately not a general HID stack. Its one job for now is to locate, in a touchpad's
//! descriptor, the mouse collection (buttons + relative X/Y) and the touchpad collection (contacts
//! with absolute X/Y), so the transport can decode reports without hardcoding one vendor's layout.
//!
//! Pure: no hardware, no kernel state — so it is tested on the host.

use alloc::vec::Vec;

/// Usage pages and usages this driver cares about.
pub mod usage {
    pub const PAGE_GENERIC_DESKTOP: u16 = 0x01;
    pub const PAGE_BUTTON: u16 = 0x09;
    pub const PAGE_DIGITIZER: u16 = 0x0D;

    pub const GD_POINTER: u16 = 0x01;
    pub const GD_MOUSE: u16 = 0x02;
    pub const GD_X: u16 = 0x30;
    pub const GD_Y: u16 = 0x31;
    pub const GD_WHEEL: u16 = 0x38;

    pub const DIG_TOUCH_PAD: u16 = 0x05;
    pub const DIG_FINGER: u16 = 0x22;
    pub const DIG_TIP_SWITCH: u16 = 0x42;
    pub const DIG_CONTACT_ID: u16 = 0x51;
    pub const DIG_CONTACT_COUNT: u16 = 0x54;
}

/// One input field: `size` bits at `bit_off` within the report (bit 0 = first bit AFTER the report
/// ID byte, when the device uses report IDs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub report_id: u8,
    pub bit_off: u32,
    pub size: u32,
    pub page: u16,
    pub usage: u16,
    /// Main-item flags. Bit 0 = constant (padding), bit 1 = variable, bit 2 = relative.
    pub flags: u32,
    pub logical_min: i32,
    pub logical_max: i32,
    /// The application collection this field is inside: (page, usage). (0, 0) if none.
    pub app: (u16, u16),
}

impl Field {
    pub fn is_constant(&self) -> bool { self.flags & 1 != 0 }
    pub fn is_relative(&self) -> bool { self.flags & 4 != 0 }

    /// Extract this field from a report body (the bytes AFTER the report ID). Signed when the
    /// logical minimum is negative, as HID specifies.
    pub fn extract(&self, body: &[u8]) -> Option<i32> {
        if self.size == 0 || self.size > 32 {
            return None;
        }
        let end = self.bit_off + self.size;
        if (end as usize + 7) / 8 > body.len() {
            return None;
        }
        let mut v: u64 = 0;
        for i in 0..self.size {
            let bit = self.bit_off + i;
            if body[(bit / 8) as usize] >> (bit % 8) & 1 != 0 {
                v |= 1 << i;
            }
        }
        if self.logical_min < 0 && self.size < 32 && v & (1 << (self.size - 1)) != 0 {
            v |= !0u64 << self.size;
        }
        Some(v as i64 as i32)
    }
}

/// Everything parsed out of one descriptor.
#[derive(Clone, Debug, Default)]
pub struct Descriptor {
    pub fields: Vec<Field>,
    /// True if any Report ID item appeared — then every report starts with its ID byte.
    pub uses_report_ids: bool,
    /// Application collections seen, in order: (page, usage, report ID of their first input).
    pub apps: Vec<(u16, u16, u8)>,
    /// Bytes that could not be parsed (a truncated item at the end). 0 on a clean parse.
    pub trailing: usize,
}

#[derive(Clone, Copy, Default)]
struct Globals {
    page: u16,
    logical_min: i32,
    logical_max: i32,
    report_size: u32,
    report_count: u32,
    report_id: u8,
}

fn sign_extend(v: u32, bytes: usize) -> i32 {
    match bytes {
        1 => v as u8 as i8 as i32,
        2 => v as u16 as i16 as i32,
        _ => v as i32,
    }
}

/// Parse a report descriptor.
pub fn parse(d: &[u8]) -> Descriptor {
    let mut out = Descriptor::default();
    let mut g = Globals::default();
    let mut stack: Vec<Globals> = Vec::new();
    // Local state, cleared after every main item.
    let mut usages: Vec<(u16, u16)> = Vec::new();
    let mut usage_min: Option<(u16, u16)> = None;
    let mut usage_max: Option<u16> = None;
    // Collection nesting: each entry is Some((page, usage)) for an application collection.
    let mut coll: Vec<Option<(u16, u16)>> = Vec::new();
    let mut app: (u16, u16) = (0, 0);
    let mut app_needs_id = false;
    // Running input bit offset per report ID.
    let mut offs: Vec<(u8, u32)> = Vec::new();

    let mut i = 0usize;
    while i < d.len() {
        let prefix = d[i];
        if prefix == 0xFE {
            // Long item: [0xFE, size, tag, data...]. None are defined; skip.
            if i + 2 >= d.len() {
                out.trailing = d.len() - i;
                break;
            }
            i += 3 + d[i + 1] as usize;
            continue;
        }
        let size = match prefix & 3 { 3 => 4, n => n as usize };
        if i + 1 + size > d.len() {
            out.trailing = d.len() - i;
            break;
        }
        let mut data: u32 = 0;
        for k in 0..size {
            data |= (d[i + 1 + k] as u32) << (8 * k);
        }
        let typ = (prefix >> 2) & 3;
        let tag = prefix >> 4;
        i += 1 + size;

        match typ {
            // Main.
            0 => {
                match tag {
                    0x8 => {
                        // Input.
                        let id = g.report_id;
                        let off = match offs.iter_mut().find(|(r, _)| *r == id) {
                            Some((_, o)) => o,
                            None => {
                                offs.push((id, 0));
                                &mut offs.last_mut().unwrap().1
                            }
                        };
                        for n in 0..g.report_count {
                            let (page, u) = if let Some((p, lo)) = usage_min {
                                let hi = usage_max.unwrap_or(lo);
                                (p, (lo as u32 + n).min(hi as u32) as u16)
                            } else if let Some(&last) = usages.last() {
                                *usages.get(n as usize).unwrap_or(&last)
                            } else {
                                (g.page, 0)
                            };
                            out.fields.push(Field {
                                report_id: id,
                                bit_off: *off + n * g.report_size,
                                size: g.report_size,
                                page,
                                usage: u,
                                flags: data,
                                logical_min: g.logical_min,
                                logical_max: g.logical_max,
                                app,
                            });
                        }
                        *off += g.report_count * g.report_size;
                        if app_needs_id {
                            if let Some(last) = out.apps.last_mut() {
                                last.2 = id;
                            }
                            app_needs_id = false;
                        }
                    }
                    0xA => {
                        // Collection. Type 1 = application.
                        let first = usages.first().copied();
                        if data & 0xFF == 1 {
                            let a = first.unwrap_or((g.page, 0));
                            coll.push(Some(a));
                            app = a;
                            out.apps.push((a.0, a.1, 0));
                            app_needs_id = true;
                        } else {
                            coll.push(None);
                        }
                    }
                    0xC => {
                        // End collection.
                        if let Some(Some(_)) = coll.pop() {
                            app = coll.iter().rev().find_map(|c| *c).unwrap_or((0, 0));
                        }
                    }
                    _ => {} // Output / Feature: not needed to read input.
                }
                usages.clear();
                usage_min = None;
                usage_max = None;
            }
            // Global.
            1 => match tag {
                0x0 => g.page = data as u16,
                0x1 => g.logical_min = sign_extend(data, size),
                0x2 => {
                    // Logical max is signed only if the minimum is negative; otherwise an 8-bit
                    // 0xFF is 255, not -1.
                    g.logical_max = if g.logical_min < 0 { sign_extend(data, size) } else { data as i32 };
                }
                0x7 => g.report_size = data,
                0x8 => {
                    g.report_id = data as u8;
                    out.uses_report_ids = true;
                }
                0x9 => g.report_count = data,
                0xA => stack.push(g),
                0xB => {
                    if let Some(s) = stack.pop() {
                        g = s;
                    }
                }
                _ => {}
            },
            // Local.
            2 => {
                // A 4-byte usage carries its own page in the high half.
                let (page, u) = if size == 4 { ((data >> 16) as u16, data as u16) } else { (g.page, data as u16) };
                match tag {
                    0x0 => usages.push((page, u)),
                    0x1 => usage_min = Some((page, u)),
                    0x2 => usage_max = Some(u),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

impl Descriptor {
    /// The first input field matching `page`/`usage` inside application `app`, optionally
    /// restricted to relative or absolute fields.
    pub fn find(&self, app: (u16, u16), page: u16, usage: u16, relative: Option<bool>) -> Option<Field> {
        self.fields.iter().copied().find(|f| {
            f.app == app
                && f.page == page
                && f.usage == usage
                && !f.is_constant()
                && relative.map_or(true, |r| f.is_relative() == r)
        })
    }
}

/// A relative-pointer (mouse) report layout, if the descriptor has one.
#[derive(Clone, Copy, Debug)]
pub struct MouseLayout {
    pub report_id: u8,
    pub buttons: [Option<Field>; 3],
    pub x: Field,
    pub y: Field,
}

pub fn mouse_layout(d: &Descriptor) -> Option<MouseLayout> {
    use usage::*;
    for &(page, u, _) in &d.apps {
        if page != PAGE_GENERIC_DESKTOP || (u != GD_MOUSE && u != GD_POINTER) {
            continue;
        }
        let app = (page, u);
        let x = d.find(app, PAGE_GENERIC_DESKTOP, GD_X, Some(true))?;
        let y = d.find(app, PAGE_GENERIC_DESKTOP, GD_Y, Some(true))?;
        let b = |n| d.find(app, PAGE_BUTTON, n, None);
        return Some(MouseLayout { report_id: x.report_id, buttons: [b(1), b(2), b(3)], x, y });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The standard 3-button boot mouse descriptor (HID 1.11, Appendix E.10), with a Report ID
    /// added, followed by a minimal precision-touchpad collection — the shape an I2C touchpad
    /// exposes in its default (mouse) mode.
    const MOUSE_THEN_PTP: &[u8] = &[
        0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, // Usage Page GD, Usage Mouse, Collection (App)
        0x85, 0x01, //   Report ID 1
        0x09, 0x01, 0xA1, 0x00, //   Usage Pointer, Collection (Physical)
        0x05, 0x09, 0x19, 0x01, 0x29, 0x03, //     Buttons 1..3
        0x15, 0x00, 0x25, 0x01, 0x95, 0x03, 0x75, 0x01, 0x81, 0x02, //  3 x 1 bit, Data Var Abs
        0x95, 0x01, 0x75, 0x05, 0x81, 0x03, //     5 bits padding (Const)
        0x05, 0x01, 0x09, 0x30, 0x09, 0x31, //     X, Y
        0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06, // 2 x 8 bit, Data Var Rel
        0xC0, 0xC0, //   End, End
        0x05, 0x0D, 0x09, 0x05, 0xA1, 0x01, // Usage Page Digitizer, Touch Pad, Collection (App)
        0x85, 0x04, //   Report ID 4
        0x09, 0x22, 0xA1, 0x02, //   Finger, Collection (Logical)
        0x09, 0x42, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x81, 0x02, // Tip switch
        0x09, 0x51, 0x25, 0x0F, 0x75, 0x07, 0x81, 0x02, // Contact ID, 7 bits
        0x05, 0x01, 0x09, 0x30, 0x26, 0xFF, 0x0F, 0x75, 0x10, 0x81, 0x02, // X abs 16 bit
        0x09, 0x31, 0x81, 0x02, //   Y abs 16 bit
        0xC0, //   End (Logical)
        0x05, 0x0D, 0x09, 0x54, 0x25, 0x05, 0x75, 0x08, 0x81, 0x02, // Contact count
        0xC0, // End (App)
    ];

    #[test]
    fn finds_both_applications_with_their_report_ids() {
        let d = parse(MOUSE_THEN_PTP);
        assert_eq!(d.trailing, 0);
        assert!(d.uses_report_ids);
        assert_eq!(d.apps, alloc::vec![(0x01, 0x02, 1), (0x0D, 0x05, 4)]);
    }

    #[test]
    fn mouse_layout_bits_are_where_the_boot_protocol_puts_them() {
        let d = parse(MOUSE_THEN_PTP);
        let m = mouse_layout(&d).expect("mouse collection");
        assert_eq!(m.report_id, 1);
        assert_eq!(m.buttons[0].unwrap().bit_off, 0);
        assert_eq!(m.buttons[2].unwrap().bit_off, 2);
        assert_eq!((m.x.bit_off, m.x.size), (8, 8));
        assert_eq!((m.y.bit_off, m.y.size), (16, 8));
        assert!(m.x.is_relative());
    }

    #[test]
    fn relative_axes_decode_signed() {
        let d = parse(MOUSE_THEN_PTP);
        let m = mouse_layout(&d).unwrap();
        // Left button down, dx = -3, dy = +5.
        let body = [0b0000_0001, 0xFD, 0x05];
        assert_eq!(m.buttons[0].unwrap().extract(&body), Some(1));
        assert_eq!(m.x.extract(&body), Some(-3));
        assert_eq!(m.y.extract(&body), Some(5));
    }

    #[test]
    fn touchpad_fields_are_offset_within_their_own_report() {
        let d = parse(MOUSE_THEN_PTP);
        let app = (0x0D, 0x05);
        let tip = d.find(app, 0x0D, 0x42, None).unwrap();
        let x = d.find(app, 0x01, 0x30, Some(false)).unwrap();
        let cc = d.find(app, 0x0D, 0x54, None).unwrap();
        assert_eq!(tip.report_id, 4);
        // Offsets restart at 0 for report 4 — they are NOT a continuation of report 1.
        assert_eq!(tip.bit_off, 0);
        assert_eq!((x.bit_off, x.size, x.logical_max), (8, 16, 0x0FFF));
        assert_eq!(cc.bit_off, 40);
    }

    #[test]
    fn logical_max_ff_is_255_when_min_is_non_negative() {
        let d = parse(&[0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x30, 0x15, 0x00, 0x25, 0xFF,
                        0x75, 0x08, 0x95, 0x01, 0x81, 0x02, 0xC0]);
        assert_eq!(d.fields[0].logical_max, 255);
    }

    #[test]
    fn truncated_descriptor_reports_trailing_bytes_instead_of_panicking() {
        let d = parse(&[0x05, 0x01, 0x26, 0xFF]);
        assert_eq!(d.trailing, 2);
    }
}
