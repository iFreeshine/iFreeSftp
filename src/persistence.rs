//! 窗口状态持久化：尺寸 / 位置 / 最大化 / 主题（T2 剩余项）。
//!
//! 文件：exe 同目录 `window.ini`（便携口径，与 C# 版同目录配置一致）；
//! `key=value` 行格式，零新依赖（R5：serde 等下载需请示，手写解析足够）。
//! 写入时机：App::ui 每帧变更检测 + 0.3s 节流（拖动/缩放期间不狂写盘）。
//! 最大化时不采纳 inner_rect 的尺寸位置（那是最大化边界），只记标志位。

use std::fs;
use std::path::PathBuf;

/// 一份窗口快照。
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WinState {
    /// 窗口左上角 X（屏幕坐标，points）。
    pub x: i32,
    /// 窗口左上角 Y。
    pub y: i32,
    /// 宽。
    pub w: f32,
    /// 高。
    pub h: f32,
    /// 是否最大化（恢复时优先于 w/h）。
    pub maximized: bool,
    /// 是否深色主题。
    pub dark: bool,
}

impl Default for WinState {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            w: 0.0,
            h: 0.0,
            maximized: false,
            dark: false,
        }
    }
}

/// exe 同目录下的状态文件路径；拿不到 exe 路径则不持久化。
fn file_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|d| d.join("window.ini"))
}

/// 读取上次快照。字段缺失 / 数值非法（尺寸过小）返回 None。
pub fn load() -> Option<WinState> {
    let txt = fs::read_to_string(file_path()?).ok()?;
    let mut st = WinState::default();
    for line in txt.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "x" => st.x = v.trim().parse().unwrap_or(st.x),
            "y" => st.y = v.trim().parse().unwrap_or(st.y),
            "w" => st.w = v.trim().parse().unwrap_or(st.w),
            "h" => st.h = v.trim().parse().unwrap_or(st.h),
            "maximized" => st.maximized = v.trim() == "1",
            "dark" => st.dark = v.trim() == "1",
            _ => {}
        }
    }
    // 完整性：四项几何值齐全且尺寸不小于最小窗口。
    if st.w >= 300.0 && st.h >= 200.0 {
        Some(st)
    } else {
        None
    }
}

/// 写入快照（尽力而为，失败静默——持久化不影响功能）。
pub fn save(st: &WinState) {
    let Some(p) = file_path() else {
        return;
    };
    let body = format!(
        "x={}\ny={}\nw={}\nh={}\nmaximized={}\ndark={}\n",
        st.x,
        st.y,
        st.w,
        st.h,
        i32::from(st.maximized),
        i32::from(st.dark),
    );
    let _ = fs::write(p, body);
}
