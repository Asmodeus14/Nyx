#include "acpi.h"

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
// NYXOS ACPI THERMAL & FAN CONTROLLER
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