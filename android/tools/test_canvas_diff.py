#!/usr/bin/env python3
"""Host test for canvas_diff.py (run: python3 -I android/tools/test_canvas_diff.py)."""
import os
import struct
import subprocess
import sys
import tempfile
import unittest
import zlib

W, H = 200, 100
HERE = os.path.dirname(os.path.abspath(__file__))


def png(path, paint):
    rows = []
    for y in range(H):
        row = bytearray([0])  # filter 0
        for x in range(W):
            row += bytes(paint(x, y)) + b"\xff"
        rows.append(bytes(row))

    def chunk(kind, body):
        c = struct.pack(">I", len(body)) + kind + body
        return c + struct.pack(">I", zlib.crc32(kind + body))

    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0))
    data += chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b"")
    open(path, "wb").write(data)


def diff(a, b):
    out = subprocess.check_output([sys.executable, "-I", os.path.join(HERE, "canvas_diff.py"), a, b])
    return float(out)


class CanvasDiff(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.base = os.path.join(self.d, "base.png")
        png(self.base, lambda x, y: (240, 240, 240))

    def shot(self, name, paint):
        p = os.path.join(self.d, name)
        png(p, paint)
        return p

    def test_toast_status_bar_and_noise_are_ignored(self):
        def noisy(x, y):
            if y < 8:  # status bar clock
                return (0, 0, 0)
            if y > 90 and 60 < x < 140:  # toast
                return (30, 30, 30)
            return (240 - (x + y) % 5, 240, 240)  # tiny noise
        self.assertLessEqual(diff(self.base, self.shot("a.png", noisy)), 0.05)

    def test_a_stroke_in_the_canvas_is_seen(self):
        def stroke(x, y):
            return (20, 20, 20) if 40 <= y <= 43 and 30 <= x <= 170 else (240, 240, 240)
        self.assertGreater(diff(self.base, self.shot("b.png", stroke)), 0.05)


if __name__ == "__main__":
    unittest.main()
