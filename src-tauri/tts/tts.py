# -*- coding: utf-8 -*-
"""马赛福 TTS 组件（Kokoro v1.1-zh ONNX + misaki[zh] G2P sidecar）。

用法：
    tts --model-dir <Kokoro模型目录> --voice <音色名> --out-dir <wav输出目录> [--speed 1.0]

模型目录内容（App 首次使用时从 ModelScope 下载，仓库 onnx-community/Kokoro-82M-v1.1-zh-ONNX）：
    model_quantized.onnx（q8；fp16 版本在 CPU 上输出全 NaN，不可用）、tokenizer.json、voices/<音色名>.bin

协议：启动后从 stdin 逐行读 JSON {"index": 0, "text": "..."}，每合成一句往
stdout 写一行 JSON {"index": 0, "wav": "<绝对路径>", "duration": 秒}；
stdin 关闭后退出。日志与进度写 stderr：
    MASAIFU_PROGRESS <已合成句数>/<总句数未知，固定0，仅作心跳>
    MASAIFU_LOG <消息>
致命错误写 stderr 并以非零码退出。

实现要点：
- G2P 用 misaki 的 version='1.1' frontend（注音符号+声调数字），与 v1.1-zh
  模型的 172 词表完全对齐；英文段没有 espeak 兜底，整段丢弃（词表外 ❓ 被过滤）
- Kokoro 单次推理上限约 510 token，长句按标点 → 词边界('/')切成 ≤400 token 的批
- 每批做能量门限裁剪首尾静音，句尾统一补 0.2s 停顿，保证连播听感自然
"""

import argparse
import json
import os
import sys

import numpy as np

SAMPLE_RATE = 24000
# 单批 token 上限（留余量，模型硬上限 510）
MAX_BATCH_TOKENS = 400
# 句尾停顿（秒）
SENTENCE_PAUSE = 0.2
# 静音裁剪：帧 RMS 低于峰值该比例视为静音
TRIM_THRESH = 0.02
TRIM_FRAME = 512


def log(message: str) -> None:
    print(f"MASAIFU_LOG {message}", file=sys.stderr, flush=True)


def trim_silence(audio: np.ndarray) -> np.ndarray:
    """裁掉首尾低能量段（Kokoro 输出首尾各有一段明显静音）。"""
    if len(audio) < TRIM_FRAME * 4:
        return audio
    n = len(audio) // TRIM_FRAME
    rms = np.sqrt((audio[: n * TRIM_FRAME].reshape(n, TRIM_FRAME) ** 2).mean(axis=1))
    thresh = rms.max() * TRIM_THRESH
    active = np.nonzero(rms > thresh)[0]
    if len(active) == 0:
        return audio
    # 首尾各保留一帧余量，避免切掉气口
    beg = max(active[0] - 1, 0) * TRIM_FRAME
    end = min((active[-1] + 2) * TRIM_FRAME, len(audio))
    return audio[beg:end]


def split_batches(phonemes: str, vocab: dict) -> list[str]:
    """把整句音素切成 ≤MAX_BATCH_TOKENS 的批，优先在标点、其次在词边界'/'处切。"""
    if len(phonemes) <= MAX_BATCH_TOKENS:
        return [phonemes] if phonemes else []
    batches, rest = [], phonemes
    while len(rest) > MAX_BATCH_TOKENS:
        window = rest[:MAX_BATCH_TOKENS]
        cut = max(window.rfind(p) for p in ",.!?:;，。！？：；")
        if cut < MAX_BATCH_TOKENS // 2:
            cut = window.rfind("/")
        if cut < MAX_BATCH_TOKENS // 2:
            cut = MAX_BATCH_TOKENS - 1
        batches.append(rest[: cut + 1])
        rest = rest[cut + 1 :]
    if rest:
        batches.append(rest)
    return [b for b in batches if any(ch in vocab for ch in b)]


class KokoroTts:
    def __init__(self, model_dir: str, voice: str, speed: float):
        import onnxruntime as rt
        from misaki import zh

        self.g2p = zh.ZHG2P(version="1.1")
        with open(os.path.join(model_dir, "tokenizer.json"), encoding="utf-8") as f:
            self.vocab = json.load(f)["model"]["vocab"]
        self.sess = rt.InferenceSession(
            os.path.join(model_dir, "model_quantized.onnx"), providers=["CPUExecutionProvider"]
        )
        voice_path = os.path.join(model_dir, "voices", f"{voice}.bin")
        if not os.path.isfile(voice_path):
            raise FileNotFoundError(f"音色文件不存在: {voice_path}")
        self.voice = np.fromfile(voice_path, dtype=np.float32).reshape(-1, 256)
        self.speed = speed

    def synthesize(self, text: str) -> np.ndarray:
        phonemes, _ = self.g2p(text)
        phonemes = " ".join(phonemes.split())
        parts = []
        for batch in split_batches(phonemes, self.vocab):
            tokens = [self.vocab[ch] for ch in batch if ch in self.vocab]
            if not tokens:
                continue
            style = self.voice[min(len(tokens), len(self.voice)) - 1][None, :]
            audio = self.sess.run(
                ["waveform"],
                {
                    "input_ids": np.array([[0, *tokens, 0]], dtype=np.int64),
                    "style": style.astype(np.float32),
                    "speed": np.array([self.speed], dtype=np.float32),
                },
            )[0].ravel()
            parts.append(trim_silence(audio))
        if not parts:
            return np.zeros(0, dtype=np.float32)
        pause = np.zeros(int(SENTENCE_PAUSE * SAMPLE_RATE), dtype=np.float32)
        return np.concatenate([*parts, pause])


def main() -> int:
    # Windows 下 stdin/stdout 默认跟随控制台代码页（如 GBK），强制 UTF-8
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")
    sys.stdin.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--voice", required=True)
    parser.add_argument("--out-dir", required=True)
    parser.add_argument("--speed", type=float, default=1.0)
    args = parser.parse_args()

    os.makedirs(args.out_dir, exist_ok=True)
    log("加载 TTS 模型…")
    tts = KokoroTts(args.model_dir, args.voice, args.speed)
    log("模型就绪，等待文本…")

    import soundfile as sf

    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            index = int(req["index"])
            text = str(req["text"])
        except (ValueError, KeyError) as e:
            log(f"跳过无法解析的输入行: {e}")
            continue
        try:
            log(f"正在合成第 {index + 1} 句…")
            audio = tts.synthesize(text)
        except Exception as e:  # noqa: BLE001 - 单句失败不拖垮整个朗读任务
            log(f"第 {index} 句合成失败: {e}")
            continue
        wav_path = os.path.abspath(os.path.join(args.out_dir, f"{index:04d}.wav"))
        sf.write(wav_path, audio, SAMPLE_RATE)
        duration = round(len(audio) / SAMPLE_RATE, 2)
        print(json.dumps({"index": index, "wav": wav_path, "duration": duration}, ensure_ascii=False), flush=True)
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:  # noqa: BLE001 - sidecar 需要把任何异常暴露给调用方
        print(f"tts 失败: {e}", file=sys.stderr)
        sys.exit(1)
