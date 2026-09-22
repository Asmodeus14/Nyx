# What Nyx can and cannot do with quantum hardware

This file exists so that nobody — including a future version of this project — has to re-derive
the answer to "why doesn't Nyx just drive a QPU the way it drives the GPU". It is the honest
statement of the boundary, and the rest of `docs/quantum/` is designed around it.

Read this before adding anything to the quantum subsystem.

## The short version

**There is no consumer-available, openly-documented, gate-model quantum processor that Nyx can
control over PCIe.** Not one. `QpuStatus::Hardware` is defined in the device model, probed for on
every boot, and is **unreachable on this machine by design**.

That is not a gap waiting to be filled by more work. It is a property of the industry as of 2026.

## Why a QPU is not "another GPU"

The intuition that a QPU is a card you plug in is wrong in a specific and instructive way.

A GPU is a single die on a board, with a BAR, a ring buffer, and a documented register interface.
The kernel writes commands into memory the device can read and rings a doorbell. Nyx's Intel driver
does exactly this — see `nyx-kernel/src/drivers/gpu/intel/`.

A gate-model QPU is **not a die on a board**. For superconducting and spin qubits it is a
dilution refrigerator — a cryostat holding millikelvin temperatures — with the qubits at the bottom
and coaxial lines running up to a rack of room-temperature control electronics. The classical host
never talks to the qubits. It talks to *the controller*, and the controller synthesises microwave
pulses.

So the thing a general-purpose OS could plausibly drive is the **quantum control electronics**, not
the QPU. And those are their own product category:

| System | Attachment | Notes |
|---|---|---|
| Quantum Machines OPX | Ethernet / TCP | vendor SDK, expects a full Linux userspace |
| Zurich Instruments SHF / HDAWG | Ethernet / PCIe | vendor SDK |
| Qblox Cluster | Ethernet | vendor SDK |
| Xilinx RFSoC (ZCU111/216) based | PCIe / Ethernet | research systems, bespoke gateware |
| NVIDIA DGX Quantum | **PCIe Gen 5** | GPU ↔ quantum controller, the closest thing to "an accelerator" |

NVIDIA's **NVQLink** (validated through 2026 with, among others, Quandela's photonic QPU on an
FPGA-based Quantum System Controller) is the emerging standard for low-latency GPU↔controller
coupling. Its target is real-time quantum error correction: a surface-code cycle is roughly 1 µs and
the decoder must answer within about ten of them. That is an HPC-scale interconnect, not a device
class a hobby OS can bind to.

None of these have a public register-level specification. All of them ship a vendor SDK targeting
a Linux distribution with a full userspace. Nyx has neither the SDK nor the ability to run it.

## Desktop quantum computers do exist — and still don't help

SpinQ sells room-temperature NMR machines that are genuinely real quantum hardware and genuinely
purchasable: the Gemini Mini series (2 qubits, around $5,000) and Triangulum / Triangle II
(3 qubits). They report ~99% gate fidelities and run at 0–30 °C with permanent magnets.

They are also **appliances**. Each ships with its own onboard control computer and operating
system, a touchscreen, and vendor software; the host connection is USB into that software stack.
There is no documented register interface for an external OS to drive the spectrometer directly.

Two qubits would not be the problem. The absence of a specification is.

## The one piece of real quantum hardware Nyx could plausibly drive is not a QPU

A **quantum random number generator** on a PCIe card — for example the ID Quantique Quantis
PCIe-40M / PCIe-240M, which Linux exposes as `/dev/qrandom0` — is a plain PCIe device with a BAR.
It harvests shot noise from a light source on a CMOS sensor. That is real quantum physics producing
real entropy, and it is exactly the shape of device `nyx-kernel/src/pci.rs` already knows how to
bind.

It is **not a gate-model processor**. It cannot run a circuit. It has no qubits in the
computational sense.

This is precisely why the device model has a separate `QuantumDeviceKind`:

```rust
pub enum QuantumDeviceKind { Qpu, QuantumControlElectronics, QuantumRng }
```

A QRNG must never be able to appear where a QPU is expected. If one is ever fitted to this machine,
it should improve `nyx-kernel/src/random.rs` — not light up the quantum subsystem.

## So where does a real quantum result come from?

**The cloud.** IonQ, IBM Quantum, AWS Braket, Azure Quantum, Rigetti and Quantinuum all expose
their processors over HTTPS with token authentication. Nyx has working TLS 1.3 on hardware
(`libs/net`, rustls + rustls-rustcrypto + webpki-roots, verified 2026-09-10), so this is reachable.

That is why the remote provider is a first-class part of this subsystem rather than a future phase.
See `remote.md`.

It is also why the status model has four states and not three: a result that came back over the
network is **`Remote`**, never `Hardware`, because the hardware is not here. And a cloud
*simulator* — IonQ serves one through the same API as its real processors — is `Remote` with
`remote_is_simulator` set, because "it came from the internet" and "it was computed by quantum
mechanics" are different claims.

## The rule this file exists to protect

> Never report `QPU: HARDWARE` when Nyx is not driving physically attached quantum hardware.

The four states are `NotPresent`, `Simulator`, `Remote`, `Hardware`. They are distinct because
conflating them would make every number this subsystem reports meaningless.

This is not a new rule for this project. `apps/sysmon` already refuses to draw a GPU utilisation
percentage, and says why:

> There is no GPU utilisation counter anywhere in this system to read. […] A plausible-looking
> number in the one window whose entire purpose is reporting true numbers is the worst lie
> available here.

The quantum subsystem inherits that standard exactly.

## If you are reading this because you want to add hardware support

Good. The order of operations is in `future-hardware.md`, and the prerequisites are real — an
IOMMU, a permission model, and DMA isolation, none of which this kernel currently has. Start there,
not here.

## References

- NVIDIA NVQLink — <https://www.nvidia.com/en-us/solutions/quantum-computing/nvqlink/>
- Quandela × NVIDIA, real-time GPU–QPU integration —
  <https://www.quandela.com/resources/blog/quandela-nvidia-gpu-qpu-integration-nvqlink/>
- *Interfacing Quantum Computing Systems with HPC Systems: An Overview*, arXiv:2509.06205 —
  <https://arxiv.org/pdf/2509.06205>
- SpinQ Gemini, *EPJ Quantum Technology* —
  <https://link.springer.com/article/10.1140/epjqt/s40507-021-00109-8>
- SpinQ Gemini Mini — <https://www.spinquanta.com/products-services/spinq-gemini-mini>
- ID Quantique Quantis QRNG PCIe —
  <https://www.idquantique.com/random-number-generation/products/quantis-qrng-pcie/>
