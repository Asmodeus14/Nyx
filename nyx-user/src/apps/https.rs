use alloc::string::String;
use alloc::format;
use alloc::vec; 

use crate::syscalls::{sys_socket, sys_connect, sys_write, sys_read, sys_close, sys_get_time};
use rand_core::{RngCore, CryptoRng, Error as RandError};
use embedded_tls::blocking::{TlsConnection, TlsContext};
use embedded_tls::{TlsConfig, Aes128GcmSha256, NoVerify};
use embedded_io::{Read, Write, ErrorType, Error as IoError, ErrorKind};

pub struct NyxRng { state: u64 }

impl NyxRng {
    pub fn new() -> Self {
        let mut lo: u32; let mut hi: u32;
        unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
        let seed = ((hi as u64) << 32) | (lo as u64);
        Self { state: if seed == 0 { 1 } else { seed } }
    }
}

impl CryptoRng for NyxRng {}

impl RngCore for NyxRng {
    fn next_u32(&mut self) -> u32 { self.next_u64() as u32 }
    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.state = x;
        x
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for chunk in dest.chunks_mut(8) {
            let val = self.next_u64().to_le_bytes();
            let len = chunk.len();
            chunk.copy_from_slice(&val[..len]);
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), RandError> { 
        self.fill_bytes(dest); Ok(()) 
    }
}

pub struct NyxTcpStream { fd: i64 }

#[derive(Debug)]
pub struct NyxIoError;

impl IoError for NyxIoError { fn kind(&self) -> ErrorKind { ErrorKind::Other } }

impl ErrorType for NyxTcpStream { type Error = NyxIoError; }

impl Read for NyxTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let start_time = sys_get_time();
        loop {
            let res = sys_read(self.fd, buf);
            if res > 0 { return Ok(res as usize); }
            if res == 0 { return Ok(0); }
            if res < 0 && res != -11 { return Err(NyxIoError); }
            
            // THE FIX: Breathe! Let the PCI bus trigger hardware interrupts.
            for _ in 0..10000 { unsafe { core::arch::asm!("nop"); } }
            
            // INCREASED TIMEOUT: Allow 15 seconds for heavy TLS crypto math
            if sys_get_time().wrapping_sub(start_time) > 15000 { return Err(NyxIoError); }
        }
    }
}

impl Write for NyxTcpStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        let start_time = sys_get_time();
        loop {
            let res = sys_write(self.fd, buf);
            if res >= 0 { return Ok(res as usize); }
            if res < 0 && res != -11 { return Err(NyxIoError); }
            
            // THE FIX
            for _ in 0..10000 { unsafe { core::arch::asm!("nop"); } }
            
            // INCREASED TIMEOUT: Allow 15 seconds for heavy TLS crypto math
            if sys_get_time().wrapping_sub(start_time) > 15000 { return Err(NyxIoError); }
        }
    }
    fn flush(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

#[repr(C)]
struct SockAddrIn { 
    sin_family: u16, sin_port: u16, sin_addr: [u8; 4], sin_zero: [u8; 8] 
}

pub fn run_https_fetch(url: &str) -> String {
    let mut host = url;
    let mut path = "/";
    
    if let Some(idx) = url.find('/') {
        host = &url[..idx];
        path = &url[idx..];
    }

    let ip = match crate::apps::dns::resolve(host) {
        Ok(addr) => addr,
        Err(e) => return format!("DNS Error: {}", e),
    };

    let fd = sys_socket(2, 1, 0); 
    if fd < 0 { return String::from("ERR: Failed to allocate TCP socket."); }

    let addr = SockAddrIn {
        sin_family: 2, 
        sin_port: 443u16.to_be(), 
        sin_addr: ip, 
        sin_zero: [0; 8],
    };

    sys_connect(fd, &addr as *const _ as *const u8, core::mem::size_of::<SockAddrIn>());
    let stream = NyxTcpStream { fd };
    
    // HEAP SHIELD: Prevents Stack Overflow when processing massive TLS Certificate chains
    let mut read_record_buffer = vec![0u8; 65535];
    let mut write_record_buffer = vec![0u8; 65535];
    
    let config = TlsConfig::new().with_server_name(host);
    
    let mut tls: TlsConnection<NyxTcpStream, Aes128GcmSha256> = TlsConnection::new(
        stream, 
        &mut read_record_buffer,
        &mut write_record_buffer,
    );

    let mut rng = NyxRng::new();
    
    match tls.open::<NyxRng, NoVerify>(TlsContext::new(&config, &mut rng)) {
        Ok(_) => {
            let encrypted_request = format!(
                "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: NyxBrowser/1.1\r\n\r\n", 
                path, host
            );
            
            let _ = tls.write(encrypted_request.as_bytes());
            let _ = tls.flush(); 
            
            let mut secure_response = String::new();
            let mut buf = [0u8; 16384];
            
            for _ in 0..400 {
                match tls.read(&mut buf) {
                    Ok(bytes_read) => {
                        if bytes_read > 0 {
                            secure_response.push_str(&String::from_utf8_lossy(&buf[..bytes_read]));
                            
                            if secure_response.len() > 32768 {
                                secure_response.push_str("\n\n[PAGE TRUNCATED TO 32KB TO PROTECT HEAP]");
                                break;
                            }
                        } else {
                            if secure_response.len() > 50 { break; }
                        }
                    }
                    Err(_) => break, 
                }
            }
            
            sys_close(fd);
            
            if secure_response.is_empty() {
                String::from("ERR: Server sent no data.")
            } else {
                secure_response
            }
        },
        Err(_) => {
            sys_close(fd);
            String::from("ERR: TLS Handshake failed. Connection dropped.")
        }
    }
}