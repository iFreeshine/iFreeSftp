//! 本地文件系统枚举（T3 真实数据接入）：std::fs 零依赖。
//!
//! 对照 C# FileView 本地模式：
//! - 根视图 = 「此电脑」驱动器列表（NavigateLocalRoot / DriveInfo.GetDrives，
//!   条目 IsDirectory、无修改时间、类型「文件夹」）；
//! - 目录项 = 名称 / 大小（FileEntry.FormatSize "1.2 MB"）类型 / 修改时间
//!   （yyyy-MM-dd HH:mm 本地时区）；
//! - 隐藏项 = FILE_ATTRIBUTE_HIDDEN (0x2)，show_hidden 开关后重载；
//! - 类型名走内置扩展名类别表（对齐 C# 非 Windows 分支的类别表 + zh-CN 文案；
//!   C# 在 Windows 走 SHGetFileInfo/注册表壳层链路，零依赖范围外，用扩展名
//!   表覆盖绝大多数并补 exe/msi 等 Windows 常见类型）。

use std::path::{Path, PathBuf};

use crate::i18n::{t, tfmt};
use crate::ui::panes::{Entry, FilePane, SortKey};

// ── 导航 ──────────────────────────────────────────────────────

/// 进入「此电脑」根（驱动器列表；C# NavigateLocalRoot）。
pub fn navigate_root(pane: &mut FilePane) {
    pane.path = None;
    pane.entries = list_drives();
    pane.selected.clear();
    apply_sort(pane);
}

/// 导航到目录：成功才更新 path/entries；失败保持原状（面包屑输错回车不出错页）。
pub fn navigate(pane: &mut FilePane, path: &Path) -> bool {
    let Some(entries) = list_dir(path, pane.show_hidden) else {
        return false;
    };
    pane.path = Some(path.to_path_buf());
    pane.entries = entries;
    pane.selected.clear();
    apply_sort(pane);
    true
}

/// 刷新：目录态重读当前目录，根态重列驱动器。无数据的窗格为空操作。
/// 比 C# 更好：重列后按名称保留原选中（C# EnterLocalPath 重列一律清空）——
/// 刷新语义下用户选中的文件不因重列丢失；被删/改名的行自然落选。
pub fn refresh(pane: &mut FilePane) {
    let Some(p) = pane.path.clone() else {
        navigate_root(pane);
        return;
    };
    // 读失败保持原状（同 navigate 语义：面包屑/列表不出错页）。
    let Some(entries) = list_dir(&p, pane.show_hidden) else {
        return;
    };
    // 旧选中行名集合 → 新列表按名回映射（重列同一目录）。
    let names: std::collections::HashSet<String> = pane
        .selected
        .iter()
        .filter_map(|&i| pane.entries.get(i).map(|e| e.name.clone()))
        .collect();
    pane.entries = entries;
    pane.selected = pane
        .entries
        .iter()
        .enumerate()
        .filter(|(_, e)| names.contains(&e.name))
        .map(|(i, _)| i)
        .collect();
    apply_sort(pane);
}

// ── 排序 ──────────────────────────────────────────────────────

/// 表头点击后按当前键重排（C# ApplySort）。
pub fn apply_sort(pane: &mut FilePane) {
    sort_entries(&mut pane.entries, pane.sort_col, pane.sort_asc);
}

/// 目录恒在前（升降序同），其余按键；同键回退按名称。
pub fn sort_entries(entries: &mut [Entry], key: SortKey, asc: bool) {
    entries.sort_by(|a, b| {
        let ord = match key {
            SortKey::Name => a.name.cmp(&b.name),
            SortKey::Size => a.size_bytes.cmp(&b.size_bytes).then(a.name.cmp(&b.name)),
            SortKey::Type => a.kind.cmp(&b.kind).then(a.name.cmp(&b.name)),
            SortKey::Modified => a.modified.cmp(&b.modified).then(a.name.cmp(&b.name)),
        };
        b.is_dir
            .cmp(&a.is_dir)
            .then(if asc { ord } else { ord.reverse() })
    });
}

// ── 新建 ──────────────────────────────────────────────────────

/// 新建文件夹：重名序号规避（dedupe，序号插扩展名前）；创建后仅重列当前
/// 目录，不选中（C# CreateNewEntryAsync :2017-2051 仅 RefreshListKeepingPath）。
pub fn create_dir(pane: &mut FilePane, base: &str) {
    create_entry(pane, base, true);
}

/// 新建空文件（语义同 create_dir）。
pub fn create_file(pane: &mut FilePane, base: &str) {
    create_entry(pane, base, false);
}

fn create_entry(pane: &mut FilePane, base: &str, is_dir: bool) {
    // 根视图 / 远程演示无本地路径：C# 根视图禁用新建；远程 mkdir 随 SSH 接入。
    let Some(root) = pane.path.clone() else {
        return;
    };
    let name = dedupe(&root, base);
    let target = root.join(&name);
    // 创建失败静默：重列后条目不出现（C# 异常处理后同样仅刷新）。
    if is_dir {
        drop(std::fs::create_dir(&target));
    } else {
        drop(std::fs::File::create(&target));
    }
    // C# :2045 创建后仅 RefreshListKeepingPath（不选中、无内联重命名）；
    // 首行 RefreshTreeIfVisible → 重建路径树（保留展开）。
    navigate(pane, &root);
    crate::ui::panes::tree_mark_dirty(pane);
}

/// 重名规避（C# :2031-2038 序号插扩展名前：GetFileNameWithoutExtension +
/// " (i)" + GetExtension）→ "新建文本文档 (2).txt"；无扩展名或 "." 起的隐藏
/// 文件（C# GetExtension 视为无扩展名）退化为全名后追加。
fn dedupe(root: &Path, base: &str) -> String {
    if !root.join(base).exists() {
        return base.to_owned();
    }
    let (stem, ext) = match base.rfind('.') {
        Some(i) if i > 0 => (&base[..i], &base[i..]),
        _ => (base, ""),
    };
    let mut n = 2;
    loop {
        let cand = format!("{stem} ({n}){ext}");
        if !root.join(&cand).exists() {
            return cand;
        }
        n += 1;
    }
}

/// 本地删除：Shell API 进回收站可还原（C# FileView.xaml.cs:4578 明确注释
/// 「用 Shell 而非 File.Delete」；std::fs::remove_* 是永久删除，不对齐）。
/// SHFileOperationW FO_DELETE + FOF_ALLOWUNDO，目录整树递归；仅 Windows 实现，
/// 其他平台走 trash crate（Linux FreeDesktop Trash 规范 / macOS NSWorkspace，
/// 同为回收站可还原语义）。
/// 应用层已有 Delete 确认弹窗 → FOF_NOCONFIRMATION 免二次确认；FOF_NOERRORUI
/// 让失败经返回码走终端日志而非系统弹窗；FOF_SILENT 免进度 UI。
/// `path` 必须为绝对路径（相对路径 FOF_ALLOWUNDO 不生效，调用方 dir.join 已保证）。
pub fn delete_to_bin(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[repr(C)]
        #[allow(non_snake_case)] // WinAPI 原名
        struct SHFILEOPSTRUCTW {
            hwnd: isize,
            wFunc: u32,
            pFrom: *const u16,
            pTo: *const u16,
            fFlags: u16,
            fAnyOperationsAborted: i32,
            hNameMappings: isize,
            lpszProgressTitle: *const u16,
        }
        const FO_DELETE: u32 = 3;
        const FOF_SILENT: u16 = 0x0004;
        const FOF_NOCONFIRMATION: u16 = 0x0010;
        const FOF_ALLOWUNDO: u16 = 0x0040;
        const FOF_NOERRORUI: u16 = 0x0400;
        unsafe extern "system" {
            fn SHFileOperationW(lpFileOp: *mut SHFILEOPSTRUCTW) -> i32;
        }
        // pFrom 约定：路径 + 双 NUL 结尾（多路径以单 NUL 分隔，此处单项）。
        let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
        from.push(0);
        from.push(0);
        let mut op = SHFILEOPSTRUCTW {
            hwnd: 0,
            wFunc: FO_DELETE,
            pFrom: from.as_ptr(),
            pTo: std::ptr::null(),
            fFlags: FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI,
            fAnyOperationsAborted: 0,
            hNameMappings: 0,
            lpszProgressTitle: std::ptr::null(),
        };
        // SAFETY: op/from 均为本函数栈变量，调用期间合法；pTo/lpszProgressTitle
        // 为空指针符合 FO_DELETE 单向删除的约定。
        let code = unsafe { SHFileOperationW(&mut op) };
        if code == 0 && op.fAnyOperationsAborted == 0 {
            Ok(())
        } else if op.fAnyOperationsAborted != 0 {
            Err("操作被中止".into())
        } else {
            Err(format!("SHFileOperation 错误码 {code}"))
        }
    }
    #[cfg(not(windows))]
    {
        // trash crate：FreeDesktop（Linux）/ NSWorkspace（macOS）回收站语义，
        // 目录整树递归；与 Windows SHFileOperationW 语义一致（可还原）。
        trash::delete(path).map_err(|e| e.to_string())
    }
}

// ── 枚举 ──────────────────────────────────────────────────────

/// 枚举目录（按开关跳过隐藏项）；读失败返回 None（导航保持原状）。
pub fn list_dir(path: &Path, show_hidden: bool) -> Option<Vec<Entry>> {
    let rd = std::fs::read_dir(path).ok()?;
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = e.metadata() else {
            continue;
        };
        if !show_hidden && is_hidden(&name, &meta) {
            continue;
        }
        let is_dir = meta.is_dir();
        let size_bytes = if is_dir { 0 } else { meta.len() };
        let kind = if is_dir {
            t("文件夹").to_owned()
        } else {
            kind_of(&name)
        };
        let size = if is_dir {
            String::new()
        } else {
            format_size(size_bytes)
        };
        let msecs = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let modified = fmt_mtime_unix(msecs);
        out.push(Entry {
            name,
            is_dir,
            size,
            kind,
            modified,
            // Windows 本地无 rwx 语义；权限列默认隐藏（C# 同）。
            perms: String::new(),
            size_bytes,
            mtime_secs: msecs,
        });
    }
    Some(out)
}

/// 「此电脑」驱动器列表（GetLogicalDrives 位掩码；对齐 C# DriveInfo.Name "C:\"）。
#[cfg(windows)]
pub(crate) fn list_drives() -> Vec<Entry> {
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }
    // SAFETY: GetLogicalDrives 无参数无副作用。
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| Entry {
            name: format!("{}:\\", (b'A' + i as u8) as char),
            is_dir: true,
            size: String::new(),
            kind: t("文件夹").to_owned(),
            // C# LastModified = MinValue → 显示空。
            modified: String::new(),
            perms: String::new(),
            size_bytes: 0,
            mtime_secs: 0,
        })
        .collect()
}

/// 非 Unix/Windows：无驱动器概念，返回空列表（UI「此电脑」节点为空）。
#[cfg(not(windows))]
pub(crate) fn list_drives() -> Vec<Entry> {
    Vec::new()
}

/// 隐藏项：Windows FILE_ATTRIBUTE_HIDDEN (0x2)；其他平台按点前缀。
pub(crate) fn is_hidden(name: &str, meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let _ = name;
        meta.file_attributes() & 0x2 != 0
    }
    #[cfg(not(windows))]
    {
        let _ = meta;
        name.starts_with('.')
    }
}

// ── 展示格式（对齐 C# FileEntry.FormatSize / DisplayModified）──

/// 字节 → "1.2 MB"：B/KB/MB/GB/TB，0.## 格式（最多两位小数去尾零）。
pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    let s = format!("{size:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    format!("{s} {}", UNITS[unit])
}

/// Unix 秒 → "yyyy-MM-dd HH:mm"（本地时区；远程 mtime 同款展示）。
pub(crate) fn fmt_mtime_unix(secs: i64) -> String {
    let secs = secs + tz_offset_secs();
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let tod = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02} {:02}:{:02}",
        tod / 3600,
        (tod % 3600) / 60
    )
}

/// 当前本地时间 HH:mm:ss（队列时间列 / 终端日志）。
pub(crate) fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + tz_offset_secs();
    let tod = secs.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", tod / 3600, (tod % 3600) / 60, tod % 60)
}

// ── 系统打开（C# OpenEntry 文件分支：默认关联程序）─────────────

/// 用系统默认关联程序打开本地文件（C# OpenEntry：文件夹进入，文件交给系统；
/// Windows = ShellExecuteW "open"）。仅 Windows 实现；其他平台空操作。
pub fn open_with_default(path: &Path) {
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn ShellExecuteW(
                hwnd: isize,
                operation: *const u16,
                file: *const u16,
                parameters: *const u16,
                directory: *const u16,
                show: i32,
            ) -> isize;
        }
        let to_wide =
            |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let file = to_wide(&path.to_string_lossy());
        let open = to_wide("open");
        unsafe {
            // SW_SHOWNORMAL = 1（C# Process.Start 默认窗口态）。
            ShellExecuteW(
                0,
                open.as_ptr(),
                file.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                1,
            );
        }
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

// ── 文件夹选择对话框（FV-01/B2；C# PickLocalFolder FileView.xaml.cs:850-884）──

/// 在途文件夹选择（后台 STA 线程持有系统对话框）：UI 帧轮询 `rx` 消费回执，
/// `Ok(Some)` = 选中路径，`Ok(None)` / 通道断开 = 取消或失败（清槽复原按钮）。
pub struct PickJob {
    /// 发起侧（0 左 / 1 右）：结果回填对应本地窗格。
    pub side: usize,
    pub rx: std::sync::mpsc::Receiver<Option<PathBuf>>,
}

/// 发起文件夹选择：专用 STA 线程跑 IFileDialog，结果经通道回传。
///
/// 根因注记：IFileDialog::Show 自带模态消息泵，绝不能在 egui 主线程（winit
/// 事件循环线程）调用——泵重入 WndProc → eframe run_ui_and_paint 无重入守卫
/// → wgpu surface 重入 acquire → panic（「打开目录」按钮崩溃根因）。C# 版
/// 同步弹无碍（WPF 对话框自管消息循环），这里改异步 + 回执。
///
/// `owner` = 主窗 HWND（UI 线程点击瞬间经 owner_window 取好传入）：对话框
/// 模态归属主窗，弹出期间主窗被禁用——观感与 C# 模态一致。
pub fn spawn_pick_folder(start: Option<PathBuf>, owner: isize, side: usize) -> PickJob {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("folder-picker".into())
        .spawn(move || {
            let dir = pick_folder_on_thread(start.as_deref(), owner);
            let _ = tx.send(dir);
        })
        .expect("spawn folder-picker thread");
    PickJob { side, rx }
}

/// 本线程活动窗口句柄（= 主窗）：作文件夹选择对话框 owner。
/// GetActiveWindow 按线程生效——后台选择线程取不到主窗，故必须在 UI 线程
/// 点击瞬间取好随请求传入。
#[cfg(windows)]
pub fn owner_window() -> isize {
    unsafe extern "system" {
        fn GetActiveWindow() -> isize;
    }
    // SAFETY: 无参数无副作用。
    unsafe { GetActiveWindow() }
}

#[cfg(not(windows))]
pub fn owner_window() -> isize {
    0
}

/// 选择线程入口：COM 按线程初始化，本线程自行 CoInitializeEx(STA)，跑完配对
/// CoUninitialize（主线程的初始化不跨线程共享）。
#[cfg(windows)]
fn pick_folder_on_thread(start: Option<&Path>, owner: isize) -> Option<PathBuf> {
    unsafe extern "system" {
        fn CoInitializeEx(pvreserved: *mut std::ffi::c_void, dwcoinit: u32) -> i32;
        fn CoUninitialize();
    }
    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    // SAFETY: 保留参数 null。S_OK / S_FALSE 均为成功（>= 0）。
    let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    let r = pick_folder_impl(start, owner);
    if hr >= 0 {
        // SAFETY: 与上方成功的初始化配对。
        unsafe { CoUninitialize() };
    }
    r
}

#[cfg(not(windows))]
fn pick_folder_on_thread(_start: Option<&Path>, _owner: isize) -> Option<PathBuf> {
    None
}

/// 系统文件夹选择对话框（仅选择线程调用）：IFileOpenDialog + FOS_PICKFOLDERS |
/// FOS_FORCEFILESYSTEM，起始目录 = `start`（解析失败用对话框默认位置，C# 同
/// catch 空处理），用户取消 / 任一步 COM 失败返回 None（C# Show().Failed → null
/// 同口径）。`owner` = 主窗 HWND（0 = 无归属）。
///
/// 零依赖手写 FFI（先例本文件 GetLogicalDrives / open_with_default）：COM 调用走
/// 裸 vtable（shobjidl_core.h 方法序），GUID / 常量均为 WinAPI 定值。
#[cfg(windows)]
fn pick_folder_impl(start: Option<&Path>, owner: isize) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;

    #[allow(non_snake_case, clippy::upper_case_acronyms)] // WinAPI 原名
    #[repr(C)]
    struct GUID {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }
    // CLSID_FileOpenDialog = {DC1C5A9C-E88A-4DDE-A5A1-60F82A20AEF7}
    const CLSID_FILE_OPEN_DIALOG: GUID = GUID {
        data1: 0xDC1C_5A9C,
        data2: 0xE88A,
        data3: 0x4DDE,
        data4: [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
    };
    // IID_IFileOpenDialog = {D57C7288-D4AD-4768-BE02-9D969532D960}
    const IID_I_FILE_OPEN_DIALOG: GUID = GUID {
        data1: 0xD57C_7288,
        data2: 0xD4AD,
        data3: 0x4768,
        data4: [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
    };
    // IID_IShellItem = {43826D1E-E718-42EE-BC55-A1E261C37BFE}
    const IID_I_SHELL_ITEM: GUID = GUID {
        data1: 0x4382_6D1E,
        data2: 0xE718,
        data3: 0x42EE,
        data4: [0xBC, 0x55, 0xA1, 0xE2, 0x61, 0xC3, 0x7B, 0xFE],
    };

    unsafe extern "system" {
        fn CoCreateInstance(
            rclsid: *const GUID,
            punkouter: *mut std::ffi::c_void,
            dwclscontext: u32,
            riid: *const GUID,
            ppv: *mut *mut std::ffi::c_void,
        ) -> i32;
        fn CoTaskMemFree(pv: *mut std::ffi::c_void);
        fn SHCreateItemFromParsingName(
            pszpath: *const u16,
            pbc: *mut std::ffi::c_void,
            riid: *const GUID,
            ppv: *mut *mut std::ffi::c_void,
        ) -> i32;
    }

    /// IFileDialog vtable（idx 0-2 = IUnknown，3 = IModalWindow::Show，4 起按
    /// shobjidl_core.h / windows crate 生成序；只标调用约定用到的方法，其余
    /// 占位不调用）。注意槽序必须逐字对齐官方接口：曾漏掉 16 = GetFileName
    /// 致 get_result 错位到 19（实为 SetFileNameLabel）——Show 返回后对话框
    /// 已销毁，再写入即 0xc0000005（「打开目录」选定后崩溃根因）。
    #[repr(C)]
    struct FileDialogVtbl {
        query_interface: *mut std::ffi::c_void, // 0
        add_ref: *mut std::ffi::c_void,         // 1
        release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32, // 2
        show: unsafe extern "system" fn(*mut std::ffi::c_void, isize) -> i32, // 3
        set_file_types: *mut std::ffi::c_void,  // 4
        set_file_type_index: *mut std::ffi::c_void, // 5
        get_file_type_index: *mut std::ffi::c_void, // 6
        advise: *mut std::ffi::c_void,          // 7
        unadvise: *mut std::ffi::c_void,        // 8
        set_options: unsafe extern "system" fn(*mut std::ffi::c_void, u32) -> i32, // 9
        get_options: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> i32, // 10
        set_default_folder: *mut std::ffi::c_void, // 11
        set_folder: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32, // 12
        get_folder: *mut std::ffi::c_void,            // 13
        get_current_selection: *mut std::ffi::c_void, // 14
        set_file_name: *mut std::ffi::c_void,         // 15
        get_file_name: *mut std::ffi::c_void,         // 16
        set_title: *mut std::ffi::c_void,             // 17
        set_ok_button_label: *mut std::ffi::c_void,   // 18
        set_file_name_label: *mut std::ffi::c_void,   // 19
        get_result:
            unsafe extern "system" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> i32, // 20
        add_place: *mut std::ffi::c_void,             // 21
        set_default_extension: *mut std::ffi::c_void, // 22
        close: *mut std::ffi::c_void,                 // 23
        set_client_guid: *mut std::ffi::c_void,       // 24
        clear_client_data: *mut std::ffi::c_void,     // 25
        set_filter: *mut std::ffi::c_void,            // 26
    }

    /// COM Release：所有接口 vtable 前三槽（IUnknown）一致，Release 固定 idx 2。
    unsafe fn com_release(p: *mut std::ffi::c_void) {
        if p.is_null() {
            return;
        }
        // SAFETY: p 为合法 COM 对象（构造自 CoCreateInstance /
        // SHCreateItemFromParsingName 成功路径）；slot 2 恒为 Release。
        // 槽寻址须先转 *mut *mut c_void 再 .add(N)：std::ffi::c_void 大小为
        // 1 字节，对 *mut c_void 直接 .add(N) 只加 N 字节——曾致 release 读到
        // 槽 2 字节处的垃圾指针即 0xc0000005（「打开目录」选定后崩溃根因之二）。
        let vt = unsafe { *(p as *mut *mut std::ffi::c_void) };
        let release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
            unsafe { std::mem::transmute(*((vt as *mut *mut std::ffi::c_void).add(2))) };
        unsafe { release(p) };
    }

    /// 对话框 COM 对象守卫：任一路径返回都 Release（C# 侧 RCW 自动回收同责）。
    struct Guard(*mut std::ffi::c_void);
    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: 构造时已确认非空合法 COM 指针。
            unsafe { com_release(self.0) }
        }
    }

    let mut dlg: *mut std::ffi::c_void = std::ptr::null_mut();
    // SAFETY: 出参指针合法；CLSCTX_INPROC_SERVER = 0x1。线程未初始化 COM 时
    // 返回失败码 → None（选择线程已在 pick_folder_on_thread 初始化 STA）。
    if unsafe {
        CoCreateInstance(
            &CLSID_FILE_OPEN_DIALOG,
            std::ptr::null_mut(),
            0x1,
            &IID_I_FILE_OPEN_DIALOG,
            &mut dlg,
        )
    } < 0
    {
        return None;
    }
    let _guard = Guard(dlg);
    // SAFETY: dlg 为合法 IFileOpenDialog，取 vtable 指针。
    let vt = unsafe { *(dlg as *mut *mut FileDialogVtbl) };
    let mut opts: u32 = 0;
    // SAFETY: get_options 出参合法；失败则从 0 起步（两个 FOS 位仍独立生效）。
    unsafe { ((*vt).get_options)(dlg, &mut opts) };
    // FOS_PICKFOLDERS = 0x20 / FOS_FORCEFILESYSTEM = 0x40（C# :855-857 同值）。
    if unsafe { ((*vt).set_options)(dlg, opts | 0x20 | 0x40) } < 0 {
        return None;
    }
    if let Some(dir) = start {
        let wide: Vec<u16> = dir
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut item: *mut std::ffi::c_void = std::ptr::null_mut();
        // SAFETY: wide 为 NUL 结尾 UTF-16；出参合法。失败 = 起始目录解析失败，
        // 用对话框默认位置（C# :867-869 catch 空处理同语义）。
        if unsafe {
            SHCreateItemFromParsingName(
                wide.as_ptr(),
                std::ptr::null_mut(),
                &IID_I_SHELL_ITEM,
                &mut item,
            )
        } >= 0
        {
            // SAFETY: item 为合法 IShellItem；SetFolder 失败同上忽略。
            unsafe { ((*vt).set_folder)(dlg, item) };
            unsafe { com_release(item) };
        }
    }
    // owner = 主窗句柄（UI 线程点击瞬间经 owner_window 取好传入；C# :872-873
    // 用主窗句柄同语义）。取消（ERROR_CANCELLED）与失败同型 None
    //（C# Show().Failed → null）。
    if unsafe { ((*vt).show)(dlg, owner) } < 0 {
        return None;
    }
    let mut res: *mut std::ffi::c_void = std::ptr::null_mut();
    if unsafe { ((*vt).get_result)(dlg, &mut res) } < 0 {
        return None;
    }
    // IShellItem::GetDisplayName vtable idx 5，SIGDN_FILESYSPATH = 0x80058000
    // （C# :877 同值）。
    // SAFETY: res 为合法 IShellItem，slot 5 恒为 GetDisplayName。
    let svt = unsafe { *(res as *mut *mut std::ffi::c_void) };
    // 槽寻址同 com_release：先转 *mut *mut c_void 再 .add(5)（+40 字节），
    // 对 *mut c_void 直接 .add(5) 只加 5 字节会读到垃圾指针（本轮崩溃点）。
    let get_display_name: unsafe extern "system" fn(
        *mut std::ffi::c_void,
        u32,
        *mut *mut u16,
    ) -> i32 = unsafe { std::mem::transmute(*((svt as *mut *mut std::ffi::c_void).add(5))) };
    let mut pw: *mut u16 = std::ptr::null_mut();
    let path = if unsafe { get_display_name(res, 0x8005_8000, &mut pw) } >= 0 && !pw.is_null() {
        // SAFETY: pw 为 CoTaskMemAlloc 分配的 NUL 结尾串，读长后 CoTaskMemFree 归还。
        let mut len = 0usize;
        while unsafe { *pw.add(len) } != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(pw, len) });
        unsafe { CoTaskMemFree(pw as _) };
        Some(PathBuf::from(s))
    } else {
        None
    };
    unsafe { com_release(res) };
    path
}

/// 本地时区偏移秒（每次现算，DST 自动跟随）：
/// GetLocalTime 墙钟换算 epoch 与 UTC 时刻差值（分钟级精度足够 HH:mm 显示）。
#[cfg(windows)]
fn tz_offset_secs() -> i64 {
    #[repr(C)]
    #[allow(non_snake_case, clippy::upper_case_acronyms)] // WinAPI 原名
    struct SYSTEMTIME {
        wYear: u16,
        wMonth: u16,
        wDayOfWeek: u16,
        wDay: u16,
        wHour: u16,
        wMinute: u16,
        wSecond: u16,
        wMilliseconds: u16,
    }
    unsafe extern "system" {
        fn GetLocalTime(lpSystemTime: *mut SYSTEMTIME);
    }
    let mut t = SYSTEMTIME {
        wYear: 0,
        wMonth: 0,
        wDayOfWeek: 0,
        wDay: 0,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    // SAFETY: 缓冲区为本函数私有栈变量，GetLocalTime 只写入 sizeof(SYSTEMTIME)。
    unsafe { GetLocalTime(&mut t) };
    let local = days_from_civil(i64::from(t.wYear), u32::from(t.wMonth), u32::from(t.wDay))
        * 86_400
        + i64::from(t.wHour) * 3600
        + i64::from(t.wMinute) * 60
        + i64::from(t.wSecond);
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut off = local - utc;
    if off < -43_200 {
        // 两次取样跨日（≤2s 时钟偏差）的负向回卷。
        off += 86_400;
    }
    off
}

#[cfg(not(windows))]
fn tz_offset_secs() -> i64 {
    0
}

// ── 日期换算（Howard Hinnant 算法，零依赖）──────────────────

/// 天数（1970-01-01 = 0）→ (年, 月, 日)。
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// (年, 月, 日) → 天数（1970-01-01 = 0）。
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400) as u64;
    let mp = (if m > 2 { m - 3 } else { m + 9 }) as u64;
    let doy = (153 * mp + 2) / 5 + u64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

// ── 类型名（对齐 C# GetTypeName 非 Windows 分支类别表）────────

/// 文件名 → 类型文本。特殊名 → 扩展名类别 → "文件"/"{EXT} 文件" 兜底。
pub(crate) fn kind_of(name: &str) -> String {
    // C# _unixNameTypeMap：无扩展名约定文件名。
    match name.to_ascii_lowercase().as_str() {
        "readme" | "license" | "copying" | "changelog" | "notice" | "install" | "todo"
        | "version" | "authors" | "news" => return t("文本文档").to_owned(),
        "makefile" | "dockerfile" => return t("脚本文件").to_owned(),
        _ => {}
    }
    let ext = extension(name);
    let cat = match ext.as_str() {
        "txt" | "md" | "log" | "ini" | "cfg" | "conf" | "rst" => t("文本文档"),
        "pdf" => t("PDF 文件"),
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff"
        | "heic" => t("图片文件"),
        "mp4" | "mkv" | "avi" | "mov" | "webm" | "wmv" | "flv" | "m4v" => t("视频文件"),
        "mp3" | "wav" | "flac" | "ogg" | "aac" | "m4a" | "opus" => t("音频文件"),
        "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "tgz" => t("压缩文件"),
        "sh" | "bash" | "zsh" | "fish" | "py" | "pl" | "rb" | "ps1" => t("脚本文件"),
        "c" | "cpp" | "h" | "hpp" | "cs" | "java" | "go" | "rs" | "swift" | "kt" | "ts" | "js"
        | "html" | "css" | "json" | "xml" | "sql" | "lua" | "php" => t("源代码文件"),
        "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "odt" | "ods" | "odp" | "rtf" => {
            t("文档文件")
        }
        "so" | "dylib" | "a" | "dll" | "lib" => t("共享库"),
        // C# ExecutableTypeName；Windows 壳层下 exe/msi 亦归此类。
        "exe" | "msi" | "bat" | "cmd" | "com" => t("应用程序"),
        _ => "",
    };
    if !cat.is_empty() {
        return cat.to_owned();
    }
    if ext.is_empty() {
        t("文件").to_owned()
    } else {
        tfmt("{0} 文件", &[&ext.to_ascii_uppercase()])
    }
}

/// C# Path.GetExtension 语义：点开头文件名（".gitignore"）无扩展名。
fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((head, ext)) if !head.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

// ============================================================================
// 字节格式化测试（第三批）—— 翻译自 C# FormatSizeTests.FormatSize（7 用例）
// 追加到 src/local.rs 末尾。
// 注：C# FileEntry.DisplaySize 两个用例（目录显示空串）属于 UI 实体属性，
//     Rust 侧目录大小在列表渲染处处理，非纯函数，不在本批。
// ============================================================================

#[cfg(test)]
mod format_tests {
    use super::format_size;

    #[test]
    fn format_size_table() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1), "1 B");
        assert_eq!(format_size(1023), "1023 B"); // 单位边界内保留整数
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1_048_576), "1 MB");
        assert_eq!(format_size(1_610_612_736), "1.5 GB");
    }
}
