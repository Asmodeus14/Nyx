#!/usr/bin/env bash
# Build the C test binaries for Nyx.
#
# ★ WHY THE LOAD ADDRESS MATTERS (root cause of the HelloC freeze, found 2026-08-22):
#
# The Nyx KERNEL is linked at 0x200000..~0x15d5000, and `clone_kernel_page_table` copies all 512
# PML4 entries, so the kernel image is mapped in EVERY address space. gcc's default base of
# 0x400000 lands squarely inside the kernel's own .text. The ELF loader saw the page as already
# mapped, kept the kernel's mapping, and copied the segment ONTO THE RUNNING KERNEL — kernel-mode
# #PF at CR2 0x400003, machine dead. So a C binary for Nyx MUST be linked where the native apps
# live. The kernel now refuses such a load rather than writing through it, but the binaries still
# have to be built correctly.
#
#   hello     musl     0x40000000   <- shipped as run.bin; what the start menu launches
#   hello_lo  musl     0x400000     <- REGRESSION TEST: must now fail cleanly, not freeze
#   bare      no libc  0x40000000   <- musl-free control: isolates libc startup from the loader
#
# -nostdinc strips gcc's own headers too, and musl's headers need gcc's freestanding ones
# (stddef.h, stdarg.h) — so gcc's include dir goes back on the path FIRST, ahead of musl's.
# Link order is fixed: crt1.o, then the object, then libc.a, then libgcc for compiler builtins.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
M="$ROOT/vendor/musl"
GCCINC="$(cc -print-file-name=include)"
OUT="$ROOT/tests/helloc"

# Nyx's own userspace base. Native apps live here (verified with readelf on any run.bin), so a
# C binary linked here is loading into ground the kernel exercises on every single boot.
NYX_BASE=0x40000000

[ -f "$M/lib/libc.a" ] || { echo "[helloc] libc.a missing — run ./Build-musl.sh" >&2; exit 1; }

# Stale artifacts are worse than missing ones: a variant that fails to build would otherwise be
# shipped as whatever the previous run left behind, and get tested under the wrong name.
rm -f "$OUT/hello" "$OUT/hello_lo" "$OUT/bare" "$OUT/smoke" "$OUT/cpptest"

build_musl() {  # $1 = output name, $2 = source file, $3... = extra linker flags
  local out="$1" src="$2"; shift 2
  cc -static -nostdinc -nostdlib -fno-stack-protector -O2 \
    -isystem "$GCCINC" \
    -isystem "$M/include" \
    -isystem "$M/obj/include" \
    -isystem "$M/arch/x86_64" \
    -isystem "$M/arch/generic" \
    "$@" \
    "$M/lib/crt1.o" \
    "$OUT/$src" \
    "$M/lib/libc.a" -lgcc \
    -o "$OUT/$out"
}

build_cpp() {  # $1 = output name, $2 = source, $3... = extra linker flags
  local out="$1" src="$2"; shift 2
  # libsupc++ is gcc's C++ ABI runtime (__cxa_throw, guards, RTTI, operator new) and libgcc_eh is
  # the unwinder. Together they are everything the C++ LANGUAGE needs -- libc++ proper is only the
  # library on top, so this gate is reachable without cmake.
  #
  # Link order is left-to-right for static archives: libsupc++ calls malloc, so libc must follow
  # it, and the unwinder calls dl_iterate_phdr, which musl also supplies.
  #
  # No C++ headers are on the include path on purpose -- see cpptest.cpp.
  #
  # ★★★ --eh-frame-hdr is MANDATORY and easy to lose. It emits .eh_frame_hdr and the matching
  # PT_GNU_EH_FRAME program header, which is how _Unwind_Find_FDE locates the unwind tables: it
  # walks the phdrs looking for exactly that entry. gcc passes it automatically in a normal link,
  # but -nostdlib suppresses the spec that does so -- leaving a binary with .eh_frame present but
  # unreachable. Every throw then finds no handler, calls std::terminate -> abort, and musl's
  # abort ends in a_crash() = `hlt`, which is privileged, so it surfaces as a #GP at an address
  # inside abort() with nothing pointing at exceptions at all.
  g++ -static -nostdinc -nostdinc++ -nostdlib -Wl,--eh-frame-hdr -fno-stack-protector -O2 \
    -isystem "$GCCINC" \
    -isystem "$M/include" \
    -isystem "$M/obj/include" \
    -isystem "$M/arch/x86_64" \
    -isystem "$M/arch/generic" \
    "$@" \
    "$M/lib/crt1.o" \
    "$OUT/$src" \
    "$OUT/gnu_compat.c" \
    "$(g++ -print-file-name=libsupc++.a)" \
    "$(g++ -print-file-name=libgcc_eh.a)" \
    "$M/lib/libc.a" -lgcc \
    -o "$OUT/$out"
}

build_bare() {  # $1 = output name, $2... = extra linker flags
  local out="$1"; shift
  # -nostartfiles: the program supplies its own _start. -e _start names it as the entry point.
  cc -static -nostdinc -nostdlib -nostartfiles -fno-stack-protector -fno-builtin -O2 \
    -isystem "$GCCINC" -e _start "$@" \
    "$OUT/bare.c" -o "$OUT/$out"
}

build_musl hello    hello.c "-Wl,-Ttext-segment=$NYX_BASE"
build_musl hello_lo hello.c                          # gcc's default 0x400000 — the regression test
build_musl smoke    smoke.c "-Wl,-Ttext-segment=$NYX_BASE"
build_bare bare     "-Wl,-Ttext-segment=$NYX_BASE"
build_cpp  cpptest cpptest.cpp "-Wl,-Ttext-segment=$NYX_BASE"

# Print the entry point and every LOAD for each. This is a build-time check on purpose: a
# -Ttext-segment that silently did nothing would make the whole experiment a null result, and
# finding that out from the serial log costs a power cycle. Read these before booting.
echo "[helloc] built 5 binaries:"
for f in hello hello_lo smoke bare cpptest; do
  echo "----- $f -----"
  readelf -hl "$OUT/$f" | grep -E 'Type:|Entry point|LOAD|INTERP|TLS' || true
done
