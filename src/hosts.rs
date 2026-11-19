use std::sync::Mutex;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AuthMode {
    Password,
    KeyFile,
}

impl AuthMode {
    fn as_str(self) -> &'static str {
        match self {
            AuthMode::Password => "Password",
            AuthMode::KeyFile => "KeyFile",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "KeyFile" => AuthMode::KeyFile,
            _ => AuthMode::Password,
        }
    }
}

#[derive(Clone)]
pub struct HostProfile {
    pub name: String,
    pub host: String,
    pub port: i32,
    pub auth_mode: AuthMode,

    pub key_file_path: String,
    pub username: String,

    pub password: String,

    pub credential_unavailable: bool,
    pub local_sync_path: String,
    pub remote_sync_path: String,
    pub sync_enabled: bool,
    pub compare_enabled: bool,

    pub keep_alive_enabled: bool,
    pub keep_alive_interval_seconds: i32,
    pub timeout_seconds: i32,
    pub max_retries: i32,
    pub retry_delay_seconds: i32,

    pub host_key_fingerprint: String,

    pub last_remote_path: String,
}

impl Default for HostProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            host: String::new(),
            port: 22,
            auth_mode: AuthMode::Password,
            key_file_path: String::new(),
            username: String::new(),
            password: String::new(),
            credential_unavailable: false,
            local_sync_path: String::new(),
            remote_sync_path: String::new(),
            sync_enabled: false,
            compare_enabled: false,
            keep_alive_enabled: false,
            keep_alive_interval_seconds: 15,
            timeout_seconds: 6000,
            max_retries: 2,
            retry_delay_seconds: 5,
            host_key_fingerprint: String::new(),
            last_remote_path: String::new(),
        }
    }
}

impl HostProfile {

    pub fn detail(&self) -> String {
        if self.username.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.username, self.host)
        }
    }

    fn key_matches(&self, host: &str, port: i32, user: &str) -> bool {
        self.host == host && self.port == port && self.username == user
    }
}

static HOSTS: Mutex<Vec<HostProfile>> = Mutex::new(Vec::new());

static LOAD_NOTICE: Mutex<Option<(String, String)>> = Mutex::new(None);

pub fn load_notice() -> Option<(String, String)> {
    LOAD_NOTICE.lock().unwrap().clone()
}

static PENDING_LOGS: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

pub fn drain_pending_logs() -> Vec<(String, String)> {
    std::mem::take(&mut *PENDING_LOGS.lock().unwrap())
}

fn push_decrypt_fail_log(reason: &str) {
    PENDING_LOGS.lock().unwrap().push((
        crate::local::now_hms(),
        crate::i18n::tfmt(
            "主机密码解密失败（{0}），已保留原密文；换回原 Windows 账户可恢复，或重新输入密码",
            &[&reason],
        ),
    ));
}

pub fn load() {
    let (loaded, notice) = load_from(&store_path());
    *LOAD_NOTICE.lock().unwrap() = notice;
    *HOSTS.lock().unwrap() = loaded;
}

fn load_from(path: &std::path::Path) -> (Vec<HostProfile>, Option<(String, String)>) {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let mut loaded = Vec::new();
            let loss = parse_into_report(&mut loaded, &text);
            for h in &mut loaded {

                match unprotect_checked(&h.password) {
                    Credential::Plain(s) => h.password = s,
                    Credential::Unavailable => {
                        h.password.clear();
                        h.credential_unavailable = true;
                    }
                }
            }
            match loss {
                Some(reason) => {
                    let notice = backup_corrupt(path).map(|b| (b, reason));
                    (loaded, notice)
                }
                None => (loaded, None),
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Vec::new(), None),
        Err(e) => {
            let notice = backup_corrupt(path).map(|b| (b, e.to_string()));
            (Vec::new(), notice)
        }
    }
}

fn backup_corrupt(path: &std::path::Path) -> Option<String> {
    let backup = path.with_file_name(format!(
        "{}.corrupt-{}.bak",
        path.file_name()?.to_str()?,
        crate::local::now_stamp()
    ));
    std::fs::copy(path, &backup).ok()?;
    Some(backup.to_string_lossy().into_owned())
}

pub fn with<R>(f: impl FnOnce(&[HostProfile]) -> R) -> R {
    f(&HOSTS.lock().unwrap())
}

pub fn replace_working_set(hs: Vec<HostProfile>) {
    *HOSTS.lock().unwrap() = hs;
}

pub fn replace_all(hs: Vec<HostProfile>) -> std::io::Result<()> {
    replace_working_set(hs);
    save()
}

pub fn append_to_disk(p: HostProfile) -> std::io::Result<()> {
    let text = std::fs::read_to_string(store_path())?;
    let mut stored = Vec::new();
    parse_into(&mut stored, &text);

    let mut new = p;
    let cipher =
        protect(&new.password).map_err(|e| std::io::Error::other(format!("密码加密失败：{e}")))?;
    new.password = cipher;
    stored.push(new);
    let (text, _) = serialize(&stored, false);
    crate::app_paths::write_atomic(&store_path(), text.as_bytes())
}

pub fn push(p: HostProfile) {
    HOSTS.lock().unwrap().push(p);
}

pub fn save() -> std::io::Result<()> {
    let hs = HOSTS.lock().unwrap();
    save_locked(&hs)
}

pub(crate) fn store_path_pub() -> std::path::PathBuf {
    store_path()
}

pub(crate) fn parse_snapshot(text: &str) -> Vec<HostProfile> {
    let mut out = Vec::new();
    parse_into(&mut out, text);
    out
}

pub(crate) fn unprotect_pub(stored: &str) -> String {
    unprotect(stored)
}

#[cfg(feature = "official")]
pub(crate) fn protect_pub(stored: &str) -> Result<String, String> {
    protect(stored)
}

fn save_locked(hs: &[HostProfile]) -> std::io::Result<()> {
    save_locked_to(&store_path(), hs)
}

fn save_locked_to(path: &std::path::Path, hs: &[HostProfile]) -> std::io::Result<()> {
    let (text, skips) = serialize(hs, true);
    if !skips.is_empty() {
        return Err(std::io::Error::other(skips.join("；")));
    }
    crate::app_paths::write_atomic(path, text.as_bytes())
}

pub fn save_single_session(
    host: &str,
    port: i32,
    user: &str,
    fingerprint: &str,
    last_path: &str,
) -> std::io::Result<()> {
    let text = std::fs::read_to_string(store_path())?;
    let mut stored = Vec::new();
    parse_into(&mut stored, &text);
    let mut hit = false;
    for h in &mut stored {
        if h.key_matches(host, port, user) {
            h.host_key_fingerprint = fingerprint.to_owned();
            h.last_remote_path = last_path.to_owned();
            hit = true;
        }
    }
    if !hit {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "主机尚未保存进存档",
        ));
    }

    let (text, _) = serialize(&stored, false);
    crate::app_paths::write_atomic(&store_path(), text.as_bytes())
}

pub fn update_fingerprint(host: &str, port: i32, user: &str, fingerprint: &str) {
    for h in HOSTS.lock().unwrap().iter_mut() {
        if h.key_matches(host, port, user) {
            h.host_key_fingerprint = fingerprint.to_owned();
        }
    }
}

fn store_path() -> std::path::PathBuf {
    crate::app_paths::data_dir().join("hosts.json")
}

fn serialize(hs: &[HostProfile], enc: bool) -> (String, Vec<String>) {
    let mut o = String::with_capacity(2048);
    o.push_str("{\r\n  \"hosts\": [\r\n");

    let mut skips: Vec<String> = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    for h in hs {
        let pw = if enc {
            match protect(&h.password) {
                Ok(p) => p,
                Err(e) => {
                    skips.push(format!("「{}」密码加密失败：{e}", host_label(h)));
                    continue;
                }
            }
        } else {
            h.password.clone()
        };
        let mut part = String::with_capacity(512);
        part.push_str("    {\r\n");
        let mut w = |k: &str, v: String| {
            part.push_str(&format!("      \"{k}\": {v},\r\n"));
        };
        w("Name", format!("\"{}\"", json_escape(&h.name)));
        w("Host", format!("\"{}\"", json_escape(&h.host)));
        w("Port", h.port.to_string());
        w("AuthMode", format!("\"{}\"", h.auth_mode.as_str()));
        w(
            "KeyFilePath",
            format!("\"{}\"", json_escape(&h.key_file_path)),
        );
        w("Username", format!("\"{}\"", json_escape(&h.username)));
        w("Password", format!("\"{}\"", json_escape(&pw)));
        w(
            "LocalSyncPath",
            format!("\"{}\"", json_escape(&h.local_sync_path)),
        );
        w(
            "RemoteSyncPath",
            format!("\"{}\"", json_escape(&h.remote_sync_path)),
        );
        w("SyncEnabled", h.sync_enabled.to_string());
        w("CompareEnabled", h.compare_enabled.to_string());
        w("KeepAliveEnabled", h.keep_alive_enabled.to_string());
        w(
            "KeepAliveIntervalSeconds",
            h.keep_alive_interval_seconds.to_string(),
        );
        w("TimeoutSeconds", h.timeout_seconds.to_string());
        w("MaxRetries", h.max_retries.to_string());
        w("RetryDelaySeconds", h.retry_delay_seconds.to_string());
        w(
            "HostKeyFingerprint",
            format!("\"{}\"", json_escape(&h.host_key_fingerprint)),
        );

        part.push_str("      \"LastRemotePath\": ");
        part.push_str(&format!("\"{}\"\r\n", json_escape(&h.last_remote_path)));
        part.push_str("    }");
        parts.push(part);
    }

    o.push_str(&parts.join(",\r\n"));
    if !parts.is_empty() {
        o.push_str("\r\n");
    }
    o.push_str("  ]\r\n}");
    (o, skips)
}

fn host_label(h: &HostProfile) -> String {
    if !h.name.is_empty() {
        h.name.clone()
    } else if !h.host.is_empty() {
        h.host.clone()
    } else {
        "未命名主机".to_owned()
    }
}

fn parse_into(out: &mut Vec<HostProfile>, text: &str) {
    parse_into_report(out, text);
}

fn parse_into_report(out: &mut Vec<HostProfile>, text: &str) -> Option<String> {
    let mut depth = 0usize;
    let mut cur: Option<HostProfile> = None;
    let mut unclosed = 0usize;
    let mut stray = 0usize;
    for line in text.lines() {
        let t = line.trim();
        let t = t.strip_suffix(',').unwrap_or(t);
        match t {
            "{" => {
                depth += 1;
                if depth == 2 {
                    cur = Some(HostProfile::default());
                }
            }
            "}" | "]" => {
                if t == "}"
                    && depth == 2
                    && let Some(p) = cur.take()
                {
                    out.push(p);
                }
                depth = depth.saturating_sub(1);
            }
            _ => {

                let Some((k, v)) = split_kv(t) else {
                    continue;
                };
                let Some(p) = cur.as_mut() else {

                    if !k.eq_ignore_ascii_case("hosts") {
                        stray += 1;
                    }
                    continue;
                };

                match k.to_ascii_lowercase().as_str() {
                    "name" => p.name = as_str(v).unwrap_or_default(),
                    "host" => p.host = as_str(v).unwrap_or_default(),
                    "port" => p.port = as_i(v).unwrap_or(22),
                    "authmode" => {
                        p.auth_mode = as_str(v)
                            .map(|s| AuthMode::parse(&s))
                            .unwrap_or(AuthMode::Password)
                    }
                    "keyfilepath" => p.key_file_path = as_str(v).unwrap_or_default(),
                    "username" => p.username = as_str(v).unwrap_or_default(),
                    "password" => p.password = as_str(v).unwrap_or_default(),
                    "localsyncpath" => p.local_sync_path = as_str(v).unwrap_or_default(),
                    "remotesyncpath" => p.remote_sync_path = as_str(v).unwrap_or_default(),
                    "syncenabled" => p.sync_enabled = as_bool(v),
                    "compareenabled" => p.compare_enabled = as_bool(v),
                    "keepaliveenabled" => p.keep_alive_enabled = as_bool(v),
                    "keepaliveintervalseconds" => {
                        p.keep_alive_interval_seconds = as_i(v).unwrap_or(15)
                    }
                    "timeoutseconds" => p.timeout_seconds = as_i(v).unwrap_or(6000),
                    "maxretries" => p.max_retries = as_i(v).unwrap_or(2),
                    "retrydelayseconds" => p.retry_delay_seconds = as_i(v).unwrap_or(5),
                    "hostkeyfingerprint" => p.host_key_fingerprint = as_str(v).unwrap_or_default(),
                    "lastremotepath" => p.last_remote_path = as_str(v).unwrap_or_default(),
                    _ => {}
                }
            }
        }
    }
    if cur.is_some() {
        unclosed += 1;
    }
    if unclosed > 0 {
        return Some(format!(
            "记录未闭合（存档可能被截断），{unclosed} 条记录不完整"
        ));
    }
    if stray > 0 {
        return Some(format!("记录外存在游离字段 {stray} 处（存档结构损坏）"));
    }
    if out.is_empty() && !text.trim().is_empty() && !text.contains("\"hosts\"") {
        return Some("存档结构无法识别（不是本程序写出的格式）".to_owned());
    }
    None
}

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let t = line.trim();
    let q0 = t.find('"')?;
    let k_rest = &t[q0 + 1..];
    let q1 = k_rest.find('"')?;
    let key = &k_rest[..q1];
    let rest = k_rest[q1 + 1..].trim();
    let rest = rest.strip_prefix(':')?;
    Some((key, rest.trim()))
}

fn as_str(v: &str) -> Option<String> {
    let t = v.trim();
    let s = t.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Ok(n) = u32::from_str_radix(&hex, 16)
                        && let Some(ch) = char::from_u32(n)
                    {
                        out.push(ch);
                    }
                }
                Some(c) => out.push(c),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    Some(out)
}

fn as_i(v: &str) -> Option<i32> {
    v.trim().parse().ok()
}

fn as_bool(v: &str) -> bool {
    v.trim() == "true"
}

pub(crate) fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

const ENC_PREFIX: &str = "enc:";

#[cfg(windows)]
const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;

#[cfg(windows)]
#[repr(C)]
struct CryptBlob {
    cb: u32,
    data: *mut u8,
}

#[cfg(windows)]
#[link(name = "crypt32")]
unsafe extern "system" {
    fn CryptProtectData(
        data_in: *const CryptBlob,
        descr: *const u16,
        entropy: *const CryptBlob,
        reserved: *const core::ffi::c_void,
        prompt: *const core::ffi::c_void,
        flags: u32,
        data_out: *mut CryptBlob,
    ) -> i32;
    fn CryptUnprotectData(
        data_in: *const CryptBlob,
        descr: *mut *mut u16,
        entropy: *const CryptBlob,
        reserved: *const core::ffi::c_void,
        prompt: *const core::ffi::c_void,
        flags: u32,
        data_out: *mut CryptBlob,
    ) -> i32;
}

#[cfg(windows)]
unsafe extern "system" {
    fn LocalFree(h: *mut core::ffi::c_void) -> *mut core::ffi::c_void;

    fn GetLastError() -> u32;
}

#[cfg(all(test, windows))]
pub(crate) static TEST_DPAPI_FAIL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(all(test, windows))]
pub(crate) static TEST_DPAPI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn protect(password: &str) -> Result<String, String> {
    if password.is_empty() || password.starts_with(ENC_PREFIX) {
        return Ok(password.to_owned());
    }
    #[cfg(windows)]
    {
        #[cfg(all(test, windows))]
        if TEST_DPAPI_FAIL.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("测试桩：DPAPI 强制失败".to_owned());
        }
        let bytes = password.as_bytes();
        let input = CryptBlob {
            cb: bytes.len() as u32,
            data: bytes.as_ptr() as *mut u8,
        };
        let mut out = CryptBlob {
            cb: 0,
            data: std::ptr::null_mut(),
        };

        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 || out.data.is_null() || out.cb == 0 {
            return Err(format!("DPAPI CryptProtectData 失败（code {ok}）"));
        }
        let cipher = unsafe { std::slice::from_raw_parts(out.data, out.cb as usize) };
        let b64 = base64_encode(cipher);
        unsafe { LocalFree(out.data as *mut core::ffi::c_void) };
        Ok(format!("{ENC_PREFIX}{b64}"))
    }
    #[cfg(not(windows))]
    {

        let _ = password;
        Err("当前平台不支持凭据加密（DPAPI）".to_owned())
    }
}

fn unprotect(stored: &str) -> String {
    if stored.is_empty() || !stored.starts_with(ENC_PREFIX) {
        return stored.to_owned();
    }
    #[cfg(windows)]
    {
        let Some(cipher) = base64_decode(&stored[ENC_PREFIX.len()..]) else {
            push_decrypt_fail_log("密文 Base64 解码失败");
            return stored.to_owned();
        };
        let input = CryptBlob {
            cb: cipher.len() as u32,
            data: cipher.as_ptr() as *mut u8,
        };
        let mut out = CryptBlob {
            cb: 0,
            data: std::ptr::null_mut(),
        };

        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            )
        };
        if ok == 0 || out.data.is_null() || out.cb == 0 {

            let e = unsafe { GetLastError() };
            push_decrypt_fail_log(&format!("DPAPI 解密失败（Win32 错误码 {e}）"));
            return stored.to_owned();
        }
        let plain = unsafe { std::slice::from_raw_parts(out.data, out.cb as usize) };
        let s = String::from_utf8_lossy(plain).into_owned();
        unsafe { LocalFree(out.data as *mut core::ffi::c_void) };
        s
    }
    #[cfg(not(windows))]
    {

        stored.to_owned()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Credential {
    Plain(String),
    Unavailable,
}

fn unprotect_checked(stored: &str) -> Credential {
    if stored.is_empty() || !stored.starts_with(ENC_PREFIX) {
        return Credential::Plain(stored.to_owned());
    }
    let s = unprotect(stored);
    if s.starts_with(ENC_PREFIX) {
        #[cfg(not(windows))]
        {
            push_decrypt_fail_log("当前平台不支持凭据加密（DPAPI）");
        }
        Credential::Unavailable
    } else {
        Credential::Plain(s)
    }
}

#[cfg(windows)]
const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[cfg(windows)]
fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18 & 63) as usize] as char);
        out.push(B64[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(windows)]
fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let mut vals = Vec::with_capacity(s.len());
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' => continue,
            _ => return None,
        };
        vals.push(v);
    }
    let mut out = Vec::with_capacity(vals.len() * 3 / 4);
    for chunk in vals.chunks(4) {
        if chunk.len() < 2 {
            break;
        }
        let mut n = 0u32;
        for (i, v) in chunk.iter().enumerate() {
            n |= u32::from(*v) << (18 - 6 * i);
        }
        out.push((n >> 16) as u8);
        if chunk.len() > 2 {
            out.push((n >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("ifree-hosts-test-{tag}.json"));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn sample(pw: &str) -> HostProfile {
        let mut h = HostProfile::default();
        h.name = "web1".to_owned();
        h.host = "10.0.0.1".to_owned();
        h.username = "root".to_owned();
        h.password = pw.to_owned();
        h
    }

    fn unique_store(tag: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ifree-cr3-{tag}-{}-{}.json",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        p
    }

    #[test]
    fn unprotect_checked_legacy_plain_is_plain() {
        assert_eq!(unprotect_checked(""), Credential::Plain(String::new()));
        assert_eq!(
            unprotect_checked("pw123"),
            Credential::Plain("pw123".to_owned())
        );
    }

    #[test]
    fn unprotect_checked_undecryptable_is_unavailable() {
        assert_eq!(unprotect_checked("enc:AAAA"), Credential::Unavailable);
    }

    #[test]
    fn load_from_clears_unavailable_credential() {
        let p = unique_store("unavailable");
        let text = concat!(
            "{\r\n  \"hosts\": [\r\n",
            "    {\r\n",
            "      \"Name\": \"web1\",\r\n",
            "      \"Host\": \"10.0.0.1\",\r\n",
            "      \"Port\": 22,\r\n",
            "      \"Username\": \"root\",\r\n",
            "      \"AuthMode\": \"Password\",\r\n",
            "      \"Password\": \"enc:AAAA\",\r\n",
            "      \"LastRemotePath\": \"\"\r\n",
            "    }\r\n",
            "  ]\r\n}"
        );
        std::fs::write(&p, text).unwrap();
        let (loaded, notice) = load_from(&p);
        assert!(notice.is_none(), "结构完整不得误报损坏: {notice:?}");
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].password.is_empty(), "密文不得留在工作集");
        assert!(loaded[0].credential_unavailable, "必须打标待重录");
        assert_eq!(
            std::fs::read(&p).unwrap(),
            text.as_bytes(),
            "载入不得改写存档"
        );
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn credential_unavailable_not_serialized() {
        let mut h = sample("");
        h.credential_unavailable = true;
        let (text, skips) = serialize(&[h], true);
        assert!(skips.is_empty());
        assert!(!text.contains("credential_unavailable"));
        assert!(text.contains("\"Password\": \"\""));
    }

    #[cfg(not(windows))]
    #[test]
    fn save_locked_batch_kept_on_protect_err() {
        let p = temp_store("nonwin");
        std::fs::write(&p, b"ORIGINAL").unwrap();
        let r = save_locked_to(&p, &[sample("secret")]);
        assert!(r.is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"ORIGINAL");

        assert!(!String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains("secret"));
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(windows)]
    #[test]
    fn save_locked_batch_kept_on_dpapi_fail() {
        use std::sync::atomic::Ordering;
        let _g = TEST_DPAPI_LOCK.lock().unwrap();
        TEST_DPAPI_FAIL.store(true, Ordering::Relaxed);
        let p = temp_store("dpapifail");
        std::fs::write(&p, b"ORIGINAL").unwrap();
        let r = save_locked_to(&p, &[sample("secret")]);
        TEST_DPAPI_FAIL.store(false, Ordering::Relaxed);
        drop(_g);
        assert!(r.is_err());
        assert!(r.unwrap_err().to_string().contains("web1"));
        assert_eq!(std::fs::read(&p).unwrap(), b"ORIGINAL");

        assert!(!String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains("secret"));
        let _ = std::fs::remove_file(&p);
    }

    fn corrupt_store(tag: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ifree-hosts-corrupt-{tag}-{}.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn load_from_truncated_backs_up() {
        let p = corrupt_store("trunc");

        let good = "{\r\n  \"hosts\": [\r\n    {\r\n      \"Name\": \"web1\",\r\n      \"Host\": \"10.0.0.1\",\r\n      \"Port\": 22,\r\n      \"Username\": \"root\",\r\n      \"Password\": \"\",\r\n      \"LastRemotePath\": \"\"\r\n    },\r\n    {\r\n      \"Name\": \"broken";
        std::fs::write(&p, good).unwrap();
        let (loaded, notice) = load_from(&p);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].name, "web1");
        let (backup, reason) = notice.expect("损坏存档必须出通知");
        assert!(reason.contains("未闭合"), "原因可读: {reason}");
        assert!(backup.contains(".corrupt-") && backup.ends_with(".bak"));
        let bpath = std::path::PathBuf::from(&backup);
        assert!(bpath.is_file(), "备份件必须生成: {backup}");
        assert_eq!(std::fs::read(&bpath).unwrap(), good.as_bytes());
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&bpath);
    }

    #[test]
    fn load_from_undecodable_backs_up() {
        let p = corrupt_store("bin");
        let raw: &[u8] = &[0xFF, 0xFE, 0x00, 0x01, 0x9C];
        std::fs::write(&p, raw).unwrap();
        let (loaded, notice) = load_from(&p);
        assert!(loaded.is_empty());
        let (backup, _) = notice.expect("读取失败必须出通知");
        let bpath = std::path::PathBuf::from(&backup);
        assert!(bpath.is_file());
        assert_eq!(std::fs::read(&bpath).unwrap(), raw);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&bpath);
    }

    #[test]
    fn load_from_clean_no_notice() {
        let p = corrupt_store("clean");
        let (text, _) = serialize(&[sample("")], true);
        std::fs::write(&p, &text).unwrap();
        let (loaded, notice) = load_from(&p);
        assert!(notice.is_none());
        assert_eq!(loaded.len(), 1);

        let dir = p.parent().unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map_while(Result::ok)
            .filter(|e| {
                e.file_name().to_string_lossy().contains(&format!(
                    "ifree-hosts-corrupt-clean-{}.corrupt-",
                    std::process::id()
                ))
            })
            .collect();
        assert!(leftovers.is_empty(), "正常存档不得生成备份件");
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(test)]
mod protect_tests {
    use super::*;

    #[cfg(windows)]
    fn stub_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_DPAPI_LOCK.lock().unwrap()
    }
    #[cfg(not(windows))]
    fn stub_guard() {}

    #[test]
    fn protect_is_idempotent() {
        let _g = stub_guard();
        let once = protect("秘密pw").unwrap();
        assert!(once.starts_with(ENC_PREFIX));
        assert_eq!(protect(&once).unwrap(), once);
    }

    #[test]
    fn protect_empty_passes_through() {
        let _g = stub_guard();
        assert_eq!(protect("").unwrap(), "");
    }

    #[test]
    fn protect_unprotect_roundtrip() {
        let _g = stub_guard();
        assert_eq!(unprotect(&protect("pw123").unwrap()), "pw123");
    }

    #[test]
    fn unprotect_legacy_plaintext_passes_through() {
        assert_eq!(unprotect("旧版明文pw"), "旧版明文pw");
        assert_eq!(unprotect(""), "");
    }

    #[test]
    fn unprotect_undecryptable_returns_original_cipher() {
        assert_eq!(unprotect("enc:AAAA"), "enc:AAAA");
        assert_eq!(unprotect("enc:!!!"), "enc:!!!");
        assert_eq!(unprotect("enc:abc"), "enc:abc");
    }

    #[test]
    fn undecryptable_cipher_flows_through_save_without_loss() {
        let _g = stub_guard();
        const CORRUPTED: &str = "enc:AAAA";
        let in_memory = unprotect(CORRUPTED);
        assert_eq!(in_memory, CORRUPTED);
        assert_eq!(protect(&in_memory).unwrap(), CORRUPTED);
    }
}
