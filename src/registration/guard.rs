//! 外部 URL 启动守卫（C# UrlGuard 对应物）。
//!
//! 远程通道（公告/购买/更新链接）下发的 URL 一律先过白名单（仅 http/https
//! 绝对地址）才允许经系统启动器打开，防止被诱导拉起 file:/自定义协议等危险
//! 目标。入口（apply_remote_state）过滤与消费点拦截双重防御。
//!
//! Rust 侧以手工规则近似 .NET `Uri.TryCreate(Absolute)` 的关键判定：
//! scheme 恰为 http/https（忽略大小写）+ 主机段非空 + 无引号/空白/尖括号/
//! 反斜杠/控制字符（.NET Uri 对这些字符直接解析失败）。

/// URL 是否为允许启动的 Web 地址（http/https 绝对地址）。
pub(crate) fn is_safe_web_url(url: &str) -> bool {
    let Some(i) = url.find("://") else {
        return false;
    };
    let (scheme, rest) = (&url[..i], &url[i + 3..]);
    if !(scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https")) {
        return false;
    }
    // 主机段非空（"http://" / "http:///x" 均非绝对地址）
    let host = match rest.find('/') {
        Some(j) => &rest[..j],
        None => rest,
    };
    if host.is_empty() {
        return false;
    }
    // .NET Uri 解析必拒的字符：引号/空白/尖括号/反斜杠/控制字符
    !url.bytes()
        .any(|b| b < 0x21 || b == b'"' || b == b'\'' || b == b'<' || b == b'>' || b == b'\\')
}

/// 校验并用系统默认浏览器打开 URL；非白名单时返回 false（消费点另行记日志）。
pub(crate) fn launch_url(url: &str) -> bool {
    if !is_safe_web_url(url) {
        return false;
    }
    // 平台默认打开方式：Windows explorer 直传单参数（不经 shell 二次解析，
    // 无注入面）；macOS open / Linux xdg-open 同为单参数直传。
    #[cfg(windows)]
    let cmd = "explorer";
    #[cfg(target_os = "macos")]
    let cmd = "open";
    #[cfg(all(unix, not(target_os = "macos")))]
    let cmd = "xdg-open";
    std::process::Command::new(cmd).arg(url).spawn().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_url_rules() {
        assert!(is_safe_web_url("https://ifrees.top"));
        assert!(is_safe_web_url("HTTP://example.com/a?b=1#c"));
        assert!(is_safe_web_url("https://ifrees.top/purchase"));
        // 非 http(s)
        assert!(!is_safe_web_url("file:///c:/windows/system32"));
        assert!(!is_safe_web_url("ms-settings:display"));
        assert!(!is_safe_web_url("ftp://example.com"));
        // 无 scheme / 相对地址
        assert!(!is_safe_web_url("ifrees.top"));
        assert!(!is_safe_web_url("//ifrees.top"));
        // 主机段缺失
        assert!(!is_safe_web_url("http://"));
        assert!(!is_safe_web_url("http:///path"));
        // 危险字符（.NET Uri 拒绝，浏览器启动前拦截）
        assert!(!is_safe_web_url("https://ifrees.top/\" onclick=\""));
        assert!(!is_safe_web_url("https://ifrees.top/a b"));
        assert!(!is_safe_web_url("https://ifrees.top\\evil"));
        assert!(!is_safe_web_url("https://ifrees.top/<x>"));
        // 空串
        assert!(!is_safe_web_url(""));
    }
}
