namespace iFreeSftp.Services;

// UrlGuard 链接编译所需的最小垫片: 真实实现依赖 WinUI 资源加载与终端 UI,
// 测试环境提供空实现即可 (被测路径仅要求调用不抛异常)。
internal static class LocalizationService
{
    public static string GetString(string key) => key;
}

internal static class TerminalLogService
{
    public static void Log(string message)
    {
    }
}
