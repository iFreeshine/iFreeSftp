use std::path::{Path, PathBuf};

use crate::i18n::{t, tfmt};
use crate::ui::panes::{Entry, FilePane, SortKey};

#[derive(Clone, Debug)]
pub enum LocalPaneReq {

    Navigate {
        path: PathBuf,
        old_path: Option<PathBuf>,
    },

    Refresh { path: PathBuf, keep: Vec<String> },

    Delete {
        dir: PathBuf,
        targets: Vec<String>,
        keep: Vec<String>,
    },
}

pub struct LocalPaneReply {
    pub req: LocalPaneReq,
    pub entries: Option<Vec<Entry>>,

    pub deleted: Vec<(String, Result<(), String>)>,
}

impl LocalPaneReply {

    pub fn worker_gone(req: LocalPaneReq) -> Self {
        Self {
            req,
            entries: None,
            deleted: Vec::new(),
        }
    }
}

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

pub fn apply_local(pane: &mut FilePane, reply: LocalPaneReply) -> Vec<String> {
    let is_delete = matches!(reply.req, LocalPaneReq::Delete { .. });
    let logs = if is_delete {
        delete_log_lines(&reply.deleted)
    } else {
        Vec::new()
    };

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

        crate::ui::panes::tree_mark_dirty(pane);
    }
    apply_sort(pane);
    logs
}

pub fn selected_names(pane: &FilePane) -> Vec<String> {
    pane.selected
        .iter()
        .filter_map(|&i| pane.entries.get(i).map(|e| e.name.clone()))
        .collect()
}

pub fn navigate_root(pane: &mut FilePane) {
    pane.path = None;
    pane.entries = list_drives();
    pane.selected.clear();

    pane.pending_local = None;
    pane.loading = false;
    pane.loading_label = None;
    pane.msg = None;
    apply_sort(pane);
}

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

pub fn refresh(pane: &mut FilePane) {
    let Some(path) = pane.path.clone() else {
        navigate_root(pane);
        return;
    };

    let keep = selected_names(pane);
    pane.pending_local = Some(LocalPaneReq::Refresh { path, keep });
    pane.loading = true;
    pane.loading_label = None;
}

pub fn apply_sort(pane: &mut FilePane) {
    sort_entries(&mut pane.entries, pane.sort_col, pane.sort_asc);
}

fn cmp_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
}

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

pub fn create_dir(pane: &mut FilePane, base: &str) {
    create_entry(pane, base, true);
}

pub fn create_file(pane: &mut FilePane, base: &str) {
    create_entry(pane, base, false);
}

fn create_entry(pane: &mut FilePane, base: &str, is_dir: bool) {

    let Some(root) = pane.path.clone() else {
        return;
    };
    let name = dedupe(&root, base);
    let target = root.join(&name);

    if is_dir {
        drop(std::fs::create_dir(&target));
    } else {
        drop(std::fs::File::create(&target));
    }

    pane.selected.clear();
    refresh(pane);
    crate::ui::panes::tree_mark_dirty(pane);
}

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

pub fn delete_to_bin(path: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        #[repr(C)]
        #[allow(non_snake_case)]
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

        trash::delete(path).map_err(|e| e.to_string())
    }
}

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

            perms: String::new(),
            size_bytes,
            mtime_secs: msecs,
        });
    }
    Some(out)
}

#[cfg(windows)]
pub(crate) fn list_drives() -> Vec<Entry> {
    unsafe extern "system" {
        fn GetLogicalDrives() -> u32;
    }

    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| Entry {
            name: format!("{}:\\", (b'A' + i as u8) as char),
            is_dir: true,
            size: String::new(),
            kind: t("文件夹").to_owned(),

            modified: String::new(),
            perms: String::new(),
            size_bytes: 0,
            mtime_secs: 0,
        })
        .collect()
}

#[cfg(not(windows))]
pub(crate) fn list_drives() -> Vec<Entry> {
    Vec::new()
}

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

pub(crate) fn now_hms() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + tz_offset_secs();
    let tod = secs.rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", tod / 3600, (tod % 3600) / 60, tod % 60)
}

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

        let cmd = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        let _ = std::process::Command::new(cmd).arg(path).spawn();
    }
}

pub struct PickJob {

    pub side: usize,
    pub rx: std::sync::mpsc::Receiver<Option<PathBuf>>,
}

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

#[cfg(windows)]
pub fn owner_window() -> isize {
    unsafe extern "system" {
        fn GetActiveWindow() -> isize;
    }

    unsafe { GetActiveWindow() }
}

#[cfg(not(windows))]
pub fn owner_window() -> isize {
    0
}

#[cfg(windows)]
fn pick_folder_on_thread(start: Option<&Path>, owner: isize) -> Option<PathBuf> {
    unsafe extern "system" {
        fn CoInitializeEx(pvreserved: *mut std::ffi::c_void, dwcoinit: u32) -> i32;
        fn CoUninitialize();
    }
    const COINIT_APARTMENTTHREADED: u32 = 0x2;

    let hr = unsafe { CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED) };
    let r = pick_folder_impl(start, owner);
    if hr >= 0 {

        unsafe { CoUninitialize() };
    }
    r
}

#[cfg(not(windows))]
fn pick_folder_on_thread(_start: Option<&Path>, _owner: isize) -> Option<PathBuf> {
    None
}

#[cfg(windows)]
fn pick_folder_impl(start: Option<&Path>, owner: isize) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStrExt;

    #[allow(non_snake_case, clippy::upper_case_acronyms)]
    #[repr(C)]
    struct GUID {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    const CLSID_FILE_OPEN_DIALOG: GUID = GUID {
        data1: 0xDC1C_5A9C,
        data2: 0xE88A,
        data3: 0x4DDE,
        data4: [0xA5, 0xA1, 0x60, 0xF8, 0x2A, 0x20, 0xAE, 0xF7],
    };

    const IID_I_FILE_OPEN_DIALOG: GUID = GUID {
        data1: 0xD57C_7288,
        data2: 0xD4AD,
        data3: 0x4768,
        data4: [0xBE, 0x02, 0x9D, 0x96, 0x95, 0x32, 0xD9, 0x60],
    };

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

    #[repr(C)]
    struct FileDialogVtbl {
        query_interface: *mut std::ffi::c_void,
        add_ref: *mut std::ffi::c_void,
        release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
        show: unsafe extern "system" fn(*mut std::ffi::c_void, isize) -> i32,
        set_file_types: *mut std::ffi::c_void,
        set_file_type_index: *mut std::ffi::c_void,
        get_file_type_index: *mut std::ffi::c_void,
        advise: *mut std::ffi::c_void,
        unadvise: *mut std::ffi::c_void,
        set_options: unsafe extern "system" fn(*mut std::ffi::c_void, u32) -> i32,
        get_options: unsafe extern "system" fn(*mut std::ffi::c_void, *mut u32) -> i32,
        set_default_folder: *mut std::ffi::c_void,
        set_folder: unsafe extern "system" fn(*mut std::ffi::c_void, *mut std::ffi::c_void) -> i32,
        get_folder: *mut std::ffi::c_void,
        get_current_selection: *mut std::ffi::c_void,
        set_file_name: *mut std::ffi::c_void,
        get_file_name: *mut std::ffi::c_void,
        set_title: *mut std::ffi::c_void,
        set_ok_button_label: *mut std::ffi::c_void,
        set_file_name_label: *mut std::ffi::c_void,
        get_result:
            unsafe extern "system" fn(*mut std::ffi::c_void, *mut *mut std::ffi::c_void) -> i32,
        add_place: *mut std::ffi::c_void,
        set_default_extension: *mut std::ffi::c_void,
        close: *mut std::ffi::c_void,
        set_client_guid: *mut std::ffi::c_void,
        clear_client_data: *mut std::ffi::c_void,
        set_filter: *mut std::ffi::c_void,
    }

    unsafe fn com_release(p: *mut std::ffi::c_void) {
        if p.is_null() {
            return;
        }

        let vt = unsafe { *(p as *mut *mut std::ffi::c_void) };
        let release: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
            unsafe { std::mem::transmute(*((vt as *mut *mut std::ffi::c_void).add(2))) };
        unsafe { release(p) };
    }

    struct Guard(*mut std::ffi::c_void);
    impl Drop for Guard {
        fn drop(&mut self) {

            unsafe { com_release(self.0) }
        }
    }

    let mut dlg: *mut std::ffi::c_void = std::ptr::null_mut();

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

    let vt = unsafe { *(dlg as *mut *mut FileDialogVtbl) };
    let mut opts: u32 = 0;

    unsafe { ((*vt).get_options)(dlg, &mut opts) };

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

        if unsafe {
            SHCreateItemFromParsingName(
                wide.as_ptr(),
                std::ptr::null_mut(),
                &IID_I_SHELL_ITEM,
                &mut item,
            )
        } >= 0
        {

            unsafe { ((*vt).set_folder)(dlg, item) };
            unsafe { com_release(item) };
        }
    }

    if unsafe { ((*vt).show)(dlg, owner) } < 0 {
        return None;
    }
    let mut res: *mut std::ffi::c_void = std::ptr::null_mut();
    if unsafe { ((*vt).get_result)(dlg, &mut res) } < 0 {
        return None;
    }

    let svt = unsafe { *(res as *mut *mut std::ffi::c_void) };

    let get_display_name: unsafe extern "system" fn(
        *mut std::ffi::c_void,
        u32,
        *mut *mut u16,
    ) -> i32 = unsafe { std::mem::transmute(*((svt as *mut *mut std::ffi::c_void).add(5))) };
    let mut pw: *mut u16 = std::ptr::null_mut();
    let path = if unsafe { get_display_name(res, 0x8005_8000, &mut pw) } >= 0 && !pw.is_null() {

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

#[cfg(windows)]
fn tz_offset_secs() -> i64 {
    #[repr(C)]
    #[allow(non_snake_case, clippy::upper_case_acronyms)]
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

        off += 86_400;
    }
    off
}

#[cfg(not(windows))]
fn tz_offset_secs() -> i64 {
    0
}

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

pub(crate) fn kind_of(name: &str) -> String {

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

fn extension(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((head, ext)) if !head.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod format_tests {
    use super::format_size;

    #[test]
    fn format_size_table() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1), "1 B");
        assert_eq!(format_size(1023), "1023 B");
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

    #[test]
    fn name_sort_case_insensitive() {
        let mut es = vec![e("Bb", ""), e("a c", ""), e("Aa", ""), e("aa", "")];
        sort_entries(&mut es, SortKey::Name, true);
        let got: Vec<&str> = es.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(got, ["a c", "Aa", "aa", "Bb"]);
    }

    #[test]
    fn type_sort_case_insensitive() {
        let mut es = vec![e("b", "TXT"), e("A", "txt"), e("C", "zip"), e("D", "")];
        sort_entries(&mut es, SortKey::Type, true);
        let got: Vec<&str> = es.iter().map(|x| x.name.as_str()).collect();

        assert_eq!(got, ["D", "A", "b", "C"]);
    }

    #[test]
    fn desc_keeps_dirs_first() {
        let mut es = vec![e("b", ""), e("d", "")];
        es[0].is_dir = true;
        sort_entries(&mut es, SortKey::Name, false);
        assert_eq!(es[0].name, "b");
        assert_eq!(es[1].name, "d");
    }

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

    fn fixture_dir() -> PathBuf {
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("ifreesftp-cr1-{}-{}", std::process::id(), n));
        std::fs::create_dir_all(&dir).expect("create fixture dir");
        dir
    }

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

    #[test]
    fn failed_read_restores_path_and_shows_message() {
        let dir = fixture_dir();
        let old = dir.join("old");
        let target = dir.join("gone");
        let mut pane = FilePane::default();

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

        navigate_root(&mut pane);
        assert!(!pane.loading && pane.pending_local.is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
