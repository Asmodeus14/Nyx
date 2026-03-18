pub mod iwlwifi;
pub mod rtl8168;

use spin::Mutex;
use smoltcp::iface::{Interface, SocketSet};
use smoltcp::time::Instant;
use core::sync::atomic::{AtomicU64, Ordering};

pub static NET_DRIVER: Mutex<Option<crate::drivers::net::rtl8168::Rtl8168Driver>> = Mutex::new(None);
pub static NET_IFACE: Mutex<Option<Interface>> = Mutex::new(None);

static FAKE_CLOCK_MS: AtomicU64 = AtomicU64::new(0);

pub fn poll_network() {
    let mut driver_lock = NET_DRIVER.lock();
    let mut iface_lock = NET_IFACE.lock();
    
    if let (Some(driver), Some(iface)) = (driver_lock.as_mut(), iface_lock.as_mut()) {
        
        // Prevent Time-Warp: Divide by 100,000 because we removed the sleep loop!
        let current_time = FAKE_CLOCK_MS.fetch_add(1, Ordering::SeqCst);
        let timestamp = Instant::from_millis((current_time / 100_000) as i64); 
        
        let mut sockets: SocketSet<'static> = SocketSet::new(alloc::vec![]);
        let _ = iface.poll(timestamp, driver, &mut sockets);
    }
}