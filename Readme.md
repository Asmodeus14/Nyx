# Nyx OS ⚛️

> Schrödinger’s Companion for the Operating System of the Future

**Nyx** is an experimental open-source operating system designed for modern and quantum-era computing.

It brings together a custom Rust kernel, native quantum support through the integrated **QCLang** programming language, AI-driven optimizations, and a revolutionary file-system design — all focused on **security**, **resource efficiency**, and **sustainability**.

## ✨ Vision & Key Features

- **Native Quantum Computing** — Hybrid classical/quantum execution via QCLang  
- **AI-Driven System** — Intelligent resource management and adaptive behavior  
- **Revolutionary File System** — Optimized for both classical and quantum workloads  
- **Memory Safety & Performance** — Built entirely in Rust  
- **Security First** — Capability-based design and quantum-safe cryptography  
- **Sustainable Computing** — Low energy footprint by design  

## Project Structure
nyx/
├── compiler/      # QCLang compiler + toolchain (Rust-inspired quantum language)
├── nyx-kernel/    # Monolithic kernel (in progress)
├── nyx-user/      # User-space programs & services
├── runtime/       # System runtime libraries
├── runner/        # QEMU-based runner & testing
├── script/        # Build & utility scripts
├── Build.sh       # Main build script
├── Cargo.toml     # Workspace definition
└── SYNTAX.md      # Full QCLang language spec
## 🚀 Quick Start

```bash
git clone https://github.com/Asmodeus14/nyx.git
cd nyx
./Build.sh
For detailed commands see CLI.md.
QCLang Example – Bell State
fn main() -> int {
    // Quantum register (affine types enforce no-cloning)
    qreg q[2] = |00>;

    H(q[0]);
    CNOT(q[0], q[1]);

    let r1: cbit = measure(q[0]);
    let r2: cbit = measure(q[1]);

    return 0;
}
Status
Pre-alpha / Early Development Stage
QCLang compiler is the most mature component
Kernel is being built as a monolithic Rust + QCLang system
Quantum simulation and basic userland in progress
Check CHANGELOG.md for latest updates.
Contributing
We welcome contributions! This is a great project for:
OS/kernel developers (Rust)
Compiler enthusiasts
Quantum computing researchers
AI/systems programmers
Please read CONTRIBUTING.md and Code of Conduct first.
License
Licensed under Apache License 2.0
See License and NOTICE.md for details.