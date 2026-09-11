# Nyx networking audit

Audited 2026-09-10, against the tree as it stood that day. Every finding cites real code. Nothing
here is hypothetical — where a claim could not be verified from the repository it is marked as such.

Severity is about *consequence on this machine*, not CVSS:

| | meaning |
|---|---|
| **CRITICAL** | silently produces wrong security outcomes, or kills the machine |
| **HIGH** | breaks real browsing, or defeats a control that exists |
| **MEDIUM** | wrong under conditions a user will actually hit |
| **LOW** | wrong under conditions they probably will not |
| **OPT** | correct, but slower or larger than it needs to be |

Status: **FIXED** in this pass, **OPEN** (recorded, deliberately not fixed), or **UPSTREAM**
(third-party, tracked for awareness).

---

## 1. Architecture

Two independent smoltcp stacks, IPv4-only, sharing one syscall surface.

```
apps/terminal                     the browser: get/links/open/back/find/reader
  └─ libs/htmltext                HTML → text, links, <base>, charset sniff   [0 deps, 40 tests]
  └─ libs/net                     URL · HTTP/1.1 · gzip · charset · DNS cache · stepped Fetch
       ├─ rustls 0.23             TLS 1.3 **and 1.2**
       │    ├─ rustls-rustcrypto  pure-Rust provider (0.0.2-alpha)
       │    ├─ rustls-webpki      X.509 parse, chain build, hostname match
       │    └─ webpki-roots       121 Mozilla roots, compiled in
       └─ std::net::TcpStream
            └─ vendor/nyx-std/sys/net_nyx.rs      PAL: syscalls 41/42/44/45/3/549/534/572
                 └─ nyx-kernel/src/interrupts.rs  socket syscalls
                      └─ smoltcp 0.9.1            TCP · UDP · ICMP · DHCPv4 · DNS
                           ├─ iwlwifi.rs          9462/JF — the only working link on this laptop
                           └─ rtl8168.rs          wired
```

HTTPS differs only in that `libs/net::Transport` wraps the `TcpStream` in `rustls::StreamOwned`
before HTTP sees it. There is no separate HTTPS path.

**Polling model.** `iface.poll()` is called from exactly two places (`drivers/net/mod.rs:626`,
`:746`), reached from the idle task (`process.rs:627`) and inline from every blocking socket
syscall. There is **no timer-driven poll and no NIC interrupt driving the stack** — WiFi runs with
`CSR_INT_MASK = 0` and has no ISR at all. In practice `enable_and_hlt` in the idle task yields a
~1 kHz poll, but only while the CPU is idle.

---

## 2. Implemented and working

| Layer | State | Notes |
|---|---|---|
| PCI discovery | ✅ | MCFG/ECAM with legacy CF8/CFC fallback (`pci.rs:197`) |
| iwlwifi 9462/JF | ✅ | firmware load → scan → WPA2 → DHCP → smoltcp |
| Ethernet | ✅ | via smoltcp `medium-ethernet` |
| ARP | ✅ | smoltcp |
| IPv4 | ✅ | smoltcp `proto-ipv4` |
| ICMP | ✅ | `socket-icmp` compiled in; no userspace ping command |
| DHCPv4 | ⚠️ | two clients, see §5 |
| Routing | ✅ | default route from DHCP, both stacks |
| UDP | ⚠️ | connect-then-write only; no `sendto`/`recvfrom` |
| TCP | ✅ | smoltcp; client only |
| DNS | ⚠️ | A records only, see §6 |
| TLS 1.3 + 1.2 | ✅ | full verification, no bypass — see §4 |
| HTTP/1.1 | ✅ | GET, headers-only, chunked, gzip, redirects |
| Terminal browser | ✅ | HTTP and HTTPS both verified on hardware |

---

## 3. CRITICAL

### C1 — smoltcp seeded with zero: predictable TCP ISNs and DNS query IDs · **FIXED**

`Config::random_seed` was never set on **any** of the three interface constructions
(`pci.rs:303`, `pci.rs:493`, `drivers/net/mod.rs:526`), leaving it at its `Default` of `0`.

smoltcp derives **TCP initial sequence numbers** and **DNS query IDs** from that seed. At zero both
are identical on every boot of every Nyx machine. The DNS consequence is the serious one: the query
ID is the primary anti-spoofing token in a UDP DNS exchange, and a predictable ID reduces cache
poisoning to winning a race, not guessing a number.

The kernel already had a real RNG (`random.rs`, RDSEED→RDRAND) and it simply was not wired here.

**Fix:** `random::seed_u64()` (`random.rs`), called at all three sites. Falls back to the weak path
rather than failing, because an unpredictable-ish seed beats a constant one and refusing to build an
interface would take the machine off the network.

### C2 — `getrandom` reported success while returning non-cryptographic bytes · **FIXED**

`random::fill()` returns `false` when it fell back to a TSC-seeded xorshift. Syscall 318
(`interrupts.rs`) **discarded that boolean** and unconditionally returned the byte count. Its caller
(`libs/net/src/rng.rs`) checks only the count, so weak entropy would reach rustls — which draws
ECDHE private keys, the client random and GCM nonces from exactly this call — with nothing in the
chain able to notice.

`random::is_cryptographic()` was written to guard this and **had zero callers**. The design
identified the risk correctly and the wiring was never finished.

**Fix:** a `NYX_GRND_CRYPTO` (`0x8000`) flag on syscall 318. Set, the syscall zeroes the buffer and
returns `EIO` rather than substituting weak bytes; `libs/net`'s `nyx_getrandom` sets it, so rustls
abandons the handshake instead of using guessable keys. Plain callers keep the always-succeed
behaviour deliberately — std seeds every `HashMap` through getrandom at startup and cannot cope with
a failure, so making the default path fail would make a DRNG-less machine unbootable.

Latent rather than live on the reference hardware (Comet Lake has both instructions).

---

## 4. HIGH

### H1 — `time sync` set the certificate clock from an unauthenticated header · **FIXED**

`SystemTime::now()` is the only input to certificate expiry checking. `time sync` fetched a `Date:`
header over **plain HTTP** and wrote it to the RTC — persistently. Anyone able to answer that request
could set the clock, and setting it **backwards** revives expired certificates. With no CRL or OCSP
(see U1) there is no second check to catch it.

The unauthenticated fetch is unavoidable — you cannot get the time over HTTPS when a wrong clock is
what breaks HTTPS. The *unbounded acceptance* was not.

**Fix:** a floor at the image's build time (`NYX_BUILD_UNIX`, emitted by a new
`apps/terminal/build.rs`) and a ceiling 20 years past it. This build did not exist before it was
compiled, so any "now" earlier than that is a lie. Closes the entire rollback class for one
comparison. Not authentication, and does not pretend to be — NTS or Roughtime is the real answer.

### H2 — non-HTTP links became requests to the current host · **FIXED**

`Url::join` tested only `location.contains("://")`. `mailto:`, `tel:` and `data:` have no slashes,
so they fell through to the **relative-path** branch: `mailto:a@b` became the path `/mailto:a@b`,
which was then requested from whatever host the reader was on. `libs/htmltext` numbers these links,
so `open <n>` hit it routinely.

**Fix:** `scheme_prefix()` implements RFC 3986 §3.1 scheme syntax; `parse` and `join` both report
`ParseError::UnsupportedScheme(name)`. A colon in a *later* path segment is still a path, and `./`
still escapes a first-segment colon, matching browsers.

### H3 — request target sent unencoded · **FIXED**

`request_line` interpolated `url.path` straight into `GET {path} HTTP/1.1`. That target is
space-delimited, so a single space produced a malformed request line — and a CR/LF in a crafted
link could inject headers.

**Fix:** `encode_path()`. Conservative: already-encoded `%` triplets pass through (re-encoding `%20`
into `%2520` asks for a different file) and reserved/sub-delimiter characters that structure a path
or query are untouched.

### H4 — sockets capped at fd 31 while `FD_MAX` is 256 · **FIXED**

`sys_socket` scanned `for i in 3..32` while every other fd allocator walks `3..FD_MAX`
(`process.rs:109`). A process could hold at most **29 sockets**, and — worse — one whose low fds
were already taken by files got `EMFILE` for a socket with 224 slots free. The browser opens a
connection per request (no keep-alive), so this is a ceiling it can reach.

### H5 — ephemeral ports wrapped into the reserved range · **FIXED**

`NEXT_LOCAL_PORT.fetch_add(1)` on a `u16` wraps at 65535 to **0**, then walks up through 22, 80,
443. After ~16k sockets a connection would source from a well-known port.

**Fix:** `next_local_port()` folds the counter into 49152..=65535 (RFC 6335 dynamic range).

### H6 — `serial_println!` inside the wired NIC ISR · **FIXED**

`rtl8168_interrupt_handler` printed a line **per packet interrupt**, with interrupts masked, to an
~11 KB/s UART that this laptop has no cable for. Under any real inbound traffic the machine would
spend all its time in the handler.

**Fix:** an atomic counter (`IRQ_NET_COUNT`). Same information, one increment.

### H8 — `connect` ignored the caller's deadline · **FIXED**

`sys_connect` hardcoded a 10 s handshake deadline (`interrupts.rs`) and ignored syscall 549's
`timeout_ms`, which applies only to read/write. The PAL's `connect_timeout` was therefore
`unsupported()`.

That is a defensible default for a client holding one address, and the wrong one for a client that
walks a name's several addresses looking for one that answers: four unreachable candidates cost
**forty seconds** of blank window. Found by regression — raising `MAX_ADDR_ATTEMPTS` from 1 to 3
doubled a 20 s failure into a 40 s one, which is the symptom the multi-address work was supposed to
cure.

Capping the retry count would have treated the symptom. The root cause is that the deadline was not
the caller's to choose.

**Fix:** syscall 42 takes a fourth argument (`timeout_ms`; `0` keeps the 10 s default, so callers
predating the argument are unaffected), the PAL implements `connect_timeout` over it via a new
`sys4` helper (fourth argument in `r10`, not `rcx` — the `syscall` instruction clobbers `rcx`), and
`libs/net` uses `CONNECT_TIMEOUT = 3 s`. Four candidates now sweep in the time one attempt used to
take, and a reachable host still connects in one round trip.

### H7 — wired stack still clamps its TCP window to one segment · **FIXED**

`rtl8168.rs` had `max_burst_size = Some(1)`. In smoltcp that is **not** a transmit hint — it clamps
the *advertised receive window* to `max_burst_size × mss` = 1460 bytes, i.e. one segment in flight
and one round trip per segment. This is the same defect measured at ~5 KB/s on a 30 Mbps link on the
WiFi path; the WiFi fix never reached the wired driver.

**Fix:** `Some(16)`, matching the driver's 16 descriptors.

---

## 5. MEDIUM

### M1 — WiFi DHCP never renews · **FIXED (option C)**

The WiFi driver's hand-written DHCP client never parsed **option 51**, so there was no lease
duration, no T1/T2 and no expiry handling: a lease was taken once at `sys_wifi_connect` and held
forever. When the server expired it, the link silently stopped routing.

**Fixed in two halves.**

*Visibility.* Options **51 (lease), 58 (T1), 59 (T2)** are now parsed — big-endian, as network byte
order requires — with the acquisition time recorded. `t1_secs()`/`t2_secs()` fall back to RFC 2131
§4.4.5's defaults (½ and ⅞ of the lease) when the server omits them, and `wifi` prints
`lease: N min left`, escalating under two minutes.

*Renewal — option C.* The WiFi socket set now carries a **smoltcp `dhcpv4::Socket`**
(`WIFI_DHCP_HANDLE`), driven from `poll_wifi` exactly as the wired path drives its own. smoltcp owns
T1/T2 internally and re-requests on schedule, so address, gateway and DNS stay current for as long
as the link is up.

★ **The two DHCP clients are sequential, never concurrent**, which is what makes this safe. The
driver still performs the first exchange over raw 802.11 frames, and that is not redundant: it is
what proves the encrypted link carries traffic **and** what calibrates `rx_desc_size` (via
`scan_rx_for_us`) — both of which must happen before an IP interface can exist at all. Then smoltcp
takes over. The original objection recorded in the code — *"two DHCP clients bidding for the same
MAC"* — applies to running them at the same time, which this does not do; the server returns the
same address to the same MAC, so the handover is invisible.

⚠️ **Behaviour change worth knowing:** `Deconfigured` now drops the address and the default route.
That is correct — an expired lease is not ours to keep using — but it is new. Previously the lease
never expired because nothing tracked it.

### M1a — smoltcp's DNS `unwrap()` panicked the kernel, twice · **FIXED**

`socket/dns.rs:588` dispatches with `cx.get_source_address(dst).unwrap()` — carrying upstream's own
*"TODO remove unwrap"* — which returns `None` whenever the interface holds no address in the
destination's family. It is a **kernel panic reached from ordinary idle-task polling**, and it fired
by two different routes:

1. a name lookup issued **before DHCP granted a lease** (guarded earlier, in `sys_dns_resolve`);
2. the M1 work above: a freshly created `dhcpv4::Socket` emits **`Deconfigured` on its first poll**
   — announcing "no lease from me yet", not reporting one lost — and the handler dutifully cleared
   the driver's static bootstrap lease while the boot-proof `example.com` query was still in flight.

★ **Guarding the syscall was never sufficient**, and that was the real lesson: the syscall is not the
only thing that starts a query. `init_wifi_iface` starts one, so does the retry inside `poll_wifi`,
and any future caller would inherit the same trap.

**Fix — gate the socket, not the callers.** `gate_dns_on_source_address` runs before *every*
`iface.poll()` on **both** stacks: with no usable IPv4 address it calls `update_servers(&[])`, and
smoltcp's own dispatch then takes the branch immediately above the unwrap —
`if pq.server_idx >= servers.len() { set Failure; continue }` — failing pending queries cleanly
through a path it already supports. No address means no DNS, which is simply true; servers are
restored when a lease configures one.

Plus `WIFI_DHCP_CONFIGURED`, so `Deconfigured` is only honoured after a real `Configured` has been
seen.

⚠️ The wired stack had the identical hole — its DHCP `Deconfigured` arm also clears the address — and
was fixed in the same pass despite never having been observed to panic.

### M2 — route installation `.unwrap()`s in interrupts-masked context · **OPEN**

`drivers/net/mod.rs:782`: `iface.routes_mut().add_default_ipv4_route(r).unwrap()`, inside
`poll_network_locked`, which runs from the idle task with interrupts masked. smoltcp's route storage
is fixed-capacity; a full table would panic the kernel. Low likelihood, fatal consequence.

### M3 — no `EINTR` in socket read/write/connect/DNS · **FIXED**

`poll` checked for a pending signal; the four blocking socket loops never did. A process parked in
a socket read could not be interrupted by anything — `kill` it and nothing happened until the fd
happened to wake up. This is also the stated blocker on the POSIX floor's Ladybird gate 3, since an
event loop is built precisely on being woken by a signal.

`signal_would_interrupt()` already existed and was already correct — it honours *disposition*, so an
IGNORED signal does not interrupt (`SIGCHLD` defaults to ignore and arrives whenever a child exits;
the naive `sigpending & !sigmask` test would make every blocking call return spurious `EINTR` in any
program that spawns processes). It simply had two callers instead of six.

**Where the check goes matters.** It sits immediately before the yield, and both the read and write
loops return on the **first** successful transfer — so at that point the transferred count is zero
and `EINTR` cannot lose data. That is what POSIX requires: a call that has already moved bytes must
report the count, not the interruption.

Two cases are not clean, and are documented rather than hidden:

- **connect** — `EINTR` leaves the handshake *running*; POSIX says so and smoltcp agrees (the socket
  stays in `SynSent`). The caller must poll for writability, not re-`connect`. Reported anyway,
  because a connect that cannot be interrupted is a process that cannot be killed for 10 s.
- **DNS** — `sys_dns_resolve` returns a packed address with 0 for failure and has no errno channel,
  so it cancels the query and reports failure. Cancelling is not optional: a query left `Pending` is
  not merely a leaked slot, smoltcp retransmits it forever and every poll walks the list.

⚠️ **This made `ErrorKind::Interrupted` reachable in userspace for the first time**, and `libs/net`
would have treated it as fatal — turning any signal into a failed page load. `would_block()` and the
five blocking read loops in `http.rs` now retry on it, which is the convention every `Read`
implementation follows.

### M4 — non-contiguous netmasks accepted · **OPEN**

WiFi prefix length is `popcount(mask)` (`mod.rs:530`), which accepts `255.0.255.0` as `/16`.

---

## 6. DNS

Kernel-side, smoltcp's `socket-dns`. Query construction, transaction IDs, retransmission and
malformed-response rejection are smoltcp's; we supply servers and consume answers.

| Property | State |
|---|---|
| A records | ✅ |
| **AAAA** | ❌ — hardcoded `DnsQueryType::A` at all three sites |
| Multiple answers | ✅ **FIXED this pass** — see below |
| CNAME | ✅ resolved by smoltcp before the answer surfaces |
| TTL | ❌ discarded; userspace cache uses a fixed 10-minute TTL |
| Transaction ID validation | ✅ smoltcp — but see C1, the IDs were predictable |
| Retries / multiple servers | ✅ smoltcp; WiFi supplies only the **first** DHCP-provided server |
| Malformed responses | ✅ smoltcp rejects; we never parse wire format ourselves |
| Cache | ✅ `libs/net/src/dns.rs`, 32 entries, 10 min |

### D1 — one address per name (fixed in the prior session, recorded here)

`sys_dns_resolve` returned **one** IPv4 and discarded the rest, because its ABI is a single packed
`u64`. A client holding one address has no fallback when that address is unreachable — which for a
large site is routine. This was the true cause of a full session of "HTTPS is broken": every symptom
(connect timeouts, handshakes that completed then stalled, "it worked an hour ago") was one
unreachable address with nowhere to fall back to.

Fixed by syscall **572** returning up to 4 addresses, a cache that stores the set, and
`dns::forget_addr` dropping only the address that failed so the next attempt advances.

### D2 — IPv6 absent at every layer · **OPEN, by decision**

`proto-ipv6` is not in smoltcp's feature list (`nyx-kernel/Cargo.toml:52`). No IPv6 address, route,
or AAAA query exists anywhere in the kernel. The std PAL has v6 stubs
(`net_nyx.rs:507 join_multicast_v6`) over a v4-only kernel.

**Current behaviour is the required one**: the browser degrades to IPv4 and never hangs waiting for
a v6 answer, because it never asks for one. Documented as a limitation, not a bug.

---

## 7. TLS

**The headline: there is no certificate-verification bypass anywhere in this repository.** A
repo-wide search for `dangerous`, `ServerCertVerifier`, `set_certificate_verifier`, `insecure`,
`NoCertificateVerification` and `danger_accept` finds only English prose and unrelated musl
constants. `.with_root_certificates(roots)` resolves to the stock `WebPkiServerVerifier` with full
chain building, expiry and hostname checking against 121 Mozilla roots.

| Property | State |
|---|---|
| TLS 1.3 | ✅ all three suites |
| TLS 1.2 | ✅ enabled (six ECDHE suites) — see L1 |
| SNI | ✅ from the URL host, same value the verifier checks |
| ALPN | ❌ unset — correct for an HTTP/1.1-only client |
| Chain validation | ✅ |
| Hostname verification | ✅ |
| Expiry / not-before | ✅ (and see H1) |
| Revocation (CRL/OCSP) | ❌ `new_without_revocation` — see U1 |
| 0-RTT early data | ✅ off |
| Key logging | ✅ off |
| Session resumption | ⚠️ silently **on**, 256 entries — see L2 |
| RNG | ✅ RDSEED→RDRAND, now fails closed (C2) |
| Custom CA | ❌ no mechanism — also no trust-injection surface |

### U1 — no revocation checking · **UPSTREAM**

`WebPkiServerVerifier::new_without_revocation`. rustls-webpki has CRL support (unfed) and no OCSP
module at all, so stapled responses are not consulted either. A revoked-but-unexpired certificate is
accepted. Genuine gap for a browser; not fixable without an upstream OCSP implementation.

### U2 — `confidentiality_limit: u64::MAX` on all AES-GCM suites · **UPSTREAM**

`rustls-rustcrypto` sets this on all nine suites; rustls's own ring provider uses `1 << 24` for
AES-GCM. rustls consumes the value to drive TLS 1.3 key update or a TLS 1.2 close, so at `u64::MAX`
neither ever fires. Correct for ChaCha20-Poly1305, wrong for AES-GCM. Practical exposure here is
negligible — `MAX_BODY` is 8 MB and one request per connection is ~2⁹ records against a 2²⁴ limit.

### L1 — described as a "TLS 1.3 client"; TLS 1.2 is fully enabled · **OPEN**

`libs/net/Cargo.toml:5` and `lib.rs:1` say TLS 1.3. `with_safe_default_protocol_versions()` plus the
`tls12` feature enables both. Not a vulnerability — the 1.2 suites are ECDHE-only AEAD, no CBC, no
static RSA — but the documentation is wrong. Restricting to 1.3 is a one-line change and would drop
a small number of reachable servers.

### L2 — session resumption silently enabled · **OPEN**

`Resumption::default()` gives a 256-entry in-memory session cache, an unintended consequence of the
config being a process-wide `OnceLock`. Contrary to the stated one-request-per-connection design.
Net positive for speed; a cross-request linkability surface.

### L3 — rustls `logging` feature disabled · **OPEN**

Its `error!`/`warn!` macros compile to nothing, including *"traffic keys exhausted, closing
connection to prevent security failure"*. Hurts incident diagnosis.

### I1 — `rsa 0.9.10` carries RUSTSEC-2023-0071 (Marvin timing attack) · **INFO**

Will fire on `cargo audit`. **Not exploitable here**: it affects private-key operations, and this
client does public-key verification only (`with_no_client_auth()`). Recorded so it is not
re-litigated.

### I2 — two `rustls-webpki` versions compiled in · **INFO**

0.103.x does the parsing; 0.102.8 is pulled by `rustls-rustcrypto` for `alg_id` DER constants only.
Binary-size waste, not a correctness issue.

### I3 — the two lockfiles disagree · **INFO**

`apps/terminal` is in the workspace `exclude` list and has its own `Cargo.lock`. **That one governs
the shipped binary** (rustls 0.23.42, webpki-roots 1.0.9); the workspace lock used for `cargo test`
resolves differently (0.23.36, 1.0.5). Host tests therefore do not exercise the exact versions that
ship.

---

## 8. HTTP

| Feature | State |
|---|---|
| GET | ✅ |
| Headers-only fetch | ✅ `head_only()` — used by `time sync` |
| HTTP/1.1, Host, User-Agent | ✅ |
| Content-Length | ✅ |
| Transfer-Encoding: chunked | ✅ incl. extensions and trailers |
| Content-Encoding: gzip | ✅ ~2.8× measured |
| Charset decoding | ✅ header, then `<meta>`, then UTF-8; cp1252 for latin-1 |
| Redirects 301/302/303/307/308 | ✅ max 8, both paths |
| Status codes | ✅ surfaced; 4xx/5xx bodies render, as a browser should |
| Keep-alive | ❌ `Connection: close` only — see O1 |
| Cookies | ❌ absent |
| POST / forms | ❌ absent |

Limits: `MAX_BODY` 8 MB, `MAX_HEADERS` 64 KB, **`MAX_HEADER_FIELDS` 128 (new)**, `MAX_REDIRECTS` 8,
`TOTAL_DEADLINE` 60 s blocking / 45 s stepped, **`MAX_URL_LEN` 8192 (new)**,
**`htmltext::MAX_LINKS` 4096 (new)**, `MAX_PAGE_CHARS` 400 000, gzip bounded by
`decompress_to_vec_with_limit`.

---

## 9. Performance

### O1 — no connection reuse · **FIXED**

`Connection: close`, one request per connection. Every `open <n>` on the same host paid a fresh DNS
lookup, TCP handshake and **full TLS handshake** — the last of which means a software certificate
chain verification on this machine. Following five links on a page at ~50 ms RTT was roughly
750 ms of pure handshake plus five signature verifications, all avoidable.

**Fix:** a single-slot idle connection keyed on `(scheme, host, port)`, 15 s maximum idle age. One
slot rather than a pool: the browser fetches one page at a time, and the case worth optimising is
following a link on the page already open. Every additional slot is more state that can
desynchronise.

Three rules make it safe, and they are the whole design:

1. **Only a *framed* response may be reused** (`response_is_reusable`). `Content-Length` or
   `chunked` — never read-to-EOF. A body delimited only by the close has no length, so "the body
   ended" and "the peer paused" are the same observation, and guessing wrong hands the remainder to
   the next request as if it were that request's response. `Fetch::finish` takes a `framed` flag
   that is true only when the framing was satisfied, not when the peer hung up.
2. **`Connection: close` from the server is obeyed.**
3. **A reused connection that EOFs having produced nothing is retried once on a fresh one.** This is
   the unavoidable keep-alive race — the server closed it while idle and only told us by hanging up
   on our request. A GET is idempotent so replaying is safe; bounded to one retry so a server that
   closes everything cannot loop us.

The kept connection is also dropped on a Wi-Fi join, alongside the DNS cache: a socket opened on the
previous network will not work on this one.

Tested: 4 host tests, and the framing guard is **mutation-verified** — making
`response_is_reusable` ignore `framed` fails `only_a_framed_body_leaves_a_connection_reusable`.

### O2 — per-frame heap allocation on the WiFi datapath · **OPEN**

RX builds a fresh `Vec` per frame and copies the payload **byte at a time** with `read_volatile`
(`iwlwifi.rs:3700`). TX allocates **twice** — once in `TxToken::consume`, once in `tx_ethernet`. The
wired driver has an `RX_BUFFER_POOL` for RX but still allocates per TX frame, and the pool is never
pre-populated, so early frames allocate anyway.

### O3 — the stack only runs when the CPU is idle or a syscall is in flight · **OPEN**

No timer-driven poll. A busy userspace task with no socket syscall in flight stalls ARP,
retransmission and the DHCP state machine.

### O4 — `tx_ethernet`'s failure return is discarded · **OPEN**

`TxToken::consume` ignores the `bool`. A failed transmit is invisible to smoltcp, which believes the
frame went out.

### O5 — RTL8168 bind duplicated between the two PCI scanners · **OPEN**

~60 lines copy-pasted (`pci.rs:239`, `:411`). The WiFi bind was factored out and has a double-bind
guard; the wired one has neither.

---

## 10. Dependencies

| Crate | Version (shipped) | Why | `no_std`? |
|---|---|---|---|
| smoltcp | 0.9.1 | TCP/IP | yes, `default-features = false` |
| rustls | 0.23.42 | TLS | `std` only |
| rustls-rustcrypto | 0.0.2-**alpha** | pure-Rust provider; ring/aws-lc are C/asm and there is no C toolchain for this target | yes |
| rustls-webpki | 0.103.13 | X.509 | yes |
| webpki-roots | 1.0.9 | 121 roots, ~400 KB source → ~250–300 KB `.rodata` | yes |
| miniz_oxide | 0.8.9 | gzip; already in-tree for PNG | yes |
| getrandom | 0.2.17 | `custom` backend → syscall 318 | yes |

`ring` appears in both lockfiles via `tools/runner` → `ovmf-prebuilt` → `ureq`. That is a host-only
QEMU launcher; under resolver v2 the graphs do not unify and `ring` never reaches `target_os=nyx`.

**The alpha provider is the main dependency risk.** Its verification paths are clean — `verify.rs`,
`verify/{ecdsa,rsa,eddsa}.rs`, `hash.rs`, `kx.rs` contain zero `panic!`/`todo!`/`unwrap`, every
failure is `map_err(|_| InvalidSignature)` — but its AEAD/HMAC key setup uses `.unwrap()` under
`panic = "abort"`, and it has the U2 defect above. Cipher coverage is complete for mainstream
HTTPS: TLS 1.3 ×3, TLS 1.2 ECDHE ×6, RSA PKCS#1 + PSS, ECDSA P-256/P-384, Ed25519, X25519/P-256/P-384.

---

## 11. Tests

Before this pass: **zero networking tests**, kernel or userspace, beyond URL/header string parsing.

Now, all host-side (`cargo test -p nyx-net -p nyx-htmltext`, no hardware):

- **`libs/net/src/url.rs` — 24 tests.** The full Phase 8 table, plus every defect in H2/H3:
  non-navigable schemes, first-segment colons, userinfo, case folding, percent-encoding, port 0,
  length cap, `..` normalisation.
- **`libs/net/src/http.rs` — 21 tests.** Framing driven through a `Dribble` reader that returns
  *n* bytes per call, swept from n=1 upward — the case where a terminator or chunk-size line
  straddles two reads is where framing bugs live. Covers Content-Length, chunked (+ extensions,
  + trailers), read-to-EOF, TE-over-CL precedence, truncation, bad chunk sizes, header byte and
  count limits, oversized bodies, non-HTTP status lines, and deadline enforcement inside the loops.
- **TLS posture test.** Asserts the root store is non-empty, ALPN unset, SNI on, early data off, and
  the provider's suite/group/algorithm counts. Verified by mutation: deleting the
  `roots.extend(...)` line makes it fail with *"root store looks empty (0) — every certificate would
  fail"*.
- **`libs/net/src/body.rs`, `dns.rs`, `libs/htmltext` — 40 tests**, gzip/charset/cache/rendering.

**Still absent:** any kernel-side test, and any test that completes a real TLS handshake. The
upstream `rustls-rustcrypto` ships a `tests-external/badssl.rs` negative-path suite (expired /
wrong-host / self-signed) that is not compiled because the crate comes from crates.io. Wiring an
equivalent is the highest-value test still missing.

---

## 12. Recommended order from here

1. **WiFi DHCP lease renewal** (M1) — the only thing that breaks a long session outright.
2. **Connection reuse** (O1) — largest performance win; the new framing tests are its prerequisite.
3. **Kernel network tests** — nothing below the syscall boundary is tested at all.
4. **`EINTR` in socket loops** (M3) — also the POSIX floor's blocker.
5. **A negative-path TLS test** — prove rejection, not just acceptance.
6. **IPv6** (D2) — a project in itself; only after the above.

---

## 12b. Cross-reference against the reference implementations

See [`linux-cross-reference.md`](linux-cross-reference.md). Three findings change open items here:

- **A-MSDU (O-series concern) is largely a non-issue.** The firmware deaggregates in hardware and
  reports `IWL_RX_MPDU_AMSDU_SUBFRAME_IDX` / `LAST_SUBFRAME` per subframe, so each arrives as its own
  MPDU. Our "only the first subframe survives" theory is not supported for this path. The software
  deaggregation rules are recorded anyway, including the one we would get wrong: **the last subframe
  is not padded**.
- ★ **The RX descriptor already carries the header length** — `IWL_RX_MPDU_MFLG2_HDR_LEN_MASK`
  (`0x1f`, in 2-byte words) and `MFLG2_PAD` (`0x20`). Our parser hand-computes the header length and
  then **scans 20 bytes for the LLC/SNAP signature** to absorb what it cannot model. Using the
  descriptor removes the scan and the NOSNAP class with it — notably HT Control (+4 on a QoS frame
  with the Order bit), which the hand-computation never accounts for.
- ★ **Decryption status is in the descriptor too** — `IWL_RX_MPDU_STATUS_DECRYPTED = BIT(11)`. Every
  frame we currently drop as NOSNAP is a group-addressed frame the firmware did not decrypt; we find
  that out by searching for a signature that cannot be present. One bit-test replaces the search and
  lets the counter report *"not decrypted"* instead of *"no SNAP"*.
- We also only accept the RFC1042 LLC/SNAP (`aa aa 03 00 00 00`) and would reject the legal
  **bridge-tunnel** variant (`aa aa 03 00 00 f8`).

✅ **Resolved on hardware.** `descsz=48` confirms the v1 layout (20-byte common header + 28-byte
variant), so `mac_flags2` at descriptor +3 and `status` at +12 are correct. The first attempt still
matched zero frames — `desc=0 scan=26` — and the reason was in the numbers: `claimed=34` against
`true=28`, a difference of exactly 8. **The descriptor's header length already includes the security
header**, and adding `ccmp` on top double-counted it. One line.

★ The self-verifying design is what made that a single boot instead of an investigation: the
descriptor offset was *tried*, the signature *checked*, and the scan kept as a fallback — so a wrong
assumption cost a counter rather than lost traffic, and the counter named the error.

---

## 12a. Confirmed on hardware: SNI-based filtering upstream

**Not a Nyx defect.** Recorded because it is indistinguishable from one, cost real debugging time,
and will recur.

`get youtube.com` connects and then receives **nothing at all**:

```
tls[hs=YES]  wr=1/238B  rd=43/0B to=43  addr=192.178.173.136:443  45543 ms
```

The same address, dialled as an **IP literal** so that rustls sends no SNI extension, answers
immediately:

```
tls[hs=YES]  wr=2/224B  rd=1/1367B to=0  addr=192.178.173.136:443  9273 ms
                                          → invalid peer certificate: UnknownIssuer
```

Same IP, same port, same route. The only difference is the server name inside the ClientHello — and
the 14-bytes-smaller hello without it gets a full TLS response. `UnknownIssuer` is the expected
outcome there: without SNI, Google serves a certificate that does not chain for an IP address.

That is an on-path device reading the ClientHello's SNI and dropping the flow. Nothing on this side
can route around it; a phone succeeds on the same Wi-Fi because it reaches Google over IPv6 and with
Encrypted ClientHello, neither of which Nyx has.

★ **The diagnostic that made this a five-minute question was `sock[rd=…]` counting RAW socket bytes
rather than plaintext.** A plaintext counter reads `0` throughout a perfectly healthy TLS handshake,
so it cannot distinguish "the peer is silent" from "the peer is mid-handshake" — which are opposite
diagnoses. See §11.

---

## 13. Hardware validation

Measured on the reference machine (Wireless-AC 9462/JF, ~30 Mbps link) against the image built
2026-09-10. Figures come from the terminal's own `link:` line and `Fetch::diagnostic()`.

| Endpoint | Result | Wire | Decoded | Time | Notes |
|---|---|---|---|---|---|
| `http://example.com` | ✅ 200 | small | — | <1 s | `time sync` uses `head_only` |
| `http://google.com` | ✅ 200 | 30 KB | 86 KB | ~4 s | redirect to `www.`, **gzip 2.8×** |
| `https://google.com` | ✅ 200 | — | — | — | after multi-address fallback |
| **`https://en.wikipedia.org/wiki/Unix`** | **✅ 200** | **91 KB** | **486 KB** | **14.6 s** | **gzip 5.3×**, TLS 1.3, `tls[hs=no]` |
| `https://youtube.com` | ❌ | — | — | 45 s → 3 s | upstream SNI filtering, see §12a |
| `https://www.youtube.com` | ❌ | — | — | 3 s | connect refused, fails fast now |
| `http://nonexistent.invalid` | ✅ correct error | — | — | — | `cannot resolve …` |

Health counters during the Wikipedia fetch: `tx_dropped=0`, `runaway=0`, `OURS=0` (all frame drops
group-addressed broadcast), `read_ptr == closed`. Nothing in the driver or ring misbehaved.

### The throughput finding, and its fix

The Wikipedia fetch read 96 KB in 336 socket reads — **303 of which timed out**:

```
polls=309   sock[rd=336/96368B wr=4/456B to=303]   14576 ms   = 7 KB/s
```

303 timeouts × `SOCKET_TIMEOUT` (20 ms) ≈ 6 s, plus 309 frame sleeps × 16 ms ≈ 5 s. **Roughly 11 of
the 14.5 seconds was the client idling, not the network.**

Cause: `Stage::Body` returned the frame as soon as one read came back empty, so each poll used ~20 ms
of its 120 ms budget and then slept. An empty socket means no data arrived in the *last* 20 ms — not
that none will arrive in the next 100.

**Fix:** treat `Some(0)` as "keep asking" and let `POLL_BUDGET` end the poll. Safe only because
`SOCKET_TIMEOUT` is short; at the old 150 ms a single read would consume the whole budget. That
pairing has now caused three separate defects and is stated as a rule in §12.

⚠️ **Re-measure after this change** — the table above predates it.

### Still to measure

IPv6-capable endpoint (n/a — no IPv6), a non-default port, and a chunked response that is not also
gzipped.
