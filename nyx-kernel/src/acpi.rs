#![allow(warnings)] // <--- THE FIX: Silence all warnings for this module and its included files!

// ==========================================
// 1. INJECT THE GENERATED C-TO-RUST BINDINGS
// ==========================================
include!(concat!(env!("OUT_DIR"), "/acpi_bindings.rs"));

// Legacy ACPI info to keep the PCI module compiling while we transition
pub struct AcpiInfo {
    pub rsdp_addr: Option<u64>,
    pub mcfg_addr: Option<u64>,
    pub madt_addr: Option<u64>, 
}

pub static mut ACPI_INFO: AcpiInfo = AcpiInfo { 
    rsdp_addr: None, 
    mcfg_addr: None, 
    madt_addr: None 
};

pub fn init(rsdp: u64) {
    unsafe { ACPI_INFO.rsdp_addr = Some(rsdp); }
}

/// The status of each step of ACPICA bring-up, kept so it can reach the screen.
///
/// `init_intel_acpica` reports every failure with `serial_println!`, and this laptop has **no serial
/// console** — that output is written and never read. So a namespace that failed to load looks
/// exactly like one that loaded fine and simply lacks the device you wanted. These are recorded at
/// boot and printed by the panel diagnostic. `0xFFFF_FFFF` means the step was never reached.
#[derive(Clone, Copy)]
pub struct AcpicaInit {
    pub subsystem: u32,
    pub tables: u32,
    pub load: u32,
    pub enable: u32,
    pub objects: u32,
}

pub static mut ACPICA_INIT: AcpicaInit = AcpicaInit {
    subsystem: 0xFFFF_FFFF,
    tables: 0xFFFF_FFFF,
    load: 0xFFFF_FFFF,
    enable: 0xFFFF_FFFF,
    objects: 0xFFFF_FFFF,
};

pub fn acpica_init_status() -> AcpicaInit {
    unsafe { ACPICA_INIT }
}

// ==========================================
// 2. THE NEW INTEL INIT SEQUENCE
// ==========================================
pub fn init_intel_acpica() {
    crate::serial_println!("[ACPI] Booting Intel ACPICA Engine...");
    
    unsafe {
        // 1. Boot the core engine
        let mut status = AcpiInitializeSubsystem();
        ACPICA_INIT.subsystem = status as u32;
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Subsystem Init Failed! Status: {}", status);
            return;
        }

        // 2. Copy the ACPI tables from the motherboard into our memory
        crate::serial_println!("[ACPI] Initializing ACPI Tables...");
        status = AcpiInitializeTables(core::ptr::null_mut(), 16, 0);
        ACPICA_INIT.tables = status as u32;
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Table Init Failed! Status: {}", status);
            return;
        }

        // --- PHASE 0: DYNAMIC MADT DETECTION ---
        crate::serial_println!("[ACPI] Extracting MADT for real SMP...");
        let mut madt_header: *mut core::ffi::c_void = core::ptr::null_mut();
        
        let get_status = AcpiGetTable(
            b"APIC\0".as_ptr() as *mut i8, 
            1, 
            &mut madt_header as *mut _ as *mut *mut _
        );
        
        if get_status == 0 && !madt_header.is_null() {
            let madt_virt = madt_header as u64;
            let madt_phys = crate::memory::virt_to_phys(madt_virt).unwrap_or(madt_virt);
            
            ACPI_INFO.madt_addr = Some(madt_phys);
            crate::serial_println!("[ACPI] MADT stored dynamically @ {:#x}", madt_phys);
        } else {
            crate::serial_println!("[ACPI] WARNING: MADT not found! Status: {}", get_status);
        }
        let mut mcfg_header: *mut core::ffi::c_void = core::ptr::null_mut();
        let mcfg_status = AcpiGetTable(
            b"MCFG\0".as_ptr() as *mut i8, 
            1, 
            &mut mcfg_header as *mut _ as *mut *mut _
        );

        if mcfg_status == 0 && !mcfg_header.is_null() {
            let mcfg_virt = mcfg_header as u64;
            let mcfg_phys = crate::memory::virt_to_phys(mcfg_virt).unwrap_or(mcfg_virt);
            
            ACPI_INFO.mcfg_addr = Some(mcfg_phys);
            crate::serial_println!("[ACPI] MCFG stored dynamically @ {:#x}", mcfg_phys);
        } else {
            crate::serial_println!("[ACPI] WARNING: MCFG not found! PCIe will fallback to Legacy Port I/O.");
        }
        // 3. Build the Hardware Namespace Tree
        crate::serial_println!("[ACPI] Loading Hardware Namespace...");
        status = AcpiLoadTables();
        ACPICA_INIT.load = status as u32;
        // ⚠️ AE_OK here does NOT mean the namespace loaded. `AcpiTbLoadNamespace` returns
        // AE_CTRL_TERMINATE when any table fails — the DSDT included — and `AcpiLoadTables`
        // rewrites that to AE_OK ("Don't let single failures abort the load"). The only honest
        // account is the table list, so dump it into the ACPICA log while it is still fresh.
        acpi_log_tables();
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Namespace Load Failed! Status: {}", status);
            return;
        }
        // ★★ Three counts, one per stage. See `acpi_log_root_count` for why this and not a name
        // lookup. The DSDT declares ~1468 root-scope names; if this says 1468 here and 637 after
        // objects-init, the parse is fine and something is UNLINKING nodes.
        acpi_log_root_count_tag(b"after-load\0".as_ptr());

        // 4. Transition the motherboard from Legacy mode to ACPI mode
        crate::serial_println!("[ACPI] Enabling ACPI Hardware Mode...");
        status = AcpiEnableSubsystem(0);
        ACPICA_INIT.enable = status as u32;
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Subsystem Enable Failed! Status: {}", status);
            return;
        }
        acpi_log_root_count_tag(b"after-enable\0".as_ptr());

        // 5. Execute the `_INI` methods to turn on the hidden hardware
        crate::serial_println!("[ACPI] Initializing Hardware Objects...");
        status = AcpiInitializeObjects(0);
        ACPICA_INIT.objects = status as u32;
        if status != 0 {
            crate::serial_println!("[ACPI] FATAL: Object Init Failed! Status: {}", status);
            return;
        }
        // ⚠️ This is the one that matters. `AcpiInitializeObjects` runs `_INI` methods, and AML
        // execution is the ONLY thing after load that writes a `Peer` — via `AcpiNsDeleteNode`,
        // which unlinks a temporary node by walking the parent's child list.
        acpi_log_root_count_tag(b"after-objects\0".as_ptr());
        crate::c_stubs::pool_report();

        crate::serial_println!("[ACPI] Intel ACPICA is FULLY ONLINE.");

        // Close the log now that bring-up is done. Everything diagnostic — the table list, the
        // DSDT/SSDT load results, any bring-up error — is already captured above. What comes after
        // is the AML interpreter's runtime warnings, which fire inside a syscall at IF=0 and are
        // what crashed `panel` and `battery`. See `AcpiOsVprintf` in custom_acpi.c.
        crate::c_stubs::close_acpi_log();
    }
}

// ==========================================
// 3. CUSTOM ACPI METHODS
// ==========================================
extern "C" {
    fn acpi_wake_cnvi_wifi() -> i32;
    fn acpi_find_i2c_hid() -> i32; 
    
    // --- NEW: THE ACPICA FAN CONTROLLER ---
    fn acpi_set_fan_state(turn_on: i32) -> i32;
    fn acpi_get_system_temp() -> i32;

    // --- Panel brightness, via \_SB.PCI0.GFX0.LCD ---
    fn acpi_panel_levels(out: *mut u8, max: i32) -> i32;
    fn acpi_panel_get() -> i32;
    fn acpi_panel_set(level: i32) -> i32;
    fn acpi_panel_diag(path_out: *mut u8, path_max: i32,
                       bcl_st: *mut u32, bqc_st: *mut u32) -> i32;
    fn acpi_path_status(path: *const u8) -> u32;
    fn acpi_count_nodes(depth: i32, nodes: *mut i32, devices: *mut i32, with_bcl: *mut i32);
    fn acpi_count_nodes_stepper(depth: i32, nodes: *mut i32);
    fn acpi_log_tables();
    fn acpi_log_root_count_tag(tag: *const u8);
    fn acpi_root_count(bad_out: *mut i32) -> i32;
    fn acpi_battery_read(out: *mut i32, max: i32) -> i32;
    fn acpi_ec_install(out_data: *mut u32, out_cmd: *mut u32) -> i32;
    fn acpi_ns_step(parent: *mut core::ffi::c_void, prev: *mut core::ffi::c_void,
                    next: *mut *mut core::ffi::c_void,
                    name_out: *mut u8, name_max: i32, type_out: *mut i32,
                    parent_out: *mut *mut core::ffi::c_void, name4_out: *mut u32) -> i32;
    fn acpi_ns_handle(path: *const u8) -> *mut core::ffi::c_void;
    fn acpi_ec_read_range(offset: i32, len: i32, out: *mut u8) -> i32;
    fn acpi_ec_last_status() -> i32;
    fn acpi_ec_battery(out: *mut i32, max: i32) -> i32;
}

/// Read raw EC space. **Governor context only** — EC transactions poll with timeouts.
///
/// Bypasses ACPI entirely. `AcpiInstallAddressSpaceHandler` walks internally and walks crash this
/// kernel, so AML can never reach the EC here; the ports can, and this uses them.
/// The last raw EC status byte. 0xFF = nothing decoding the port; 0x00 = idle but OBF never set.
pub fn ec_last_status() -> i32 { unsafe { acpi_ec_last_status() } }

/// Battery read straight from EC registers — no AML, no namespace walk.
///
/// The offsets come from this laptop's own DSDT (`ECG6` at dsdt.dsl:65152, `ECG9` at :65284), so
/// this is the same data by the same route the firmware uses, minus the AML interpreter. Requires
/// the bank select at EC register 0x03 first, which is why a raw dump showed nothing.
pub fn ec_battery() -> Battery {
    let mut v = [0i32; 11];
    let present = unsafe { acpi_ec_battery(v.as_mut_ptr(), 11) };
    Battery {
        present: present == 1,
        power_unit: v[1],
        design_cap: v[2],
        last_full_cap: v[3],
        design_voltage: v[4],
        state: v[5],
        present_rate: v[6],
        remaining_cap: v[7],
        voltage: v[8],
        have_bif: v[9] == 1,
        have_bst: v[10] == 1,
    }
}

pub fn ec_read_range(offset: i32, out: &mut [u8]) -> usize {
    let n = unsafe { acpi_ec_read_range(offset, out.len() as i32, out.as_mut_ptr()) };
    if n < 0 { 0 } else { n as usize }
}

/// One namespace node. See `ns_step`.
pub struct NsNode {
    pub handle: u64,
    pub name: [u8; 96],
    pub name_len: usize,
    pub obj_type: i32,
    /// The node's own `Parent` pointer. Siblings all share one — see `acpi_ns_step`.
    pub parent: u64,
    /// The raw four-byte ACPI name, unprocessed. Cannot lie the way a built pathname can.
    pub name4: u32,
}

/// Advance one node in `parent`'s scope. `prev = 0` starts at the first child.
///
/// **Governor context only.** This is the single-step primitive `AcpiWalkNamespace` is built from,
/// exposed so userspace can drive traversal and print each name BEFORE stepping — when a node kills
/// the machine, its predecessor is already on screen. A whole-namespace walk in the kernel can only
/// report "it died"; this reports where.
pub fn ns_step(parent: u64, prev: u64) -> Option<NsNode> {
    let mut next: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut par: *mut core::ffi::c_void = core::ptr::null_mut();
    let mut name = [0u8; 96];
    let mut t: i32 = -1;
    let mut n4: u32 = 0;
    let ok = unsafe {
        acpi_ns_step(parent as *mut _, prev as *mut _, &mut next,
                     name.as_mut_ptr(), 96, &mut t, &mut par, &mut n4)
    };
    if ok != 1 || next.is_null() { return None; }
    let n = name.iter().position(|&b| b == 0).unwrap_or(0);
    Some(NsNode {
        handle: next as u64,
        name,
        name_len: n,
        obj_type: t,
        parent: par as u64,
        name4: n4,
    })
}

/// Resolve an absolute ACPI path to a handle. 0 if not found.
pub fn ns_handle(path: &str) -> u64 {
    let mut buf = [0u8; 128];
    let n = path.len().min(127);
    buf[..n].copy_from_slice(&path.as_bytes()[..n]);
    unsafe { acpi_ns_handle(buf.as_ptr()) as u64 }
}

/// ACPI object type names, for the browser.
pub fn ns_type_name(t: i32) -> &'static str {
    match t {
        0 => "Any", 1 => "Integer", 2 => "String", 3 => "Buffer", 4 => "Package",
        5 => "FieldUnit", 6 => "Device", 7 => "Event", 8 => "Method", 9 => "Mutex",
        10 => "Region", 11 => "Power", 12 => "Processor", 13 => "Thermal",
        14 => "BufferField", 16 => "LocalRegionField", 17 => "LocalBankField",
        18 => "LocalIndexField", 19 => "LocalReference", 20 => "LocalAlias",
        21 => "LocalMethodAlias", 22 => "LocalNotify", 23 => "LocalAddressHandler",
        24 => "LocalResource", 25 => "LocalResourceField", 26 => "LocalScope",
        _ => "?",
    }
}

/// Bring up the Embedded Controller and register its address-space handler.
///
/// **Governor context only (IF=1).** EC transactions poll with timeouts.
///
/// Returns `(ok, data_port, cmd_port)`. Battery methods must not be evaluated unless this returned
/// true: `_STA`/`_BIF`/`_BST` read through `OperationRegion(EmbeddedControl)`, and evaluating them
/// with no handler registered is what took the machine down at `acpi probe 5`.
pub fn ec_install() -> (bool, u32, u32) {
    let (mut d, mut c) = (0u32, 0u32);
    let ok = unsafe { acpi_ec_install(&mut d, &mut c) } == 1;
    (ok, d, c)
}

/// The ACPI control-method battery (`PNP0C0A`), straight from `_BIF` and `_BST`.
///
/// ⚠️ Capacities and rates are **not** in a fixed unit. `power_unit` is 0 for mW/mWh and 1 for
/// mA/mAh, chosen by the firmware. Carried through rather than normalised, because converting
/// mA to mW needs a voltage that is itself only nominal and the error would be silent.
#[derive(Clone, Copy, Default)]
pub struct Battery {
    pub present: bool,
    pub power_unit: i32,
    pub design_cap: i32,
    pub last_full_cap: i32,
    pub design_voltage: i32,
    /// bit0 discharging, bit1 charging, bit2 critical.
    pub state: i32,
    pub present_rate: i32,
    pub remaining_cap: i32,
    pub voltage: i32,
    pub have_bif: bool,
    pub have_bst: bool,
}

impl Battery {
    /// Charge as a percentage of **last-full**, not design capacity.
    ///
    /// A worn cell that genuinely holds 60% of its original charge should still read 100% when it
    /// is as full as it can get — that is what every other OS shows, and measuring against design
    /// capacity would make a healthy-but-aged battery look permanently broken.
    pub fn percent(&self) -> Option<u32> {
        if !self.have_bst || self.last_full_cap <= 0 { return None; }
        Some(((self.remaining_cap as i64 * 100) / self.last_full_cap as i64).clamp(0, 100) as u32)
    }

    pub fn charging(&self) -> bool { self.state & 0b10 != 0 }
    pub fn discharging(&self) -> bool { self.state & 0b01 != 0 }
    pub fn critical(&self) -> bool { self.state & 0b100 != 0 }

    /// The unit label for capacities (`mWh`/`mAh`) and rates (`mW`/`mA`).
    pub fn units(&self) -> (&'static str, &'static str) {
        if self.power_unit == 1 { ("mAh", "mA") } else { ("mWh", "mW") }
    }

    /// Time remaining at the present rate, in minutes. `None` when idle or not discharging.
    pub fn minutes_remaining(&self) -> Option<u32> {
        if !self.discharging() || self.present_rate <= 0 { return None; }
        Some(((self.remaining_cap as i64 * 60) / self.present_rate as i64).min(u32::MAX as i64) as u32)
    }
}

// ═══════════════════════════════════════════════════════════════════════════════════════════════
// ★ THE ACPI CACHE — why AML must not be evaluated from a syscall
// ═══════════════════════════════════════════════════════════════════════════════════════════════
//
// `panel` and `battery` wedged the machine. The post-mortem heartbeat named it exactly:
//
//     tick=27 userspace idle 19s   cores alive: 0b11111110
//     core 1 wrote this ... no core scheduled ANY userspace task for 19s
//
// Bit 0 clear — **core 0 died first**, inside the syscall, and took scheduling with it.
//
// SYSCALL runs with IF=0. `AcpiEvaluateObject` executes AML: it takes the interpreter mutex,
// allocates, walks the namespace, touches OperationRegions (SystemMemory / SystemIO / PCI_Config),
// and for `_BCM` on this laptop ends in a firmware SMI. Every one of those can block on something a
// *preemptible* task holds — which is the preemption-boundary deadlock already documented in
// `drivers/net/mod.rs` and in the thermal governor a few lines from here.
//
// So nothing below is evaluated in a syscall any more. The thermal governor — a real kernel task
// with IF=1, already ticking once a second — refreshes this cache, and the syscalls do nothing but
// copy scalars out of it.
//
// **Both sides use `try_lock`, never `lock`.** That is the whole point: a syscall at IF=0 that
// blocks on a lock this preemptible task holds is precisely the bug being fixed. Losing a refresh
// costs one second; losing the machine costs a power cycle.

#[derive(Clone, Copy)]
pub struct AcpiCache {
    pub panel_found: bool,
    pub panel_path: [u8; 96],
    pub panel_path_len: usize,
    pub panel_levels: [u8; 64],
    pub panel_n: usize,
    /// `u32::MAX` when `_BQC` would not answer.
    pub panel_pct: u32,
    pub bcl_status: u32,
    pub bqc_status: u32,
    pub battery: Battery,
    pub nodes: i32,
    pub devices: i32,
    pub with_bcl: i32,
    /// True once the EmbeddedControl address-space handler is registered. Battery is meaningless
    /// without it — and evaluating battery methods without it crashes the kernel.
    pub ec_ok: bool,
    pub ec_data_port: u32,
    pub ec_cmd_port: u32,
    /// Raw EC register space, filled by .
    pub ec_dump: [u8; 256],
    pub ec_dump_len: usize,
    /// Last raw EC status byte, so a timeout reports what it saw.
    pub ec_status: i32,
    /// False until the governor's first pass has run — so "no panel" and "not asked yet" are
    /// distinguishable, which is the distinction this whole path keeps getting wrong.
    pub ready: bool,
}

impl AcpiCache {
    const EMPTY: AcpiCache = AcpiCache {
        panel_found: false,
        panel_path: [0; 96],
        panel_path_len: 0,
        panel_levels: [0; 64],
        panel_n: 0,
        panel_pct: u32::MAX,
        bcl_status: 5,
        bqc_status: 5,
        battery: Battery {
            present: false, power_unit: 0, design_cap: 0, last_full_cap: 0, design_voltage: 0,
            state: 0, present_rate: 0, remaining_cap: 0, voltage: 0,
            have_bif: false, have_bst: false,
        },
        nodes: 0,
        devices: 0,
        with_bcl: 0,
        ec_ok: false,
        ec_data_port: 0,
        ec_cmd_port: 0,
        ec_dump: [0; 256],
        ec_dump_len: 0,
        ec_status: -1,
        ready: false,
    };
}

static CACHE: spin::Mutex<AcpiCache> = spin::Mutex::new(AcpiCache::EMPTY);

/// A brightness change asked for by userspace, in percent. `-1` means nothing pending.
///
/// `_BCM` ends in a firmware SMI on this machine — the CPU is frozen while SMM services it. That is
/// emphatically not something to do from a syscall at IF=0, so the request is queued here and the
/// governor applies it on its next tick.
static BRIGHT_REQUEST: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(-1);

/// Queue a brightness change. Returns immediately; the governor applies it within a tick.
pub fn request_brightness(pct: u32) {
    BRIGHT_REQUEST.store(pct.clamp(5, 100) as i32, core::sync::atomic::Ordering::Relaxed);
}

/// Read the cache without ever blocking. `None` if the governor holds it this instant.
pub fn cache() -> Option<AcpiCache> {
    CACHE.try_lock().map(|c| *c)
}

/// Re-evaluate everything ACPI. **Kernel-task context only (IF=1).** Never call from a syscall.
pub fn refresh_cache() {
    // Apply a queued brightness change first — it is the only write, and doing it before the read
    // means `_BQC` below reports the value we just set rather than a stale one.
    let want = BRIGHT_REQUEST.swap(-1, core::sync::atomic::Ordering::Relaxed);
    if want >= 0 {
        panel_set(want as u32);
    }

    // ── OPT-IN, ONE STEP AT A TIME ─────────────────────────────────────────────────────────────
    //
    // Three boots died the same way: #GP inside ACPICA about two seconds after ring 3 — i.e. on the
    // governor's first pass — and the allocator rewrite did not change it. So the fault is in one
    // specific ACPI call, and guessing which has been costing a power cycle per guess.
    //
    // Nothing runs automatically any more. `acpi probe <n>` asks for exactly one step on the next
    // tick, and the step number is written to the CMOS breadcrumb FIRST, so if it dies the next
    // boot names the call that did it. That also gives back a machine that boots and stays up,
    // which matters more than an automatic refresh nobody asked for.
    //
    // Steps: 1 walk  2 panel_diag  3 _BCL  4 _BQC  5 battery  6 _BCM
    // A step that survives records 100+n, so "entered 3, never left" is distinguishable from
    // "3 completed and something later died".
    // ★ Battery refreshes EVERY TICK, unconditionally — it is not gated behind a probe.
    //
    // The probe gate exists because ACPI *walks* crash this kernel. Raw EC reads do not walk: they
    // resolve one handle, read `_CRS`, and talk to two I/O ports. All of that is hardware-verified,
    // and the Entity needs live numbers rather than something you have to ask for by hand.
    //
    // try_lock, never lock — a syscall at IF=0 blocking on a lock this preemptible task holds is the
    // wedge this whole subsystem was rebuilt to avoid.
    // ⚠️ ec_install() FIRST, every tick. It is idempotent (returns immediately once installed), but
    // `acpi_ec_battery` refuses to read until it has run — so calling ec_battery() alone silently
    // returned zeros, and the Entity said "No battery" while the terminal, which had been through
    // `acpi probe 7`, showed the real numbers. Same data, two paths, one of them uninitialised.
    // ★★★ WATCH THE ROOT CHAIN ACROSS THIS TICK'S OWN ACPI CALLS.
    //
    // Bring-up ends with **1802 children and a clean NULL tail** — measured, at all three stages,
    // with `FAILED=0` from the pool. By the time userspace runs `acpi ls` it is 637 and ends on a
    // block that is not a node. So the namespace is built correctly and something breaks it at
    // RUNTIME, and the only ACPI activity in between is this function.
    //
    // Counting on either side of the two calls below turns "it is broken by the time I look" into
    // "it broke during THIS call on THIS tick". `ec_install` evaluates `_CRS`; AML execution creates
    // temporary namespace nodes and deletes them, and `AcpiNsDeleteNode` unlinks by walking the
    // parent's child list and rewriting a `Peer` — on a chain 1802 long. That is the only code after
    // load that writes a `Peer` at all.
    //
    // Counting is three pointer loads per node with a cap and full validation; it cannot crash and
    // it cannot itself perturb the namespace.
    let before = root_count();
    let (ec_ok, dport, cport) = ec_install();
    let mid = root_count();
    let bat = if ec_ok { ec_battery() } else { Battery::default() };
    let after = root_count();
    note_root_counts(before, mid, after);

    if let Some(mut c) = CACHE.try_lock() {
        c.ec_ok = ec_ok;
        c.ec_data_port = dport;
        c.ec_cmd_port = cport;
        c.battery = bat;
        c.ready = true;
    }

    let step = PROBE.swap(0, core::sync::atomic::Ordering::Relaxed);
    if step == 0 {
        return;
    }
    crate::postmortem::user_mark(step);

    match step {
        1 => {
            // ★ THE FULL NAMESPACE WALK IS GONE. It is what was killing the machine.
            //
            // `acpi probe 1` left the CMOS breadcrumb at 1 ("entered, never returned"), which is the
            // one byte of the post-mortem that reads back reliably. So `count_nodes` was the culprit
            // — a walk over every node in the namespace whose callback re-entered ACPICA twice per
            // node (`AcpiGetType`, `AcpiGetHandle`) across thousands of objects of every type.
            //
            // It is deleted rather than debugged because it was only ever a diagnostic, and the
            // question it answered — "did the DSDT load?" — is now answered directly and safely by
            // ACPICA's own log: "13 ACPI AML tables successfully acquired and loaded", DSDT flagged
            // LOADED. Keeping a walk that crashes in order to re-derive a fact we already have is a
            // bad trade, and this one cost several power cycles.
            //
            // `nyx_lcd_handle()` still falls back to a walk if the fixed LCD path misses, but that
            // callback only does a single `AcpiGetHandle` and terminates on the first hit — it is
            // not this. If step 2 also dies, that fallback is the next thing to remove.
            // ★ RESTORED as the cheapest reproducer for the walk bug.
            //
            // It was removed after it killed the machine, on the grounds that its answer ("did the
            // DSDT load?") was available elsewhere. But the OS layer has since been implemented for
            // real — counting semaphores, exclusive locks, per-task thread ids, where before every
            // one was a no-op returning a shared fake handle — and `AcpiNsWalkNamespace` is the
            // path that leans hardest on exactly those. So this is now the one-command test of
            // whether that fixed it.
            //
            // If this survives, `AcpiInstallAddressSpaceHandler` (which walks the EC subtree) should
            // too, and the EC driver in custom_acpi.c comes up with no further changes.
            let (nodes, devices, with_bcl) = count_nodes(PROBE_ARG.load(core::sync::atomic::Ordering::Relaxed) as i32);
            if let Some(mut c) = CACHE.try_lock() {
                c.nodes = nodes;
                c.devices = devices;
                c.with_bcl = with_bcl;
                c.ready = true;
            }
        }
        8 => {
            // ★ The control for step 1: the same descent, driven by `AcpiGetNextObject`.
            //
            // Run this whenever step 1 dies. Two outcomes, both conclusive:
            //   survives -> `AcpiNsWalkNamespace` is the bug and this is the replacement walk
            //   dies on the same node (the panic screen names it) -> the namespace DATA is bad, and
            //               no amount of work on the walker will help
            let nodes = count_nodes_stepper(PROBE_ARG.load(core::sync::atomic::Ordering::Relaxed) as i32);
            if let Some(mut c) = CACHE.try_lock() {
                c.nodes = nodes;
                c.ready = true;
            }
        }
        2 => {
            let d = panel_diag();
            if let Some(mut c) = CACHE.try_lock() {
                c.panel_found = d.found;
                c.panel_path = d.path;
                c.panel_path_len = d.path_len;
                c.bcl_status = d.bcl_status;
                c.bqc_status = d.bqc_status;
                c.ready = true;
            }
        }
        3 => {
            let mut levels = [0u8; 64];
            let n = panel_levels(&mut levels).unwrap_or(0);
            if let Some(mut c) = CACHE.try_lock() {
                c.panel_levels = levels;
                c.panel_n = n;
                c.ready = true;
            }
        }
        4 => {
            let pct = panel_get().unwrap_or(u32::MAX);
            if let Some(mut c) = CACHE.try_lock() {
                c.panel_pct = pct;
                c.ready = true;
            }
        }
        5 => {
            // Battery, gated on the EC coming up first.
            //
            // `_STA`/`_BIF`/`_BST` read through `OperationRegion(EmbeddedControl)`. Evaluating them
            // with no handler registered is what killed the machine here before, so the gate is
            // load-bearing, not a nicety: if the EC does not come up we record that and evaluate
            // NOTHING. The ports are stashed so `battery` can print them — if they look wrong, that
            // is the first thing worth seeing on a box with no serial console.
            // ★ Sub-breadcrumbs. Step 5 does two unrelated things — bring the EC up, then evaluate
            // battery methods — and "5, entered, never returned" cannot distinguish them. The
            // single-byte CMOS mark is the only post-mortem field that reads back reliably on this
            // machine, so spend it at finer granularity rather than adding another whole boot.
            //
            //   50 = entering acpi_ec_install (AcpiGetDevices + _CRS walk + install handler)
            //   51 = EC installed OK, ports known
            //   52 = entering battery() — _STA/_BIF/_BST, the first real EC traffic
            //   105 = step 5 completed
            crate::postmortem::user_mark(50);
            let (ok, dport, cport) = ec_install();
            if let Some(mut c) = CACHE.try_lock() {
                c.ec_ok = ok;
                c.ec_data_port = dport;
                c.ec_cmd_port = cport;
                c.ready = true;
            }
            if !ok {
                return;
            }
            crate::postmortem::user_mark(51);

            crate::postmortem::user_mark(52);
            let bat = battery();
            if let Some(mut c) = CACHE.try_lock() {
                c.battery = bat;
            }
        }

        7 => {
            // EC RAW DUMP — the route around the walk bug.
            //
            // `AcpiInstallAddressSpaceHandler` walks internally, and walks #GP this kernel — all
            // three structural differences from the working stepper were tested and ruled out
            // (reader lock bypassed, ACPI_NS_WALK_NO_UNLOCK, a real OS layer). So AML can never
            // reach the EC here and `_BIF`/`_BST` are unreachable. The PORTS are not: mark 53
            // (handle) and mark 54 (`_CRS`) both pass, so the EC can be read directly.
            //
            // This dumps all 256 EC registers into the cache so `ec dump` can print them. Finding
            // the battery fields is then correlation, not guesswork: Fedora reports BAT0's decoded
            // values for this exact machine, and charge (mAh) and voltage (mV) are distinctive
            // 16-bit little-endian patterns. Dump twice a few minutes apart and the bytes that move
            // are the live ones.
            crate::postmortem::user_mark(70);
            let (ok, dport, cport) = ec_install();
            let mut buf = [0u8; 256];
            // Battery FIRST: it selects the bank at EC register 0x03, without which the window at
            // 0x10..0x22 reads stale zeros. That select is why the earlier raw dump matched no
            // anchor — the data was never in view.
            let bat = if ok { ec_battery() } else { Battery::default() };
            let n = if ok { ec_read_range(0, &mut buf) } else { 0 };
            if let Some(mut c) = CACHE.try_lock() {
                c.ec_ok = ok;
                c.ec_data_port = dport;
                c.ec_cmd_port = cport;
                c.ec_dump = buf;
                c.ec_dump_len = n;
                c.ec_status = ec_last_status();
                c.battery = bat;
                c.ready = true;
            }
            crate::postmortem::user_mark(107);
        }
        6 => {
            let want = BRIGHT_REQUEST.swap(-1, core::sync::atomic::Ordering::Relaxed);
            if want >= 0 {
                panel_set(want as u32);
            }
        }
        _ => {}
    }

    crate::postmortem::user_mark(100u8.saturating_add(step));
    return;

}

/// Which ACPI step the governor should run on its next tick. 0 = do nothing.
///
/// Nothing runs on its own: three boots died on the governor's automatic first pass and each guess
/// at which call was responsible cost a power cycle. `acpi probe <n>` requests exactly one.
static PROBE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Ask the governor to run one ACPI step. See `refresh_cache` for the step numbers.
pub fn request_probe(step: u8, arg: u32) {
    PROBE_ARG.store(arg, core::sync::atomic::Ordering::Relaxed);
    PROBE.store(step, core::sync::atomic::Ordering::Relaxed);
}

/// Extra parameter for the requested step. For step 1 this is the walk depth (0 = unlimited).
static PROBE_ARG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub fn battery() -> Battery {
    let mut v = [0i32; 11];
    let found = unsafe { acpi_battery_read(v.as_mut_ptr(), 11) };
    Battery {
        present: found == 1,
        power_unit: v[1],
        design_cap: v[2],
        last_full_cap: v[3],
        design_voltage: v[4],
        state: v[5],
        present_rate: v[6],
        remaining_cap: v[7],
        voltage: v[8],
        have_bif: v[9] == 1,
        have_bst: v[10] == 1,
    }
}

/// Does an absolute ACPI path resolve in the live namespace?
pub fn path_status(path: &str) -> u32 {
    // ACPICA takes a NUL-terminated C string; 128 is well past the longest legal ACPI path.
    let mut buf = [0u8; 128];
    let n = path.len().min(127);
    buf[..n].copy_from_slice(&path.as_bytes()[..n]);
    unsafe { acpi_path_status(buf.as_ptr()) }
}

/// `(nodes, devices, with_bcl)` as seen by a full namespace walk.
/// The root's child-chain length right now, and whether it ends on a non-node.
///
/// Safe anywhere: follows `Peer` only, validates each node's parent and name, caps at 8192 and stops
/// **before** dereferencing anything that fails validation.
pub fn root_count() -> (i32, bool) {
    let mut bad: i32 = 0;
    let n = unsafe { acpi_root_count(&mut bad) };
    (n, bad != 0)
}

// ── Boot checkpoints ───────────────────────────────────────────────────────────────────────────
//
// ★★★ The governor watch answered its question and pointed somewhere else: `first 637 now 637`,
// dangling, never moved. So `ec_install` and `ec_battery` are innocent — the chain was ALREADY
// short on the very first tick. Bring-up ends with 1802 and a clean NULL, so it is destroyed
// somewhere in the rest of `kernel_main`.
//
// Same technique, one level up: count at each stage of boot and see which one eats it. This is the
// third time counting-at-checkpoints has moved the investigation when reasoning did not, so it is
// worth keeping the mechanism rather than the one-off.
//
// ⚠️ Cannot use the ACPICA log — `close_acpi_log()` runs at the end of `init_intel_acpica`, before
// any of these fire. So the marks live in a static and `panel` prints them.
//
// Recording allocates nothing and takes a `try_lock`: this runs during early boot, before the heap
// is under pressure and on paths where blocking would be fatal.
pub struct BootMark {
    pub tag: &'static str,
    pub count: i32,
    pub bad: bool,
}

pub static BOOT_MARKS: spin::Mutex<([Option<BootMark>; 24], usize)> =
    spin::Mutex::new(([const { None }; 24], 0));

/// Count the root chain and file it under `tag`. Call from anywhere in `kernel_main`.
pub fn boot_checkpoint(tag: &'static str) {
    let (n, bad) = root_count();
    if let Some(mut m) = BOOT_MARKS.try_lock() {
        let i = m.1;
        if i < 24 {
            m.0[i] = Some(BootMark { tag, count: n, bad });
            m.1 = i + 1;
        }
    }
}

/// The checkpoints as text, for `panel`. The first line whose count is not the previous one names
/// the boot stage that broke the namespace.
pub fn boot_marks_report() -> alloc::string::String {
    let mut out = alloc::string::String::new();
    if let Some(m) = BOOT_MARKS.try_lock() {
        let mut prev: Option<i32> = None;
        for e in m.0.iter().flatten() {
            let flag = match prev {
                Some(p) if p != e.count => "   <<< CHANGED HERE",
                _ => "",
            };
            out.push_str(&alloc::format!(
                "    {:<18} {}{}{}\n",
                e.tag,
                e.count,
                if e.bad { " DANGLING" } else { "" },
                flag
            ));
            prev = Some(e.count);
        }
    }
    if out.is_empty() {
        out.push_str("    (none recorded)\n");
    }
    out
}

// ── The runtime chain watch ────────────────────────────────────────────────────────────────────
//
// One shot at the truth, kept from the FIRST tick that sees a change. Later ticks are fallout: once
// the chain is short, every subsequent tick reports it short, and the interesting number is the one
// from the moment it moved.
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::Relaxed};
/// Chain length as of the first governor tick — the baseline bring-up handed over.
pub static ROOT_FIRST: AtomicI32 = AtomicI32::new(-1);
/// Chain length at the end of the most recent tick.
pub static ROOT_NOW: AtomicI32 = AtomicI32::new(-1);
/// Which tick first saw the count change, and the three counts from that tick.
pub static ROOT_BREAK_TICK: AtomicU32 = AtomicU32::new(0);
pub static ROOT_BREAK_BEFORE: AtomicI32 = AtomicI32::new(-1);
pub static ROOT_BREAK_MID: AtomicI32 = AtomicI32::new(-1);
pub static ROOT_BREAK_AFTER: AtomicI32 = AtomicI32::new(-1);
pub static ROOT_DANGLING: AtomicBool = AtomicBool::new(false);
static ROOT_TICK: AtomicU32 = AtomicU32::new(0);

fn note_root_counts(before: (i32, bool), mid: (i32, bool), after: (i32, bool)) {
    let tick = ROOT_TICK.fetch_add(1, Relaxed);
    let _ = ROOT_FIRST.compare_exchange(-1, before.0, Relaxed, Relaxed);
    ROOT_NOW.store(after.0, Relaxed);
    if after.1 || mid.1 || before.1 {
        ROOT_DANGLING.store(true, Relaxed);
    }
    // First tick where anything moved wins, and it records all three so the CALL is named, not just
    // the tick: before != mid blames `ec_install`, mid != after blames `ec_battery`, and
    // before != ROOT_FIRST blames something outside this function entirely.
    let moved = before.0 != mid.0 || mid.0 != after.0 || before.0 != ROOT_FIRST.load(Relaxed);
    if moved && ROOT_BREAK_TICK.load(Relaxed) == 0 {
        ROOT_BREAK_TICK.store(tick + 1, Relaxed);
        ROOT_BREAK_BEFORE.store(before.0, Relaxed);
        ROOT_BREAK_MID.store(mid.0, Relaxed);
        ROOT_BREAK_AFTER.store(after.0, Relaxed);
    }
}

pub fn count_nodes(depth: i32) -> (i32, i32, i32) {
    let (mut n, mut d, mut b) = (0i32, 0i32, 0i32);
    unsafe { acpi_count_nodes(depth, &mut n, &mut d, &mut b) };
    (n, d, b)
}

/// The same walk, descending with `AcpiGetNextObject` instead of `AcpiNsWalkNamespace`.
///
/// The one variable this isolates is the WALKER. Both versions publish the same per-node cursor, so
/// if they die the panic screen says whether they died on the same node. See `NyxStepWalk` in
/// `custom_acpi.c` for why the earlier "traversal is sound" reasoning did not hold.
pub fn count_nodes_stepper(depth: i32) -> i32 {
    let mut n = 0i32;
    unsafe { acpi_count_nodes_stepper(depth, &mut n) };
    n
}

/// A text report locating exactly where the live namespace stops matching the decompiled DSDT.
///
/// Built in the kernel and handed to userspace as plain text rather than a packed struct. The facts
/// live here; the shape of them changes every time a hypothesis is eliminated, and re-cutting an ABI
/// for each one is how a diagnostic stops getting extended.
/// The bring-up statuses alone — pure formatting of values captured at boot.
///
/// Split out of `ns_report` because that one probes the live namespace (`AcpiGetHandle`,
/// `AcpiWalkNamespace`) and so must never run in a syscall. This half touches no ACPICA at all and
/// is safe anywhere.
pub fn init_report() -> alloc::string::String {
    use alloc::format;
    let i = acpica_init_status();
    let step = |s: u32| -> alloc::string::String {
        if s == 0xFFFF_FFFF { alloc::string::String::from("never reached") }
        else { format!("{:#06x} {}", s, acpi_status_name(s)) }
    };
    let mut out = alloc::string::String::new();
    // ★ Which kernel this actually is. See `build.rs`: a stale flash once produced two boots of
    // byte-identical diagnostic output and sent a whole cycle after a code bug that did not exist.
    // If this number has not changed since the last build, the machine is running the old image.
    out.push_str(&format!("  build stamp: {}\n", env!("NYX_BUILD_STAMP")));
    out.push_str("  ACPICA bring-up:\n");
    out.push_str(&format!("    subsystem {}\n", step(i.subsystem)));
    out.push_str(&format!("    tables    {}\n", step(i.tables)));
    out.push_str(&format!("    load      {}  (AE_OK here does NOT prove the DSDT loaded)\n",
                          step(i.load)));
    out.push_str(&format!("    enable    {}\n", step(i.enable)));
    out.push_str(&format!("    objects   {}\n", step(i.objects)));
    out
}

/// ⚠️ **Kernel-task context only (IF=1).** Probes the live namespace; must not run in a syscall.
pub fn ns_report() -> alloc::string::String {
    use alloc::format;
    let i = acpica_init_status();
    let step = |s: u32| -> alloc::string::String {
        if s == 0xFFFF_FFFF { alloc::string::String::from("never reached") }
        else { format!("{:#06x} {}", s, acpi_status_name(s)) }
    };

    let mut out = alloc::string::String::new();
    out.push_str("  ACPICA bring-up:\n");
    out.push_str(&format!("    subsystem {}\n", step(i.subsystem)));
    out.push_str(&format!("    tables    {}\n", step(i.tables)));
    // ⚠️ Do not read this line as "the namespace loaded". AcpiLoadTables rewrites the
    // any-table-failed status (AE_CTRL_TERMINATE) to AE_OK on purpose. The walk counts below and
    // the ACPICA log are the load's only honest witnesses.
    out.push_str(&format!("    load      {}  (AE_OK here does NOT prove the DSDT loaded)\n",
                          step(i.load)));
    out.push_str(&format!("    enable    {}\n", step(i.enable)));
    out.push_str(&format!("    objects   {}\n", step(i.objects)));

    out.push_str("  namespace ladder:\n");
    for p in [
        "\\_SB",
        "\\_SB.PCI0",
        "\\_SB.PCI0.GFX0",
        "\\_SB.PCI0.GFX0.LCD",
        "\\_SB.PCI0.GFX0.DD1F",
    ] {
        let s = path_status(p);
        out.push_str(&format!(
            "    {:<22} {}\n",
            p,
            if s == 0 { "ok" } else { acpi_status_name(s) }
        ));
    }

    let (n, d, b) = count_nodes(0);
    out.push_str(&format!(
        "  walk: {} nodes, {} devices, {} with _BCL\n",
        n, d, b
    ));
    out
}

/// What the panel probe actually found. Read-only.
pub struct PanelDiag {
    /// The resolved ACPI path, e.g. `\_SB.PCI0.GFX0.LCD`. Empty if no panel device was found.
    pub path: [u8; 96],
    pub path_len: usize,
    /// True if a device carrying `_BCL` exists at all.
    pub found: bool,
    /// Raw `ACPI_STATUS` from evaluating `_BCL` and `_BQC`. 0 is AE_OK.
    pub bcl_status: u32,
    pub bqc_status: u32,
}

/// Probe the panel and report *why* it did or did not work.
///
/// Separate from [`panel_levels`] on purpose. That one answers "give me the levels" and folds every
/// failure into `None`; this one answers "what is wrong", which is a different question and the only
/// one worth spending a power cycle on.
pub fn panel_diag() -> PanelDiag {
    let mut d = PanelDiag {
        path: [0u8; 96],
        path_len: 0,
        found: false,
        bcl_status: 5, // AE_NOT_FOUND
        bqc_status: 5,
    };
    let r = unsafe {
        acpi_panel_diag(d.path.as_mut_ptr(), d.path.len() as i32,
                        &mut d.bcl_status, &mut d.bqc_status)
    };
    d.found = r == 1;
    d.path_len = d.path.iter().position(|&b| b == 0).unwrap_or(0);
    d
}

/// Name an `ACPI_STATUS` in the few cases that matter here.
///
/// Not the full ACPICA exception table — just the ones this path can realistically produce, so the
/// screen shows a cause rather than a hex number the reader has to go look up. Anything else falls
/// through to its code, which is still better than "could not evaluate".
pub fn acpi_status_name(st: u32) -> &'static str {
    match st {
        0x0000 => "AE_OK",
        0x0001 => "AE_ERROR",
        0x0004 => "AE_NO_MEMORY",
        0x0005 => "AE_NOT_FOUND — a name the method calls is missing (SSDT not loaded?)",
        0x0006 => "AE_NOT_EXIST",
        0x0007 => "AE_ALREADY_EXISTS",
        0x0008 => "AE_TYPE",
        0x0009 => "AE_NULL_OBJECT",
        0x000A => "AE_NULL_ENTRY",
        0x000C => "AE_BAD_PARAMETER — bad path or handle, not a firmware problem",
        0x0011 => "AE_NO_HANDLER",
        0x1001 => "AE_BAD_CHARACTER",
        0x1002 => "AE_BAD_PATHNAME",
        0x2002 => "AE_AML_OPERAND_TYPE",
        _ => "unrecognised ACPI_STATUS",
    }
}

/// The panel's supported brightness levels, straight out of `_BCL`. Read-only.
///
/// The first two entries are the AC and battery defaults per the ACPI spec; the rest are the
/// selectable levels. Returned unfiltered — this is evidence, and trimming it is how you end up
/// arguing about what the firmware actually said.
pub fn panel_levels(out: &mut [u8]) -> Option<usize> {
    let n = unsafe { acpi_panel_levels(out.as_mut_ptr(), out.len() as i32) };
    if n < 0 { None } else { Some(n as usize) }
}

/// The current panel level from `_BQC`, 0..=100. Read-only.
pub fn panel_get() -> Option<u32> {
    let v = unsafe { acpi_panel_get() };
    if v < 0 { None } else { Some(v as u32) }
}

/// Set the panel level through `_BCM`.
///
/// ⚠️ This is the one call here that reaches hardware, and on this machine `_BCM` ends in an SMI —
/// firmware takes the whole machine to service it. That is the *supported* path, not a poke: ACPICA
/// runs the ASL as written, including the OS-identification handshake the SMM handler expects. The
/// distinction matters, because `laptop_fans::trigger_dell_fans()` firing a guessed SMI is what
/// caused the overheating arc, and "we are calling the method the firmware published" is exactly
/// what makes this a different thing.
pub fn panel_set(level: u32) -> bool {
    unsafe { acpi_panel_set(level.clamp(5, 100) as i32) == 1 }
}

pub fn get_acpi_temperature() -> u8 {
    unsafe { acpi_get_system_temp() as u8 }
}

pub fn power_on_wifi_via_acpi() -> bool {
    crate::serial_println!("[ACPI] Initiating Motherboard 'Wake Everything' sequence...");
    let count = unsafe { acpi_wake_cnvi_wifi() };
    crate::serial_println!("[ACPI] Blasted _PS0 (Power On) to {} hidden hardware nodes!", count);
    true
}

pub fn scan_for_modern_inputs() {
    crate::serial_println!("[ACPI] Scanning motherboard for I2C-HID devices (PNP0C50)...");
    crate::vga_println!("[ACPI] Scanning for I2C Trackpads...");
    
    let count = unsafe { acpi_find_i2c_hid() };
    
    if count > 0 {
        crate::serial_println!("[ACPI] SUCCESS: Found {} I2C-HID device(s)!", count);
        crate::vga_println!("[ACPI] Found {} I2C-HID device(s)!", count);
    } else {
        crate::serial_println!("[ACPI] No I2C-HID devices found. It might be USB-based.");
        crate::vga_println!("[ACPI] No I2C-HID found.");
    }
}

pub fn set_active_cooling(enable: bool) {
    let state = if enable { 1 } else { 0 };
    unsafe {
        acpi_set_fan_state(state);
    }
}

pub fn get_dsdt_data(buf_ptr: *mut u8, max_len: usize) -> usize {
    unsafe {
        let mut dsdt_header: *mut core::ffi::c_void = core::ptr::null_mut();
        
        // Ask Intel ACPICA to find the DSDT table for us
        let status = AcpiGetTable(
            b"DSDT\0".as_ptr() as *mut i8, 
            1, 
            &mut dsdt_header as *mut _ as *mut *mut _
        );
        
        if status == 0 && !dsdt_header.is_null() {
            let dsdt_virt = dsdt_header as u64;
            // Read the length of the table (stored at offset 4 of the header)
            let length = core::ptr::read_volatile((dsdt_virt + 4) as *const u32) as usize;
            
            let copy_len = core::cmp::min(length, max_len);
            core::ptr::copy_nonoverlapping(dsdt_virt as *const u8, buf_ptr, copy_len);
            return copy_len;
        }
        0
    }
}
// ==========================================
// 5. POWER MANAGEMENT (SLEEP / OFF)
// ==========================================
/// Switch the machine off (ACPI S5). Never returns if it works.
///
/// ★ REWRITTEN 2026-09-08, after the first userspace shutdown ended in a red screen.
///
/// The old body was `AcpiEnterSleepStatePrep(5)` then `AcpiEnterSleepState(5)`, which is the
/// textbook call pair and does far more than switching off needs. `...StatePrep` evaluates `_PTS`
/// (a real AML **method**, which on this class of laptop writes to the embedded controller),
/// may evaluate `_GTS` and `_SST`, and walks the GPE blocks disabling wake sources. Any of those
/// can fault in a kernel whose ACPI operation-region coverage is partial — and this one's is: the
/// EC handler cannot be installed because doing so walks the namespace and #GPs (which is why the
/// battery is read through raw ports instead, see the EC memory note).
///
/// So the preferred path now skips all of it. Powering off is, at bottom, two things:
///
///   1. **Ask what value means "off"** — `AcpiGetSleepTypeData(5, &a, &b)` reads `\_S5_`, which is
///      a static **package** on essentially every machine, not a method. Cheap, and it evaluates
///      nothing that can touch a device.
///   2. **Write it to PM1_CNT** — set `SLP_TYP` to that value and `SLP_EN`, and the chipset cuts
///      power. This is `AcpiHwLegacySleep`'s final act with everything else stripped away.
///
/// The full ACPICA pair is kept as a **fallback**, because a machine that needs `_PTS` run before it
/// will power off is a real thing and this order tries the safe one first.
///
/// ⚠️ Every stage announces itself on the SCREEN, not the serial port. There is no serial console on
/// this laptop, and "the shutdown did nothing" and "the shutdown faulted" are questions that a line
/// of text answers and a dead machine does not.
/// ⚠️ **No `vga_println!` on the success path any more.** The caller has already painted the
/// shutdown screen (`boot_screen::farewell`), and progress chatter over the top of it is precisely
/// the "is it broken or is it working?" look this whole path was changed to remove. The failure
/// messages stay — at that point diagnostics beat aesthetics, and they go through `boot_screen`
/// rather than the log so they land on the screen that is actually up.
pub fn poweroff() {
    unsafe {
        // ── 1. The narrow path ────────────────────────────────────────────────────────────────
        let mut slp_a: u8 = 0;
        let mut slp_b: u8 = 0;
        let st = AcpiGetSleepTypeData(5, &mut slp_a as *mut u8, &mut slp_b as *mut u8);
        if st == 0 {
            let fadt = &*core::ptr::addr_of!(AcpiGbl_FADT);
            // Prefer the 64-bit GAS form when the firmware supplies one and it is SystemIO
            // (SpaceId 1); fall back to the legacy 32-bit field otherwise.
            let pick = |x: &ACPI_GENERIC_ADDRESS, legacy: u32| -> u16 {
                if x.SpaceId == 1 && x.Address != 0 {
                    x.Address as u16
                } else {
                    legacy as u16
                }
            };
            let a_port = pick(&fadt.XPm1aControlBlock, fadt.Pm1aControlBlock);
            let b_port = pick(&fadt.XPm1bControlBlock, fadt.Pm1bControlBlock);
            // Serial, not the screen: the shutdown screen is up by now. This line mattered while the
            // S5 path was being brought up and is kept for the next time it needs bisecting.
            crate::serial_println!(
                "[ACPI] S5 type a={} b={}  PM1a={:#06x} PM1b={:#06x}", slp_a, slp_b, a_port, b_port);

            // SLP_TYP is bits 10-12, SLP_EN is bit 13. Read-modify-write so the rest of PM1_CNT —
            // SCI_EN above all — survives; clearing it would hand the machine back to the firmware
            // mid-write.
            const SLP_TYP_MASK: u16 = 0x1C00;
            const SLP_EN: u16 = 0x2000;
            core::arch::asm!("cli", options(nomem, nostack));
            if a_port != 0 {
                let mut port = x86_64::instructions::port::Port::<u16>::new(a_port);
                let v = (port.read() & !SLP_TYP_MASK) | ((slp_a as u16) << 10) | SLP_EN;
                port.write(v);
            }
            if b_port != 0 {
                let mut port = x86_64::instructions::port::Port::<u16>::new(b_port);
                let v = (port.read() & !SLP_TYP_MASK) | ((slp_b as u16) << 10) | SLP_EN;
                port.write(v);
            }
            // The chipset takes a moment. Spin rather than halt — a `hlt` here on a machine that
            // was going to power off anyway costs nothing, but on one that is not it makes the box
            // look hung instead of letting the fallback run.
            for _ in 0..200_000_000u64 {
                core::hint::spin_loop();
            }
            x86_64::instructions::interrupts::enable();
            crate::vga_println!("[ACPI] PM1_CNT write did not take. Trying the full ACPICA path.");
        } else {
            crate::vga_println!("[ACPI] \\_S5_ unavailable (status {:#x}). Trying ACPICA.", st);
        }

        // ── 2. The full path, including _PTS ──────────────────────────────────────────────────
        AcpiEnterSleepStatePrep(5);
        core::arch::asm!("cli", options(nomem, nostack));
        AcpiEnterSleepState(5);

        // ── 3. Neither worked ─────────────────────────────────────────────────────────────────
        x86_64::instructions::interrupts::enable();
        crate::vga_println!("[ACPI] The firmware refused S5. Hold the power button.");
    }
}

/// Restart the machine. Never returns.
///
/// Four methods, most correct first, because **a reset that does not reset leaves the machine in
/// `cli; hlt` with the screen still lit** — which from the outside is indistinguishable from the
/// class of freeze this project has spent months chasing. Each is given a moment to take effect
/// before the next is tried; the trailing `hlt` loop is unreachable in practice.
///
/// 1. `AcpiReset` — writes the FADT's reset register with the firmware's own reset value. The only
///    method here that is not a hardware quirk. It returns `AE_NOT_EXIST` on a board that declares
///    no reset register, which is why it cannot be the only one.
/// 2. Port `0xCF9`, the PCH reset control register: `0x02` arms, `0x04` triggers, `0x08` asks for a
///    full rather than a soft reset — written together as `0x0E`. This is what Linux reaches for on
///    essentially every Intel chipset, this one included.
/// 3. Port `0x64` command `0xFE` — the 8042 keyboard controller pulsing the CPU's RESET line. A
///    convention from 1984 that most firmware still honours.
/// 4. A triple fault: load a null IDT and take an interrupt. No handler, no double-fault handler and
///    no task gate, so the CPU resets. Guaranteed by the architecture, and the reason this function
///    can promise never to return.
pub fn restart() -> ! {
    crate::serial_println!("\n[ACPI] Restarting...");
    unsafe {
        AcpiReset();
        spin_a_moment();

        let mut cf9 = x86_64::instructions::port::Port::<u8>::new(0xCF9);
        cf9.write(0x0Eu8);
        spin_a_moment();

        // Drain the 8042's input buffer first, or the command is dropped and this looks like the
        // method simply not working.
        let mut status = x86_64::instructions::port::Port::<u8>::new(0x64);
        for _ in 0..100_000 {
            if status.read() & 0x02 == 0 {
                break;
            }
        }
        status.write(0xFEu8);
        spin_a_moment();

        core::arch::asm!("cli", options(nomem, nostack));
        let null_idt = x86_64::structures::DescriptorTablePointer {
            limit: 0,
            base: x86_64::VirtAddr::new(0),
        };
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3", options(nomem, nostack));
        loop {
            core::arch::asm!("hlt", options(nomem, nostack));
        }
    }
}

/// A short busy wait between reset attempts. Deliberately not a sleep: by the time this runs the
/// scheduler may have nothing left to switch to, and a reset already in flight needs the CPU to
/// stay out of its way rather than to yield.
fn spin_a_moment() {
    for _ in 0..50_000_000u64 {
        core::hint::spin_loop();
    }
}