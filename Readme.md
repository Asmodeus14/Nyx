<img width="849" height="383" alt="image" src="https://github.com/user-attachments/assets/fdf3d4da-0f6c-4186-9a69-a7a67f8a8ad0" />

# ⚛️ QCLang

> **Schrödinger’s Companion** — a modern, type-safe quantum systems programming language.

![License](https://img.shields.io/badge/license-Apache%202.0-blue.svg)
![Version](https://img.shields.io/badge/version-0.1.4-green.svg)
![Status](https://img.shields.io/badge/status-active-success.svg)

QCLang is a high-level programming language designed to bridge the gap between **classical systems programming** and **quantum circuit execution**.  
It features a Rust-inspired syntax, a strict **affine type system** for quantum safety, and compilation to **OpenQASM 2.0**.

```

      /\_/\ 
     ( o.o )      Q U A N T U M   C A T
      > ^ <    Schrödinger’s Companion


````

---

## ✨ Features

- **Hybrid Control Flow**  
  Seamlessly mix classical logic (`if`, `for`, `while`) with quantum operations.

- **Quantum Safety (Affine Types)**  
  Compile-time enforcement of the *No-Cloning Theorem*:
  - Prevents quantum variable reassignment  
  - Prevents use-after-measurement

- **Modern, Familiar Syntax**  
  Strong typing, structs, tuples, and type aliases inspired by Rust.

- **QIR & Optimizing Backend**  
  Generates optimized **OpenQASM 2.0** (QIR phase in progress).

- **Detailed Diagnostics**  
  Helpful compile-time errors with actionable hints.

---

## 🚀 Getting Started

### Prerequisites
- Rust (latest stable)
- Cargo

### Installation (from source)

```bash
git clone https://github.com/Asmodeus14/qclang.git
cd qclang
cargo install --path .
````

---

## 📦 Releases

Prebuilt binaries and versioned releases are available for download.

👉 **Download from GitHub Releases:**
[https://github.com/Asmodeus14/qclang/releases](https://github.com/Asmodeus14/qclang/releases)

Each release includes:

* `qclang` CLI binary
* Release notes
* Supported platform details

> This is the recommended way to install QCLang if you don’t want to build from source.

---

## 🧪 Usage

Compile a `.qc` file to OpenQASM:

```bash
qclang compile my_circuit.qc --show
```

Run syntax checks only:

```bash
qclang check my_circuit.qc
```

---

## 📖 Syntax Example

### Bell State Preparation

```rust
fn main() -> int {
    // Initialize quantum register
    qreg q[2] = |00>;

    // Apply gates (affine: no reassignment)
    H(q[0]);
    CNOT(q[0], q[1]);

    // Measurement consumes the qubits
    let r1: cbit = measure(q[0]);
    let r2: cbit = measure(q[1]);

    return 0;
}
```

See `syntax.md` for the full language specification.

---

## 🛠️ Project Structure

* `src/lexer.rs` — Tokenizer (Logos)
* `src/parser.rs` — Recursive descent parser
* `src/semantics/` — Type system & ownership checks
* `src/qir/` — Quantum Intermediate Representation
* `src/codegen/` — OpenQASM 2.0 backend

---

## 🤝 Contributing

Contributions are welcome!
Please see `CONTRIBUTING.md` for guidelines.

This project is especially friendly to:

* Compiler enthusiasts
* Quantum computing researchers
* Systems programmers

---

## 📄 License

Licensed under the **Apache License 2.0**.
See the `LICENSE` file for details.


