# -*- coding: utf-8 -*-
"""马赛福录音卡守护 sidecar。

常驻进程：BLE 长连接挂住录音卡，轮询文件列表并增量下载（OGG 封装 + ffmpeg 时长校验；
校验异常降级为非致命直接保存，下载失败退避 3 轮防死循环重下）。
守护主循环移植自 录音卡/pnote_sync/daemon.py，退出由 stop_event 驱动（替代 KeyboardInterrupt）。

控制通道（JSON-lines，stdout 只出协议行，日志全走 stderr）：
    stdin 每行一个请求:  {"id": 1, "cmd": "start|stop|status|file_list|sync_now|gc_device|shutdown", ...}
    stdout 每行一个响应: {"type": "resp", "id": 1, "ok": true, "data": {...}} / {"type": "resp", "id": 1, "ok": false, "error": "..."}
    stdout 事件:         {"type": "event", "event": "ready|state|progress|file_synced", "data": {...}}

用法:
    python pnote_sidecar.py --out-dir <OGG归档目录> [--ffmpeg <路径>] [--device-name cb08]
退出: stdin EOF 或 {"cmd": "shutdown"}，退出码 0。
"""
import argparse
import asyncio
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

PROTOCOL_VERSION = 1
POLL_SECONDS = 30.0
SCAN_CHUNK_SECONDS = 15.0
# 协议怪癖（测试记录）：笔反复重连 25+ 次后笔侧协议栈卡死，超限提示用户重启录音卡
MAX_RECONNECT_STREAK = 25


class DeviceNotFound(RuntimeError):
    """扫描窗口内未发现录音卡（休眠或不在范围内）。"""


def log(msg: str):
    print(f"MASAIFU_LOG {msg}", file=sys.stderr, flush=True)


def emit(event: str, data: dict):
    print(json.dumps({"type": "event", "event": event, "data": data}, ensure_ascii=False), flush=True)


def respond(rid, ok: bool, data=None, error=None):
    msg = {"type": "resp", "id": rid, "ok": ok}
    if ok:
        msg["data"] = data
    else:
        msg["error"] = error
    print(json.dumps(msg, ensure_ascii=False), flush=True)


class _Libs:
    """延迟加载的 bleak 相关模块（本机未装 bleak 时 status/stop/shutdown 仍可用）。"""


class Daemon:
    """移植 daemon.py 主循环 + wait_and_connect。

    差异：日志走 stderr；每轮迭代后更新内存状态并推 state 事件；
    无设备时推 sleeping=true 的 state 事件安静等待；退出由 stop_event 驱动；
    连接类操作全部经 _conn_lock 串行（守护循环与 sync_now/file_list/gc_device 不会并发抢连接）。
    """

    def __init__(self, out_dir: Path, ffmpeg: Path | None, device_name: str, poll: float):
        self.out_dir = out_dir
        self.ffmpeg = ffmpeg
        self.have_ffmpeg = ffmpeg is not None and ffmpeg.exists()
        self.device_name = device_name
        self.poll = poll
        self.pen = None
        self.running = False
        self.stop_event = asyncio.Event()
        self.task: asyncio.Task | None = None
        self._conn_lock = asyncio.Lock()
        self._libs: _Libs | None = None
        self._fail_cooldown: dict[str, int] = {}  # 文件名 -> 剩余退避轮数（下载失败后置 3）
        self._no_bleak = False  # 无 bleak 环境（调试机）：守护循环以纯休眠模式运行
        self.since: str | None = None  # 同步窗口：'YYYYMMDD' 紧凑串，只收该日期起的录音；None=全部
        self.state = {
            "running": False,
            "connected": False,
            "battery": None,
            "charging": False,
            "capacity": None,
            "version": None,
            "sleeping": False,
        }

    # ---- 模块加载（bleak 延迟到真正连接时）----

    def _load_libs(self) -> _Libs:
        if self._libs is None:
            try:
                from bleak import BleakError
                from opuswrap import verify_ogg, wrap_opus_blocks
                from pnote import PNoteClient, find_device
            except ImportError as e:
                raise RuntimeError(f"蓝牙组件 bleak 不可用：{e}") from e
            libs = _Libs()
            libs.BleakError = BleakError
            libs.PNoteClient = PNoteClient
            libs.find_device = find_device
            libs.wrap_opus_blocks = wrap_opus_blocks
            libs.verify_ogg = verify_ogg
            self._libs = libs
        return self._libs

    def _conn_errors(self) -> tuple:
        try:
            bleak_err = self._load_libs().BleakError
        except RuntimeError:
            bleak_err = ()
        return (ConnectionError, OSError, asyncio.TimeoutError, bleak_err)

    # ---- 状态与事件 ----

    def _push_state(self, **kw):
        changed = any(self.state.get(k) != v for k, v in kw.items())
        self.state.update(kw)
        if changed:  # state 事件只在变化时推（协议约定：连接状态变化时推）
            emit("state", dict(self.state))

    def _clear_device_state(self):
        self._push_state(connected=False, battery=None, charging=False,
                         capacity=None, version=None)

    def snapshot(self) -> dict:
        return dict(self.state)

    # ---- 连接管理（调用方须持有 _conn_lock）----

    async def _disconnect_pen(self):
        pen, self.pen = self.pen, None
        if pen is None:
            return
        try:
            await pen.stop_event_router()
        except Exception:
            pass
        try:
            await pen.disconnect()
        except Exception:
            pass

    async def _refresh_device_info(self):
        """连接后/每轮迭代后刷新电量、容量、固件版本并推 state（单项失败不阻塞）。"""
        pen = self.pen
        battery = self.state.get("battery")
        try:
            battery = await pen.get_battery(timeout=5)
        except Exception as e:
            log(f"电量查询失败（沿用上轮值）：{type(e).__name__}: {e}")
        capacity = self.state.get("capacity")
        try:
            capacity = list(await pen.get_capacity(timeout=5))
        except Exception as e:
            log(f"容量查询失败（沿用上轮值）：{type(e).__name__}: {e}")
        version = self.state.get("version")
        try:
            version = await pen.get_version(timeout=5)
        except Exception as e:
            log(f"版本查询失败（沿用上轮值）：{type(e).__name__}: {e}")
        self._push_state(running=True, connected=True, sleeping=False, battery=battery,
                         charging=(battery == 110), capacity=capacity, version=version)

    async def _ensure_connected(self, scan_seconds: float = SCAN_CHUNK_SECONDS):
        """确保已连接并返回 pen。找不到设备抛 DeviceNotFound。"""
        pen = self.pen
        if pen is not None and pen._client is not None and pen._client.is_connected:
            return pen
        await self._disconnect_pen()
        if self._no_bleak:
            raise DeviceNotFound("bleak 不可用，无法扫描设备")
        libs = self._load_libs()
        device = await libs.find_device(name=self.device_name, timeout=scan_seconds)
        if device is None:
            raise DeviceNotFound(f"扫描 {scan_seconds:.0f} 秒未发现录音卡（休眠或不在范围内）")
        log(f"发现设备 {device.name} ({device.address})，正在连接...")
        pen = libs.PNoteClient(device)
        await pen.connect(device)
        log("连接成功，正在对时...")
        await pen.sync_time()
        log("对时完成，启动事件路由")
        pen.start_event_router()  # 按键回执依赖（pnote.py 事件路由，断开前须 stop）
        self.pen = pen
        await self._refresh_device_info()
        return pen

    # ---- 下载（移植 sync.py download_one，事件版）----

    async def _download_one(self, pen, entry) -> Path:
        libs = self._libs
        ext = ".ogg" if self.have_ffmpeg else ".opus"
        out_path = self.out_dir / f"{entry.base}{ext}"
        log(f">>> 下载 {entry.name}（{entry.size / 1024:.0f} KB，约 {entry.time} 秒音频）...")
        total = entry.size
        last = [0]

        def on_progress(n: int):
            if n - last[0] >= 262144:  # 每 256KB 报一次 progress 事件
                last[0] = n
                emit("progress", {"file": entry.name, "received": n, "total": total})

        data, _used = await pen.download(entry, preferred=".opus", on_progress=on_progress)
        content = libs.wrap_opus_blocks(data) if self.have_ffmpeg else data
        verify_state = "raw"  # verified=时长一致 / skipped=校验异常跳过 / mismatch=时长不符 / raw=未校验
        if self.have_ffmpeg:
            try:
                verify_state = "verified" if libs.verify_ogg(content, entry.time, self.ffmpeg) else "mismatch"
            except Exception as e:  # noqa: BLE001 - 校验超时/失败不阻塞保存，转写环节兜底
                log(f"⚠ 时长校验失败/超时（{type(e).__name__}: {e}），跳过校验直接保存（转写环节会兜底验证内容）")
                verify_state = "skipped"
            if verify_state == "mismatch":
                raise ValueError(f"解码校验失败：时长与列表值 {entry.time}s 不符，未保存")
        tmp = out_path.with_name(out_path.name + ".part")  # 先写 .part 再 rename，防半截文件
        tmp.write_bytes(content)
        tmp.rename(out_path)
        kind = {"verified": "OGG/Opus，已校验", "skipped": "OGG/Opus，时长校验跳过", "raw": "裸 Opus"}[verify_state]
        log(f"✓ 已保存 {out_path.name}（{len(data) / 1024:.0f} KB，{kind}）")
        return out_path

    # ---- 同步迭代（daemon.py 每 POLL 秒一轮的主体）----

    async def _sync_iteration(self, pen) -> int:
        # 下载失败退避：每轮轮询把所有 cooldown -1，<=0 的剔除
        for n in [n for n, c in self._fail_cooldown.items() if c <= 1]:
            del self._fail_cooldown[n]
        for n in self._fail_cooldown:
            self._fail_cooldown[n] -= 1
        # 查录音状态，识别正在录制的文件（边录边下拿到的是半成品快照，时长校验会拒绝）
        rec_name = None
        try:
            record_state = await pen.get_record_state(timeout=5)
            if record_state == 1:
                rec_name = await pen.get_current_record_name(timeout=5)
        except asyncio.TimeoutError:
            pass  # 状态查询失败不阻塞同步
        files = await pen.get_file_list()
        new_count = 0
        for e in files:
            # 同步窗口过滤（无状态：窗口外直接跳过不记录，调宽窗口后可再捞回）
            if self.since and len(e.name) >= 12 and e.name[4:12] < self.since:
                continue
            ext = ".ogg" if self.have_ffmpeg else ".opus"
            if (self.out_dir / f"{e.base}{ext}").exists():
                continue
            if self._fail_cooldown.get(e.name, 0) > 0:
                log(f"⏳ {e.name} 上次下载失败，退避中（剩 {self._fail_cooldown[e.name]} 轮），本轮跳过")
                continue
            if rec_name and e.base in rec_name:
                log(f"⏸ {e.name} 正在录制中，本轮跳过（保存后下轮再收）")
                continue
            log(f"🎙 发现新录音 {e.name}（{e.size / 1024:.0f} KB，约 {e.time} 秒）")
            try:
                ogg_path = await self._download_one(pen, e)
            except Exception:
                self._fail_cooldown[e.name] = 3  # 失败退避 3 轮，防任何原因的失败立刻全量重下
                raise
            new_count += 1
            # OGG 已校验落盘（.part→rename 完成）才发 file_synced
            emit("file_synced", {
                "name": e.name,
                "ogg": str(ogg_path),
                "seconds": e.time,
                "size": e.size,
            })
        if new_count:
            log(f"本轮同步完成，新增 {new_count} 个文件")
        return new_count

    # ---- 守护主循环（daemon.py run() 移植，stop_event 驱动退出）----

    async def _interruptible_sleep(self, seconds: float):
        try:
            await asyncio.wait_for(self.stop_event.wait(), timeout=seconds)
        except asyncio.TimeoutError:
            pass

    async def _run(self):
        conn_errors = self._conn_errors()
        fail_streak = 0
        waited_log_at = time.time()
        while not self.stop_event.is_set():
            try:
                async with self._conn_lock:
                    if self.stop_event.is_set():
                        break
                    pen = await self._ensure_connected()
                    await self._sync_iteration(pen)
                    await self._refresh_device_info()  # 每轮迭代后回报最新 state
                fail_streak = 0
                await self._interruptible_sleep(self.poll)
            except DeviceNotFound:
                # 笔休眠：安静等待唤醒，不 crash
                if not self.state.get("sleeping"):
                    log("未发现录音卡，进入休眠等待（按录音卡按键唤醒后自动连接）")
                    waited_log_at = time.time()
                self._clear_device_state()
                self._push_state(running=True, sleeping=True)
                if time.time() - waited_log_at > 300:
                    log("仍在等待录音卡唤醒...")
                    waited_log_at = time.time()
                await self._interruptible_sleep(self.poll)
            except conn_errors as e:
                fail_streak += 1
                if fail_streak == MAX_RECONNECT_STREAK:
                    log(f"⚠️ 连续重连已达 {MAX_RECONNECT_STREAK} 次，笔侧协议栈可能卡死，请重启录音卡")
                log(f"连接异常，稍后重连：{type(e).__name__}: {e}")
                async with self._conn_lock:
                    await self._disconnect_pen()
                self._clear_device_state()
                self._push_state(running=True, sleeping=False)
                await self._interruptible_sleep(60 if fail_streak >= MAX_RECONNECT_STREAK else 3)
            except asyncio.CancelledError:
                raise
            except Exception as e:
                log(f"未预期异常：{type(e).__name__}: {e}")
                await self._interruptible_sleep(5)

    # ---- 控制命令 ----

    async def start(self) -> dict:
        self.out_dir.mkdir(parents=True, exist_ok=True)
        try:
            self._load_libs()
        except RuntimeError as e:
            # 无 bleak 的调试机：循环以纯休眠模式运行，state 事件照常（协议不塌）
            self._no_bleak = True
            log(f"{e}；守护循环以休眠模式运行（本机永远扫不到设备）")
        if self.task is not None and not self.task.done():
            return {"running": True}
        self.stop_event.clear()
        self.running = True
        self._push_state(running=True)
        self.task = asyncio.create_task(self._run())
        return {"running": True}

    async def stop(self) -> dict:
        self.running = False
        self.stop_event.set()
        task, self.task = self.task, None
        if task is not None:
            try:
                await asyncio.wait_for(task, timeout=30)
            except (asyncio.TimeoutError, asyncio.CancelledError):
                task.cancel()
                log("守护循环停止超时，已取消")
        async with self._conn_lock:
            await self._disconnect_pen()
        self._clear_device_state()
        self._push_state(running=False, sleeping=False)
        return {"running": False}

    async def status(self) -> dict:
        # 无连接也必须能应答（ok:true, connected:false）
        return self.snapshot()

    async def file_list(self) -> dict:
        async with self._conn_lock:
            pen = await self._ensure_connected()
            files = await pen.get_file_list()
        return {"files": [{"name": e.name, "size": e.size, "seconds": e.time} for e in files]}

    async def sync_now(self) -> dict:
        async with self._conn_lock:
            pen = await self._ensure_connected()
            new = await self._sync_iteration(pen)
            await self._refresh_device_info()
        return {"new": new}

    async def connect(self) -> dict:
        """立即扫描并连接（幂等：已连接时仅刷新设备信息）。
        供断开/休眠状态下手动重连，不等守护循环的退避间隔。"""
        async with self._conn_lock:
            await self._ensure_connected()
            await self._refresh_device_info()
        return self.snapshot()

    async def config(self, since: str | None = None) -> dict:
        """更新同步窗口：since 为 'YYYYMMDD' 紧凑串（只收该日起的录音），空/None=全部。"""
        self.since = since or None
        log(f"同步窗口：{'全部' if self.since is None else self.since + ' 起'}")
        return {"since": self.since}

    async def gc_device(self, names: list) -> dict:
        async with self._conn_lock:
            pen = await self._ensure_connected()
            files = await pen.get_file_list()
            by_name = {e.name: e for e in files}
            by_base = {e.base: e for e in files}
            deleted, failed = 0, []
            for n in names:
                entry = by_name.get(n) or by_base.get(str(n).rstrip("."))
                if entry is None:
                    failed.append(n)
                    continue
                try:
                    await pen.delete_file(entry)  # 需带与列表相同的 28B raw_entry
                    deleted += 1
                except Exception:
                    failed.append(n)
        log(f"gc_device：请求 {len(names)} 个，删除 {deleted} 个，失败 {len(failed)} 个")
        return {"deleted": deleted, "failed": failed}


async def _handle_request(daemon: Daemon, req: dict):
    rid = req.get("id")
    cmd = req.get("cmd")
    handlers = {
        "start": daemon.start,
        "stop": daemon.stop,
        "status": daemon.status,
        "file_list": daemon.file_list,
        "sync_now": daemon.sync_now,
        "connect": daemon.connect,
        "config": lambda: daemon.config(req.get("since")),
        "gc_device": lambda: daemon.gc_device(list(req.get("names") or [])),
    }
    handler = handlers.get(cmd)
    if handler is None:
        respond(rid, False, error=f"未知命令: {cmd!r}")
        return
    try:
        data = await handler()
        respond(rid, True, data=data)
    except Exception as e:  # noqa: BLE001 - 任何 BLE/IO 异常都转成 ok:false，进程不死
        respond(rid, False, error=f"{type(e).__name__}: {e}")


async def _stdin_lines():
    """后台守护线程逐行读 stdin，经队列转给事件循环。

    不用 connect_read_pipe：Windows Proactor 在 stdin 为文件重定向（非管道）时
    无法注册 IOCP，会抛 WinError 6；线程读法对管道/文件/控制台均适用。
    线程设 daemon：shutdown 后 stdin 仍开着也不挡进程退出。
    """
    import threading

    queue: asyncio.Queue = asyncio.Queue()
    loop = asyncio.get_running_loop()

    def _reader():
        try:
            for line in sys.stdin:
                loop.call_soon_threadsafe(queue.put_nowait, line)
        finally:
            loop.call_soon_threadsafe(queue.put_nowait, None)  # EOF

    threading.Thread(target=_reader, daemon=True, name="stdin-reader").start()
    while True:
        line = await queue.get()
        if line is None:
            return  # stdin EOF = 退出（照 tts sidecar 惯例）
        yield line


async def main() -> int:
    # Windows 下 stdout/stderr 默认跟随控制台代码页（如 GBK），强制 UTF-8 供 Rust 侧解析
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    sys.stdin.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser(description="马赛福录音卡守护 sidecar")
    parser.add_argument("--out-dir", required=True, help="OGG 归档目录")
    parser.add_argument("--ffmpeg", default="", help="ffmpeg 路径（为空则不校验时长，存裸 .opus）")
    parser.add_argument("--device-name", default="cb08", help="设备广播名（默认 cb08）")
    parser.add_argument("--poll", type=float, default=POLL_SECONDS, help="轮询间隔秒数（默认 30）")
    args = parser.parse_args()

    ffmpeg = Path(args.ffmpeg) if args.ffmpeg else None
    daemon = Daemon(Path(args.out_dir), ffmpeg, args.device_name, args.poll)

    # Rust 等这行确认进程可用
    emit("ready", {"version": PROTOCOL_VERSION})

    async for line in _stdin_lines():
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except json.JSONDecodeError as e:
            respond(None, False, error=f"无法解析的输入行: {e}")
            continue
        if req.get("cmd") == "shutdown":
            respond(req.get("id"), True, data={"shutdown": True})
            break
        await _handle_request(daemon, req)
    await daemon.stop()
    return 0


if __name__ == "__main__":
    try:
        sys.exit(asyncio.run(main()))
    except Exception as e:  # noqa: BLE001 - sidecar 需要把任何异常暴露给调用方
        print(f"pnote-sidecar 失败: {type(e).__name__}: {e}", file=sys.stderr)
        sys.exit(1)
