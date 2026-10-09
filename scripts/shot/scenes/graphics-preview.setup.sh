mkdir -p "$WORK/data"
cat > "$WORK/data/config.json" <<'JSON'
{"prewarm_agents": false, "prewarm_sessions": false, "inline_graphics": "halfblocks"}
JSON

python3 - "$DEMO/mockup.png" <<'PY'
import struct, sys, zlib

path = sys.argv[1]
width, height = 48, 24
rows = []
for y in range(height):
    row = bytearray([0])
    for x in range(width):
        if 4 <= x < 44 and 4 <= y < 20:
            r, g, b, a = 80 + x * 3, 120 + y * 4, 230, 255
        elif (x + y) % 6 < 3:
            r, g, b, a = 245, 180, 70, 255
        else:
            r, g, b, a = 40, 48, 68, 255
        row.extend([r % 256, g % 256, b, a])
    rows.append(bytes(row))

def chunk(kind, data):
    return (
        struct.pack(">I", len(data))
        + kind
        + data
        + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
    )

png = b"\x89PNG\r\n\x1a\n"
png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
png += chunk(b"IDAT", zlib.compress(b"".join(rows)))
png += chunk(b"IEND", b"")
open(path, "wb").write(png)
PY

cat > "$DEMO/diagram.mmd" <<'EOF'
flowchart LR
  User["User asks Claude"] --> File["Claude writes mockup.png"]
  File --> Open["nebula open mockup.png"]
  Open --> Preview["inline image preview"]
EOF

git -C "$DEMO" add mockup.png diagram.mmd
git -C "$DEMO" -c user.name=shot -c user.email=shot@example.invalid commit -q -m "add graphics previews"
