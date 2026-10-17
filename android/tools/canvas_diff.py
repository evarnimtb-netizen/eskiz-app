#!/usr/bin/env python3
"""Compare the canvas area of two screenshots (PNG, 8-bit RGB/RGBA, no interlace), standard library only.

usage: canvas_diff.py A.png B.png   -> prints the percentage of canvas pixels that differ

The crop leaves out the status bar, the top bar, the sidebar and the toast strip, so a clock
rollover or a toast cannot change the result: x 10%..90%, y 18%..82%. A pixel differs when any
channel differs by more than 24 (anti-aliasing and compression noise stay below that).
"""
import sys
import zlib

CROP = (0.10, 0.18, 0.90, 0.82)
CHANNEL_TOLERANCE = 24


def read_png(path):
    data = open(path, "rb").read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path}: not a PNG")
    pos, idat, ihdr = 8, [], None
    while pos < len(data):
        n = int.from_bytes(data[pos:pos + 4], "big")
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + n]
        if kind == b"IHDR":
            ihdr = body
        elif kind == b"IDAT":
            idat.append(body)
        pos += 12 + n
    w, h = int.from_bytes(ihdr[0:4], "big"), int.from_bytes(ihdr[4:8], "big")
    depth, ctype, interlace = ihdr[8], ihdr[9], ihdr[12]
    if depth != 8 or ctype not in (2, 6) or interlace:
        raise SystemExit(f"{path}: unsupported PNG (need 8-bit RGB/RGBA, not interlaced)")
    bpp = 3 if ctype == 2 else 4
    raw = zlib.decompress(b"".join(idat))
    stride = w * bpp
    rows, prev = [], bytearray(stride)
    for y in range(h):
        base = y * (stride + 1)
        f, line = raw[base], bytearray(raw[base + 1:base + 1 + stride])
        if f == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 255
        elif f == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 255
        elif f == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 255
        elif f == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 255
        rows.append(line)
        prev = line
    return w, h, bpp, rows


def main():
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    wa, ha, ba, ra = read_png(sys.argv[1])
    wb, hb, bb, rb = read_png(sys.argv[2])
    if (wa, ha) != (wb, hb):
        print("100.0")
        return
    x0, y0, x1, y1 = int(wa * CROP[0]), int(ha * CROP[1]), int(wa * CROP[2]), int(ha * CROP[3])
    differ = 0
    for y in range(y0, y1):
        la, lb = ra[y], rb[y]
        for x in range(x0, x1):
            ia, ib = x * ba, x * bb
            if (abs(la[ia] - lb[ib]) > CHANNEL_TOLERANCE or abs(la[ia + 1] - lb[ib + 1]) > CHANNEL_TOLERANCE
                    or abs(la[ia + 2] - lb[ib + 2]) > CHANNEL_TOLERANCE):
                differ += 1
    print(f"{100.0 * differ / ((x1 - x0) * (y1 - y0)):.4f}")


main()
