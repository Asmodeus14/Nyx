# The QPU device model

`libs/quantum/src/device.rs`, mirrored by hand in `libs/api/src/lib.rs` and
`nyx-kernel/src/quantum.rs`.

## The four states

```rust
#[repr(u32)]
pub enum QpuStatus {
    NotPresent = 0,   // nothing, anywhere
    Simulator  = 1,   // classical software, running on this CPU
    Remote     = 2,   // reachable over the network, not here
    Hardware   = 3,   // a real QPU, physically attached to this machine
}
```

These are distinct because conflating them would make every number the subsystem reports
meaningless. The rule:

> Never report `HARDWARE` when Nyx is not driving physically attached quantum hardware.

That is enforced **structurally**, not by convention:

- `QpuStatus::Hardware` is produced in exactly one place in the tree — `quantum::probe_pci`, on an
  exact match against `KNOWN_QUANTUM_DEVICES`. Nothing in userspace can construct it.
- `SimBackend::info()` hardcodes `Simulator`. There is no setter, no constructor argument, and no
  `#[cfg]` that changes it.
- `QpuSession::backend_for` has **no arm for `Hardware`**. If the kernel ever reports an attached
  QPU, opening it fails with *"attached but Nyx has no driver for it; running it on a simulator
  instead would misreport the result"*. Falling back to the simulator would be the precise failure
  this whole subsystem exists to prevent.

## ⚠️ Why `Remote` alone is not enough

This is the subtlest part of the model.

Cloud providers serve classical simulators **through the same API, with the same job lifecycle,
returning the same JSON** as their real processors. IonQ's `simulator` and its `qpu.aria-1` differ by
one string in one field.

If both reported `Remote` and nothing else, a classical simulation could be presented as a quantum
result — the exact failure the local `Simulator` state prevents, reintroduced one layer out. *"It came
from the internet"* and *"it was computed by quantum mechanics"* are different claims.

So `QpuInfo` carries `remote_is_simulator`, and:

| status | `remote_is_simulator` | rendered | `is_quantum()` |
|---|---|---|---|
| `NotPresent` | — | `NOT PRESENT` | false |
| `Simulator` | — | `SIMULATOR` | false |
| `Remote` | set | `REMOTE/sim` | **false** |
| `Remote` | clear | `REMOTE/hw` | true |
| `Hardware` | — | `HARDWARE` | true |

`QpuInfo::is_quantum()` is the single predicate every display must use. Matching on `status` directly
is the mistake it exists to prevent.

`ionq::target_is_simulator` is deliberately conservative: it tests `!name.starts_with("qpu.")`, so an
unrecognised future target name errs toward **claiming less**, not more.

## Device kinds

```rust
pub enum QuantumDeviceKind { Qpu, QuantumControlElectronics, QuantumRng, UnidentifiedAccelerator }
```

Four, because four genuinely different things can appear:

- **`Qpu`** — takes a circuit, returns measurements. The only kind where `is_executor()` is true.
- **`QuantumControlElectronics`** — the classical half of a quantum computer (an AWG rack, an FPGA
  controller). Nyx can see one on the bus; it cannot drive one without a vendor SDK.
- **`QuantumRng`** — real quantum hardware that cannot run a circuit. A Quantis PCIe card belongs to
  `random.rs`, not here. This variant exists so a QRNG can never appear where a processor is
  expected.
- **`UnidentifiedAccelerator`** — see below.

## ⚠️ A class code cannot identify a QPU

PCI class `0x12` is "Processing Accelerator" and `0xFF` is "unassigned". A PCIe quantum accelerator
would plausibly appear as either — and so would an AI inference card, a research FPGA, or anything
nobody bothered to classify.

Nyx has **no verified vendor/device ID for any quantum processor**, so guessing from the class alone
would manufacture the `HARDWARE` claim this model forbids. The probe therefore records such a device
as `UnidentifiedAccelerator` with status `NotPresent` — a diagnostic, explicitly not a claim.

It is *recorded* rather than only logged because this laptop has no serial console: anything the
probe learns and writes to `serial_println!` is learned and thrown away.

## `QpuInfo`

`#[repr(C)]`, **append-only**, 224 bytes, alignment 8. Field order **is** the ABI — there is no
bindgen across the ring boundary in this tree, exactly as with `SysMetrics` and `WindowQuad`. All
three mirrors carry

```rust
const _: () = assert!(core::mem::size_of::<QpuInfo>() == 224);
```

so adding a field breaks the build in all three places rather than silently reinterpreting every
later field.

### "Not reported" is not zero

| field | "unknown" value |
|---|---|
| `coherence_t1_ns`, `coherence_t2_ns`, `calibrated_unix` | `0` |
| `max_shots`, `max_depth`, `queue_capacity` | `0` |
| `queue_depth` | **`u32::MAX`** — because `0` would claim an empty queue exists |
| `qubits` | `0`, and only when nothing is present |

The accessors return `Option` (`coherence_t1()`, `queue()`, `calibrated()`) and the terminal prints
`—` rather than a number. A simulator has no T1; a provider may publish no calibration date.
Substituting a plausible default is what `apps/sysmon` refuses to do with GPU utilisation, in its own
words:

> A plausible-looking number in the one window whose entire purpose is reporting true numbers is the
> worst lie available here.

### Connectivity

`topology` is a coarse enum (`Unknown`/`AllToAll`/`Linear`/`Grid`/`Explicit`). The full coupling map
is `O(n²)` and deliberately **not** in this fixed-size struct; a backend that knows its map exposes it
separately. `Unknown` means "not reported", not "no structure".

## Strings

`vendor[32]`, `arch[32]`, `name[64]`, NUL-padded ASCII. `set_cstr` truncates on a **character**
boundary — a byte-wise cut would split a multi-byte UTF-8 sequence and make the whole field read back
as `""`, so a device whose name was one byte too long would lose its name entirely rather than its
last character.

## Version skew

`QpuStatus::from_raw` maps anything unrecognised to `NotPresent`, and `from_raw` exists for every
enum. A kernel reporting a status this build does not understand is version skew, and the safe
reading of "I don't know what this is" is "assume nothing is there" — it fails closed.
