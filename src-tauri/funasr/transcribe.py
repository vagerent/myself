# -*- coding: utf-8 -*-
"""马赛福会议录音转写组件（funasr-onnx sidecar）。

用法：
    transcribe --model-dir <ASR模型目录> --vad-dir <VAD模型目录> --audio <16kHz 单声道 wav>
               [--spk-dir <说话人模型目录>]

流程：Fsmn_vad 按静音点切出语音段 → （可选）CAM++ 逐段提取声纹 embedding 并聚类
出「说话人N」→ 相邻同说话人语音段拼成 ≤30 秒的块 → 逐块送 SenseVoiceSmall 转写。
每步往 stderr 打一行进度：
    MASAIFU_PROGRESS <已处理秒数>/<总秒数>
普通日志往 stderr 打：MASAIFU_LOG <消息>
最终结果在 stdout 输出一行 JSON：
    {"text": "纯文本（说话人/时间已拼好）", "segments": [{"start": 秒, "end": 秒, "speaker": 1, "text": "..."}]}
未启用说话人分离时 segments 的 speaker 为 null。致命错误写 stderr 并以非零码退出。

模型文件由 App 在首次使用时从 ModelScope 下载：
- ASR：iic/SenseVoiceSmall-onnx（model_quant.onnx、config.yaml、am.mvn、tokens.json、
  configuration.json）+ iic/SenseVoiceSmall 的 chn_jpn_yue_eng_ko_spectok.bpe.model
- VAD：damo/speech_fsmn_vad_zh-cn-16k-common-onnx（model_quant.onnx、config.yaml、
  am.mvn、configuration.json）
- 说话人：iic/CosyVoice-300M-SFT 的 campplus.onnx（CAM++ 声纹模型，28MB）
"""

import argparse
import json
import os
import sys

# 单个转写块的最大时长（秒）；SenseVoiceSmall 在 30 秒以内的片段上效果最好
CHUNK_SECS = 30.0
# 短于该时长的语音段不提声纹（embedding 不可靠），归并给相邻说话人
MIN_SPK_SECS = 0.6
# 说话人聚类的余弦相似度阈值：越大区分越狠（同一人可能被拆成多个编号）。
# 实测：同一人的不同段落通常 >0.6，不同人（含变调模拟）通常 <0.45，取中间值
SPK_THRESHOLD = 0.5
# 进度条阶段划分：声纹分析占前 30%，逐块转写占后 70%
SPK_PROGRESS_FRAC = 0.3


def report_progress(done_secs: float, total_secs: float) -> None:
    print(f"MASAIFU_PROGRESS {done_secs:.1f}/{total_secs:.1f}", file=sys.stderr, flush=True)


def log(message: str) -> None:
    print(f"MASAIFU_LOG {message}", file=sys.stderr, flush=True)


class CampPlus:
    """CAM++ 声纹 embedding（onnxruntime 直跑，输入 80 维 kaldi fbank，输出 192 维向量）。"""

    def __init__(self, model_path: str):
        import onnxruntime

        self.sess = onnxruntime.InferenceSession(
            model_path, providers=["CPUExecutionProvider"]
        )
        self.input_name = self.sess.get_inputs()[0].name

    def embedding(self, mono, sr: int):
        """mono: 16kHz float32 波形切片。返回 L2 归一化的 embedding (192,)。"""
        import kaldi_native_fbank as knf
        import numpy as np

        opts = knf.FbankOptions()
        opts.frame_opts.samp_freq = float(sr)
        opts.frame_opts.dither = 0.0
        opts.energy_floor = 1.0
        opts.mel_opts.num_bins = 80
        fbank_fn = knf.OnlineFbank(opts)
        fbank_fn.accept_waveform(sr, mono.tolist())
        frames = fbank_fn.num_frames_ready
        mat = np.empty((frames, 80), dtype=np.float32)
        for i in range(frames):
            mat[i, :] = fbank_fn.get_frame(i)
        # 按时间轴做均值归一化（CAM++ 训练时的 CMVN 方式）
        mat -= mat.mean(axis=0, keepdims=True)
        emb = self.sess.run(None, {self.input_name: mat[None, ...]})[0][0]
        return emb / (np.linalg.norm(emb) + 1e-8)


def cluster_speakers(embs, threshold: float):
    """embs: [N, 192] 已归一化。在线顺序聚类（按质心余弦相似度），返回每段的簇号。

    不做层次聚类：会议场景说话人按时间顺序交替出现，顺序聚类 + 质心均值
    已够用，且纯 numpy 实现、无 scipy 依赖。簇号按首次出现顺序从 0 编号。
    """
    import numpy as np

    labels = []
    centroids = []  # 每个簇的归一化质心
    counts = []
    for emb in embs:
        best, best_sim = -1, threshold
        for i, c in enumerate(centroids):
            sim = float(np.dot(emb, c))
            if sim > best_sim:
                best, best_sim = i, sim
        if best < 0:
            centroids.append(emb.copy())
            counts.append(1)
            labels.append(len(centroids) - 1)
        else:
            counts[best] += 1
            c = centroids[best] * (counts[best] - 1) + emb
            centroids[best] = c / (np.linalg.norm(c) + 1e-8)
            labels.append(best)
    return labels


def fmt_time(secs: float) -> str:
    secs = int(secs)
    h, rem = divmod(secs, 3600)
    m, s = divmod(rem, 60)
    return f"{h}:{m:02d}:{s:02d}" if h else f"{m:02d}:{s:02d}"


def main() -> int:
    # Windows 下 stdout/stderr 默认跟随控制台代码页（如 GBK），强制 UTF-8 供 Rust 侧解析
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")

    parser = argparse.ArgumentParser()
    parser.add_argument("--model-dir", required=True)
    parser.add_argument("--vad-dir", required=True)
    parser.add_argument("--spk-dir", default=None)
    parser.add_argument("--audio", required=True)
    args = parser.parse_args()

    import soundfile as sf

    # 延迟导入，让 --help 等场景不必加载 onnxruntime
    from funasr_onnx import Fsmn_vad, SenseVoiceSmall
    from funasr_onnx.utils.postprocess_utils import rich_transcription_postprocess

    wav, sr = sf.read(args.audio, dtype="float32", always_2d=True)
    mono = wav.mean(axis=1)
    total_secs = len(mono) / sr
    if total_secs <= 0:
        print(json.dumps({"text": "", "segments": []}, ensure_ascii=False))
        return 0

    log("加载语音识别模型…")
    asr = SenseVoiceSmall(args.model_dir, batch_size=1, quantize=True)
    log("加载端点检测模型…")
    vad = Fsmn_vad(args.vad_dir, quantize=True)

    spk = None
    if args.spk_dir:
        spk_path = os.path.join(args.spk_dir, "campplus.onnx")
        if os.path.isfile(spk_path):
            log("加载说话人分离模型…")
            spk = CampPlus(spk_path)
        else:
            log(f"未找到说话人模型 {spk_path}，跳过说话人分离")

    log("语音端点检测…")
    segments = vad(mono)
    # VAD 输出按波形嵌套一层（[[[开始毫秒, 结束毫秒], ...]]），先拍平
    flat = []
    for item in segments:
        if not item:
            continue
        if isinstance(item[0], (list, tuple)):
            flat.extend(item)
        else:
            flat.append(item)
    # 统一成 [(开始秒, 结束秒)]
    segs = [
        (float(s[0]) / 1000.0, float(s[1]) / 1000.0)
        for s in flat
        if len(s) >= 2 and s[1] > s[0]
    ]
    if not segs:
        print(json.dumps({"text": "", "segments": []}, ensure_ascii=False))
        return 0

    # ---- 说话人分离：逐段提 embedding → 聚类 → 过短段归并给前一个说话人 ----
    speakers = None  # 每个语音段的说话人编号（从 1 开始）；None 表示未启用
    if spk is not None:
        log("正在分析说话人…")
        spk_idx = [i for i, (b, e) in enumerate(segs) if e - b >= MIN_SPK_SECS]
        embs = []
        for i in spk_idx:
            b, e = segs[i]
            embs.append(spk.embedding(mono[int(b * sr): int(e * sr)], sr))
            report_progress(e * SPK_PROGRESS_FRAC, total_secs)
        if embs:
            import numpy as np

            sub_labels = cluster_speakers(np.stack(embs), SPK_THRESHOLD)
            speakers = [None] * len(segs)
            for i, lab in zip(spk_idx, sub_labels):
                speakers[i] = lab + 1
            # 过短段（无 embedding）归并给前一个说话人；开头没有前驱就给后一个
            for i in range(len(speakers)):
                if speakers[i] is None and i > 0:
                    speakers[i] = speakers[i - 1]
            for i in range(len(speakers) - 1, -1, -1):
                if speakers[i] is None and i + 1 < len(speakers):
                    speakers[i] = speakers[i + 1]
            n_spk = len(set(s for s in speakers if s))
            log(f"识别到 {n_spk} 位说话人，开始逐块转写…")

    # ---- 切块：相邻且同说话人的语音段拼成 ≤30 秒的块（切点都在静音/换人处） ----
    chunks = []  # [(开始秒, 结束秒, 说话人编号|None)]
    cur_beg, cur_end = segs[0]
    cur_spk = speakers[0] if speakers else None
    for idx, (beg, end) in enumerate(segs[1:], start=1):
        spk_id = speakers[idx] if speakers else None
        if end - cur_beg > CHUNK_SECS or spk_id != cur_spk:
            chunks.append((cur_beg, cur_end, cur_spk))
            cur_beg, cur_end, cur_spk = beg, end, spk_id
        else:
            cur_end = end
    chunks.append((cur_beg, cur_end, cur_spk))

    if spk is None:
        log(f"共 {len(chunks)} 个语音块，开始逐块转写…")
    base = SPK_PROGRESS_FRAC if spk is not None else 0.0
    span = 1.0 - base
    out_segments = []
    report_progress(total_secs * base, total_secs)
    for beg, end, spk_id in chunks:
        piece = mono[int(beg * sr): int(end * sr)]
        # 跳过过短的碎块（<0.1 秒）
        if len(piece) >= int(sr * 0.1):
            res = asr(piece, language="zh", use_itn=True)
            if res:
                text = rich_transcription_postprocess(res[0])
                if text:
                    out_segments.append(
                        {"start": round(beg, 2), "end": round(end, 2),
                         "speaker": spk_id, "text": text}
                    )
        report_progress(total_secs * (base + span * end / total_secs), total_secs)

    lines = []
    for seg in out_segments:
        prefix = f"[{fmt_time(seg['start'])}]"
        if seg["speaker"]:
            prefix = f"[说话人{seg['speaker']} {fmt_time(seg['start'])}]"
        lines.append(f"{prefix} {seg['text']}")
    result = {"text": "\n".join(lines), "segments": out_segments}
    print(json.dumps(result, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as e:  # noqa: BLE001 - sidecar 需要把任何异常暴露给调用方
        print(f"transcribe 失败: {e}", file=sys.stderr)
        sys.exit(1)
