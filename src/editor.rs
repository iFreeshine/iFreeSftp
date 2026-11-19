use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

pub const PROBE_BYTES: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteEnc {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Ansi,
}

impl WriteEnc {

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

pub fn decode(bytes: &[u8]) -> (String, WriteEnc) {
    if bytes.len() >= 3 && bytes[..3] == [0xEF, 0xBB, 0xBF] {
        return (
            String::from_utf8_lossy(&bytes[3..]).into_owned(),
            WriteEnc::Utf8Bom,
        );
    }
    if bytes.len() >= 2 && bytes[..2] == [0xFF, 0xFE] {

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

        Ok(s) => (s.to_owned(), WriteEnc::Utf8),
        Err(_) => (ansi_decode(bytes), WriteEnc::Ansi),
    }
}

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

#[cfg(windows)]
fn ansi_decode(bytes: &[u8]) -> String {
    let len = bytes.len();
    if len == 0 {
        return String::new();
    }

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

#[cfg(windows)]
fn ansi_encode(text: &str) -> Vec<u8> {
    let u16s: Vec<u16> = text.encode_utf16().collect();
    if u16s.is_empty() {
        return Vec::new();
    }

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

pub fn is_text_file(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    if data.is_empty() {
        return true;
    }
    is_text_data(&data[..data.len().min(PROBE_BYTES)])
}

const REMOTE_BINARY_EXTS: &[&str] = &[
    "exe", "dll", "so", "dylib", "a", "lib", "obj", "o", "bin", "iso", "img", "zip", "rar", "7z",
    "gz", "tgz", "bz2", "xz", "zst", "tar", "jar", "war", "class", "msi", "apk", "jpg", "jpeg",
    "png", "gif", "bmp", "ico", "webp", "tif", "tiff", "svgz", "pdf", "doc", "docx", "xls", "xlsx",
    "ppt", "pptx", "mp3", "wav", "flac", "ogg", "mp4", "avi", "mkv", "mov", "wmv", "webm", "ttf",
    "otf", "woff", "woff2", "eot", "db", "sqlite", "sqlite3", "pdb",
];

pub fn is_remote_binary_ext(name: &str) -> bool {
    match std::path::Path::new(name).extension() {
        Some(ext) => {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            REMOTE_BINARY_EXTS.contains(&ext.as_str())
        }
        None => false,
    }
}

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

pub fn is_remote_text_ext(name: &str) -> bool {
    match std::path::Path::new(name).extension() {
        Some(ext) => {
            let ext = ext.to_string_lossy().to_ascii_lowercase();
            REMOTE_TEXT_EXTS.contains(&ext.as_str())
        }
        None => false,
    }
}

fn remote_edit_root() -> PathBuf {
    let base = std::env::var("TEMP")
        .or_else(|_| std::env::var("TMP"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("iFreeSftp").join("RemoteEdit")
}

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

pub fn write_remote_copy(host: &str, remote_path: &str, data: &[u8]) -> Result<PathBuf, String> {
    let name = remote_path.rsplit('/').next().unwrap_or("file");
    let dir = remote_tmp_dir(host, remote_path);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(sanitize(name));
    std::fs::write(&path, data).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn remove_remote_copy(dir_host: &str, remote_path: &str) {
    let _ = std::fs::remove_dir_all(remote_tmp_dir(dir_host, remote_path));
}

#[derive(Clone)]
pub struct RemoteRef {

    pub host: String,
    pub remote_path: String,

    pub base_mtime: Option<i64>,
}

pub struct SearchUi {
    pub open: bool,
    pub show_replace: bool,
    pub find: String,
    pub replace: String,
    pub case_sensitive: bool,

    pub matches: Vec<(usize, usize)>,
    pub current: usize,

    pub sig: (String, bool, usize),
}

pub struct EditorDoc {
    pub id: u64,

    pub path: PathBuf,

    pub title: String,

    pub remote: Option<RemoteRef>,
    pub text: String,
    pub dirty: bool,

    pub dark: bool,
    pub enc: WriteEnc,
    pub wrap: bool,
    pub line_numbers: bool,
    pub search: SearchUi,

    pub load_error: Option<String>,

    pub save_err: Option<String>,

    pub cursor_cache: (usize, usize, usize),

    pub sel_cache: Option<(usize, usize)>,

    pub lines_hint: usize,

    pub pending_upload: bool,

    pub last_receipt: Option<String>,

    pub close_ask: bool,

    pub closed: bool,
}

pub struct SavedEvent {
    pub doc_id: u64,

    pub local_path: PathBuf,
    pub remote_path: String,

    pub host: String,

    pub base_mtime: Option<i64>,
}

pub struct EditorHub {
    pub docs: Vec<Arc<Mutex<EditorDoc>>>,
    next_id: u64,
}

static SAVED: Mutex<Vec<SavedEvent>> = Mutex::new(Vec::new());

impl EditorHub {
    pub fn new() -> Self {
        Self {
            docs: Vec::new(),
            next_id: 1,
        }
    }

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

    pub fn push_saved(ev: SavedEvent) {
        SAVED.lock().unwrap().push(ev);
    }

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
            id: 0,
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
            pending_upload: false,
            last_receipt: None,
            close_ask: false,
            closed: false,
        }
    }

    pub fn window_title(&self) -> String {
        if self.dirty {
            format!("{} * - iFreeSftp", self.title)
        } else {
            format!("{} - iFreeSftp", self.title)
        }
    }

    pub fn save(&mut self) -> Result<(), String> {
        if let Some(err) = &self.load_error {
            return Err(err.clone());
        }
        let bytes = encode(&self.text, self.enc);
        std::fs::write(&self.path, bytes).map_err(|e| e.to_string())?;
        self.lines_hint = self.text.matches('\n').count() + 1;
        if let Some(r) = &self.remote {

            self.pending_upload = true;
            self.last_receipt = None;
            EditorHub::push_saved(SavedEvent {
                doc_id: self.id,
                local_path: self.path.clone(),
                remote_path: r.remote_path.clone(),
                host: r.host.clone(),
                base_mtime: r.base_mtime,
            });
        } else {
            self.dirty = false;
        }
        Ok(())
    }
}

fn oversize_msg() -> String {
    crate::i18n::t("文件超过 10 MB，暂不支持远程编辑").to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_save_keeps_dirty_until_receipt_and_local_saves_clear() {
        let dir = std::env::temp_dir().join("ifreesftp-cr6-editor-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let copy = dir.join("remote_save_nail.txt");
        std::fs::write(&copy, b"v1").unwrap();
        let mut hub = EditorHub::new();
        hub.open_remote("h1", &copy, "/r.txt".into(), Some(1000), true);
        let doc = hub.docs[0].clone();
        {
            let mut d = doc.lock().unwrap();
            d.text = "v2".into();
            d.dirty = true;
            d.save().unwrap();
            assert!(d.dirty, "远程文档 save 后 dirty 恒为 true（回执未到）");
            assert!(d.pending_upload, "已入队、回执未到");
            assert!(d.last_receipt.is_none(), "成功回执未到不得有旧值");
        }
        let evs = EditorHub::drain_saved();
        assert_eq!(evs.len(), 1, "save 入队一条回传事件");
        assert_eq!(evs[0].doc_id, doc.lock().unwrap().id);
        assert_eq!(evs[0].host, "h1");
        assert_eq!(evs[0].remote_path, "/r.txt");
        assert_eq!(evs[0].base_mtime, Some(1000));

        let local = dir.join("local_save_nail.txt");
        std::fs::write(&local, b"v1").unwrap();
        let mut hub2 = EditorHub::new();
        hub2.open_local(&local, true);
        let ldoc = hub2.docs[0].clone();
        let mut d = ldoc.lock().unwrap();
        d.text = "v2".into();
        d.dirty = true;
        d.save().unwrap();
        assert!(!d.dirty, "纯本地文档写盘成功即视为已保存");
        assert!(!d.pending_upload);
    }
}
