# 构建 FunASR 转写 sidecar

马赛福的「会议录音」功能依赖一个本地 Python sidecar（funasr-onnx + SenseVoiceSmall）。
sidecar 在**构建安装包前**生成一次，产物放在 `src-tauri/resources/funasr-sidecar/`
（已 gitignore），打包时通过 `tauri.conf.json` 的 `bundle.resources` 带进安装包。
模型文件不包含在 sidecar 里，由 App 首次使用时从 ModelScope 下载。

以下步骤已在 Windows + Python 3.12 上验证通过（funasr-onnx 0.4.2、PyInstaller 6.22）。

## 步骤（Windows / Git Bash）

```bash
# 1. 准备独立虚拟环境（仓库根目录，已 gitignore）
python -m venv .venv-sidecar

# 2. 安装依赖
.venv-sidecar/Scripts/python.exe -m pip install funasr-onnx pyinstaller

# 3. 冻结为 one-dir 可执行程序
.venv-sidecar/Scripts/python.exe -m PyInstaller --onedir --name transcribe --noconfirm --clean \
  --collect-all funasr_onnx --collect-all onnxruntime --collect-all librosa \
  --collect-all soundfile --collect-all kaldi_native_fbank --collect-all sentencepiece \
  --collect-all jieba --collect-all numba \
  --distpath src-tauri/resources \
  src-tauri/funasr/transcribe.py

# 4. 先清掉旧产物，再把新产物并入 funasr-sidecar 目录（保留占位 README.txt）
#    注意顺序：必须先删旧 _internal/，否则 mv 会因目录非空失败（还可能只移走一半文件）
rm -rf src-tauri/resources/funasr-sidecar/_internal src-tauri/resources/funasr-sidecar/transcribe.exe
mv src-tauri/resources/transcribe/transcribe.exe src-tauri/resources/transcribe/_internal \
   src-tauri/resources/funasr-sidecar/
rmdir src-tauri/resources/transcribe
```

完成后目录结构（约 380MB）：

```
src-tauri/resources/funasr-sidecar/
  transcribe.exe        # 入口
  _internal/            # Python 运行时与依赖
  README.txt            # 占位说明（勿删，保证 resources glob 始终有匹配）
```

## 验证

```bash
# 冒烟测试（modelscope 手动下载模型 + 官方示例语音转 16k wav 后）
# 注意：除 iic/SenseVoiceSmall-onnx 的 5 个文件外，还需：
#   1. 从 iic/SenseVoiceSmall 补 sentencepiece 分词器 chn_jpn_yue_eng_ko_spectok.bpe.model
#   2. 从 damo/speech_fsmn_vad_zh-cn-16k-common-onnx 下载 VAD 模型（4 个小文件，
#      放在独立的 fsmn-vad 目录，通过 --vad-dir 传入）
#   3. 从 iic/CosyVoice-300M-SFT 下载 campplus.onnx（CAM++ 声纹模型，放 campplus 目录，
#      通过 --spk-dir 传入；缺省时跳过说话人分离，仅输出时间戳）
src-tauri/resources/funasr-sidecar/transcribe.exe \
  --model-dir ./models/sensevoice-small-onnx \
  --vad-dir ./models/fsmn-vad \
  --spk-dir ./models/campplus \
  --audio ./models/zh_16k.wav
# 期望：stderr 输出 MASAIFU_PROGRESS 进度行，stdout 输出带 [说话人N mm:ss] 前缀的
# {"text": "...", "segments": [...]}
```

## 备注

- App 运行时会依次在「安装目录资源目录」和「src-tauri/resources/」（开发态）查找
  `funasr-sidecar/transcribe.exe`，因此 `cargo tauri dev` 下按上面路径放好即可联调。
- 传给 sidecar 的音频统一由 Rust 侧解码成 16kHz 单声道 wav，sidecar 不需要 ffmpeg。
- 若 funasr-onnx 版本对 SenseVoiceSmall 支持有变化，优先对照 FunASR 官方文档的
  「ONNX 导出」一节调整 `transcribe.py`。
