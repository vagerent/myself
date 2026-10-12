# -*- coding: utf-8 -*-
"""CB08 录音笔 BLE 协议客户端（bleak）。

依据：厂商协议 + CB08 真机验证版《录音笔 BLE 通讯协议 V1.0》(协议.md)。

帧格式（总长 = 6 + LEN）:
    [0]   0x5A 帧头
    [1]   SEQ 包序号 0~255 循环
    [2:4] CRC-16/XMODEM，LE（覆盖 LEN 两字节 + DATA）
    [4:6] LEN，LE（= DATA 真实字节数）
    [6:]  DATA = [TYPE:1B][CMD:1B][PARAMS...]

下行整数 LE、上行整数 LE；例外：文件列表 count/time/size 为 BE。
AE22 / AE23 各用独立帧缓存，解析后统一分发。
"""
import asyncio
import datetime
import struct
import sys
from pathlib import Path

from bleak import BleakClient, BleakScanner, BleakError

RUN_LOG = Path(__file__).parent / "run_log.txt"


def set_log_file(path):
    """切换日志文件（守护程序长期运行时用独立日志）。"""
    global RUN_LOG
    RUN_LOG = Path(path)


def log(msg: str):
    line = f"[{datetime.datetime.now():%H:%M:%S}] {msg}"
    # stdout 是 JSON-lines 控制通道，日志一律走 stderr（sidecar 协议约定）
    print(line, file=sys.stderr, flush=True)
    try:
        with RUN_LOG.open("a", encoding="utf-8") as f:
            f.write(line + "\n")
    except OSError:
        pass

SVC_UUID = "0000ae20-0000-1000-8000-00805f9b34fb"
CHAR_WRITE = "0000ae21-0000-1000-8000-00805f9b34fb"
CHAR_NOTIFY_A = "0000ae22-0000-1000-8000-00805f9b34fb"  # 控制应答/音频/列表/文件数据
CHAR_NOTIFY_B = "0000ae23-0000-1000-8000-00805f9b34fb"  # 按键/录音状态

FRAME_HEAD = 0x5A

# TYPE=2 文件通道 cmd
CMD_LIST_REQ = 0
CMD_LIST_DATA = 1
CMD_IMPORT_REQ = 2
CMD_IMPORT_START = 3
CMD_FILE_DATA = 4
CMD_IMPORT_END = 5       # 官方协议：导入结束（兼容旧值 8）
CMD_IMPORT_END_ALT = 8   # 反汇编中出现的兼容值
CMD_IMPORT_STOP = 7
CMD_DELETE_ONE = 8
CMD_DELETE_ALL = 9
CMD_LIST_END = 18

LIST_ENTRY_SIZE = 28
NAME_FIELD_LEN = 20
IMPORT_NAME_LEN = 24  # 2-2 请求的文件名定长 24B，不足补 0

LIST_END_IDLE = 1.2     # 旧固件不发 2-18 时的收尾空闲秒数
TRANSFER_IDLE = 12.0    # 下载中无数据的超时秒数


def crc16_xmodem(data: bytes) -> int:
    crc = 0
    for b in data:
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def build_frame(seq: int, payload: bytes) -> bytes:
    L = len(payload)
    crc = crc16_xmodem(bytes((L & 0xFF, L >> 8)) + payload)
    return bytes((FRAME_HEAD, seq & 0xFF, crc & 0xFF, crc >> 8, L & 0xFF, L >> 8)) + payload


class FrameParser:
    """流式切帧：按 LEN 跨通知重组，一个通知可能含半帧或多帧。"""

    def __init__(self, on_payload):
        self._buf = bytearray()
        self._on_payload = on_payload

    def feed(self, data: bytes):
        self._buf += data
        while True:
            while self._buf and self._buf[0] != FRAME_HEAD:
                del self._buf[0]
            if len(self._buf) < 6:
                return
            L = self._buf[4] | (self._buf[5] << 8)
            total = 6 + L
            if len(self._buf) < total:
                return
            frame = bytes(self._buf[:total])
            del self._buf[:total]
            crc_stored = frame[2] | (frame[3] << 8)
            if crc16_xmodem(frame[4:]) != crc_stored:  # 坏帧：丢帧头重新同步
                continue
            self._on_payload(frame[1], frame[6:])


class FileEntry:
    """文件列表条目。time 官方定义为录音时长（秒），个别固件可能是时间戳。"""

    __slots__ = ("name", "time", "size", "raw_entry")

    def __init__(self, raw_entry: bytes, swapped: bool = False):
        a, b = struct.unpack(">II", raw_entry[:8])
        # 官方：time 在前 size 在后；加启发式防御字段顺序异常
        if not swapped:
            self.time, self.size = a, b
        else:
            self.size, self.time = a, b
        self.name = raw_entry[8:28].split(b"\x00")[0].decode("utf-8", "replace")
        self.raw_entry = raw_entry

    @property
    def base(self) -> str:
        return self.name.rstrip(".")

    def download_candidates(self, preferred=".wav"):
        """按协议 7.2：优先 preferred 扩展名（.wav 设备转码 / .opus 原始码流），
        再另一格式，最后列表原始截断名。"""
        alt = ".opus" if preferred == ".wav" else ".wav"
        cands = [self.base + preferred, self.base + alt]
        if self.name not in cands:
            cands.append(self.name)
        return cands

    def __repr__(self):
        return f"<FileEntry {self.name!r} size={self.size} time={self.time}>"


async def find_device(name="cb08", address=None, timeout=20, max_age=5.0):
    """只响应扫描期间收到的"新鲜"广播，避免 Windows watcher 里的陈旧设备条目。

    陈旧条目是之前多次失败的根因：条目还在，但笔已休眠，连上后 GATT 发现不完整。
    address 传入（绑定 MAC）时只认该地址，忽略广播名——同名设备再多也只连绑定的那张。
    """
    import time

    want_addr = (address or "").strip().upper()

    holder = {}
    found = asyncio.Event()

    def cb(d, ad):
        if want_addr:
            if (d.address or "").upper() != want_addr:
                return
        elif (ad.local_name or d.name or "").lower() != name.lower():
            return
        holder["device"] = d
        holder["at"] = time.monotonic()
        found.set()

    scanner = BleakScanner(detection_callback=cb)
    await scanner.start()
    deadline = time.monotonic() + timeout
    try:
        while time.monotonic() < deadline:
            remain = deadline - time.monotonic()
            try:
                await asyncio.wait_for(found.wait(), timeout=min(1.0, max(0.1, remain)))
            except asyncio.TimeoutError:
                continue
            if time.monotonic() - holder["at"] <= max_age:
                return holder["device"]
            found.clear()
    finally:
        await scanner.stop()
    return None


class PNoteClient:
    def __init__(self, device, name="cb08", address=None):
        self.device = device
        self.device_name = name
        self.device_address = address
        self.seq = 0
        self.events = asyncio.Queue()  # (type, cmd, args, source_uuid)
        self._parser_a = FrameParser(lambda s, p: self._dispatch(p, CHAR_NOTIFY_A))
        self._parser_b = FrameParser(lambda s, p: self._dispatch(p, CHAR_NOTIFY_B))
        self._entry_order_swap = False  # 首次列表后锁定

    def _dispatch(self, payload: bytes, source: str):
        if not payload:
            return
        if len(payload) == 1:  # 仅 TYPE 的 ACK
            self.events.put_nowait((payload[0], None, b"", source))
            return
        self.events.put_nowait((payload[0], payload[1], payload[2:], source))

    async def connect(self, device=None, retries=4):
        """连接并订阅。device 为扫描到的设备对象；失败时重扫再试。"""
        last_err = None
        for attempt in range(1, retries + 1):
            if device is None:
                device = await find_device(name=self.device_name,
                                           address=self.device_address, timeout=20)
                if device is None:
                    last_err = RuntimeError("扫描不到录音笔")
                    log(f"connect: 第 {attempt} 次：扫不到新鲜广播")
                    continue
            self._client = BleakClient(device, timeout=30)
            try:
                await self._client.connect()
                svc_uuids = [s.uuid for s in self._client.services]
                have = {c.uuid for s in self._client.services for c in s.characteristics}
                need = {CHAR_WRITE, CHAR_NOTIFY_A, CHAR_NOTIFY_B}
                missing = need - have
                log(f"connect: 第 {attempt} 次连接成功，发现 {len(svc_uuids)} 个服务，缺 {sorted(u[:8] for u in missing) or '无'}")
                if missing:
                    raise RuntimeError(f"服务发现不完整，缺少: {missing}")
                # Windows 蓝牙栈偶发 GATT 订阅永久阻塞，加超时让它走重试路径
                log("connect: 订阅 AE22...")
                await asyncio.wait_for(
                    self._client.start_notify(CHAR_NOTIFY_A, self._on_notify_a), timeout=10
                )
                log("connect: 订阅 AE23...")
                await asyncio.wait_for(
                    self._client.start_notify(CHAR_NOTIFY_B, self._on_notify_b), timeout=10
                )
                self.device = device
                return self
            except Exception as e:
                last_err = e
                log(f"connect: 第 {attempt} 次失败: {type(e).__name__}: {e}")
                try:
                    await self._client.disconnect()
                except Exception:
                    pass
                device = None  # 下次重试重新扫描
                if attempt < retries:
                    wait = min(2 * attempt, 6)
                    await asyncio.sleep(wait)
        raise last_err

    async def disconnect(self):
        if self._client and self._client.is_connected:
            await self._client.disconnect()

    async def __aenter__(self):
        if self._client is not None and self._client.is_connected:
            return self
        return await self.connect()

    async def __aexit__(self, *exc):
        await self.disconnect()

    def _on_notify_a(self, char, data):
        self._parser_a.feed(bytes(data))

    def _on_notify_b(self, char, data):
        self._parser_b.feed(bytes(data))

    async def send(self, payload: bytes, interval=0.12):
        """整帧一次写入 AE21（协议 7.2 强制约束：2-2 等命令不可拆包）。"""
        self.seq = (self.seq + 1) & 0xFF
        frame = build_frame(self.seq, payload)
        # 链路半死时 write with response 可能永久阻塞，加超时交给上层重试
        await asyncio.wait_for(
            self._client.write_gatt_char(CHAR_WRITE, frame, response=True), timeout=10
        )
        if interval:
            await asyncio.sleep(interval)

    async def send_ack(self):
        """回 ACK 帧（DATA 仅 TYPE 字节，厂商协议 3.2 节）。"""
        await self.send(b"\x03", interval=0)

    # ---- TYPE 0 控制 ----

    async def sync_time(self, when=None):
        now = when or datetime.datetime.now()
        payload = bytes((0, 0)) + struct.pack(
            "<HBBBBB", now.year, now.month, now.day, now.hour, now.minute, now.second
        )
        await self.send(payload)

    async def get_battery(self, timeout=5):
        """返回 0~100，110 表示充电中。"""
        await self.send(bytes((0, 3)))
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            t, cmd, args, _ = await self._next_event(max(0.1, deadline - asyncio.get_event_loop().time()))
            if t == 0 and cmd == 4:
                return args[0]

    async def get_capacity(self, timeout=5):
        """返回 (remain_kb, total_kb)。"""
        await self.send(bytes((0, 1)))
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            t, cmd, args, _ = await self._next_event(max(0.1, deadline - asyncio.get_event_loop().time()))
            if t == 0 and cmd == 2:
                remain, total = struct.unpack("<II", args[:8])
                return remain, total

    async def get_version(self, timeout=5):
        await self.send(bytes((0, 10)))
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            t, cmd, args, _ = await self._next_event(max(0.1, deadline - asyncio.get_event_loop().time()))
            if t == 0 and cmd == 11:
                return args.decode("ascii", "replace")

    # ---- TYPE 3 录音控制 ----

    async def get_record_state(self, timeout=5):
        """返回 1=录音中 / 2=未录音 / 3=暂停；超时返回 None。"""
        await self.send(bytes((3, 19)))
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            t, cmd, args, _ = await self._next_event(max(0.1, deadline - asyncio.get_event_loop().time()))
            if t == 3 and cmd == 20:
                return args[0] if args else None

    async def get_current_record_name(self, timeout=5):
        """正在录制的文件名；未在录音或超时返回 None。"""
        await self.send(bytes((3, 23)))
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            t, cmd, args, _ = await self._next_event(max(0.1, deadline - asyncio.get_event_loop().time()))
            if t == 3 and cmd == 24:
                return args.decode("utf-8", "replace") if args else ""

    # ---- 按键事件守护（连接期间必须回执，否则笔的录音键无效）----

    BUTTON_CMDS = {1: 2, 3: 4, 5: 6, 7: 8}  # 按键事件 -> 回执命令

    def start_event_router(self):
        """启动后台事件路由：自动回执按键事件，其余事件转给 data_events。

        注意：不能就地放回 self.events（同队列 get/put 会构成无挂起点的
        忙等死循环，独占事件循环），必须用独立队列转发。
        """
        self.data_events = asyncio.Queue()
        self._router_task = asyncio.create_task(self._route_events())

    async def stop_event_router(self):
        task = getattr(self, "_router_task", None)
        if task:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

    async def _route_events(self):
        while True:
            t, cmd, args, src = await self.events.get()  # 空队列时正常挂起
            if t == 3 and cmd in self.BUTTON_CMDS:
                try:
                    await self.send(bytes((3, self.BUTTON_CMDS[cmd], 1)), interval=0)
                    log(f"按键事件 cmd={cmd}（开始/保存/暂停/继续），已回执成功")
                except Exception as e:
                    log(f"按键回执失败: {e}")
            else:
                self.data_events.put_nowait((t, cmd, args, src))

    def _consumer_queue(self):
        """事件路由启动后 API 消费者改从 data_events 读，避免与路由竞争。"""
        return getattr(self, "data_events", None) or self.events

    async def _next_event(self, timeout):
        q = self._consumer_queue()
        return await asyncio.wait_for(q.get(), timeout=timeout)

    # ---- TYPE 2 文件 ----

    async def get_file_list(self, timeout=30):
        await self.send(bytes((2, CMD_LIST_REQ)))
        entries = []
        last_data_at = None
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            now = asyncio.get_event_loop().time()
            if last_data_at is not None and now - last_data_at > LIST_END_IDLE:
                return entries  # 旧固件不发 2-18 的兜底
            remain = max(0.1, min(deadline, (last_data_at or now) + LIST_END_IDLE) - now)
            t, cmd, args, _ = await self._next_event(remain)
            if t != 2:
                continue
            if cmd == CMD_LIST_DATA:
                count = struct.unpack(">I", args[:4])[0]
                for i in range(count):
                    raw = args[4 + i * LIST_ENTRY_SIZE : 4 + (i + 1) * LIST_ENTRY_SIZE]
                    if len(raw) < LIST_ENTRY_SIZE:
                        break
                    if not entries and i == 0 and not self._entry_order_swap:
                        self._entry_order_swap = self._looks_swapped(raw)
                    entries.append(FileEntry(raw, swapped=self._entry_order_swap))
                last_data_at = asyncio.get_event_loop().time()
            elif cmd == CMD_LIST_END:
                return entries

    @staticmethod
    def _looks_swapped(raw_entry: bytes) -> bool:
        """防御字段顺序异常：duration 应远小于 size（压缩音频约数 KB/s）。"""
        a, b = struct.unpack(">II", raw_entry[:8])
        return a > 10_000_000 and 0 < b <= 10_000_000  # a 太大不可能是时长

    async def import_file(self, filename, offset=0, on_progress=None, timeout=600, max_link_drops=5):
        """请求导入（下载）单个文件。filename 定长 24B NUL 填充，整帧单写。

        返回 (data, code)：code 0完成 1文件不存在 2offset过大 3其他停止。
        连接中断时自动重连并用 offset 续传（协议原生支持断点续传）。
        """
        buf = bytearray()
        drops = 0
        while True:
            try:
                await self._clear_events()
                await self.sync_time()  # 协议：连接后须先对时
                await self._import_session(filename, offset + len(buf), buf, on_progress, timeout)
                return bytes(buf), self._last_code
            except (ConnectionError, asyncio.TimeoutError, OSError, BleakError) as e:
                drops += 1
                log(f"import_file: 第 {drops} 次中断（{type(e).__name__}: {e}），尝试重连续传（已收 {len(buf)} B）...")
                if drops > max_link_drops:
                    raise
                await self._reconnect()

    async def _import_session(self, filename, offset, buf, on_progress, timeout):
        name_bytes = filename.encode("utf-8")[:IMPORT_NAME_LEN].ljust(IMPORT_NAME_LEN, b"\x00")
        payload = bytes((2, CMD_IMPORT_REQ)) + struct.pack("<I", offset) + name_bytes
        await self.send(payload)

        last_data_at = None
        deadline = asyncio.get_event_loop().time() + timeout
        started = False
        while True:
            if not self._client.is_connected:
                raise ConnectionError(f"下载 {filename} 中连接断开（已收 {len(buf)} 字节）")
            now = asyncio.get_event_loop().time()
            idle_limit = TRANSFER_IDLE if started else 5.0
            if last_data_at is not None and now - last_data_at > idle_limit:
                raise TimeoutError(f"导入 {filename} 数据中断（已收 {len(buf)} 字节）")
            remain = max(0.1, min(deadline, (last_data_at or now) + idle_limit) - now)
            t, cmd, args, _ = await self._next_event(remain)
            if t != 2:
                continue
            if cmd == CMD_IMPORT_START:
                started = True
                last_data_at = asyncio.get_event_loop().time()
            elif cmd == CMD_FILE_DATA:
                buf += args
                last_data_at = asyncio.get_event_loop().time()
                if on_progress:
                    on_progress(len(buf))
            elif cmd in (CMD_IMPORT_END, CMD_IMPORT_END_ALT):
                self._last_code = args[0] if args else 0
                return

    async def _clear_events(self):
        q = self._consumer_queue()
        while True:
            try:
                q.get_nowait()
            except asyncio.QueueEmpty:
                return

    async def _reconnect(self, timeout=180):
        """断线后重连：等新鲜广播 → 连接。笔若休眠需人工唤醒。"""
        deadline = asyncio.get_event_loop().time() + timeout
        while True:
            device = await find_device(timeout=min(20, max(1, deadline - asyncio.get_event_loop().time())))
            if device is None:
                if asyncio.get_event_loop().time() > deadline:
                    raise TimeoutError("重连超时：扫描不到录音笔（可能已休眠，按一下按键唤醒）")
                continue
            try:
                await self.connect(device)
                return
            except Exception as e:
                log(f"_reconnect: 连接失败（{e}），继续重试...")
                if asyncio.get_event_loop().time() > deadline:
                    raise

    async def download(self, entry: FileEntry, preferred=".wav", on_progress=None):
        """按协议 7.2 逐个尝试下载候选名，返回 (data, used_name)。"""
        last_err = None
        for cand in entry.download_candidates(preferred):
            data, code = await self.import_file(cand, on_progress=on_progress)
            if code == 0:
                return data, cand
            last_err = (cand, code)
        raise FileNotFoundError(f"所有候选名均失败: {last_err}")

    async def stop_import(self):
        await self.send(bytes((2, CMD_IMPORT_STOP)))

    async def delete_file(self, entry: FileEntry):
        """删除单个文件：协议要求带与列表相同的 28B 条目。"""
        await self.send(bytes((2, CMD_DELETE_ONE)) + entry.raw_entry)

    async def delete_all_files(self):
        await self.send(bytes((2, CMD_DELETE_ALL)))
