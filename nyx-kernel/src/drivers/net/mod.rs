pub mod iwlwifi;
pub mod rtl8168;

use spin::Mutex;
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::time::Instant;
use core::sync::atomic::{AtomicBool, Ordering};

pub static NET_DRIVER: Mutex<Option<crate::drivers::net::rtl8168::Rtl8168Driver>> = Mutex::new(None);
pub static NET_IFACE: Mutex<Option<Interface>> = Mutex::new(None);

pub static NETWORK_PENDING: AtomicBool = AtomicBool::new(false);

pub fn poll_network() {
    let mut driver_lock = NET_DRIVER.lock();
    let mut iface_lock = NET_IFACE.lock();
    
    if let (Some(driver), Some(iface)) = (driver_lock.as_mut(), iface_lock.as_mut()) {
        
        // FIX: Read the actual physical CPU heartbeat so smoltcp can process ARP timeouts!
        let mut lo: u32;
        let mut hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let tsc = ((hi as u64) << 32) | (lo as u64);
        let time_ms = tsc / 2_000_000; // Convert CPU cycles to approx milliseconds
        
        let timestamp = Instant::from_millis(time_ms as i64); 
        
        let mut sockets: SocketSet<'static> = SocketSet::new(alloc::vec![]);
        let _ = iface.poll(timestamp, driver, &mut sockets);
    }
}