use alloc::string::String;
use crate::syscalls::{sys_socket, sys_connect, sys_write, sys_read, sys_close, sys_getrandom};
use rand_core::{RngCore, CryptoRng, Error as RandError};
use embedded_tls::blocking::{TlsConnection, TlsContext};
use embedded_tls::{TlsConfig, Aes128GcmSha256, NoVerify};
use embedded_io::{Read, Write, ErrorType, Error as IoError, ErrorKind};

// --- 1. HARDWARE-BACKED RNG ---
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
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
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
        self.fill_bytes(dest); 
        Ok(()) 
    }
}

// --- 2. TCP STREAM WRAPPER ---
pub struct NyxTcpStream { fd: i64 }

#[derive(Debug)]
pub struct NyxIoError;
impl IoError for NyxIoError { fn kind(&self) -> ErrorKind { ErrorKind::Other } }
impl ErrorType for NyxTcpStream { type Error = NyxIoError; }

impl Read for NyxTcpStream {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, Self::Error> {
        // Massive loop to allow the internet time to respond
        for _ in 0..50000 { 
            let res = sys_read(self.fd, buf);
            if res > 0 { return Ok(res as usize); }
            if res == 0 { return Ok(0); } // EOF
            if res != -11 { return Err(NyxIoError); } // Error (-11 is EAGAIN)
            
            for _ in 0..200000 { unsafe { core::arch::asm!("nop"); } }
        }
        Err(NyxIoError) // Timeout
    }
}

impl Write for NyxTcpStream {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        // 🚨 CLEAN Write Polling - The kernel auto-polls now, no dummy read needed!
        for _ in 0..50000 {
            let res = sys_write(self.fd, buf);
            if res >= 0 { return Ok(res as usize); }
            if res != -11 { return Err(NyxIoError); }
            
            for _ in 0..200000 { unsafe { core::arch::asm!("nop"); } }
        }
        Err(NyxIoError)
    }
    fn flush(&mut self) -> Result<(), Self::Error> { Ok(()) }
}

// --- 3. SOCKET STRUCTURE ---
#[repr(C)]
struct SockAddrIn { 
    sin_family: u16, 
    sin_port: u16, 
    sin_addr: [u8; 4], 
    sin_zero: [u8; 8] 
}

// --- 4. MAIN HTTPS FUNCTION ---
pub fn run_https_fetch() -> String {
    let fd = sys_socket(2, 1, 0); 
    if fd < 0 { return String::from("ERR: Failed to allocate TCP socket."); }

    // Cloudflare Secure DNS Server on Port 443
    let addr = SockAddrIn {
        sin_family: 2, 
        sin_port: 443u16.to_be(), 
        sin_addr: [1, 1, 1, 1],   
        sin_zero: [0; 8],
    };

    sys_connect(fd, &addr as *const _ as *const u8, core::mem::size_of::<SockAddrIn>());

    let stream = NyxTcpStream { fd };
    
    // 16KB Buffers required by the TLS specification
    let mut read_record_buffer = [0u8; 16384];
    let mut write_record_buffer = [0u8; 16384];
    
    let config = TlsConfig::new().with_server_name("cloudflare-dns.com");
    
    let mut tls: TlsConnection<NyxTcpStream, Aes128GcmSha256> = TlsConnection::new(
        stream, 
        &mut read_record_buffer,
        &mut write_record_buffer,
    );

    let mut rng = NyxRng::new();
    
    // We pass NoVerify here because we don't have a Root CA Certificate Store built yet!
    match tls.open::<NyxRng, NoVerify>(TlsContext::new(&config, &mut rng)) {
        Ok(_) => {
            let encrypted_request = "GET / HTTP/1.1\r\nHost: 1.1.1.1\r\nConnection: close\r\n\r\n";
            let _ = tls.write(encrypted_request.as_bytes());
            
            let mut secure_response = String::from(">>> TLS HANDSHAKE SUCCESS!<<<\n");
            let mut buf = [0u8; 4096];
            
            if let Ok(bytes_read) = tls.read(&mut buf) {
                if let Ok(text) = core::str::from_utf8(&buf[..bytes_read]) {
                    secure_response.push_str(text);
                }
            }
            
            sys_close(fd);
            secure_response
        },
        Err(_) => {
            sys_close(fd);
            String::from("ERR: TLS Handshake failed (Crypto error or connection drop).")
        }
    }
}