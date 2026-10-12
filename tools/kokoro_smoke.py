# -*- coding: utf-8 -*-
"""Kokoro v1.1-zh ONNX + misaki[zh] 冒烟测试。

用法（仓库根目录）：
    .venv-tts/Scripts/python.exe tools/kokoro_smoke.py

验证点：多音字、数字/日期、中英混排、长句；每个音色各合成一句。
产物：models/kokoro/smoke_out/*.wav，终端打印音素、耗时与 RTF。
"""

import json
import os
import sys
import time

sys.stdout.reconfigure(encoding="utf-8", errors="replace")

import numpy as np
import onnxruntime as rt
import soundfile as sf

MODEL_DIR = os.path.join(os.path.dirname(__file__), "..", "models", "kokoro")
SAMPLE_RATE = 24000
MAX_TOKENS = 510

TEST_CASES = [
    ("多音字", "银行行长来到长江边上，重要的事要重复一遍。"),
    ("数字日期", "二零二六年八月二十七日下午三点半，十二个人参会，业绩增长百分之三十五点六，营收一千二百万元。"),
    ("中英混排", "下周我们要把 API 和 SDK 接入 CI 流程，OKR 也要同步更新。"),
    ("长句", "刚才讨论了三个议题，第一是新版本的发布节奏，第二是语音转写的准确率优化，第三是下季度的预算安排，大家还有什么补充吗？"),
]
VOICES = ["zf_001", "zf_002", "zf_003", "zf_004", "zm_009", "zm_010", "zm_011", "zm_012"]


def load_vocab(path):
    d = json.load(open(path, encoding="utf-8"))
    return d["model"]["vocab"]


def tokenize(phonemes: str, vocab: dict) -> list[int]:
    return [vocab[ch] for ch in phonemes if ch in vocab]


def main():
    from misaki import zh

    # v1.1-zh 模型必须配 version='1.1' 的注音符号 frontend（输出注音+声调数字）
    g2p = zh.ZHG2P(version="1.1")
    vocab = load_vocab(os.path.join(MODEL_DIR, "tokenizer.json"))
    sess = rt.InferenceSession(
        os.path.join(MODEL_DIR, "model_quantized.onnx"), providers=["CPUExecutionProvider"]
    )
    out_dir = os.path.join(MODEL_DIR, "smoke_out")
    os.makedirs(out_dir, exist_ok=True)

    def synth(text: str, voice: np.ndarray, speed: float = 1.0):
        phonemes, _ = g2p(text)
        phonemes = " ".join(phonemes.split())
        tokens = tokenize(phonemes, vocab)
        dropped = sorted(set(phonemes) - set(vocab) - {" "})
        if len(tokens) > MAX_TOKENS:
            print(f"  !! tokens {len(tokens)} 超过 {MAX_TOKENS}，需要切分")
            tokens = tokens[:MAX_TOKENS]
        style = voice[min(len(tokens), len(voice)) - 1][None, :]
        t0 = time.time()
        audio = sess.run(
            ["waveform"],
            {
                "input_ids": np.array([[0, *tokens, 0]], dtype=np.int64),
                "style": style.astype(np.float32),
                "speed": np.array([speed], dtype=np.float32),
            },
        )[0].ravel()
        cost = time.time() - t0
        return phonemes, dropped, audio, cost

    # 预热一次，后续计时才是真实推理速度
    warm = np.fromfile(os.path.join(MODEL_DIR, "voices", "zf_001.bin"), dtype=np.float32).reshape(-1, 256)
    synth("预热", warm)

    for name, text in TEST_CASES:
        voice = np.fromfile(os.path.join(MODEL_DIR, "voices", "zf_001.bin"), dtype=np.float32).reshape(-1, 256)
        phonemes, dropped, audio, cost = synth(text, voice)
        dur = len(audio) / SAMPLE_RATE
        path = os.path.join(out_dir, f"case_{name}.wav")
        sf.write(path, audio, SAMPLE_RATE)
        print(f"[{name}] {dur:.1f}s 音频 / 合成 {cost:.2f}s (RTF {cost/dur:.2f}) -> {path}")
        print(f"  音素: {phonemes[:120]}{'...' if len(phonemes) > 120 else ''}")
        if dropped:
            print(f"  !! 词表外被丢弃的字符: {dropped}")

    # 8 个音色各来一句
    sample = "大家好，我是你的会议纪要助手。"
    for v in VOICES:
        voice = np.fromfile(os.path.join(MODEL_DIR, "voices", f"{v}.bin"), dtype=np.float32).reshape(-1, 256)
        _, _, audio, _ = synth(sample, voice)
        sf.write(os.path.join(out_dir, f"voice_{v}.wav"), audio, SAMPLE_RATE)
        print(f"[音色 {v}] ok")


if __name__ == "__main__":
    main()
