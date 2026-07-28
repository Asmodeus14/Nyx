//! Nyx networking PAL — a real `std::net::TcpStream` over the kernel's socket syscalls.
//!
//! This is browser-engine blocker #3. rustls, and every HTTP client, are written against
//! `std::net::TcpStream`; until this module existed there was nothing for them to link to, even
//! though the kernel has had working TCP for a while (see `nyx_api::TcpStream`, which this
//! supersedes for std binaries).
//!
//! What the kernel actually gives us, and therefore what is and is not implemented here:
//!
//! * `socket(41)` / `connect(42)` / `read(0)` / `write(1)` / `close(3)`, plus `dns_resolve(534)`.
//!   Those back `TcpStream` and `lookup_host`.
//! * **No `listen`/`accept`, no `bind` for UDP** — so `TcpListener` and `UdpSocket` stay exactly as
//!   `sys/net/connection/unsupported.rs` has them. Nyx is a client; nothing needs to serve yet.
//! * **No IPv6 anywhere in the stack** (smoltcp is configured v4-only, and `sockaddr_in` is the only
//!   address the kernel parses). Connecting to a V6 address fails with `Unsupported` rather than
//!   pretending.
//! * **Read/write timeouts are REAL** (syscall 549). They were `Unsupported` at first, on the
//!   principle that a timeout accepted and not enforced turns "this request is slow" into "this
//!   thread is gone forever". Then a stalled TLS handshake did exactly that with nothing on screen,
//!   which is what the syscall was reserved for. The kernel keeps ONE deadline covering both
//!   directions, so setting either sets both.
//! * **No `shutdown`, no `fcntl`.** Still `Unsupported` rather than silently doing nothing.
//!   (Non-blocking mode is a partial exception: the kernel only honours `SOCK_NONBLOCK` at
//!   `socket()` time, so `set_nonblocking(false)` is a truthful no-op and `true` is refused.)
//!
//! Everything not listed above is copied verbatim from `sys/net/connection/unsupported.rs`.
use crate::fmt;
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut};
use crate::sync::atomic::{AtomicU64, Ordering};
use crate::net::{Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, SocketAddrV4, ToSocketAddrs};
use crate::sys::unsupported;
use crate::time::Duration;

// --- Nyx syscalls (Linux-compatible numbers; see nyx-kernel/src/interrupts.rs). ---
const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_CLOSE: usize = 3;
const SYS_SOCKET: usize = 41;
const SYS_CONNECT: usize = 42;
const SYS_DNS_RESOLVE: usize = 534;
const SYS_SOCKET_SET_TIMEOUT: usize = 549;

const AF_INET: usize = 2;
const SOCK_STREAM: usize = 1;

/// The only sockaddr the kernel parses. It insists on `addr_len >= 16`, which is exactly this.
#[repr(C)]
struct SockAddrIn {
    sin_family: u16,
    sin_port: u16,
    sin_addr: [u8; 4],
    sin_zero: [u8; 8],
}

#[inline]
unsafe fn sys3(n: usize, a1: usize, a2: usize, a3: usize) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a1, in("rsi") a2, in("rdx") a3,
            out("rcx") _, out("r11") _, options(nostack),
        );
    }
    ret
}

/// Kernel error returns are negative Linux errnos (-9 EBADF, -111 ECONNREFUSED, -110 ETIMEDOUT...).
#[inline]
fn err(ret: isize) -> io::Error {
    io::Error::from_raw_os_error(-ret as i32)
}

pub struct TcpStream {
    fd: usize,
    /// Remembered from `connect`, because the kernel has no `getpeername`. This is exact rather
    /// than an approximation: it is the address the connection was actually opened to.
    peer: SocketAddr,
    /// Mirrors what was pushed to the kernel, so `read_timeout()` can answer honestly. The kernel
    /// keeps one deadline for both directions; these two exist to report back what was asked.
    ///
    /// Atomics, not `Cell`: `std::net::TcpStream` is `Sync`, and the PAL type has to be too.
    /// Milliseconds, with 0 meaning `None` — matching the kernel's own sentinel.
    read_timeout: AtomicU64,
    write_timeout: AtomicU64,
}

/// Convert a caller's timeout into the kernel's millisecond form. 0 means "block forever".
fn timeout_ms(timeout: Option<Duration>) -> io::Result<u64> {
    match timeout {
        None => Ok(0),
        Some(d) => {
            // POSIX reads a zero timeout as "never block", which this kernel can only express via
            // SOCK_NONBLOCK at socket() time. Refuse rather than quietly meaning the opposite —
            // 0 is the kernel's sentinel for *forever*, so passing it through would invert intent.
            let ms = d.as_millis();
            if ms == 0 {
                return Err(io::const_error!(
                    io::ErrorKind::InvalidInput,
                    "cannot set a zero timeout",
                ));
            }
            Ok(ms.min(u64::MAX as u128) as u64)
        }
    }
}

/// Push a deadline to the kernel. One knob covers read and write (see syscall 549).
fn set_timeout(fd: usize, ms: u64) -> io::Result<()> {
    let ret = unsafe { sys3(SYS_SOCKET_SET_TIMEOUT, fd, ms as usize, 0) };
    if ret < 0 { Err(err(ret)) } else { Ok(()) }
}

fn ms_to_duration(ms: u64) -> Option<Duration> {
    if ms == 0 { None } else { Some(Duration::from_millis(ms)) }
}

impl TcpStream {
    pub fn connect<A: ToSocketAddrs>(addr: A) -> io::Result<TcpStream> {
        super::each_addr(addr, TcpStream::connect_addr)
    }

    fn connect_addr(addr: &SocketAddr) -> io::Result<TcpStream> {
        let v4 = match addr {
            SocketAddr::V4(v4) => *v4,
            SocketAddr::V6(_) => {
                return Err(io::const_error!(
                    io::ErrorKind::Unsupported,
                    "nyx has no IPv6 stack",
                ));
            }
        };

        let fd = unsafe { sys3(SYS_SOCKET, AF_INET, SOCK_STREAM, 0) };
        if fd < 0 {
            return Err(err(fd));
        }

        // ★ Own the fd BEFORE the connect below can fail. Each socket costs 64 KiB of smoltcp
        // buffers in the kernel, and a browser opens them constantly — an early return that skipped
        // the close would exhaust the kernel heap, which is the same leak the kernel's own EMFILE
        // path had to fix. With the fd in a `TcpStream`, `?` closes it via `Drop`.
        let stream = TcpStream {
            fd: fd as usize,
            peer: SocketAddr::V4(v4),
            read_timeout: AtomicU64::new(0),
            write_timeout: AtomicU64::new(0),
        };

        let sa = SockAddrIn {
            sin_family: AF_INET as u16,
            sin_port: v4.port().to_be(),
            sin_addr: v4.ip().octets(),
            sin_zero: [0; 8],
        };
        let ret = unsafe {
            sys3(
                SYS_CONNECT,
                stream.fd,
                &sa as *const SockAddrIn as usize,
                core::mem::size_of::<SockAddrIn>(),
            )
        };
        if ret != 0 {
            return Err(err(ret));
        }
        Ok(stream)
    }

    /// The kernel already gives up on a handshake after 10 s, but it will not take a caller's
    /// deadline, so this cannot honour `timeout` and says so rather than ignoring it.
    pub fn connect_timeout(_: &SocketAddr, _: Duration) -> io::Result<TcpStream> {
        unsupported()
    }

    /// ★ The kernel holds ONE deadline per socket covering both directions, so setting either of
    /// these sets both. Reported back separately only so `read_timeout()`/`write_timeout()` can
    /// return what the caller asked for; the shorter of the two is what actually applies.
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        let ms = timeout_ms(timeout)?;
        set_timeout(self.fd, ms)?;
        self.read_timeout.store(ms, Ordering::Relaxed);
        Ok(())
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        let ms = timeout_ms(timeout)?;
        set_timeout(self.fd, ms)?;
        self.write_timeout.store(ms, Ordering::Relaxed);
        Ok(())
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(ms_to_duration(self.read_timeout.load(Ordering::Relaxed)))
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        Ok(ms_to_duration(self.write_timeout.load(Ordering::Relaxed)))
    }

    /// No `MSG_PEEK` — the kernel's recv consumes from the smoltcp ring.
    pub fn peek(&self, _: &mut [u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let ret = unsafe { sys3(SYS_READ, self.fd, buf.as_mut_ptr() as usize, buf.len()) };
        if ret < 0 { Err(err(ret)) } else { Ok(ret as usize) }
    }

    pub fn read_buf(&self, mut cursor: BorrowedCursor<'_, u8>) -> io::Result<()> {
        // Read straight into the uninitialised tail. The fs PAL bounces through a 512-byte stack
        // buffer instead, which is fine for config files and ruinous for a page download.
        let ret = unsafe {
            let dst = cursor.as_mut();
            sys3(SYS_READ, self.fd, dst.as_mut_ptr() as usize, dst.len())
        };
        if ret < 0 {
            return Err(err(ret));
        }
        // SAFETY: the kernel wrote exactly `ret` bytes into the front of the cursor.
        unsafe { cursor.advance(ret as usize) };
        Ok(())
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        // No readv syscall; serve the first non-empty buffer, which is a legal short read.
        match bufs.iter_mut().find(|b| !b.is_empty()) {
            Some(buf) => self.read(buf),
            None => Ok(0),
        }
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        let ret = unsafe { sys3(SYS_WRITE, self.fd, buf.as_ptr() as usize, buf.len()) };
        if ret < 0 { Err(err(ret)) } else { Ok(ret as usize) }
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        match bufs.iter().find(|b| !b.is_empty()) {
            Some(buf) => self.write(buf),
            None => Ok(0),
        }
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        Ok(self.peer)
    }

    /// No `getsockname`: the kernel picks the ephemeral port internally and never reports it back.
    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        unsupported()
    }

    /// No half-close. `Drop` sends the FIN; there is no way to shut one direction independently.
    pub fn shutdown(&self, _: Shutdown) -> io::Result<()> {
        unsupported()
    }

    /// Needs `dup`, which the kernel's fd table does not expose to userspace.
    pub fn duplicate(&self) -> io::Result<TcpStream> {
        unsupported()
    }

    pub fn set_linger(&self, _: Option<Duration>) -> io::Result<()> {
        unsupported()
    }

    pub fn linger(&self) -> io::Result<Option<Duration>> {
        unsupported()
    }

    pub fn set_keepalive(&self, _: bool) -> io::Result<()> {
        unsupported()
    }

    pub fn keepalive(&self) -> io::Result<bool> {
        unsupported()
    }

    /// smoltcp does not coalesce small writes, so Nagle is effectively already off.
    pub fn set_nodelay(&self, nodelay: bool) -> io::Result<()> {
        if nodelay { Ok(()) } else { unsupported() }
    }

    pub fn nodelay(&self) -> io::Result<bool> {
        Ok(true)
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        unsupported()
    }

    pub fn ttl(&self) -> io::Result<u32> {
        unsupported()
    }

    /// No `SO_ERROR`; errors surface from the call that hit them.
    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        Ok(None)
    }

    /// The kernel only reads `SOCK_NONBLOCK` out of the `socket()` type argument, so this can only
    /// truthfully answer for the mode the socket is already in — which is blocking, since
    /// `connect_addr` never sets the flag.
    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        if nonblocking { unsupported() } else { Ok(()) }
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        unsafe { sys3(SYS_CLOSE, self.fd, 0, 0) };
    }
}

impl fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TcpStream").field("fd", &self.fd).field("peer", &self.peer).finish()
    }
}

/// One A record, because that is all `dns_resolve(534)` returns.
pub struct LookupHost {
    addr: Option<SocketAddr>,
}

impl Iterator for LookupHost {
    type Item = SocketAddr;
    fn next(&mut self) -> Option<SocketAddr> {
        self.addr.take()
    }
}

pub fn lookup_host(host: &str, port: u16) -> io::Result<LookupHost> {
    // A dotted-quad needs no resolver, and going to the network for one would fail on a link with
    // no DNS server even though the caller already told us the address.
    if let Ok(ip) = host.parse::<Ipv4Addr>() {
        return Ok(LookupHost { addr: Some(SocketAddr::V4(SocketAddrV4::new(ip, port))) });
    }

    let packed = unsafe { sys3(SYS_DNS_RESOLVE, host.as_ptr() as usize, host.len(), 0) } as usize;
    if packed == 0 {
        // NXDOMAIN, timeout, or no usable link — the syscall collapses all three into 0.
        return Err(io::const_error!(
            io::ErrorKind::Uncategorized,
            "failed to lookup address information",
        ));
    }

    let ip = Ipv4Addr::new(
        (packed & 0xFF) as u8,
        ((packed >> 8) & 0xFF) as u8,
        ((packed >> 16) & 0xFF) as u8,
        ((packed >> 24) & 0xFF) as u8,
    );
    Ok(LookupHost { addr: Some(SocketAddr::V4(SocketAddrV4::new(ip, port))) })
}

// ---------------------------------------------------------------------------------------------
// Below here is `sys/net/connection/unsupported.rs`, unchanged. The kernel has no listen/accept
// and no bound UDP, so there is nothing to implement yet.
// ---------------------------------------------------------------------------------------------

pub struct TcpListener(!);

impl TcpListener {
    pub fn bind<A: ToSocketAddrs>(_: A) -> io::Result<TcpListener> {
        unsupported()
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.0
    }

    pub fn accept(&self) -> io::Result<(TcpStream, SocketAddr)> {
        self.0
    }

    pub fn duplicate(&self) -> io::Result<TcpListener> {
        self.0
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.0
    }

    pub fn set_only_v6(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn only_v6(&self) -> io::Result<bool> {
        self.0
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.0
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        self.0
    }
}

impl fmt::Debug for TcpListener {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

pub struct UdpSocket(!);

impl UdpSocket {
    pub fn bind<A: ToSocketAddrs>(_: A) -> io::Result<UdpSocket> {
        unsupported()
    }

    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.0
    }

    pub fn socket_addr(&self) -> io::Result<SocketAddr> {
        self.0
    }

    pub fn recv_from(&self, _: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0
    }

    pub fn peek_from(&self, _: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.0
    }

    pub fn send_to(&self, _: &[u8], _: &SocketAddr) -> io::Result<usize> {
        self.0
    }

    pub fn duplicate(&self) -> io::Result<UdpSocket> {
        self.0
    }

    pub fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        self.0
    }

    pub fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
        self.0
    }

    pub fn read_timeout(&self) -> io::Result<Option<Duration>> {
        self.0
    }

    pub fn write_timeout(&self) -> io::Result<Option<Duration>> {
        self.0
    }

    pub fn set_broadcast(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn broadcast(&self) -> io::Result<bool> {
        self.0
    }

    pub fn set_multicast_loop_v4(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn multicast_loop_v4(&self) -> io::Result<bool> {
        self.0
    }

    pub fn set_multicast_ttl_v4(&self, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn multicast_ttl_v4(&self) -> io::Result<u32> {
        self.0
    }

    pub fn set_multicast_loop_v6(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn multicast_loop_v6(&self) -> io::Result<bool> {
        self.0
    }

    pub fn join_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        self.0
    }

    pub fn join_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn leave_multicast_v4(&self, _: &Ipv4Addr, _: &Ipv4Addr) -> io::Result<()> {
        self.0
    }

    pub fn leave_multicast_v6(&self, _: &Ipv6Addr, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn set_ttl(&self, _: u32) -> io::Result<()> {
        self.0
    }

    pub fn ttl(&self) -> io::Result<u32> {
        self.0
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        self.0
    }

    pub fn set_nonblocking(&self, _: bool) -> io::Result<()> {
        self.0
    }

    pub fn recv(&self, _: &mut [u8]) -> io::Result<usize> {
        self.0
    }

    pub fn peek(&self, _: &mut [u8]) -> io::Result<usize> {
        self.0
    }

    pub fn send(&self, _: &[u8]) -> io::Result<usize> {
        self.0
    }

    pub fn connect<A: ToSocketAddrs>(&self, _: A) -> io::Result<()> {
        self.0
    }
}

impl fmt::Debug for UdpSocket {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}
