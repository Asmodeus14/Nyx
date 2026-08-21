#!/usr/bin/env bash
# Build musl's libc.a for Nyx.
#
# This is NOT a musl architecture port. Nyx's syscalls are Linux-numbered and Linux-shaped, so it is
# stock musl for x86_64-linux plus ONE patch: dropping __NR_open so musl falls back to openat.
#
# That fallback is a first-class musl configuration, not a hack — arm64 and riscv64 use it, and every
# SYS_open reference in musl sits behind a single #ifdef in src/internal/syscall.h with an openat
# #else branch. Nyx's own open(2) is (ptr, LEN, flags), a Nyx-ism no C library produces; openat(257)
# takes a NUL-terminated string, which is what musl passes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MUSL="$ROOT/vendor/musl"
PATCH="$ROOT/vendor/musl-patches/0001-drop-SYS_open-use-openat.patch"
HDR="$MUSL/arch/x86_64/bits/syscall.h.in"

[ -d "$MUSL" ] || { echo "[musl] vendor/musl missing" >&2; exit 1; }
[ -f "$PATCH" ] || { echo "[musl] patch missing: $PATCH" >&2; exit 1; }

if grep -qE '^#define __NR_open[[:space:]]' "$HDR"; then
  echo "[musl] applying $(basename "$PATCH")"
  patch -p1 -d "$MUSL" < "$PATCH"
else
  echo "[musl] already patched"
fi

# Assert the patch did what it claims. A silently-unapplied patch leaves musl calling open(2), which
# on Nyx means handing a C-string POINTER to a kernel expecting a LENGTH — that fails as garbage
# rather than as an error, so it is checked, never assumed. This is also what makes a musl version
# bump fail loudly here instead of at runtime.
if grep -qE '^#define __NR_open[[:space:]]' "$HDR"; then
  echo "[musl] FATAL: __NR_open still defined after patching" >&2
  exit 1
fi
grep -qE '^#define __NR_openat[[:space:]]' "$HDR" || {
  echo "[musl] FATAL: __NR_openat missing — wrong arch header?" >&2
  exit 1
}

cd "$MUSL"
[ -f config.mak ] || ./configure --disable-shared --prefix="$MUSL/_install" >/dev/null
make -j"$(nproc)" >/dev/null

# Verify the BUILT code, not just the header: open() must issue openat (257 = 0x101).
if ! objdump -d obj/src/fcntl/open.lo | grep -q 'mov .*0x101'; then
  echo "[musl] FATAL: built open() does not issue openat(257)" >&2
  exit 1
fi

echo "[musl] lib/libc.a $(stat -c%s lib/libc.a) bytes — open() -> openat(257) verified"
