# Nyx documentation

Start with **[ARCHITECTURE.md](ARCHITECTURE.md)** — one diagram of the whole system and a map of the
repository — then follow the links for the part you care about.

```text
Nyx documentation
│
├── 🧠 Architecture ........ ARCHITECTURE.md
├── 🚀 Roadmap ............. ROADMAP.md
├── 🔨 Build & run ......... BUILD.md
├── ⚙️ Kernel .............. KERNEL.md        memory · scheduler · interrupts · syscalls · drivers · ACPI
├── 🎨 Graphics ............ GRAPHICS.md      Intel GPU · blitter · display · 3D engine · debugging
├── 🖥️ Desktop ............. UI.md            Meridian shell · window protocol · apps
├── 🌐 Networking
│   ├── network-architecture.md   how a URL becomes pixels
│   ├── https.md                  the TLS stack and what it verifies
│   ├── terminal-browser.md       the terminal's text browser
│   ├── network-audit.md          findings, severities, fixes
│   └── linux-cross-reference.md  reading Linux for facts, not code
├── ⚛️ Quantum ............. quantum/          the QPU as a compute resource
├── 🧮 QCLang .............. qclang/           language, CLI, changelog
└── 🗄️ Archive ............. archive/          superseded plans kept for their reasoning
```

## Index

| Document | Covers |
|---|---|
| [ARCHITECTURE.md](ARCHITECTURE.md) | system diagram, design decisions, repository map |
| [ROADMAP.md](ROADMAP.md) | the current goal (POSIX floor → musl → libc++ → Ladybird), history, open items |
| [BUILD.md](BUILD.md) | toolchain, `Build.sh`, booting in QEMU, hardware, host tests, CI |
| [KERNEL.md](KERNEL.md) | boot sequence, memory, SMP scheduling, interrupt vectors, the syscall table, drivers, ACPI |
| [GRAPHICS.md](GRAPHICS.md) | Intel GPU bring-up, GGTT layout, blitter, display, render engine, compositor, GPU text, mini-GL, debugging and resolved bugs |
| [UI.md](UI.md) | the Meridian desktop, frame and input paths, window protocol, apps, limitations |
| [network-architecture.md](network-architecture.md) | the network path, both smoltcp stacks, sockets and syscalls |
| [https.md](https.md) | rustls configuration, certificate verification, entropy, what breaks HTTPS |
| [terminal-browser.md](terminal-browser.md) | `get` / `links` / `open` and how the browser stays responsive |
| [network-audit.md](network-audit.md) | the networking audit (2026-09-10) |
| [linux-cross-reference.md](linux-cross-reference.md) | using Linux as a reference without copying GPL code |
| [quantum/](quantum/architecture.md) | architecture, device model, circuit IR, simulator, syscalls, security, remote providers, limitations, future hardware |
| [qclang/SYNTAX.md](qclang/SYNTAX.md) | the QCLang language specification |
| [qclang/CLI.md](qclang/CLI.md) | the `qclang` command-line tool |
| [qclang/CHANGELOG.md](qclang/CHANGELOG.md) | QCLang compiler history |
| [archive/userspace-evolution.md](archive/userspace-evolution.md) | the (completed) std / image viewer / on-device compiler plan |

## Conventions

Status markers, used the same way everywhere: 🟢 implemented · 🟡 experimental · 🔴 broken ·
🚧 in progress · ⬜ planned (the roadmap uses ✅ for completed milestones).

Where a statement could not be checked against the source it says **Verification required** rather
than guessing. The source is the authority: if a document and the code disagree, the code is right
and the document is a bug.
