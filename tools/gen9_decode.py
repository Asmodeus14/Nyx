#!/usr/bin/env python3
"""
gen9_decode.py — offline Gen9 (SKL/KBL/CML) command-stream decoder.

Purpose: stop flying blind. We can't *execute* the Gen9 3D pipeline in WSL
(no public functional simulator, QEMU doesn't emulate RCS), but we CAN decode
the exact command DWORDs the kernel submits and check every field against
Mesa's authoritative bit layout — the same thing `aubinator` does, minus the
Mesa build. This catches the class of bug where a malformed 3DSTATE command is
silently accepted but drops primitives (e.g. CL_inv=0).

Workflow:
  1. Kernel dumps the ring stream over serial, bracketed by
     `--CSDUMP-BEGIN <label> (<n> dw)--` ... `--CSDUMP-END--`
     (see cmd::hexdump_stream()).
  2. Save the serial log to a file (or pipe it in).
  3. python3 gen9_decode.py serial.log
     -> every command decoded field-by-field.

The bit layout comes from mesa-gen9.xml (Mesa genxml, SKL/gen9). Each command's
identity lives in its DW0 opcode fields (Command Type / SubType / Opcode /
Sub Opcode), all carrying `default=` in the XML; every field gives absolute
[start:end] bit positions across the whole multi-dword command.

Usage:
  gen9_decode.py [FILE]                 decode CSDUMP block(s) in FILE (or stdin)
  gen9_decode.py --xml PATH FILE        use a specific genxml
  gen9_decode.py --all FILE             show opcode/identity fields too (verbose)
  gen9_decode.py --raw FILE             treat whole input as hex (ignore markers)
"""

import sys
import os
import re
import struct
import xml.etree.ElementTree as ET

DEFAULT_XML = "/mnt/c/CODE/Nyx-Firmwares/Intel_Docs/mesa-gen9.xml"

# Fields that only establish which command this is — hidden unless --all.
OPCODE_FIELD_NAMES = {
    "Command Type", "Command SubType", "Pipeline",
    "3D Command Opcode", "3D Command Sub Opcode",
    "MI Command Opcode", "DWord Length",
}


def is_opcode_field(name):
    return name in OPCODE_FIELD_NAMES or name.endswith("Opcode")


class Field:
    __slots__ = ("name", "start", "end", "type", "default")

    def __init__(self, el):
        self.name = el.get("name")
        self.start = int(el.get("start"))
        self.end = int(el.get("end"))
        self.type = el.get("type")
        d = el.get("default")
        self.default = int(d) if d is not None else None


class Instr:
    __slots__ = ("name", "bias", "length", "engine", "fields",
                 "mask", "expected", "score", "dwlen_field")

    def __init__(self, el):
        self.name = el.get("name")
        self.bias = int(el.get("bias")) if el.get("bias") else 0
        self.length = int(el.get("length")) if el.get("length") else None
        self.engine = el.get("engine", "")
        # Only direct <field> children (skip <group>/<struct> internals for now).
        self.fields = [Field(f) for f in el.findall("field")]
        self.dwlen_field = next(
            (f for f in self.fields if f.name == "DWord Length"), None)
        self._compute_signature()

    def _compute_signature(self):
        mask = 0
        expected = 0
        for f in self.fields:
            if f.default is None or f.end > 31:
                continue
            if f.name == "DWord Length":
                continue
            if not is_opcode_field(f.name):
                continue
            width = f.end - f.start + 1
            m = ((1 << width) - 1) << f.start
            mask |= m
            expected |= (f.default << f.start) & m
        self.mask = mask
        self.expected = expected
        self.score = bin(mask).count("1")


def load_enums(root):
    enums = {}
    for e in root.findall("enum"):
        vals = {}
        for v in e.findall("value"):
            try:
                vals[int(v.get("value"))] = v.get("name")
            except (TypeError, ValueError):
                pass
        enums[e.get("name")] = vals
    return enums


def extract(dws, start, end):
    """Extract bits [start:end] (inclusive) from a little-endian dword list."""
    val = 0
    for b in range(start, end + 1):
        dwi = b >> 5
        if dwi < len(dws):
            val |= ((dws[dwi] >> (b & 31)) & 1) << (b - start)
    return val


def fmt_field(f, raw, enums):
    width = f.end - f.start + 1
    t = (f.type or "").lower()
    if t == "float" and width == 32:
        return "%g" % struct.unpack("<f", struct.pack("<I", raw))[0]
    if t == "int":
        if raw >> (width - 1):  # sign bit set
            raw -= (1 << width)
        return str(raw)
    if t == "bool":
        return "1" if raw else "0"
    if t in ("address", "offset"):
        return "0x%x" % raw
    if f.type in enums:
        name = enums[f.type].get(raw)
        return "%s (%d)" % (name, raw) if name else str(raw)
    # plain uint — show decimal, plus hex when large
    return "%d (0x%x)" % (raw, raw) if raw > 9 else str(raw)


def build_table(xml_path):
    root = ET.parse(xml_path).getroot()
    instrs = [Instr(i) for i in root.findall("instruction")]
    # Only opcode-bearing instrs are matchable; render/blitter/any engines all ok.
    instrs = [i for i in instrs if i.mask != 0]
    return instrs, load_enums(root)


def match(instrs, dw0):
    best = None
    for i in instrs:
        if (dw0 & i.mask) == i.expected:
            if best is None or i.score > best.score:
                best = i
    return best


def cmd_length(instr, dw0):
    if instr.dwlen_field is not None:
        n = extract([dw0], instr.dwlen_field.start, instr.dwlen_field.end)
        return n + instr.bias
    return instr.length if instr.length else 1


def decode_stream(dws, instrs, enums, show_all):
    i = 0
    idx = 0
    out = []
    while i < len(dws):
        dw0 = dws[i]
        instr = match(instrs, dw0)
        if instr is None:
            out.append("  [%3d] 0x%08x  ?? (no matching command)" % (i, dw0))
            i += 1
            idx += 1
            continue
        n = cmd_length(instr, dw0)
        n = max(1, min(n, len(dws) - i))
        body = dws[i:i + n]
        hexb = " ".join("%08x" % d for d in body)
        out.append("  [%3d] #%-2d %-34s (%d dw)  %s" %
                   (i, idx, instr.name, n, hexb))
        for f in instr.fields:
            if not show_all and (is_opcode_field(f.name) or f.name == "DWord Length"):
                continue
            if f.start >= n * 32:
                continue
            raw = extract(body, f.start, f.end)
            out.append("           %-42s = %s" %
                       (f.name, fmt_field(f, raw, enums)))
        i += n
        idx += 1
    return out


HEX8 = re.compile(r"(?:0x)?([0-9a-fA-F]{8})\b")
BEGIN = re.compile(r"--CSDUMP-BEGIN\s*(.*?)--")
END = "--CSDUMP-END--"


def extract_blocks(text, raw_mode):
    """Yield (label, [dwords]) blocks. In raw mode, one block of all hex tokens."""
    if raw_mode:
        dws = [int(m.group(1), 16) for m in HEX8.finditer(text)]
        if dws:
            yield ("raw", dws)
        return
    lines = text.splitlines()
    i = 0
    found = False
    while i < len(lines):
        m = BEGIN.search(lines[i])
        if not m:
            i += 1
            continue
        found = True
        label = m.group(1).strip() or "stream"
        i += 1
        hexes = []
        while i < len(lines) and END not in lines[i]:
            hexes += [int(h, 16) for h in HEX8.findall(lines[i])]
            i += 1
        yield (label, hexes)
        i += 1
    if not found:
        # No markers — fall back to raw so a bare paste still works.
        dws = [int(m.group(1), 16) for m in HEX8.finditer(text)]
        if dws:
            yield ("stream", dws)


def main():
    args = sys.argv[1:]
    xml_path = DEFAULT_XML
    show_all = False
    raw_mode = False
    infile = None
    it = iter(range(len(args)))
    i = 0
    while i < len(args):
        a = args[i]
        if a == "--xml":
            i += 1
            xml_path = args[i]
        elif a in ("--all", "-a"):
            show_all = True
        elif a in ("--raw", "-r"):
            raw_mode = True
        elif a in ("-h", "--help"):
            print(__doc__)
            return 0
        else:
            infile = a
        i += 1

    if not os.path.exists(xml_path):
        sys.stderr.write("error: genxml not found: %s\n"
                         "  pass --xml PATH\n" % xml_path)
        return 2

    text = open(infile).read() if infile else sys.stdin.read()
    instrs, enums = build_table(xml_path)

    n_blocks = 0
    for label, dws in extract_blocks(text, raw_mode):
        n_blocks += 1
        print("=" * 78)
        print("CSDUMP: %s  (%d dwords)" % (label, len(dws)))
        print("=" * 78)
        for line in decode_stream(dws, instrs, enums, show_all):
            print(line)
        print()
    if n_blocks == 0:
        sys.stderr.write("no command dwords found in input\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
