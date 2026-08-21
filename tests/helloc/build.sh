#!/usr/bin/env bash
# Build the musl C hello-world for Nyx.
#
# -nostdinc strips gcc's own headers too, and musl's headers need gcc's freestanding ones
# (stddef.h, stdarg.h) — so gcc's include dir goes back on the path FIRST, ahead of musl's.
# Link order is fixed: crt1.o, then the object, then libc.a, then libgcc for compiler builtins.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
M="$ROOT/vendor/musl"
GCCINC="$(cc -print-file-name=include)"

[ -f "$M/lib/libc.a" ] || { echo "[helloc] libc.a missing — run ./Build-musl.sh" >&2; exit 1; }

cc -static -nostdinc -nostdlib -fno-stack-protector -O2 \
  -isystem "$GCCINC" \
  -isystem "$M/include" \
  -isystem "$M/obj/include" \
  -isystem "$M/arch/x86_64" \
  -isystem "$M/arch/generic" \
  "$M/lib/crt1.o" \
  "$ROOT/tests/helloc/hello.c" \
  "$M/lib/libc.a" -lgcc \
  -o "$ROOT/tests/helloc/hello"

echo "[helloc] built:"
file "$ROOT/tests/helloc/hello"
echo "[helloc] entry + segments:"
readelf -hl "$ROOT/tests/helloc/hello" | grep -E 'Type:|Entry point|LOAD|INTERP|TLS' || true
