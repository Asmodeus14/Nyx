use alloc::vec::Vec;
use alloc::string::String;
use alloc::format;
use crate::syscalls::{sys_socket, sys_connect, sys_write, sys_read, sys_close, sys_get_time};

#[repr(C)]
struct SockAddrIn {
    sin_family: u16, sin_port: u16, sin_addr: [u8; 4], sin_zero: [u8; 8]
}

pub fn resolve(domain: &str) -> Result<[u8; 4], String> {
    let fd = sys_socket(2, 2, 0); 
    if fd < 0 { return Err(String::from("Failed to allocate UDP socket")); }

    let addr = SockAddrIn {
        sin_family: 2, 
        sin_port: 53u16.to_be(), 
        sin_addr: [1, 1, 1, 1], 
        sin_zero: [0; 8],
    };

    sys_connect(fd, &addr as *const _ as *const u8, core::mem::size_of::<SockAddrIn>());

    let mut packet = Vec::new();
    packet.extend_from_slice(&[0x12, 0x34, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    
    for part in domain.split('.') {
        packet.push(part.len() as u8);
        packet.extend_from_slice(part.as_bytes());
    }
    packet.push(0); 
    packet.extend_from_slice(&[0x00, 0x01, 0x00, 0x01]);

    let mut buf = [0u8; 512];
    let mut read_len = 0;

    for _attempt in 1..=3 {
        sys_write(fd, &packet);
        let start_time = sys_get_time();

        loop {
            let res = sys_read(fd, &mut buf);
            if res > 0 { 
                read_len = res as usize; 
                break; 
            }
            if res < 0 && res != -11 {
                sys_close(fd);
                return Err(String::from("UDP Socket Hard Error"));
            }
            
            // 🚨 THE FIX: Give the Ethernet Interrupt handler time to acquire the driver lock!
            for _ in 0..10000 { unsafe { core::arch::asm!("nop"); } }

            let current_time = sys_get_time();
            if current_time.wrapping_sub(start_time) > 2000 {
                break; 
            }
        }
        if read_len > 0 { break; } 
    }

    sys_close(fd);

    if read_len < 12 { return Err(String::from("DNS Timeout: UDP Packet dropped by network.")); }

    let rcode = buf[3] & 0x0F;
    if rcode == 3 { return Err(String::from("NXDOMAIN: Domain does not exist.")); } 
    else if rcode != 0 { return Err(format!("DNS Server Error: RCODE {}", rcode)); }

    let mut idx = 12; 
    loop {
        if idx >= read_len { break; }
        let len = buf[idx];
        if len == 0 { idx += 1; break; }
        if len & 0xC0 == 0xC0 { idx += 2; break; }
        idx += (len as usize) + 1;
    }
    idx += 4; 

    if idx >= read_len { return Err(String::from("Malformed DNS Packet")); }

    let ancount = u16::from_be_bytes([buf[6], buf[7]]);
    for _ in 0..ancount {
        if idx >= read_len { break; }
        loop {
            if idx >= read_len { break; }
            let len = buf[idx];
            if len == 0 { idx += 1; break; }
            if len & 0xC0 == 0xC0 { idx += 2; break; }
            idx += (len as usize) + 1;
        }
        
        if idx + 10 > read_len { break; }
        let atype = u16::from_be_bytes([buf[idx], buf[idx+1]]);
        let dlen = u16::from_be_bytes([buf[idx+8], buf[idx+9]]) as usize;
        idx += 10;
        
        if atype == 1 && dlen == 4 && idx + 4 <= read_len {
            return Ok([buf[idx], buf[idx+1], buf[idx+2], buf[idx+3]]);
        }
        idx += dlen; 
    }
    
    Err(String::from("No A-Record (IPv4) found for this domain."))
}