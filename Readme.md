# Nyx OS ⚛️

**Schrödinger’s Companion for the Operating System of the Future**

[![Rust](https://img.shields.io/badge/Rust-nightly%20%7C%201.80+-000000?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)
[![Build Status](https://img.shields.io/github/actions/workflow/status/Asmodeus14/Nyx/build.yaml?branch=master&label=build&logo=github&color=green)](https://github.com/Asmodeus14/Nyx/actions/workflows/build.yaml)
[![Dev Containers](https://img.shields.io/badge/Dev%20Containers-supported-blue?logo=visualstudiocode&logoColor=white&labelColor=007ACC&color=007ACC)](https://github.com/Asmodeus14/Nyx/tree/master/.devcontainer)
[![Codespaces Ready](https://img.shields.io/badge/GitHub%20Codespaces-Ready-brightgreen?logo=github)](https://github.com/codespaces/new?hide_repo_select=true&ref=master&repo=Asmodeus14/Nyx)
[![Repo Size](https://img.shields.io/github/repo-size/Asmodeus14/Nyx?color=informational&logo=github)](https://github.com/Asmodeus14/Nyx)
[![Lines of Code](https://img.shields.io/tokei/lines/github/Asmodeus14/Nyx?color=blueviolet)](https://github.com/Asmodeus14/Nyx)
[![Last Commit](https://img.shields.io/github/last-commit/Asmodeus14/Nyx/master?color=important&logo=github)](https://github.com/Asmodeus14/Nyx/commits/master)
[![Contributors](https://img.shields.io/github/contributors/Asmodeus14/Nyx?color=purple)](https://github.com/Asmodeus14/Nyx/graphs/contributors)
[![Open Issues](https://img.shields.io/github/issues/Asmodeus14/Nyx?color=yellow)](https://github.com/Asmodeus14/Nyx/issues)
[![Pull Requests](https://img.shields.io/github/issues-pr/Asmodeus14/Nyx?color=orange)](https://github.com/Asmodeus14/Nyx/pulls)
[![GitHub stars](https://img.shields.io/github/stars/Asmodeus14/Nyx?style=social)](https://github.com/Asmodeus14/Nyx/stargazers)
[![GitHub forks](https://img.shields.io/github/forks/Asmodeus14/Nyx?style=social)](https://github.com/Asmodeus14/Nyx/forks)
[![Pre-Alpha](https://img.shields.io/badge/Status-Pre--Alpha-red?style=flat)](https://github.com/Asmodeus14/Nyx)
---

# Overview

Nyx is an **experimental open-source operating system** built from the ground up for the intersection of:

* Classical computing
* Quantum computing
* AI-driven system intelligence

It is designed as a **next-generation OS platform** where classical programs, quantum circuits, and machine intelligence coexist natively.

Core design principles:

* **Memory safety**
* **Security-first architecture**
* **High performance**
* **Low energy footprint**
* **Future-ready computing**

> ⚠️ Nyx is currently **Pre-Alpha** and under active development.

---

# Vision

Nyx aims to become a **unified computing platform** where:

* Classical programs
* Quantum circuits
* AI-driven system intelligence

run together **as first-class citizens**.

Instead of relying on external simulators or toolchains, Nyx integrates **quantum execution directly into the operating system architecture**.

---

# Key Features

## Hybrid Classical–Quantum Execution

QCLang programs compile into kernel-integrated quantum operations.

* Quantum simulation backend (current)
* Native hardware support (future)

---

## AI-Optimized Kernel

Adaptive kernel behavior driven by machine learning:

* Predictive scheduling
* Dynamic resource allocation
* Entropy-aware system tuning

---

## Next-Generation File System

Experimental filesystem designed for hybrid workloads:

* Quantum metadata support
* Fractal-inspired storage layout
* Built-in lossless compression primitives

---

## Rust-First Architecture

Entire stack built with **Rust**:

* Kernel
* Userspace
* Compiler toolchain

Benefits:

* Memory safety
* High performance
* Reduced system vulnerabilities

---

## Security Model

Security features include:

* Capability-based permissions
* Post-quantum cryptography
* Memory safety guarantees

---

## Simulation-First Development

Development workflow built around **QEMU simulation** for rapid iteration and testing.

---

# Current Status

**Pre-Alpha (March 2026)**

Progress:

| Component                  | Status             |
| -------------------------- | ------------------ |
| QCLang Compiler            | Most mature        |
| Kernel (nyx-kernel)        | Active development |
| Userspace                  | Early stage        |
| Runtime Libraries          | Early stage        |
| Quantum Simulation Backend | Prototype          |

Capabilities:

* Boots in **QEMU**
* Partial **real hardware boot**
* Early **DRM/Nouveau handshake work**

---

# Project Structure

```
nyx/
├── .devcontainer/        # Dev Containers config
├── .github/workflows/    # CI pipelines
├── compiler/             # QCLang compiler
├── nyx-kernel/           # Rust monolithic kernel
├── nyx-user/             # Userspace binaries
├── runtime/              # Runtime libraries & syscalls
├── runner/               # QEMU runner and disk images
├── script/               # Build scripts and helpers
├── Build.sh              # One-command build script
├── Cargo.toml            # Rust workspace
├── SYNTAX.md             # QCLang language spec
├── CLI.md                # CLI reference
├── CONTRIBUTING.md       # Contribution guide
```

---

# Quick Start (Simulation Mode)

### Requirements

* Rust (nightly toolchain)
* QEMU

---

### Clone the repository

```bash
git clone https://github.com/Asmodeus14/Nyx.git
cd Nyx
```

---

### Build the project

```bash
./Build.sh
```

This builds the entire Rust workspace and prepares QEMU disk images.

---

### Run Nyx in QEMU

```bash
./runner/run-qemu.sh
```

See **CLI.md** for advanced options and debugging flags.

---

# QCLang Example

### Hello Quantum World (Bell State)

```rust
fn main() -> int {

    // Affine quantum register
    qreg q[2] = |00>;

    // Create superposition
    H(q[0]);

    // Entangle qubits
    CNOT(q[0], q[1]);

    let r1: cbit = measure(q[0]);
    let r2: cbit = measure(q[1]);

    // r1 == r2 due to entanglement
    return 0;
}
```

Full language specification available in:

`SYNTAX.md`

---

# Contributing

Nyx welcomes contributions from developers interested in:

* Operating systems
* Rust kernel development
* Quantum programming languages
* Quantum simulation
* AI-driven systems
* Security engineering

Ways to contribute:

* Bug reports
* Documentation improvements
* QCLang language features
* Kernel enhancements
* QEMU testing infrastructure

Please read:

* `CONTRIBUTING.md`
* `CODE_OF_CONDUCT.md`

before submitting pull requests.

---

# License

Licensed under **Apache License 2.0**.

See:

* `LICENSE`
* `NOTICE.md`

for full details.

---

# Closing

> **Nyx — because every great OS needs a bit of mystery and entanglement.**

**Let's build the future, one qubit at a time.** 🖤

---
