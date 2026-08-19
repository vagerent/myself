# funasr-sidecar 占位目录

真正的转写组件（transcribe.exe 及 _internal/）由 `tools/build_sidecar.md` 中的
PyInstaller 步骤生成后放入本目录。此占位文件仅用于让 tauri.conf.json 的
bundle.resources glob 在 sidecar 尚未构建时也能匹配，请勿删除。
