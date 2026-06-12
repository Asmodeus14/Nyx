use alloc::string::String;
use alloc::format;
use crate::syscalls::{sys_socket, sys_connect, sys_write, sys_read, sys_close};

#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

pub fn run_nyxfetch() -> String {
    let fd = sys_socket(2, 1, 0); 
    if fd < 0 {
        return String::from("ERR: Failed to allocate TCP socket.");
    }

    // We are dialing Cloudflare (1.1.1.1) on Port 80
    let addr = SockAddrIn {
        sin_family: 2, 
        sin_port: 80u16.to_be(), 
        sin_addr: [1, 1, 1, 1], 
        sin_zero: [0; 8],
    };

    let connect_res = sys_connect(fd, &addr as *const _ as *const u8, core::mem::size_of::<SockAddrIn>());
    
    if connect_res < 0 && connect_res != -11 {
        sys_close(fd);
        return format!("ERR: TCP Handshake failed. Code: {}", connect_res);
    }

    let http_request = "GET / HTTP/1.1\r\nHost: 1.1.1.1\r\nConnection: close\r\n\r\n";
    let mut sent = false;
    
    // 🚨 CLEAN Write Polling Loop - No more stolen dummy bytes!
    for _ in 0..50000 {
        if sys_write(fd, http_request.as_bytes()) > 0 {
            sent = true;
            break;
        }
        for _ in 0..200000 { unsafe { core::arch::asm!("nop"); } }
    }

    if !sent {
        sys_close(fd);
        return String::from("ERR: Handshake timed out. No SYN-ACK.");
    }

    let mut buffer = [0u8; 4096];
    let mut final_response = String::new();
    
    // Wait for the HTTP response
    for _ in 0..50000 {
        let bytes_read = sys_read(fd, &mut buffer);
        if bytes_read > 0 {
            if let Ok(response) = core::str::from_utf8(&buffer[..bytes_read as usize]) {
                final_response.push_str(response);
            }
        } else if bytes_read == 0 || bytes_read != -11 {
            if !final_response.is_empty() { break; }
        }
        for _ in 0..200000 { unsafe { core::arch::asm!("nop"); } }
    }

    sys_close(fd);
    
    if final_response.is_empty() {
        String::from("ERR: Server connected, but sent no data.")
    } else {
        final_response
    }
}