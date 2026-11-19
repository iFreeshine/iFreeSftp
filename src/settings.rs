use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq)]
pub struct AppSettings {

    pub theme: String,

    pub language: String,

    pub connect_timeout: i32,

    pub max_retries: i32,

    pub retry_delay: i32,

    pub operation_timeout: i32,

    pub connect_open_mode: usize,

    pub concurrent_transfers: i32,

    pub editor_builtin: bool,

    pub editor_path: String,

    pub remote_delete_exec: bool,

    pub keep_alive: bool,

    pub keep_alive_interval: i32,

    pub update_check: bool,

    pub resume: bool,

    pub resume_threshold_kb: i32,

    pub tar_accel: bool,

    pub tar_threshold: i32,

    pub verify: bool,

    pub verify_algo: usize,

    pub preserve_mtime: bool,

    pub dnd_stream_batch: i32,

    pub(crate) dirty: bool,
    pub(crate) last_change: Option<Instant>,

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

pub fn load() {
    let mut s = GLOBAL.lock().unwrap();
    let got = s.get_or_insert_with(AppSettings::default);
    if let Ok(text) = std::fs::read_to_string(store_path()) {
        parse_into(got, &text);
    }
}

pub fn with<R>(f: impl FnOnce(&AppSettings) -> R) -> R {
    let mut g = GLOBAL.lock().unwrap();
    f(g.get_or_insert_with(AppSettings::default))
}

pub fn sync_from_dialog(next: AppSettings) {
    let mut g = GLOBAL.lock().unwrap();
    let cur = g.get_or_insert_with(AppSettings::default);
    if cur.dirty_snapshot_fields(&next) {
        *cur = next;
        cur.dirty = true;
        cur.last_change = Some(Instant::now());
    }
}

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

pub fn set_theme(dark: bool) {
    let mut g = GLOBAL.lock().unwrap();
    let s = g.get_or_insert_with(AppSettings::default);
    s.theme = if dark { "Dark" } else { "Light" }.to_owned();
    s.dirty = true;
    s.last_change = Some(Instant::now());
}

pub fn startup_theme_dark() -> bool {
    with(|s| crate::theme::resolve_dark(&s.theme, crate::theme::system_prefers_light()))
}

impl AppSettings {

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

pub(crate) const RETRY_BACKOFF: Duration = Duration::from_secs(2);

pub(crate) fn retry_wait(s: &AppSettings) -> Option<Duration> {
    s.last_attempt
        .and_then(|t| RETRY_BACKOFF.checked_sub(t.elapsed()))
}

pub fn maybe_flush(ctx: &egui::Context) -> bool {
    let due = {
        let mut g = GLOBAL.lock().unwrap();
        let Some(s) = g.as_mut() else {
            return false;
        };
        if !s.dirty {
            return false;
        }

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

                ctx.request_repaint_after(RETRY_BACKOFF);
            }
        }
    }
    due
}

#[derive(Default)]
pub(crate) struct SaveErrorState {
    msg: Option<String>,
    reported: bool,
}

impl SaveErrorState {

    fn set_failure(&mut self, msg: String) {
        if self.msg.as_deref() != Some(msg.as_str()) {
            self.msg = Some(msg);
            self.reported = false;
        }
    }

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

pub fn save_error() -> Option<String> {
    SAVE_ERROR.lock().unwrap().msg.clone()
}

pub fn take_save_error() -> Option<String> {
    SAVE_ERROR.lock().unwrap().take_for_log()
}

pub fn flush_now() -> std::io::Result<()> {
    let mut g = GLOBAL.lock().unwrap();
    let Some(s) = g.as_mut() else {
        return Ok(());
    };
    flush_to(s, &store_path())
}

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

    if o.ends_with(",\r\n") {
        o.truncate(o.len() - 3);
        o.push_str("\r\n");
    }
    o.push('}');
    o
}

fn parse_into(s: &mut AppSettings, text: &str) {
    for line in text.lines() {

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

    #[test]
    fn theme_key_parsed_and_resolved_like_csharp() {

        for (theme_val, dark) in [("Dark", true), ("Light", false)] {
            let text =
                format!("{{\r\n  \"theme\": \"{theme_val}\",\r\n  \"language\": \"zh-CN\"\r\n}}");
            let mut s = AppSettings::default();
            parse_into(&mut s, &text);
            assert_eq!(s.theme, theme_val);
            assert_eq!(crate::theme::resolve_dark(&s.theme, true), dark);
            assert_eq!(crate::theme::resolve_dark(&s.theme, false), dark);
        }

        let mut s = AppSettings::default();
        parse_into(
            &mut s,
            "{\r\n  \"theme\": \"\",\r\n  \"language\": \"zh-CN\"\r\n}",
        );
        assert_eq!(s.theme, "");
        assert!(!crate::theme::resolve_dark(&s.theme, true));
        assert!(crate::theme::resolve_dark(&s.theme, false));

        let mut s = AppSettings::default();
        parse_into(&mut s, "{\r\n  \"language\": \"zh-CN\"\r\n}");
        assert_eq!(s.theme, "");
        assert!(crate::theme::resolve_dark(&s.theme, false));
        assert_eq!(s.language, "zh-CN");
    }

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

        std::fs::create_dir_all(&dir).unwrap();
        s.last_attempt = None;
        assert!(flush_to(&mut s, &path).is_ok(), "恢复可写后应自动落盘成功");
        assert!(!s.dirty, "成功后才清脏标记");
        assert!(s.last_attempt.is_none() && s.last_change.is_none());

        let text = std::fs::read_to_string(&path).unwrap();
        let mut reloaded = AppSettings::default();
        parse_into(&mut reloaded, &text);
        assert_eq!(reloaded.language, "en-US", "重启后语言仍在");
        assert_eq!(reloaded.connect_timeout, 42, "重启后数值项仍在");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cr2_retry_backoff_is_independent_of_debounce() {
        let mut s = AppSettings::default();
        s.dirty = true;
        s.last_change = Some(Instant::now());
        assert!(retry_wait(&s).is_none(), "无失败尝试时不应退避");
        s.last_attempt = Some(Instant::now());
        let w = retry_wait(&s).expect("失败后应处于退避窗口");
        assert!(w > Duration::ZERO && w <= RETRY_BACKOFF);
        s.last_change = None;
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
