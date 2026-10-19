#!/usr/bin/env python3
"""Host test for png_same.py (run: python3 -I android/tools/test_png_same.py)."""
import os
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
W, H = 20, 10


def png(path, paint, ctype=6):
    rows = []
    for y in range(H):
        row = bytearray([0])
        for x in range(W):
            row += bytes(paint(x, y)) + (b"\xff" if ctype == 6 else b"")
        rows.append(bytes(row))

    def chunk(kind, body):
        c = struct.pack(">I", len(body)) + kind + body
        return c + struct.pack(">I", zlib.crc32(kind + body))

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, ctype, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b"")
    open(path, "wb").write(data)


def run(*args):
    p = subprocess.run([sys.executable, "-I", os.path.join(HERE, "png_same.py"), *args], capture_output=True, text=True)
    return p.returncode, p.stdout.strip()


class PngSame(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()

    def p(self, name, paint, ctype=6):
        path = os.path.join(self.d, name)
        png(path, paint, ctype)
        return path

    def test_equal_images_are_same_even_across_rgb_and_rgba(self):
        a = self.p("a.png", lambda x, y: (x * 10, y * 20, 7))
        b = self.p("b.png", lambda x, y: (x * 10, y * 20, 7), ctype=2)
        self.assertEqual(run(a, b), (0, "same"))

    def test_one_pixel_difference_is_found(self):
        a = self.p("a.png", lambda x, y: (1, 2, 3))
        b = self.p("b.png", lambda x, y: (1, 2, 4) if (x, y) == (5, 5) else (1, 2, 3))
        code, out = run(a, b)
        self.assertEqual(code, 1)
        self.assertIn("1 of 200", out)
        self.assertEqual(run(a, b, "--tol", "1")[0], 0)

    def test_different_sizes_differ(self):
        a = self.p("a.png", lambda x, y: (0, 0, 0))
        path = os.path.join(self.d, "c.png")
        global W
        old, W = W, 21
        png(path, lambda x, y: (0, 0, 0))
        W = old
        self.assertEqual(run(a, path)[0], 1)

    def test_pixel_mode(self):
        a = self.p("a.png", lambda x, y: (32, 128, 255))
        self.assertEqual(run("--pixel", a, "10", "5", "32", "128", "255")[0], 0)
        self.assertEqual(run("--pixel", a, "10", "5", "0", "0", "0")[0], 1)
        self.assertEqual(run("--pixel", a, "99", "5", "32", "128", "255")[0], 1)


if __name__ == "__main__":
    unittest.main()
