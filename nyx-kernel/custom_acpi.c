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
// An I2C-HID touchpad cannot be described statically. This DSDT declares the SAME device slot on
// four different I2C buses and patches its _HID and slave address at _INI from an NVS variable
// (SDS0) — the one slot becomes WCOM4831@0x0A, ALPS0000@0x2C, ELAN2097@0x10, NTRG0001@0x07,
// SYNA2393 or DLL077A depending on which panel the factory fitted, and _STA says which is real.
// Worse, `_CRS` here is a Method whose result depends on OSYS and SDM0, so even the resource
// template cannot be read statically. Every number a driver needs must come from evaluated AML.
//
// ⚠️ AML EVALUATION CONTEXT. Nothing here may be called from a syscall: SYSCALL runs with IF=0 and
// AcpiEvaluateObject takes the interpreter mutex, allocates, and can end in a firmware SMI — that
// is the preemption-boundary deadlock that wedged the machine on `panel` and `battery`
// (see the cache note in acpi.rs). Call this from the thermal governor (IF=1) via an `acpi probe`
// step, and have syscalls copy scalars out of the published cache.

// Mixed-endian encoding of 3cdff6f7-4267-4555-ad05-b30a3d8938de, the standard "HID I2C Device"
// _DSM UUID (the DSDT names it HIDG). Function 1 returns the HID descriptor register address.
static const UINT8 NyxHidI2cDsmUuid[16] = {
    0xF7, 0xF6, 0xDF, 0x3C, 0x67, 0x42, 0x55, 0x45,
    0xAD, 0x05, 0xB3, 0x0A, 0x3D, 0x89, 0x38, 0xDE
};

typedef struct {
    UINT32 valid;
    UINT32 sta;
    UINT32 slave_addr;
    UINT32 speed_hz;
    UINT32 gpio_pin;
    UINT32 hid_desc_reg;
    UINT32 ctrl_adr;      /* controller _ADR: (device << 16) | function */
    char   path[72];
    char   ctrl_path[72];
} NyxI2cHidInfo;

typedef struct {
    NyxI2cHidInfo *out;
    int            max;
    int            count;
} NyxHidScan;

static ACPI_STATUS NyxHidResourceCb(ACPI_RESOURCE *Resource, void *Context) {
    NyxI2cHidInfo *info = (NyxI2cHidInfo *)Context;

    if (Resource->Type == ACPI_RESOURCE_TYPE_SERIAL_BUS) {
        if (Resource->Data.CommonSerialBus.Type == ACPI_RESOURCE_SERIAL_TYPE_I2C) {
            ACPI_RESOURCE_I2C_SERIALBUS *i2c = &Resource->Data.I2cSerialBus;
            info->slave_addr = i2c->SlaveAddress;
            info->speed_hz   = i2c->ConnectionSpeed;
            /* Which controller this hangs off, e.g. "\\_SB.PCI0.I2C1" — resolved to a PCI
             * bus/device/function below via that controller's _ADR. */
            if (i2c->ResourceSource.StringPtr) {
                int i = 0;
                for (; i < (int)sizeof(info->ctrl_path) - 1
                       && i2c->ResourceSource.StringPtr[i]; i++) {
                    info->ctrl_path[i] = i2c->ResourceSource.StringPtr[i];
                }
                info->ctrl_path[i] = 0;
            }
        }
    } else if (Resource->Type == ACPI_RESOURCE_TYPE_GPIO) {
        ACPI_RESOURCE_GPIO *gpio = &Resource->Data.Gpio;
        /* Only the INTERRUPT-type GPIO is the "report ready" line. A GpioIo entry here would be a
         * reset or power-enable pin, which is a different thing and must not be mistaken for it. */
        if (gpio->ConnectionType == ACPI_RESOURCE_GPIO_TYPE_INT
            && gpio->PinTableLength > 0 && gpio->PinTable) {
            info->gpio_pin = gpio->PinTable[0];
        }
    }
    return AE_OK;
}

/* Evaluate an object expecting an Integer. Fixed stack buffer: the result is small and bounded, so
 * there is no reason to take the ACPI_ALLOCATE_BUFFER path and own a free. Returns 1 on success. */
static int NyxEvalInteger(ACPI_HANDLE Object, const char *Name, UINT64 *out) {
    char local[96];
    ACPI_BUFFER buf;
    buf.Length = sizeof(local);
    buf.Pointer = local;

    if (ACPI_FAILURE(AcpiEvaluateObject(Object, (char *)Name, NULL, &buf))) {
        return 0;
    }
    ACPI_OBJECT *obj = (ACPI_OBJECT *)buf.Pointer;
    if (!obj || obj->Type != ACPI_TYPE_INTEGER) {
        return 0;
    }
    *out = obj->Integer.Value;
    return 1;
}

/* _DSM(HIDG, rev 1, func 1) -> the register at which the HID descriptor is read over I2C. */
static UINT32 NyxHidDescriptorRegister(ACPI_HANDLE Object) {
    ACPI_OBJECT args[4];
    ACPI_OBJECT_LIST arglist;
    char local[128];
    ACPI_BUFFER buf;

    args[0].Type = ACPI_TYPE_BUFFER;
    args[0].Buffer.Length = 16;
    args[0].Buffer.Pointer = (UINT8 *)NyxHidI2cDsmUuid;
    args[1].Type = ACPI_TYPE_INTEGER;
    args[1].Integer.Value = 1;           /* revision */
    args[2].Type = ACPI_TYPE_INTEGER;
    args[2].Integer.Value = 1;           /* function 1: descriptor address */
    args[3].Type = ACPI_TYPE_PACKAGE;    /* conventionally an empty package */
    args[3].Package.Count = 0;
    args[3].Package.Elements = NULL;

    arglist.Count = 4;
    arglist.Pointer = args;

    buf.Length = sizeof(local);
    buf.Pointer = local;

    if (ACPI_FAILURE(AcpiEvaluateObject(Object, (char *)"_DSM", &arglist, &buf))) {
        return 0;
    }
    ACPI_OBJECT *obj = (ACPI_OBJECT *)buf.Pointer;
    if (!obj || obj->Type != ACPI_TYPE_INTEGER) {
        return 0;
    }
    return (UINT32)obj->Integer.Value;
}

static ACPI_STATUS I2cHidCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **ReturnValue) {
    NyxHidScan *scan = (NyxHidScan *)Context;
    if (scan->count >= scan->max) {
        return AE_OK;
    }

    NyxI2cHidInfo *info = &scan->out[scan->count];
    UINT64 sta = 0;

    /* A device with no _STA is present by definition; one WITH _STA must have bit 0 set. Three of
     * the four touch devices this firmware declares are templates for boards this is not, and
     * _STA is the only thing that distinguishes them. */
    if (NyxEvalInteger(Object, "_STA", &sta)) {
        info->sta = (UINT32)sta;
        if ((sta & 0x01) == 0) {
            return AE_OK;
        }
    } else {
        info->sta = 0x0F;
    }

    /* Fixed buffer, not ACPI_ALLOCATE_BUFFER — a path is bounded and this avoids owning a free. */
    ACPI_BUFFER namebuf;
    namebuf.Length = sizeof(info->path);
    namebuf.Pointer = info->path;
    if (ACPI_FAILURE(AcpiGetName(Object, ACPI_FULL_PATHNAME, &namebuf))) {
        info->path[0] = 0;
    }

    nyx_mark(70);
    AcpiWalkResources(Object, (char *)"_CRS", NyxHidResourceCb, info);
    nyx_mark(71);
    info->hid_desc_reg = NyxHidDescriptorRegister(Object);
    nyx_mark(72);

    /* Resolve the controller path to a PCI address. _ADR on an LPSS controller encodes
     * (device << 16) | function, so 0x00150001 is 00:15.1. */
    if (info->ctrl_path[0]) {
        ACPI_HANDLE ctrl;
        if (ACPI_SUCCESS(AcpiGetHandle(NULL, info->ctrl_path, &ctrl))) {
            UINT64 adr = 0;
            if (NyxEvalInteger(ctrl, "_ADR", &adr)) {
                info->ctrl_adr = (UINT32)adr;
            }
        }
    }

    /* Only claim the entry if it is usable. Slave address 0 means the _CRS walk found no I2C
     * descriptor, which makes every other field meaningless — publishing it would aim the driver
     * at an address that does not exist. */
    if (info->slave_addr != 0) {
        info->valid = 1;
        scan->count += 1;
    }
    return AE_OK;
}

int acpi_find_i2c_hid(void) {
    NyxI2cHidInfo scratch[8];
    NyxHidScan scan;
    for (int i = 0; i < 8; i++) {
        NyxI2cHidInfo zero = {0};
        scratch[i] = zero;
    }
    scan.out = scratch;
    scan.max = 8;
    scan.count = 0;

    /* AcpiGetDevices matches the _CID list as well as _HID, which is essential here: these devices
     * carry a vendor-specific _HID patched in at _INI and only PNP0C50 as their _CID. A _HID-only
     * matcher would find nothing. */
    AcpiGetDevices((char *)"PNP0C50", I2cHidCallback, &scan, NULL);
    return scan.count;
}

/* Fill `out` with up to `max` present I2C-HID devices. Returns how many were written. */
int acpi_get_i2c_hid(NyxI2cHidInfo *out, int max) {
    NyxHidScan scan;
    for (int i = 0; i < max; i++) {
        NyxI2cHidInfo zero = {0};
        out[i] = zero;
    }
    scan.out = out;
    scan.max = max;
    scan.count = 0;

    AcpiGetDevices((char *)"PNP0C50", I2cHidCallback, &scan, NULL);
    return scan.count;
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
// ★★ `parent_out` and `name4_out` are the whole diagnostic now, and they are why this function grew.
//
// `acpi ls` reported 512 distinct handles off the ROOT with no repeat — but 512 was its own loop cap,
// not a count, and a real ACPI root has ten to twenty children. So the sibling chain does not
// terminate, and the question is where it stops being a real chain.
//
// Two facts answer that, and neither can be got from the full pathname:
//
//   `Parent` — every genuine sibling shares one. The first node whose parent differs from the first
//              node's parent is the exact index where the walk left the child list. No crash needed.
//   `Name`   — the RAW four-byte field, one load. `AcpiGetName(ACPI_FULL_PATHNAME)` cannot be trusted
//              here: it walks `Parent` upward to build the path, so on a runaway node it either
//              fails or invents a plausible string out of garbage. The raw word cannot lie.
int acpi_ns_step(void *parent, void *prev, void **next,
                 char *name_out, int name_max, int *type_out,
                 void **parent_out, unsigned int *name4_out) {
    if (next) *next = NULL;
    if (name_out && name_max > 0) name_out[0] = '\0';
    if (type_out) *type_out = -1;
    if (parent_out) *parent_out = NULL;
    if (name4_out) *name4_out = 0;

    ACPI_HANDLE p = parent ? (ACPI_HANDLE)parent : ACPI_ROOT_OBJECT;
    ACPI_HANDLE out = NULL;

    if (ACPI_FAILURE(AcpiGetNextObject(ACPI_TYPE_ANY, p, (ACPI_HANDLE)prev, &out)) || !out) {
        return 0;
    }
    if (next) *next = (void *)out;

    {
        ACPI_NAMESPACE_NODE *n = (ACPI_NAMESPACE_NODE *)out;
        if (parent_out) *parent_out = (void *)n->Parent;
        if (name4_out) *name4_out = n->Name.Integer;
    }

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

/* Bounded spin, in iterations of `AcpiOsStall(1)`. The EC is slow (tens of microseconds) but must
 * never hang the governor: a wedged core takes scheduling down with it, which is exactly how this
 * path failed before.
 *
 * ★ 100000 -> 10000, alongside the `AcpiOsGetTimer` fix, and the two go together. This was written
 * when `AcpiOsStall(1)` actually waited ~0.5 us, so the loop was a ~50 ms budget that nobody had
 * costed; with the timer corrected each iteration is a real microsecond and the SAME constant would
 * silently become 100 ms per wait. `nyx_ec_read_byte` performs three waits, so a single failing
 * register read would spin for 300 ms — on the 1 Hz governor tick, and 300x over on a transaction
 * the ACPI spec and Linux both budget in single-digit milliseconds.
 *
 * 10 ms is still generous for hardware that answers in tens of microseconds, and it is now an
 * honest number: iterations x 1 us. ⚠️ An iteration count is not a timeout unless the per-iteration
 * delay is real — this one was not, for the life of the file. */
#define EC_TIMEOUT      10000

static UINT32 nyx_ec_data_port = 0;
static UINT32 nyx_ec_cmd_port  = 0;
static int    nyx_ec_installed = 0;
/* Whether the EmbeddedControl address-space handler registered, and what ACPICA said. Separate from
 * `nyx_ec_installed`, which only means "the ports are known": the direct-port battery works without
 * a handler, AML does not. Two different claims, and they were conflated. */
static int    nyx_ec_handler_ok = 0;
static ACPI_STATUS nyx_ec_handler_status = AE_NOT_EXIST;
/* `_REG` is run as its own step; AE_NOT_EXIST means it has not been attempted this boot. */
static ACPI_STATUS nyx_ec_reg_status = AE_NOT_EXIST;
/* Has `_REG(3, 1)` actually been evaluated? NOT the same question as "is a handler attached", and on
 * this firmware it is the one that decides whether AML uses the EC at all. See acpi_ec_run_reg. */
static int    nyx_ec_reg_done = 0;
/* Stack-depth witnesses. See acpi_stack_probe / acpi_battery_read.
 *
 * ⚠️ `volatile` on purpose. The point of `nyx_out_ptr_entry` vs `nyx_out_ptr` is to catch the caller's
 * own parameter changing across a call — which is undefined-behaviour territory the optimiser is
 * entitled to assume cannot happen, and would happily fold the comparison to a constant false. */
static volatile unsigned long long nyx_rsp_at_entry = 0;
static volatile unsigned long long nyx_rsp_at_unpack = 0;
static volatile unsigned long long nyx_rsp_min_handler = 0;
static volatile unsigned long long nyx_out_ptr_entry = 0;
static volatile unsigned long long nyx_out_ptr = 0;
/* Where a battery read lands its result, so nothing has to be written through a caller's pointer
 * while an AML interpreter frame is still unwinding beneath us. See acpi_battery_fetch. */
static volatile int nyx_batt_vals[11];
static volatile int nyx_batt_present = 0;

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
    /* ★ The deepest point we ever get to observe. AML calls this from the bottom of the interpreter's
     * recursion, so the low-water mark here bounds how much stack `_BIF`/`_BST` actually consume —
     * the number needed to confirm or kill the overflow theory, rather than reasoning about it. */
    {
        unsigned long long rsp;
        __asm__ volatile ("mov %%rsp, %0" : "=r"(rsp));
        if (nyx_rsp_min_handler == 0 || rsp < nyx_rsp_min_handler) nyx_rsp_min_handler = rsp;
    }

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

    // ★★★ THE HANDLER INSTALL IS BACK (2026-09-09). THE WALK BUG IS FIXED.
    //
    // This was skipped for the life of the project on the grounds that
    // `AcpiInstallAddressSpaceHandler` walks the device subtree internally (evhandler.c) and "walks
    // #GP this kernel". That was true, and it was never an ACPI bug: the kernel heap was backed by
    // physical memory below 1 MB and AP bring-up wrote the SMP trampoline through it. See
    // `memory::LOW_MEM_RESERVED`. Namespace walks are sound now.
    //
    // Registering this is what lets AML reach the EC, which is what `_BIF`/`_BST` need. The raw-port
    // path stays as the fallback and is still what the Entity reads today — it is hardware-verified
    // against Fedora, and replacing something that works with something newly re-enabled, on a
    // machine that costs a power cycle per test, is not a trade worth making blind.
    //
    // What the AML path buys if it holds: `_BIF`/`_BST` are a vendor-neutral battery layout, where
    // the raw offsets are a map for THIS Dell and nothing else.
    //
    // ⚠️ A failure here is NOT fatal and must not be. `nyx_ec_installed` is still set either way, so
    // the direct-port battery keeps working exactly as before; only the AML route is lost. The
    // status is recorded rather than discarded so `acpi ec` can report which of the two is live —
    // "the EC works" has meant two different things in this project and they should be separable.
    // ⚠️⚠️ NOT INSTALLED HERE. See `acpi_ec_install_handler` below.
    //
    // It was, for exactly one build, and the machine kernel-panicked the instant it reached ring 3.
    // This function runs on the thermal governor's tick — once a second, starting as soon as
    // userspace does — so putting a newly re-enabled, unproven ACPI call in it meant the very first
    // tick took the machine down, on a box with no serial console and no way to opt out without a
    // rebuild. The install is now a deliberate, one-shot request.
    nyx_ec_installed = 1;
    if (out_data) *out_data = nyx_ec_data_port;
    if (out_cmd)  *out_cmd  = nyx_ec_cmd_port;
    return 1;
}

/* ★★ Register the EmbeddedControl address-space handler. OPT-IN, one shot, never automatic.
 *
 * This is what lets AML reach the EC, i.e. what `_BIF`/`_BST` need. It was blocked for the life of
 * the project because the install walks the device subtree and walks #GP'd this kernel — which was
 * never an ACPI bug, but the sub-1 MB heap corruption (see `memory::LOW_MEM_RESERVED`).
 *
 * ⚠️ With walks fixed it STILL panicked the machine, immediately on reaching ring 3. So the walk was
 * not the only thing wrong with this path, and the remaining suspect is what the install does after
 * walking: `AcpiEvExecuteRegMethods` evaluates **`_REG`** for every EmbeddedControl region. That is
 * real AML telling the firmware "the OS owns the EC now", and on this class of laptop it does
 * substantial work — and every EC access it makes is routed straight back into `NyxEcSpaceHandler`,
 * whose poll loop leans on `AcpiOsStall`, which is known to be wrong by a factor of ~250 (it treats
 * a raw TSC as 100 ns units). Re-entering our own half-finished handler from inside its own
 * installation is a plausible way to hang or fault.
 *
 * So: deliberate only, `acpi probe 9`, with breadcrumbs on both sides. The machine boots either way,
 * and the raw-port battery — which is hardware-verified — is untouched by this succeeding or failing.
 */
int acpi_ec_install_handler(void) {
    if (nyx_ec_handler_ok) return 1;
    if (!nyx_ec_installed) return 0;

    ACPI_HANDLE ec = NULL;
    if (ACPI_FAILURE(AcpiGetHandle(NULL, (char*)"\\_SB.PCI0.LPCB.ECDV", &ec)) || !ec) return 0;

    /* ★★ SPLIT IN TWO, because the first breadcrumb pair could not tell them apart.
     *
     * The boot said mark 55 — died inside `AcpiInstallAddressSpaceHandler` — and I had claimed that
     * meant "the install itself, not `_REG`". Wrong: `_REG` runs INSIDE that call.
     * `AcpiInstallAddressSpaceHandlerInternal` attaches the handler and then, if Run_Reg, calls
     * `AcpiEvExecuteRegMethods` before returning. So one pair of marks around the whole thing
     * covered both candidates and distinguished nothing.
     *
     * ACPICA supports exactly this separation, and its own header recommends it:
     *
     *   "To avoid this problem pass FALSE for Run_Reg and later on call AcpiExecuteRegMethods()"
     *
     * So: attach the handler (55 -> 56), then run _REG as its own step (57 -> 58).
     *
     *   dies at 55 -> attaching the handler is fatal: the internal walk, or the region setup
     *   dies at 57 -> `_REG` is fatal: real AML, which now re-enters NyxEcSpaceHandler
     *
     * ★ And this may be more than a bisect. `_REG` failing is survivable — it tells the firmware the
     * OS owns the EC, and plenty of AML reads work without it — so if 55->56 completes we have a
     * registered handler even if _REG has to stay off. */
    nyx_mark(55);
    nyx_ec_handler_status = AcpiInstallAddressSpaceHandlerNo_Reg(
        ec, ACPI_ADR_SPACE_EC, NyxEcSpaceHandler, NULL, NULL);
    nyx_mark(56);
    nyx_ec_handler_ok = ACPI_SUCCESS(nyx_ec_handler_status) ? 1 : 0;
    if (!nyx_ec_handler_ok) return 0;

    /* Separate probe step, so a fatal _REG does not cost the handler as well. */
    return 1;
}

/* Run `_REG` for the EC address space — the second half of a normal handler install, split out.
 *
 * Its own probe step (`acpi probe 10`) so that a fatal `_REG` does not also cost the handler
 * registration, and so the two can be attributed separately. This is real AML: it tells the firmware
 * the OS now owns the EC, and every EC access it makes routes back into `NyxEcSpaceHandler`.
 *
 * ★★★ NOT OPTIONAL ON THIS MACHINE, AND THE DSDT SAYS SO OUTRIGHT (2026-09-09).
 *
 * I had written that "`_REG` is survivable to skip — plenty of AML reads work without it". That is
 * true in general and **false for this firmware**. Decompiled, `_STA` on BAT0 reaches the EC like so:
 *
 *     _STA -> ECG5() -> ECRB(0x06) -> \_SB.PCI0.LPCB.ECDV.ECR1(0x06)
 *
 *     Method (ECR1, 1, Serialized) {
 *         If ((ECRD == Zero)) { Return (EISC (0x80, Arg0, Zero)) }   // SMI mailbox
 *         ... Local0 = EC00 / EC01 / ... / EC06 ...                  // the EC region
 *     }
 *
 * `Name (ECRD, Zero)` — it defaults to zero, and the ONLY assignment of `ECRD = One` anywhere in the
 * DSDT is inside `Method (_REG, 2)` on the EC device. So until `_REG(3, 1)` runs, `ECRD` is zero and
 * **every** EC access takes the other branch: `EISC` -> `GENS(0x08, ...)` -> `SMBF`, which builds a
 * dynamic `OperationRegion (WWPR, SystemMemory, SMBA + 4, 4)` from an NVS-supplied base and then
 * fires an SMI via `ASMI()`. That is a different mechanism entirely, and a bad `SMBA` there
 * dereferences a wild physical address — which is the non-canonical #GP(0) that `acpi probe 5` dies
 * with, at breadcrumb 61, inside the very first method.
 *
 * ⚠️ So installing the handler WITHOUT `_REG` does not merely leave the firmware uninformed — it
 * leaves `NyxEcSpaceHandler` **never consulted at all**, while AML quietly takes an SMI path we have
 * no business on. Attaching alone is not a safe half-measure here; it is a trap that looks like one. */
int acpi_ec_run_reg(void) {
    if (!nyx_ec_handler_ok) return 0;
    ACPI_HANDLE ec = NULL;
    if (ACPI_FAILURE(AcpiGetHandle(NULL, (char*)"\\_SB.PCI0.LPCB.ECDV", &ec)) || !ec) return 0;
    nyx_mark(57);
    ACPI_STATUS st = AcpiExecuteRegMethods(ec, ACPI_ADR_SPACE_EC);
    nyx_mark(58);
    nyx_ec_reg_status = st;
    nyx_ec_reg_done = ACPI_SUCCESS(st) ? 1 : 0;
    return nyx_ec_reg_done;
}

/* Did `_REG` run, and what did it return? Separate from acpi_ec_handler_state on purpose: on this
 * firmware the handler being attached says nothing about whether AML will use it (see above). */
int acpi_ec_reg_state(unsigned int *status_out) {
    if (status_out) *status_out = (unsigned int)nyx_ec_reg_status;
    return nyx_ec_reg_done;
}

/* ★★★ Read `\ECRD` straight out of the namespace.
 *
 * GROUND TRUTH, not a flag we maintain. `ECRD` is the single bit that decides whether AML reads the
 * EC through our address-space handler or detours onto the SMI mailbox, so the honest question is
 * never "did we call something" but "what does that integer say right now". Two flags in this file
 * already drifted apart from what they claimed; this one cannot, because it is the firmware's own.
 *
 * `Scope (\)` at dsdt.dsl:65054 declares `Name (ECRD, Zero)` — root scope, hence the leading `\\`. */
int acpi_ec_ecrd_read(unsigned long long *val_out) {
    ACPI_HANDLE h = NULL;
    if (ACPI_FAILURE(AcpiGetHandle(NULL, (char*)"\\ECRD", &h)) || !h) return 0;
    ACPI_OPERAND_OBJECT *obj = AcpiNsGetAttachedObject((ACPI_NAMESPACE_NODE *)h);
    if (!obj || obj->Common.Type != ACPI_TYPE_INTEGER) return 0;
    if (val_out) *val_out = (unsigned long long)obj->Integer.Value;
    return 1;
}

/* ★★★★ Set `\ECRD = 1` WITHOUT running `_REG`. The way past a fatal `_REG` on this machine.
 *
 * `acpi probe 10` died at mark 57: `AcpiExecuteRegMethods` went in and never returned. Look at what
 * `_REG` actually does and that is unsurprising — it is two lines of bookkeeping followed by a call
 * into `ECIN()`, which is where all the risk lives:
 *
 *     ECRD = One                      <- the ONLY part we need
 *     ECIN ()
 *         LIDS = ECG3 ()              <- EC read, fine, that is our handler
 *         ^^^GFX0.GLID (LIDS)         <- cross-tree call into the graphics device
 *         Notify (LID0, 0x80)         <- needs AcpiOsExecute to queue work
 *         ECS3 () / ECS2 (ACOS, ACSE)
 *         GENS (0x2D, Zero, Zero)     <- SMI mailbox: SystemMemory region from NVS + ASMI()
 *         EISC (0x81, 0xB8, ...)      <- SMI mailbox again
 *
 * Note it still takes the SMI path AFTER setting ECRD, so `_REG` cannot be made safe by fixing the
 * EC side alone. Three independent ways to die, and bisecting them costs a power cycle each.
 *
 * ★ But none of that is a precondition for reading a battery. `ECIN` is LID/AC notification
 * housekeeping — it tells the OS about a lid switch and pokes the firmware over SMI. `_STA`, `_BIF`
 * and `_BST` only need `ECRD != 0` so that `ECR1` reads `EC00..EC06` from the EmbeddedControl region
 * instead of detouring. That integer lives at root scope and we can simply set it.
 *
 * ⚠️ This is a deliberate lie to the firmware: we assert "the OS owns the EC" without performing the
 * handshake that normally accompanies it. What we skip is notification, not initialisation of the
 * data path — the raw-port battery has been reading these same registers, correctly and
 * hardware-verified against Fedora, with no `_REG` at all for the life of the project. So the EC
 * does not need `ECIN()` to answer; the AML just needs permission to ask it directly.
 *
 * ⚠️ Requires the handler. Setting ECRD with nothing registered would point AML at an
 * EmbeddedControl region that has no handler at all, which is strictly worse than the SMI detour. */
int acpi_ec_force_ecrd(void) {
    if (!nyx_ec_handler_ok) return 0;
    ACPI_HANDLE h = NULL;
    if (ACPI_FAILURE(AcpiGetHandle(NULL, (char*)"\\ECRD", &h)) || !h) return 0;
    ACPI_OPERAND_OBJECT *obj = AcpiNsGetAttachedObject((ACPI_NAMESPACE_NODE *)h);
    if (!obj || obj->Common.Type != ACPI_TYPE_INTEGER) return 0;

    /* 69 -> about to write, 71 -> written and survived. A plain integer store should be incapable of
     * faulting, which is exactly the kind of assumption this project keeps being wrong about. */
    nyx_mark(69);
    obj->Integer.Value = 1;
    nyx_mark(71);

    unsigned long long v = 0;
    return (acpi_ec_ecrd_read(&v) && v == 1) ? 1 : 0;
}

/* Did the EmbeddedControl handler register, and what did ACPICA say?
 *
 * Reported rather than inferred: "the EC works" has meant two different things in this project —
 * ports readable, versus AML able to reach it — and only the second one makes `_BIF`/`_BST` work. */
int acpi_ec_handler_state(unsigned int *status_out) {
    if (status_out) *status_out = (unsigned int)nyx_ec_handler_status;
    return nyx_ec_handler_ok;
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

    // ★ One mark per AML evaluation, because "died somewhere in battery()" is not an answer.
    //
    // Mark 52 brackets this whole function, which evaluates three unrelated methods through the EC.
    // That is the same too-coarse bracket that made mark 55 useless for the handler install: a pair
    // of breadcrumbs only bisects if what sits between them is one thing. The CMOS byte is the only
    // post-mortem field that reads back reliably on this machine, so spend it finely rather than
    // buying the same ambiguity with another power cycle.
    //
    //   61 -> into _STA     62 -> _STA returned
    //   63 -> into _BIF     64 -> _BIF returned
    //   65 -> into _BST     66 -> _BST returned
    //
    // An even mark means AML completed and we faulted in our own unpacking; an odd one means the
    // interpreter did not come back, and the next question is which OperationRegion it was in.

    // _STA bit 4 (0x10) is "battery present". A bay with no battery still publishes the device.
    ACPI_BUFFER r; ACPI_OBJECT *o;
    UINT64 sta = 0;
    r.Length = ACPI_ALLOCATE_BUFFER; r.Pointer = NULL;
    nyx_mark(61);
    if (ACPI_SUCCESS(AcpiEvaluateObject(Object, (char*)"_STA", NULL, &r)) && r.Pointer) {
        o = (ACPI_OBJECT *)r.Pointer;
        if (o->Type == ACPI_TYPE_INTEGER) sta = o->Integer.Value;
        AcpiOsFree(r.Pointer);
    }
    nyx_mark(62);
    if (!(sta & 0x10)) return AE_OK;
    b->present = 1;

    r.Length = ACPI_ALLOCATE_BUFFER; r.Pointer = NULL;
    nyx_mark(63);
    if (ACPI_SUCCESS(AcpiEvaluateObject(Object, (char*)"_BIF", NULL, &r)) && r.Pointer) {
        o = (ACPI_OBJECT *)r.Pointer;
        b->have_bif = nyx_pkg_int(o, 0, &b->power_unit)
                    & nyx_pkg_int(o, 1, &b->design_cap)
                    & nyx_pkg_int(o, 2, &b->last_full_cap)
                    & nyx_pkg_int(o, 4, &b->design_voltage);
        AcpiOsFree(r.Pointer);
    }
    nyx_mark(64);

    r.Length = ACPI_ALLOCATE_BUFFER; r.Pointer = NULL;
    nyx_mark(65);
    if (ACPI_SUCCESS(AcpiEvaluateObject(Object, (char*)"_BST", NULL, &r)) && r.Pointer) {
        o = (ACPI_OBJECT *)r.Pointer;
        b->have_bst = nyx_pkg_int(o, 0, &b->state)
                    & nyx_pkg_int(o, 1, &b->present_rate)
                    & nyx_pkg_int(o, 2, &b->remaining_cap)
                    & nyx_pkg_int(o, 3, &b->voltage);
        AcpiOsFree(r.Pointer);
    }
    nyx_mark(66);
    return AE_OK;
}

// Fills 11 ints in the NyxBattery order above. Returns 1 if a present battery was found.
int acpi_battery_read(int *out, int max) {
    NyxBattery b;
    /* Snapshot the parameter and the frame BEFORE any AML runs. Everything after this point is
     * compared against these two numbers, so "the interpreter smashed our frame" stops being a story
     * and becomes a subtraction. */
    nyx_out_ptr_entry = (unsigned long long)out;
    {
        unsigned long long rsp;
        __asm__ volatile ("mov %%rsp, %0" : "=r"(rsp));
        nyx_rsp_at_entry = rsp;
    }
    memset(&b, 0, sizeof(b));

    // ★ Direct path, same reasoning as the EC above: walks crash this kernel, direct handles do not.
    // `Device (BAT0)` is in `Scope (\_SB)` at dsdt.dsl:66042 with `_HID` = PNP0C0A. There is a BAT1
    // too, but this laptop has one bay and Fedora reports only BAT0.
    ACPI_HANDLE bat = NULL;
    // 60 = the handle resolved. Separates "BAT0 could not be looked up" from "we got into the
    // methods", which mark 52 alone could not.
    if (ACPI_SUCCESS(AcpiGetHandle(NULL, (char*)"\\_SB.BAT0", &bat)) && bat) {
        nyx_mark(60);
        NyxBatteryCallback(bat, 0, &b, NULL);
    }

    unsigned long long rsp;
    __asm__ volatile ("mov %%rsp, %0" : "=r"(rsp));
    nyx_rsp_at_unpack = rsp;
    nyx_out_ptr = (unsigned long long)out;

    nyx_mark(72);

    /* ★★★★ STOP WRITING THROUGH THE CALLER'S POINTER FROM INSIDE THE AML CONTEXT.
     *
     * Three boots narrowed this and each one came back "not that":
     *
     *   72, no 73   -> the fault is in the unpack
     *   77, no 73   -> `out` is non-NULL and canonical (test was too weak)
     *   77, no 73   -> `out` is ALSO unchanged across the call AND high-half
     *
     * So the pointer is correct, correctly shaped, and the same one we were handed — and writing 44
     * bytes through it still faults. It names a Rust local in the caller's live frame, on the stack
     * we are currently executing on, which cannot be unmapped in any story I can construct. Every
     * `AcpiOs*` wait is a bounded spin, so no task switch moved us. I am out of theories that
     * survive contact with the evidence, and a fourth guess costs another power cycle.
     *
     * So change the shape of the problem instead of guessing again: land the result in a STATIC and
     * let the caller collect it afterwards, outside the AML context, via `acpi_battery_fetch`. No
     * caller pointer is dereferenced here at all.
     *
     * ★ This is a candidate FIX and the experiment at once, which is why it is worth the boot:
     *     - crash gone  -> the problem is specifically writing to the caller's frame after AML, and
     *                      we ALSO finally get the battery values
     *     - crash stays -> `out` was never the issue and the fault is elsewhere in this range
     *
     * ⚠️ The static is `volatile` and written field-by-field: it must not become a memcpy from a
     * local that the compiler is free to sink back onto the stack. */
    nyx_batt_vals[0]  = b.present;
    nyx_batt_vals[1]  = b.power_unit;
    nyx_batt_vals[2]  = b.design_cap;
    nyx_batt_vals[3]  = b.last_full_cap;
    nyx_batt_vals[4]  = b.design_voltage;
    nyx_batt_vals[5]  = b.state;
    nyx_batt_vals[6]  = b.present_rate;
    nyx_batt_vals[7]  = b.remaining_cap;
    nyx_batt_vals[8]  = b.voltage;
    nyx_batt_vals[9]  = b.have_bif;
    nyx_batt_vals[10] = b.have_bst;
    nyx_batt_present  = b.present;

    nyx_mark(73);
    (void)out; (void)max;
    return b.present;
}

/* Collect what the last `acpi_battery_read` found. Pure copy out of a static — no AML, no namespace,
 * no interpreter frame anywhere beneath it. Deliberately a separate call so the write into the
 * caller's memory happens well after the ACPI context has been left. */
int acpi_battery_fetch(int *out, int max) {
    if (!out || max <= 0) return 0;
    int n = (max < 11) ? max : 11;
    for (int i = 0; i < n; i++) out[i] = nyx_batt_vals[i];
    return nyx_batt_present;
}

/* ── AC adapter ────────────────────────────────────────────────────────────────────────────────
 *
 * No AML, and deliberately so. The DSDT hands us the answer directly:
 *
 *     Method (_PSR) { Local0 = ECG5 (); Local0 &= One;  ... Return (Local0) }   // AC online
 *     Method (BAT0._STA) { Local0 = ECG5 (); Local0 &= 0x02; ... }              // battery present
 *
 * and `ECG5()` is `ECRB(0x06)`. So both bits live in EC register 0x06 and a raw read gets them with
 * no interpreter, no handler and no risk. ⚠️ `_PSR` additionally calls `PNOT()` whenever the state
 * differs from `PWRS` — a Serialized method that notifies every power consumer — which is real work
 * we neither need nor want on a poll. Reading the register is the whole of the information.
 *
 * No bank select here: 0x03 selects the BATTERY window, and `ECG5` does not touch it. */
int acpi_ec_ac_status(int *ac_online, int *batt_present) {
    UINT8 v = 0;
    if (!nyx_ec_read_byte(0x06, &v)) return 0;
    if (ac_online)    *ac_online    = (v & 0x01) ? 1 : 0;
    if (batt_present) *batt_present = (v & 0x02) ? 1 : 0;
    return 1;
}

/* ── Thermal ───────────────────────────────────────────────────────────────────────────────────
 *
 * ★★★ THIS IS THE CLEAREST PAYOFF OF SETTING `ECRD`, AND THE MOST DANGEROUS THING TO GET WRONG.
 *
 *     Method (_TMP, 0, Serialized) {
 *         If (\ECRD) { Local0 = ECDV.KDRT (n); Return ((0x0AAC + (Local0 * 0x0A))) }
 *         Else       { Return (0x0BB8) }
 *     }
 *
 * ⚠️⚠️ With `ECRD` clear, every sensor on this machine returns **0x0BB8 = 3000 deci-Kelvin =
 * 26.85 C** — a hardcoded constant that looks exactly like a plausible idle temperature. A thermal
 * stack built against that would have looked like it worked, forever, on a laptop that overheats.
 * `26.85 C` (or a suspiciously round 3000) is the tell that `ECRD` is zero, NOT a reading.
 *
 * Units are deci-Kelvin: 0x0AAC = 2732 = 273.2 K = 0 C, and `KDRT` returns whole degrees C, so
 * degrees C = (dK - 2732) / 10. Values are parked in a static and collected by
 * `acpi_thermal_fetch`, for the same reason the battery is — see acpi_battery_read. */
static const char *NYX_TZ_PATHS[4] = {
    "\\_SB.PCI0.B0D4",              /* package / CPU  (KDRT 0) */
    "\\_SB.PCI0.LPCB.ECDV.TMEM",    /* memory         (KDRT 2) */
    "\\_SB.PCI0.LPCB.ECDV.TSKN",    /* skin                    */
    "\\_SB.PCI0.LPCB.ECDV.NGFF",    /* M.2 / SSD               */
};
static volatile int nyx_tz_dk[4];
static volatile int nyx_tz_found = 0;

int acpi_thermal_read(void) {
    int n = 0;
    for (int i = 0; i < 4; i++) {
        nyx_tz_dk[i] = 0;
        ACPI_HANDLE h = NULL;
        if (ACPI_FAILURE(AcpiGetHandle(NULL, (char*)NYX_TZ_PATHS[i], &h)) || !h) continue;
        ACPI_BUFFER r; r.Length = ACPI_ALLOCATE_BUFFER; r.Pointer = NULL;
        /* 80..83, one per sensor: four evaluations bracketed by a single pair would repeat the
         * mark-55 mistake of bracketing several things and distinguishing none. */
        nyx_mark((unsigned char)(80 + i));
        if (ACPI_SUCCESS(AcpiEvaluateObject(h, (char*)"_TMP", NULL, &r)) && r.Pointer) {
            ACPI_OBJECT *o = (ACPI_OBJECT *)r.Pointer;
            if (o->Type == ACPI_TYPE_INTEGER) {
                nyx_tz_dk[i] = (int)(o->Integer.Value & 0x7FFFFFFF);
                n++;
            }
            AcpiOsFree(r.Pointer);
        }
    }
    nyx_mark(84);
    nyx_tz_found = n;
    return n;
}

int acpi_thermal_fetch(int *out, int max) {
    if (!out || max <= 0) return 0;
    int n = (max < 4) ? max : 4;
    for (int i = 0; i < n; i++) out[i] = nyx_tz_dk[i];
    return nyx_tz_found;
}

/* Stack depth observed across the AML evaluation, so the overflow theory can be checked rather than
 * argued. `rsp_min` is the deepest point our handler was ever called at — the interpreter's own
 * recursion sits below the governor frame, and this is the only place we get to see it. */
int acpi_stack_probe(unsigned long long *vals5) {
    if (!vals5) return 0;
    vals5[0] = nyx_rsp_at_entry;    /* frame on the way in                     */
    vals5[1] = nyx_rsp_at_unpack;   /* frame on the way out — must be identical */
    vals5[2] = nyx_rsp_min_handler; /* deepest the interpreter took us          */
    vals5[3] = nyx_out_ptr_entry;   /* the caller's pointer on the way in       */
    vals5[4] = nyx_out_ptr;         /* and on the way out                       */
    return 1;
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
extern void nyx_walk_at(unsigned int kind, unsigned int level, unsigned int index,
                        unsigned int name, unsigned long long node, unsigned long long parent);
extern void nyx_walk_done(void);

// ★★★ COUNT THE ROOT'S CHILD CHAIN AT EACH STAGE OF BRING-UP.
//
// This is the measurement that separates the two remaining stories, and nothing softer will do it —
// three hypotheses in a row have died because they were reasoned out instead of measured.
//
// The facts: the DSDT declares ~1468 root-scope names (1353 field units + 115 top-level), the load
// reports success with every AML table LOADED and no error in the log, yet the live chain is 637 long
// and ends on a block that is not a node. So either
//
//   (a) the nodes were never created — the parse stopped and lied about it, or
//   (b) they were created and something UNLINKED them afterwards.
//
// ⚠️ It cannot be settled with `AcpiGetHandle`, which was the obvious idea: name lookup walks the
// same `Peer` chain via `AcpiNsSearchOneScope`, so a failure there means "not created" OR "chain
// broken" and cannot tell them apart — and it would walk into the same garbage node and #GP.
//
// Counting the chain at three points does settle it. `AcpiInitializeObjects` executes `_INI` methods,
// and AML execution creates temporary namespace nodes and DELETES them — `AcpiNsDeleteNode` unlinks
// by walking the parent's child list and rewriting a `Peer`. That is the only code in the whole
// sequence that writes a `Peer` after load, and it runs on a chain 1468 long.
//
// So: 1468 after load and 637 after objects-init points at deletion. Short at every stage points at
// the parse. One number per stage, and it lands in the ACPICA log, which is open during bring-up.
//
// Walks nothing but `Peer` with a hard cap, validates as it goes, and stops at the first node that
// is not one — so it cannot crash the machine it is trying to diagnose.
static void acpi_log_root_count(const char *tag) {
    char line[160];
    ACPI_NAMESPACE_NODE *root = AcpiGbl_RootNode;
    if (!root) {
        snprintf(line, sizeof(line), "  root-count %s: NO ROOT NODE\n", tag);
        nyx_acpi_log(line);
        return;
    }

    unsigned int n = 0;
    unsigned int bad = 0;
    ACPI_NAMESPACE_NODE *last_good = NULL;
    ACPI_NAMESPACE_NODE *node = root->Child;

    while (node && n < 8192) {
        // A real node is a child of this root with four printable name bytes. Anything else means we
        // have left the list, and reading its `Peer` is the step that faults — so stop here.
        unsigned int name = node->Name.Integer;
        int name_ok = 1;
        for (int i = 0; i < 4; i++) {
            unsigned char b = (unsigned char)(name >> (i * 8));
            if (!((b >= 'A' && b <= 'Z') || (b >= '0' && b <= '9') || b == '_')) name_ok = 0;
        }
        if (node->Parent != root || !name_ok) { bad = 1; break; }
        last_good = node;
        n++;
        node = node->Peer;
    }

    snprintf(line, sizeof(line),
             "  root-count %s: %u children, tail %s, last %.4s\n",
             tag, n,
             bad ? "DANGLING (peer -> non-node)" : (node ? "CAPPED" : "clean NULL"),
             last_good ? (const char *)&last_good->Name.Ascii[0] : "----");
    nyx_acpi_log(line);
}

void acpi_log_root_count_tag(const char *tag) { acpi_log_root_count(tag); }

// ★★★ The same count, on demand from userspace, because the three bring-up counts came back
// **1802 children, clean NULL, at every stage** — and `acpi ls` then saw 637 and garbage.
//
// So the namespace is built correctly and something breaks it LATER. The only ACPI activity between
// the end of bring-up and userspace typing a command is the thermal governor's per-tick calls, which
// evaluate AML — and AML execution creates and deletes temporary namespace nodes. `AcpiNsDeleteNode`
// unlinks by walking the parent's child list and rewriting a `Peer`, on a chain 1802 long.
//
// Run this repeatedly and watch the number. It turns "it is broken by the time I look" into "it
// broke between these two ticks", which is the difference between a theory and a bisect.
//
// Same guarantees as the bring-up version: follows `Peer` only, validates every node, capped, stops
// before dereferencing anything that is not a node. It cannot crash.
int acpi_root_count(int *bad_out) {
    ACPI_NAMESPACE_NODE *root = AcpiGbl_RootNode;
    if (bad_out) *bad_out = 0;
    if (!root) return -1;

    int n = 0;
    ACPI_NAMESPACE_NODE *node = root->Child;
    while (node && n < 8192) {
        unsigned int name = node->Name.Integer;
        int name_ok = 1;
        for (int i = 0; i < 4; i++) {
            unsigned char b = (unsigned char)(name >> (i * 8));
            if (!((b >= 'A' && b <= 'Z') || (b >= '0' && b <= '9') || b == '_')) name_ok = 0;
        }
        if (node->Parent != root || !name_ok) { if (bad_out) *bad_out = 1; break; }
        n++;
        node = node->Peer;
    }
    return n;
}

// ★★ The callback now PUBLISHES THE NODE, and that is the point of this whole pass.
//
// A pure counter told us "it dies somewhere in the walk", which we already knew. The walker crosses
// thousands of nodes and dies on one; the useful question is WHICH. `nyx_walk_at` writes the cursor
// to plain kernel memory and `panic_screen::fatal` prints it, so the red screen now names the node —
// one boot instead of one power cycle per depth guess. It deliberately does NOT go through the CMOS
// breadcrumb: bytes 1 and 3 of that field read back as zero on this board.
static ACPI_STATUS NyxCountCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **Ret) {
    NyxNsCount *c = (NyxNsCount *)Context;
    ACPI_NAMESPACE_NODE *n = (ACPI_NAMESPACE_NODE *)Object;
    nyx_walk_at(1, Level, (unsigned int)c->nodes, n ? n->Name.Integer : 0,
                (unsigned long long)(ACPI_SIZE)n,
                (unsigned long long)(ACPI_SIZE)(n ? n->Parent : NULL));
    c->nodes++;
    return AE_OK;
}

// ★★ THE SAME WALK BUILT ON THE STEPPER — the control experiment, and the likely replacement.
//
// ⚠️ FIRST, THE CORRECTION THAT MOTIVATES IT. The reasoning above rests on "`acpi ls` enumerated 512
// root children with no crash, so traversal is sound" — and that is NOT a like-for-like comparison.
// `acpi ls` enumerates the ROOT's children only: one chain of `->Peer` pointers at depth 1. It never
// follows a single `->Child` pointer. `AcpiNsWalkNamespace` DESCENDS — it dereferences `->Child` on
// every node and `->Parent` on the way back up — and those pointers have never been exercised by
// anything that works.
//
// So "the only difference is the locking" was false, which is exactly why bypassing the reader lock
// and then ACPI_NS_WALK_NO_UNLOCK each changed nothing. Neither was ever the difference. Descent is.
//
// This walk descends using ONLY the public stepper, so it isolates that one variable:
//
//   Survives where the ACPICA walker dies -> the walker is at fault, and this becomes the walk that
//                                            `AcpiInstallAddressSpaceHandler` gets rebuilt on.
//   Dies at the same node                 -> the NODE is at fault; the walker was the messenger, and
//                                            the four-character name on the panic screen looks up
//                                            directly in nyx-recv/dsdt.dsl.
//
// Recursion is bounded by `depth`, and the ACPI namespace is single-digit levels deep, so the kernel
// stack is not at risk.
static void NyxStepWalk(ACPI_HANDLE parent, UINT32 level, UINT32 max_depth, NyxNsCount *c) {
    ACPI_HANDLE child = NULL;
    while (ACPI_SUCCESS(AcpiGetNextObject(ACPI_TYPE_ANY, parent, child, &child)) && child) {
        ACPI_NAMESPACE_NODE *n = (ACPI_NAMESPACE_NODE *)child;
        nyx_walk_at(2, level, (unsigned int)c->nodes, n ? n->Name.Integer : 0,
                    (unsigned long long)(ACPI_SIZE)n,
                    (unsigned long long)(ACPI_SIZE)parent);
        c->nodes++;
        if (level < max_depth) {
            NyxStepWalk(child, level + 1, max_depth, c);
        }
    }
}

void acpi_count_nodes_stepper(int depth, int *nodes) {
    NyxNsCount c = { 0, 0, 0 };
    UINT32 d = (depth <= 0) ? ACPI_UINT32_MAX : (UINT32)depth;
    nyx_mark(90);
    NyxStepWalk(ACPI_ROOT_OBJECT, 1, d, &c);
    nyx_walk_done();
    nyx_mark(91);
    if (nodes) *nodes = c.nodes;
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

    // ⚠️⚠️ THE TWO PARAGRAPHS BELOW ARE KEPT AS A RECORD OF A WRONG TURN. Both experiments they
    // describe were run, both cost a power cycle, and neither changed anything — because the premise
    // they share is false. "`acpi ls` enumerated 512 root children, so traversal is sound" compares
    // a walk that DESCENDS against one that never left depth 1. See `NyxStepWalk` above.
    //
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
    nyx_walk_done();
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
