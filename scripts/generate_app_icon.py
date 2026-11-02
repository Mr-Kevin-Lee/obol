#!/usr/bin/env python3
"""Generate the source PNG for the desktop app icon (spec D43, task 105).

A coin — an obol is the fare paid to Charon — drawn in the project's own
Okabe-Ito palette (spec §14) so the app icon can't drift from the
colours the dashboard itself uses. Standard library only: writing a PNG
by hand is a few lines of zlib and struct, and is cheaper than taking a
dependency on an image library for one build-time asset.

Run from the repo root, then feed the result to `cargo tauri icon`:

    python3 -I scripts/generate_app_icon.py /tmp/obol-icon.png
    cargo tauri icon /tmp/obol-icon.png -o crates/desktop/icons

Committing the generated icons rather than running this at build time
keeps the build hermetic; this script exists so they're reproducible,
not so they're regenerated.
"""

import struct
import sys
import zlib

SIZE = 1024
SAMPLES = 3  # per axis, for anti-aliased edges

# Okabe-Ito, same values as crates/cli/src/dashboard.rs and the Svelte UI.
BLUE = (0, 114, 178)
ORANGE = (230, 159, 0)

COIN_RADIUS = 480.0
RING_OUTER = 330.0
RING_INNER = 270.0


def coverage(cx: float, cy: float, radius: float) -> float:
    """Fraction of a pixel's supersamples falling inside a circle."""
    hits = 0
    step = 1.0 / SAMPLES
    for sy in range(SAMPLES):
        for sx in range(SAMPLES):
            dx = cx + (sx + 0.5) * step - SIZE / 2.0
            dy = cy + (sy + 0.5) * step - SIZE / 2.0
            if dx * dx + dy * dy <= radius * radius:
                hits += 1
    return hits / (SAMPLES * SAMPLES)


def build_rows() -> bytes:
    rows = bytearray()
    for y in range(SIZE):
        rows.append(0)  # PNG filter type 0 (None)
        for x in range(SIZE):
            coin = coverage(x, y, COIN_RADIUS)
            if coin == 0.0:
                rows.extend((0, 0, 0, 0))
                continue
            # The ring is the annulus between the two radii: inside the
            # outer circle but outside the inner one.
            ring = coverage(x, y, RING_OUTER) * (1.0 - coverage(x, y, RING_INNER))
            r = round(BLUE[0] * (1 - ring) + ORANGE[0] * ring)
            g = round(BLUE[1] * (1 - ring) + ORANGE[1] * ring)
            b = round(BLUE[2] * (1 - ring) + ORANGE[2] * ring)
            rows.extend((r, g, b, round(coin * 255)))
    return bytes(rows)


def chunk(kind: bytes, payload: bytes) -> bytes:
    return (
        struct.pack(">I", len(payload))
        + kind
        + payload
        + struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
    )


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <output.png>")

    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)  # 8-bit RGBA
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(build_rows(), 9))
        + chunk(b"IEND", b"")
    )

    with open(sys.argv[1], "wb") as out:
        out.write(png)
    print(f"wrote {sys.argv[1]} ({len(png)} bytes, {SIZE}x{SIZE})")


if __name__ == "__main__":
    main()
