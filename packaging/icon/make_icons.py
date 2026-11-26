#!/usr/bin/env python3
"""Draws the application icon and writes every format we ship (no third-party packages needed).

    python3 packaging/icon/make_icons.py

Outputs (committed, so builds do not need this script):
  packaging/macos/AppIcon.icns   macOS app icon (PNG-compressed icns)
  packaging/windows/app.ico      Windows installer / shortcut icon (PNG-compressed ico)
  site/icon.png                  256 px icon for the website (favicon and touch icon)

Design: a dark rounded square holding three gauge bars in the widget's own colours
(green, amber, red = ok, warning, critical), so the icon reads as "usage meters".
"""
import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BIG = 1024  # master canvas

TOP, BOTTOM = (0x36, 0x36, 0x40), (0x1a, 0x1a, 0x1f)  # background gradient
TRACK = (0x5a, 0x5a, 0x66)
BARS = [  # (centre y, fill fraction, colour)
    (372, 0.58, (0x34, 0xc7, 0x59)),
    (512, 0.80, (0xff, 0xb3, 0x00)),
    (652, 0.93, (0xeb, 0x40, 0x34)),
]
BODY = 824  # Apple's icon grid: an 824 px body centred on the 1024 canvas
BODY_RADIUS = 185
BAR_W, BAR_H, BAR_X0 = 520, 64, 252


def sd_round_rect(px, py, cx, cy, hw, hh, r):
    """Signed distance (negative inside) to a rounded rectangle."""
    qx = abs(px - cx) - (hw - r)
    qy = abs(py - cy) - (hh - r)
    outside = math.hypot(max(qx, 0.0), max(qy, 0.0))
    return outside + min(max(qx, qy), 0.0) - r


def cover(sd):
    return min(1.0, max(0.0, 0.5 - sd))  # one pixel of anti-aliasing


def render():
    """Premultiplied float RGBA rows at BIG x BIG."""
    rows = []
    half_body = BODY / 2
    for y in range(BIG):
        row = []
        py = y + 0.5
        t = min(1.0, max(0.0, (py - (BIG - BODY) / 2) / BODY))
        bg = tuple(TOP[i] + (BOTTOM[i] - TOP[i]) * t for i in range(3))
        for x in range(BIG):
            px = x + 0.5
            a = cover(sd_round_rect(px, py, BIG / 2, BIG / 2, half_body, half_body, BODY_RADIUS))
            if a <= 0.0:
                row.append((0.0, 0.0, 0.0, 0.0))
                continue
            # faint inner highlight along the top edge
            hl = 0.10 * max(0.0, 1.0 - (py - (BIG - BODY) / 2) / 40.0)
            r, g, b = (min(255.0, c + 255.0 * hl) for c in bg)
            if abs(py - 512) < 330 and BAR_X0 - 40 < px < BAR_X0 + BAR_W + 40:
                for cy, frac, colour in BARS:
                    if abs(py - cy) > BAR_H:
                        continue
                    ta = cover(sd_round_rect(px, py, BAR_X0 + BAR_W / 2, cy, BAR_W / 2, BAR_H / 2, BAR_H / 2))
                    if ta > 0:
                        r, g, b = (r + (TRACK[i] - (r, g, b)[i]) * ta for i in range(3))
                    fw = BAR_W * frac
                    fa = cover(sd_round_rect(px, py, BAR_X0 + fw / 2, cy, fw / 2, BAR_H / 2, BAR_H / 2))
                    if fa > 0:
                        r, g, b = (r + (colour[i] - (r, g, b)[i]) * fa for i in range(3))
            row.append((r * a, g * a, b * a, a * 255.0))
        rows.append(row)
    return rows


def shrink(rows, factor):
    """Box filter in premultiplied space."""
    n = BIG // factor
    out = []
    area = factor * factor
    for y in range(n):
        row = []
        for x in range(n):
            sr = sg = sb = sa = 0.0
            for dy in range(factor):
                src = rows[y * factor + dy]
                for dx in range(factor):
                    pr, pg, pb, pa = src[x * factor + dx]
                    sr += pr; sg += pg; sb += pb; sa += pa
            row.append((sr / area, sg / area, sb / area, sa / area))
        out.append(row)
    return out


def to_png(rows):
    h, w = len(rows), len(rows[0])
    raw = bytearray()
    for row in rows:
        raw.append(0)
        for pr, pg, pb, pa in row:
            if pa <= 0.0:
                raw += b"\x00\x00\x00\x00"
                continue
            k = 255.0 / pa  # un-premultiply for PNG
            raw += bytes(max(0, min(255, round(v))) for v in (pr * k, pg * k, pb * k, pa))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(bytes(raw), 9)) + chunk(b"IEND", b""))


def icns(pngs):
    """pngs: {pixel size: png bytes}. Chunk types per Apple's icns format (PNG payloads)."""
    layout = [(b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128), (b"ic08", 256),
              (b"ic09", 512), (b"ic10", 1024), (b"ic11", 32), (b"ic12", 64), (b"ic13", 256), (b"ic14", 512)]
    body = b"".join(t + struct.pack(">I", 8 + len(pngs[s])) + pngs[s] for t, s in layout)
    return b"icns" + struct.pack(">I", 8 + len(body)) + body


def ico(pngs, sizes=(16, 32, 64, 128, 256)):
    entries, data = b"", b""
    offset = 6 + 16 * len(sizes)
    for s in sizes:
        png = pngs[s]
        entries += struct.pack("<BBBBHHII", 0 if s == 256 else s, 0 if s == 256 else s, 0, 0, 1, 32, len(png), offset)
        data += png
        offset += len(png)
    return struct.pack("<HHH", 0, 1, len(sizes)) + entries + data


def main():
    big = render()
    pngs = {BIG: to_png(big)}
    for size in (512, 256, 128, 64, 32, 16):
        pngs[size] = to_png(shrink(big, BIG // size))
    (ROOT / "packaging/macos").mkdir(parents=True, exist_ok=True)
    (ROOT / "packaging/windows").mkdir(parents=True, exist_ok=True)
    (ROOT / "packaging/macos/AppIcon.icns").write_bytes(icns(pngs))
    (ROOT / "packaging/windows/app.ico").write_bytes(ico(pngs))
    (ROOT / "site/icon.png").write_bytes(pngs[256])
    print("wrote AppIcon.icns, app.ico, site/icon.png")


if __name__ == "__main__":
    main()
