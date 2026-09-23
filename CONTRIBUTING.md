# Contributing to Nyx

Thanks for looking. Read [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) first, and
[`docs/ROADMAP.md`](docs/ROADMAP.md) for where the project is heading.

## Rules

1. **Hardware is the final test.** QEMU is the fast loop ([`docs/BUILD.md`](docs/BUILD.md)), but it
   has no Intel GPU, Wi-Fi or touchpad and its timing differs from real silicon. A change that works
   in QEMU and fails on hardware is not done. Say what you tested on.
2. **The kernel is `#![no_std]`.** `core`, `alloc` and the vendored C (ACPICA, lwext4) only.
3. **Never block the window server.** `apps/shell` draws the whole desktop; any wait it makes needs a
   timeout.
4. **Syscalls:** the next free native number is recorded in [`docs/KERNEL.md`](docs/KERNEL.md#system-calls).
   Run `tools/check_dup_syscall_arms.sh` before adding one — a duplicate arm shadows silently.
5. **Document what is true.** If you change behaviour, update the doc that describes it. Mark
   anything you could not verify as *Verification required* rather than guessing.
6. **No GPL code.** Nyx is Apache-2.0. Linux may be read for facts, never copied — see
   [`docs/linux-cross-reference.md`](docs/linux-cross-reference.md).

## Submitting

- Make sure the GitHub Actions workflow (`.github/workflows/build.yaml`) passes.
- Run the host tests for any library you touched ([`docs/BUILD.md`](docs/BUILD.md#host-tests)).
- Open a pull request that says what you changed and what hardware (or QEMU) you tested it on.
