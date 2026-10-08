# -*- coding: utf-8 -*-
"""录音笔同步工具（手动运行版）。

发现录音笔 → 连接 → 拉文件列表 → 增量下载新文件 → 封装为 OGG → 校验 → 本地存档。
- 保存为 .ogg（Opus 无损重封装，任何播放器可放）；无 ffmpeg 时退化为裸 .opus
- 已在笔上的文件不删除；本地已有的文件跳过
- 每文件下载后用 ffmpeg 实际解码校验时长，损坏的不落盘、下次重下
- 断线自动重连续传

用法：
    python sync.py                 # 等笔 10 分钟，同步到 ./recordings
    python sync.py --wait 30       # 等笔 30 分钟
    python sync.py --out D:\\备份\\录音

退出码：0 正常（含无新文件），2 没找到笔，1 出错。
"""
import argparse
import asyncio
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from pnote import PNoteClient, find_device, log
from opuswrap import wrap_opus_blocks, verify_ogg

sys.stdout.reconfigure(encoding="utf-8")

EXIT_OK = 0
EXIT_ERROR = 1
EXIT_NOT_FOUND = 2

DEFAULT_FFMPEG = Path(r"D:\SynologyDrive\Job\Project\AI\ffmpeg\ffmpeg.exe")


def make_progress(label):
    last = [0]

    def on_progress(n):
        if n - last[0] >= 262144:  # 每 256KB 报一次
            log(f"  [{label}] 已收 {n/1024:.0f} KB")
            last[0] = n

    return on_progress


async def sync_once(wait_seconds, out_dir: Path, ffmpeg: Path):
    log(f"=== 同步开始：等待录音笔（最长 {wait_seconds/60:.0f} 分钟）===")
    device = await find_device(timeout=wait_seconds)
    if device is None:
        log("未发现录音笔（休眠或不在范围内）。请唤醒后重新运行。")
        return EXIT_NOT_FOUND

    have_ffmpeg = ffmpeg.exists()

    async with await PNoteClient(device).connect(device) as pen:
        log(f"已连接 {device.name} ({device.address})")
        await pen.sync_time()
        battery = await pen.get_battery()
        log(f"电量 {battery}{'（充电中）' if battery == 110 else '%'}")

        files = await pen.get_file_list()
        log(f"设备里共 {len(files)} 个文件")

        out_dir.mkdir(parents=True, exist_ok=True)
        new_files, skipped, failed = [], [], []

        for e in files:
            try:
                out_path = await download_one(pen, e, out_dir, ffmpeg, have_ffmpeg)
            except Exception as ex:
                log(f"✗ {e.name} 下载失败：{type(ex).__name__}: {ex}")
                failed.append(e)
                continue
            if out_path is None:
                skipped.append(e)
            else:
                new_files.append(out_path)

    log(f"=== 同步结束：新增 {len(new_files)}，跳过 {len(skipped)}，失败 {len(failed)} ===")
    return EXIT_OK if not failed else EXIT_ERROR


async def download_one(pen, entry, out_dir: Path, ffmpeg: Path, have_ffmpeg: bool):
    """下载单个文件并落盘（先写 .part 再改名，防半截文件）。返回 Path；已存在返回 None。"""
    ext = ".ogg" if have_ffmpeg else ".opus"
    out_path = out_dir / f"{entry.base}{ext}"
    if out_path.exists():
        return None
    log(f">>> 下载 {entry.name}（{entry.size/1024:.0f} KB，约 {entry.time} 秒音频）...")
    data, used = await pen.download(entry, preferred=".opus",
                                    on_progress=make_progress(entry.name))

    content = wrap_opus_blocks(data) if have_ffmpeg else data
    if have_ffmpeg and not verify_ogg(content, entry.time, ffmpeg):
        raise ValueError(f"解码校验失败：时长与列表值 {entry.time}s 不符，未保存")
    tmp = out_path.with_name(out_path.name + ".part")
    tmp.write_bytes(content)
    tmp.rename(out_path)
    kind = "OGG/Opus，已校验" if have_ffmpeg else "裸 Opus"
    log(f"✓ 已保存 {out_path.name}（{len(data)/1024:.0f} KB，{kind}）")
    return out_path


def main():
    ap = argparse.ArgumentParser(description="CB08 录音笔同步工具")
    ap.add_argument("--wait", type=float, default=10, help="等待录音笔的分钟数（默认 10）")
    ap.add_argument("--out", default="recordings", help="本地存档目录（默认 ./recordings）")
    ap.add_argument("--ffmpeg", default=str(DEFAULT_FFMPEG), help="ffmpeg 路径")
    args = ap.parse_args()

    Path("run_log.txt").write_text("", encoding="utf-8")
    try:
        code = asyncio.run(sync_once(args.wait * 60, Path(args.out), Path(args.ffmpeg)))
    except KeyboardInterrupt:
        log("用户中断")
        code = EXIT_ERROR
    if sys.stdin.isatty():  # 手动运行时不闪退；后台/计划任务跳过
        try:
            input("\n按回车退出...")
        except EOFError:
            pass
    sys.exit(code)


if __name__ == "__main__":
    main()
