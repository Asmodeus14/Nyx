#include "acpi.h"
// ACPICA's internal header. Needed for three things the public API does not expose: `acclib.h`'s
// local `snprintf`/`vsnprintf` (we build with no libc), and the `AcpiGbl_RootTableList` /
// `AcpiGbl_DsdtIndex` globals, which are the only honest account of what the table manager loaded.
#include "accommon.h"
// For AcpiNsWalkNamespace and ACPI_NS_WALK_UNLOCK — the internal walker, called directly so the
// namespace reader lock can be bypassed. accommon.h does not pull this one in.
#include "acnamesp.h"

// ==========================================
// 1. WAKE EVERYTHING CALLBACK (LEGACY WIFI)
// ==========================================
static ACPI_STATUS WakeEverythingCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **ReturnValue) {
    ACPI_HANDLE TempHandle;
    
    // Check if this specific piece of hardware has a Power State 0 (_PS0) switch
    if (ACPI_SUCCESS(AcpiGetHandle(Object, (char*)"_PS0", &TempHandle))) {
        
        // It has a power switch! YANK IT!
        AcpiEvaluateObject(Object, (char*)"_PS0", NULL, NULL);
        
        // Tally up how many things we woke up
        *((int*)Context) += 1; 
    }

    return AE_OK; // Keep walking the tree
}

int acpi_wake_cnvi_wifi(void) {
    int wake_count = 0;
    
    // Walk the entire tree and blast the power-on signal to everything
    AcpiWalkNamespace(ACPI_TYPE_ANY, ACPI_ROOT_OBJECT, ACPI_UINT32_MAX, 
                      WakeEverythingCallback, NULL, &wake_count, NULL);
                      
    return wake_count;
}

// ==========================================
// 2. THE MODERN I2C-HID SCANNER
// ==========================================
static ACPI_STATUS I2cHidCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **ReturnValue) {
    ACPI_BUFFER Buffer;
    Buffer.Length = ACPI_ALLOCATE_BUFFER;
    Buffer.Pointer = NULL;

    // We found one! Increment the counter that Rust gave us a pointer to.
    *((int*)Context) += 1; 
    
    // (Optional) If we wanted to parse the exact hardware path string, we do it here.
    AcpiGetName(Object, ACPI_FULL_PATHNAME, &Buffer);
    if (Buffer.Pointer) {
        AcpiOsFree(Buffer.Pointer);
    }

    return AE_OK; 
}

int acpi_find_i2c_hid(void) {
    int device_count = 0;
    
    // PNP0C50 is the industry standard Hardware ID for I2C Human Interface Devices
    AcpiGetDevices((char*)"PNP0C50", I2cHidCallback, &device_count, NULL);
                      
    return device_count;
}

// ==========================================
// 3. NYXOS ACPI THERMAL & FAN CONTROLLER
// ==========================================

// Callback function to evaluate _ON or _OFF on a fan
static ACPI_STATUS TurnOnFanCallback(ACPI_HANDLE Object, UINT32 NestingLevel, void *Context, void **ReturnValue) {
    int turn_on = *(int*)Context;
    if (turn_on) {
        AcpiEvaluateObject(Object, "_ON", NULL, NULL);
    } else {
        AcpiEvaluateObject(Object, "_OFF", NULL, NULL);
    }
    return AE_OK; // Keep searching for more fans
}

// Global hook to find fans and toggle them
int acpi_set_fan_state(int turn_on) {
    int context = turn_on;
    AcpiGetDevices("PNP0C0B", TurnOnFanCallback, &context, NULL);
    return 1;
}

// --- THE RAW ACPICA THERMAL READER ---
static ACPI_STATUS GetTempCallback(ACPI_HANDLE Object, UINT32 NestingLevel, void *Context, void **ReturnValue) {
    unsigned long long *max_temp = (unsigned long long*)Context;
    
    // Create a small stack buffer to hold the returned ACPI Object
    char local_buffer[128];
    ACPI_BUFFER ret_buf;
    ret_buf.Length = sizeof(local_buffer);
    ret_buf.Pointer = local_buffer;
    
    // Evaluate _TMP (Temperature) using the core API
    ACPI_STATUS status = AcpiEvaluateObject(Object, "_TMP", NULL, &ret_buf);
    
    if (ACPI_SUCCESS(status)) {
        ACPI_OBJECT *obj = (ACPI_OBJECT *)ret_buf.Pointer;
        
        // Ensure the object we got back is actually an integer
        if (obj && obj->Type == ACPI_TYPE_INTEGER) {
            unsigned long long temp = obj->Integer.Value;
            
            // Convert Kelvin to Celsius: C = (K - 273.2)
            if (temp > 2732) {
                unsigned long long temp_c = (temp - 2732) / 10;
                if (temp_c > *max_temp) {
                    *max_temp = temp_c;
                }
            }
        }
    }
    return AE_OK; // Keep searching other thermal zones
}

// Global hook to get the hottest zone
int acpi_get_system_temp() {
    unsigned long long max_temp = 0;
    
    // Search the entire motherboard namespace for Thermal Zones
    AcpiWalkNamespace(ACPI_TYPE_THERMAL, ACPI_ROOT_OBJECT, ACPI_UINT32_MAX, GetTempCallback, NULL, &max_temp, NULL);
    
    if (max_temp == 0) return 50; // Safe fallback if BIOS is missing thermal zones
    return (int)max_temp;
}

// ==========================================
// 3b. ACPICA'S OWN LOG — the channel that was never connected
// ==========================================
//
// These two were empty Rust stubs in `c_stubs.rs` carrying the comment "You can wire this to your
// print macro later to see logs!". Everything ACPICA tried to tell us went in the bin.
//
// That matters more here than it would elsewhere, because `AcpiLoadTables` REPORTS SUCCESS WHEN THE
// DSDT FAILS TO LOAD: `AcpiTbLoadNamespace` returns AE_CTRL_TERMINATE on any table failure and
// `AcpiLoadTables` converts that straight to AE_OK ("Don't let single failures abort the load").
// The count of failures exists only in an ACPI_ERROR message — i.e. only here. Discarding this
// output turns a loud, specific failure into a namespace that is silently empty.
//
// In C rather than Rust because they are variadic; ACPICA supplies both `vsnprintf` (utprint.c) and
// `va_list` (acgcc.h), so there is no libc dependency.
extern void nyx_acpi_log(const char *s);
/// One-byte CMOS breadcrumb, readable on the next boot. The only post-mortem field that survives
/// intact on this machine — see c_stubs.rs.
extern void nyx_mark(unsigned char v);

void ACPI_INTERNAL_VAR_XFACE AcpiOsPrintf(const char *Format, ...) {
    va_list args;
    va_start(args, Format);
    AcpiOsVprintf(Format, args);
    va_end(args);
}

// Is ACPICA logging live? Set for bring-up, cleared once the namespace is up. See below.
extern int nyx_acpi_log_enabled(void);

void AcpiOsVprintf(const char *Format, va_list Args) {
    // ⚠️ OFF outside bring-up, and the reason is not tidiness.
    //
    // ACPICA emits predefined-name warnings from deep inside the AML interpreter — e.g.
    // `AcpiNsCheckReference` (nspredef.c:471) when `_BCL`/`_BST` return something it did not
    // expect. Those fire while evaluating a method, which on Nyx happens inside a SYSCALL with
    // IF=0, on the syscall stack, under interpreter recursion. Adding a formatter with a stack
    // frame to that path crashed `panel` and `battery` outright — the warning path had been dead
    // for the life of the project because this function was an empty stub, and connecting it lit
    // it up in the worst possible context.
    //
    // Everything worth reading (table load, DSDT/SSDT results, bring-up errors) happens before
    // the namespace is up, so capturing exactly that window costs nothing and removes the hazard.
    // The runtime warnings are noise anyway: ACPICA suppresses repeats per node, and the ones this
    // firmware produces are known off-spec return types we already handle.
    if (!nyx_acpi_log_enabled()) return;

    // Stack, not the ACPICA heap: this runs during table load, and a logger that allocates cannot
    // report an allocation failure. Sized down from 512 — ACPICA's own messages are well under
    // this, and it sits on top of an already deep call chain.
    char buf[256];
    vsnprintf(buf, sizeof(buf), Format, Args);
    buf[sizeof(buf) - 1] = '\0';
    nyx_acpi_log(buf);
}

// What the table manager actually found. Text, appended to the same log.
//
// `AcpiLoadTables` returning AE_OK says nothing about whether the DSDT loaded, so this reports the
// facts it cannot lie about: how many tables were installed, which index the DSDT landed at, and
// every signature in the root list.
void acpi_log_tables(void) {
    char line[128];

    snprintf(line, sizeof(line), "  tables installed: %u (max %u), DSDT index %u\n",
             (unsigned)AcpiGbl_RootTableList.CurrentTableCount,
             (unsigned)AcpiGbl_RootTableList.MaxTableCount,
             (unsigned)AcpiGbl_DsdtIndex);
    nyx_acpi_log(line);

    for (UINT32 i = 0; i < AcpiGbl_RootTableList.CurrentTableCount; i++) {
        ACPI_TABLE_DESC *t = &AcpiGbl_RootTableList.Tables[i];
        snprintf(line, sizeof(line), "    [%2u] %.4s  addr %p  len %u  flags %#x %s\n",
                 (unsigned)i, t->Signature.Ascii, (void *)(uintptr_t)t->Address,
                 (unsigned)t->Length, (unsigned)t->Flags,
                 (t->Flags & ACPI_TABLE_IS_LOADED) ? "LOADED" : "");
        nyx_acpi_log(line);
    }
}

// ==========================================
// 3a2. THE NAMESPACE STEPPER — one node per call
// ==========================================
//
// `AcpiWalkNamespace` #GPs this kernel even at depth 1 with an inert callback, and a breadcrumb can
// only say "it died in the walk" — never WHICH node. That has cost a power cycle per guess.
//
// So the traversal is inverted: `AcpiGetNextObject` is the single-step primitive walks are built
// from, and this exposes exactly one step. Userspace calls it in a loop and PRINTS EACH NAME BEFORE
// asking for the next one, so when a node kills the machine its predecessor is already on screen.
// The crash names itself instead of being bisected.
//
// It also makes the namespace browsable for its own sake, which is worth having regardless of this
// bug — `acpi ls \_SB` should not require a kernel rebuild.
//
// `prev` = NULL starts at the first child of `parent`; pass back the returned handle to advance.
// Returns 1 on success, 0 when the scope is exhausted or the call failed.
int acpi_ns_step(void *parent, void *prev, void **next,
                 char *name_out, int name_max, int *type_out) {
    if (next) *next = NULL;
    if (name_out && name_max > 0) name_out[0] = '\0';
    if (type_out) *type_out = -1;

    ACPI_HANDLE p = parent ? (ACPI_HANDLE)parent : ACPI_ROOT_OBJECT;
    ACPI_HANDLE out = NULL;

    if (ACPI_FAILURE(AcpiGetNextObject(ACPI_TYPE_ANY, p, (ACPI_HANDLE)prev, &out)) || !out) {
        return 0;
    }
    if (next) *next = (void *)out;

    ACPI_OBJECT_TYPE t = ACPI_TYPE_ANY;
    if (ACPI_SUCCESS(AcpiGetType(out, &t)) && type_out) *type_out = (int)t;

    if (name_out && name_max > 1) {
        ACPI_BUFFER nb;
        nb.Length = ACPI_ALLOCATE_BUFFER;
        nb.Pointer = NULL;
        if (ACPI_SUCCESS(AcpiGetName(out, ACPI_FULL_PATHNAME, &nb)) && nb.Pointer) {
            const char *s = (const char *)nb.Pointer;
            int i = 0;
            while (s[i] && i < name_max - 1) { name_out[i] = s[i]; i++; }
            name_out[i] = '\0';
            AcpiOsFree(nb.Pointer);
        }
    }
    return 1;
}

/// Resolve an absolute path to a handle, so `acpi ls` can descend. 0 if not found.
void *acpi_ns_handle(const char *path) {
    ACPI_HANDLE h = NULL;
    if (!path || !path[0]) return NULL;
    if (ACPI_FAILURE(AcpiGetHandle(NULL, (char*)path, &h))) return NULL;
    return (void *)h;
}

// ==========================================
// 3b2. THE EMBEDDED CONTROLLER (PNP0C09)
// ==========================================
//
// Why this exists: `_STA`, `_BIF` and `_BST` on a laptop read through
// `OperationRegion (…, EmbeddedControl, …)`. ACPICA installs default address-space handlers for
// SystemMemory, SystemIO and PCI_Config **only** — there is none for EmbeddedControl, because the
// EC is an OS driver's job. Evaluating a battery method with no handler registered is what killed
// the kernel at `acpi probe 5`.
//
// ⚠️ PORTS ARE READ FROM `_CRS`, NOT HARDCODED. The usual pair is 0x62 (data) / 0x66 (command), but
// this machine's `ECRS` declares both IO ranges with `Range Minimum 0x0000` and patches them at
// runtime through the `_Y5A`/`_Y5B` name-fields, so the template as written in the DSDT contains
// zeros. Reading the *evaluated* `_CRS` is the only way to get the real values. Per the ACPI spec
// the first IO resource is the data port and the second is the command/status port.
//
// ⚠️ GOVERNOR CONTEXT ONLY (IF=1). Every transaction below polls with a timeout. Doing that from a
// syscall at IF=0 is the preemption-boundary wedge documented throughout this driver.

#define EC_OBF          0x01    /* status: output buffer full — data is ready to read */
#define EC_IBF          0x02    /* status: input buffer full  — EC has not consumed our write */
#define EC_CMD_READ     0x80
#define EC_CMD_WRITE    0x81

/* Bounded spin. The EC is slow (tens of microseconds) but must never hang the governor: a wedged
 * core takes scheduling down with it, which is exactly how this path failed before. */
#define EC_TIMEOUT      100000

static UINT32 nyx_ec_data_port = 0;
static UINT32 nyx_ec_cmd_port  = 0;
static int    nyx_ec_installed = 0;

/// Last raw EC status byte, kept so a timeout can say WHAT it saw rather than just "timed out".
/// 0xFF means nothing is decoding that port; 0x00 means the EC is idle but never raised OBF.
static UINT32 nyx_ec_last_status = 0xDEAD;

int acpi_ec_last_status(void) { return (int)nyx_ec_last_status; }

static UINT32 nyx_ec_status(void) {
    UINT32 v = 0;
    AcpiOsReadPort(nyx_ec_cmd_port, &v, 8);
    nyx_ec_last_status = v;
    return v;
}

/* Wait until the EC has consumed whatever we wrote. Returns 0 on timeout. */
static int nyx_ec_wait_ibf_clear(void) {
    for (int i = 0; i < EC_TIMEOUT; i++) {
        if (!(nyx_ec_status() & EC_IBF)) return 1;
        AcpiOsStall(1);
    }
    return 0;
}

/* Wait until the EC has produced a byte for us. Returns 0 on timeout. */
static int nyx_ec_wait_obf_set(void) {
    for (int i = 0; i < EC_TIMEOUT; i++) {
        if (nyx_ec_status() & EC_OBF) return 1;
        AcpiOsStall(1);
    }
    return 0;
}

static int nyx_ec_read_byte(UINT8 addr, UINT8 *out) {
    if (!nyx_ec_wait_ibf_clear()) return 0;
    AcpiOsWritePort(nyx_ec_cmd_port, EC_CMD_READ, 8);
    if (!nyx_ec_wait_ibf_clear()) return 0;
    AcpiOsWritePort(nyx_ec_data_port, addr, 8);
    if (!nyx_ec_wait_obf_set()) return 0;
    UINT32 v = 0;
    AcpiOsReadPort(nyx_ec_data_port, &v, 8);
    *out = (UINT8)(v & 0xFF);
    return 1;
}

static int nyx_ec_write_byte(UINT8 addr, UINT8 val) {
    if (!nyx_ec_wait_ibf_clear()) return 0;
    AcpiOsWritePort(nyx_ec_cmd_port, EC_CMD_WRITE, 8);
    if (!nyx_ec_wait_ibf_clear()) return 0;
    AcpiOsWritePort(nyx_ec_data_port, addr, 8);
    if (!nyx_ec_wait_ibf_clear()) return 0;
    AcpiOsWritePort(nyx_ec_data_port, val, 8);
    return 1;
}

/* The address-space handler ACPICA calls for every EmbeddedControl region access.
 *
 * EC space is byte-addressed, so a wider access is serviced as consecutive byte transactions —
 * that is what the spec requires and what the AML expects. */
static ACPI_STATUS NyxEcSpaceHandler(UINT32 Function, ACPI_PHYSICAL_ADDRESS Address,
                                     UINT32 BitWidth, UINT64 *Value,
                                     void *HandlerContext, void *RegionContext) {
    if (!Value || BitWidth == 0 || (BitWidth & 0x07)) return AE_BAD_PARAMETER;
    if (Address > 0xFF) return AE_BAD_ADDRESS;

    UINT32 bytes = BitWidth / 8;
    if (Address + bytes > 0x100) return AE_BAD_ADDRESS;

    if (Function == ACPI_READ) {
        UINT64 result = 0;
        for (UINT32 i = 0; i < bytes; i++) {
            UINT8 b = 0;
            if (!nyx_ec_read_byte((UINT8)(Address + i), &b)) return AE_TIME;
            result |= ((UINT64)b) << (i * 8);
        }
        *Value = result;
        return AE_OK;
    }

    if (Function == ACPI_WRITE) {
        for (UINT32 i = 0; i < bytes; i++) {
            UINT8 b = (UINT8)((*Value >> (i * 8)) & 0xFF);
            if (!nyx_ec_write_byte((UINT8)(Address + i), b)) return AE_TIME;
        }
        return AE_OK;
    }

    return AE_BAD_PARAMETER;
}

/* Pull the two IO ports out of the EVALUATED _CRS. First resource = data, second = command. */
static ACPI_STATUS NyxEcResourceCallback(ACPI_RESOURCE *Res, void *Context) {
    int *seen = (int *)Context;
    UINT32 base = 0;

    if (Res->Type == ACPI_RESOURCE_TYPE_IO) {
        base = Res->Data.Io.Minimum;
    } else if (Res->Type == ACPI_RESOURCE_TYPE_FIXED_IO) {
        base = Res->Data.FixedIo.Address;
    } else {
        return AE_OK;
    }

    if (*seen == 0)      nyx_ec_data_port = base;
    else if (*seen == 1) nyx_ec_cmd_port  = base;
    (*seen)++;
    return AE_OK;
}

static ACPI_STATUS NyxEcFindCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **Ret) {
    ACPI_HANDLE *out = (ACPI_HANDLE *)Context;
    if (!*out) *out = Object;
    return AE_OK;
}

/* Find the EC, learn its ports, and register the EmbeddedControl handler.
 *
 * Idempotent. Returns 1 once the handler is live, 0 if the EC could not be brought up — in which
 * case battery methods must NOT be evaluated, since that is the crash.
 *
 * Ports are reported back so the caller can put them on screen: if the values look wrong, that is
 * the first thing worth seeing, and this machine has no serial console. */
int acpi_ec_install(unsigned int *out_data, unsigned int *out_cmd) {
    if (out_data) *out_data = nyx_ec_data_port;
    if (out_cmd)  *out_cmd  = nyx_ec_cmd_port;
    if (nyx_ec_installed) return 1;

    // ★ DIRECT PATH, NOT AcpiGetDevices.
    //
    // Every crash in this subsystem has involved a namespace WALK — `AcpiWalkNamespace` in
    // count_nodes, `AcpiGetDevices("PNP0C0A")` in battery, and `AcpiGetDevices("PNP0C09")` here.
    // Every direct `AcpiGetHandle` + `AcpiEvaluateObject` has worked: the panel resolves and
    // `_BCL`/`_BQC` both return AE_OK. The breadcrumb pinned this one at mark 50, i.e. inside
    // install, before the ports were even known.
    //
    // So the walk machinery is the common factor and is avoided entirely. The path is not a guess:
    // `Device (ECDV)` sits in `Scope (_SB.PCI0.LPCB)` at dsdt.dsl:63644, `_HID` = PNP0C09,
    // `_STA` = 0x0F. NOTE the escaped backslash — see NYX_LCD_PATH for what a bare `\_` compiles to.
    // Breadcrumbs, one per call — the direct-path change did not move the failure off mark 50, and
    // this function makes three very different calls. 53/54/55 say which one.
    nyx_mark(53);
    ACPI_HANDLE ec = NULL;
    if (ACPI_FAILURE(AcpiGetHandle(NULL, (char*)"\\_SB.PCI0.LPCB.ECDV", &ec)) || !ec) return 0;

    nyx_mark(54);
    int seen = 0;
    if (ACPI_FAILURE(AcpiWalkResources(ec, (char*)"_CRS", NyxEcResourceCallback, &seen))) return 0;
    /* Both ports must be real. A zero here means the template was not patched, and poking port 0
     * would be a blind write to whatever lives there. */
    if (seen < 2 || nyx_ec_data_port == 0 || nyx_ec_cmd_port == 0) {
        nyx_ec_data_port = 0;
        nyx_ec_cmd_port = 0;
        return 0;
    }

    // ★ THE HANDLER INSTALL IS DELIBERATELY SKIPPED.
    //
    // `AcpiInstallAddressSpaceHandler` walks the device subtree internally (evhandler.c), and walks
    // #GP this kernel — proven at mark 55, and the walk bug survived every structural fix
    // (reader-lock bypass, NO_UNLOCK, a real OS layer, an inert callback). Registering the handler
    // is what lets *AML* reach the EC, i.e. what `_BIF`/`_BST` need. We do not need AML: the ports
    // are known and `nyx_ec_read_byte` talks to them directly.
    //
    // What that costs, stated plainly: `_BIF`/`_BST` give a vendor-neutral battery layout, and
    // reading EC registers directly does not — the offsets are model-specific and undocumented, so
    // whatever we hardcode is a map for THIS Dell and nothing else. That is a real downgrade, taken
    // knowingly because the correct path is blocked behind a bug that has already cost many boots.
    //
    // If the walk bug is ever fixed, restore the install and `acpi_battery_read` works unchanged.
    nyx_ec_installed = 1;
    if (out_data) *out_data = nyx_ec_data_port;
    if (out_cmd)  *out_cmd  = nyx_ec_cmd_port;
    return 1;
}

// Read `len` bytes from EC space starting at `offset`, straight off the ports.
//
// No ACPI, no AML, no namespace walk — which is the entire point. Returns bytes read.
//
// This is the raw material for finding the battery registers: dump it, then look for the values
// Fedora already reports for BAT0. Charge in mAh and voltage in mV are distinctive 16-bit
// little-endian patterns, and dumping twice a few minutes apart shows which bytes actually move.
/// Select which battery the EC's data window exposes. **Must precede every battery read.**
///
/// This is why a raw dump showed nothing: EC register 0x03 is a bank select, and without writing it
/// the battery window reads stale zeros. Straight out of the DSDT — both `ECG9` (behind `_BIF`) and
/// `ECG6` (behind `_BST`) open with `ECWB (0x03, Arg0)` before touching anything else.
static int nyx_ec_select_battery(UINT8 index) {
    return nyx_ec_write_byte(0x03, index);
}

static int nyx_ec_word(UINT8 addr, int *out) {
    UINT8 lo = 0, hi = 0;
    if (!nyx_ec_read_byte(addr, &lo)) return 0;
    if (!nyx_ec_read_byte((UINT8)(addr + 1), &hi)) return 0;
    *out = ((int)hi << 8) | lo;
    return 1;
}

/// Read the battery straight from EC registers, bypassing ACPI entirely.
///
/// The offsets are not guesses — they are exactly what this machine's own AML reads, decompiled
/// from the DSDT (`ECG6` at dsdt.dsl:65152 and `ECG9` at :65284):
///
///   0x03  W  battery select        0x1E  W  last-full capacity
///   0x10  B  state                 0x20  W  design capacity
///   0x12  W  present rate (signed) 0x22  W  design voltage
///   0x14  W  present voltage
///   0x16  W  remaining capacity
///
/// ⚠️ Still model-specific. `_BIF`/`_BST` would be vendor-neutral, but reaching them needs an
/// EmbeddedControl handler and installing one walks the namespace, which #GPs this kernel. This is
/// the same data by the same route the firmware uses — just without the interpreter in between.
///
/// Fills the 11-int layout `acpi::Battery` expects. Returns 1 if the battery reported present.
int acpi_ec_battery(int *out, int max) {
    if (!nyx_ec_installed || !out || max < 11) return 0;
    if (!nyx_ec_select_battery(1)) return 0;

    UINT8 state = 0;
    int rate = 0, volt = 0, remain = 0, lastfull = 0, design = 0, dvolt = 0;
    if (!nyx_ec_read_byte(0x10, &state)) return 0;
    nyx_ec_word(0x12, &rate);
    nyx_ec_word(0x14, &volt);
    nyx_ec_word(0x16, &remain);
    nyx_ec_word(0x1E, &lastfull);
    nyx_ec_word(0x20, &design);
    nyx_ec_word(0x22, &dvolt);

    // `_BST` treats bit 15 as a sign bit and negates — a discharging battery reports a negative
    // rate. We hand back the magnitude and let `state` say the direction, which is what the ACPI
    // layout means by "present rate".
    if (rate & 0x8000) rate = (0x10000 - rate) & 0xFFFF;

    // A battery that reports no capacity at all is not present, whatever the state byte says.
    int present = (lastfull > 0 || remain > 0) ? 1 : 0;

    out[0]  = present;
    out[1]  = 1;          // power unit: mAh/mA — Fedora reports this pack in uAh, so charge units
    out[2]  = design;
    out[3]  = lastfull;
    out[4]  = dvolt;
    out[5]  = state;
    out[6]  = rate;
    out[7]  = remain;
    out[8]  = volt;
    out[9]  = (design > 0 || lastfull > 0);   // have_bif equivalent
    out[10] = 1;                              // have_bst equivalent
    return present;
}

int acpi_ec_read_range(int offset, int len, unsigned char *out) {
    if (!nyx_ec_installed || !out || offset < 0 || len <= 0) return 0;
    if (offset + len > 0x100) len = 0x100 - offset;
    int n = 0;
    for (int i = 0; i < len; i++) {
        if (!nyx_ec_read_byte((UINT8)(offset + i), &out[i])) break;
        n++;
    }
    return n;
}

// ==========================================
// 3c. BATTERY — the ACPI control-method battery (PNP0C0A)
// ==========================================
//
// `_BIF` is static design data (design capacity, last-full capacity, voltage, model). `_BST` is the
// live state (charging/discharging, present rate, remaining capacity, voltage). Both hang off a
// device with HID PNP0C0A, found by `AcpiGetDevices` rather than a hardcoded path — same reasoning
// as the panel, and this time with no excuse, since the path differs across vendors far more.
//
// ⚠️ Units are not percent and not watts. `_BIF`/`_BST` report in mWh/mW **or** mAh/mA depending on
// the "Power Unit" field of `_BIF[0]`: 0 = mW/mWh, 1 = mA/mAh. Reported raw with the unit flag, so
// the caller can label them honestly instead of quietly showing milliamps as milliwatts.
//
// Percentage is remaining/last-full, NOT remaining/design — a worn battery that genuinely holds 60%
// of its original charge should read 100% when full, which is what every other OS shows.

typedef struct {
    int present;        // a PNP0C0A device exists and _STA says it is there
    int power_unit;     // 0 = mW/mWh, 1 = mA/mAh
    int design_cap;
    int last_full_cap;
    int design_voltage;
    int state;          // bit0 discharging, bit1 charging, bit2 critical
    int present_rate;
    int remaining_cap;
    int voltage;
    int have_bif;
    int have_bst;
} NyxBattery;

static int nyx_pkg_int(ACPI_OBJECT *pkg, UINT32 i, int *out) {
    if (!pkg || pkg->Type != ACPI_TYPE_PACKAGE || i >= pkg->Package.Count) return 0;
    ACPI_OBJECT *e = &pkg->Package.Elements[i];
    if (e->Type != ACPI_TYPE_INTEGER) return 0;
    *out = (int)(e->Integer.Value & 0x7FFFFFFF);
    return 1;
}

static ACPI_STATUS NyxBatteryCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **Ret) {
    NyxBattery *b = (NyxBattery *)Context;
    if (b->present) return AE_OK;          // first battery wins

    // _STA bit 4 (0x10) is "battery present". A bay with no battery still publishes the device.
    ACPI_BUFFER r; ACPI_OBJECT *o;
    UINT64 sta = 0;
    r.Length = ACPI_ALLOCATE_BUFFER; r.Pointer = NULL;
    if (ACPI_SUCCESS(AcpiEvaluateObject(Object, (char*)"_STA", NULL, &r)) && r.Pointer) {
        o = (ACPI_OBJECT *)r.Pointer;
        if (o->Type == ACPI_TYPE_INTEGER) sta = o->Integer.Value;
        AcpiOsFree(r.Pointer);
    }
    if (!(sta & 0x10)) return AE_OK;
    b->present = 1;

    r.Length = ACPI_ALLOCATE_BUFFER; r.Pointer = NULL;
    if (ACPI_SUCCESS(AcpiEvaluateObject(Object, (char*)"_BIF", NULL, &r)) && r.Pointer) {
        o = (ACPI_OBJECT *)r.Pointer;
        b->have_bif = nyx_pkg_int(o, 0, &b->power_unit)
                    & nyx_pkg_int(o, 1, &b->design_cap)
                    & nyx_pkg_int(o, 2, &b->last_full_cap)
                    & nyx_pkg_int(o, 4, &b->design_voltage);
        AcpiOsFree(r.Pointer);
    }

    r.Length = ACPI_ALLOCATE_BUFFER; r.Pointer = NULL;
    if (ACPI_SUCCESS(AcpiEvaluateObject(Object, (char*)"_BST", NULL, &r)) && r.Pointer) {
        o = (ACPI_OBJECT *)r.Pointer;
        b->have_bst = nyx_pkg_int(o, 0, &b->state)
                    & nyx_pkg_int(o, 1, &b->present_rate)
                    & nyx_pkg_int(o, 2, &b->remaining_cap)
                    & nyx_pkg_int(o, 3, &b->voltage);
        AcpiOsFree(r.Pointer);
    }
    return AE_OK;
}

// Fills 11 ints in the NyxBattery order above. Returns 1 if a present battery was found.
int acpi_battery_read(int *out, int max) {
    NyxBattery b;
    memset(&b, 0, sizeof(b));

    // ★ Direct path, same reasoning as the EC above: walks crash this kernel, direct handles do not.
    // `Device (BAT0)` is in `Scope (\_SB)` at dsdt.dsl:66042 with `_HID` = PNP0C0A. There is a BAT1
    // too, but this laptop has one bay and Fedora reports only BAT0.
    ACPI_HANDLE bat = NULL;
    if (ACPI_SUCCESS(AcpiGetHandle(NULL, (char*)"\\_SB.BAT0", &bat)) && bat) {
        NyxBatteryCallback(bat, 0, &b, NULL);
    }

    int vals[11] = { b.present, b.power_unit, b.design_cap, b.last_full_cap, b.design_voltage,
                     b.state, b.present_rate, b.remaining_cap, b.voltage, b.have_bif, b.have_bst };
    int n = (max < 11) ? max : 11;
    for (int i = 0; i < n; i++) out[i] = vals[i];
    return b.present;
}

// ==========================================
// 4. ACPICA OSL STUBS
// ==========================================

// Stub for AcpiOsEnterSleep to satisfy the linker without causing Rust naming collisions
ACPI_STATUS AcpiOsEnterSleep(UINT8 SleepState, UINT32 RegaValue, UINT32 RegbValue) {
    // TODO: Implement actual sleep logic for the kernel later
    return 0; // AE_OK
}
// ==========================================
// 5. PANEL BRIGHTNESS  (\_SB.PCI0.GFX0.LCD)
// ==========================================
//
// This laptop does NOT drive its backlight through the PCH PWM register — the enable bit is clear
// because firmware owns the panel. Its DSDT routes brightness through an SMI mailbox:
//
//   _BCM(level) -> GENS(0x09, {0x02, level}, 2) -> write cmd at SMBA (0x67EC3000), payload at +4,
//                  then outb(0xB2, 0xE0) to ring the SMI doorbell.
//
// Hand-rolling that mailbox was tried and the firmware ignored the request. Going through ACPICA
// instead is both safer and correct: it runs OSID()/_OSI first, builds the buffer the way the ASL
// says, and issues the SMI itself. This is the supported path — the same one every other OS uses —
// rather than a vendor poke guessed from outside, which is the distinction that matters here given
// what blind-firing an SMI cost this project once already.

// ★ The backslash here MUST be escaped. Written once as "\_SB..." — `\_` is not a valid C escape,
// so gcc warns and emits a bare `_`, turning this into a RELATIVE path. AcpiGetHandle rejects a
// relative path with a NULL parent (`nsxfname.c`: "Relative path with null prefix is disallowed")
// and returns AE_BAD_PARAMETER before it ever touches the namespace. The panel then reports "no LCD
// device" on a machine whose LCD device is present and fine.
#define NYX_LCD_PATH "\\_SB.PCI0.GFX0.LCD"

// Find the device carrying `_BCL`, wherever it lives.
//
// The fixed path is tried first because it is this machine's, and it is one lookup. The walk is the
// fallback: `\_SB.PCI0.GFX0.LCD` is an Intel-integrated-graphics convention, not a rule, and a
// hardcoded path that silently misses is precisely the failure mode above. Walking also means the
// diagnostic can print where the panel actually turned up rather than only that it did not.
static ACPI_STATUS NyxFindBclCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **Ret) {
    ACPI_HANDLE tmp;
    if (ACPI_SUCCESS(AcpiGetHandle(Object, (char*)"_BCL", &tmp))) {
        *((ACPI_HANDLE*)Context) = Object;
        return AE_CTRL_TERMINATE;   // first one wins; there is one panel
    }
    return AE_OK;
}

static ACPI_HANDLE nyx_lcd_handle(void) {
    ACPI_HANDLE h = NULL;
    if (ACPI_SUCCESS(AcpiGetHandle(NULL, (char*)NYX_LCD_PATH, &h)) && h) return h;

    // ACPI_TYPE_ANY, not ACPI_TYPE_DEVICE: the type filter is one more thing that can be wrong, and
    // the callback is a single child lookup either way. `acpi_wake_cnvi_wifi` walks ANY for the same
    // reason and is known to work on this machine.
    h = NULL;
    AcpiWalkNamespace(ACPI_TYPE_ANY, ACPI_ROOT_OBJECT, ACPI_UINT32_MAX,
                      NyxFindBclCallback, NULL, &h, NULL);
    return h;
}

// _BCL — the supported brightness levels. READ ONLY.
//
// Per the ACPI spec the first two entries are the levels used on AC and on battery; the rest are
// the selectable levels. Returned verbatim, first two included, because the caller is a diagnostic
// and reinterpreting the data is how you lose the evidence.
//
// Returns how many levels were written, or -1 if the method could not be evaluated.
int acpi_panel_levels(unsigned char *out, int max) {
    ACPI_HANDLE lcd = nyx_lcd_handle();
    if (!lcd || !out || max <= 0) return -1;

    ACPI_BUFFER ret;
    ret.Length = ACPI_ALLOCATE_BUFFER;
    ret.Pointer = NULL;
    if (ACPI_FAILURE(AcpiEvaluateObject(lcd, (char*)"_BCL", NULL, &ret)) || !ret.Pointer) return -1;

    ACPI_OBJECT *pkg = (ACPI_OBJECT *)ret.Pointer;
    int n = 0;
    if (pkg->Type == ACPI_TYPE_PACKAGE) {
        for (UINT32 i = 0; i < pkg->Package.Count && n < max; i++) {
            ACPI_OBJECT *e = &pkg->Package.Elements[i];
            if (e->Type == ACPI_TYPE_INTEGER) {
                out[n++] = (unsigned char)(e->Integer.Value & 0xFF);
            }
        }
    }
    AcpiOsFree(ret.Pointer);
    return n;
}

// _BQC — the current level. READ ONLY. Returns 0..100, or -1.
//
// ⚠️ On this machine `_BQC` is NOT a hardware read. The ASL is `Local0 = BRT0; Return (Local0)`,
// and `BRT0` is a plain Name initialised to 0x64 that only `_BCM` ever writes. So it reports 100
// until something sets the brightness this boot, no matter where the panel actually sits. That is
// the firmware's design, not a bug here — do not "fix" it by reading the PWM register instead,
// which on this laptop is disabled and reads back a value the panel is not obeying.
int acpi_panel_get(void) {
    ACPI_HANDLE lcd = nyx_lcd_handle();
    if (!lcd) return -1;

    ACPI_BUFFER ret;
    ret.Length = ACPI_ALLOCATE_BUFFER;
    ret.Pointer = NULL;
    if (ACPI_FAILURE(AcpiEvaluateObject(lcd, (char*)"_BQC", NULL, &ret)) || !ret.Pointer) return -1;

    ACPI_OBJECT *obj = (ACPI_OBJECT *)ret.Pointer;
    int v = (obj->Type == ACPI_TYPE_INTEGER) ? (int)(obj->Integer.Value & 0xFF) : -1;
    AcpiOsFree(ret.Pointer);
    return v;
}

// _BCM — set the level. THIS IS THE ONE THAT RINGS THE SMI DOORBELL.
//
// Clamped to 5..100 here as well as in the Rust caller. `_BCL` on this machine lists no level below
// 5, and a panel driven to invisible is indistinguishable from a hang on a box with no serial
// console — the recovery would be a blind power cycle. Belt and braces on purpose.
int acpi_panel_set(int level) {
    ACPI_HANDLE lcd = nyx_lcd_handle();
    if (!lcd) return 0;
    if (level < 5) level = 5;
    if (level > 100) level = 100;

    ACPI_OBJECT arg;
    arg.Type = ACPI_TYPE_INTEGER;
    arg.Integer.Value = (UINT64)level;

    ACPI_OBJECT_LIST args;
    args.Count = 1;
    args.Pointer = &arg;

    return ACPI_SUCCESS(AcpiEvaluateObject(lcd, (char*)"_BCM", &args, NULL)) ? 1 : 0;
}

// The read-only diagnostic. One call answers every question the last boot left open.
//
// `acpi_panel_levels` returning -1 conflated "no handle" with "the method failed", which cost a
// power cycle to disambiguate on a machine with no serial console. This reports them separately and
// carries the raw ACPI_STATUS out so the screen can name the actual error.
//
// Worth knowing what a failure here is likely to mean: both `_BCL` and `_BCM` open by calling
// `OIDE()`, which the DSDT declares External — it is defined in a graphics SSDT, not the DSDT. If
// that table is absent or unloaded the call is unresolved and evaluation returns AE_NOT_FOUND
// (0x05) with a perfectly good handle in hand. That is a different problem from a missing device
// and wants a different fix, so the two must not look alike from the outside.
//
// Does this absolute path resolve? Returns the raw ACPI_STATUS (0 = AE_OK).
//
// Used to walk a ladder — `\_SB`, `\_SB.PCI0`, `\_SB.PCI0.GFX0`, `\_SB.PCI0.GFX0.LCD` — so the
// screen can say exactly where the live namespace stops matching the DSDT we decompiled. Guessing
// which rung broke costs a power cycle each time; asking costs nothing.
unsigned int acpi_path_status(const char *path) {
    ACPI_HANDLE h = NULL;
    return (unsigned int)AcpiGetHandle(NULL, (char*)path, &h);
}

// Count what the walk can actually see.
//
// Distinguishes "the namespace has no panel" from "the walk visits nothing", which look identical
// from the outside and are completely different bugs.
typedef struct { int nodes; int devices; int with_bcl; } NyxNsCount;

// ★ CALLBACK STRIPPED TO A COUNTER — this is a bisect, not the final shape.
//
// The OS layer has been implemented for real (counting semaphores, exclusive locks, per-task thread
// ids) and `acpi probe 1` STILL dies. So the remaining question is binary: is the walk machinery
// itself broken, or was it my callback re-entering ACPICA twice per node?
//
// It used to call `AcpiGetType` and `AcpiGetHandle(Object, "_BCL")` on every node in the namespace —
// thousands of nested ACPICA calls from inside a walk that has already released and re-acquired the
// namespace mutex. That is a very different thing from traversing the tree.
//
// Now it touches nothing but a counter. The breadcrumbs say which:
//   60 = about to enter AcpiWalkNamespace with an inert callback
//   61 = the walk RETURNED  -> traversal is fine; the fault was the re-entrant calls above
//   dies at 60             -> AcpiNsWalkNamespace itself is broken, independent of any callback
static ACPI_STATUS NyxCountCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **Ret) {
    NyxNsCount *c = (NyxNsCount *)Context;
    c->nodes++;
    return AE_OK;
}

// `depth` bounds the descent, so the failure can be bisected from userspace WITHOUT a rebuild.
//
// A direct `AcpiGetHandle` follows one known path and works; the walk descends into every child of
// every node. If the namespace has a bad link somewhere, only the walk reaches it — so the useful
// question is no longer "does it crash" but "at what depth". `acpi probe 1 <depth>` answers that
// one power cycle at a time, and each answer is a fact rather than a guess.
//
// The breadcrumb encodes the depth: mark 60+depth on entry, 80+depth on return. So "died at 63"
// means depth 3 was fatal and depth 2 was survivable.
void acpi_count_nodes(int depth, int *nodes, int *devices, int *with_bcl) {
    NyxNsCount c = { 0, 0, 0 };
    UINT32 d = (depth <= 0) ? ACPI_UINT32_MAX : (UINT32)depth;
    nyx_mark((unsigned char)(60 + (depth > 0 && depth < 16 ? depth : 0)));

    // ★ AcpiNsWalkNamespace DIRECTLY — deliberately skipping AcpiWalkNamespace's reader lock.
    //
    // `acpi ls` enumerated 512 root children with no crash, so traversal is sound. It steps with
    // `AcpiGetNextObject`, which takes ONLY ACPI_MTX_NAMESPACE (nsxfobj.c). `AcpiWalkNamespace`
    // additionally takes `AcpiUtAcquireReadLock(&AcpiGbl_NamespaceRwLock)` and holds it for the
    // whole walk — and that is the single thing walks do that no working call does.
    //
    // So: take the namespace mutex the same way the stepper does, and call the internal walker
    // without the RW lock. If this survives, the reader lock is the bug and every walk-based call
    // (AcpiGetDevices, AcpiInstallAddressSpaceHandler -> EC -> battery) unblocks.
    // ★ ACPI_NS_WALK_NO_UNLOCK — the last structural difference from the stepper.
    //
    // The reader-lock bypass did not fix it, so AcpiGbl_NamespaceRwLock is cleared. What remains is
    // this: with ACPI_NS_WALK_UNLOCK the walker RELEASES and RE-ACQUIRES ACPI_MTX_NAMESPACE around
    // every callback while holding a cursor into the tree across the gap. `AcpiGetNextObject` — which
    // enumerates 512 root nodes without a stumble — never does that; it takes the mutex, moves one
    // node, and releases.
    //
    // NO_UNLOCK holds the mutex for the whole walk, so the cursor is never exposed across a gap.
    // Safe here because the callback is inert and executes no AML (AML would need the mutex).
    //
    // Survives -> the release/re-acquire is the bug, and the fix is in the ACPI_MTX_NAMESPACE
    //             bookkeeping rather than anywhere near the namespace data.
    // Still dies -> the walker's own cursor (AcpiNsGetNextNodeTyped) is at fault, and the answer is
    //             to stop using AcpiNsWalkNamespace entirely and build what we need on the stepper.
    if (ACPI_SUCCESS(AcpiUtAcquireMutex(ACPI_MTX_NAMESPACE))) {
        AcpiNsWalkNamespace(ACPI_TYPE_ANY, ACPI_ROOT_OBJECT, d, ACPI_NS_WALK_NO_UNLOCK,
                            NyxCountCallback, NULL, &c, NULL);
        (void) AcpiUtReleaseMutex(ACPI_MTX_NAMESPACE);
    }
    nyx_mark((unsigned char)(80 + (depth > 0 && depth < 16 ? depth : 0)));
    if (nodes) *nodes = c.nodes;
    if (devices) *devices = c.devices;
    if (with_bcl) *with_bcl = c.with_bcl;
}

// Returns 1 if a panel handle was found at all, 0 if not.
int acpi_panel_diag(char *path_out, int path_max, unsigned int *bcl_st, unsigned int *bqc_st) {
    if (bcl_st) *bcl_st = AE_NOT_FOUND;
    if (bqc_st) *bqc_st = AE_NOT_FOUND;
    if (path_out && path_max > 0) path_out[0] = '\0';

    ACPI_HANDLE lcd = nyx_lcd_handle();
    if (!lcd) return 0;

    if (path_out && path_max > 1) {
        ACPI_BUFFER nb;
        nb.Length = ACPI_ALLOCATE_BUFFER;
        nb.Pointer = NULL;
        if (ACPI_SUCCESS(AcpiGetName(lcd, ACPI_FULL_PATHNAME, &nb)) && nb.Pointer) {
            int i = 0;
            const char *s = (const char *)nb.Pointer;
            while (s[i] && i < path_max - 1) { path_out[i] = s[i]; i++; }
            path_out[i] = '\0';
            AcpiOsFree(nb.Pointer);
        }
    }

    ACPI_BUFFER ret;
    ret.Length = ACPI_ALLOCATE_BUFFER;
    ret.Pointer = NULL;
    ACPI_STATUS s = AcpiEvaluateObject(lcd, (char*)"_BCL", NULL, &ret);
    if (bcl_st) *bcl_st = (unsigned int)s;
    if (ret.Pointer) AcpiOsFree(ret.Pointer);

    ret.Length = ACPI_ALLOCATE_BUFFER;
    ret.Pointer = NULL;
    s = AcpiEvaluateObject(lcd, (char*)"_BQC", NULL, &ret);
    if (bqc_st) *bqc_st = (unsigned int)s;
    if (ret.Pointer) AcpiOsFree(ret.Pointer);

    return 1;
}
