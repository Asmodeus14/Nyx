# Changelog

All notable changes to the **QCLang** project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2025-01-30

### Added
- **Phase 2 Optimizations**: Implemented advanced optimization passes.
  - *Dead Qubit Elimination*: Automatically removes quantum operations on unmeasured qubits.
  - *Gate Cancellation*: Detects and removes redundant adjacent gates (e.g., `H` followed by `H`).
- **Self-Updater**: New `qclang update` command.
  - Connects to GitHub Releases to fetch the latest binary.
  - Interactive CLI menu to select versions or auto-update via `--latest`.
- **Professional CLI**: Complete overhaul of the command-line interface.
  - Added `qclang info` for build/repo metadata.
  - Added `qclang run` for single-step compilation and execution.
  - Implemented professional progress bars, status tables, and structured logging.
  - Added `qclang benchmark` to measure compiler throughput.
- **CI/CD**: automated release pipeline now builds, renames, and packages binaries for Windows (`.zip`) and Linux (`.tar.gz`) automatically.

### Changed
- **Compiler Backend**:
  - `QirBuilder` rewritten to correctly handle `measure()` statements as values.
  - Fixed array indexing logic in quantum registers (`q[0]`).
- **Output**: Improved compilation summary stats (Qubit count, Gate depth, Measurement count).

### Fixed
- **Critical Bug**: Fixed an issue where the optimizer incorrectly marked *all* qubits as dead if they were measured into a variable (e.g., `cbit c = measure(q);`), resulting in empty QASM output.
- **Workflow**: Updated GitHub Actions to use `v4` artifact actions, fixing deprecated `v3` errors.

## [0.1.4] - 2025-01-28

### Added
- **Phase 1.5 Pipeline**: Introduced Quantum Intermediate Representation (QIR) generation step.
- **Optimization Pass**: Added basic QIR optimizations (gate cancellation, constant folding).
- **Quantum Loop Syntax**: Support for `qfor` and `qif` constructs in the parser.
- **CLI Improvements**: Added `--show` flag to display compilation phases and final OpenQASM output.
- **ASCII Art**: "Quantum Cat" logo added to compiler startup sequence.

### Changed
- **Semantic Analysis**: Stricter enforcement of Affine Typing.
  - *Fix*: Reassigning a `qubit` variable now raises a hard semantic error.
  - *Fix*: Using a qubit after `measure()` is now correctly blocked.
- **Parser**: Updated gate application syntax to forbid assignment (e.g., `q = H(q)` is now invalid; use `H(q)`).

### Fixed
- Fixed semantic error hints for immutable variable assignment.
- Resolved issue where `measure()` return type was not correctly inferred as `cbit`.

## [0.3.0] - Previous Release
- Initial Parser and Lexer implementation.
- Basic AST structure.
- OpenQASM 2.0 code generation backbone.
