#!/usr/bin/env python3
import os, re, sys

base = "/home/singh/.rustup/toolchains/nightly-2026-07-01-x86_64-unknown-linux-gnu/lib/rustlib/src/rust/library/std/src/sys"

print("=== cfg_select inventory (fallback?) ===")
for root, dirs, files in sorted(os.walk(base)):
    for f in sorted(files):
        if f == "mod.rs":
            p = os.path.join(root, f)
            t = open(p).read()
            if "cfg_select!" in t:
                rel = os.path.relpath(p, base)
                has_default = bool(re.search(r"\n\s*_\s*=>", t))
                tag = "DFLT" if has_default else "NONE"
                print(tag, " ", rel)
