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

# ★ Scope to syscall_dispatch_inner's body FIRST. The check used to scan the whole file for
# 8-space-indented numeric arms, so ANY other numeric match in interrupts.rs tripped it — adding
# a signal-disposition table (`17 | 23 | 28 => ...`) reported five bogus duplicates. That matters
# more than it sounds: this script exists because a shadowed arm is invisible to the compiler, and
# a checker that cries wolf is one people start passing over. Narrowing it loses nothing, because
# only arms of THIS match can shadow each other.
#
# The function ends at the first column-0 `}`, which is unambiguous in rustfmt'd code. Line numbers
# are preserved so the failure message still points at real lines.
# `NR":"$0` reproduces grep -n's exact format, which the parsing below already expects.
BODY="$(awk '/^fn syscall_dispatch_inner/{f=1} f{print NR":"$0; if (seen && /^}/) exit; seen=1}' "$F")"
[ -n "$BODY" ] || { echo "[dup-arms] ✗ could not locate syscall_dispatch_inner in $F" >&2; exit 1; }

# Arms of the dispatcher's `match id`, which are indented 8 spaces. Handles both `59 =>` and
# multi-key arms like `22 | 293 =>`.
mapfile -t ARMS < <(grep -E '^[0-9]+: {8}[0-9]+( *\| *[0-9]+)* *=>' <<<"$BODY")

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
