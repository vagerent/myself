# -*- coding: utf-8 -*-
"""录音笔 OPUS 裸块 ↔ OGG/Opus 封装。

设备文件格式（实测验证）：
    块0     = 40 字节文件头（"KA" 开头，其余基本为 0）
    块1..N  = 每块 40 字节 = 一个 20ms Opus 包（16kHz 单声道）
封装为 OGG 后 ffmpeg/VLC/浏览器均可直接播放，且是无损重打包（不重编码）。

时长校验不依赖 ffmpeg：OGG 页头自带 granule 位置（48kHz 采样时钟），
ogg_duration_seconds 纯解析页结构即可（RFC 7845：时长 = 末页 granule / 48000）。
"""
import struct

BLOCK = 40
SAMPLES_PER_PKT = 960  # 20ms @ 48kHz（Opus 内部时钟）
GRANULE_RATE = 48000

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


def ogg_duration_seconds(ogg_bytes: bytes) -> float | None:
    """解析 OGG 页头的 granule 位置算时长（秒），不依赖 ffmpeg。

    OggOpus 时长 = 最大 granule / 48000（RFC 7845）。封装由本模块自己生成，
    页结构必然合法；解析失败（截断/非 OGG）返回 None，调用方降级处理。
    """
    pos = 0
    n = len(ogg_bytes)
    max_granule = -1
    while pos + 27 <= n:
        if ogg_bytes[pos : pos + 4] != b"OggS":
            return None
        granule = int.from_bytes(ogg_bytes[pos + 6 : pos + 14], "little", signed=True)
        if granule > max_granule:
            max_granule = granule
        nsegs = ogg_bytes[pos + 26]
        table = pos + 27
        if table + nsegs > n:
            return None
        payload_len = sum(ogg_bytes[table : table + nsegs])
        pos = table + nsegs + payload_len
    if max_granule < 0:
        return None
    return max_granule / GRANULE_RATE
