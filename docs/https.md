# HTTPS on Nyx

What the TLS stack is, what it verifies, and the two things that most often break it on this
machine. Findings and severities live in [`network-audit.md`](network-audit.md).

## The stack

```
libs/net::Transport::Tls
  └─ rustls 0.23                     protocol, state machine, verification driver
       ├─ rustls-rustcrypto 0.0.2-alpha   pure-Rust crypto provider
       ├─ rustls-webpki 0.103             X.509 parse · chain build · hostname match
       └─ webpki-roots 1.0                121 Mozilla roots, compiled into the binary
```

`ring` and `aws-lc-rs` — rustls's usual providers — are C and assembly, and there is no C toolchain
for `target_os = "nyx"`. That is the whole reason for the pure-Rust provider, and the reason it is
an alpha: it is the only maintained option.

## What is verified

**There is no certificate-verification bypass in this repository, and adding one is not acceptable.**
`.with_root_certificates(roots)` resolves to the stock `WebPkiServerVerifier`:

- full chain construction to a trusted root (≤ 6 intermediates, with a signature-work budget that
  hardens against certificate-bomb DoS)
- **hostname verification** against SAN, using the same `ServerName` passed for SNI — so the name
  checked and the name sent cannot diverge
- **validity window** — not-before and not-after, against `SystemTime::now()`
- name constraints

Not verified:

- **revocation.** `new_without_revocation`: no CRL is fed, and rustls-webpki has no OCSP module at
  all, so stapled responses are not consulted either. A revoked-but-unexpired certificate is
  accepted. This is a real gap and cannot be closed without upstream work.

## Protocol and algorithms

TLS **1.3 and 1.2** — despite `libs/net`'s own description as a "TLS 1.3 client".
`with_safe_default_protocol_versions()` plus the `tls12` feature enables both.

| | |
|---|---|
| TLS 1.3 suites | AES-128-GCM, AES-256-GCM, ChaCha20-Poly1305 |
| TLS 1.2 suites | ECDHE-{ECDSA,RSA} × {AES-128-GCM, AES-256-GCM, ChaCha20-Poly1305} |
| Key exchange | X25519, secp256r1, secp384r1 |
| Signatures | RSA PKCS#1 & PSS (SHA-256/384/512), ECDSA P-256/P-384, Ed25519 |
| SNI | on, from the URL host |
| ALPN | **unset** — correct for an HTTP/1.1-only client |
| 0-RTT early data | off |
| Key logging | off |
| Session resumption | on (256 entries) — not intended, see audit L2 |

No CBC, no static RSA key transport, no 3DES, no RC4. A mainstream server (Let's Encrypt ECDSA
P-256 or RSA-2048, TLS 1.3, X25519) is fully supported.

## Entropy

rustls draws ECDHE private keys, the client random and GCM nonces from `getrandom`, which on Nyx is
syscall 318 → `random::fill()` → **RDSEED**, falling back to RDRAND. Both are CPUID-detected and the
RDSEED retry loop follows Intel's guidance (64 attempts, then RDRAND).

If neither instruction exists, `fill()` degrades to a TSC-seeded xorshift and **returns `false`**.
`libs/net` sets `NYX_GRND_CRYPTO` (0x8000) on the syscall, which makes the kernel zero the buffer and
return `EIO` rather than hand back guessable bytes — rustls then abandons the handshake. That is the
correct outcome: a failed connection is recoverable, a session key drawn from a guessable PRNG is
not.

Plain `getrandom` (no flag) keeps its always-succeed behaviour, because std seeds every `HashMap`
through it at startup and cannot cope with an error.

## The two things that actually break HTTPS here

### 1. The clock

**This is the most common HTTPS failure on this machine and it is not a TLS problem.**

rustls checks the certificate's validity window against `SystemTime::now()`, which comes from the
CMOS RTC. **The RTC on this laptop does not retain its value across a power cycle** — it returns to
a round-numbered 2024 default every boot, which is a flat CMOS battery, not a software fault. Every
`https://` fetch then fails with:

```
invalid peer certificate: certificate not valid yet
```

Fix it per boot:

```
date          # RTC and SystemTime side by side
time sync     # set from an http:// Date header; verified by reading the RTC back
```

`time sync` **refuses** a time earlier than the image's build stamp or more than 20 years after it.
That bound matters: the value is fetched over plain HTTP and is unauthenticated by construction, and
a clock set *backwards* revives expired certificates — with no revocation checking, there is no
second line of defence. The floor closes that whole class. It is not authentication; NTS or
Roughtime would be.

### 2. One unreachable address

Historically the biggest cause of "HTTPS is broken" on Nyx, and it had nothing to do with TLS: the
DNS syscall returned **one** A record and discarded the rest, so an unreachable address had no
fallback. Symptoms were connect timeouts, handshakes that completed and then stalled, and "it worked
an hour ago" — all one address at a time, drawn from a rotating list.

Syscall 572 now returns up to four, and a failed connect drops only the address that failed. If you
see a connect timeout, `dns <host>` shows how many addresses the name has.

## Diagnosing a failure

`get` prints a stage line after 4 seconds and on failure:

```
head addr=142.251.154.119:443 polls=20 plain=0B body=0B
     tls[hs=no r=1 w=0] sock[rd=21/6523B wr=4/435B to=16 errno=110]
```

- **`tls[hs=…]`** — `YES` means still handshaking; `no` means the handshake **completed** and any
  stall after that is HTTP, not TLS.
- **`sock[…]`** counts the **raw socket**, underneath rustls. This matters: `plain=` counts
  *decrypted* bytes and stays at 0 for an entire healthy handshake, so it says nothing about whether
  the peer replied. `sock[rd=…]` does.
- **`addr=`** — differing on every attempt means multi-address fallback is working.

Errors are distinguishable by design: `cannot resolve X`, `X is not answering`, a TLS message with
an `explain()` hint, or an HTTP status. "Network error" is not an output this stack produces.

## Known limitations

- No revocation checking (CRL or OCSP).
- No custom CA mechanism — the trust list requires a recompile. Corporate MITM proxies and
  self-signed dev servers cannot work. The flip side is that there is no trust-injection surface.
- No IDN/punycode: a Unicode hostname fails at `ServerName::try_from`. Fail-closed.
- IPv6 literals are rejected at parse; there is no IPv6 stack.
- `confidentiality_limit: u64::MAX` in the alpha provider disables automatic key rotation for
  AES-GCM. Negligible here (8 MB body cap, one request per connection) but a deviation from rustls's
  reference provider.
- rustls's `logging` feature is off, so its own security-relevant messages compile to nothing.
