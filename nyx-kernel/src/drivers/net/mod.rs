pub mod iwlwifi;
pub mod rtl8168;

use spin::Mutex;
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::time::Instant;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering}; 

pub static NET_DRIVER: Mutex<Option<crate::drivers::net::rtl8168::Rtl8168Driver>> = Mutex::new(None);
pub static NET_IFACE: Mutex<Option<Interface>> = Mutex::new(None);

// 🚨 THE FIX: A persistent, global SocketSet that the syscalls can push to!
pub static GLOBAL_SOCKETS: Mutex<Option<SocketSet<'static>>> = Mutex::new(None);

pub static NETWORK_PENDING: AtomicBool = AtomicBool::new(false);
static LAST_TIME: AtomicU64 = AtomicU64::new(0);

pub fn poll_network() {
    let mut driver_lock = NET_DRIVER.lock();
    let mut iface_lock = NET_IFACE.lock();
    let mut sockets_lock = GLOBAL_SOCKETS.lock();
    
    // 🚨 FOOLPROOF INIT: Create the empty SocketSet on the very first tick
    if sockets_lock.is_none() {
        *sockets_lock = Some(SocketSet::new(alloc::vec![]));
    }

    if let (Some(driver), Some(iface), Some(sockets)) = (driver_lock.as_mut(), iface_lock.as_mut(), sockets_lock.as_mut()) {
        
        static PRINTED_IP: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
        if !PRINTED_IP.swap(true, core::sync::atomic::Ordering::Relaxed) {
            crate::serial_println!("[NET] smoltcp configured with IPs: {:?}", iface.ipv4_addr());
        }

        let mut lo: u32;
        let mut hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let tsc = ((hi as u64) << 32) | (lo as u64);
        let time_ms = tsc / 2_000_000; 
        
        let timestamp = Instant::from_millis(time_ms as i64); 
        
        // 🚨 Poll using the persistent global sockets!
        let _ = iface.poll(timestamp, driver, sockets);
    }
}