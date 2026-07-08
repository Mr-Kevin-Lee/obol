#!/usr/bin/env python3
"""Regenerates crates/core/tests/fixtures/chase_statement_sample.pdf.

The previous version of this fixture had its xref byte offsets computed
by hand, which produced a malformed trailer that `pdf-extract`/`lopdf`
rejected outright ("invalid file trailer"). This script computes every
offset programmatically from the actual encoded bytes instead, so there's
no manual counting to get wrong.

All content is synthetic placeholder data — no real account numbers,
balances, or statement text.
"""

import pathlib

OUT_PATH = pathlib.Path(__file__).resolve().parent.parent / "crates/core/tests/fixtures/chase_statement_sample.pdf"

content_stream = (
    b"BT /F1 12 Tf 72 720 Td (CHASE) Tj "
    b"0 -20 Td (Chase Checking Statement) Tj "
    b"0 -20 Td (Account ending in 6789) Tj "
    b"0 -20 Td (Statement Date: 06/30/2026) Tj "
    b"0 -20 Td (New Balance $1,234.56) Tj ET"
)

objects = [
    b"<< /Type /Catalog /Pages 2 0 R >>",
    b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    b"<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> "
    b"/MediaBox [0 0 612 792] /Contents 5 0 R >>",
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    b"<< /Length %d >>\nstream\n%s\nendstream" % (len(content_stream), content_stream),
]

buf = bytearray()
buf += b"%PDF-1.4\n"

offsets = []
for i, body in enumerate(objects, start=1):
    offsets.append(len(buf))
    buf += b"%d 0 obj\n%s\nendobj\n" % (i, body)

xref_offset = len(buf)
n = len(objects) + 1  # + the free object 0

buf += b"xref\n"
buf += b"0 %d\n" % n
buf += b"0000000000 65535 f \n"
for off in offsets:
    buf += b"%010d 00000 n \n" % off

buf += b"trailer\n"
buf += b"<< /Size %d /Root 1 0 R >>\n" % n
buf += b"startxref\n"
buf += b"%d\n" % xref_offset
buf += b"%%EOF"

OUT_PATH.parent.mkdir(parents=True, exist_ok=True)
OUT_PATH.write_bytes(bytes(buf))
print(f"wrote {OUT_PATH} ({len(buf)} bytes)")
