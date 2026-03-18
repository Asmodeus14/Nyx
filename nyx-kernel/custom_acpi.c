#include "acpi.h"

// This callback runs for literally every object in the motherboard
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