"""Tests for check-imports.py: run `python3 -m unittest packaging/windows/test_check_imports.py`
(or `python3 packaging/windows/test_check_imports.py`). Builds tiny PE images in memory."""
import importlib.util
import os
import struct
import unittest

spec = importlib.util.spec_from_file_location("check_imports", os.path.join(os.path.dirname(__file__), "check-imports.py"))
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)


def make_pe(dlls, delay=(), pe32plus=True):
    """A minimal PE with one .idata section holding the import (and delay-import) tables."""
    pe_off = 0x80
    opt_size = 240 if pe32plus else 224
    dir_base_in_opt = 112 if pe32plus else 96
    sec_off = pe_off + 24 + opt_size
    raw = 0x400
    rva = 0x1000
    img = bytearray(raw)
    img[0:2] = b"MZ"
    struct.pack_into("<I", img, 0x3C, pe_off)
    img[pe_off:pe_off + 4] = b"PE\0\0"
    struct.pack_into("<H", img, pe_off + 6, 1)
    struct.pack_into("<H", img, pe_off + 20, opt_size)
    opt = pe_off + 24
    struct.pack_into("<H", img, opt, 0x20B if pe32plus else 0x10B)
    body = bytearray()

    def add_table(names, size, name_at):
        table_at = len(body)
        body.extend(b"\0" * (size * (len(names) + 1)))
        for i, n in enumerate(names):
            name_rva = rva + len(body)
            body.extend(n.encode() + b"\0")
            struct.pack_into("<I", body, table_at + size * i + name_at, name_rva)
        return rva + table_at, size * (len(names) + 1)

    imp = add_table(dlls, 20, 12)
    dly = add_table(list(delay), 32, 4) if delay else (0, 0)
    struct.pack_into("<II", img, opt + dir_base_in_opt + 8 * 1, *imp)
    struct.pack_into("<II", img, opt + dir_base_in_opt + 8 * 13, *dly)
    sec = sec_off
    img[sec:sec + 8] = b".idata\0\0"
    struct.pack_into("<IIII", img, sec + 8, len(body), rva, len(body), raw)
    return bytes(img) + bytes(body)


class CheckImports(unittest.TestCase):
    def test_lists_import_and_delay_import_tables(self):
        for plus in (True, False):
            names = ci.imported_dlls(make_pe(["KERNEL32.dll", "user32.dll"], ["d3d12.dll"], plus))
            self.assertEqual(names, ["KERNEL32.dll", "user32.dll", "d3d12.dll"])

    def test_static_crt_build_passes(self):
        names = ci.imported_dlls(make_pe(["KERNEL32.dll", "USER32.dll", "ntdll.dll", "bcryptprimitives.dll", "api-ms-win-core-synch-l1-2-0.dll"]))
        self.assertEqual(ci.forbidden(names), [])

    def test_dynamic_crt_build_is_flagged(self):
        names = ci.imported_dlls(make_pe(["KERNEL32.dll", "VCRUNTIME140.dll", "api-ms-win-crt-runtime-l1-1-0.dll", "MSVCP140.dll"], ["ucrtbase.dll"]))
        bad = ci.forbidden(names)
        self.assertEqual([b.lower() for b in bad], ["api-ms-win-crt-runtime-l1-1-0.dll", "msvcp140.dll", "ucrtbase.dll", "vcruntime140.dll"])

    def test_non_pe_is_rejected(self):
        with self.assertRaises(ValueError):
            ci.imported_dlls(b"\x7fELF" + bytes(100))


if __name__ == "__main__":
    unittest.main()
