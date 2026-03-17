pub mod iwlwifi;
pub mod rtl8168;

use spin::Mutex;
use smoltcp::iface::{Interface, SocketSet};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicI64, Ordering};

// Global storage for our hardware driver and network brain
pub static NET_DRIVER: Mutex<Option<rtl8168::Rtl8168Driver>> = Mutex::new(None);
pub static NET_IFACE: Mutex<Option<Interface>> = Mutex::new(None);

// A simple fake hardware timer to keep the TCP/IP stack advancing
static NET_TICKS: AtomicI64 = AtomicI64::new(0);

/// Called continuously by the background thread to process internet traffic
pub fn poll_network() {
    let mut driver_lock = NET_DRIVER.lock();
    let mut iface_lock = NET_IFACE.lock();

    // If both the driver and the interface have been initialized by the PCI bus...
    if let (Some(driver), Some(iface)) = (driver_lock.as_mut(), iface_lock.as_mut()) {
        
        // Advance time by 10 milliseconds
        let time = NET_TICKS.fetch_add(10, Ordering::SeqCst);
        let timestamp = smoltcp::time::Instant::from_millis(time);
        
        // An empty socket set (smoltcp handles basic Pings automatically without sockets!)
        let mut sockets = SocketSet::new(Vec::new());
        
        // POLL: Check the RX ring, process packets, and fire outgoing TX replies
        iface.poll(timestamp, driver, &mut sockets);
    }
}