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

// ── CR-1：本地异步 I/O ─────────────────────────────────────────
// UI 帧内只登记请求 + 置 loading；逐项 metadata 枚举与 Shell 删除一律在
// `run_local` 所在的工作线程执行，回执由 `ui::poll_local_jobs` 帧尾入账。
// 帧内唯一保留的一次文件系统调用是 `navigate` 的 `read_dir` 句柄探测
// （判目录可进入，供同步浏览的成败分支使用），它不迭代、不取 metadata。

/// 本地窗格待办请求，暂挂 `FilePane.pending_local`，帧尾由 `poll_local_jobs` 取走。
#[derive(Clone, Debug)]
pub enum LocalPaneReq {
    /// 导航：线程枚举 path，回执后更新 path / 清空选中；失败按 old_path 回滚。
    Navigate {
        path: PathBuf,
        old_path: Option<PathBuf>,
    },
    /// 刷新：回执按 keep（发起时的选中行名）保留选中；keep 为空即清空。
    Refresh { path: PathBuf, keep: Vec<String> },
    /// 删除：线程内逐项进回收站，随后同线程枚举 dir 作删后重列（不再回到帧内）。
    Delete {
        dir: PathBuf,
        targets: Vec<String>,
        keep: Vec<String>,
    },
}

/// 工作线程回执。`entries = None` = 枚举失败（保持原列表，不静默清空）。
pub struct LocalPaneReply {
    pub req: LocalPaneReq,
    pub entries: Option<Vec<Entry>>,
    /// Delete 的逐项结果（(展示路径, 成败)）；其余请求为空。
    pub deleted: Vec<(String, Result<(), String>)>,
}

impl LocalPaneReply {
    /// 线程异常退出时的失败回执：仅用于清 loading / 回滚，不产生假成功汇总。
    pub fn worker_gone(req: LocalPaneReq) -> Self {
        Self {
            req,
            entries: None,
            deleted: Vec::new(),
        }
    }
}

/// 线程体：本地文件系统 I/O 的唯一入口（`list_dir` / `delete_to_bin` 纯函数不动）。
pub fn run_local(req: LocalPaneReq, show_hidden: bool) -> LocalPaneReply {
    let mut deleted = Vec::new();
    let path = match &req {
        LocalPaneReq::Navigate { path, .. } | LocalPaneReq::Refresh { path, .. } => path.clone(),
        LocalPaneReq::Delete { dir, targets, .. } => {
            for name in targets {
                let p = dir.join(name);
                let shown = p.to_string_lossy().into_owned();
                deleted.push((shown, delete_to_bin(&p)));
            }
            dir.clone()
        }
    };
    let entries = list_dir(&path, show_hidden);
    LocalPaneReply {
        req,
        entries,
        deleted,
    }
}

/// 删除日志（纯函数）：逐项成功留痕、失败可定位；**部分失败绝不写「已删除 N 项」**。
pub fn delete_log_lines(results: &[(String, Result<(), String>)]) -> Vec<String> {
    let mut out = Vec::new();
    let mut fails: Vec<(&String, &String)> = Vec::new();
    for (shown, r) in results {
        match r {
            Ok(()) => out.push(tfmt("[本地] 已删除 {0}", &[shown])),
            Err(e) => fails.push((shown, e)),
        }
    }
    if fails.len() > 1 {
        let detail = fails
            .iter()
            .map(|(p, e)| format!("{p}（{e}）"))
            .collect::<Vec<_>>()
            .join("、");
        out.push(tfmt(
            "[本地] 批量删除失败 {0} 项：{1}",
            &[&fails.len(), &detail],
        ));
    } else if let Some((shown, e)) = fails.first() {
        out.push(tfmt("[本地] 删除失败 {0}：{1}", &[shown, e]));
    }
    out
}

/// 帧内回执入账：纯内存，不碰文件系统。返回需写入终端的删除日志行。
pub fn apply_local(pane: &mut FilePane, reply: LocalPaneReply) -> Vec<String> {
    let is_delete = matches!(reply.req, LocalPaneReq::Delete { .. });
    let logs = if is_delete {
        delete_log_lines(&reply.deleted)
    } else {
        Vec::new()
    };
    // CR-1 竞态：请求发出后用户已导航到别处（或回「此电脑」根），旧回执不得
    // 用旧目录内容覆盖当前窗格（ADR-031「骗」级）。远程侧由 tag + tab.wait
    // 处理，本地侧按请求目录与当前 path 比对；删除日志仍照常入账。
    let req_path = match &reply.req {
        LocalPaneReq::Navigate { path, .. } | LocalPaneReq::Refresh { path, .. } => path,
        LocalPaneReq::Delete { dir, .. } => dir,
    };
    if pane.path.as_ref() != Some(req_path) {
        return logs;
    }
    pane.loading = false;
    pane.loading_label = None;
    let Some(entries) = reply.entries else {
        // 枚举失败：保持原列表（同旧 navigate / refresh 语义），但显式提示不静默。
        if let LocalPaneReq::Navigate { old_path, .. } = &reply.req {
            pane.path = old_path.clone();
        }
        pane.msg = Some(t("目录读取失败").to_owned());
        return logs;
    };
    pane.msg = None;
    match &reply.req {
        LocalPaneReq::Navigate { path, .. } => {
            pane.path = Some(path.clone());
            pane.entries = entries;
            pane.selected.clear();
            pane.sel_anchor = None;
        }
        LocalPaneReq::Refresh { keep, .. } | LocalPaneReq::Delete { keep, .. } => {
            pane.entries = entries;
            pane.selected = pane
                .entries
                .iter()
                .enumerate()
                .filter(|(_, e)| keep.contains(&e.name))
                .map(|(i, _)| i)
                .collect();
        }
    }
    if is_delete {
        // 删除影响树中目录名 → 重建路径树（旧 apply_delete 同款）。
        crate::ui::panes::tree_mark_dirty(pane);
    }
    apply_sort(pane);
    logs
}

/// 发起请求时快照选中行名（回执后按名回映射，被删 / 改名的行自然落选）。
pub fn selected_names(pane: &FilePane) -> Vec<String> {
    pane.selected
        .iter()
        .filter_map(|&i| pane.entries.get(i).map(|e| e.name.clone()))
        .collect()
}

// ── 导航 ──────────────────────────────────────────────────────

/// 进入「此电脑」根（驱动器列表；C# NavigateLocalRoot）。
pub fn navigate_root(pane: &mut FilePane) {
    pane.path = None;
    pane.entries = list_drives();
    pane.selected.clear();
    // CR-1：回根是同步的（GetLogicalDrives 位掩码，不逐项枚举）。清掉在途标记，
    // 否则旧目录请求被丢弃后其过期回执无人清 loading，窗格会永久卡「加载中…」。
    pane.pending_local = None;
    pane.loading = false;
    pane.loading_label = None;
    pane.msg = None;
    apply_sort(pane);
}

/// 导航到目录：成功才更新 path/entries；失败保持原状（面包屑输错回车不出错页）。
/// CR-1：帧内只探测一次目录句柄，逐项枚举交工作线程；path 乐观更新、回执失败回滚。
/// 返回值语义不变（供同步浏览判成败），因此 10 处调用点与 `sync.rs` 无需改动。
pub fn navigate(pane: &mut FilePane, path: &Path) -> bool {
    if std::fs::read_dir(path).is_err() {
        return false;
    }
    let old_path = pane.path.clone();
    pane.path = Some(path.to_path_buf());
    pane.pending_local = Some(LocalPaneReq::Navigate {
        path: path.to_path_buf(),
        old_path,
    });
    pane.loading = true;
    pane.loading_label = None;
    true
}

/// 刷新：目录态重读当前目录，根态重列驱动器。无数据的窗格为空操作。
/// 比 C# 更好：重列后按名称保留原选中（C# EnterLocalPath 重列一律清空）——
/// 刷新语义下用户选中的文件不因重列丢失；被删/改名的行自然落选。
pub fn refresh(pane: &mut FilePane) {
    let Some(path) = pane.path.clone() else {
        navigate_root(pane);
        return;
    };
    // CR-1：原「读失败保持原状」由回执里的 entries=None 分支承接；帧内不枚举。
    let keep = selected_names(pane);
    pane.pending_local = Some(LocalPaneReq::Refresh { path, keep });
    pane.loading = true;
    pane.loading_label = None;
}

// ── 排序 ──────────────────────────────────────────────────────

/// 表头点击后按当前键重排（C# ApplySort）。
pub fn apply_sort(pane: &mut FilePane) {
    sort_entries(&mut pane.entries, pane.sort_col, pane.sort_asc);
}

/// 大小写不敏感比较（C# StringComparer.OrdinalIgnoreCase 类语义，T19-4 对齐
/// FileView ApplySort :1422-1433）：逐字符 Unicode 小写折叠后按码位比较。
/// ASCII 与常见中西文下与 OrdinalIgnoreCase 排序结果一致；完全等价的大小写
/// 变体（"Aa"/"aa"）判等后交由稳定排序保持原序（C# LINQ OrderBy 亦稳定）。
fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

/// 目录恒在前（升降序同），其余按键；同键回退按名称。
/// 名称/类型键及全部名称回退为大小写不敏感（T19-4）：本地与远程列表共用本
/// 函数（表头点击 panes.rs / 列表到达 ui::mod Ev::Listing / 同步浏览 sync.rs
/// 均走 apply_sort），C# FileView 双侧 ApplySort 同为 OrdinalIgnoreCase。
pub fn sort_entries(entries: &mut [Entry], key: SortKey, asc: bool) {
    entries.sort_by(|a, b| {
        let ord = match key {
            SortKey::Name => cmp_ignore_case(&a.name, &b.name),
            SortKey::Size => a
                .size_bytes
                .cmp(&b.size_bytes)
                .then(cmp_ignore_case(&a.name, &b.name)),
            SortKey::Type => {
                cmp_ignore_case(&a.kind, &b.kind).then(cmp_ignore_case(&a.name, &b.name))
            }
            SortKey::Modified => a
                .modified
                .cmp(&b.modified)
                .then(cmp_ignore_case(&a.name, &b.name)),
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
    // CR-1：重列交工作线程（keep 为空 = 清空选中，同旧 navigate 语义）。
    pane.selected.clear();
    refresh(pane);
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

/// 当前本地时间戳 yyyyMMdd-HHmmss（损坏存档备份文件名用；C# `DateTime.Now:yyyyMMdd-HHmmss` 同式）。
pub(crate) fn now_stamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + tz_offset_secs();
    let (y, mo, d) = civil_from_days(secs.div_euclid(86_400));
    let tod = secs.rem_euclid(86_400);
    format!(
        "{y:04}{mo:02}{d:02}-{:02}{:02}{:02}",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

// ── 系统打开（C# OpenEntry 文件分支：默认关联程序）─────────────

/// 用系统默认关联程序打开本地文件（C# OpenEntry：文件夹进入，文件交给系统）。
/// Windows = ShellExecuteW "open"；Linux = xdg-open / macOS = open。
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
        // 单参数直传，不经 shell 二次解析（无注入面；registration/guard.rs 同款）。
        // spawn 失败（极简桌面无 xdg-open）静默，与 Windows ShellExecuteW 失败
        // 无错误弹窗同口径。
        let cmd = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(cmd).arg(path).spawn();
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
/// （唯一调用在 Windows GetLocalTime 时区偏移探测内。）
#[cfg(windows)]
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

#[cfg(test)]
mod sort_tests {
    use super::{cmp_ignore_case, sort_entries};
    use crate::ui::panes::{Entry, SortKey};

    fn e(name: &str, kind: &str) -> Entry {
        Entry {
            name: name.to_owned(),
            is_dir: false,
            size: String::new(),
            kind: kind.to_owned(),
            modified: String::new(),
            perms: String::new(),
            size_bytes: 0,
            mtime_secs: 0,
        }
    }

    /// T19-4：名称排序大小写不敏感（C# FileView ApplySort :1422-1433
    /// StringComparer.OrdinalIgnoreCase）。混合序列断言：字节序会把 "Aa"/"Bb"
    /// （0x41/0x42）排在 "a c"（0x61）前；不敏感语义下 "a c" < "Aa" == "aa" <
    /// "Bb"（空格 0x20 < 'a' 0x61），判等项由稳定排序保持原序。
    #[test]
    fn name_sort_case_insensitive() {
        let mut es = vec![e("Bb", ""), e("a c", ""), e("Aa", ""), e("aa", "")];
        sort_entries(&mut es, SortKey::Name, true);
        let got: Vec<&str> = es.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(got, ["a c", "Aa", "aa", "Bb"]);
    }

    /// 类型键同理：kind 比较不敏感，组内回退名称亦不敏感。
    #[test]
    fn type_sort_case_insensitive() {
        let mut es = vec![e("b", "TXT"), e("A", "txt"), e("C", "zip"), e("D", "")];
        sort_entries(&mut es, SortKey::Type, true);
        let got: Vec<&str> = es.iter().map(|x| x.name.as_str()).collect();
        // kind 不敏感归组："TXT"=="txt"（组内按名 A < b）、""（空）<"txt"<"zip"。
        assert_eq!(got, ["D", "A", "b", "C"]);
    }

    /// 降序：仅键倒置，目录恒在前（C# OrderByDescending(IsDirectory) 同）。
    #[test]
    fn desc_keeps_dirs_first() {
        let mut es = vec![e("b", ""), e("d", "")];
        es[0].is_dir = true;
        sort_entries(&mut es, SortKey::Name, false);
        assert_eq!(es[0].name, "b");
        assert_eq!(es[1].name, "d");
    }

    /// cmp_ignore_case 基础语义：等价大小写变体判等；空格按码位参与比较。
    #[test]
    fn cmp_ignore_case_basic() {
        assert_eq!(cmp_ignore_case("Aa", "aa"), std::cmp::Ordering::Equal);
        assert_eq!(cmp_ignore_case("a c", "Aa"), std::cmp::Ordering::Less);
        assert_eq!(cmp_ignore_case("Bb", "a c"), std::cmp::Ordering::Greater);
    }
}

#[cfg(test)]
mod cr1_io_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// CR-10 教训：Windows 上 cargo test 默认并行，固定临时文件名会互抢
    /// （os error 5 / error 32）。pid + 进程内序号保证独占。
    fn fixture_dir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ifreesftp-cr1-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

    /// 钉子①：帧内只登记请求，不做逐项枚举；列表由工作线程回执填入。
    #[test]
    fn navigate_registers_request_without_frame_enumeration() {
        let dir = fixture_dir();
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::write(dir.join("b.txt"), b"b").unwrap();
        let mut pane = FilePane::default();
        assert!(navigate(&mut pane, &dir), "目录可进入");
        assert!(pane.pending_local.is_some(), "帧内只登记请求");
        assert!(pane.loading, "在途须可见");
        assert!(pane.entries.is_empty(), "帧内不得做逐项枚举");

        let req = pane.pending_local.take().unwrap();
        let reply = run_local(req, pane.show_hidden);
        assert_eq!(reply.entries.as_ref().map(|v| v.len()), Some(2));
        let logs = apply_local(&mut pane, reply);
        assert!(logs.is_empty(), "非删除请求不产生删除日志");
        assert_eq!(pane.entries.len(), 2, "回执才填列表");
        assert!(!pane.loading, "回执清 loading");
        assert_eq!(pane.path, Some(dir.clone()));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 钉子②：枚举失败回滚路径、显式报错，绝不静默清空 / 假装成功。
    #[test]
    fn failed_read_restores_path_and_shows_message() {
        let dir = fixture_dir();
        let old = dir.join("old");
        let target = dir.join("gone");
        let mut pane = FilePane::default();
        // 真实 navigate 会乐观把 path 置为目标；夹具须还原该状态，
        // 否则回执按「已导航到别处」的过期回执丢弃（CR-1 竞态守卫）。
        pane.path = Some(target.clone());
        pane.loading = true;
        let req = LocalPaneReq::Navigate {
            path: target,
            old_path: Some(old.clone()),
        };
        let reply = LocalPaneReply {
            req,
            entries: None,
            deleted: Vec::new(),
        };
        assert!(apply_local(&mut pane, reply).is_empty());
        assert_eq!(pane.path, Some(old), "失败回滚旧路径");
        assert!(!pane.loading, "失败也须清 loading");
        assert_eq!(pane.msg.as_deref(), Some(t("目录读取失败")));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 钉子③：部分失败逐项可定位，绝不写「已删除 N 项」的假成功汇总。
    #[test]
    fn delete_log_lines_never_fakes_success_on_partial_failure() {
        let results = vec![
            ("C:/a".to_owned(), Ok(())),
            ("C:/b".to_owned(), Err("拒绝访问".to_owned())),
        ];
        let lines = delete_log_lines(&results);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("C:/a"), "成功项留痕");
        assert!(
            lines[1].contains("C:/b") && lines[1].contains("拒绝访问"),
            "失败项路径与原因可定位"
        );
        assert!(
            !lines.iter().any(|l| l.contains("已删除 1 项")),
            "部分失败不得写成功汇总"
        );
        let many = vec![
            ("C:/x".to_owned(), Err("e1".to_owned())),
            ("C:/y".to_owned(), Err("e2".to_owned())),
        ];
        let lines = delete_log_lines(&many);
        assert_eq!(lines.len(), 1, "批量失败合并一条摘要");
        assert!(
            lines[0].contains("批量删除失败 2 项")
                && lines[0].contains("C:/x")
                && lines[0].contains("C:/y")
        );
    }

    /// 钉子④：旧目录的过期回执不得覆盖已导航到的新目录（ADR-031「骗」级）。
    #[test]
    fn stale_local_reply_does_not_overwrite_current_pane() {
        let dir = fixture_dir();
        let old = dir.join("old");
        let new = dir.join("new");
        let mut pane = FilePane::default();
        pane.path = Some(new.clone());
        pane.loading = true;
        let stale = vec![crate::ui::panes::Entry::placeholder()];
        let reply = LocalPaneReply {
            req: LocalPaneReq::Refresh {
                path: old,
                keep: Vec::new(),
            },
            entries: Some(stale),
            deleted: Vec::new(),
        };
        assert!(apply_local(&mut pane, reply).is_empty());
        assert_eq!(pane.path, Some(new), "path 不被旧回执改写");
        assert!(pane.entries.is_empty(), "列表不被旧回执覆盖");
        // 回根是同步路径，须自行清在途标记，避免过期回执无人清 loading。
        navigate_root(&mut pane);
        assert!(!pane.loading && pane.pending_local.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
