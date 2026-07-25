//! SMBIOS / DMI — asking the machine who it is.
//!
//! Every laptop-specific thing worth doing is keyed on the manufacturer. Fan tachometers, EC
//! register maps and SMI protocols are all vendor-private and mutually incompatible: EC register
//! 0x55 is a fan target on one vendor and a battery charge threshold on another. Linux handles this
//! with a driver per vendor (`dell-smm-hwmon`, `thinkpad_acpi`, `asus-wmi`), each of which refuses to
//! load unless DMI says it is on the right machine.
//!
//! We had no way to ask, and the cost of that showed up twice:
//!   * `laptop_fans` "solved" it by firing ASUS *and* HP *and* Acer *and* MSI *and* Lenovo *and* Dell
//!     register writes at the real ACPI embedded controller and hoping one landed. One of them
//!     switched the firmware's own fan governor off and put nothing in its place.
//!   * With those writes disabled, the fan readout became a hardcoded "unknown" — correct, but
//!     permanently so, because nothing could ever establish which vendor path was the right one.
//!
//! This module is the missing input. It is strictly read-only: it locates the SMBIOS entry point,
//! walks the structure table and pulls out the system manufacturer and product name. Nothing here
//! touches hardware.
//!
//! Limitation worth knowing: we scan the legacy 0xF0000–0xFFFFF window for the anchor. Most UEFI
//! firmware still shadows the entry point there for compatibility, but it is not guaranteed — the
//! architecturally correct source on a UEFI boot is the EFI configuration table, which
//! `bootloader_api` 0.11 does not hand us (it forwards only the RSDP). If the scan comes up empty we
//! say so and stay at `Vendor::Unknown` rather than guessing.

use crate::serial_println;

const STR_MAX: usize = 64;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vendor {
    Unknown,
    Dell,
    Hp,
    Lenovo,
    Asus,
    Acer,
    Msi,
    Samsung,
    Toshiba,
    Apple,
    Framework,
}

/// A fixed-size string field, so the whole identity is `Copy` and needs no allocator. SMBIOS strings
/// are short by convention; anything longer is truncated rather than dropped.
#[derive(Clone, Copy)]
pub struct DmiStr {
    buf: [u8; STR_MAX],
    len: u8,
}

impl DmiStr {
    const EMPTY: DmiStr = DmiStr { buf: [0; STR_MAX], len: 0 };

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    fn set(&mut self, s: &[u8]) {
        let n = core::cmp::min(s.len(), STR_MAX);
        self.buf[..n].copy_from_slice(&s[..n]);
        self.len = n as u8;
    }
}

#[derive(Clone, Copy)]
pub struct SystemIdent {
    pub manufacturer: DmiStr,
    pub product: DmiStr,
    pub bios_vendor: DmiStr,
    /// ACPI OEM ID from the RSDP — the fallback identity when there is no reachable SMBIOS. Six
    /// characters, e.g. "DELL", "LENOVO", "ACRSYS". See `note_acpi_oem`.
    pub acpi_oem: DmiStr,
    /// ACPI OEM Table ID from the XSDT/RSDT, eight characters. Usually a board or platform codename,
    /// which sometimes names the vendor when the OEM ID is a generic BIOS-writer string.
    pub acpi_oem_table: DmiStr,
    pub vendor: Vendor,
    /// True once SMBIOS specifically was found and parsed. Distinct from `vendor != Unknown`, which
    /// the ACPI fallback can also satisfy.
    pub found: bool,
}

impl SystemIdent {
    const EMPTY: SystemIdent = SystemIdent {
        manufacturer: DmiStr::EMPTY,
        product: DmiStr::EMPTY,
        bios_vendor: DmiStr::EMPTY,
        acpi_oem: DmiStr::EMPTY,
        acpi_oem_table: DmiStr::EMPTY,
        vendor: Vendor::Unknown,
        found: false,
    };
}

/// Written exactly once, from `init()`, during single-threaded early boot — before SMP, before any
/// task exists. Read-only thereafter.
///
/// Deliberately NOT behind a `Mutex`: `fan_rpm()` reads the vendor from the System Monitor syscall
/// arm, which runs with interrupts off, while boot-time printing reads it with interrupts on. A lock
/// reachable from both sides of the preemption boundary is exactly the shape that has deadlocked
/// this kernel before, and there is nothing to protect here — the value never changes again.
static mut IDENT: SystemIdent = SystemIdent::EMPTY;

pub fn ident() -> &'static SystemIdent {
    unsafe { &*core::ptr::addr_of!(IDENT) }
}

pub fn vendor() -> Vendor {
    ident().vendor
}

/// "Dell Inc. Latitude 5410", or "unknown machine" when SMBIOS gave us nothing.
pub fn describe() -> &'static str {
    let id = ident();
    if id.manufacturer.is_empty() { "unknown machine" } else { id.manufacturer.as_str() }
}

// ─────────────────────────────────────────────────────────────────────────────
// Physical memory access
// ─────────────────────────────────────────────────────────────────────────────

/// Borrow `len` bytes of physical memory through the bootloader's complete physical mapping, but
/// only after confirming every page of it is actually mapped. The SMBIOS structure table lives
/// wherever the firmware put it (usually an EFI-reserved region well above 1 MB), so this is the one
/// place we follow a firmware-supplied pointer into memory we did not choose — an unmapped read here
/// would be a page fault during boot, before there is any handler worth having.
unsafe fn phys_slice(pa: u64, len: usize) -> Option<&'static [u8]> {
    if len == 0 { return None; }
    let base = crate::memory::phys_to_virt(pa)?;
    let end = base.checked_add(len as u64)?;
    let mut page = base & !0xFFF;
    while page < end {
        crate::memory::virt_to_phys(page)?;
        page += 4096;
    }
    Some(core::slice::from_raw_parts(base as *const u8, len))
}

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

/// An anchor candidate at `b`, fully resolved: validated, followed, and parsed.
///
/// Validation is deliberately all-or-nothing. The wider scan below walks ordinary RAM, where a
/// checksum-valid *copy* of a dead entry point can survive, so "the signature matched" is nowhere
/// near enough — we only accept a candidate whose structure table actually parses into a Type 1 with
/// a non-empty manufacturer. That makes a false positive require a coincidence in three independent
/// places, and means the scan can safely keep looking after a near-miss.
unsafe fn try_anchor(b: &[u8]) -> Option<SystemIdent> {
    let (table_pa, table_len) = if b.starts_with(b"_SM3_") {
        // SMBIOS 3.x (64-bit). Checked first: a machine with both must be read through this one,
        // since the 32-bit entry point cannot describe a table above 4 GB.
        let eps_len = *b.get(6)? as usize;
        if eps_len < 24 || eps_len > b.len() || !checksum_ok(&b[..eps_len]) {
            return None;
        }
        (
            u64::from_le_bytes([b[16], b[17], b[18], b[19], b[20], b[21], b[22], b[23]]),
            u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize,
        )
    } else if b.starts_with(b"_SM_") {
        let eps_len = *b.get(5)? as usize;
        if eps_len < 31 || eps_len > b.len() || !checksum_ok(&b[..eps_len]) {
            return None;
        }
        (
            u32::from_le_bytes([b[0x18], b[0x19], b[0x1A], b[0x1B]]) as u64,
            u16::from_le_bytes([b[0x16], b[0x17]]) as usize,
        )
    } else {
        return None;
    };

    if table_pa == 0 || table_len == 0 || table_len > 1 << 20 {
        return None;
    }

    let mut id = SystemIdent::EMPTY;
    parse_table(table_pa, table_len, &mut id);
    if id.manufacturer.is_empty() {
        return None;
    }
    id.found = true;
    Some(id)
}

/// Scan one physical range for an anchor. `pa_end` is exclusive.
///
/// No per-page mapping check here, unlike `phys_slice`: every range passed in comes from the
/// bootloader's own memory map, and the complete physical mapping covers all of it by construction.
/// Checking each page would mean a locked four-level page walk per 4 KB — on a multi-gigabyte pass
/// that is the difference between a fraction of a second and minutes.
unsafe fn scan_region(pa_start: u64, pa_end: u64) -> Option<SystemIdent> {
    // Entry points live below 4 GB in practice (the 32-bit anchor cannot express anything else, and
    // firmware places the 3.x one alongside it). Capping bounds the worst-case scan.
    const LIMIT: u64 = 4 << 30;
    let start = pa_start.max(0x1000); // page 0 is the IVT/BDA, never a table
    let end = pa_end.min(LIMIT);
    if start + 32 >= end {
        return None;
    }

    let base = crate::memory::phys_to_virt(start)?;
    let win = core::slice::from_raw_parts(base as *const u8, (end - start) as usize);

    // Anchors are 16-byte aligned in PHYSICAL address space, so start from the first aligned
    // physical address at or after `start` rather than from offset 0.
    let mut off = ((16 - (start % 16)) % 16) as usize;
    while off + 32 <= win.len() {
        // Cheap reject first — this loop runs a few hundred million times on the last-resort pass.
        if win[off] == b'_' {
            if let Some(id) = try_anchor(&win[off..]) {
                crate::serial_println!("[SMBIOS] entry point at {:#x}", start + off as u64);
                return Some(id);
            }
        }
        off += 16;
    }
    None
}

/// Find and parse the SMBIOS tables, wherever this firmware happened to put them.
///
/// Three passes, cheapest and most likely first:
///   1. the legacy 0xF0000 window — where a BIOS, or a UEFI that still shadows for compatibility,
///      puts it. **This machine has neither**, which is what sent us looking further.
///   2. firmware-owned regions (EfiRuntimeServicesData, ACPI reclaim/NVS, reserved). This is where a
///      UEFI machine's tables actually live, and it is a small total area.
///   3. ordinary RAM, as a last resort. Slower and it can only ever find a stale copy of something —
///      hence the strict validation in `try_anchor`.
///
/// The architecturally correct answer is none of these: on UEFI the entry point is published through
/// the **EFI configuration table** (SMBIOS3 GUID f2fd1544-…), and scanning is what Linux does only on
/// legacy BIOS. We scan because `bootloader_api` 0.11 forwards the RSDP and nothing else — there is
/// no way to reach the EFI system table from here. If that ever changes, delete all of this.
unsafe fn find_ident(regions: &[bootloader_api::info::MemoryRegion]) -> Option<SystemIdent> {
    use bootloader_api::info::MemoryRegionKind;

    if let Some(id) = scan_region(0xF_0000, 0x10_0000) {
        return Some(id);
    }

    for reg in regions.iter().filter(|r| r.kind != MemoryRegionKind::Usable) {
        if let Some(id) = scan_region(reg.start, reg.end) {
            return Some(id);
        }
    }

    crate::serial_println!("[SMBIOS] not in firmware-reserved memory — sweeping RAM...");
    for reg in regions.iter().filter(|r| r.kind == MemoryRegionKind::Usable) {
        if let Some(id) = scan_region(reg.start, reg.end) {
            return Some(id);
        }
    }
    None
}

/// SMBIOS entry points are checksummed so that a stray "_SM_" in some option ROM's copyright banner
/// can't be mistaken for one. Bytes must sum to zero mod 256.
fn checksum_ok(bytes: &[u8]) -> bool {
    bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b)) == 0
}

// ─────────────────────────────────────────────────────────────────────────────
// Structure table
// ─────────────────────────────────────────────────────────────────────────────

/// Fetch string number `idx` (1-based; 0 means "not set") from the string set that follows a
/// structure's formatted area. `strings` starts at the first string byte.
fn dmi_string(strings: &[u8], idx: u8) -> Option<&[u8]> {
    if idx == 0 { return None; }
    let mut remaining = idx;
    let mut start = 0usize;
    for i in 0..strings.len() {
        if strings[i] != 0 { continue; }
        if i == start { return None; } // empty string = end of the set, index ran off the end
        remaining -= 1;
        if remaining == 0 { return Some(&strings[start..i]); }
        start = i + 1;
    }
    None
}

unsafe fn parse_table(table_pa: u64, table_len: usize, out: &mut SystemIdent) {
    let table = match phys_slice(table_pa, table_len) {
        Some(t) => t,
        None => {
            serial_println!(
                "[SMBIOS] structure table at {:#x} is not mapped — skipping.", table_pa);
            return;
        }
    };

    let mut off = 0usize;
    while off + 4 <= table.len() {
        let ty = table[off];
        let hdr_len = table[off + 1] as usize;
        if hdr_len < 4 || off + hdr_len > table.len() { break; }
        if ty == 127 { break; } // end-of-table

        let formatted = &table[off..off + hdr_len];
        let strings_at = off + hdr_len;

        // The string set is terminated by a double NUL; that pair also ends the structure.
        let mut p = strings_at;
        let mut end = table.len();
        while p + 1 < table.len() {
            if table[p] == 0 && table[p + 1] == 0 {
                end = p + 2;
                break;
            }
            p += 1;
        }
        let strings = &table[strings_at..core::cmp::min(end, table.len())];

        match ty {
            // Type 0 — BIOS Information. Useful mostly as a sanity check that we parsed anything.
            0 if hdr_len > 0x04 => {
                if let Some(s) = dmi_string(strings, formatted[0x04]) {
                    out.bios_vendor.set(s);
                }
            }
            // Type 1 — System Information. This is the one that matters.
            1 if hdr_len > 0x05 => {
                if let Some(s) = dmi_string(strings, formatted[0x04]) {
                    out.manufacturer.set(s);
                }
                if let Some(s) = dmi_string(strings, formatted[0x05]) {
                    out.product.set(s);
                }
            }
            _ => {}
        }

        off = end;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Vendor classification
// ─────────────────────────────────────────────────────────────────────────────

fn contains_ci(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || n.len() > h.len() { return false; }
    for start in 0..=(h.len() - n.len()) {
        if (0..n.len()).all(|i| h[start + i].eq_ignore_ascii_case(&n[i])) {
            return true;
        }
    }
    false
}

/// Classify from the ACPI OEM ID / OEM Table ID.
///
/// Coarser than SMBIOS and only used when SMBIOS is unreachable, but it is real evidence: these
/// fields are baked into the firmware image by whoever built it. The catch is that plenty of vendors
/// ship their BIOS writer's generic string instead of their own — "ALASKA" is American Megatrends,
/// "PTLTD" is Phoenix, "INSYDE" is Insyde, "OEMID"/"INTEL" are placeholders. Those tell us who wrote
/// the firmware, not who built the laptop, so they must stay `Unknown` rather than be guessed at.
fn classify_acpi(oem: &str, oem_table: &str) -> Vendor {
    for s in [oem, oem_table] {
        if contains_ci(s, "DELL") { return Vendor::Dell; }
        if contains_ci(s, "LENOVO") || contains_ci(s, "TP-") { return Vendor::Lenovo; }
        if contains_ci(s, "HPQOEM") || contains_ci(s, "HPQ") { return Vendor::Hp; }
        if contains_ci(s, "ACRSYS") || contains_ci(s, "ACRPRD") { return Vendor::Acer; }
        if contains_ci(s, "ASUS") { return Vendor::Asus; }
        if contains_ci(s, "MSI_NB") || contains_ci(s, "MICSTR") { return Vendor::Msi; }
        if contains_ci(s, "SECCSD") || contains_ci(s, "SECGSD") { return Vendor::Samsung; }
        if contains_ci(s, "TOSQCI") || contains_ci(s, "TOSCPL") || contains_ci(s, "TOSINV") {
            return Vendor::Toshiba;
        }
        if contains_ci(s, "APPLE") { return Vendor::Apple; }
    }
    Vendor::Unknown
}

/// Read the ACPI OEM ID from the RSDP (and the OEM Table ID from the XSDT/RSDT behind it).
///
/// This is the consolation prize for a machine whose SMBIOS we cannot reach: `bootloader_api` hands
/// us the RSDP and nothing else, and the RSDP happens to carry a six-character OEM ID at offset 9 —
/// the one piece of vendor identity we are *guaranteed* to be able to read. Call after `init`; it
/// only fills in a vendor if SMBIOS left it Unknown.
pub fn note_acpi_oem(rsdp_pa: u64) {
    let mut id = *ident();

    unsafe {
        let rsdp = match phys_slice(rsdp_pa, 36) {
            Some(r) => r,
            None => return,
        };
        if !rsdp.starts_with(b"RSD PTR ") {
            crate::serial_println!("[SMBIOS] RSDP at {:#x} has no signature.", rsdp_pa);
            return;
        }
        id.acpi_oem.set(trim_fixed(&rsdp[9..15]));

        // Revision >= 2 means the XSDT (64-bit pointers) is the one to use; revision 0 is ACPI 1.0
        // and only has the 32-bit RSDT.
        let sdt_pa = if rsdp[15] >= 2 {
            u64::from_le_bytes([
                rsdp[24], rsdp[25], rsdp[26], rsdp[27], rsdp[28], rsdp[29], rsdp[30], rsdp[31],
            ])
        } else {
            u32::from_le_bytes([rsdp[16], rsdp[17], rsdp[18], rsdp[19]]) as u64
        };
        if sdt_pa != 0 {
            if let Some(sdt) = phys_slice(sdt_pa, 36) {
                id.acpi_oem_table.set(trim_fixed(&sdt[16..24]));
            }
        }

        if id.vendor == Vendor::Unknown {
            id.vendor = classify_acpi(id.acpi_oem.as_str(), id.acpi_oem_table.as_str());
        }
        IDENT = id;
    }

    crate::serial_println!(
        "[SMBIOS] ACPI OEM ID \"{}\", OEM Table ID \"{}\" -> vendor {:?}",
        id.acpi_oem.as_str(), id.acpi_oem_table.as_str(), id.vendor);
    if id.vendor == Vendor::Unknown {
        crate::serial_println!(
            "[SMBIOS] That is a BIOS-writer string, not a machine vendor — still unidentified.");
    }
}

/// ACPI's fixed-width character fields are space-padded and are not NUL-terminated. Trailing NULs
/// show up too on firmware that zero-fills instead.
fn trim_fixed(field: &[u8]) -> &[u8] {
    let mut end = field.len();
    while end > 0 && (field[end - 1] == b' ' || field[end - 1] == 0) {
        end -= 1;
    }
    &field[..end]
}

/// Match on the SMBIOS system manufacturer. These are the strings real firmware reports — "LENOVO"
/// is upper-case, HP ships both "HP" and the older "Hewlett-Packard", and MSI identifies itself only
/// by its legal name, which contains neither "M" nor "S" nor "I" in that order.
fn classify(manufacturer: &str) -> Vendor {
    if contains_ci(manufacturer, "Dell") { return Vendor::Dell; }
    if contains_ci(manufacturer, "Lenovo") { return Vendor::Lenovo; }
    if contains_ci(manufacturer, "ASUS") { return Vendor::Asus; }
    if contains_ci(manufacturer, "Acer") { return Vendor::Acer; }
    if contains_ci(manufacturer, "Micro-Star") || contains_ci(manufacturer, "MSI") {
        return Vendor::Msi;
    }
    if contains_ci(manufacturer, "Samsung") { return Vendor::Samsung; }
    if contains_ci(manufacturer, "Toshiba") { return Vendor::Toshiba; }
    if contains_ci(manufacturer, "Apple") { return Vendor::Apple; }
    if contains_ci(manufacturer, "Framework") { return Vendor::Framework; }
    // Checked last: "HP" is two letters and would match inside other manufacturers' strings.
    if contains_ci(manufacturer, "Hewlett") || manufacturer.trim().eq_ignore_ascii_case("HP") {
        return Vendor::Hp;
    }
    Vendor::Unknown
}

// ─────────────────────────────────────────────────────────────────────────────

/// Call once, early, after the physical memory mapping is live and before anything asks who we are.
pub fn init(regions: &[bootloader_api::info::MemoryRegion]) {
    let mut id = unsafe { find_ident(regions) }.unwrap_or(SystemIdent::EMPTY);
    id.vendor = classify(id.manufacturer.as_str());
    unsafe { IDENT = id; }

    if id.found {
        serial_println!(
            "[SMBIOS] System: {} {} (BIOS by {})",
            id.manufacturer.as_str(),
            id.product.as_str(),
            if id.bios_vendor.is_empty() { "<unknown>" } else { id.bios_vendor.as_str() });
        serial_println!("[SMBIOS] Vendor class: {:?}", id.vendor);
        if id.vendor == Vendor::Unknown {
            serial_println!(
                "[SMBIOS] No vendor driver for this machine — fan telemetry will read as unknown.");
        }
    } else {
        serial_println!("[SMBIOS] No entry point anywhere in physical memory.");
        serial_println!("[SMBIOS] Falling back to the ACPI OEM ID for machine identity.");
    }
}

/// The best machine name we have, written into `out` (NUL-padded), returning its length.
///
/// One place decides what "who is this machine" renders as, so the System Monitor and the boot log
/// can't tell different stories. Falls back through SMBIOS -> ACPI OEM -> nothing, and says which
/// one it used, because "Dell Inc. Latitude 5410" and "ACPI OEM DELL" are very different degrees of
/// confidence and a UI that blurred them would be lying by omission.
pub fn machine_string(out: &mut [u8]) -> usize {
    let id = ident();
    let mut n = 0usize;

    let mut push = |s: &str, n: &mut usize| {
        let bytes = s.as_bytes();
        let take = core::cmp::min(bytes.len(), out.len().saturating_sub(*n));
        out[*n..*n + take].copy_from_slice(&bytes[..take]);
        *n += take;
    };

    if !id.manufacturer.is_empty() {
        push(id.manufacturer.as_str(), &mut n);
        if !id.product.is_empty() {
            push(" ", &mut n);
            push(id.product.as_str(), &mut n);
        }
    } else if !id.acpi_oem.is_empty() {
        push("ACPI OEM \"", &mut n);
        push(id.acpi_oem.as_str(), &mut n);
        push("\" (no SMBIOS)", &mut n);
    }
    n
}
