//! 应用设置存储（T6 设置持久化）：对齐 C# AppSettingsService。
//!
//! - 存储路径 应用数据目录\settings.json（app_paths::data_dir；Windows 与 C#
//!   共用同一存档；缺失回退 exe 同目录）；
//! - 手写行式 JSON（R5 无 serde；C# WriteIndented 每属性一行，行解析互读兼容）；
//! - 500ms 防抖合并落盘（egui 无常驻定时器：dirty + 末次变更时刻，root() 每帧
//!   `maybe_flush`，未到期预约重绘——对齐 C# Timer 防抖语义）；
//! - 主题沿用 window.ini（D-016 先例，此处占位空串保持文件结构）；语言随本存档
//!   持久化（标题栏语言钮 / 设置弹窗下拉共用 `toggle_language` / 弹窗整包回写）。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// C# 各字段默认值与钳制范围（AppSettingsService.cs）。
#[derive(Clone, PartialEq)]
pub struct AppSettings {
    /// "Dark" | "Light" | ""（此 Rust 版恒空：主题走 window.ini）。
    pub theme: String,
    /// "zh-CN" | "en-US" | ""（空 = 中文；i18n::is_en 只认 "en-US"）。
    pub language: String,
    /// 连接超时秒（0=禁用），0-300，默认 20。
    pub connect_timeout: i32,
    /// 最大重试次数，0-99，默认 2。
    pub max_retries: i32,
    /// 登录重试延迟秒，0-999，默认 5。
    pub retry_delay: i32,
    /// 操作超时秒（0=禁用），0-86400，默认 15。
    pub operation_timeout: i32,
    /// 连接打开方式 ask | current | new。
    pub connect_open_mode: usize,
    /// 并发传输数 1-3，默认 2（当前 worker 顺序执行，仅存档）。
    pub concurrent_transfers: i32,
    /// 编辑器模式 wpf | external（bool：true=内置）。
    pub editor_builtin: bool,
    /// 外部编辑器路径。
    pub editor_path: String,
    /// 远程删除方式 exec | recursive（bool：true=服务端命令优先）。
    pub remote_delete_exec: bool,
    /// 保活开关，默认 true。
    pub keep_alive: bool,
    /// 保活间隔秒 15-60，默认 30。
    pub keep_alive_interval: i32,
    /// 更新提醒开关，默认 true。
    pub update_check: bool,
    /// 断点续传开关，默认 true。
    pub resume: bool,
    /// 续传阈值 KB 200-1024，默认 1024。
    pub resume_threshold_kb: i32,
    /// tar 加速开关，默认 false（T5 已实现：本机 tar 打包+服务端解包，失败回退逐文件）。
    pub tar_accel: bool,
    /// tar 生效最小文件数 10-10000，默认 50。
    pub tar_threshold: i32,
    /// 传输后校验，默认 false（已实现：exec 通道比对哈希，命令通道不可用跳过）。
    pub verify: bool,
    /// 校验算法 Auto|SHA256|SHA512|SM3（下标 0-3）。
    pub verify_algo: usize,
    /// 保留修改时间（C# 存档键 preserveMtimeEnabled 互读保留）。Rust 版未实现
    /// （需 filetime，R5 未批准），设置 UI 已摘除，此字段仅存档往返、无消费点。
    pub preserve_mtime: bool,
    // 内部：防抖状态（不入盘；crate 内可构造——弹窗整包回写用）。
    pub(crate) dirty: bool,
    pub(crate) last_change: Option<Instant>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            theme: String::new(),
            language: String::new(),
            connect_timeout: 20,
            max_retries: 2,
            retry_delay: 5,
            operation_timeout: 15,
            connect_open_mode: 0,
            concurrent_transfers: 2,
            editor_builtin: true,
            editor_path: String::new(),
            remote_delete_exec: true,
            keep_alive: true,
            keep_alive_interval: 30,
            update_check: true,
            resume: true,
            resume_threshold_kb: 1024,
            // tar 加速默认关：端到端用例（tar 可用/不可用/非 POSIX 回退 + Fallback
            // 对拍）补齐前不默认启用；门判据与回退逻辑单测已在 tar.rs 落地。
            tar_accel: false,
            tar_threshold: 50,
            verify: false,
            verify_algo: 0,
            preserve_mtime: true,
            dirty: false,
            last_change: None,
        }
    }
}

static GLOBAL: Mutex<Option<AppSettings>> = Mutex::new(None);

/// 启动载入（文件缺失/损坏保持默认——C# 同语义）。
pub fn load() {
    let mut s = GLOBAL.lock().unwrap();
    let got = s.get_or_insert_with(AppSettings::default);
    if let Ok(text) = std::fs::read_to_string(store_path()) {
        parse_into(got, &text);
    }
}

/// 只读快照访问。
pub fn with<R>(f: impl FnOnce(&AppSettings) -> R) -> R {
    let mut g = GLOBAL.lock().unwrap();
    f(g.get_or_insert_with(AppSettings::default))
}

/// 弹窗值回写（整包比对，变化才置脏）。settings 弹窗每帧调用。
pub fn sync_from_dialog(next: AppSettings) {
    let mut g = GLOBAL.lock().unwrap();
    let cur = g.get_or_insert_with(AppSettings::default);
    if cur.dirty_snapshot_fields(&next) {
        *cur = next;
        cur.dirty = true;
        cur.last_change = Some(Instant::now());
    }
}

/// 语言翻转（标题栏语言钮）：zh-CN ↔ en-US，置脏防抖落盘并立即重绘。
/// 设置弹窗打开时走弹窗下拉 + 整包回写，不经过这里（遮罩挡住标题栏，无并发）。
pub fn toggle_language(ctx: &egui::Context) {
    {
        let mut g = GLOBAL.lock().unwrap();
        let s = g.get_or_insert_with(AppSettings::default);
        s.language = if s.language == "en-US" {
            "zh-CN"
        } else {
            "en-US"
        }
        .to_owned();
        s.dirty = true;
        s.last_change = Some(Instant::now());
    }
    ctx.request_repaint();
}

impl AppSettings {
    /// 字段级比对（忽略 dirty/last_change）。
    fn dirty_snapshot_fields(&self, next: &AppSettings) -> bool {
        let mut a = self.clone();
        a.dirty = false;
        a.last_change = None;
        let mut b = next.clone();
        b.dirty = false;
        b.last_change = None;
        a != b
    }
}

/// 每帧防抖落盘（root() 调用）：到期写盘；未到期预约重绘。返回是否仍在防抖中。
pub fn maybe_flush(ctx: &egui::Context) -> bool {
    let due = {
        let mut g = GLOBAL.lock().unwrap();
        let Some(s) = g.as_mut() else {
            return false;
        };
        if !s.dirty {
            return false;
        }
        match s.last_change {
            Some(t) if t.elapsed() >= Duration::from_millis(500) => true,
            Some(t) => {
                let wait = Duration::from_millis(500) - t.elapsed();
                ctx.request_repaint_after(wait);
                false
            }
            None => true,
        }
    };
    if due {
        match flush_now() {
            Ok(()) => *SAVE_ERROR.lock().unwrap() = None,
            Err(e) => {
                // H4：落盘失败不静默——错误挂起待 UI 帧循环取走显式上报
                //（take 语义防每帧刷屏）；脏标记已清，下次改动再试。
                *SAVE_ERROR.lock().unwrap() = Some(crate::i18n::tfmt("设置保存失败：{0}", &[&e]));
            }
        }
    }
    due
}

/// 最近一次设置落盘失败（H4：Some = 待上报；UI 帧循环 take 走显式提示；
/// 成功落盘自动清空）。dirty 已清不自动重试，避免每帧反复写盘。
static SAVE_ERROR: Mutex<Option<String>> = Mutex::new(None);

/// 取走并清空待上报的设置落盘错误（root() 每帧调用）。
pub fn take_save_error() -> Option<String> {
    SAVE_ERROR.lock().unwrap().take()
}

/// 立即落盘挂起改动（退出前语义；当前版本窗口关闭即进程退出，防抖窗口短，
/// 风险可忽略——保留接口对齐 C# FlushNow）。失败 → Err（maybe_flush 转挂起上报）。
pub fn flush_now() -> std::io::Result<()> {
    let mut g = GLOBAL.lock().unwrap();
    let Some(s) = g.as_mut() else {
        return Ok(());
    };
    if !s.dirty {
        return Ok(());
    }
    s.dirty = false;
    let text = serialize(s);
    crate::app_paths::write_atomic(&store_path(), text.as_bytes())
}

fn store_path() -> std::path::PathBuf {
    crate::app_paths::data_dir().join("settings.json")
}

// ── 行式 JSON 读写（每属性一行；字符串值仅含转义）──────────

/// 最小 JSON 字符串转义（\ " 与 <0x20；中文原样——C# UnsafeRelaxedJsonEscaping 同式）。
fn json_escape(s: &str) -> String {
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

fn serialize(s: &AppSettings) -> String {
    let mut o = String::with_capacity(1024);
    o.push_str("{\r\n");
    let mut w = |k: &str, v: String| {
        o.push_str(&format!("  \"{k}\": {v},\r\n"));
    };
    w("theme", format!("\"{}\"", json_escape(&s.theme)));
    w("language", format!("\"{}\"", json_escape(&s.language)));
    w("connectTimeout", s.connect_timeout.to_string());
    w("maxRetries", s.max_retries.to_string());
    w("retryDelay", s.retry_delay.to_string());
    w("operationTimeout", s.operation_timeout.to_string());
    w(
        "connectOpenMode",
        format!(
            "\"{}\"",
            ["ask", "current", "new"][s.connect_open_mode.min(2)]
        ),
    );
    w("concurrentTransfers", s.concurrent_transfers.to_string());
    w(
        "editorMode",
        format!("\"{}\"", if s.editor_builtin { "wpf" } else { "external" }),
    );
    w("editorPath", format!("\"{}\"", json_escape(&s.editor_path)));
    w(
        "remoteDeleteMode",
        format!(
            "\"{}\"",
            if s.remote_delete_exec {
                "exec"
            } else {
                "recursive"
            }
        ),
    );
    w("keepAliveEnabled", s.keep_alive.to_string());
    w("keepAliveInterval", s.keep_alive_interval.to_string());
    w("updateCheckEnabled", s.update_check.to_string());
    w("resumeEnabled", s.resume.to_string());
    w("resumeThresholdKb", s.resume_threshold_kb.to_string());
    w("tarAccelEnabled", s.tar_accel.to_string());
    w("tarAccelMinFiles", s.tar_threshold.to_string());
    w("verifyAfterTransfer", s.verify.to_string());
    w(
        "verifyAlgorithm",
        format!(
            "\"{}\"",
            ["Auto", "SHA256", "SHA512", "SM3"][s.verify_algo.min(3)]
        ),
    );
    w("preserveMtimeEnabled", s.preserve_mtime.to_string());
    // 末行无逗号（重写最后一条）。
    if o.ends_with(",\r\n") {
        o.truncate(o.len() - 3);
        o.push_str("\r\n");
    }
    o.push('}');
    o
}

/// 行式解析：逐行取 "key": value；非法/缺失键保持现值（C# TryGetProperty 同语义）。
fn parse_into(s: &mut AppSettings, text: &str) {
    for line in text.lines() {
        // 去掉行尾分隔逗号再拆键值（写出侧每行末带逗号、末行除外；逗号进值会令
        // as_str 去引号失败）。
        let t = line.trim();
        let t = t.strip_suffix(',').unwrap_or(t);
        let Some((k, v)) = split_kv(t) else {
            continue;
        };
        match k {
            "theme" => {
                if let Some(t) = as_str(v) {
                    s.theme = t;
                }
            }
            "language" => {
                if let Some(t) = as_str(v) {
                    s.language = t;
                }
            }
            "connectTimeout" => {
                if let Some(n) = as_i(v) {
                    s.connect_timeout = n.clamp(0, 300);
                }
            }
            "maxRetries" => {
                if let Some(n) = as_i(v) {
                    s.max_retries = n.clamp(0, 99);
                }
            }
            "retryDelay" => {
                if let Some(n) = as_i(v) {
                    s.retry_delay = n.clamp(0, 999);
                }
            }
            "operationTimeout" => {
                if let Some(n) = as_i(v) {
                    s.operation_timeout = n.clamp(0, 86400);
                }
            }
            "connectOpenMode" => {
                if let Some(t) = as_str(v) {
                    s.connect_open_mode = match t.as_str() {
                        "current" => 1,
                        "new" => 2,
                        "ask" => 0,
                        _ => s.connect_open_mode,
                    };
                }
            }
            "concurrentTransfers" => {
                if let Some(n) = as_i(v) {
                    s.concurrent_transfers = n.clamp(1, 3);
                }
            }
            "editorMode" => {
                if let Some(t) = as_str(v) {
                    s.editor_builtin = t != "external";
                }
            }
            "editorPath" => {
                if let Some(t) = as_str(v) {
                    s.editor_path = t.trim().to_owned();
                }
            }
            "remoteDeleteMode" => {
                if let Some(t) = as_str(v) {
                    s.remote_delete_exec = t != "recursive";
                }
            }
            "keepAliveEnabled" => {
                if let Some(b) = as_bool(v) {
                    s.keep_alive = b;
                }
            }
            "keepAliveInterval" => {
                if let Some(n) = as_i(v) {
                    s.keep_alive_interval = n.clamp(15, 60);
                }
            }
            "updateCheckEnabled" => {
                if let Some(b) = as_bool(v) {
                    s.update_check = b;
                }
            }
            "resumeEnabled" => {
                if let Some(b) = as_bool(v) {
                    s.resume = b;
                }
            }
            "resumeThresholdKb" => {
                if let Some(n) = as_i(v) {
                    s.resume_threshold_kb = n.clamp(200, 1024);
                }
            }
            "tarAccelEnabled" => {
                if let Some(b) = as_bool(v) {
                    s.tar_accel = b;
                }
            }
            "tarAccelMinFiles" => {
                if let Some(n) = as_i(v) {
                    s.tar_threshold = n.clamp(10, 10000);
                }
            }
            "verifyAfterTransfer" => {
                if let Some(b) = as_bool(v) {
                    s.verify = b;
                }
            }
            "verifyAlgorithm" => {
                if let Some(t) = as_str(v) {
                    s.verify_algo = match t.as_str() {
                        "SHA256" => 1,
                        "SHA512" => 2,
                        "SM3" => 3,
                        "Auto" => 0,
                        _ => s.verify_algo,
                    };
                }
            }
            "preserveMtimeEnabled" => {
                if let Some(b) = as_bool(v) {
                    s.preserve_mtime = b;
                }
            }
            _ => {}
        }
    }
}

/// 拆 `"key": value`（行首尾空白容忍）。
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

/// 字符串值去引号（反转义常见四类；C# 写出值只含这些）。
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

fn as_bool(v: &str) -> Option<bool> {
    match v.trim() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}
