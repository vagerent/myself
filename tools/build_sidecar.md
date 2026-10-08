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

## TTS sidecar（朗读功能）

会议条目的「朗读」依赖第二个 sidecar：Kokoro v1.1-zh（ONNX q8 量化）+ misaki[zh] G2P。
构建方式与转写 sidecar 相同，产物放 `src-tauri/resources/tts-sidecar/`（已 gitignore）。
模型文件不包含在 sidecar 里，由 App 首次使用时从 ModelScope 下载
（`onnx-community/Kokoro-82M-v1.1-zh-ONNX`：fp16 模型 + tokenizer + 8 个音色）。

```bash
# 1. 独立虚拟环境（仓库根目录，已 gitignore）
python -m venv .venv-tts

# 2. 安装依赖
.venv-tts/Scripts/python.exe -m pip install onnxruntime "misaki[zh]" soundfile numpy pyinstaller

# 3. 冻结为 one-dir 可执行程序
.venv-tts/Scripts/python.exe -m PyInstaller --onedir --name tts --noconfirm --clean \
  --collect-all onnxruntime --collect-all misaki --collect-all jieba \
  --collect-all pypinyin --collect-all pypinyin_dict --collect-all cn2an \
  --collect-all soundfile --collect-all addict --collect-all regex \
  --distpath src-tauri/resources \
  src-tauri/tts/tts.py

# 4. 并入 tts-sidecar 目录
rm -rf src-tauri/resources/tts-sidecar/_internal src-tauri/resources/tts-sidecar/tts.exe
mkdir -p src-tauri/resources/tts-sidecar
mv src-tauri/resources/tts/tts.exe src-tauri/resources/tts/_internal src-tauri/resources/tts-sidecar/
rmdir src-tauri/resources/tts
```

产物约 216MB。验证（模型需先按 `src-tauri/src/tts.rs` 的清单下载到 `models/kokoro/`）：

```bash
echo '{"index": 0, "text": "大家好，我是你的会议纪要助手。"}' | \
  src-tauri/resources/tts-sidecar/tts.exe \
    --model-dir ./models/kokoro --voice zf_001 --out-dir /tmp/tts_test
# 期望：stderr 输出 MASAIFU_LOG，stdout 输出 {"index":0,"wav":"...","duration":...}
```

## 备注（TTS）

- G2P 必须用 `misaki.zh.ZHG2P(version='1.1')`（注音符号+声调数字），老版 IPA 输出
  与 v1.1-zh 模型的 172 词表不匹配；英文段没有 espeak 兜底会被丢弃。
- 模型必须选 q8（model_quantized.onnx，约 127MB，CPU 实测 RTF 约 0.8）：
  fp16 版本在 CPUExecutionProvider 上推理输出全 NaN（写出的 wav 是直流+削波，
  听感为爆音后无声）；fp16 看似快 3 倍正是因为大量运算塌缩成 NaN。
- sidecar 从 stdin 逐行读 `{"index", "text"}` JSON，每句合成完往 stdout 写一行结果，
  stdin 关闭后退出；Rust 侧负责分句、缓存（tts-cache）与取消。

## 构建录音卡 sidecar

「来源 → 录音卡」的 BLE 常驻同步依赖第三个 sidecar：bleak 3.0.2（Windows 蓝牙后端）。
源码在 `src-tauri/pnote/`（pnote_sidecar.py 为入口），冻结方式与上面两个 sidecar 相同，
产物放 `src-tauri/resources/pnote-sidecar/`（已 gitignore）。

```bash
# 1. 独立虚拟环境（仓库根目录，已 gitignore）
python -m venv .venv-pnote

# 2. 安装依赖
.venv-pnote/Scripts/python.exe -m pip install bleak==3.0.2 pyinstaller

# 3. 冻结为 one-dir 可执行程序
#    注意 winrt 坑：bleak 的 Windows 后端依赖 winrt 包（GATT 封装），PyInstaller
#    默认分析不到其中的延迟 import，冻结时必须显式收集，否则运行时才报
#    ModuleNotFoundError: winrt.windows.foundation（或 winsdk 同名模块）。
#    优先 --collect-all winrt；若报「找不到模块 winrt」改用
#    --hidden-import winrt.windows.foundation；都不行就先不加 winrt 参数冻结，
#    以冒烟结果为准（ bleak 3.0.2 在 PyInstaller 下两种都够用）。
.venv-pnote/Scripts/python.exe -m PyInstaller --onedir --name pnote --noconfirm --clean \
  --collect-all bleak --collect-all winrt \
  --distpath src-tauri/resources \
  src-tauri/pnote/pnote_sidecar.py

# 4. 并入 pnote-sidecar 目录
mkdir -p src-tauri/resources/pnote-sidecar
mv src-tauri/resources/pnote/pnote.exe src-tauri/resources/pnote/_internal \
   src-tauri/resources/pnote-sidecar/
rmdir src-tauri/resources/pnote
```

验证（无录音卡是预期情况，能看到 sleeping=true 即可）：

```bash
# 状态查询：期望 stdout 先出 ready 事件，再出
# {"type":"resp","id":1,"ok":true,...,"connected":false}，stdin EOF 后退出码 0
src-tauri/resources/pnote-sidecar/pnote.exe --out-dir /tmp/pnote_out <<< '{"id":1,"cmd":"status"}'

# 启动守护循环：保持 stdin 打开约 35 秒，期望 30 秒内收到
# {"type":"event","event":"state","data":{...,"sleeping":true}}（找不到设备不 crash）
{ echo '{"id":1,"cmd":"start"}'; sleep 35; } | \
  src-tauri/resources/pnote-sidecar/pnote.exe --out-dir /tmp/pnote_out
```

## 备注（pnote）

- 控制协议为 stdin/stdout JSON-lines：请求 `{"id", "cmd"}`（start/stop/status/
  file_list/sync_now/gc_device/shutdown），响应 `{"type":"resp",...}`，事件
  `{"type":"event","event":"ready|state|progress|file_synced",...}`；日志全走 stderr
  （`MASAIFU_LOG` 前缀 + 诊断行），stdout 只有协议行。
- stdin EOF 或 `{"cmd":"shutdown"}` 即退出（退出码 0），Rust 侧据此管理进程生命周期。
- App 运行时在「安装目录资源目录」和 `src-tauri/resources/`（开发态）查找
  `pnote-sidecar/pnote.exe`。
- 电量只信协议 `0-3` 查询（GATT 0x2A19 读数不可靠，详见实施计划的协议怪癖清单）。
