//! Wi-Fi agent — headless, short-lived, and the only process on this machine that blocks in the
//! radio.
//!
//! It has two jobs, told apart by whether it was launched with an argument.
//!
//! **No argument** — the original job, and still what `apps/init` spawns at boot: read the
//! remembered network out of `wifi.conf` and rejoin it. The kernel deliberately has no built-in
//! network; it scans at boot and then waits. This is what turns "waits" into "reconnects to the
//! network you used last time" without the desktop having to be up.
//!
//! **With an argument** — Meridian step 20. `apps/shell` now owns the network picker (it is a
//! drill-down on the Entity surface, `libs/meridian/src/network.rs`) and hands each radio operation
//! here rather than making it itself. See the note on [`nyx_api::WifiOp`] for the full reasoning;
//! the short version is that the shell IS the window server, four of the six Wi-Fi syscalls block
//! for seconds, and a window server that stops compositing for fifteen seconds is not slow, it is
//! frozen — with the hardware cursor still gliding over the stale frame, which is exactly what a
//! GPU hang looks like from the outside.
//!
//! So this stayed a separate process for the same reason it was one in the first place, quoting its
//! own first version: *"the connect call blocks for ~10 s, and as its own process that costs
//! nothing."* Step 20 did not invent a mechanism; it noticed the mechanism already existed.
//!
//! Nothing here reports back. The kernel publishes the outcome in `sys_wifi_status`, which the
//! shell polls anyway — a second channel saying the same thing is a second channel that can
//! disagree with the first.

#![no_std]
#![no_main]

use nyx_api::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.entry")]
pub extern "C" fn _start() -> ! {
    // `_start` is a plain `extern "C" fn` with no argv reachable from Rust — hence the syscall.
    let mut arg = [0u8; WIFI_OP_MAX];
    let n = sys_launch_arg(&mut arg).min(arg.len());
    let request = core::str::from_utf8(&arg[..n]).ok().and_then(wifi_op_decode);

    match request {
        Some((op, ssid, psk)) => run_op(op, ssid, psk),
        // No argument, or one we do not recognise. Fall through to the boot job — an agent that
        // refused to reconnect because it was handed something odd would take the machine offline
        // to complain about a string.
        None => boot_reconnect(),
    }
}

/// Perform one operation for the shell, then exit. Each of these is a single blocking syscall; the
/// process exists only to be the thing that blocks.
fn run_op(op: WifiOp, ssid: &str, psk: &str) -> ! {
    match op {
        WifiOp::Scan => {
            sys_print("[WIFIAGENT] Scanning all channels...\n");
            sys_wifi_scan();
        }
        WifiOp::Join | WifiOp::JoinOnce => {
            sys_print("[WIFIAGENT] Joining a network...\n");
            match sys_wifi_connect(ssid, psk) {
                WIFI_CONNECTED => {
                    // Remember it only NOW, and only if we were asked to. A passphrase that
                    // completed the 4-way handshake is known-good, so the boot agent — this same
                    // binary, tomorrow morning — can never be handed a wrong one. Saving on the
                    // attempt rather than on the result is how a machine ends up retrying a typo on
                    // every power cycle.
                    //
                    // ⚠️ `wifi.conf` is plain text on an unencrypted filesystem. That was true
                    // before this step and is unchanged by it; it is worth restating because the
                    // passphrase now also passes through a launch argument, and it would be easy
                    // to conclude the argument is the weak link. It is not — the file is, and
                    // `join-once` is the only way to join without writing to it.
                    if op == WifiOp::Join {
                        wifi_save_network(ssid, psk);
                    }
                    sys_print("[WIFIAGENT] Connected.\n");
                }
                WIFI_AUTH_FAILED => sys_print("[WIFIAGENT] The password was rejected.\n"),
                WIFI_NO_LEASE => sys_print("[WIFIAGENT] Joined, but no address was offered.\n"),
                _ => sys_print("[WIFIAGENT] Could not join.\n"),
            }
        }
        WifiOp::Leave => {
            sys_print("[WIFIAGENT] Leaving the network...\n");
            sys_wifi_disconnect();
        }
        WifiOp::RadioOn | WifiOp::RadioOff => {
            let on = op == WifiOp::RadioOn;
            sys_print(if on {
                "[WIFIAGENT] Turning the radio on...\n"
            } else {
                "[WIFIAGENT] Turning the radio off...\n"
            });
            sys_wifi_set_radio(on);
        }
    }
    sys_exit(0);
}

/// The boot job: rejoin the remembered network, if there is one.
fn boot_reconnect() -> ! {
    let mut buf = [0u8; 160];
    let (slen, plen) = match wifi_load_network(&mut buf) {
        Some(v) => v,
        None => {
            sys_print("[WIFIAGENT] No remembered network - leaving the radio idle.\n");
            sys_exit(0);
        }
    };
    let ssid = match core::str::from_utf8(&buf[..slen]) {
        Ok(s) => s,
        Err(_) => {
            sys_print("[WIFIAGENT] Saved SSID is not valid UTF-8 - ignoring it.\n");
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
            if st.state == WIFI_CONNECTED {
                sys_exit(0);
            }
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
