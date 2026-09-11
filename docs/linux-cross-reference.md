# Cross-referencing the Linux reference implementations

## Why this file exists

The iwlwifi RX ring bug — a consumer index wrapping at 2³² against a firmware producer that wraps at
512 — cost most of a session, several power cycles, four wrong diagnoses, and a runaway guard just to
survive being measured. Linux's `iwl_pcie_rx_handle` does:

```c
i = (i + 1) & (rxq->queue_size - 1);
```

**One diff would have found it.** So would a diff for `random_seed` (Linux has generated TCP ISNs
from a keyed hash since the 1990s), for DHCP lease renewal (RFC 2131 §4.4.5 T1/T2), and for returning
a single A record when `getaddrinfo` has always returned a list.

This machine has **no QEMU and no serial console** — every experiment costs a power cycle. Reading a
reference implementation costs nothing. For the driver and protocol layers, reading comes first now.

## The licence boundary — read for facts, never transcribe

**Nyx is Apache-2.0. Linux is GPL-2.0.** GPL code cannot be copied into an Apache-2.0 project.

| Allowed | Not allowed |
|---|---|
| Hardware register offsets, bit values, firmware command IDs | Copying a function |
| Protocol rules and wire formats | Copying distinctive comments |
| Algorithms as ideas, reimplemented independently | Reproducing structure line for line |

Register semantics and wire formats are facts, not copyrightable expression. The code is expression.
Everything below is recorded as **facts and constants**, and any Nyx implementation must be written
independently from those facts.

This matters more than usual for iwlwifi: the device has **no public datasheet**, so the Linux driver
*is* the documentation.

---

## 1. A-MSDU — our concern was half wrong, and the real finding is better

**What I expected:** `rx_ethernet` returns only the first subframe of an aggregate and silently
drops the rest, because it finds one LLC/SNAP signature and returns.

**What the reference shows:** on this generation the **firmware deaggregates in hardware** and
delivers each subframe as its own RX MPDU. `enum iwl_rx_mpdu_amsdu_info` carries:

| Constant | Value | Meaning |
|---|---|---|
| `IWL_RX_MPDU_AMSDU_SUBFRAME_IDX_MASK` | `0x7f` | which subframe this is |
| `IWL_RX_MPDU_AMSDU_LAST_SUBFRAME` | `0x80` | set on the final subframe |

A per-subframe index and a last-subframe flag only make sense if each subframe arrives separately.
So the "drops all but the first" theory is **not** supported for the firmware-deaggregated path, and
`rx_amsdu = 0` in testing is consistent with that.

**Kept for reference anyway** (`net/wireless/util.c`, `ieee80211_amsdu_to_8023s`), because software
deaggregation is still needed if the flag ever appears:

- subframe header is **DA(6) · SA(6) · Length(2)** = 14 bytes
- the length is **big-endian** — our code already reads it big-endian, which is correct
- padding to a 4-byte boundary: **`padding = (4 - subframe_len) & 0x3`**
- ⚠️ **the LAST subframe is not padded**: `last = remaining <= subframe_len + padding`
- next subframe: `offset += 14` then `offset += len + padding`
- validation: reject when `subframe_len > remaining` before trusting it
- inner LLC/SNAP is `aa aa 03 00 00 00` (RFC1042) **or `aa aa 03 00 00 f8` (bridge-tunnel)** — strip
  6 + 2 = 8 bytes

★ **We only recognise `aa aa 03 00 00 00`.** The bridge-tunnel variant ending `f8` is a second legal
encapsulation we would reject as NOSNAP.

---

## 2. The RX descriptor already tells us what we are guessing at

This is the significant finding, and it makes the NOSNAP drops addressable.

From `drivers/net/wireless/intel/iwlwifi/fw/api/rx.h`:

| Constant | Value | Meaning |
|---|---|---|
| `IWL_RX_MPDU_MFLG2_HDR_LEN_MASK` | `0x1f` | **802.11 header length, in 2-byte words** |
| `IWL_RX_MPDU_MFLG2_PAD` | `0x20` | 2 bytes of padding were inserted after the header |
| `IWL_RX_MPDU_MFLG2_AMSDU` | `0x40` | frame is part of an A-MSDU |
| `IWL_RX_MDPU_MFLG1_ADDRTYPE_MASK` | `0x03` | address type |
| `IWL_RX_MPDU_MFLG1_MIC_CRC_LEN_MASK` | `0xf0` | MIC/CRC length, `>> 3`, in 2-byte words |

**Our driver computes the header length by hand** — 24, plus 6 for a 4-address frame, plus 2 for QoS
— and then **scans up to 20 bytes for the SNAP signature** to absorb the device's padding and any
field it did not account for. The device has been reporting the exact answer all along:

```
header_len = (mac_flags2 & 0x1f) * 2
payload starts at header_len (+ 2 if mac_flags2 & 0x20)
```

That removes the scan, and with it the whole class of NOSNAP drops caused by a header layout the
hand-computation does not model — **HT Control (+4 bytes on a QoS frame with the Order bit)** being
the obvious one we never handle.

### Decryption status is also in the descriptor

| Constant | Value |
|---|---|
| `IWL_RX_MPDU_STATUS_DECRYPTED` | `BIT(11)` |
| `IWL_RX_MPDU_STATUS_SEC_MASK` | `7 << 8` |
| `..._SEC_NONE / WEP / CCM / TKIP / EXT_ENC / GCM` | `0..5 << 8` |

**Every frame we drop as NOSNAP is a group-addressed frame the firmware did not decrypt** (measured:
`dropped=N (group=N OURS=0)`, CCMP KeyID 1). We discover that by scanning for a signature that cannot
possibly be there and failing. Testing `STATUS_DECRYPTED` would reject those in one bit-test and —
more useful — let the counter say *"not decrypted"* rather than *"no SNAP found"*, which are
different facts.

### ⚠️ What is still missing before this can be implemented

The **byte offsets of `mac_flags2` and `status` within `iwl_rx_mpdu_desc`**, for the descriptor
version this 9462/JF uses. The struct has v1/v3 variants and unions, and the driver already tracks
`rx_desc_size` for exactly that reason.

**Not guessing them.** Getting an offset wrong here produces a plausible-looking wrong header length,
which is precisely the failure mode this document exists to stop. That is one more targeted read of
`rx.h` — cheap, and it must happen before any code changes.

---

## 3. DHCP lease renewal — RFC, not Linux

The WiFi driver's hand-written DHCP client never parses **option 51 (lease time)** and has no
renewal timer, so a lease is taken once and held forever (audit M1).

RFC 2131 §4.4.5 is the reference, and it needs no source reading:

- **T1 = 0.5 × lease** → unicast DHCPREQUEST to the leasing server (RENEWING)
- **T2 = 0.875 × lease** → broadcast DHCPREQUEST to any server (REBINDING)
- lease expiry → drop the address, stop using the interface
- T1/T2 may be overridden by **option 58** and **option 59** respectively

Also relevant: we keep only the **first** DNS server from option 6, which is a `[u8; 4]`. Option 6
carries a list, and a resolver with one server has no fallback — the same shape of bug as returning
one A record.

---

## 4. Where cross-referencing does *not* help

Worth stating so it is not over-applied. These were genuinely ours to get wrong, with no upstream
analogue:

- **`SOCKET_TIMEOUT` vs `POLL_BUDGET`** — Nyx has no threads and pumps the network from a repaint
  loop. Linux has softirqs and NAPI; there is nothing to compare.
- **`max_burst_size`** — a smoltcp quirk (it clamps the advertised receive window). The right
  reference was smoltcp's own source, and that is where it was eventually found.
- **SNI filtering upstream** — no source tells you what an ISP does to your packets.

The rule that falls out: **cross-reference the hardware and the protocols; reason from first
principles about our own architecture.**

---

## 5. Standing practice

For any future driver or protocol work:

1. Find the reference implementation (Linux for hardware, the RFC for protocols, the crate's own
   source for library behaviour) **before** writing or debugging on hardware.
2. Extract facts — constants, offsets, wire formats, state-machine rules — into this file.
3. Implement independently from those facts. Never paste.
4. Only then spend a power cycle.

Confirmed by the one time the project already did this: **Fedora running on this same laptop was the
oracle** that proved the device is a 9462/JF and not the AX201 everyone had assumed. That method
worked and then lapsed.
