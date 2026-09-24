# Firmware

Third-party firmware that Nyx loads into devices. These files are **not** Nyx code and are not
covered by Nyx's Apache-2.0 licence: each is redistributed unmodified under its own licence, which
sits beside it.

| File | What | Licence |
|---|---|---|
| `intel/iwlwifi.ucode` | Intel Wi-Fi firmware for the 9462-class adapter, build `release/core74::aa2dd297` (from the file header). Embedded in the kernel with `include_bytes!` by `nyx-kernel/src/drivers/net/iwlwifi.rs` and uploaded to the adapter at boot. | [`intel/LICENCE.iwlwifi_firmware`](intel/LICENCE.iwlwifi_firmware) — Intel's redistribution licence, as distributed in [linux-firmware](https://gitlab.com/kernel-firmware/linux-firmware) |

`sha256  8213aed16505b2a5945b9b04ce517bd24c51222cc205a16454f66530f24075b8  intel/iwlwifi.ucode`

The licence permits redistribution in binary form **without modification**. Do not patch these
files; replace them with another unmodified release instead.
