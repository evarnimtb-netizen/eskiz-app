#!/usr/bin/env python3
r"""Fail if a Windows executable imports the Visual C++ runtime.

Eskiz is built with the static CRT (`-C target-feature=+crt-static`), so eskiz.exe must not import
VCRUNTIME*.dll, MSVCP*.dll, CONCRT*.dll, VCOMP*.dll, UCRTBASE.dll or the api-ms-win-crt-* forwarders:
a clean Windows PC does not have the Visual C++ Redistributable, and the program would not start.

Usage: check-imports.py path\to\eskiz.exe [--list]

Reads the PE import table directly (no dumpbin or Visual Studio needed). Exit status 0 = fine,
1 = forbidden imports (named), 2 = not a PE file.
"""
import re
import struct
import sys

FORBIDDEN = re.compile(r"^(vcruntime|msvcp|concrt|vcomp|vccorlib|ucrtbase|msvcr|api-ms-win-crt-)", re.I)


def imported_dlls(data: bytes):
    """The DLL names in the import and delay-import tables of a PE32/PE32+ image."""
    if data[:2] != b"MZ":
        raise ValueError("not a PE file (no MZ header)")
    pe = struct.unpack_from("<I", data, 0x3C)[0]
    if data[pe:pe + 4] != b"PE\0\0":
        raise ValueError("not a PE file (no PE signature)")
    n_sections, = struct.unpack_from("<H", data, pe + 6)
    opt_size, = struct.unpack_from("<H", data, pe + 20)
    opt = pe + 24
    magic, = struct.unpack_from("<H", data, opt)
    dir_base = opt + (112 if magic == 0x20B else 96)
    sections = []
    sec = opt + opt_size
    for i in range(n_sections):
        vsize, vaddr, rawsize, rawptr = struct.unpack_from("<IIII", data, sec + 40 * i + 8)
        sections.append((vaddr, max(vsize, rawsize), rawptr))

    def rva_to_off(rva):
        for vaddr, size, rawptr in sections:
            if vaddr <= rva < vaddr + size:
                return rawptr + rva - vaddr
        raise ValueError(f"RVA {rva:#x} is outside every section")

    def cstr(off):
        end = data.index(b"\0", off)
        return data[off:end].decode("ascii", "replace")

    names = []
    # Directory 1: import table (20-byte descriptors, name RVA at +12).
    # Directory 13: delay import table (32-byte descriptors, name RVA at +4).
    for index, size, name_at in ((1, 20, 12), (13, 32, 4)):
        rva, total = struct.unpack_from("<II", data, dir_base + 8 * index)
        if not rva or not total:
            continue
        off = rva_to_off(rva)
        while True:
            desc = data[off:off + size]
            if len(desc) < size or not any(desc):
                break
            name_rva, = struct.unpack_from("<I", desc, name_at)
            if name_rva:
                names.append(cstr(rva_to_off(name_rva)))
            off += size
    return names


def forbidden(names):
    return sorted({n for n in names if FORBIDDEN.match(n)}, key=str.lower)


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    path = argv[1]
    try:
        with open(path, "rb") as f:
            names = imported_dlls(f.read())
    except (OSError, ValueError, struct.error) as e:
        print(f"check-imports: {path}: {e}", file=sys.stderr)
        return 2
    if "--list" in argv:
        print("\n".join(sorted(set(names), key=str.lower)))
    bad = forbidden(names)
    if bad:
        print(f"check-imports: {path} imports the C++ runtime ({', '.join(bad)}).", file=sys.stderr)
        print("Build with the static CRT: RUSTFLAGS='-C target-feature=+crt-static' (see .cargo/config.toml).", file=sys.stderr)
        return 1
    print(f"check-imports: {path}: {len(set(names))} imported DLLs, none from the Visual C++ runtime")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
