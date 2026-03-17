#include "acpi.h"

// This callback runs for every single device in the motherboard's ACPI tree
static ACPI_STATUS PowerOnWifiCallback(ACPI_HANDLE Object, UINT32 Level, void *Context, void **ReturnValue) {
    ACPI_DEVICE_INFO *Info;
    ACPI_STATUS Status;

    Status = AcpiGetObjectInfo(Object, &Info);
    if (ACPI_FAILURE(Status)) { return AE_OK; }

    // Look for the CNVi module at PCI Device 20 (0x14), Function 3
    if ((Info->Valid & ACPI_VALID_ADR) && (Info->Address == 0x00140003)) {
        
        // WE FOUND IT! Execute _PS0 (Power State 0 / Fully On)
        Status = AcpiEvaluateObject(Object, (char*)"_PS0", NULL, NULL);
        
        if (ACPI_SUCCESS(Status)) {
            *((int*)Context) = 1; // Mark as successfully powered on!
        }
    }

    ACPI_FREE(Info);
    return AE_OK; // Continue walking the tree
}

// The function we will call from Rust
int acpi_wake_cnvi_wifi(void) {
    int success = 0;
    // Walk the entire ACPI Namespace from the root down
    AcpiWalkNamespace(ACPI_TYPE_DEVICE, ACPI_ROOT_OBJECT, ACPI_UINT32_MAX, 
                      PowerOnWifiCallback, NULL, &success, NULL);
    return success;
}