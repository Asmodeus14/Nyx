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
    pub const PAGE_CONSUMER: u16 = 0x0C;
    pub const CONSUMER_AC_PAN: u16 = 0x238;

    pub const DIG_TOUCH_PAD: u16 = 0x05;
    pub const DIG_FINGER: u16 = 0x22;
    pub const DIG_TIP_SWITCH: u16 = 0x42;
    pub const DIG_CONTACT_ID: u16 = 0x51;
    pub const DIG_CONTACT_COUNT: u16 = 0x54;
    pub const DIG_CONFIDENCE: u16 = 0x47;
    pub const DIG_DEVICE_CONFIG: u16 = 0x0E;
    /// Device Mode / Input Mode, a Feature in the Device Configuration collection. 0 = mouse,
    /// 3 = precision touchpad.
    pub const DIG_INPUT_MODE: u16 = 0x52;
    pub const INPUT_MODE_MOUSE: i32 = 0;
    pub const INPUT_MODE_TOUCHPAD: i32 = 3;
    /// Selective Reporting: whether the device reports surface contacts, and clickpad buttons.
    /// Linux's hid-multitouch sets both to 1 alongside Input Mode.
    pub const DIG_SURFACE_SWITCH: u16 = 0x57;
    pub const DIG_BUTTON_SWITCH: u16 = 0x58;
    /// The Windows 8 certification blob, a 256-byte vendor Feature. Linux reads it once at probe
    /// "to enable some devices" (hid-multitouch.c).
    pub const PAGE_MS_VENDOR: u16 = 0xFF00;
    pub const MS_WIN8_BLOB: u16 = 0xC5;
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
    /// [`KIND_INPUT`] or [`KIND_FEATURE`]. Offsets are counted separately per kind and report ID:
    /// a Feature report is a different report from the Input report that shares its ID.
    pub kind: u8,
}

/// An Input main item: data the device sends in its input reports.
pub const KIND_INPUT: u8 = 0;
/// A Feature main item: configuration the host reads or writes with GET/SET_REPORT — where a
/// precision touchpad's Input Mode lives.
pub const KIND_FEATURE: u8 = 2;

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
    // Running bit offset per (kind, report ID) — Input and Feature reports are laid out separately.
    let mut offs: Vec<(u8, u8, u32)> = Vec::new();

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
                    0x8 | 0xB => {
                        // Input, or Feature.
                        let kind = if tag == 0x8 { KIND_INPUT } else { KIND_FEATURE };
                        let id = g.report_id;
                        let off = match offs.iter_mut().find(|(k, r, _)| *k == kind && *r == id) {
                            Some((_, _, o)) => o,
                            None => {
                                offs.push((kind, id, 0));
                                &mut offs.last_mut().unwrap().2
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
                                kind,
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
                    _ => {} // Output: nothing here writes output reports.
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
            f.kind == KIND_INPUT
                && f.app == app
                && f.page == page
                && f.usage == usage
                && !f.is_constant()
                && relative.map_or(true, |r| f.is_relative() == r)
        })
    }

    /// The first Feature field with this page and usage, anywhere in the descriptor.
    pub fn find_feature(&self, page: u16, usage: u16) -> Option<Field> {
        self.fields
            .iter()
            .copied()
            .find(|f| f.kind == KIND_FEATURE && f.page == page && f.usage == usage)
    }

    /// Byte length of Feature report `id`, excluding the report ID byte itself.
    pub fn feature_report_len(&self, id: u8) -> usize {
        let bits = self
            .fields
            .iter()
            .filter(|f| f.kind == KIND_FEATURE && f.report_id == id)
            .map(|f| f.bit_off + f.size)
            .max()
            .unwrap_or(0);
        ((bits + 7) / 8) as usize
    }
}

/// Where a precision touchpad's Input Mode lives: a Feature field (Digitizer, usage 0x52) in the
/// Device Configuration collection. Writing 3 there switches the device from its default mouse
/// emulation to reporting every contact.
#[derive(Clone, Copy, Debug)]
pub struct InputMode {
    pub report_id: u8,
    pub field: Field,
    /// Byte length of the whole Feature report, excluding the report ID.
    pub report_len: usize,
}

pub fn input_mode(d: &Descriptor) -> Option<InputMode> {
    use usage::*;
    let field = d.fields.iter().copied().find(|f| {
        f.kind == KIND_FEATURE && f.page == PAGE_DIGITIZER && f.usage == DIG_INPUT_MODE
    })?;
    Some(InputMode { report_id: field.report_id, field, report_len: d.feature_report_len(field.report_id) })
}

/// One contact slot in a touchpad report. A report may carry several; in "hybrid" mode a frame of
/// N contacts arrives spread over several reports, each using the slots it needs.
#[derive(Clone, Copy, Debug)]
pub struct ContactSlot {
    pub tip: Field,
    pub id: Option<Field>,
    /// Palm rejection: 0 = the device thinks this is not a deliberate finger.
    pub confidence: Option<Field>,
    pub x: Field,
    pub y: Field,
}

/// Up to this many contact slots per report — Windows precision touchpads report at most 5.
pub const MAX_SLOTS: usize = 5;

/// A precision touchpad's input report layout.
#[derive(Clone, Copy, Debug)]
pub struct PtpLayout {
    pub report_id: u8,
    pub slots: [Option<ContactSlot>; MAX_SLOTS],
    pub n_slots: usize,
    /// Contacts in this FRAME. Non-zero only in the first report of a frame (hybrid mode).
    pub contact_count: Option<Field>,
    /// The clickpad's physical button.
    pub button: Option<Field>,
    pub x_max: i32,
    pub y_max: i32,
}

/// Group the touch pad collection's input fields into contact slots.
///
/// ★ Grouped by ORDER, not by nesting: a precision touchpad declares each finger as a logical
/// collection, and the flattened field list keeps their order. The rule is that a field the finger
/// being assembled ALREADY HAS starts the next finger. The first version started a finger at every
/// Tip Switch instead — which breaks on ELAN parts, whose fingers declare Confidence BEFORE Tip
/// Switch (as one two-bit field), handing each finger's confidence to the one before it.
pub fn ptp_layout(d: &Descriptor) -> Option<PtpLayout> {
    use usage::*;
    let app = (PAGE_DIGITIZER, DIG_TOUCH_PAD);
    let mut slots: [Option<ContactSlot>; MAX_SLOTS] = [None; MAX_SLOTS];
    let mut n = 0usize;
    // (tip, id, confidence, x, y) being assembled.
    type Partial = [Option<Field>; 5];
    let mut cur: Partial = [None; 5];
    let mut report_id = None;
    let flush = |cur: &mut Partial, slots: &mut [Option<ContactSlot>; MAX_SLOTS], n: &mut usize| {
        if let [Some(tip), id, conf, Some(x), Some(y)] = *cur {
            if *n < MAX_SLOTS {
                slots[*n] = Some(ContactSlot { tip, id, confidence: conf, x, y });
                *n += 1;
            }
        }
        *cur = [None; 5];
    };
    for f in d.fields.iter().copied() {
        if f.kind != KIND_INPUT || f.app != app || f.is_constant() {
            continue;
        }
        let which = match (f.page, f.usage) {
            (PAGE_DIGITIZER, DIG_TIP_SWITCH) => 0,
            (PAGE_DIGITIZER, DIG_CONTACT_ID) => 1,
            (PAGE_DIGITIZER, DIG_CONFIDENCE) => 2,
            (PAGE_GENERIC_DESKTOP, GD_X) if !f.is_relative() => 3,
            (PAGE_GENERIC_DESKTOP, GD_Y) if !f.is_relative() => 4,
            _ => continue,
        };
        if cur[which].is_some() {
            flush(&mut cur, &mut slots, &mut n);
        }
        if which == 0 {
            report_id.get_or_insert(f.report_id);
        }
        cur[which] = Some(f);
    }
    flush(&mut cur, &mut slots, &mut n);
    let first = slots[0]?;
    Some(PtpLayout {
        report_id: report_id?,
        slots,
        n_slots: n,
        contact_count: d.find(app, PAGE_DIGITIZER, DIG_CONTACT_COUNT, None),
        button: d.find(app, PAGE_BUTTON, 1, None),
        x_max: first.x.logical_max,
        y_max: first.y.logical_max,
    })
}

/// A relative-pointer (mouse) report layout, if the descriptor has one.
#[derive(Clone, Copy, Debug)]
pub struct MouseLayout {
    pub report_id: u8,
    pub buttons: [Option<Field>; 3],
    pub x: Field,
    pub y: Field,
    /// Vertical wheel (Generic Desktop 0x38). ★ A touchpad in mouse mode reports its OWN
    /// two-finger scroll here — the firmware does the gesture — so this is scrolling without
    /// precision mode.
    pub wheel: Option<Field>,
    /// Horizontal scroll (Consumer page, AC Pan 0x238).
    pub pan: Option<Field>,
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
        let wheel = d.find(app, PAGE_GENERIC_DESKTOP, GD_WHEEL, None);
        let pan = d.find(app, PAGE_CONSUMER, CONSUMER_AC_PAN, None);
        return Some(MouseLayout { report_id: x.report_id, buttons: [b(1), b(2), b(3)], x, y, wheel, pan });
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

    /// A Windows-style precision touchpad: two contact slots (tip, confidence, contact ID, X, Y),
    /// then contact count and the clickpad button; plus a Device Configuration collection whose
    /// Feature report 3 holds Input Mode then Device Identifier.
    const PTP_FULL: &[u8] = &[
        0x05, 0x0D, 0x09, 0x05, 0xA1, 0x01, // Digitizer / Touch Pad, Collection (App)
        0x85, 0x04, //   Report ID 4
        // Finger 1
        0x09, 0x22, 0xA1, 0x02,
        0x09, 0x42, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x81, 0x02, // tip, 1 bit
        0x09, 0x47, 0x81, 0x02, //                                               confidence, 1 bit
        0x09, 0x51, 0x25, 0x0F, 0x75, 0x06, 0x81, 0x02, //                       contact id, 6 bits
        0x05, 0x01, 0x09, 0x30, 0x26, 0x40, 0x0B, 0x75, 0x10, 0x81, 0x02, //     X abs, max 2880
        0x09, 0x31, 0x26, 0x40, 0x07, 0x81, 0x02, //                             Y abs, max 1856
        0xC0,
        // Finger 2 — identical shape
        0x05, 0x0D, 0x09, 0x22, 0xA1, 0x02,
        0x09, 0x42, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x01, 0x81, 0x02,
        0x09, 0x47, 0x81, 0x02,
        0x09, 0x51, 0x25, 0x0F, 0x75, 0x06, 0x81, 0x02,
        0x05, 0x01, 0x09, 0x30, 0x26, 0x40, 0x0B, 0x75, 0x10, 0x81, 0x02,
        0x09, 0x31, 0x26, 0x40, 0x07, 0x81, 0x02,
        0xC0,
        0x05, 0x0D, 0x09, 0x54, 0x25, 0x05, 0x75, 0x08, 0x81, 0x02, //  contact count, 8 bits
        0x05, 0x09, 0x09, 0x01, 0x25, 0x01, 0x75, 0x01, 0x81, 0x02, //  button 1, 1 bit
        0x75, 0x07, 0x81, 0x03, //                                       padding
        0xC0,
        0x05, 0x0D, 0x09, 0x0E, 0xA1, 0x01, // Digitizer / Device Configuration, Collection (App)
        0x85, 0x03, //   Report ID 3
        0x09, 0x22, 0xA1, 0x02,
        0x09, 0x52, 0x15, 0x00, 0x25, 0x0A, 0x75, 0x08, 0x95, 0x01, 0xB1, 0x02, // Input Mode (Feature)
        0x09, 0x53, 0xB1, 0x02, //                                                Device Identifier
        0xC0, 0xC0,
    ];

    #[test]
    fn ptp_layout_groups_each_finger_by_its_tip_switch() {
        let d = parse(PTP_FULL);
        assert_eq!(d.trailing, 0);
        let p = ptp_layout(&d).expect("touch pad collection");
        assert_eq!(p.report_id, 4);
        assert_eq!(p.n_slots, 2);
        let s0 = p.slots[0].unwrap();
        let s1 = p.slots[1].unwrap();
        // Slot 1: tip@0, confidence@1, id@2 (6 bits), X@8, Y@24 — 40 bits per finger.
        assert_eq!(s0.tip.bit_off, 0);
        assert_eq!(s0.confidence.unwrap().bit_off, 1);
        assert_eq!(s0.id.unwrap().bit_off, 2);
        assert_eq!((s0.x.bit_off, s0.y.bit_off), (8, 24));
        // Slot 2 starts where slot 1 ended.
        assert_eq!(s1.tip.bit_off, 40);
        assert_eq!((s1.x.bit_off, s1.y.bit_off), (48, 64));
        assert_eq!(p.contact_count.unwrap().bit_off, 80);
        assert_eq!(p.button.unwrap().bit_off, 88);
        assert_eq!((p.x_max, p.y_max), (2880, 1856));
    }

    #[test]
    fn ptp_contact_decodes_from_a_real_report_body() {
        let p = ptp_layout(&parse(PTP_FULL)).unwrap();
        let s0 = p.slots[0].unwrap();
        // Finger touching, confident, id 5, at (1000, 500); one contact; button up.
        let mut body = [0u8; 12];
        body[0] = 0b0001_0111; // tip=1, conf=1, id=5 (bits 2..7)
        body[1..3].copy_from_slice(&1000u16.to_le_bytes());
        body[3..5].copy_from_slice(&500u16.to_le_bytes());
        body[10] = 1; // contact count
        assert_eq!(s0.tip.extract(&body), Some(1));
        assert_eq!(s0.confidence.unwrap().extract(&body), Some(1));
        assert_eq!(s0.id.unwrap().extract(&body), Some(5));
        assert_eq!(s0.x.extract(&body), Some(1000));
        assert_eq!(s0.y.extract(&body), Some(500));
        assert_eq!(p.contact_count.unwrap().extract(&body), Some(1));
        assert_eq!(p.button.unwrap().extract(&body), Some(0));
    }

    /// The ELAN ordering: each finger declares Confidence and Tip Switch as ONE two-bit field, with
    /// Confidence first. Grouping "a new finger at every tip switch" hands finger 2's confidence to
    /// finger 1; grouping by "a field this finger already has starts the next one" gets it right.
    #[test]
    fn ptp_layout_handles_confidence_declared_before_tip() {
        let finger: &[u8] = &[
            0x05, 0x0D, 0x09, 0x22, 0xA1, 0x02,
            0x09, 0x47, 0x09, 0x42, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02,
            0x09, 0x51, 0x25, 0x0F, 0x75, 0x06, 0x95, 0x01, 0x81, 0x02,
            0x05, 0x01, 0x09, 0x30, 0x26, 0x40, 0x0B, 0x75, 0x10, 0x81, 0x02,
            0x09, 0x31, 0x26, 0x40, 0x07, 0x81, 0x02,
            0xC0,
        ];
        let mut d: alloc::vec::Vec<u8> = alloc::vec![0x05, 0x0D, 0x09, 0x05, 0xA1, 0x01, 0x85, 0x04];
        d.extend_from_slice(finger);
        d.extend_from_slice(finger);
        d.push(0xC0);
        let p = ptp_layout(&parse(&d)).expect("layout");
        assert_eq!(p.n_slots, 2);
        let (s0, s1) = (p.slots[0].unwrap(), p.slots[1].unwrap());
        assert_eq!(s0.confidence.unwrap().bit_off, 0);
        assert_eq!(s0.tip.bit_off, 1);
        assert_eq!(s1.confidence.unwrap().bit_off, 40, "finger 2's confidence belongs to finger 2");
        assert_eq!(s1.tip.bit_off, 41);
        assert_eq!((s1.x.bit_off, s1.y.bit_off), (48, 64));
    }

    #[test]
    fn input_mode_is_found_in_its_own_feature_report() {
        let d = parse(PTP_FULL);
        let m = input_mode(&d).expect("input mode feature");
        assert_eq!(m.report_id, 3);
        assert_eq!((m.field.bit_off, m.field.size), (0, 8));
        // Input Mode + Device Identifier: two bytes.
        assert_eq!(m.report_len, 2);
        // Feature fields must NOT shift the input layout of the same app, nor be found as inputs.
        assert!(d.find((0x0D, 0x0E), 0x0D, 0x52, None).is_none());
    }

    /// A vendor blob declared with a 4-byte usage (page in the high half), as Windows precision
    /// touchpads declare the Win8 certification blob, plus Selective Reporting switches.
    #[test]
    fn finds_the_win8_blob_and_the_selective_reporting_switches() {
        let d = parse(&[
            0x06, 0x00, 0xFF, 0x09, 0x01, 0xA1, 0x01, // Usage Page 0xFF00, Usage 1, Collection (App)
            0x85, 0x5C, //   Report ID 92
            0x09, 0xC5, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75, 0x08, 0x96, 0x00, 0x01, 0xB1, 0x02, // 256 B
            0xC0,
            0x05, 0x0D, 0x09, 0x0E, 0xA1, 0x01, 0x85, 0x07,
            0x09, 0x57, 0x09, 0x58, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0xB1, 0x02,
            0x75, 0x06, 0x95, 0x01, 0xB1, 0x03,
            0xC0,
        ]);
        let blob = d.find_feature(0xFF00, 0xC5).expect("blob");
        assert_eq!(blob.report_id, 92);
        assert_eq!(d.feature_report_len(92), 256);
        let s = d.find_feature(0x0D, 0x57).expect("surface switch");
        let b = d.find_feature(0x0D, 0x58).expect("button switch");
        assert_eq!((s.report_id, s.bit_off, b.bit_off), (7, 0, 1));
        assert_eq!(d.feature_report_len(7), 1);
    }

    /// A mouse with a wheel and AC Pan — the shape of a touchpad's mouse-mode collection, which
    /// carries the firmware's own two-finger scroll.
    #[test]
    fn mouse_layout_finds_wheel_and_pan() {
        let d = parse(&[
            0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, 0x01,
            0x05, 0x09, 0x19, 0x01, 0x29, 0x02, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x02, 0x81, 0x02,
            0x95, 0x06, 0x81, 0x03,
            0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06,
            0x05, 0x0C, 0x0A, 0x38, 0x02, 0x95, 0x01, 0x81, 0x06, // AC Pan, 8 bits relative
            0xC0,
        ]);
        let m = mouse_layout(&d).expect("mouse");
        assert_eq!(m.wheel.expect("wheel").bit_off, 24);
        assert_eq!(m.pan.expect("pan").bit_off, 32);
        // Wheel up one notch decodes as +1.
        assert_eq!(m.wheel.unwrap().extract(&[0, 0, 0, 1, 0]), Some(1));
        assert_eq!(m.wheel.unwrap().extract(&[0, 0, 0, 0xFF, 0]), Some(-1));
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
