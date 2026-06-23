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
pub static NET_IFACE: Mutex<Option<Interface>> = Mutex::new(None);
pub static GLOBAL_SOCKETS: Mutex<Option<SocketSet<'static>>> = Mutex::new(None);
pub static DHCP_HANDLE: Mutex<Option<SocketHandle>> = Mutex::new(None);

// 🔥 MILESTONE 3.2: Global handle for the DNS resolution socket
pub static DNS_HANDLE: Mutex<Option<SocketHandle>> = Mutex::new(None);

pub static NETWORK_PENDING: AtomicBool = AtomicBool::new(false);
static LAST_TIME: AtomicU64 = AtomicU64::new(0);

lazy_static::lazy_static! {
    pub static ref RX_BUFFER_POOL: Mutex<Vec<Box<[u8; 2048]>>> = Mutex::new(Vec::new());
}

pub fn poll_network() {
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
        
        let mut lo: u32; let mut hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let tsc = ((hi as u64) << 32) | (lo as u64);
        
        let tsc_mhz = crate::time::TSC_MHZ.load(Ordering::Relaxed);
        let time_ms = tsc / (tsc_mhz * 1000); 
        
        let timestamp = Instant::from_millis(time_ms as i64); 
        
        let _ = iface.poll(timestamp, driver, sockets);
        
        // 🔥 THE FIX: Extract DHCP config FIRST, drop the lock, THEN apply it!
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