//! 主机库（T6 主机持久化）：对齐 C# HostProfile / HostStoreService。
//!
//! - 存档 应用数据目录\hosts.json（app_paths::data_dir；Windows 与 C# 版共用，
//!   结构 `{"hosts": [...]}`，字段名/顺序/类型与 C# 写出一致（手写行式 JSON，
//!   R5 无 serde；行解析与 C# WriteIndented 互读）；
//! - 密码红线（C# §6）：内存工作集明文仅供运行时，落盘 DPAPI（当前用户）密文
//!   前缀 `enc:`；旧版明文原样读入、下次保存升级为密文；已带 enc: 前缀幂等。
//!   DPAPI 经 crypt32 原生 FFI（CryptProtectData/CryptUnprotectData），
//!   零新依赖（R5）；
//! - 损坏存档：parse_into 逐行容错（坏记录跳过、可解析部分照常载入），不让
//!   应用启动失败；**无 .bak 备份逻辑**（i18n 表内两条「已备份到」为 C# resw
//!   迁移残留，Rust 无调用点，见 README 能力矩阵）；
//! - 指纹确认流程（首连弹窗/密钥变更拒绝）随 SSH 实连验证任务接入，字段先行。

use std::sync::Mutex;

/// 认证方式（C# AuthMode：JSON 读写为 "Password"/"KeyFile" 字符串，
/// 未知值回退 Password —— 单条坏值不毁整个存档）。
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

/// 主机档案：连接与同步/浏览记忆的载体（字段语义对齐 C# HostProfile.cs）。
#[derive(Clone)]
pub struct HostProfile {
    pub name: String,
    pub host: String,
    pub port: i32,
    pub auth_mode: AuthMode,
    /// 私钥文件路径（仅 KeyFile 生效；档案不保存私钥内容 —— C# 红线 2）。
    pub key_file_path: String,
    pub username: String,
    /// 内存明文（运行时需要）；落盘时 DPAPI 密文（enc: 前缀）。
    pub password: String,
    pub local_sync_path: String,
    pub remote_sync_path: String,
    pub sync_enabled: bool,
    pub compare_enabled: bool,
    /// 死字段保留（旧档案 JSON 兼容，C# 定案不接线）。
    pub keep_alive_enabled: bool,
    pub keep_alive_interval_seconds: i32,
    pub timeout_seconds: i32,
    pub max_retries: i32,
    pub retry_delay_seconds: i32,
    /// 已信任主机密钥指纹（SHA256 base64，ssh 同格式；空 = 未确认）。
    pub host_key_fingerprint: String,
    /// 上次浏览的远程目录（列举成功即单字段落盘；连接时初始目录优先级
    /// 同步目录 > 本字段 > 登录主目录）。
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
    /// 主机列表副标题（user@host；C# HostPick Detail 同口径）。
    pub fn detail(&self) -> String {
        if self.username.is_empty() {
            self.host.clone()
        } else {
            format!("{}@{}", self.username, self.host)
        }
    }

    /// 连接匹配键（Host+Port+Username，Ordinal —— C# SaveSingle/判重口径）。
    fn key_matches(&self, host: &str, port: i32, user: &str) -> bool {
        self.host == host && self.port == port && self.username == user
    }
}

static HOSTS: Mutex<Vec<HostProfile>> = Mutex::new(Vec::new());

/// 启动载入（main.rs 调用；parse_into 逐行容错，无备份逻辑）。
pub fn load() {
    let mut loaded = Vec::new();
    if let Ok(text) = std::fs::read_to_string(store_path()) {
        parse_into(&mut loaded, &text);
        for h in &mut loaded {
            h.password = unprotect(&h.password);
        }
    } else {
        return;
    }
    *HOSTS.lock().unwrap() = loaded;
}

/// 只读快照访问（HostPicker 列表）。
pub fn with<R>(f: impl FnOnce(&[HostProfile]) -> R) -> R {
    f(&HOSTS.lock().unwrap())
}

/// 显式全量落盘（主机设置面板「保存」：工作集整表替换盘上记录，C# Save 语义）。
/// 落盘失败 → Err（H4：UI 显式报错；改动仍在内存工作集，可修复后重试）。
pub fn replace_all(hs: Vec<HostProfile>) -> std::io::Result<()> {
    *HOSTS.lock().unwrap() = hs;
    save()
}

/// 单条追加落盘（C# AppendHosts：读盘-追加-写回，只把该条密文写进存档，
/// 不动工作集里其他未保存修改；复制/导入即存语义）。
/// 读盘/加密/写盘任一失败 → Err（H4/S1：不静默、不明文落盘；调用方显式报错，
/// 条目仍留在内存工作集，「保存」可重试）。
pub fn append_to_disk(p: HostProfile) -> std::io::Result<()> {
    let text = std::fs::read_to_string(store_path())?;
    let mut stored = Vec::new();
    parse_into(&mut stored, &text);
    // 密文流转：盘上原条目按原文写回（protect=false），新条目单独加密。
    let mut new = p;
    let cipher =
        protect(&new.password).map_err(|e| std::io::Error::other(format!("密码加密失败：{e}")))?;
    new.password = cipher;
    stored.push(new);
    let (text, _) = serialize(&stored, false);
    crate::app_paths::write_atomic(&store_path(), text.as_bytes())
}

/// 单条进内存工作集（C# Hosts.Add：导入链路与 append_to_disk 配对使用，不落盘；
/// HostPanel 重开时经 with 快照立即可见）。
pub fn push(p: HostProfile) {
    HOSTS.lock().unwrap().push(p);
}

/// 全量落盘（工作集写入用；replace_all 内部调用）。
pub fn save() -> std::io::Result<()> {
    let hs = HOSTS.lock().unwrap();
    save_locked(&hs)
}

/// 存档路径（导入解析器读自有存档用）。
pub(crate) fn store_path_pub() -> std::path::PathBuf {
    store_path()
}

/// 存档文本 → 工作集快照（导入解析器读自有存档用；解析容错，坏行忽略）。
pub(crate) fn parse_snapshot(text: &str) -> Vec<HostProfile> {
    let mut out = Vec::new();
    parse_into(&mut out, text);
    out
}

/// DPAPI 密文解密（导入 OwnStore 口令 fail-safe 迁移用；解密失败回原样）。
pub(crate) fn unprotect_pub(stored: &str) -> String {
    unprotect(stored)
}

/// DPAPI 密文加密（registration.dat 注册码存取用；空串/已带前缀幂等原样返回；
/// 加密失败 → Err，调用方不得明文落盘）。
pub(crate) fn protect_pub(stored: &str) -> Result<String, String> {
    protect(stored)
}

fn save_locked(hs: &[HostProfile]) -> std::io::Result<()> {
    save_locked_to(&store_path(), hs)
}

/// 写盘核心（路径注入便于测试）：先 serialize 取跳过清单——有跳条**整批不落盘**
/// 直接返回带主机明细的 Err（盘上存档原封不动，绝不出现半新半旧存档；改动留在
/// 内存工作集，修复后「保存」重试）；无跳条才 write_atomic。
fn save_locked_to(path: &std::path::Path, hs: &[HostProfile]) -> std::io::Result<()> {
    let (text, skips) = serialize(hs, true);
    if !skips.is_empty() {
        return Err(std::io::Error::other(skips.join("；")));
    }
    crate::app_paths::write_atomic(path, text.as_bytes())
}

/// 单字段落盘（C# SaveSingle 边界）：只写该主机「会话过程产生的持久状态」
/// （指纹 + 上次浏览目录），按 Host+Port+Username 在盘上定位；盘上无此记录
/// （新增未保存）→ Err(NotFound)（调用方按「仅本会话生效」提示）；读取失败
/// → Err(NotFound) 同义；写盘/加密错误上抛（H4：不静默），其他记录密文原样流转。
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
    // 密文原样流转：serialize 传 protect=false（parse 得到的 password 即盘上原文）。
    let (text, _) = serialize(&stored, false);
    crate::app_paths::write_atomic(&store_path(), text.as_bytes())
}

/// 指纹回写工作集（C# SetTrustedFingerprint 对应物）：按 Host+Port+Username
/// 定位已加载记录更新指纹字段（内存态，随显式「保存」落盘）。save_single_session
/// 只写盘上记录，工作集由本函数补齐，防显式保存时旧值回滚新指纹。
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

// ── 行式 JSON（C# WriteIndented 结构；逐行解析容忍性同 settings.rs）──────

fn serialize(hs: &[HostProfile], enc: bool) -> (String, Vec<String>) {
    let mut o = String::with_capacity(2048);
    o.push_str("{\r\n  \"hosts\": [\r\n");
    // S1：加密失败不静默明文落盘也不清空 —— 该条整条不写出（其余照写），
    // 错误带主机标识累积进跳过清单返回；save_locked 写盘后按清单上抛，
    // 由调用方决定 UI 报错。
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
        // 末字段无逗号。
        part.push_str("      \"LastRemotePath\": ");
        part.push_str(&format!("\"{}\"\r\n", json_escape(&h.last_remote_path)));
        part.push_str("    }");
        parts.push(part);
    }
    // 条目 join：条目间 ",\r\n"，末条无逗号（字节级同 C# WriteIndented；
    // 跳条后剩余条目依序相连，不残留占位或尾逗号）。
    o.push_str(&parts.join(",\r\n"));
    if !parts.is_empty() {
        o.push_str("\r\n");
    }
    o.push_str("  ]\r\n}");
    (o, skips)
}

/// 错误提示里的主机标识（Name 优先，缺则 Host；全空 = 未命名主机）。
fn host_label(h: &HostProfile) -> String {
    if !h.name.is_empty() {
        h.name.clone()
    } else if !h.host.is_empty() {
        h.host.clone()
    } else {
        "未命名主机".to_owned()
    }
}

/// 逐行解析：`{` 深度计记录边界（只数独占一行的花括号，字符串值不参与），
/// kv 行写入当前记录；非法键/缺失保持默认（单条坏值不毁存档）。
fn parse_into(out: &mut Vec<HostProfile>, text: &str) {
    let mut depth = 0usize;
    let mut cur: Option<HostProfile> = None;
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
                // kv 行用去尾逗号后的 t 拆分（分隔逗号不进值，否则 as_str 去引号失败）。
                let Some((k, v)) = split_kv(t) else {
                    continue;
                };
                let Some(p) = cur.as_mut() else { continue };
                // C# HostImportParser 存档字典 OrdinalIgnoreCase（小写键兼容有
                // HostImportParserTests 守护）：键统一小写匹配，写出仍 PascalCase。
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
}

/// 拆 `"key": value`（行首尾空白容忍；与 settings.rs 同式）。
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

/// 字符串值去引号（最小反转义；C# 写出值只含这些）。
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

/// 最小 JSON 转义（与 settings.rs 同式）。
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

// ── DPAPI（crypt32 FFI，零新依赖）────────────────────────────

const ENC_PREFIX: &str = "enc:";
pub(crate) const ENC_PREFIX_PUB: &str = ENC_PREFIX;

// H5：crypt32 FFI 与常量仅 Windows 编译；其他平台无凭据加密能力（Keychain/
// libsecret 后续平台接入时再加），protect 返回 Err（调用方不得明文落盘）、
// unprotect 对 enc: 密文原样返回（fail-closed）。
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
}

// 测试桩：强制 DPAPI 失败（save_locked 整批不落盘 / 注册激活 SaveFailed
// 语义验证用；cfg(all(test, windows)) 保证正式构建零痕迹）。进程级全局，
// 测试间经 TEST_DPAPI_LOCK 互斥串行，防 store.rs 真 DPAPI 往返测试串扰。
#[cfg(all(test, windows))]
pub(crate) static TEST_DPAPI_FAIL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(all(test, windows))]
pub(crate) static TEST_DPAPI_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// DPAPI（当前用户）加密，enc: 前缀 base64；空值原样返回；已带前缀幂等
/// （解密失败保留在内存的原密文不会被二次加密 —— C# Protect 同语义）。
/// 加密失败 → Err（S1：调用方不得明文落盘）；非 Windows 无加密能力 → Err
/// （本期不接 Keychain/libsecret：hosts 该条跳过落盘 / 注册码激活失败）。
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
        // SAFETY：输入 blob 指向调用方栈上缓冲，输出由系统分配（LocalFree 释放）。
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
        // 红线：无加密能力绝不返回明文（明文落盘 = 凭据泄漏）；调用方按 Err
        // 跳条（hosts）/ 激活失败（注册码），非 Windows 激活页有静态提示。
        let _ = password;
        Err("当前平台不支持凭据加密（DPAPI）".to_owned())
    }
}

/// 解密 DPAPI 密文；无前缀视为旧版明文原样返回（兼容升级）；解不开保留
/// 原密文（换机/换用户 fail-closed，认证失败属预期 —— C# Unprotect 同语义）。
fn unprotect(stored: &str) -> String {
    if stored.is_empty() || !stored.starts_with(ENC_PREFIX) {
        return stored.to_owned();
    }
    #[cfg(windows)]
    {
        let Some(cipher) = base64_decode(&stored[ENC_PREFIX.len()..]) else {
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
        // SAFETY：输入 blob 指向局部 Vec 缓冲；输出由系统分配（LocalFree 释放）。
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
            return stored.to_owned();
        }
        let plain = unsafe { std::slice::from_raw_parts(out.data, out.cb as usize) };
        let s = String::from_utf8_lossy(plain).into_owned();
        unsafe { LocalFree(out.data as *mut core::ffi::c_void) };
        s
    }
    #[cfg(not(windows))]
    {
        // 无 DPAPI 可解：密文原样返回（fail-closed）。
        stored.to_owned()
    }
}

// ── base64（标准字母表 + padding；零依赖）────────────────────

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

    /// 非 Windows：protect 无加密能力恒 Err → 整批不落盘，盘上存档字节不变。
    #[cfg(not(windows))]
    #[test]
    fn save_locked_batch_kept_on_protect_err() {
        let p = temp_store("nonwin");
        std::fs::write(&p, b"ORIGINAL").unwrap();
        let r = save_locked_to(&p, &[sample("secret")]);
        assert!(r.is_err());
        assert_eq!(std::fs::read(&p).unwrap(), b"ORIGINAL");
        // 明文红线：盘上不得出现密码原文
        assert!(!String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains("secret"));
        let _ = std::fs::remove_file(&p);
    }

    /// Windows：DPAPI 失败桩 → 整批不落盘、盘上存档字节不变、Err 带主机明细。
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
        assert!(r.unwrap_err().to_string().contains("web1")); // 明细含主机标识
        assert_eq!(std::fs::read(&p).unwrap(), b"ORIGINAL");
        // 明文红线：盘上不得出现密码原文
        assert!(!String::from_utf8_lossy(&std::fs::read(&p).unwrap()).contains("secret"));
        let _ = std::fs::remove_file(&p);
    }
}

// ============================================================================
// 凭据保护测试（第三批）—— 翻译自 C# HostStoreServiceTests（6 用例）
// 追加到 src/hosts.rs 末尾。protect/unprotect 为模块私有 fn，
// cfg(test) 子模块经 use super::* 直接访问，零重构。
// DPAPI 走真实 Windows API（当前用户上下文，roundtrip 成立）。
// ============================================================================

#[cfg(test)]
mod protect_tests {
    use super::*;

    // DPAPI 失败桩（TEST_DPAPI_FAIL）是进程级全局：并行用例在桩置位窗口内
    // 调 protect 会被误伤（roundtrip 实测翻车）。凡真实调 protect 的用例持锁串行。
    #[cfg(windows)]
    fn stub_guard() -> std::sync::MutexGuard<'static, ()> {
        TEST_DPAPI_LOCK.lock().unwrap()
    }
    #[cfg(not(windows))]
    fn stub_guard() {}

    // C# Protect_IsIdempotent：已带 enc: 前缀原样返回，不被二次加密
    // （核心回归：防"解密失败 → 空串 → Save 覆写 → 密码永久丢失"链）。
    #[test]
    fn protect_is_idempotent() {
        let _g = stub_guard();
        let once = protect("秘密pw").unwrap();
        assert!(once.starts_with(ENC_PREFIX));
        assert_eq!(protect(&once).unwrap(), once);
    }

    // C# Protect_EmptyOrNull_PassesThrough（Rust 无 null，"" 等价覆盖）。
    #[test]
    fn protect_empty_passes_through() {
        let _g = stub_guard();
        assert_eq!(protect("").unwrap(), "");
    }

    // C# Protect_Unprotect_Roundtrip。
    #[test]
    fn protect_unprotect_roundtrip() {
        let _g = stub_guard();
        assert_eq!(unprotect(&protect("pw123").unwrap()), "pw123");
    }

    // C# Unprotect_LegacyPlaintextOrEmpty_PassesThrough：无前缀 = 旧版明文兼容。
    #[test]
    fn unprotect_legacy_plaintext_passes_through() {
        assert_eq!(unprotect("旧版明文pw"), "旧版明文pw");
        assert_eq!(unprotect(""), "");
    }

    // C# Unprotect_Undecryptable_ReturnsOriginalCipher：合法 Base64 但 DPAPI
    // 解不开 / 非法 Base64 / 长度不对——全部保留原密文（fail-closed）。
    // 修复前返回 ""，Save 会把 "" 覆写回存档 → 密码永久丢失。
    #[test]
    fn unprotect_undecryptable_returns_original_cipher() {
        assert_eq!(unprotect("enc:AAAA"), "enc:AAAA"); // DPAPI 解不开
        assert_eq!(unprotect("enc:!!!"), "enc:!!!"); // 非法 Base64
        assert_eq!(unprotect("enc:abc"), "enc:abc"); // Base64 长度不对
    }

    // C# UndecryptableCipher_FlowsThroughSave_WithoutLoss：坏密文流经
    // Unprotect → Protect 全链路必须原样写回（不二次加密不清空）。
    #[test]
    fn undecryptable_cipher_flows_through_save_without_loss() {
        let _g = stub_guard();
        const CORRUPTED: &str = "enc:AAAA";
        let in_memory = unprotect(CORRUPTED);
        assert_eq!(in_memory, CORRUPTED);
        assert_eq!(protect(&in_memory).unwrap(), CORRUPTED);
    }
}
