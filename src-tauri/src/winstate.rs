use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize};

// 窗口状态：记住全量窗口的大小与位置（物理像素）
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
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

// 启动时恢复上次窗口状态；窗口默认 hidden，无闪烁
pub fn apply_on_startup(app: &AppHandle) {
    let Some(w) = app.get_webview_window("main") else {
        return;
    };
    let s = load(app);
    if let (Some(width), Some(height)) = (s.full_width, s.full_height) {
        let _ = w.set_size(PhysicalSize::new(width.max(720), height.max(500)));
    }
    if let (Some(x), Some(y)) = (s.x, s.y) {
        if position_visible(app, x, y) {
            let _ = w.set_position(PhysicalPosition::new(x, y));
        } else {
            let _ = w.center();
        }
    }
}

// 位置落在任一显示器范围内才算有效
// （窗口最小化时 Windows 会记录 (-32000, -32000) 之类的屏幕外占位坐标，直接恢复会「看不到窗口」）
fn position_visible(app: &AppHandle, x: i32, y: i32) -> bool {
    app.available_monitors()
        .map(|ms| {
            ms.iter().any(|m| {
                let p = m.position();
                let sz = m.size();
                x >= p.x - 8 && y >= p.y - 8 && x < p.x + sz.width as i32 && y < p.y + sz.height as i32
            })
        })
        .unwrap_or(false)
}
