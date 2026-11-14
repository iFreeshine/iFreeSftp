//! 外部 URL 启动守卫（C# UrlGuard 对应物）—— 装配 shim。
//!
//! 白名单判定决策（http/https 绝对地址 + .NET Uri 解析必拒字符集）随闭源 core
//! 分发（`ifree-core/src/registration/guard.rs`）；本文件只做装配：
//! - 正式发布构建：委托 `crate::ifree_core::registration::guard`；
//! - 源码编译版：钉子仅放行官网域名——远程下发的第三方链接（购买页等）在
//!   源码编译版不打开（本形态无激活能力，购买入口无意义），官网/更新检查
//!   链接保持可用；安全语义（仅 Web 地址）不弱化。
//!
//! 平台启动器留在本侧（执行不属于决策）：Windows explorer 直传单参数（不经
//! shell 二次解析，无注入面）；macOS open / Linux xdg-open 同为单参数直传。

/// URL 是否为允许启动的 Web 地址（http/https 绝对地址）。
pub(crate) fn is_safe_web_url(url: &str) -> bool {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::registration::guard::is_safe_web_url(url)
    }
    #[cfg(not(feature = "official"))]
    {
        url.starts_with("https://ifrees.top") || url.starts_with("http://ifrees.top")
    }
}

/// 校验并用系统默认浏览器打开 URL；非白名单时返回 false（消费点另行记日志）。
pub(crate) fn launch_url(url: &str) -> bool {
    if !is_safe_web_url(url) {
        return false;
    }
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    std::process::Command::new(cmd).arg(url).spawn().is_ok()
}
