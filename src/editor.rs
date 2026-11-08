//! 内置文本编辑器模型层（T7，对标 C# WpfEditorWindow 的数据面）：
//! 10MB 上限、编码探测与写回、文本启发式、远程临时副本、文档中心。
//!
//! C# 语义对齐点：
//! - ReadFile：BOM（UTF-8 / UTF-16 LE/BE）优先；无 BOM 严格 UTF-8 试解码，
//!   失败按系统 ANSI（中文环境 GBK，kernel32 CP_ACP，零新依赖）；Latin-1 兜底；
//! - IsTextData：BOM→文本 / NUL→二进制 / 控制字符（除 \t\n\r\f\v）占比 >5%→二进制；
//! - 远程编辑：临时副本 %TEMP%\iFreeSftp\RemoteEdit\<host>\<sha256(路径)前16>\
//!   <name>，编辑器只认本地路径；保存后由主窗口回传上传（base mtime 冲突检测）；
//! - 保存回传事件经静态队列（编辑器 viewport 线程 → 主窗口 poll）。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// 远程编辑/内置编辑器允许的最大文件大小（C# MaxFileSize 同值）。
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;
/// 文本探测头部字节数（C# ProbeBytes 同值）。
pub const PROBE_BYTES: usize = 8192;

/// 写回编码（C# _writeEncoding 五态；探测结果原样保留，不因保存升级）。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteEnc {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Ansi,
}

impl WriteEnc {
    /// 状态栏用标签（C# 载入提示同口径）。
    pub fn label(self) -> &'static str {
        match self {
            WriteEnc::Utf8 => "UTF-8",
            WriteEnc::Utf8Bom => "UTF-8 BOM",
            WriteEnc::Utf16Le => "UTF-16 LE",
            WriteEnc::Utf16Be => "UTF-16 BE",
            WriteEnc::Ansi => "ANSI",
        }
    }
}

/// 读入文件 → (文本, 写回编码)（C# WpfEditorWindow.ReadFile 移植）。
pub fn decode(bytes: &[u8]) -> (String, WriteEnc) {
    if bytes.len() >= 3 && bytes[..3] == [0xEF, 0xBB, 0xBF] {
        return (
            String::from_utf8_lossy(&bytes[3..]).into_owned(),
            WriteEnc::Utf8Bom,
        );
    }
    if bytes.len() >= 2 && bytes[..2] == [0xFF, 0xFE] {
        // C# Encoding.Unicode.GetString：无效代理对替换 U+FFFD（lossy 等价）。
        let u16s: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_le_bytes(*c))
            .collect();
        return (String::from_utf16_lossy(&u16s), WriteEnc::Utf16Le);
    }
    if bytes.len() >= 2 && bytes[..2] == [0xFE, 0xFF] {
        let u16s: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u16::from_be_bytes(*c))
            .collect();
        return (String::from_utf16_lossy(&u16s), WriteEnc::Utf16Be);
    }
    match std::str::from_utf8(bytes) {
        // C# 严格 UTF-8：DecoderFallbackException 即转 ANSI；from_utf8 错误等价。
        Ok(s) => (s.to_owned(), WriteEnc::Utf8),
        Err(_) => (ansi_decode(bytes), WriteEnc::Ansi),
    }
}

/// 文本 → 字节（含 BOM；ANSI 不可映射字符落 '?' —— C# WriteAllText 同行为）。
pub fn encode(text: &str, enc: WriteEnc) -> Vec<u8> {
    match enc {
        WriteEnc::Utf8 => text.as_bytes().to_vec(),
        WriteEnc::Utf8Bom => {
            let mut v = vec![0xEF, 0xBB, 0xBF];
            v.extend_from_slice(text.as_bytes());
            v
        }
        WriteEnc::Utf16Le => {
            let mut v = vec![0xFF, 0xFE];
            for u in text.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        }
        WriteEnc::Utf16Be => {
            let mut v = vec![0xFE, 0xFF];
            for u in text.encode_utf16() {
                v.extend_from_slice(&u.to_be_bytes());
            }
            v
        }
        WriteEnc::Ansi => ansi_encode(text),
    }
}

// ── kernel32 ANSI（CP_ACP；零新依赖，R5）─────────────────────

#[cfg(windows)]
const CP_ACP: u32 = 0;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn MultiByteToWideChar(
        code_page: u32,
        flags: u32,
        src: *const u8,
        src_len: i32,
        dst: *mut u16,
        dst_len: i32,
    ) -> i32;
    fn WideCharToMultiByte(
        code_page: u32,
        flags: u32,
        src: *const u16,
        src_len: i32,
        dst: *mut u8,
        dst_len: i32,
        default_char: *const u8,
        used_default: *mut i32,
    ) -> i32;
}

/// CP_ACP → UTF-16（失败回退 Latin-1 —— C# SystemAnsiEncoding 兜底同语义）。
#[cfg(windows)]
fn ansi_decode(bytes: &[u8]) -> String {
    let len = bytes.len();
    if len == 0 {
        return String::new();
    }
    // SAFETY：src 指向调用方切片；两段式调用（先测长再填充）是 WinAPI 标准用法。
    unsafe {
        let need = MultiByteToWideChar(
            CP_ACP,
            0,
            bytes.as_ptr(),
            len as i32,
            std::ptr::null_mut(),
            0,
        );
        if need > 0 {
            let mut buf = vec![0u16; need as usize];
            let got = MultiByteToWideChar(
                CP_ACP,
                0,
                bytes.as_ptr(),
                len as i32,
                buf.as_mut_ptr(),
                need,
            );
            if got > 0 {
                buf.truncate(got as usize);
                return String::from_utf16_lossy(&buf);
            }
        }
    }
    bytes.iter().map(|b| *b as char).collect()
}

/// UTF-16 → CP_ACP（默认字符 '?'）。
#[cfg(windows)]
fn ansi_encode(text: &str) -> Vec<u8> {
    let u16s: Vec<u16> = text.encode_utf16().collect();
    if u16s.is_empty() {
        return Vec::new();
    }
    // SAFETY：src 指向局部 Vec；两段式调用（先测长再填充）是 WinAPI 标准用法。
    unsafe {
        let need = WideCharToMultiByte(
            CP_ACP,
            0,
            u16s.as_ptr(),
            u16s.len() as i32,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            std::ptr::null_mut(),
        );
        if need > 0 {
            let mut buf = vec![0u8; need as usize];
            let got = WideCharToMultiByte(
                CP_ACP,
                0,
                u16s.as_ptr(),
                u16s.len() as i32,
                buf.as_mut_ptr(),
                need,
                b"?" as *const u8,
                std::ptr::null_mut(),
            );
            if got > 0 {
                buf.truncate(got as usize);
                return buf;
            }
        }
    }
    text.as_bytes().to_vec()
}

// 非 Windows 无 CP_ACP 概念：ANSI 读写按 Latin-1 直通（与 C# 编码兜底链最后一环、
// Windows 转码失败回退路径同语义；R-01 降级声明，不做 GBK 码表零依赖实现）。
#[cfg(not(windows))]
fn ansi_decode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

#[cfg(not(windows))]
fn ansi_encode(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| if (c as u32) <= 0xFF { c as u8 } else { b'?' })
        .collect()
}

// ── 文本探测（C# IsTextData / IsTextFile 移植）────────────────

/// 启发式文本判定：BOM→文本；NUL→二进制；控制字符（除 \t\n\r\f\v）占比 >5%→二进制。
pub fn is_text_data(data: &[u8]) -> bool {
    if data.len() >= 2 && (data[..2] == [0xFF, 0xFE] || data[..2] == [0xFE, 0xFF]) {
        return true;
    }
    if data.len() >= 3 && data[..3] == [0xEF, 0xBB, 0xBF] {
        return true;
    }
    let mut controls = 0usize;
    for b in data {
        if *b == 0 {
            return false;
        }
        if *b < 0x20 && !matches!(*b, 0x09..=0x0D) {
            controls += 1;
        }
    }
    controls * 100 <= data.len() * 5
}

/// 头部 8KB 探测（本地右键「编辑」启用依据；读失败按非文本）。
pub fn is_text_file(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    if data.is_empty() {
        return true; // 空文件可编辑
    }
    is_text_data(&data[..data.len().min(PROBE_BYTES)])
}

// ── 远程编辑启用（C# RemoteEditMenuEnabled / RemoteBinaryExtensions）──────

/// 远程二进制扩展名黑名单（C# RemoteBinaryExtensions，小写无点）：命中即置灰
/// 「编辑」（与 10MB 上限双保险）；白名单与未知扩展名可点，打开后按内容探测回退。
const REMOTE_BINARY_EXTS: &[&str] = &[
    "exe", "dll", "so", "dylib", "a", "lib", "obj", "o", "bin", "iso", "img", "zip", "rar", "7z",
    "gz", "tgz", "bz2", "xz", "zst", "tar", "jar", "war", "class", "msi", "apk", "jpg", "jpeg",
    "png", "gif", "bmp", "ico", "webp", "tif", "tiff", "svgz", "pdf", "doc", "docx", "xls", "xlsx",
    "ppt", "pptx", "mp3", "wav", "flac", "ogg", "mp4", "avi", "mkv", "mov", "wmv", "webm", "ttf",
    "otf", "woff", "woff2", "eot", "db", "sqlite", "sqlite3", "pdb",
];

/// 文件名是否命中远程二进制扩展名黑名单（无扩展名不算命中 → 可点后探测回退）。
pub fn is_remote_binary_ext(name: &str) -> bool {
    match std::path::Path::new(name).extension() {
        Some(ext) => {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            REMOTE_BINARY_EXTS.contains(&ext.as_str())
        }
        None => false,
    }
}

/// 远程文本扩展名白名单（C# RemoteTextExtensions，小写无点）：命中免内容探测
/// 直接打开；未命中且不在黑名单的走打开后内容探测回退。
const REMOTE_TEXT_EXTS: &[&str] = &[
    "txt",
    "md",
    "log",
    "csv",
    "tsv",
    "ini",
    "cfg",
    "conf",
    "config",
    "json",
    "xml",
    "yml",
    "yaml",
    "toml",
    "properties",
    "sql",
    "html",
    "htm",
    "css",
    "scss",
    "js",
    "mjs",
    "ts",
    "jsx",
    "tsx",
    "vue",
    "py",
    "rb",
    "pl",
    "php",
    "java",
    "kt",
    "swift",
    "c",
    "h",
    "cpp",
    "hpp",
    "cc",
    "cs",
    "go",
    "rs",
    "sh",
    "bash",
    "zsh",
    "bat",
    "cmd",
    "ps1",
    "lua",
    "r",
    "m",
    "dart",
    "gradle",
    "sln",
    "csproj",
    "props",
    "targets",
    "editorconfig",
    "gitignore",
    "gitattributes",
];

/// 文件名是否命中远程文本扩展名白名单（C# 打开前白名单免内容探测）。
pub fn is_remote_text_ext(name: &str) -> bool {
    match std::path::Path::new(name).extension() {
        Some(ext) => {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            REMOTE_TEXT_EXTS.contains(&ext.as_str())
        }
        None => false,
    }
}

// ── 远程临时副本（C# RemoteEditSession TempDirectory 语义）────

/// 副本根：%TEMP%\iFreeSftp\RemoteEdit。
fn remote_edit_root() -> PathBuf {
    let base = std::env::var("TEMP")
        .or_else(|_| std::env::var("TMP"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("iFreeSftp").join("RemoteEdit")
}

/// 主机子目录名（C# SanitizeFileName：Windows 非法字符与控制符替换 '_'）。
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                || (c as u32) < 0x20
            {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// 远程文件临时副本目录：RemoteEdit\<host>\\<sha256(路径)前16>（同一路径复用，
/// 关窗即删目录 —— C# onClosed 清理语义）。
fn remote_tmp_dir(host: &str, remote_path: &str) -> PathBuf {
    use ring::digest::{SHA256, digest};
    let hex = digest(&SHA256, remote_path.as_bytes());
    let tag: String = hex
        .as_ref()
        .iter()
        .take(8)
        .map(|b| format!("{b:02X}"))
        .collect();
    remote_edit_root().join(sanitize(host)).join(tag)
}

/// 写远程副本文件（返回本地路径）；副本保存原始字节（保存回传直接传文件）。
pub fn write_remote_copy(host: &str, remote_path: &str, data: &[u8]) -> Result<PathBuf, String> {
    let name = remote_path.rsplit('/').next().unwrap_or("file");
    let dir = remote_tmp_dir(host, remote_path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(sanitize(name));
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

/// 删除远程副本目录（关窗清理；失败静默 —— %TEMP% 内容无回收价值）。
pub fn remove_remote_copy(dir_host: &str, remote_path: &str) {
    let _ = std::fs::remove_dir_all(remote_tmp_dir(dir_host, remote_path));
}

// ── 文档与中心 ───────────────────────────────────────────────

/// 远程档案引用（保存回传 + base mtime 冲突检测）。
#[derive(Clone)]
pub struct RemoteRef {
    /// 连接主机（临时副本目录定位 + 终端日志前缀）。
    pub host: String,
    pub remote_path: String,
    /// 下载时刻远端 mtime（秒，UTC）；回传前 STAT 比对差 ≥1s 视为已变更。
    pub base_mtime: Option<i64>,
}

/// 查找/替换面板状态（C# AvalonEdit SearchPanel 内存语义，非持久化）。
pub struct SearchUi {
    pub open: bool,
    pub show_replace: bool,
    pub find: String,
    pub replace: String,
    pub case_sensitive: bool,
    /// (起始,结束) char 偏移；随输入/文本重算。
    pub matches: Vec<(usize, usize)>,
    pub current: usize,
    /// 重算签名：(查找词, 大小写, 文本字节数)；不变则跳过 O(n) 扫描。
    pub sig: (String, bool, usize),
}

/// 一篇打开的文档（编辑器 viewport 线程持 Arc<Mutex<_>> 直用）。
pub struct EditorDoc {
    pub id: u64,
    /// 本地路径（远程 = 临时副本）。
    pub path: PathBuf,
    /// 标题（文件名）。
    pub title: String,
    /// Some = 远程档案（保存后自动回传上传）。
    pub remote: Option<RemoteRef>,
    pub text: String,
    pub dirty: bool,
    /// 主题（主窗口每帧同步）。
    pub dark: bool,
    pub enc: WriteEnc,
    pub wrap: bool,
    pub line_numbers: bool,
    pub search: SearchUi,
    /// Some = 打开失败（远程下载报错等）：窗口只显示错误与关闭钮。
    pub load_error: Option<String>,
    /// 上次保存失败信息（状态栏红字，保存成功后清）。
    pub save_err: Option<String>,
    /// 状态栏行列缓存：(光标 char 序, 行 1-based, 列 1-based)。
    pub cursor_cache: (usize, usize, usize),
    /// 当前选区缓存（char 起止；编辑帧写入，「替换当前」比对用 —— 避免无 galley 读 state）。
    pub sel_cache: Option<(usize, usize)>,
    /// 行数提示（打开/保存时重算一次，行号列宽用；避免每帧 O(n) 扫描）。
    pub lines_hint: usize,
    /// 未保存关闭确认层。
    pub close_ask: bool,
    /// 已关（主窗口回收 Arc + 清副本）。
    pub closed: bool,
}

/// 保存回传事件（编辑器线程 push，主窗口 drain 后走上传链）。
pub struct SavedEvent {
    pub doc_id: u64,
    /// 副本路径（上传源文件）。
    pub local_path: PathBuf,
    pub remote_path: String,
}

/// 全部打开的文档 + 事件队列（主窗口 State 持有；doc Arc 交给 viewport 闭包）。
pub struct EditorHub {
    pub docs: Vec<Arc<Mutex<EditorDoc>>>,
    next_id: u64,
}

/// 保存事件静态队列（编辑器线程 → 主窗口；EditorHub 在 State，viewport 闭包够不到）。
static SAVED: Mutex<Vec<SavedEvent>> = Mutex::new(Vec::new());

impl EditorHub {
    pub fn new() -> Self {
        Self {
            docs: Vec::new(),
            next_id: 1,
        }
    }

    /// 本地文件打开（右键「编辑」内置模式；10MB 上限外报 load_error 文档）。
    pub fn open_local(&mut self, path: &Path, dark: bool) {
        let (text, enc, load_error) = match std::fs::read(path) {
            Ok(bytes) if bytes.len() as u64 > MAX_FILE_SIZE => {
                (String::new(), WriteEnc::Utf8, Some(oversize_msg()))
            }
            Ok(bytes) => {
                let (t, e) = decode(&bytes);
                (t, e, None)
            }
            Err(e) => (String::new(), WriteEnc::Utf8, Some(e.to_string())),
        };
        self.push(EditorDoc::new(path, None, text, enc, load_error, dark));
    }

    /// 远程副本打开（下载完成后调用；副本已落盘）。
    pub fn open_remote(
        &mut self,
        host: &str,
        copy: &Path,
        remote_path: String,
        base_mtime: Option<i64>,
        dark: bool,
    ) {
        let (text, enc, load_error) = match std::fs::read(copy) {
            Ok(bytes) => {
                let (t, e) = decode(&bytes);
                (t, e, None)
            }
            Err(e) => (String::new(), WriteEnc::Utf8, Some(e.to_string())),
        };
        self.push(EditorDoc::new(
            copy,
            Some(RemoteRef {
                host: host.to_owned(),
                remote_path,
                base_mtime,
            }),
            text,
            enc,
            load_error,
            dark,
        ));
    }

    fn push(&mut self, mut d: EditorDoc) {
        d.id = self.next_id;
        self.next_id += 1;
        self.docs.push(Arc::new(Mutex::new(d)));
    }

    /// 排队保存回传（编辑器保存成功后调用）。
    pub fn push_saved(ev: SavedEvent) {
        SAVED.lock().unwrap().push(ev);
    }

    /// 主窗口取走待回传事件。
    pub fn drain_saved() -> Vec<SavedEvent> {
        std::mem::take(&mut *SAVED.lock().unwrap())
    }
}

impl EditorDoc {
    fn new(
        path: &Path,
        remote: Option<RemoteRef>,
        text: String,
        enc: WriteEnc,
        load_error: Option<String>,
        dark: bool,
    ) -> Self {
        let title = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let lines_hint = text.matches('\n').count() + 1;
        Self {
            id: 0, // EditorHub.push 回填
            path: path.to_path_buf(),
            title,
            remote,
            text,
            dirty: false,
            dark,
            enc,
            wrap: true,
            line_numbers: true,
            search: SearchUi {
                open: false,
                show_replace: false,
                find: String::new(),
                replace: String::new(),
                case_sensitive: false,
                matches: Vec::new(),
                current: 0,
                sig: (String::new(), false, 0),
            },
            load_error,
            save_err: None,
            cursor_cache: (0, 1, 1),
            sel_cache: None,
            lines_hint,
            close_ask: false,
            closed: false,
        }
    }

    /// 标题文本（C# UpdateTitleText：`名 * - iFreeSftp`；状态栏由窗口层拼）。
    pub fn window_title(&self) -> String {
        if self.dirty {
            format!("{} * - iFreeSftp", self.title)
        } else {
            format!("{} - iFreeSftp", self.title)
        }
    }

    /// 保存：按探测编码写回本地路径；远程文档推回传事件。
    /// 返回 Err(消息) = 写盘失败（窗口内提示）。
    pub fn save(&mut self) -> Result<(), String> {
        if let Some(err) = &self.load_error {
            return Err(err.clone());
        }
        let bytes = encode(&self.text, self.enc);
        std::fs::write(&self.path, bytes).map_err(|e| e.to_string())?;
        self.dirty = false;
        self.lines_hint = self.text.matches('\n').count() + 1;
        if let Some(r) = &self.remote {
            EditorHub::push_saved(SavedEvent {
                doc_id: self.id,
                local_path: self.path.clone(),
                remote_path: r.remote_path.clone(),
            });
        }
        Ok(())
    }
}

fn oversize_msg() -> String {
    crate::i18n::t("文件超过 10 MB，暂不支持远程编辑").to_owned()
}
