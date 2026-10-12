namespace iFreeSftp.Services;

/// <summary>
/// 外部 URL 启动守卫：远程通道（公告/购买/更新链接）下发的 URL 一律先过白名单
/// （仅 http/https 绝对地址）才允许经系统启动器打开，防止被诱导拉起 file:/
/// 自定义协议等危险目标。入口（ApplyRemoteState）过滤与消费点拦截双重防御。
/// </summary>
internal static class UrlGuard
{
    /// <summary>URL 是否为允许启动的 Web 地址（http/https 绝对地址）。</summary>
    public static bool IsSafeWebUrl(string? url)
        => Uri.TryCreate(url, UriKind.Absolute, out var u)
            && u.Scheme is "https" or "http";

    /// <summary>校验并用系统启动器打开 URL；非白名单时记拦截日志并返回 false。</summary>
    public static async Task<bool> TryLaunchAsync(string? url)
    {
        if (!IsSafeWebUrl(url))
        {
            TerminalLogService.Log(LocalizationService.GetString("LogUrlBlocked"));
            return false;
        }

        return await Windows.System.Launcher.LaunchUriAsync(new Uri(url!));
    }
}
