# -*- coding: utf-8 -*-
"""录音笔 OPUS 裸块 ↔ OGG/Opus 封装。

设备文件格式（实测验证）：
    块0     = 40 字节文件头（"KA" 开头，其余基本为 0）
    块1..N  = 每块 40 字节 = 一个 20ms Opus 包（16kHz 单声道）
封装为 OGG 后 ffmpeg/VLC/浏览器均可直接播放，且是无损重打包（不重编码）。
"""
import struct
import subprocess
import tempfile
from pathlib import Path

BLOCK = 40
SAMPLES_PER_PKT = 960  # 20ms @ 48kHz（Opus 内部时钟）

_CRC_TABLE = []
for i in range(256):
    r = i << 24
    for _ in range(8):
        r = ((r << 1) ^ 0x04C11DB7) & 0xFFFFFFFF if r & 0x80000000 else (r << 1) & 0xFFFFFFFF
    _CRC_TABLE.append(r)


def _ogg_crc(data: bytes) -> int:
    crc = 0
    for b in data:
        crc = ((crc << 8) & 0xFFFFFFFF) ^ _CRC_TABLE[((crc >> 24) & 0xFF) ^ b]
    return crc


def _ogg_page(payload: bytes, granule: int, serial: int, seq: int, flags: int) -> bytes:
    segs = []
    n = len(payload)
    while True:
        segs.append(min(255, n))
        if n < 255:
            break
        n -= 255
    header = (
        b"OggS" + bytes([0, flags]) + struct.pack("<q", granule)
        + struct.pack("<II", serial, seq) + b"\x00\x00\x00\x00"
        + bytes([len(segs)]) + bytes(segs)
    )
    page = header + payload
    return page[:22] + struct.pack("<I", _ogg_crc(page)) + page[26:]


def wrap_opus_blocks(data: bytes, serial: int = 0xCB08) -> bytes:
    """设备文件字节流 → OGG 字节流。"""
    blocks = [data[i : i + BLOCK] for i in range(0, len(data), BLOCK)]
    packets = blocks[1:]  # 块0 是文件头
    head = (
        b"OpusHead" + bytes([1, 1]) + struct.pack("<H", 312)
        + struct.pack("<I", 16000) + struct.pack("<h", 0) + bytes([0])
    )
    tags = b"OpusTags" + struct.pack("<I", 10) + b"pnote-sync" + struct.pack("<I", 0)
    pages = [_ogg_page(head, 0, serial, 0, 2), _ogg_page(tags, 0, serial, 1, 0)]
    n = len(packets)
    for i, pkt in enumerate(packets):
        flags = 4 if i == n - 1 else 0  # EOS 必须在算 CRC 之前定好
        pages.append(_ogg_page(pkt, (i + 1) * SAMPLES_PER_PKT, serial, i + 2, flags))
    return b"".join(pages)


def verify_ogg(ogg_bytes: bytes, expect_seconds, ffmpeg: Path, tolerance=0.1) -> bool:
    """用 ffmpeg 实际解码，校验时长与文件列表中的录音时长一致。"""
    with tempfile.TemporaryDirectory() as td:
        ogg = Path(td) / "t.ogg"
        wav = Path(td) / "t.wav"
        ogg.write_bytes(ogg_bytes)
        r = subprocess.run(
            [str(ffmpeg), "-y", "-v", "error", "-i", str(ogg), str(wav)],
            capture_output=True, timeout=180,
        )
        if r.returncode != 0:
            return False
        import wave

        with wave.open(str(wav), "rb") as w:
            dur = w.getnframes() / w.getframerate()
    return abs(dur - expect_seconds) <= max(tolerance, expect_seconds * 0.05)
