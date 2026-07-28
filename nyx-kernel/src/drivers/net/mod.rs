pub mod iwlwifi;
pub mod rtl8168;

use spin::Mutex;
use smoltcp::iface::{Interface, SocketSet, SocketHandle};
use smoltcp::socket::dhcpv4::{Socket as DhcpSocket, Event as DhcpEvent};
use smoltcp::socket::dns::{Socket as DnsSocket, GetQueryResultError};
use smoltcp::time::Instant;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering}; 
use alloc::boxed::Box;
use alloc::vec::Vec;

pub static NET_DRIVER: Mutex<Option<crate::drivers::net::rtl8168::Rtl8168Driver>> = Mutex::new(None);
// WiFi (Intel Wireless-AC 9462 CNVi) — a second smoltcp Device, on its own interface so a fault
// here can never disturb the wired stack.
pub static WIFI_DRIVER: Mutex<Option<crate::drivers::net::iwlwifi::IntelWifiDriver>> = Mutex::new(None);
pub static WIFI_IFACE: Mutex<Option<Interface>> = Mutex::new(None);
pub static WIFI_SOCKETS: Mutex<Option<SocketSet<'static>>> = Mutex::new(None);
pub static WIFI_DNS_HANDLE: Mutex<Option<SocketHandle>> = Mutex::new(None);
static WIFI_DNS_QUERY: Mutex<Option<smoltcp::socket::dns::QueryHandle>> = Mutex::new(None);
static WIFI_DNS_TRIES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
pub static NET_IFACE: Mutex<Option<Interface>> = Mutex::new(None);
pub static GLOBAL_SOCKETS: Mutex<Option<SocketSet<'static>>> = Mutex::new(None);
pub static DHCP_HANDLE: Mutex<Option<SocketHandle>> = Mutex::new(None);

//  MILESTONE 3.2: Global handle for the DNS resolution socket
pub static DNS_HANDLE: Mutex<Option<SocketHandle>> = Mutex::new(None);

pub static NETWORK_PENDING: AtomicBool = AtomicBool::new(false);
static LAST_TIME: AtomicU64 = AtomicU64::new(0);

lazy_static::lazy_static! {
    pub static ref RX_BUFFER_POOL: Mutex<Vec<Box<[u8; 2048]>>> = Mutex::new(Vec::new());
}

// ============================================================================
// P8c: the WiFi locking rules. Read this before touching WIFI_* from a syscall.
// ============================================================================
//
// ★ WIFI_DRIVER / WIFI_IFACE / WIFI_SOCKETS / WIFI_DNS_* are plain spin locks, and the IDLE TASK
//   takes them from poll_wifi with interrupts ENABLED — so the holder can be preempted mid-hold.
//   A syscall, by contrast, runs with interrupts DISABLED (SYSCALL clears IF via the FMASK MSR set
//   in init_syscalls). Blocking on one of those locks from a syscall is therefore a HARD DEADLOCK:
//   the holder is a task that can only be resumed by the timer interrupt, and the spinner has the
//   timer masked, so on that core neither ever makes progress. The core spins at 100% forever —
//   and the thermal governor is itself an ordinary kernel task, so it dies with the scheduler and
//   nothing is left to command the fans or throttle the package. That is a runaway-heat wedge, not
//   just a hang.
//
//   The rule, enforced by the syscall arms in interrupts.rs:
//     - Those four locks are only ever acquired with interrupts ENABLED. The long operations
//       (scan / connect / disconnect) re-enable IF for their duration, exactly as the sys_sleep_ms
//       arm does. Preemptible holder + preemptible waiter is merely slow; it cannot deadlock.
//     - The frequent pollers (tray glyph at 1 Hz, picker at 2 Hz) never touch the driver at all.
//       They read WIFI_SNAPSHOT below.
//
// ★ WIFI_SNAPSHOT is the opposite discipline: only ever taken with interrupts off, never nested
//   inside anything but the driver lock, and only ever held for a bounded memcpy. A lock that is
//   never held across a preemption point can't participate in this deadlock at all.

/// A published, read-only view of the radio: exactly the bytes syscalls 544/546 hand back.
///
/// Publishing rather than reaching into the driver is what keeps the pollers off WIFI_DRIVER — a
/// join holds that lock for ~10 seconds, and a 1 Hz tray poll that blocked on it would be a
/// once-a-second chance to wedge the machine (see the rules above).
pub struct WifiSnapshot {
    /// nyx_api::WifiStatus, 53 bytes: state u32, ip/mask/router/dns, ssid_len, ssid[32].
    pub status: [u8; 53],
    /// nyx_api::WifiNetwork, 44 bytes each, hidden SSIDs already filtered out.
    pub nets: Vec<[u8; 44]>,
    /// False until a driver has been bound at all, so "no adapter" stays distinguishable from
    /// "adapter present but idle" — the picker prints a different message for each.
    pub present: bool,
}

pub static WIFI_SNAPSHOT: Mutex<WifiSnapshot> = Mutex::new(WifiSnapshot {
    status: [0; 53],
    nets: Vec::new(),
    present: false,
});

/// The extra LinkState the driver itself never produces: the radio is administratively off. Mirrors
/// `nyx_api::WIFI_RADIO_OFF` — keep the two in step.
pub const WIFI_STATE_RADIO_OFF: u32 = 6;

/// Wi-Fi soft-kill — the "Wi-Fi: Off" switch in the picker.
///
/// This is deliberately a SOFT kill: the firmware stays loaded and the adapter stays powered. Only
/// the things the user can see stop happening — the link is torn down, the interface is dropped, and
/// scans and joins are refused until it goes back on. Turning the radio back on is then instant.
///
/// The alternative, an actual power-down and firmware reload, was rejected: reloading firmware on
/// this adapter is the single operation that can fail outright and leave it needing
/// `restart_firmware()`, and a toggle the user flicks casually must not be able to break the radio.
///
/// An atomic rather than a lock, because it is read from `publish_wifi_snapshot` (interrupts on) and
/// from the syscall arms (interrupts off) — the shape that has deadlocked this kernel before.
static WIFI_RADIO_OFF: AtomicBool = AtomicBool::new(false);

pub fn wifi_radio_on() -> bool {
    !WIFI_RADIO_OFF.load(Ordering::Relaxed)
}

pub fn set_wifi_radio_off(off: bool) {
    WIFI_RADIO_OFF.store(off, Ordering::Relaxed);
}

/// Serialises the long WiFi operations against each other. They run with interrupts enabled, so two
/// of them really can overlap (the picker and the boot agent both call connect); without this the
/// second would block on WIFI_DRIVER for the first's full ten seconds. Refusing is better than
/// queueing — the caller gets the current state back immediately instead of freezing its UI.
static WIFI_BUSY: AtomicBool = AtomicBool::new(false);

/// Claim the radio for a long operation. False means one is already running; do nothing.
pub fn wifi_try_begin() -> bool {
    WIFI_BUSY
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
}

pub fn wifi_end() {
    WIFI_BUSY.store(false, Ordering::Release);
}

/// The LinkState from the last published snapshot, for callers that couldn't run.
pub fn wifi_cached_state() -> u32 {
    // Masked for the same reason as the publish below: this is reachable from both sides of the
    // preemption boundary, and the read is two words.
    x86_64::instructions::interrupts::without_interrupts(|| {
        let s = WIFI_SNAPSHOT.lock();
        u32::from_le_bytes([s.status[0], s.status[1], s.status[2], s.status[3]])
    })
}

/// Re-publish the snapshot from the live driver. Call this at the end of every operation that can
/// change the link or the scan list, while still holding the driver lock and with interrupts on.
pub fn publish_wifi_snapshot(w: &crate::drivers::net::iwlwifi::IntelWifiDriver) {
    let (state, ip, ssid) = w.status();
    let mut status = [0u8; 53];

    // ★ "Radio off" is imposed HERE, not at each caller. Every reader — the tray glyph, the picker,
    // the boot agent — goes through this snapshot, so a single override is what stops them from
    // disagreeing with each other about whether Wi-Fi is on. Everything else is left zeroed: with the
    // radio off there is no address, no SSID and no network list to show, and reporting a stale lease
    // would make the switch look like it did nothing.
    if !wifi_radio_on() {
        status[0..4].copy_from_slice(&WIFI_STATE_RADIO_OFF.to_le_bytes());
        x86_64::instructions::interrupts::without_interrupts(|| {
            let mut snap = WIFI_SNAPSHOT.lock();
            snap.status = status;
            snap.nets = Vec::new();
            snap.present = true;
        });
        return;
    }

    status[0..4].copy_from_slice(&(state as u32).to_le_bytes());
    status[4..8].copy_from_slice(&ip);
    status[8..12].copy_from_slice(&w.dhcp_mask);
    status[12..16].copy_from_slice(&w.dhcp_router);
    status[16..20].copy_from_slice(&w.dhcp_dns);
    status[20] = ssid.len() as u8;
    status[21..21 + ssid.len()].copy_from_slice(ssid);

    let mut nets = Vec::new();
    for net in w.networks().iter() {
        let name = &net.ssid[..net.ssid_len as usize];
        if name.is_empty() {
            continue; // hidden SSID: nothing for the user to click
        }
        let mut e = [0u8; 44];
        e[..name.len()].copy_from_slice(name);
        e[32] = net.ssid_len;
        e[33] = net.channel;
        e[34] = net.band;
        e[35] = (net.secure as u8)
            | ((net.rsn as u8) << 1)
            | (((!ssid.is_empty() && ssid == name) as u8) << 2);
        e[36..42].copy_from_slice(&net.bssid);
        nets.push(e);
    }

    // Built before the lock so the only thing done under it is the handover.
    //
    // ★ Masked, because this runs inside the interrupts-ON window of a Wi-Fi syscall (a join takes
    // seconds, so it must be preemptible) while the readers — the 1 Hz tray and the 2 Hz picker — take
    // this same lock with interrupts OFF. Without the mask the timer could preempt us mid-handover
    // and the next tray poll would spin on it forever. Publishing is a bounded memcpy, so making the
    // holder non-preemptible costs nothing.
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut snap = WIFI_SNAPSHOT.lock();
        snap.status = status;
        snap.nets = nets;
        snap.present = true;
    });
}

// ============================================================================
// Userspace socket routing: which of the two stacks a socket lives in.
// ============================================================================
//
// There are two independent smoltcp stacks here (wired rtl8168 and WiFi), and until now the socket
// syscalls could only ever reach the wired one. On a laptop whose only working link is WiFi that
// meant userspace had no network at all. A socket therefore records the stack it was created in and
// every later operation is routed back to it — a socket must never be looked up in the other set,
// because SocketHandles are indices and would silently alias an unrelated socket.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NetStack {
    Wired,
    Wifi,
}

/// True once `init_wifi_iface` has published an interface. A lock-free flag rather than probing
/// `WIFI_IFACE`, so the (very hot) routing decision never has to touch a lock that a ten-second
/// join might be holding.
static WIFI_IFACE_UP: AtomicBool = AtomicBool::new(false);

/// Bumped every time a WiFi interface is created. `teardown_wifi_iface` destroys the whole
/// `SocketSet`, so handles held by userspace outlive the set they refer to; after a reconnect a
/// fresh set would hand out the SAME indices to different sockets. A socket carries the generation
/// it was created in, and a mismatch means "this connection died with the link" — without it a
/// read on a stale fd would quietly return another program's data.
static WIFI_IFACE_GEN: AtomicU64 = AtomicU64::new(0);

pub fn wifi_iface_gen() -> u64 {
    WIFI_IFACE_GEN.load(Ordering::Relaxed)
}

/// The stack a newly created socket should live in, plus its generation. WiFi wins when it is up:
/// it is the link that has a default route on this machine.
pub fn active_stack() -> (NetStack, u64) {
    if WIFI_IFACE_UP.load(Ordering::Relaxed) && wifi_radio_on() {
        (NetStack::Wifi, wifi_iface_gen())
    } else {
        (NetStack::Wired, 0)
    }
}

/// Is a socket from `stack`/`gen` still backed by a live interface?
pub fn stack_alive(stack: NetStack, gen: u64) -> bool {
    match stack {
        NetStack::Wired => true,
        NetStack::Wifi => WIFI_IFACE_UP.load(Ordering::Relaxed) && wifi_iface_gen() == gen,
    }
}

/// Run `f` against a stack's `SocketSet` and `Interface`, taking the locks with the interrupt
/// discipline that stack requires. `None` means the stack isn't up (or the WiFi link went away).
///
/// ★ The two halves have OPPOSITE rules, which is the whole reason this helper exists — see the
/// discipline notes at the top of this file:
///   - **Wired** locks are shared with `poll_network_locked`, which runs masked. We mask too, so an
///     interrupts-off syscall only ever waits out a bounded critical section.
///   - **WiFi** locks are held by the idle task WITH INTERRUPTS ON, so blocking on one from a
///     syscall (which runs at IF=0) is a hard deadlock: the holder can only be resumed by the timer,
///     and the waiter has the timer masked. We therefore enable interrupts for the acquisition and
///     restore the caller's IF afterwards, making both parties preemptible.
pub fn with_stack<R>(
    stack: NetStack,
    gen: u64,
    f: impl FnOnce(&mut SocketSet<'static>, &mut Interface) -> R,
) -> Option<R> {
    match stack {
        NetStack::Wired => x86_64::instructions::interrupts::without_interrupts(|| {
            let mut iface_lock = NET_IFACE.lock();
            let mut sockets_lock = GLOBAL_SOCKETS.lock();
            match (sockets_lock.as_mut(), iface_lock.as_mut()) {
                (Some(s), Some(i)) => Some(f(s, i)),
                _ => None,
            }
        }),
        NetStack::Wifi => {
            if !stack_alive(NetStack::Wifi, gen) {
                return None;
            }
            let had = x86_64::instructions::interrupts::are_enabled();
            if !had {
                x86_64::instructions::interrupts::enable();
            }
            // Same order poll_wifi uses (iface before sockets); it only ever try_locks, but keeping
            // one global order means a future blocking caller can't invert it into a cycle.
            let out = {
                let mut iface_lock = WIFI_IFACE.lock();
                let mut sockets_lock = WIFI_SOCKETS.lock();
                match (sockets_lock.as_mut(), iface_lock.as_mut()) {
                    (Some(s), Some(i)) => Some(f(s, i)),
                    _ => None,
                }
            };
            if !had {
                x86_64::instructions::interrupts::disable();
            }
            out
        }
    }
}

/// `with_stack` for operations that need only the socket set.
///
/// Deliberately NOT `with_stack(.., |s, _| ..)`: the wired set is usable before its interface has a
/// DHCP lease, and creating a socket must keep working in that window (it is also where the wired
/// set gets lazily created, matching the original `sys_socket`). Requiring an `Interface` here would
/// make `socket()` fail on an unconfigured wired link.
pub fn with_sockets<R>(
    stack: NetStack,
    gen: u64,
    f: impl FnOnce(&mut SocketSet<'static>) -> R,
) -> Option<R> {
    match stack {
        NetStack::Wired => x86_64::instructions::interrupts::without_interrupts(|| {
            let mut sockets_lock = GLOBAL_SOCKETS.lock();
            if sockets_lock.is_none() {
                *sockets_lock = Some(SocketSet::new(alloc::vec![]));
            }
            sockets_lock.as_mut().map(f)
        }),
        NetStack::Wifi => {
            if !stack_alive(NetStack::Wifi, gen) {
                return None;
            }
            let had = x86_64::instructions::interrupts::are_enabled();
            if !had {
                x86_64::instructions::interrupts::enable();
            }
            let out = WIFI_SOCKETS.lock().as_mut().map(f);
            if !had {
                x86_64::instructions::interrupts::disable();
            }
            out
        }
    }
}

/// Drive one stack. Used by the socket syscalls' wait loops, which must not spin on a stack that
/// nothing else is polling.
pub fn poll_stack(stack: NetStack) {
    match stack {
        NetStack::Wired => poll_network(),
        NetStack::Wifi => poll_wifi(),
    }
}

/// The DNS socket belonging to a stack. Each stack has its own, pointed at the servers from that
/// link's own DHCP lease, so a lookup is always asked of the resolver that can actually answer it.
///
/// ★ The WiFi arm must NOT be a bare `try_lock`. `poll_wifi` holds `WIFI_DNS_HANDLE` for the whole
/// of `iface.poll()`, so a lookup that happens to land during a poll would miss and be reported as
/// "there is no resolver" — a coin-flip failure that looks exactly like a dead link. Block for it,
/// but with the WiFi discipline (enable IF for the acquisition, restore the caller's IF), because
/// the holder is the preemptible idle task and waiting at IF=0 would deadlock outright.
pub fn dns_handle(stack: NetStack) -> Option<SocketHandle> {
    match stack {
        NetStack::Wired => {
            x86_64::instructions::interrupts::without_interrupts(|| *DNS_HANDLE.lock())
        }
        NetStack::Wifi => {
            if !WIFI_IFACE_UP.load(Ordering::Relaxed) {
                return None;
            }
            let had = x86_64::instructions::interrupts::are_enabled();
            if !had {
                x86_64::instructions::interrupts::enable();
            }
            let out = *WIFI_DNS_HANDLE.lock();
            if !had {
                x86_64::instructions::interrupts::disable();
            }
            out
        }
    }
}

/// Sockets whose owning process died while their `SocketSet` was locked by someone else.
///
/// Process teardown runs with interrupts DISABLED on purpose ("don't get buried alive"), so it can
/// neither block on the WiFi locks (deadlock — the holder needs the timer to finish) nor enable
/// interrupts to take them safely (it would risk being descheduled mid-teardown). Deferring is the
/// way out: the pollers already hold the right lock with the right discipline, so they do the free.
/// Without this a browser's connections would leak 64 KiB of smoltcp buffers each.
static REAP_QUEUE: Mutex<Vec<(NetStack, u64, crate::scheduler::SocketKind)>> = Mutex::new(Vec::new());

/// Free a socket now if its stack's lock is free, otherwise queue it for the poller.
///
/// Safe to call at any interrupt state — it never blocks on a WiFi lock. The wired path *does* block,
/// but under `without_interrupts`, which is the discipline `poll_network_locked` already follows, so
/// its holder is never preemptible and the wait is bounded.
pub fn destroy_socket_deferred(stack: NetStack, gen: u64, kind: crate::scheduler::SocketKind) {
    let done = match stack {
        NetStack::Wired => x86_64::instructions::interrupts::without_interrupts(|| {
            match GLOBAL_SOCKETS.lock().as_mut() {
                Some(s) => {
                    free_in(s, &kind);
                    true
                }
                None => true, // no set at all: nothing to free
            }
        }),
        NetStack::Wifi => {
            if !stack_alive(NetStack::Wifi, gen) {
                true // the whole SocketSet is already gone
            } else {
                match WIFI_SOCKETS.try_lock() {
                    Some(mut g) => match g.as_mut() {
                        Some(s) => {
                            free_in(s, &kind);
                            true
                        }
                        None => true,
                    },
                    None => false, // busy — defer
                }
            }
        }
    };
    if !done {
        x86_64::instructions::interrupts::without_interrupts(|| {
            REAP_QUEUE.lock().push((stack, gen, kind));
        });
    }
}

fn free_in(sockets: &mut SocketSet<'static>, kind: &crate::scheduler::SocketKind) {
    match *kind {
        crate::scheduler::SocketKind::Tcp(h) => {
            sockets.get_mut::<smoltcp::socket::tcp::Socket>(h).abort(); // send TCP RST
            sockets.remove(h);
        }
        crate::scheduler::SocketKind::Udp(h) => {
            sockets.remove(h);
        }
    }
}

/// Free any deferred sockets belonging to `stack`. Called by the pollers with `sockets` already held.
fn drain_reaps(stack: NetStack, gen: u64, sockets: &mut SocketSet<'static>) {
    // Masked and bounded: this lock is reached from process teardown at IF=0 as well as from here,
    // so it must never be held across a preemption point (the WIFI_SNAPSHOT discipline).
    let mine = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut q = REAP_QUEUE.lock();
        if q.is_empty() {
            return Vec::new();
        }
        let mut mine = Vec::new();
        let mut keep = Vec::new();
        for e in q.drain(..) {
            // A stale generation means that whole SocketSet is gone; drop the entry, don't requeue.
            if e.0 == stack && e.1 == gen {
                mine.push(e);
            } else if e.0 == stack {
                // same stack, dead generation: nothing to free
            } else {
                keep.push(e);
            }
        }
        *q = keep;
        mine
    });
    for (_, _, kind) in mine.iter() {
        free_in(sockets, kind);
    }
}

/// Bring the WiFi link up as an IP interface, using the lease the driver already negotiated with
/// its own DHCP exchange. Configuring statically (rather than adding a smoltcp DHCP socket) avoids
/// two DHCP clients bidding for the same MAC, and means any traffic that flows is proof the
/// 802.11⟷802.3 translation is correct. Called once, after connect() reports a lease.
pub fn init_wifi_iface() {
    use smoltcp::iface::Config;
    use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, Ipv4Address, Ipv4Cidr};

    let mut driver_lock = WIFI_DRIVER.lock();
    let driver = match driver_lock.as_mut() { Some(d) => d, None => return };
    if !driver.is_online() {
        crate::serial_println!("[WIFI] P7: no lease — skipping interface setup.");
        return;
    }
    let (ip, mask, router, dns) = driver.lease();
    let prefix = mask.iter().map(|b| b.count_ones()).sum::<u32>() as u8;

    let mut config = Config::new();
    config.hardware_addr = Some(HardwareAddress::Ethernet(
        EthernetAddress::from_bytes(&driver.mac_addr)));
    let mut iface = Interface::new(config, driver);

    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::Ipv4(Ipv4Cidr::new(
            Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]), prefix)));
    });
    if router != [0, 0, 0, 0] {
        let _ = iface.routes_mut().add_default_ipv4_route(
            Ipv4Address::new(router[0], router[1], router[2], router[3]));
    }

    // A DNS lookup is the cheapest end-to-end proof: it needs ARP for the gateway, UDP out through
    // the encrypted link, routing, and a reply back through the GTK/PTK decrypt path.
    let server = if dns == [0, 0, 0, 0] { [8, 8, 8, 8] } else { dns };
    let servers = alloc::vec![smoltcp::wire::IpAddress::Ipv4(
        Ipv4Address::new(server[0], server[1], server[2], server[3]))];
    let mut sockets = SocketSet::new(alloc::vec![]);
    let dns_handle = sockets.add(DnsSocket::new(&servers[..], alloc::vec![]));
    WIFI_DNS_TRIES.store(0, Ordering::Relaxed);   // P8: reset per connect, not per boot

    {
        let sock = sockets.get_mut::<DnsSocket>(dns_handle);
        match sock.start_query(iface.context(), "example.com",
                               smoltcp::wire::DnsQueryType::A) {
            Ok(q) => {
                *WIFI_DNS_QUERY.lock() = Some(q);
                crate::serial_println!("[WIFI] P7: resolving example.com via {}.{}.{}.{}…",
                    server[0], server[1], server[2], server[3]);
            }
            Err(e) => crate::serial_println!("[WIFI] P7: DNS query failed to start: {:?}", e),
        }
    }

    crate::serial_println!(
        "[WIFI] P7: interface up — {}.{}.{}.{}/{} gw {}.{}.{}.{}",
        ip[0], ip[1], ip[2], ip[3], prefix, router[0], router[1], router[2], router[3]);

    // New generation before publishing, so any socket created against this interface is stamped
    // with it; UP goes last, once there is actually something behind the flag.
    WIFI_IFACE_GEN.fetch_add(1, Ordering::Relaxed);
    *WIFI_DNS_HANDLE.lock() = Some(dns_handle);
    *WIFI_SOCKETS.lock() = Some(sockets);
    *WIFI_IFACE.lock() = Some(iface);
    WIFI_IFACE_UP.store(true, Ordering::Release);
}

/// P8: drop the WiFi IP interface. Called when the link goes away (disconnect / network switch) so
/// smoltcp stops polling a device whose lease no longer exists.
pub fn teardown_wifi_iface() {
    // Down first: this stops new sockets being routed here, and makes every socket already stamped
    // with this generation report a dead link rather than indexing into a set that is about to go.
    WIFI_IFACE_UP.store(false, Ordering::Release);
    *WIFI_DNS_QUERY.lock() = None;
    *WIFI_DNS_HANDLE.lock() = None;
    *WIFI_SOCKETS.lock() = None;
    *WIFI_IFACE.lock() = None;
    crate::serial_println!("[WIFI] P8: IP interface torn down.");
}

/// Drive the WiFi interface. Mirrors poll_network() but touches only the WIFI_* statics, so it can
/// never deadlock against or disturb the wired path.
///
/// ★ Acquires every one of its locks with try_lock and gives up on the first miss, which makes it
/// safe to call from ANY context, at any interrupt state. That matters more than it looks: this is
/// reached from poll_network(), which the socket read/write and DNS syscall arms call directly with
/// interrupts still disabled. A blocking acquire on any of these would let an ordinary socket read
/// wedge the machine against the idle task holding the same lock — the deadlock described at the
/// top of this file. A skipped poll costs nothing; the idle task retries within a millisecond.
pub fn poll_wifi() {
    // Radio off: the interface is already gone, so the work below would find nothing anyway. Bailing
    // explicitly keeps the idle task off the driver lock entirely while the switch is off.
    if !wifi_radio_on() {
        return;
    }

    // A connect or scan holds the driver for SECONDS. Blocking here would spin a core for the whole
    // join instead of halting, even in the cases where it wouldn't deadlock outright.
    let mut driver_lock = match WIFI_DRIVER.try_lock() {
        Some(g) => g,
        None => return,
    };
    let (mut iface_lock, mut sockets_lock, mut dns_lock) =
        match (WIFI_IFACE.try_lock(), WIFI_SOCKETS.try_lock(), WIFI_DNS_HANDLE.try_lock()) {
            (Some(i), Some(s), Some(d)) => (i, s, d),
            _ => return,
        };

    if let (Some(driver), Some(iface), Some(sockets), Some(dns_handle)) =
        (driver_lock.as_mut(), iface_lock.as_mut(), sockets_lock.as_mut(), dns_lock.as_mut())
    {
        // Free sockets whose process exited while this set was locked (see REAP_QUEUE).
        drain_reaps(NetStack::Wifi, wifi_iface_gen(), sockets);

        let tsc = unsafe { core::arch::x86_64::_rdtsc() };
        let tsc_mhz = crate::time::TSC_MHZ.load(Ordering::Relaxed).max(1);
        let timestamp = Instant::from_millis((tsc / (tsc_mhz * 1000)) as i64);

        let _ = iface.poll(timestamp, driver, sockets);

        // Report the DNS answer. A failure here is almost always "nothing came back", so retry a
        // few times before giving up — and print the Device counters, which say whether the link
        // is moving frames at all (the difference between "no TX", "no RX" and "RX not parsed").
        let mut query_lock = match WIFI_DNS_QUERY.try_lock() {
            Some(g) => g,
            None => return,
        };
        if let Some(q) = *query_lock {
            let sock = sockets.get_mut::<DnsSocket>(*dns_handle);
            match sock.get_query_result(q) {
                Ok(addrs) => {
                    crate::serial_println!("[WIFI] P7: *** DNS RESOLVED — INTERNET REACHABLE *** {:?}",
                        addrs.first());
                    *query_lock = None;
                }
                Err(GetQueryResultError::Pending) => {}
                Err(e) => {
                    let (seen, passed, tx) = driver.net_stats();
                    let n = WIFI_DNS_TRIES.fetch_add(1, Ordering::Relaxed) + 1;
                    crate::serial_println!(
                        "[WIFI] P7: DNS attempt {} failed ({:?}) — data frames seen={} parsed={} tx={}",
                        n, e, seen, passed, tx);
                    *query_lock = None;
                    if n < 4 {
                        match sock.start_query(iface.context(), "example.com",
                                               smoltcp::wire::DnsQueryType::A) {
                            Ok(nq) => *query_lock = Some(nq),
                            Err(se) => crate::serial_println!("[WIFI] P7: retry failed to start: {:?}", se),
                        }
                    }
                }
            }
        }
    }
}

pub fn poll_network() {
    // ★ The WIRED half runs with interrupts masked — the equivalent of spin_lock_irqsave.
    //
    // These five locks are blocked on from syscall context with interrupts OFF (the socket read,
    // socket write and DNS-resolve arms all take GLOBAL_SOCKETS directly, and reach the rest
    // through this function). Meanwhile this function is ALSO called with interrupts ON — from the
    // idle task, and from the socket arms that re-enable IF around their blocking waits. That mix
    // is precisely the deadlock described at the top of this file: a preemptible holder plus a
    // non-preemptible waiter on the same core, neither able to make progress.
    //
    // Masking here makes the holder non-preemptible instead, so an interrupts-off waiter only ever
    // waits out a bounded critical section. Fixing it in the callee rather than at the six call
    // sites means a future caller can't reintroduce the bug by forgetting. `without_interrupts`
    // saves and restores IF, so calling it from an already-masked syscall is a no-op wrapper.
    //
    // Cost: one wired smoltcp poll's worth of interrupt latency in the idle path. That is the
    // standard trade for a lock shared with non-preemptible context, and it is bounded by the
    // rtl8168 RX ring.
    x86_64::instructions::interrupts::without_interrupts(poll_network_locked);

    // ★★ The WiFi poll runs OUTSIDE that mask, deliberately. It used to be the last statement of
    // poll_network_locked, and that starved userspace to a standstill.
    //
    // Why it must not be masked: on an associated link this drains a beacon flood, and every frame
    // costs a signature scan plus an 802.11 -> 802.3 conversion. Masking it meant the idle task —
    // which calls this in a tight loop — spent essentially all of its time with IF=0, so the timer
    // never preempted it and no userspace task was ever scheduled again. The kernel stayed alive
    // (serial kept printing); the UI was simply never given the CPU. It presents as "userspace froze
    // a few seconds after I stopped touching it", because going idle is what lets the idle task run
    // back-to-back. Note this is STARVATION, not deadlock: nothing is stuck, the machine is just
    // never in a state where it can switch tasks.
    //
    // Why it is safe to leave interrupts on: poll_wifi `try_lock`s every one of its locks and bails
    // on the first miss, so it cannot block, let alone deadlock, whatever IF happens to be. And
    // nothing ever blocks on a WIFI_* lock with interrupts off — the tray and picker syscalls read
    // the masked WIFI_SNAPSHOT instead, and the scan/connect/disconnect arms enable IF before they
    // touch the driver. So a preemptible holder here closes no cycle.
    poll_wifi();
}

fn poll_network_locked() {
    let was_pending = NETWORK_PENDING.swap(false, Ordering::Acquire);

    let mut driver_lock = NET_DRIVER.lock();
    let mut iface_lock = NET_IFACE.lock();
    let mut sockets_lock = GLOBAL_SOCKETS.lock();
    let mut dhcp_lock = DHCP_HANDLE.lock();
    let mut dns_lock = DNS_HANDLE.lock();
    
    if sockets_lock.is_none() {
        let mut sockets = SocketSet::new(alloc::vec![]);
        let dhcp_socket = DhcpSocket::new();
        *dhcp_lock = Some(sockets.add(dhcp_socket));

        // Create the DNS Socket (Defaults to Google DNS until DHCP overwrites it)
        let dns_servers = alloc::vec![smoltcp::wire::IpAddress::Ipv4(smoltcp::wire::Ipv4Address::new(8, 8, 8, 8))];
        
        let dns_socket = DnsSocket::new(&dns_servers[..], alloc::vec![]);
        *dns_lock = Some(sockets.add(dns_socket));

        *sockets_lock = Some(sockets);
    }

    if let (Some(driver), Some(iface), Some(sockets), Some(dhcp_handle), Some(dns_handle)) = 
        (driver_lock.as_mut(), iface_lock.as_mut(), sockets_lock.as_mut(), dhcp_lock.as_mut(), dns_lock.as_mut()) {
        
        if was_pending { driver.ack_interrupt(); }

        // Free sockets whose process exited while this set was locked (see REAP_QUEUE).
        drain_reaps(NetStack::Wired, 0, sockets);

        let mut lo: u32; let mut hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let tsc = ((hi as u64) << 32) | (lo as u64);
        
        let tsc_mhz = crate::time::TSC_MHZ.load(Ordering::Relaxed);
        let time_ms = tsc / (tsc_mhz * 1000); 
        
        let timestamp = Instant::from_millis(time_ms as i64); 
        
        let _ = iface.poll(timestamp, driver, sockets);
        
        //  THE FIX: Extract DHCP config FIRST, drop the lock, THEN apply it!
        let mut dhcp_config = None;
        let mut dhcp_deconfig = false;
        
        {
            let dhcp_socket = sockets.get_mut::<DhcpSocket>(*dhcp_handle);
            match dhcp_socket.poll() {
                Some(DhcpEvent::Configured(config)) => {
                    let mut dns_servers = alloc::vec::Vec::new();
                    for s in config.dns_servers.iter() {
                        dns_servers.push(smoltcp::wire::IpAddress::Ipv4(*s));
                    }
                    dhcp_config = Some((config.address, config.router, dns_servers));
                }
                Some(DhcpEvent::Deconfigured) => {
                    dhcp_deconfig = true;
                }
                None => {}
            }
        } // <-- dhcp_socket lock is safely dropped right here!

        // Now we can safely update iface and grab the DNS socket
        if let Some((address, router, dns_servers)) = dhcp_config {
            crate::serial_println!("[DHCP] Lease Acquired!");
            crate::serial_println!("[DHCP] IP: {:?}", address);
            crate::serial_println!("[DHCP] Gateway: {:?}", router);
            crate::serial_println!("[DHCP] DNS Servers: {:?}", dns_servers);
            
            iface.update_ip_addrs(|addrs| {
                addrs.clear();
                addrs.push(smoltcp::wire::IpCidr::Ipv4(address)).unwrap();
            });
            
            if let Some(r) = router {
                iface.routes_mut().add_default_ipv4_route(r).unwrap();
            }

            if !dns_servers.is_empty() {
                let dns_sock = sockets.get_mut::<DnsSocket>(*dns_handle);
                dns_sock.update_servers(&dns_servers[..]);
            }
        } else if dhcp_deconfig {
            crate::serial_println!("[DHCP] Lease Lost. Deconfiguring.");
            iface.update_ip_addrs(|addrs| addrs.clear());
            iface.routes_mut().remove_default_ipv4_route();
        }
    }

}