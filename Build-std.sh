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

# 2. Build the std crate for the nyx target.
STD_BUILD="cargo build --release --target $TARGET_JSON \
  -Z build-std=std,panic_abort \
  -Z build-std-features=compiler-builtins-mem \
  -Z json-target-spec"

( cd "$ROOT_DIR/$CRATE_DIR" && $STD_BUILD )
echo "[Build-std] built $CRATE_DIR for x86_64-unknown-nyx"
