# Contributing to NyxOS

If you are looking to contribute to the kernel, the QCLang soul engine, or the bare-metal graphics stack, your code must align with the current Phase roadmap.

## Contribution Rules:

1. **Bare Metal First:** If it works in QEMU but panics on real hardware, the PR will be rejected. Always test your commits against the provided `.devcontainer` build environment and flash it to a physical machine.
2. **Zero Overhead:** The Nyx Entity (`nyx-entityd`) must run seamlessly in the background. Do not introduce blocking operations in the kernel scheduler that stall the entity's state loop.
3. **No External Dependencies (Kernel):** The `nyx-kernel` is strictly `#![no_std]`. Rely only on `core`, `alloc`, and raw hardware I/O. 
4. **Preserve the Aesthetic:** Keep the code minimal, deeply documented, and clean. If you are touching the rendering stack, respect the minimalist cyberpunk geometry.

## How to Submit:
* Ensure the GitHub Actions `build.yaml` passes completely.
* Open a PR detailing exactly what hardware you tested the changes on.
* Wait for a core maintainer to review the architecture.