#![no_std]
#![no_main]
#![allow(warnings)] // avoid the annotate_snippets renderer ICE on snippet-bearing warnings (see libs/gui)

use nyx_api::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    sys_print("[INIT] NyxOS Init Orchestrator Started (PID 1)\n");

    // 1. Spawn the Window Server dynamically from the NVMe Drive!
    sys_print("[INIT] Spawning WindowServer.nyx from SSD...\n");
    let gui_pid = sys_fork();
    if gui_pid == 0 {
        // 🚨 THE FIX: Point to the new App Bundle path
        sys_execve("/mnt/nvme/apps/WindowServer.nyx/run.bin\0");
        sys_exit(1);
    }

    // 2. Reconnect to the remembered Wi-Fi network, if there is one. Headless and short-lived: it
    // blocks for ~10 s inside the kernel while it associates, which is exactly why it runs as its
    // own process rather than inline here.
    sys_print("[INIT] Spawning WifiAgent.nyx (auto-reconnect)...\n");
    if sys_fork() == 0 {
        sys_execve("/mnt/nvme/apps/WifiAgent.nyx/run.bin\0");
        sys_exit(1);
    }

    loop { sys_sleep_ms(1000); }
}
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_print("[INIT] FATAL: Orchestrator Panicked!\n");
    sys_exit(99);
}