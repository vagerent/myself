"""生成马赛福应用图标（纯 Python 标准库，无外部依赖）。"""

import math
import os
import struct
import zlib


def draw(size: int) -> bytes:
    px = bytearray(size * size * 4)
    c = size / 2.0
    r = 0.22 * size
    ring_r = 0.30 * size
    ring_w = 0.055 * size
    dot_r = 0.075 * size
    dark = [66, 53, 0]  # 深棕，与亮黄底形成对比
    for y in range(size):
        for x in range(size):
            qx = abs(x - c) - (size / 2.0 - r)
            qy = abs(y - c) - (size / 2.0 - r)
            d = math.hypot(max(qx, 0.0), max(qy, 0.0)) + min(max(qx, qy), 0.0) - r
            if d > 0:
                continue
            t = (x + y) / (2.0 * size)
            # 亮黄对角渐变：#ffc800 -> #ffe733
            col = [255.0, 200 + 31 * t, 51 * t]
            dist = math.hypot(x - c, y - c)
            accent = max(0.0, 1.0 - dist / (size * 0.55))
            col = [col[0], col[1] + 12 * accent, col[2] + 30 * accent]
            dr = abs(dist - ring_r)
            if dr <= ring_w:
                f = 1.0 - dr / ring_w
                col = [ch + (dk - ch) * f * 0.92 for ch, dk in zip(col, dark)]
            if dist <= dot_r:
                col = [ch + (dk - ch) * 0.9 for ch, dk in zip(col, dark)]
            idx = (y * size + x) * 4
            px[idx] = int(col[0])
            px[idx + 1] = int(col[1])
            px[idx + 2] = int(col[2])
            px[idx + 3] = 255
    return bytes(px)


def write_png(path: str, size: int, rgba: bytes) -> None:
    raw = b"".join(
        b"\x00" + rgba[y * size * 4 : (y + 1) * size * 4] for y in range(size)
    )

    def chunk(tag: bytes, data: bytes) -> bytes:
        out = struct.pack(">I", len(data)) + tag + data
        out += struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        return out

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def write_ico(path: str, png_bytes: bytes) -> None:
    data = struct.pack("<HHH", 0, 1, 1)
    data += struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(png_bytes), 22)
    data += png_bytes
    with open(path, "wb") as f:
        f.write(data)


def main() -> None:
    out_dir = os.path.join(os.path.dirname(__file__), "..", "src-tauri", "icons")
    os.makedirs(out_dir, exist_ok=True)
    specs = [
        ("32x32.png", 32),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 256),
    ]
    big_png = None
    for name, size in specs:
        rgba = draw(size)
        write_png(os.path.join(out_dir, name), size, rgba)
        if size == 256:
            big_png = rgba
            with open(os.path.join(out_dir, "icon.ico.tmp"), "wb") as f:
                pass
    # 用 256 PNG 封装 ICO（Windows Vista+ 支持 PNG 压缩条目）
    if big_png is not None:
        tmp = os.path.join(out_dir, "icon.ico.tmp")
        raw = b"".join(
            b"\x00" + big_png[y * 256 * 4 : (y + 1) * 256 * 4] for y in range(256)
        )

        def chunk(tag: bytes, data: bytes) -> bytes:
            out = struct.pack(">I", len(data)) + tag + data
            out += struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
            return out

        png = b"\x89PNG\r\n\x1a\n"
        png += chunk(b"IHDR", struct.pack(">IIBBBBB", 256, 256, 8, 6, 0, 0, 0))
        png += chunk(b"IDAT", zlib.compress(raw, 9))
        png += chunk(b"IEND", b"")
        write_ico(os.path.join(out_dir, "icon.ico"), png)
        os.remove(tmp)
    print("icons written to", out_dir)


if __name__ == "__main__":
    main()
