#!/usr/bin/env bash
# Build a `target_os = "nyx"` std binary (Workstream B). Reproducible entry point for the std
# toolchain: (1) ensure the nyx PAL is injected into the active rust-src, (2) build the crate with
# -Z build-std=std. Usage: ./Build-std.sh <crate-dir> [<out-bin-name>]
#   e.g. ./Build-std.sh tests/stdhello stdhello
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "$0")" && pwd)"
CRATE_DIR="${1:?usage: Build-std.sh <crate-dir> [bin-name]}"
TARGET_JSON="$ROOT_DIR/targets/x86_64-unknown-nyx.json"

# 1. Inject / refresh the Nyx std PAL in the toolchain's rust-src (idempotent).
python3 "$ROOT_DIR/vendor/nyx-std/apply.py"

# 1b. ★ Force a std rebuild whenever the PAL sources move.
#
# apply.py rewrites files INSIDE the toolchain's rust-src, and cargo's build-std fingerprint does not
# reliably notice — you get a fast "successful" build that silently links the PREVIOUS PAL. This is
# not theoretical: it shipped a terminal whose `set_read_timeout` still returned `Unsupported` after
# the syscall behind it had been implemented, and the failure surfaced on hardware as a bare
# "operation not supported on this platform" with nothing pointing at the toolchain.
#
# The tell is a std build that takes ~8s and never prints `Compiling std`. Stamp the PAL sources and
# drop the std fingerprint when the hash changes.
PAL_HASH=$(find "$ROOT_DIR/vendor/nyx-std" -type f \( -name '*.rs' -o -name '*.py' \) \
  | sort | xargs cat | sha256sum | cut -d' ' -f1)
STAMP="$ROOT_DIR/$CRATE_DIR/target/.nyx-pal-stamp"
if [ ! -f "$STAMP" ] || [ "$(cat "$STAMP")" != "$PAL_HASH" ]; then
  echo "[Build-std] PAL sources changed -> forcing a std rebuild"
  rm -rf "$ROOT_DIR/$CRATE_DIR"/target/*/release/.fingerprint/std-* \
         "$ROOT_DIR/$CRATE_DIR"/target/*/release/deps/libstd-* 2>/dev/null || true
  mkdir -p "$(dirname "$STAMP")"
  echo "$PAL_HASH" > "$STAMP"
fi

# 2. Build the std crate for the nyx target.
STD_BUILD="cargo build --release --target $TARGET_JSON \
  -Z build-std=std,panic_abort \
  -Z build-std-features=compiler-builtins-mem \
  -Z json-target-spec"

( cd "$ROOT_DIR/$CRATE_DIR" && $STD_BUILD )
echo "[Build-std] built $CRATE_DIR for x86_64-unknown-nyx"
