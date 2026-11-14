//! 应用设置存储（T6 设置持久化）：对齐 C# AppSettingsService。
//!
//! - 存储路径 应用数据目录\settings.json（app_paths::data_dir；Windows 与 C#
//!   共用同一存档；缺失回退 exe 同目录）；
//! - 手写行式 JSON（R5 无 serde；C# WriteIndented 每属性一行，行解析互读兼容）；
//! - 500ms 防抖合并落盘（egui 无常驻定时器：dirty + 末次变更时刻，root() 每帧
//!   `maybe_flush`，未到期预约重绘——对齐 C# Timer 防抖语义）；CR-2：落盘失败
//!   **保留脏标记**并按独立退避时刻自动重试，失败态持续可读至成功落盘清零；
//! - 主题键随本存档持久化（T19-1：与 C# AppSettingsService.Theme 互认，
//!   空 = 跟随系统；启动解析与标题栏/设置面板显式写入见 theme.rs / main.rs）；
//!   语言随本存档持久化（标题栏语言钮 / 设置弹窗下拉共用 `toggle_language` /
//!   弹窗整包回写）。

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// C# 各字段默认值与钳制范围（AppSettingsService.cs）。
#[derive(Clone, PartialEq)]
pub struct AppSettings {
    /// "Dark" | "Light" | ""（T19-1：与 C# AppSettingsService.Theme 互认——
    /// 空/未知 = 跟随系统（注册表探测，theme::system_prefers_light）；显式值
    /// 由标题栏主题钮 / 设置弹窗写入，双端共用同一 settings.json）。
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
    /// 并发传输数（生效路数，钳 1-3，默认 2）：固定 3 条 lane（MAX_LANES），
    /// 前 N 路领任务，超出路数的 lane 空转待命（transfer.rs lane 并发模型）。
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
    /// tar 加速开关，默认 false（纯 Rust 内嵌打包/解包 + gzip，失败回退逐文件）。
    pub tar_accel: bool,
    /// tar 生效最小文件数 10-10000，默认 50。
    pub tar_threshold: i32,
    /// 传输后校验，默认 false（已实现：exec 通道比对哈希，命令通道不可用跳过）。
    pub verify: bool,
    /// 校验算法 Auto|SHA256|SHA512|SM3（下标 0-3）。
    pub verify_algo: usize,
    /// 保留修改时间（C# preserveMtimeEnabled，默认开启）：上传成功后远端
    /// SETSTAT mtime、下载成功后本地 filetime 设 mtime（transfer.rs 结算点
    /// 消费）；tar 加速路径经 tar 头天然保留。
    pub preserve_mtime: bool,
    /// 流式落位阈值（T20-H 内定值，不进设置面板，手编档可调）：万级拖拽
    /// 批次冲突预检 StatBatch 流式回流，每回流满 N 个无冲突条目即入队一批
    /// （队列逐批增长，估算与传输并行）。1-65535，默认 256。
    pub dnd_stream_batch: i32,
    // 内部：防抖状态（不入盘；crate 内可构造——弹窗整包回写用）。
    pub(crate) dirty: bool,
    pub(crate) last_change: Option<Instant>,
    /// CR-2：末次**落盘尝试**时刻（失败后退避重试用）。与 `last_change` 的
    /// 500ms 防抖「末次变更」互不干扰：拿防抖时刻当重试计时会破坏防抖语义，
    /// 且失败后 UI 空闲不重绘时永远醒不过来。成功落盘后清空。
    pub(crate) last_attempt: Option<Instant>,
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
            dnd_stream_batch: 256,
            dirty: false,
            last_change: None,
            last_attempt: None,
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

/// 主题显式落盘（标题栏主题钮，T19-1）：写 "Dark"/"Light" 置脏防抖——对齐
/// C# ThemeService.Toggle :59 即时落盘语义（egui 视觉即时，落盘走防抖）。
/// 空 = 跟随系统仅存在于从未配置状态：一旦显式切换即覆盖为显式值（C# 同）。
pub fn set_theme(dark: bool) {
    let mut g = GLOBAL.lock().unwrap();
    let s = g.get_or_insert_with(AppSettings::default);
    s.theme = if dark { "Dark" } else { "Light" }.to_owned();
    s.dirty = true;
    s.last_change = Some(Instant::now());
}

/// 启动主题解析（T19-1；main.rs App::new 消费）：settings.json theme 键 +
/// 系统主题探测 → 是否暗色（C# ThemeService.Initialize 同语义，互认 C# 写值）。
pub fn startup_theme_dark() -> bool {
    with(|s| crate::theme::resolve_dark(&s.theme, crate::theme::system_prefers_light()))
}

impl AppSettings {
    /// 字段级比对（忽略 dirty/last_change）。
    fn dirty_snapshot_fields(&self, next: &AppSettings) -> bool {
        let mut a = self.clone();
        a.dirty = false;
        a.last_change = None;
        a.last_attempt = None;
        let mut b = next.clone();
        b.dirty = false;
        b.last_change = None;
        b.last_attempt = None;
        a != b
    }
}

/// 落盘失败后的重试间隔（CR-2）：设置写入低频，恢复可写后 ≤2s 自动落盘。
/// 固定退避而非指数：长期不可恢复时不让用户等成 30s+ 的静默；`maybe_flush`
/// 在等待期间主动预约重绘，egui 空闲也能醒来重试。
pub(crate) const RETRY_BACKOFF: Duration = Duration::from_secs(2);

/// 距下次允许重试还有多久（纯函数，可钉）。`None` = 可立即尝试。
pub(crate) fn retry_wait(s: &AppSettings) -> Option<Duration> {
    s.last_attempt
        .and_then(|t| RETRY_BACKOFF.checked_sub(t.elapsed()))
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
        // CR-2：失败后退避窗口用独立的 last_attempt 计时（不碰 last_change 的
        // 500ms 防抖语义）。这里必须主动预约重绘——dirty 保留后 UI 空闲时
        // egui 不重绘，没有这一句就永远等不到「恢复可写自动落盘」。
        if let Some(wait) = retry_wait(s) {
            ctx.request_repaint_after(wait);
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
            Ok(()) => SAVE_ERROR.lock().unwrap().clear(),
            Err(e) => {
                let msg = crate::i18n::tfmt("设置保存失败：{0}", &[&e]);
                SAVE_ERROR.lock().unwrap().set_failure(msg);
                // 失败后同样主动预约：退避到期时必有一帧来重试。
                ctx.request_repaint_after(RETRY_BACKOFF);
            }
        }
    }
    due
}

/// CR-2：失败态持续可读（主窗口底部全局条每帧取），终端日志仍只报一条。
/// 抽成值类型，让钉子只测纯逻辑、不碰进程级全局静态（CR-10 并行夹具铁律）。
#[derive(Default)]
pub(crate) struct SaveErrorState {
    msg: Option<String>,
    reported: bool,
}

impl SaveErrorState {
    /// 同一条失败不重置 `reported`；错误串变化（新失败原因）视为新的一条。
    fn set_failure(&mut self, msg: String) {
        if self.msg.as_deref() != Some(msg.as_str()) {
            self.msg = Some(msg);
            self.reported = false;
        }
    }
    /// 终端日志一次性上报：取过后 `msg` 仍在（持续可见不因此消失）。
    fn take_for_log(&mut self) -> Option<String> {
        if self.reported {
            return None;
        }
        let msg = self.msg.clone()?;
        self.reported = true;
        Some(msg)
    }
    fn clear(&mut self) {
        *self = Self::default();
    }
}

static SAVE_ERROR: Mutex<SaveErrorState> = Mutex::new(SaveErrorState {
    msg: None,
    reported: false,
});

/// 当前是否仍处于落盘失败态（成功落盘前恒 Some，不因上报而清空）。
pub fn save_error() -> Option<String> {
    SAVE_ERROR.lock().unwrap().msg.clone()
}

/// 取一次待上报的设置落盘错误（root() 每帧调用）：同一条失败只报一次防刷屏；
/// 成功落盘后若再失败会重新上报。
pub fn take_save_error() -> Option<String> {
    SAVE_ERROR.lock().unwrap().take_for_log()
}

/// 立即落盘挂起改动（退出前语义；保留接口对齐 C# FlushNow）。
/// CR-2：**写盘成功才清脏标记**；失败保留 dirty + 记录尝试时刻，由
/// `maybe_flush` 退避自动重试，不需用户重改一遍。
pub fn flush_now() -> std::io::Result<()> {
    let mut g = GLOBAL.lock().unwrap();
    let Some(s) = g.as_mut() else {
        return Ok(());
    };
    flush_to(s, &store_path())
}

/// 局部落盘（CR-2 可测核心）：`path` 参数化，钉子只用局部 `AppSettings`
/// + 独占临时目录，不碰 `GLOBAL`、不调 `load()`、不写用户真实 settings.json。
pub(crate) fn flush_to(s: &mut AppSettings, path: &std::path::Path) -> std::io::Result<()> {
    if !s.dirty {
        return Ok(());
    }
    let text = serialize(s);
    s.last_attempt = Some(Instant::now());
    crate::app_paths::write_atomic(path, text.as_bytes())?;
    s.dirty = false;
    s.last_change = None;
    s.last_attempt = None;
    Ok(())
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
    w("dndStreamBatch", s.dnd_stream_batch.to_string());
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
            "dndStreamBatch" => {
                if let Some(n) = as_i(v) {
                    s.dnd_stream_batch = n.clamp(1, 65535);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// T19-1 贯通用例（判据②自动部分）：C# WriteIndented 同款行式 JSON →
    /// parse_into → resolve_dark 启动认值语义（含空 = 跟随系统）。
    #[test]
    fn theme_key_parsed_and_resolved_like_csharp() {
        // C# 显式写值（ThemeService 落盘 "Dark"/"Light"）：认值与系统探测无关。
        for (theme_val, dark) in [("Dark", true), ("Light", false)] {
            let text =
                format!("{{\r\n  \"theme\": \"{theme_val}\",\r\n  \"language\": \"zh-CN\"\r\n}}");
            let mut s = AppSettings::default();
            parse_into(&mut s, &text);
            assert_eq!(s.theme, theme_val);
            assert_eq!(crate::theme::resolve_dark(&s.theme, true), dark);
            assert_eq!(crate::theme::resolve_dark(&s.theme, false), dark);
        }
        // 空 = 跟随系统（C# Theme "" → IsSystemThemeLight 注册表探测）。
        let mut s = AppSettings::default();
        parse_into(
            &mut s,
            "{\r\n  \"theme\": \"\",\r\n  \"language\": \"zh-CN\"\r\n}",
        );
        assert_eq!(s.theme, "");
        assert!(!crate::theme::resolve_dark(&s.theme, true)); // 系统亮 → 亮
        assert!(crate::theme::resolve_dark(&s.theme, false)); // 系统暗 → 暗
        // C# 旧档无 theme 键：保持默认空 = 跟随系统；language 正常入账。
        let mut s = AppSettings::default();
        parse_into(&mut s, "{\r\n  \"language\": \"zh-CN\"\r\n}");
        assert_eq!(s.theme, "");
        assert!(crate::theme::resolve_dark(&s.theme, false));
        assert_eq!(s.language, "zh-CN");
    }

    /// CR-2 夹具：pid + 进程内自增序号的独占临时目录（CR-10 铁律——
    /// cargo test 默认并行时固定名会互抢，产生 os error 5 / error 32）。
    /// 故意**不创建**父目录：write_atomic 的 File::create 直接 Err，
    /// create_dir_all 即「恢复可写」，纯文件系统行为、无需测试桩。
    fn cr2_dir(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ifreesftp-cr2-{}-{}-{}.d",
            tag,
            std::process::id(),
            seq
        ))
    }

    #[test]
    fn cr2_flush_failure_keeps_dirty_and_marks_attempt() {
        let dir = cr2_dir("fail");
        let path = dir.join("settings.json");
        let mut s = AppSettings::default();
        s.language = "en-US".into();
        s.dirty = true;
        s.last_change = Some(Instant::now());
        assert!(flush_to(&mut s, &path).is_err(), "父目录缺失应写盘失败");
        assert!(s.dirty, "写盘失败必须保留脏标记（旧码先清）");
        assert!(s.last_attempt.is_some(), "失败须记尝试时刻供退避");
        assert!(!path.exists(), "失败不得产生正式存档");
    }

    #[test]
    fn cr2_retry_after_recovery_persists_new_value() {
        let dir = cr2_dir("recover");
        let path = dir.join("settings.json");
        let mut s = AppSettings::default();
        s.language = "en-US".into();
        s.connect_timeout = 42;
        s.dirty = true;
        s.last_change = Some(Instant::now());
        assert!(flush_to(&mut s, &path).is_err());
        assert!(s.dirty, "失败后改动不得丢");
        // 只恢复可写、不重改任何设置：模拟用户什么都没做，靠自动重试落盘。
        std::fs::create_dir_all(&dir).unwrap();
        s.last_attempt = None; // 模拟 2s 退避到期
        assert!(flush_to(&mut s, &path).is_ok(), "恢复可写后应自动落盘成功");
        assert!(!s.dirty, "成功后才清脏标记");
        assert!(s.last_attempt.is_none() && s.last_change.is_none());
        // 重启读回（走现成纯函数 parse_into，不调 load()/不碰 GLOBAL）。
        let text = std::fs::read_to_string(&path).unwrap();
        let mut reloaded = AppSettings::default();
        parse_into(&mut reloaded, &text);
        assert_eq!(reloaded.language, "en-US", "重启后语言仍在");
        assert_eq!(reloaded.connect_timeout, 42, "重启后数值项仍在");
        let _ = std::fs::remove_dir_all(&dir); // 测试缓存直接删，不进回收站
    }

    #[test]
    fn cr2_retry_backoff_is_independent_of_debounce() {
        let mut s = AppSettings::default();
        s.dirty = true;
        s.last_change = Some(Instant::now()); // 防抖刚变更
        assert!(retry_wait(&s).is_none(), "无失败尝试时不应退避");
        s.last_attempt = Some(Instant::now());
        let w = retry_wait(&s).expect("失败后应处于退避窗口");
        assert!(w > Duration::ZERO && w <= RETRY_BACKOFF);
        s.last_change = None; // 防抖态变化不得影响重试计时
        assert!(retry_wait(&s).is_some());
        s.last_attempt = Instant::now().checked_sub(Duration::from_secs(3));
        assert!(retry_wait(&s).is_none(), "退避到期须允许重试");
    }

    #[test]
    fn cr2_save_error_state_persists_but_logs_once() {
        let mut e = SaveErrorState::default();
        assert!(e.msg.is_none() && e.take_for_log().is_none());
        e.set_failure("磁盘已满".into());
        assert_eq!(e.take_for_log().as_deref(), Some("磁盘已满"));
        assert!(e.take_for_log().is_none(), "同一条失败只上报一次");
        assert_eq!(e.msg.as_deref(), Some("磁盘已满"), "持续可见不因上报消失");
        e.set_failure("磁盘已满".into());
        assert!(e.take_for_log().is_none(), "重复失败不刷屏");
        e.set_failure("只读目录".into());
        assert_eq!(
            e.take_for_log().as_deref(),
            Some("只读目录"),
            "新原因重新上报"
        );
        e.clear();
        assert!(e.msg.is_none() && !e.reported, "成功落盘后失败态清空");
    }

    #[test]
    fn cr2_clean_settings_are_not_rewritten() {
        let dir = cr2_dir("clean");
        let path = dir.join("settings.json");
        let mut s = AppSettings::default();
        assert!(flush_to(&mut s, &path).is_ok());
        assert!(!path.exists(), "干净状态不得产生写盘动作");
    }
}
