#![cfg_attr(not(test), no_std)]

//! # `nyx-quantum` — the QPU as a compute resource
//!
//! Nyx models three compute substrates. This crate is the third:
//!
//! ```text
//! CPU   general-purpose classical   scheduler.rs, PerCpu, SysMetrics
//! GPU   parallel classical          drivers/gpu/intel, syscalls 501-538
//! QPU   quantum                     here
//! ```
//!
//! It holds the device model, the circuit IR, the backend trait, and a state-vector simulator.
//! It holds **no** networking, no compiler, and no syscalls — those are `libs/quantum-rt`,
//! `tools/compiler` and `libs/api` respectively. `no_std + alloc`, zero dependencies, so the kernel
//! could link the device types and a `no_std` app can run a circuit.
//!
//! ## The rule everything here is built around
//!
//! > Never report `HARDWARE` when Nyx is not driving physically attached quantum hardware.
//!
//! Nyx has no quantum hardware and, as of 2026, **no consumer gate-model QPU exists that it could
//! drive over PCIe** — see `docs/quantum/limitations.md`, which is the file to read before doing
//! anything hardware-shaped here. So [`QpuStatus`] has four states, not a boolean, and
//! [`QpuStatus::Hardware`] is producible only by the kernel's PCI probe. The simulator's `info()`
//! hardcodes [`QpuStatus::Simulator`] and there is no setter in the crate.
//!
//! This is the same standard `apps/sysmon` already holds itself to when it refuses to draw a GPU
//! utilisation percentage it cannot measure.
//!
//! ## What was already here
//!
//! `tools/compiler` (`qclang_compiler`) is a complete quantum compiler — `.ql` → QIR → OpenQASM —
//! with a dependency-free state-vector evaluator that already runs on-device in `apps/qcstudio`.
//! **This crate does not replace it.** [`Circuit`] is a lossless subset of
//! `qclang_compiler::program::Step`, `sim` is a port of that evaluator, and `libs/quantum-rt`
//! adapts between the two. `tools/compiler` is not modified.
//!
//! ## Example
//!
//! ```
//! use nyx_quantum::{Circuit, QpuBackend, SimBackend, Shots};
//!
//! let mut sim = SimBackend::new();
//! let outcome = sim.run(&Circuit::bell(), Shots(1024), 0x5EED).unwrap();
//!
//! // A Bell pair is correlated: only 00 and 11 ever appear.
//! //
//! // `counts_for` returns Option because a *provider* reports probabilities rather than counts,
//! // and there is no honest conversion between the two. Here it is Some, because the simulator
//! // really did draw shots.
//! assert_eq!(outcome.counts_for("01"), Some(0));
//! assert_eq!(outcome.counts_for("10"), Some(0));
//! assert_eq!(outcome.counts_for("00").unwrap() + outcome.counts_for("11").unwrap(), 1024);
//!
//! assert!(!outcome.provenance.is_quantum, "this was a classical simulation");
//! ```

extern crate alloc;

pub mod backend;
pub mod circuit;
// Credential parsing lives here — pure `no_std` string work with no filesystem — because two
// consumers with incompatible file APIs need to agree about it: `libs/quantum-rt` (std::fs) and
// `apps/settings` (no_std, nyx_api::sys_open). A duplicated parser would let the screen and the
// command line disagree about whether a key is configured.
pub mod creds;
pub mod device;
pub mod sim;
pub mod sim_backend;

pub use backend::{
    JobHandle, JobState, Outcome, Provenance, QpuBackend, QpuError, Registry, Shots, Stage,
};
pub use circuit::{gate_set_contains, gate_set_mask, Circuit, CircuitError, Gate, Op};
pub use device::{
    ExecModel, QpuInfo, QpuStatus, QuantumDeviceKind, Topology, NAME_LEN, QPU_INFO_SIZE, VENDOR_LEN,
};
pub use sim_backend::SimBackend;
