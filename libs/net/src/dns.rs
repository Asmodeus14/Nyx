//! A userspace cache in front of the kernel's resolver.
//!
//! There is no cache anywhere below this. `sys_dns_resolve` (nyx-kernel/src/interrupts.rs) starts a
//! fresh smoltcp query and *busy-polls the stack* until the server answers or five seconds pass, and
//! it does that for every single lookup. So the cost is not merely a round trip on the radio: it is
//! a round trip during which the calling thread spins.
//!
//! Browsing hits the same host over and over — every `open <n>` on a page's own links, and every hop
//! of a redirect chain, because [`crate::fetch::Fetch`] restarts its state machine at `Stage::Resolve`
//! when it follows a `Location`. `http://x` → `https://www.x/` therefore paid for two lookups of
//! names it had just resolved.
//!
//! ## What is deliberately not cached
//!
//! Failures. A lookup that fails on this machine is usually not the name's fault — it is the link,
//! which on a laptop comes and goes. Caching a negative answer would mean a Wi-Fi reconnection did
//! not fix browsing until a timeout the user cannot see had expired, and "wait, it fixed itself" is
//! the worst possible debugging experience.
//!
//! ## The TTL
//!
//! Fixed, not read from the record: the kernel syscall returns four bytes of address and no TTL, so
//! there is nothing to honour even if we wanted to. Ten minutes is chosen against the failure mode —
//! a stale entry means a dead connection attempt, which surfaces immediately as a failed fetch and
//! is one `reload` away from being right — not against DNS correctness, which we cannot achieve from
//! here anyway.

use std::net::{IpAddr, SocketAddr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a resolved address is trusted.
const TTL: Duration = Duration::from_secs(600);

/// Entries kept. A browsing session touches a handful of hosts; beyond that the oldest goes. Small
/// enough that the linear scan below is cheaper than anything cleverer.
const CAPACITY: usize = 32;

struct Entry {
    host: String,
    /// EVERY address the resolver gave for this name, in the order it gave them.
    ///
    /// Plural, and that is the point. A name with one usable address is a name that fails outright
    /// the moment that address is unreachable — which for a large site is a routine, temporary
    /// condition, not an outage. Holding the whole set is what lets a connect failure fall through
    /// to the next candidate instead of to the user.
    addrs: Vec<IpAddr>,
    at: Instant,
}

static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());

/// The cached address for `host`, if one is still fresh.
///
/// A poisoned lock is treated as a miss. This is a cache: the correct response to not being able to
/// read it is to do the work again, never to fail the fetch.
pub fn lookup(host: &str, port: u16) -> Option<SocketAddr> {
    lookup_all(host, port).and_then(|v| v.into_iter().next())
}

/// Every cached address for `host`, freshest-first, or `None` on a miss.
pub fn lookup_all(host: &str, port: u16) -> Option<Vec<SocketAddr>> {
    let cache = CACHE.lock().ok()?;
    let e = cache.iter().find(|e| e.host == host)?;
    if e.at.elapsed() > TTL {
        return None;
    }
    Some(e.addrs.iter().map(|ip| SocketAddr::new(*ip, port)).collect())
}

/// Record every address a lookup returned.
pub(crate) fn remember_all(host: &str, addrs: Vec<IpAddr>) {
    if addrs.is_empty() {
        return;
    }
    let Ok(mut cache) = CACHE.lock() else { return };
    // Drop anything expired while we hold the lock — this is the only place that runs regularly, and
    // a cache that only ever grows would keep handing out stale entries once it hit the cap.
    cache.retain(|e| e.at.elapsed() <= TTL);

    if let Some(e) = cache.iter_mut().find(|e| e.host == host) {
        e.addrs = addrs;
        e.at = Instant::now();
        return;
    }
    if cache.len() >= CAPACITY {
        cache.remove(0);
    }
    cache.push(Entry { host: host.to_string(), addrs, at: Instant::now() });
}

/// Drop ONE address from a host's set, keeping the rest.
///
/// What a connect failure actually proves: this address did not answer. It says nothing about the
/// others, and throwing the whole entry away would send the next attempt back to the resolver to be
/// handed the same list in the same order. Removing just the failed one makes each attempt walk
/// forward through the candidates instead of retrying the first forever.
pub fn forget_addr(host: &str, addr: IpAddr) {
    if let Ok(mut cache) = CACHE.lock() {
        if let Some(e) = cache.iter_mut().find(|e| e.host == host) {
            e.addrs.retain(|a| *a != addr);
            if e.addrs.is_empty() {
                cache.retain(|e| e.host != host);
            }
        }
    }
}

/// Drop one host's entry.
///
/// For a caller that has just discovered the cached address does not work — a connect that times
/// out proves the answer is useless regardless of how fresh it is. Without this, caching turns one
/// unlucky DNS reply into ten minutes of guaranteed failure, which is strictly worse than not
/// caching at all: an uncached client would have re-resolved and quite likely got a working
/// address on the next try.
pub fn forget(host: &str) {
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|e| e.host != host);
    }
}

/// Forget everything.
///
/// Wanted after a Wi-Fi join: the new network has its own resolver and, on a captive portal or a
/// split-horizon corporate DNS, its own answers for the same names. Carrying the previous link's
/// addresses across is how a machine ends up unable to reach anything on a network that works.
pub fn flush() {
    if let Ok(mut cache) = CACHE.lock() {
        cache.clear();
    }
}

/// How many entries are held. For the terminal's `dns` command, which reports it — a cache you
/// cannot see the state of is a cache you will eventually blame for something it did not do.
pub fn cached_hosts() -> Vec<String> {
    match CACHE.lock() {
        Ok(cache) => cache
            .iter()
            .filter(|e| e.at.elapsed() <= TTL)
            .map(|e| {
                let list: Vec<String> = e.addrs.iter().map(|a| a.to_string()).collect();
                format!("{} -> {}", e.host, list.join(", "))
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip(a: u8, b: u8, c: u8, d: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(a, b, c, d))
    }

    // The cache is process-global, so these run as one test rather than racing each other under
    // cargo's thread-per-test.
    #[test]
    fn the_cache_answers_repeats_and_can_be_flushed() {
        flush();
        assert!(lookup("example.com", 443).is_none(), "a flushed cache must not answer");

        remember_all("example.com", vec![ip(93, 184, 216, 34)]);
        let hit = lookup("example.com", 443).expect("just remembered");
        assert_eq!(hit.ip(), ip(93, 184, 216, 34));

        // A name with several addresses keeps all of them, and a failed one drops out without
        // taking the others with it — that is the whole point of holding a set.
        remember_all("multi.test", vec![ip(1, 1, 1, 1), ip(2, 2, 2, 2), ip(3, 3, 3, 3)]);
        assert_eq!(lookup_all("multi.test", 80).unwrap().len(), 3);
        forget_addr("multi.test", ip(2, 2, 2, 2));
        let left: Vec<_> = lookup_all("multi.test", 80).unwrap().iter().map(|s| s.ip()).collect();
        assert_eq!(left, vec![ip(1, 1, 1, 1), ip(3, 3, 3, 3)]);
        // Losing the last address removes the name entirely rather than leaving an empty entry.
        forget_addr("multi.test", ip(1, 1, 1, 1));
        forget_addr("multi.test", ip(3, 3, 3, 3));
        assert!(lookup_all("multi.test", 80).is_none());

        // The port is not part of the identity: a name resolves to an address, and 80 and 443 are
        // the same lookup. Caching per (host, port) would halve the hit rate on any site that
        // redirects http to https.
        assert_eq!(lookup("example.com", 80).unwrap().port(), 80);

        // Re-remembering updates in place rather than growing the cache.
        remember_all("example.com", vec![ip(1, 2, 3, 4)]);
        assert_eq!(lookup("example.com", 443).unwrap().ip(), ip(1, 2, 3, 4));
        assert_eq!(cached_hosts().len(), 1);

        // Capacity is a hard bound; the oldest is evicted.
        for i in 0..CAPACITY as u16 + 5 {
            remember_all(&format!("h{i}.test"), vec![ip(10, 0, (i >> 8) as u8, i as u8)]);
        }
        assert!(cached_hosts().len() <= CAPACITY, "cache grew past its cap");

        flush();
        assert!(cached_hosts().is_empty());
    }
}

/// Ask the kernel for EVERY A record, not just the first.
///
/// `sys_dns_resolve` (534) returns one packed address because that is its whole ABI, and the std
/// PAL's `lookup_host` is built on it — so `to_socket_addrs` can never yield more than one address
/// on this target no matter how many the server sent. Syscall 572 exists to return the rest.
///
/// On the host this is `to_socket_addrs`, which already returns the full set.
#[cfg(target_os = "nyx")]
pub(crate) fn resolve_all(host: &str) -> Vec<IpAddr> {
    use std::net::Ipv4Addr;
    const SYS_DNS_RESOLVE_ALL: usize = 572;
    const MAX: usize = 4;

    let mut out = [0u32; MAX];
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_DNS_RESOLVE_ALL => ret,
            in("rdi") host.as_ptr() as usize,
            in("rsi") host.len(),
            in("rdx") out.as_mut_ptr() as usize,
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    if ret <= 0 {
        return Vec::new();
    }
    out.iter()
        .take((ret as usize).min(MAX))
        .map(|p| {
            // Same little-endian octet packing the kernel uses for 534.
            Ipv4Addr::new(
                (p & 0xFF) as u8,
                ((p >> 8) & 0xFF) as u8,
                ((p >> 16) & 0xFF) as u8,
                ((p >> 24) & 0xFF) as u8,
            )
            .into()
        })
        .collect()
}

#[cfg(not(target_os = "nyx"))]
pub(crate) fn resolve_all(host: &str) -> Vec<IpAddr> {
    use std::net::ToSocketAddrs;
    match (host, 0u16).to_socket_addrs() {
        Ok(it) => it.map(|s| s.ip()).collect(),
        Err(_) => Vec::new(),
    }
}
