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