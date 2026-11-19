use std::fs;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WinState {

    pub x: i32,

    pub y: i32,

    pub w: f32,

    pub h: f32,

    pub maximized: bool,

    pub dark: bool,

    pub col_size: bool,
    pub col_type: bool,
    pub col_modified: bool,
    pub col_perms: bool,
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
            col_size: true,
            col_type: true,
            col_modified: true,
            col_perms: true,
        }
    }
}

fn file_path() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(|d| d.join("window.ini"))
}

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
            "col_size" => st.col_size = v.trim() == "1",
            "col_type" => st.col_type = v.trim() == "1",
            "col_modified" => st.col_modified = v.trim() == "1",
            "col_perms" => st.col_perms = v.trim() == "1",
            _ => {}
        }
    }

    if st.w >= 300.0 && st.h >= 200.0 {
        Some(st)
    } else {
        None
    }
}

pub fn load_cols() -> Option<[bool; 4]> {
    let txt = fs::read_to_string(file_path()?).ok()?;
    let mut c = [true; 4];
    for line in txt.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "col_size" => c[0] = v.trim() == "1",
            "col_type" => c[1] = v.trim() == "1",
            "col_modified" => c[2] = v.trim() == "1",
            "col_perms" => c[3] = v.trim() == "1",
            _ => {}
        }
    }
    Some(c)
}

pub fn save(st: &WinState) {
    let Some(p) = file_path() else {
        return;
    };
    let body = format!(
        "x={}\ny={}\nw={}\nh={}\nmaximized={}\ndark={}\ncol_size={}\ncol_type={}\ncol_modified={}\ncol_perms={}\n",
        st.x,
        st.y,
        st.w,
        st.h,
        i32::from(st.maximized),
        i32::from(st.dark),
        i32::from(st.col_size),
        i32::from(st.col_type),
        i32::from(st.col_modified),
        i32::from(st.col_perms),
    );
    let _ = fs::write(p, body);
}
