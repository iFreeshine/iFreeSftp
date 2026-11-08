//! 注册服务状态机（C# RegistrationService 逐项对齐）。
//!
//! - RegState 合并 C# `Current`（RegistrationState record）与各静态属性，
//!   Default() 初始值与 C# 字段初始化器一致；
//! - activate：Trim → is_full/验签 → 落盘 → 内存态 → 后台上报（落盘失败 →
//!   SaveFailed「激活未完成」—— C# Activate / TryActivateCore / ReportActivationAsync）；
//! - refresh_remote：24h 节流读缓存（命中同步应用不联网），miss/force 走
//!   后台线程 GET /v1/state（10s 超时 + UA "iFreeSftp"），结果经事件队列
//!   通知 UI（C# await 返回 bool，egui 立即模式以事件 + repaint 等价表达）；
//! - apply_json_locked 逐块镜像 C# ApplyRemoteState 的容错语义：
//!   属性缺失 → 跳过该属性；容器非 object / 值类型不符 → 整体中止
//!   （C# TryGetProperty/GetBoolean/GetString 抛异常被 try/catch 吞掉，
//!   此前已赋值保留）；revoked 要求 Array（否则跳过不中止）；
//! - is_newer_version = Models.VersionCompare.IsNewer（数字逐段，补 0）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use crate::registration::core::{self, ErrorKind, RegistrationCore, VerifyResult};
use crate::registration::machine;
use crate::registration::store::{self, CredentialStore};

/// 注册服务端点（C# RegistrationService.EndpointBase；Cloudflare Worker 自定义域，
/// tools/worker/reg-worker.js；不可达时远程调用静默失败，激活/验签为离线不受影响）。
const ENDPOINT_BASE: &str = "https://reg.ifrees.top";

/// 本地版本（C# Assembly.GetName().Version.ToString(3)；Cargo.toml version 恰三段）。
const LOCAL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// 缓存节流窗口（C# TimeSpan.FromHours(24)）。
const CACHE_TTL_SECS: u64 = 24 * 3600;

/// 注册状态快照（C# Current record + 各静态属性合并；Clone 供 UI 每帧读）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RegState {
    // ── C# Current（RegistrationState record）──
    pub registered: bool,
    pub revoked: bool,
    pub user_name: String,
    /// u32 秒（UTC）
    pub issued_at: u32,
    /// u32 秒（UTC）；0 = 永久
    pub expires_at: u32,
    pub tier: String,
    // ── C# 静态属性 ──
    /// 远程公告文本（空 = 无）。
    pub remote_notice: String,
    /// 勋章总开关（远程可关；未拉到远程数据前默认开）。
    pub badge_enabled: bool,
    /// 远程勋章文字（空 = 客户端用本地默认文案）。
    pub badge_text: String,
    /// 购买页链接双渠道（Worker 环境变量下发；空 = UI 隐藏对应按钮）。
    pub purchase_cn: String,
    pub purchase_intl: String,
    /// 版本探测：远程下发了更高版本号（UI 还需看设置 update_check 开关）。
    pub update_available: bool,
    /// 远程下发的最新版本号（空 = 无信息）。
    pub latest_version: String,
    /// "有新版本"徽标跳转页（远程未下发时用官网默认）。
    pub update_url: String,
}

impl Default for RegState {
    fn default() -> Self {
        Self {
            registered: false,
            revoked: false,
            user_name: String::new(),
            issued_at: 0,
            expires_at: 0,
            tier: String::new(),
            remote_notice: String::new(),
            badge_enabled: true,
            badge_text: String::new(),
            purchase_cn: String::new(),
            purchase_intl: String::new(),
            update_available: false,
            latest_version: String::new(),
            update_url: "https://ifrees.top".to_owned(),
        }
    }
}

/// 异步远程刷新结果事件（C# RefreshRemoteStateAsync 的 await 返回值等价物；
/// UI 每帧 take_events 消费）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RegEvent {
    /// 远程刷新完成：true = 拿到可用状态（含缓存命中），false = 网络失败保持本地。
    RemoteRefreshDone(bool),
}

/// 激活结果（C# Activate 返回 null=成功 / 错误文案；文案映射留 UI 层按 i18n 取）。
#[derive(Debug, Clone)]
pub(crate) enum ActivateResult {
    /// 验签通过（状态已入 RegState，读 state() 展示；载荷供测试断言）。
    Success(#[allow(dead_code)] VerifyResult),
    /// 源码编译版无激活能力（RegErrOfficialOnly）。
    OfficialOnly,
    /// 验签失败（UI 按 ErrorKind 映射 RegErrSignature/RegErrExpired/RegErrFormat）。
    VerifyFail(ErrorKind),
    /// 验签通过但注册码落盘失败（UI 显示「激活未完成」并保留输入框可重试）。
    SaveFailed(String),
}

/// 注册服务（C# 静态类对应物：全局单例 + 依赖注入点供测试）。
pub(crate) struct Registration {
    core: Box<dyn RegistrationCore>,
    store: Arc<dyn CredentialStore>,
    st: Arc<Mutex<RegState>>,
    events: Arc<Mutex<Vec<RegEvent>>>,
    cache_path: PathBuf,
}

static GLOBAL: OnceLock<Arc<Registration>> = OnceLock::new();

/// 启动装配（镜像 C# Load + 启动后 RefreshRemoteStateAsync；main/App 初始化调用）。
pub(crate) fn init(ctx: egui::Context) {
    let reg = Registration::new();
    reg.load();
    let _ = GLOBAL.set(reg);
    // 启动即拉远程（24h 节流内走缓存，不联网）
    global().refresh_remote(false, ctx);
}

/// 全局实例（未显式 init 时惰性装配兜底，永不 panic：读盘但不触发远程刷新）。
pub(crate) fn global() -> &'static Arc<Registration> {
    GLOBAL.get_or_init(|| {
        let reg = Registration::new();
        reg.load();
        reg
    })
}

impl Registration {
    /// 生产装配：Stub/Official 核心 + 平台凭据存储（Windows DPAPI / 非 Windows
    /// NoopStore 不落盘）+ 默认缓存路径。
    pub(crate) fn new() -> Arc<Self> {
        #[cfg(windows)]
        let store: Arc<dyn CredentialStore> = Arc::new(store::DpapiStore::default_store());
        #[cfg(not(windows))]
        let store: Arc<dyn CredentialStore> = Arc::new(store::NoopStore);
        Self::with_parts(core::make_core(), store, remote_cache_path())
    }

    /// 测试装配点（核心/存储/缓存路径均可注入）。
    pub(crate) fn with_parts(
        core: Box<dyn RegistrationCore>,
        store: Arc<dyn CredentialStore>,
        cache_path: PathBuf,
    ) -> Arc<Self> {
        Arc::new(Self {
            core,
            store,
            st: Arc::new(Mutex::new(RegState::default())),
            events: Arc::new(Mutex::new(Vec::new())),
            cache_path,
        })
    }

    /// 状态快照（UI 每帧读；C# 各静态属性直读）。
    pub(crate) fn state(&self) -> RegState {
        self.st.lock().unwrap().clone()
    }

    /// 取走累积事件（UI 每帧 poll；异步远程刷新完成通知）。
    pub(crate) fn take_events(&self) -> Vec<RegEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }

    fn push_event(&self, e: RegEvent) {
        self.events.lock().unwrap().push(e);
    }

    /// 启动加载（C# Load）：无码/解密失败 = 未注册（默认态）；有码重验签
    /// （save:false —— 文件被篡改则视为未注册，不回写）。
    pub(crate) fn load(&self) {
        if let Some(code) = self.store.load_code() {
            self.try_activate_core(&code, false);
        }
    }

    /// 激活（C# Activate）：Trim → 验签 → 存储 → 后台上报。
    pub(crate) fn activate(&self, code: &str) -> ActivateResult {
        let code = code.trim();
        let r = self.try_activate_core(code, true);
        if matches!(r, ActivateResult::Success(_)) {
            // 上报哈希用激活码原文（NoopStore 下 load 恒 None，取盘会失真）
            self.spawn_report(code);
        }
        r
    }

    /// C# TryActivateCore：验签 + 落盘 + 内存态 + StateChanged。
    fn try_activate_core(&self, code: &str, save: bool) -> ActivateResult {
        // 验签真实现随闭源核心分发；源码编译版（Stub）无激活能力
        if !self.core.is_full() {
            return ActivateResult::OfficialOnly;
        }
        let result = self.core.verify(code);
        if !result.ok {
            return ActivateResult::VerifyFail(result.error);
        }
        // 落盘成功才置内存态（C# 落盘失败抛异常 = 激活失败，同语义不静默）；
        // 失败 → SaveFailed，UI 显示「激活未完成」并保留输入框可重试。
        if save && let Err(e) = self.store.save_code(code) {
            return ActivateResult::SaveFailed(e.to_string());
        }
        {
            let mut st = self.st.lock().unwrap();
            st.registered = true;
            st.revoked = false; // 重新激活即视为申诉；吊销名单在下次远程刷新时再裁决
            st.user_name = result.user_name.clone();
            st.issued_at = result.issued_at;
            st.expires_at = result.expires_at;
            st.tier = result.tier.clone();
        }
        ActivateResult::Success(result)
    }

    /// 激活上报（C# ReportActivationAsync fire-and-forget；失败静默，软绑定仅统计）。
    fn spawn_report(&self, code: &str) {
        let (registered, user_name) = {
            let st = self.st.lock().unwrap();
            (st.registered, st.user_name.clone())
        };
        if !registered {
            return;
        }
        // 数据先收集，线程只做网络（h 取激活码原文哈希 —— C# LoadRawCode）
        let h = machine::code_hash(code);
        let body = format!(
            "{{\"h\":\"{}\",\"m\":\"{}\",\"u\":\"{}\",\"v\":\"{}\"}}",
            h,
            machine::machine_id(),
            crate::hosts::json_escape(&user_name),
            LOCAL_VERSION,
        );
        std::thread::spawn(move || {
            let _ = http()
                .post(&format!("{ENDPOINT_BASE}/v1/activate"))
                .send_string(&body);
        });
    }

    /// 远程刷新（C# RefreshRemoteStateAsync）：24h 节流读缓存；miss/force 走
    /// 后台线程网络拉取，结果经事件队列 + repaint 通知 UI。
    pub(crate) fn refresh_remote(self: &Arc<Self>, force: bool, ctx: egui::Context) {
        // 节流命中：同步应用缓存（本地 IO，毫秒级；C# await 同帧返回 true）
        if !force && self.try_apply_cache() {
            return;
        }
        let this = Arc::clone(self);
        std::thread::spawn(move || {
            let ok = this.fetch_remote();
            // 仅手动检查（force）回报结果供设置页展示；启动静默刷新不搅动
            // 设置页文案（C# 启动 `_ = RefreshRemoteStateAsync()` 返回值无人消费）
            if force {
                this.push_event(RegEvent::RemoteRefreshDone(ok));
            }
            ctx.request_repaint();
        });
    }

    /// GET /v1/state → 应用 → 写缓存（C# 网络分支；任何失败 false 保持本地状态）。
    fn fetch_remote(&self) -> bool {
        let resp = match http().get(&format!("{ENDPOINT_BASE}/v1/state")).call() {
            Ok(r) => r,
            Err(_) => return false,
        };
        let json = match resp.into_string() {
            Ok(s) => s,
            Err(_) => return false,
        };
        self.apply_json_str(&json);
        self.write_cache(&json);
        true
    }

    /// 节流判定 + 缓存应用（C# 缓存分支）：缓存新于 24h → 应用并 true（不联网）。
    fn try_apply_cache(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(&self.cache_path) else {
            return false;
        };
        // parse 失败按 0（unix epoch）→ age 必超窗 → miss（C# MinValue 同效）
        let ts = parse_cached_time(&text);
        if unix_now().saturating_sub(ts) < CACHE_TTL_SECS {
            let Some((_, json)) = text.split_once('\n') else {
                return false;
            };
            self.apply_json_str(json);
            return true;
        }
        false
    }

    /// 应用远程状态文本（C# ApplyRemoteState 入口：解析失败整体吞掉无变化）。
    fn apply_json_str(&self, json: &str) {
        let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
            return;
        };
        let mut st = self.st.lock().unwrap();
        // Err = C# 异常被 catch 吞掉：中止且保留此前赋值
        let _ = self.apply_json_locked(&mut st, &root);
    }

    /// 逐块镜像 C# ApplyRemoteState：属性缺失跳过；容器非 object / 值类型不符
    /// → Err 中止（调用方忽略 Err，已赋值保留 —— C# 异常 + catch 语义）。
    fn apply_json_locked(&self, st: &mut RegState, root: &serde_json::Value) -> Result<(), ()> {
        if let Some(badge) = prop(root, "badge")? {
            if let Some(e) = prop(badge, "enabled")? {
                st.badge_enabled = e.as_bool().ok_or(())?;
            }
            if let Some(t) = prop(badge, "text")? {
                st.badge_text = t.as_str().ok_or(())?.to_owned();
            }
        }

        if let Some(notice) = prop(root, "notice")? {
            if let Some(t) = prop(notice, "text")? {
                // 截断超长公告防异常数据撑爆 UI/内存（按 UTF-16 code unit 计数 —— C# string.Length）
                st.remote_notice = truncate_utf16(t.as_str().ok_or(())?, 2000);
            }
        }

        if let Some(purchase) = prop(root, "purchase")? {
            if let Some(v) = prop(purchase, "cn")? {
                st.purchase_cn = v.as_str().ok_or(())?.to_owned();
            }
            if let Some(v) = prop(purchase, "intl")? {
                st.purchase_intl = v.as_str().ok_or(())?.to_owned();
            }
        }

        if let Some(update) = prop(root, "update")? {
            if let Some(v) = prop(update, "latestVersion")? {
                st.latest_version = v.as_str().ok_or(())?.to_owned();
            }
            if let Some(v) = prop(update, "url")? {
                let u = v.as_str().ok_or(())?;
                // 入口白名单：仅接受 http/https，非法值维持默认官网（消费点另有 UrlGuard 拦截）
                if crate::registration::guard::is_safe_web_url(u) {
                    st.update_url = u.to_owned();
                }
            }
            // C# 在 update 块内无条件重算（latestVersion/url 是否存在不影响）
            st.update_available = is_newer_version(&st.latest_version, LOCAL_VERSION);
        }

        if let Some(revoked) = prop(root, "revoked")? {
            // 要求 Array，否则跳过不中止（C# ValueKind 检查短路）
            if let Some(items) = revoked.as_array() {
                let mine = machine::code_hash(&self.store.load_code().unwrap_or_default());
                for item in items {
                    // 元素非字符串 → C# GetString() 抛 → 中止（吊销标记保留既有值）
                    if item.as_str().ok_or(())? == mine {
                        st.revoked = true;
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    /// 写缓存 `"{unix秒}\n{json}"`（C# WriteAllText）。
    /// 过滤方案（防篡改）：缓存只存时间戳与公告/勋章等展示块，purchase.* 与
    /// update.url 不落盘 —— 篡改者只能删信息不能注信息（URL 消费点另有
    /// UrlGuard 白名单兜底）；缓存命中会话这些字段维持默认（apply 缺块跳过）。
    fn write_cache(&self, json: &str) {
        let Ok(mut v) = serde_json::from_str::<serde_json::Value>(json) else {
            return; // 解析失败不写缓存（下轮 miss 重拉）
        };
        if let Some(o) = v.as_object_mut() {
            o.remove("purchase");
            if let Some(update) = o.get_mut("update").and_then(|u| u.as_object_mut()) {
                update.remove("url");
            }
        }
        if let Some(dir) = self.cache_path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text = format!("{}\n{v}", unix_now());
        let _ = crate::app_paths::write_atomic(&self.cache_path, text.as_bytes());
    }
}

/// C# TryGetProperty 等价：Ok(None)=属性缺失（跳过）；Err=容器非 object
/// （C# 对非 object 容器 TryGetProperty 抛 InvalidOperationException → 中止）。
fn prop<'a>(v: &'a serde_json::Value, key: &str) -> Result<Option<&'a serde_json::Value>, ()> {
    match v.as_object() {
        Some(m) => Ok(m.get(key)),
        None => Err(()),
    }
}

/// 公告截断（max 按 UTF-16 code unit 计 —— C# string.Length / [..2000] 语义）。
fn truncate_utf16(s: &str, max: usize) -> String {
    let units: Vec<u16> = s.encode_utf16().collect();
    if units.len() <= max {
        return s.to_owned();
    }
    String::from_utf16_lossy(&units[..max])
}

/// 数字逐段版本比较（C# Models.VersionCompare：Split('.') 去空段，段内
/// int.TryParse 失败记 0，补 0 逐段比，首个不等段 remote > local 才 true）。
/// i32 对齐 C# int 溢出语义（超长数字段解析失败记 0）。
pub(crate) fn is_newer_version(remote: &str, local: &str) -> bool {
    let parse = |v: &str| -> Vec<i32> {
        v.split('.')
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<i32>().unwrap_or(0))
            .collect()
    };
    let (rv, lv) = (parse(remote), parse(local));
    for i in 0..rv.len().max(lv.len()) {
        let a = rv.get(i).copied().unwrap_or(0);
        let b = lv.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

/// C# ParseCachedTime：前缀（首个 '\n' 之前）须存在且非空才解析；失败 → 0
/// （= unix epoch，age 必超窗 → miss，与 C# DateTimeOffset.MinValue 同效）。
fn parse_cached_time(cached: &str) -> u64 {
    match cached.split_once('\n') {
        Some((pre, _)) if !pre.is_empty() => pre.parse::<u64>().unwrap_or(0),
        _ => 0,
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// remote.json 缓存路径（C# RemoteCachePath：DataDir 同目录 —— app_paths::data_dir；
/// MSIX 分离目录 Rust 版不做，与 hosts/settings 一致）。
fn remote_cache_path() -> PathBuf {
    crate::app_paths::data_dir().join("remote.json")
}

/// 共享 HTTP 客户端（C# static HttpClient：10s 超时 + UA "iFreeSftp"）。
fn http() -> &'static ureq::Agent {
    static HTTP: OnceLock<ureq::Agent> = OnceLock::new();
    HTTP.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(10))
            .user_agent("iFreeSftp")
            .build()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str, ext: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("ifree-reg-svc-{tag}.{ext}"));
        let _ = std::fs::remove_file(&p);
        p
    }

    /// 测试实例：Stub 核心（源码编译版语义）+ temp DPAPI 存储 + temp 缓存。
    fn test_reg(tag: &str) -> Arc<Registration> {
        Registration::with_parts(
            core::make_core(),
            Arc::new(store::DpapiStore::at_path(temp_path(tag, "dat"))),
            temp_path(tag, "cache"),
        )
    }

    #[test]
    fn default_state_matches_csharp_initializers() {
        let st = RegState::default();
        assert!(!st.registered && !st.revoked);
        assert_eq!(st.user_name, "");
        assert_eq!((st.issued_at, st.expires_at), (0, 0));
        assert_eq!(st.remote_notice, "");
        assert!(st.badge_enabled); // C# BadgeEnabled = true
        assert_eq!(st.badge_text, "");
        assert_eq!(
            (st.purchase_cn.as_str(), st.purchase_intl.as_str()),
            ("", "")
        );
        assert!(!st.update_available);
        assert_eq!(st.latest_version, "");
        assert_eq!(st.update_url, "https://ifrees.top"); // C# UpdateUrl 默认官网
    }

    #[test]
    fn version_compare_csharp_semantics() {
        // 数字逐段，remote > local 才 true
        assert!(is_newer_version("0.9.1", "0.9.0"));
        assert!(is_newer_version("0.10.0", "0.9.9"));
        assert!(is_newer_version("1.0", "0.99.99"));
        assert!(is_newer_version("1.0.0.1", "1.0.0"));
        // 相等 / 更旧
        assert!(!is_newer_version("0.9.0", "0.9.0"));
        assert!(!is_newer_version("0.9.0", "0.9.1"));
        // 补 0 等长（"0.9" ≡ "0.9.0"）
        assert!(!is_newer_version("0.9", "0.9.0"));
        // 数值比较非字典序
        assert!(!is_newer_version("2", "10"));
        // 非数字段记 0（C# int.TryParse 失败 → 0）
        assert!(!is_newer_version("abc", "0.1")); // [0] vs [0,1]
        assert!(is_newer_version("1.0", "abc"));
        // 去空段（C# RemoveEmptyEntries）
        assert!(!is_newer_version("0..9", "0.9"));
        // 空串
        assert!(!is_newer_version("", ""));
    }

    #[test]
    fn apply_full_json_all_fields() {
        let reg = test_reg("full");
        reg.apply_json_str(
            r#"{
                "badge": {"enabled": false, "text": "支持者"},
                "notice": {"text": "公告内容"},
                "purchase": {"cn": "https://afdian.net/x", "intl": "https://ko-fi.com/x"},
                "update": {"latestVersion": "99.0.0", "url": "https://ifrees.top/dl"},
                "revoked": ["deadbeef"]
            }"#,
        );
        let st = reg.state();
        assert!(!st.badge_enabled);
        assert_eq!(st.badge_text, "支持者");
        assert_eq!(st.remote_notice, "公告内容");
        assert_eq!(st.purchase_cn, "https://afdian.net/x");
        assert_eq!(st.purchase_intl, "https://ko-fi.com/x");
        assert_eq!(st.latest_version, "99.0.0");
        assert_eq!(st.update_url, "https://ifrees.top/dl");
        assert!(st.update_available); // 99.0.0 > 0.9.0
        assert!(!st.revoked); // 哈希不匹配（盘上无码 → 哈希空串哈希）
    }

    #[test]
    fn apply_partial_skips_missing() {
        let reg = test_reg("partial");
        reg.apply_json_str(r#"{"badge": {"text": "只改文字"}}"#);
        let st = reg.state();
        assert!(st.badge_enabled); // enabled 缺失 → 开关保持
        assert_eq!(st.badge_text, "只改文字");
        assert_eq!(st.remote_notice, ""); // 其他块不动
        assert!(!st.update_available); // update 块缺失 → 不重算
    }

    #[test]
    fn apply_empty_and_invalid_json_noop() {
        let reg = test_reg("noop");
        reg.apply_json_str("{}");
        assert_eq!(reg.state(), RegState::default());
        // 非法 JSON：C# JsonDocument.Parse 抛 → catch 吞（无变化）
        reg.apply_json_str("not json");
        assert_eq!(reg.state(), RegState::default());
    }

    #[test]
    fn apply_bad_type_aborts_keeps_prior_assignments() {
        let reg = test_reg("abort");
        // badge 块正常 → notice.text 类型错（数字）→ 整体中止：
        // badge 已应用；purchase/update/revoked 块不应用
        reg.apply_json_str(
            r#"{
                "badge": {"enabled": false, "text": "已应用"},
                "notice": {"text": 123},
                "purchase": {"cn": "https://should-not-apply"},
                "update": {"latestVersion": "99.0.0"}
            }"#,
        );
        let st = reg.state();
        assert!(!st.badge_enabled);
        assert_eq!(st.badge_text, "已应用");
        assert_eq!(st.remote_notice, ""); // 中止点本块
        assert_eq!(st.purchase_cn, ""); // 中止点之后
        assert!(!st.update_available);
        assert_eq!(st.latest_version, "");
    }

    #[test]
    fn apply_bad_container_aborts() {
        let reg = test_reg("container");
        // badge 非.object（字符串）→ C# badge.TryGetProperty 抛 → 中止
        reg.apply_json_str(r#"{"badge": "str", "notice": {"text": "skip"}}"#);
        let st = reg.state();
        assert_eq!(st.badge_text, "");
        assert_eq!(st.remote_notice, ""); // 中止，未达 notice
    }

    #[test]
    fn apply_root_non_object_noop() {
        let reg = test_reg("root");
        // 根为标量/数组：C# root.TryGetProperty 抛 → 吞
        reg.apply_json_str("[1,2,3]");
        assert_eq!(reg.state(), RegState::default());
    }

    #[test]
    fn notice_truncates_at_2000_utf16_units() {
        let reg = test_reg("trunc");
        // 2001 个中文（BMP 内 char == UTF-16 unit）→ 截 2000
        let long = "公".repeat(2001);
        reg.apply_json_str(&format!(r#"{{"notice": {{"text": "{long}"}}}}"#));
        assert_eq!(reg.state().remote_notice.chars().count(), 2000);
        // 边界内不截
        let ok = "a".repeat(2000);
        reg.apply_json_str(&format!(r#"{{"notice": {{"text": "{ok}"}}}}"#));
        assert_eq!(reg.state().remote_notice.len(), 2000);
    }

    #[test]
    fn update_url_whitelist_keeps_default() {
        let reg = test_reg("urlwl");
        // url 非白名单 → 不覆盖（默认官网保持）；update_available 照算
        reg.apply_json_str(
            r#"{"update": {"latestVersion": "99.0.0", "url": "javascript:alert(1)"}}"#,
        );
        let st = reg.state();
        assert_eq!(st.update_url, "https://ifrees.top");
        assert!(st.update_available);
        // file: 协议同理
        reg.apply_json_str(r#"{"update": {"url": "file:///c:/x"}}"#);
        assert_eq!(reg.state().update_url, "https://ifrees.top");
    }

    #[test]
    fn update_url_bad_type_aborts_before_available() {
        let reg = test_reg("urltype");
        // latestVersion 已应用，url 类型错 → 中止 → update_available 不重算
        reg.apply_json_str(r#"{"update": {"latestVersion": "1.2.3", "url": 42}}"#);
        let st = reg.state();
        assert_eq!(st.latest_version, "1.2.3");
        assert!(!st.update_available);
    }

    #[test]
    fn revoked_array_matching_marks_state() {
        // save_code 走真 DPAPI protect：与 DPAPI 失败桩用例互斥（并行 flake 根治）
        let _g = crate::hosts::TEST_DPAPI_LOCK.lock().unwrap();
        let reg = test_reg("revoked");
        // 盘上存码 → 哈希入黑名单 → 标记吊销
        reg.store.save_code("IFREE1.TEST.CODE").unwrap();
        let mine = machine::code_hash("IFREE1.TEST.CODE");
        reg.apply_json_str(&format!(r#"{{"revoked": ["other", "{mine}"]}}"#));
        assert!(reg.state().revoked);
        // 不匹配 → 不标
        let reg2 = test_reg("revoked2");
        reg2.store.save_code("IFREE1.TEST.CODE").unwrap();
        reg2.apply_json_str(r#"{"revoked": ["other"]}"#);
        assert!(!reg2.state().revoked);
    }

    #[test]
    fn revoked_non_array_skipped_without_abort() {
        let reg = test_reg("revskip");
        // revoked 为字符串：跳过该块不中止（C# ValueKind 短路）
        reg.apply_json_str(r#"{"revoked": "oops", "badge": {"enabled": false}}"#);
        assert!(!reg.state().revoked);
        assert!(!reg.state().badge_enabled); // revoked 之后块照常应用
    }

    #[test]
    fn revoked_non_string_element_aborts() {
        let reg = test_reg("revel");
        // 元素为数字：C# GetString() 抛 → 中止（revoked 未标）
        reg.store.save_code("IFREE1.TEST.CODE").unwrap();
        let mine = machine::code_hash("IFREE1.TEST.CODE");
        reg.apply_json_str(&format!(
            r#"{{"badge": {{"enabled": false}}, "revoked": [123, "{mine}"]}}"#
        ));
        let st = reg.state();
        assert!(!st.revoked); // 中止于首个元素
        assert!(!st.badge_enabled); // 之前块已应用
    }

    #[test]
    fn parse_cached_time_rules() {
        // 正常前缀
        assert_eq!(parse_cached_time("1700000000\n{}"), 1_700_000_000);
        // 无换行 / 空前缀 / 非数字前缀 → 0（必 miss）
        assert_eq!(parse_cached_time("1700000000"), 0);
        assert_eq!(parse_cached_time("\n{}"), 0);
        assert_eq!(parse_cached_time("abc\n{}"), 0);
        assert_eq!(parse_cached_time(""), 0);
        // 负数前缀：u64 解析失败 → 0（C# 负时间戳同样必 miss）
        assert_eq!(parse_cached_time("-5\n{}"), 0);
    }

    #[test]
    fn cache_throttle_hit_applies_without_network() {
        let reg = test_reg("throttle");
        // 新缓存 → 节流命中 → 应用缓存内容（不联网）
        reg.write_cache(r#"{"badge": {"enabled": false}}"#);
        assert!(reg.try_apply_cache());
        assert!(!reg.state().badge_enabled);
        // 旧时间戳 → miss
        let _ = std::fs::write(
            &reg.cache_path,
            format!("1\n{}", r#"{"badge": {"enabled": false}}"#),
        );
        assert!(!reg.try_apply_cache());
        // 坏前缀 → miss
        let _ = std::fs::write(&reg.cache_path, "garbage-no-newline");
        assert!(!reg.try_apply_cache());
        // 缓存文件缺失 → miss
        let reg2 = test_reg("throttle2");
        assert!(!reg2.try_apply_cache());
    }

    #[test]
    fn cache_roundtrip_format_matches_csharp() {
        let reg = test_reg("cachert");
        reg.write_cache(r#"{"ok":true}"#);
        let text = std::fs::read_to_string(&reg.cache_path).unwrap();
        // 格式 "{unix}\n{json}"（C# ToUnixTimeSeconds + "\n" + json）
        let (pre, json) = text.split_once('\n').unwrap();
        assert!(pre.parse::<u64>().is_ok());
        assert_eq!(json, r#"{"ok":true}"#);
    }

    /// 源码编译版判据：激活被拒（OfficialOnly），存储不写入。
    /// （正式发布构建同输入走真验签 → Format，属另一条路径，不适用本断言。）
    #[cfg(not(feature = "official"))]
    #[test]
    fn activate_official_only_on_source_build() {
        let reg = test_reg("activate");
        // Stub 核心：激活被拒（OfficialOnly），存储不写入
        match reg.activate(" IFREE1.x.y ") {
            ActivateResult::OfficialOnly => {}
            _ => panic!("源码编译版应返回 OfficialOnly"),
        }
        assert!(reg.store.load_code().is_none());
        assert!(!reg.state().registered);
    }

    #[test]
    fn load_missing_file_stays_default() {
        let reg = test_reg("load");
        reg.load();
        assert_eq!(reg.state(), RegState::default());
    }

    /// Full 核心替身（is_full=true + 验签恒过）：绕开 feature 限制测激活链路。
    struct FullCore;

    impl RegistrationCore for FullCore {
        fn is_full(&self) -> bool {
            true
        }
        fn verify(&self, _code: &str) -> VerifyResult {
            VerifyResult {
                ok: true,
                error: ErrorKind::None,
                user_name: "tester".to_owned(),
                issued_at: 0,
                expires_at: 0,
                tier: "pro".to_owned(),
            }
        }
    }

    /// 落盘失败替身：save 恒 Err（模拟磁盘满/权限拒绝）。
    struct FailStore;

    impl CredentialStore for FailStore {
        fn load_code(&self) -> Option<String> {
            None
        }
        fn save_code(&self, _code: &str) -> std::io::Result<()> {
            Err(std::io::Error::other("disk full"))
        }
    }

    #[test]
    fn activate_save_failure_returns_savefailed_without_state() {
        let reg = Registration::with_parts(
            Box::new(FullCore),
            Arc::new(FailStore),
            temp_path("savefail", "cache"),
        );
        // 验签通过但落盘失败 → SaveFailed（C# 异常上抛 = 激活失败，同语义）
        match reg.activate(" IFREE1.x.y ") {
            ActivateResult::SaveFailed(e) => assert!(e.contains("disk full")),
            _ => panic!("落盘失败应返回 SaveFailed"),
        }
        // 内存态未置：本会话与重启后均按未注册
        assert!(!reg.state().registered);
    }

    /// protect 失败桩调 activate（Windows 置 TEST_DPAPI_FAIL 强制 DPAPI 失败；
    /// 非 Windows protect 天然 Err）：断言 SaveFailed 且 registration.dat
    /// 不存在 / 不含注册码明文。
    #[test]
    fn activate_protect_failure_leaves_no_plaintext_dat() {
        #[cfg(windows)]
        let _g = crate::hosts::TEST_DPAPI_LOCK.lock().unwrap();
        #[cfg(windows)]
        crate::hosts::TEST_DPAPI_FAIL.store(true, std::sync::atomic::Ordering::Relaxed);
        let dat = temp_path("protectfail", "dat");
        let reg = Registration::with_parts(
            Box::new(FullCore),
            Arc::new(store::DpapiStore::at_path(dat.clone())),
            temp_path("protectfail", "cache"),
        );
        let r = reg.activate("IFREE1.aaa.bbb");
        #[cfg(windows)]
        crate::hosts::TEST_DPAPI_FAIL.store(false, std::sync::atomic::Ordering::Relaxed);
        let is_save_failed = matches!(r, ActivateResult::SaveFailed(_));
        assert!(is_save_failed, "应返回 SaveFailed，实际 {r:?}");
        // registration.dat 不存在；若存在（防御性）也不得含注册码明文
        assert!(
            !dat.exists() || !std::fs::read_to_string(&dat).unwrap().contains("IFREE1."),
            "注册码明文泄漏进 registration.dat"
        );
    }

    #[test]
    fn activate_success_with_full_core_sets_state() {
        // 真 DPAPI 往返：与 TEST_DPAPI_FAIL 桩互斥（进程级全局，防并行串扰；
        // 同 registration/store.rs:101 roundtrip 用例的锁约定）
        let _g = crate::hosts::TEST_DPAPI_LOCK.lock().unwrap();
        let reg = Registration::with_parts(
            Box::new(FullCore),
            Arc::new(store::DpapiStore::at_path(temp_path("okcore", "dat"))),
            temp_path("okcore", "cache"),
        );
        assert!(matches!(
            reg.activate(" IFREE1.x.y "),
            ActivateResult::Success(_)
        ));
        assert!(reg.state().registered);
    }

    #[test]
    fn cache_filters_purchase_and_update_url() {
        let reg = test_reg("cachefilter");
        reg.write_cache(
            r#"{"badge":{"enabled":false},"purchase":{"cn":"https://cn.example","intl":"https://intl.example"},"update":{"latestVersion":"99.0.0","url":"https://dl.example"},"notice":{"text":"hi"}}"#,
        );
        let text = std::fs::read_to_string(&reg.cache_path).unwrap();
        let (_, json) = text.split_once('\n').unwrap();
        // 展示块留存；purchase.* 与 update.url 被过滤；latestVersion 留存
        assert!(json.contains("badge") && json.contains("notice"));
        assert!(!json.contains("purchase"));
        assert!(!json.contains("url"));
        assert!(json.contains("latestVersion"));
        // 缓存命中会话：公告/勋章照应用，被过滤字段维持默认
        let reg2 = test_reg("cachefilter2");
        let _ = std::fs::copy(&reg.cache_path, &reg2.cache_path);
        assert!(reg2.try_apply_cache());
        assert!(!reg2.state().badge_enabled);
        assert_eq!(reg2.state().remote_notice, "hi");
        assert_eq!(reg2.state().purchase_cn, "");
        assert_eq!(reg2.state().update_url, "https://ifrees.top");
    }
}
