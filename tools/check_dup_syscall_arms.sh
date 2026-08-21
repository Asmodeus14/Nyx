#!/usr/bin/env bash
# Fail the build if the syscall dispatcher has two match arms for the same number.
#
# Rust tries match arms IN ORDER, so an earlier arm silently shadows a later one — and the
# kernel is `#![allow(warnings)]`, so the compiler never says a word. This has bitten twice:
# a `10 => { rax = 0 }` mprotect stub shadowed the real mprotect implementation, which meant
# the feature could never have worked no matter how correct the code below it was.
#
# Run before every kernel build.
set -uo pipefail

F="${1:-nyx-kernel/src/interrupts.rs}"

# Arms of the dispatcher's `match id`, which are indented 8 spaces. Handles both `59 =>` and
# multi-key arms like `22 | 293 =>`.
mapfile -t ARMS < <(grep -nE '^        [0-9]+( *\| *[0-9]+)* *=>' "$F")

echo "[dup-arms] $F: ${#ARMS[@]} numeric dispatcher arms"

# Expand each arm into one "number line" pair.
: > /tmp/_arm_pairs
for a in "${ARMS[@]}"; do
  line="${a%%:*}"
  keys="$(sed -E 's/^[0-9]+: *//; s/ *=>.*//' <<<"$a")"
  for k in ${keys//|/ }; do echo "$k $line" >> /tmp/_arm_pairs; done
done

dups="$(awk '{c[$1]=c[$1]" "$2} END{for(k in c){n=split(c[k],a," "); if(n>1) print k": lines"c[k]}}' /tmp/_arm_pairs)"

if [ -n "$dups" ]; then
  echo "[dup-arms] ✗ DUPLICATE ARMS — the earlier one wins and the later one is DEAD CODE:"
  echo "$dups"
  exit 1
fi

echo "[dup-arms] ✓ no duplicates"
