use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};

// 窗口状态：精简/全量两种模式各自记住大小，位置共用（物理像素）
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub compact_width: Option<u32>,
    pub compact_height: Option<u32>,
    pub full_width: Option<u32>,
    pub full_height: Option<u32>,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

fn state_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|d| d.join("window-state.json"))
}

pub fn load(app: &AppHandle) -> WindowState {
    state_path(app)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save(app: &AppHandle, state: &WindowState) -> Result<(), String> {
    let path = state_path(app).ok_or("无法获取数据目录")?;
    let json = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

// 启动时恢复上次窗口状态（初始为精简模式）；窗口默认 hidden，无闪烁
pub fn apply_on_startup(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let s = load(app);
    if let (Some(width), Some(height)) = (s.compact_width, s.compact_height) {
        let _ = w.set_size(PhysicalSize::new(width, height));
    }
    if let (Some(x), Some(y)) = (s.x, s.y) {
        let _ = w.set_position(PhysicalPosition::new(x, y));
    }
}
