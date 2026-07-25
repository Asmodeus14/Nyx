//! Wi-Fi auto-connect agent — headless, spawned by init at boot, exits as soon as it's done.
//!
//! The kernel deliberately has no built-in network: it scans at boot and then waits. This is what
//! makes "waits" turn back into "reconnects to the network you used last time" without the desktop
//! having to be up, or a window having to open.
//!
//! It lives in userspace rather than the kernel for two reasons: the connect call blocks for ~10 s
//! (as its own process that costs nothing, in the boot path it would stall every other daemon), and
//! the credentials file is only readable once the NVMe filesystem is mounted, which is well after
//! the PCI bring-up that owns the radio.

#![no_std]
#![no_main]

use nyx_api::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    let mut buf = [0u8; 160];
    let (slen, plen) = match wifi_load_network(&mut buf) {
        Some(v) => v,
        None => {
            sys_print("[WIFIAGENT] No remembered network — leaving the radio idle.\n");
            sys_exit(0);
        }
    };
    let ssid = match core::str::from_utf8(&buf[..slen]) {
        Ok(s) => s,
        Err(_) => {
            sys_print("[WIFIAGENT] Saved SSID is not valid UTF-8 — ignoring it.\n");
            sys_exit(1);
        }
    };
    let psk = core::str::from_utf8(&buf[slen..slen + plen]).unwrap_or("");

    // The boot scan runs during PCI bring-up, so results are normally ready before we are. Give it
    // a few seconds anyway — connect() resolves the SSID against the scan list, and an empty list
    // would look like "network out of range" when it's really "we asked too early".
    let mut probe = [WifiNetwork::default(); 4];
    let mut waited = 0;
    while sys_wifi_list(&mut probe) == 0 && waited < 10 {
        if let Some(st) = sys_wifi_status() {
            if st.state == WIFI_CONNECTED { sys_exit(0); }
        } else {
            sys_print("[WIFIAGENT] No Wi-Fi adapter on this machine.\n");
            sys_exit(0);
        }
        sys_sleep_ms(500);
        waited += 1;
    }

    sys_print("[WIFIAGENT] Reconnecting to the remembered network...\n");
    match sys_wifi_connect(ssid, psk) {
        WIFI_CONNECTED => sys_print("[WIFIAGENT] Connected.\n"),
        WIFI_AUTH_FAILED => sys_print("[WIFIAGENT] Saved password was rejected.\n"),
        WIFI_NO_LEASE => sys_print("[WIFIAGENT] Joined, but no IP address was offered.\n"),
        _ => sys_print("[WIFIAGENT] Could not reconnect.\n"),
    }
    sys_exit(0);
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    sys_exit(111);
}
