# Nyx networking architecture

How a URL becomes pixels, and where each layer lives. Companion to
[`network-audit.md`](network-audit.md) (findings) and [`https.md`](https.md) (TLS specifics).

Nyx is not POSIX-shaped underneath. It presents `std::net::TcpStream` to userspace, but there is no
BSD socket layer — the PAL calls Nyx syscalls directly and smoltcp does TCP/IP inside the kernel.

## The path

```
  get https://example.com/page
        │
  ┌─────▼──────────────────────────────────────────────┐
  │ apps/terminal                                       │  the browser
  │   do_get → start_load → Fetch                       │  non-blocking; pumped from update()
  └─────┬──────────────────────────────────────────────┘
        │
  ┌─────▼───────────────┐        ┌──────────────────────┐
  │ libs/net::url       │        │ libs/htmltext        │  HTML → text, links, <base>
  │   scheme/host/port  │        │   40 host tests      │
  │   path encoding     │        └──────────────────────┘
  └─────┬───────────────┘
        │
  ┌─────▼───────────────┐
  │ libs/net::dns       │  userspace cache: host → up to 4 addresses, 10 min
  │   syscall 572       │
  └─────┬───────────────┘
        │
  ┌─────▼───────────────────────────────────────────────┐
  │ libs/net::fetch (stepped)  ·  libs/net::http         │
  │   Stage: Resolve → Connect → Head → Body             │
  │   framing · gzip · charset · redirects               │
  └─────┬───────────────────────────────────────────────┘
        │                      https only
  ┌─────▼───────────────┐   ┌──────────────────────────┐
  │ std::net::TcpStream │◄──┤ rustls::StreamOwned      │
  └─────┬───────────────┘   └──────────────────────────┘
        │
  ┌─────▼───────────────────────────────────────────────┐
  │ vendor/nyx-std/sys/net_nyx.rs        the PAL         │
  │   41 socket · 42 connect · 44 write · 45 read        │
  │   3 close · 549 set-timeout · 534/572 resolve        │
  └─────┬───────────────────────────────────────────────┘
        │
  ┌─────▼───────────────────────────────────────────────┐
  │ nyx-kernel/src/interrupts.rs                         │
  │   spin-yield loops: poll_stack → check → int 0x41    │
  └─────┬───────────────────────────────────────────────┘
        │
  ┌─────▼───────────────────────────────────────────────┐
  │ smoltcp 0.9.1   TCP · UDP · ICMP · DHCPv4 · DNS      │
  │   two independent stacks: WIFI_* and NET_*           │
  └─────┬───────────────────────────────────────────────┘
        │
  ┌─────▼───────────────┐   ┌──────────────────────────┐
  │ iwlwifi.rs 9462/JF  │   │ rtl8168.rs  wired        │
  └─────────────────────┘   └──────────────────────────┘
```

## Two stacks, chosen per socket

`active_stack()` (`drivers/net/mod.rs`) picks WiFi when the interface is up and the radio is on,
else wired. **The choice is made once, when the socket is created**, and recorded on the socket
along with a *generation* stamp. Teardown drops the whole `SocketSet`, and a fresh one would reuse
the same indices — the generation is what stops a stale handle from addressing a new socket.

On this laptop only the radio works; the wired NIC is present but goes nowhere.

## Threading and the poll model

There are no threads in userspace and no softirq in the kernel. `iface.poll()` runs from exactly two
call sites, reached from:

- **the idle task** (`process.rs`), which does `poll_network(); enable_and_hlt()` — roughly 1 kHz
  while the CPU is idle, and not at all while it is busy;
- **inline from every blocking socket syscall**, which loops `poll_stack → check → sti; int 0x41; cli`.

There is **no timer-driven poll**. The wired NIC has an ISR that only sets a flag; the WiFi driver
runs with `CSR_INT_MASK = 0` and has no ISR at all — it is pure polling.

Consequence: the stack advances when someone is waiting on it or the machine is idle. See O3 in the
audit.

## Blocking, and why the browser does not

Socket read/write honour a per-socket deadline (syscall 549); `connect` has a hardcoded 10 s and DNS
5 s. All of them block the calling thread.

`apps/terminal` is a windowed app whose event loop is also its repaint loop, so a blocking fetch
means a window that does not repaint and **cannot be closed** — `MSG_WINDOW_CLOSE` is not processed
while the handler is inside a syscall. `libs/net::Fetch` exists for this: a resumable state machine
polled once per frame from `NyxApp::update()`, with a short socket timeout read as "nothing yet,
come back next frame" rather than as failure.

`fetch` (the raw diagnostic command) still uses the blocking path, deliberately, and still freezes
its window. `get` does not.

## Where each concern lives

| Concern | File |
|---|---|
| URL syntax, scheme rules, percent-encoding | `libs/net/src/url.rs` |
| HTTP framing, redirects, limits, TLS config | `libs/net/src/http.rs` |
| Resumable fetch, stage machine, diagnostics | `libs/net/src/fetch.rs` |
| gzip, charset decoding | `libs/net/src/body.rs` |
| DNS cache, multi-address, `forget_addr` | `libs/net/src/dns.rs` |
| Entropy for TLS (syscall 318 + crypto flag) | `libs/net/src/rng.rs` |
| HTML → text, links, `<base>`, reader mode | `libs/htmltext/src/lib.rs` |
| PAL: sockets, DNS, timeouts | `vendor/nyx-std/sys/net_nyx.rs` |
| Socket syscalls, DNS syscall, entropy syscall | `nyx-kernel/src/interrupts.rs` |
| Interface construction, DHCP, poll, reaping | `nyx-kernel/src/drivers/net/mod.rs` |
| WiFi driver, RX ring, DHCP client | `nyx-kernel/src/drivers/net/iwlwifi.rs` |
| PCI discovery and driver binding | `nyx-kernel/src/pci.rs` |

## Syscalls

| # | Name |
|---|---|
| 41 / 42 | socket / connect |
| 44 / 45 / 3 | write / read / close |
| 7 / 23 | poll / select |
| 228 | clock_gettime — backs certificate validity |
| 318 | getrandom (`NYX_GRND_CRYPTO` = 0x8000 fails closed) |
| 534 | dns_resolve — one address |
| 549 | socket_set_timeout |
| 543–548 | wifi scan/list/connect/status/disconnect/radio |
| 550 / 552 | realtime offset / set RTC |
| 569–571 | WiFi RX counters, NOSNAP dump, RX ring state (diagnostics) |
| 572 | dns_resolve_all — up to 4 addresses |

This table lists the networking syscalls only; the full, current allocation (and the next free
number) is in [KERNEL.md](KERNEL.md#system-calls). Duplicates are silent (`#![allow(warnings)]` hides `unreachable_patterns`) — run
`tools/check_dup_syscall_arms.sh` before claiming one.
