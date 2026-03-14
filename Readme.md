[![Rust](https://img.shields.io/badge/Rust-1.80+-000000?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Build Status](https://img.shields.io/github/actions/workflow/status/Asmodeus14/Nyx/ci.yml?branch=master&label=build&logo=github)](https://github.com/Asmodeus14/Nyx/actions/workflows/Build.yml)
[![Crates.io](https://img.shields.io/crates/v/nyx-kernel?label=nyx-kernel&logo=rust)](https://crates.io/crates/nyx-kernel)
[![GitHub stars](https://img.shields.io/github/stars/Asmodeus14/Nyx?style=social)](https://github.com/Asmodeus14/Nyx/stargazers)
[![GitHub forks](https://img.shields.io/github/forks/Asmodeus14/Nyx?style=social)](https://github.com/Asmodeus14/Nyx/forks)
[![Pre-Alpha](https://img.shields.io/badge/Status-Pre--Alpha-red?style=flat)](https://github.com/Asmodeus14/Nyx)

# Nyx OS ⚛️

**Schrödinger’s Companion for the Operating System of the Future**

Nyx is an **experimental open-source operating system** built from the ground up for the intersection of classical, quantum, and AI-driven computing.

It features:
- A **custom monolithic kernel** written in safe, high-performance **Rust**
- **Native quantum programming** through the integrated **QCLang** language (Rust-inspired syntax with quantum primitives)
- **AI-driven** resource management, adaptive scheduling, and system behavior
- A **next-generation file system** designed for hybrid classical-quantum workloads
- Strong focus on **memory safety**, **security** (capability-based + post-quantum crypto), **efficiency**, and **sustainability** (low energy footprint)

This is **pre-alpha** software — actively developed, incomplete, and evolving rapidly.

## Vision

Nyx aims to be more than an OS: a unified platform where classical code, quantum circuits, and machine intelligence coexist natively. No bolted-on simulators or external toolchains — quantum is first-class.

## Key Features (Current & Planned)

- **Hybrid Classical/Quantum Execution** — QCLang compiles to kernel-integrated quantum ops (simulation now, real hardware later)
- **AI-Optimized Kernel** — Adaptive scheduling, predictive resource allocation, entropy-aware behavior
- **Revolutionary FS** — Quantum-metadata support, fractal-inspired layouts, lossless compression primitives
- **Rust Everywhere** — Kernel + userspace + compiler toolchain in Rust for safety and speed
- **Security Model** — Fine-grained capabilities, quantum-resistant algorithms
- **Simulation-First Development** — QEMU-based runner for rapid iteration

## Current Status

**Pre-alpha / Early Development** (as of March 2026)

- Most mature: **QCLang compiler** + syntax/runtime
- In progress: **nyx-kernel** (Rust monolithic kernel with QCLang hooks)
- Early stages: userspace, runtime libs, quantum simulation backend
- No bootloader yet; runs in QEMU for now

See [CHANGELOG.md](./CHANGELOG.md) for commit-level updates.

## Project Structure

```text
nyx/
├── compiler/        # QCLang compiler, parser, codegen (Rust-inspired quantum lang)
├── nyx-kernel/      # Custom monolithic Rust kernel (core + quantum extensions)
├── nyx-user/        # Userspace binaries, init, services
├── runtime/         # Core runtime libraries & syscalls
├── runner/          # QEMU runner, disk images, test harness
├── script/          # Build utilities, helpers
├── .github/workflows/  # CI pipelines
├── Build.sh         # Main one-command build script
├── Cargo.toml       # Rust workspace (kernel + compiler + libs)
├── SYNTAX.md        # Detailed QCLang language specification
├── CLI.md           # Build/run CLI reference
├── CONTRIBUTING.md  # How to contribute
└── ... (see root for full list)
Quick Start (Simulation Mode)
Requires: Rust (nightly/toolchain pinned via rust-toolchain.toml), QEMU.
Bashgit clone https://github.com/Asmodeus14/Nyx.git
cd Nyx
./Build.sh

./Build.sh builds the workspace and prepares QEMU images.
See CLI.md for run options, debug flags, etc.

Example: Boot the current kernel image in QEMU
Bash./runner/run-qemu.sh
QCLang — Hello Quantum World (Bell State)
Rustfn main() -> int {
    // Affine quantum register: enforces no-cloning theorem at compile time
    qreg q[2] = |00>;          // Initialize to |00⟩

    H(q[0]);                   // Hadamard on qubit 0 → superposition
    CNOT(q[0], q[1]);          // Entangle qubits

    let r1: cbit = measure(q[0]);  // Collapse & read
    let r2: cbit = measure(q[1]);

    // r1 == r2 always (due to entanglement)
    return 0;
}
Full language spec → SYNTAX.md
Contributing
Nyx is open to contributions — especially from people excited about:

Systems / kernel programming in Rust
Compiler design (quantum languages)
Quantum computing (algorithms, simulation, error correction)
AI for systems (scheduling, optimization)
Low-level performance & security

Please read CONTRIBUTING.md and Code_Of_Conduct.md first.
We especially welcome:

Bug reports
Documentation improvements
Small features in QCLang or kernel
Testing/QEMU harness enhancements

License
Apache License 2.0
See License and NOTICE.md for full details.

Nyx — because every great OS needs a bit of mystery and entanglement.
Let's build the future, one qubit at a time. 🖤