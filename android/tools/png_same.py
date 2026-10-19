#!/usr/bin/env python3
"""Full-image pixel compare of two PNGs (8-bit RGB/RGBA, no interlace), standard library only.

usage: png_same.py A.png B.png [--tol N]      exit 0 when every pixel of A equals B (each channel within N, default 0)
       png_same.py --pixel FILE X Y R G B [--tol N]   exit 0 when that pixel is the colour (alpha ignored)

Prints one line: "same", or "differ: <count> of <total> pixels", or "size A WxH vs B WxH".
"""
import importlib.util
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def _read_png():
    spec = importlib.util.spec_from_file_location("canvas_diff", os.path.join(HERE, "canvas_diff.py"))
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod.read_png


def compare(a, b, tol=0):
    """Return (size_equal, differing_pixels, total) for two read_png results."""
    wa, ha, ba, ra = a
    wb, hb, bb, rb = b
    if (wa, ha) != (wb, hb):
        return False, 0, 0
    differ = 0
    for y in range(ha):
        la, lb = ra[y], rb[y]
        for x in range(wa):
            ia, ib = x * ba, x * bb
            n = 3
            bad = any(abs(la[ia + c] - lb[ib + c]) > tol for c in range(n))
            if not bad and ba == 4 and bb == 4:
                bad = abs(la[ia + 3] - lb[ib + 3]) > tol
            if bad:
                differ += 1
    return True, differ, wa * ha


def pixel_is(img, x, y, rgb, tol=0):
    w, h, bpp, rows = img
    if not (0 <= x < w and 0 <= y < h):
        return False
    row = rows[y]
    return all(abs(row[x * bpp + c] - rgb[c]) <= tol for c in range(3))


def main(argv):
    read_png = _read_png()
    tol = 0
    if "--tol" in argv:
        i = argv.index("--tol")
        tol = int(argv[i + 1])
        argv = argv[:i] + argv[i + 2:]
    if argv and argv[0] == "--pixel":
        if len(argv) != 7:
            raise SystemExit(__doc__)
        x, y, r, g, b = (int(v) for v in argv[2:7])
        ok = pixel_is(read_png(argv[1]), x, y, (r, g, b), tol)
        print("same" if ok else "differ")
        return 0 if ok else 1
    if len(argv) != 2:
        raise SystemExit(__doc__)
    a, b = read_png(argv[0]), read_png(argv[1])
    same_size, differ, total = compare(a, b, tol)
    if not same_size:
        print(f"size A {a[0]}x{a[1]} vs B {b[0]}x{b[1]}")
        return 1
    if differ:
        print(f"differ: {differ} of {total} pixels")
        return 1
    print("same")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
