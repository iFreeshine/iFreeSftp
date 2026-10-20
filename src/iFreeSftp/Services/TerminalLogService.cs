using System.Collections.Concurrent;
using System.Linq;

namespace iFreeSftp.Services;

/// <summary>单条终端日志：时间戳与正文分离（终端框按两列渲染，续行对齐时间戳之后）。
/// 注意用普通 class 而非 record —— XamlCompiler 对 record 的 x:DataType 绑定有兼容问题。</summary>
public sealed class TerminalLogEntry
{
    public TerminalLogEntry(string time, string text)
    {
        Time = time;
        Text = text;
    }

    /// <summary>时间戳（HH:mm:ss）。</summary>
    public string Time { get; }

    /// <summary>日志正文。</summary>
    public string Text { get; }
}

/// <summary>
/// 终端日志总线：连接等后台流程把过程日志发布到这里，右下角终端容器订阅显示。
/// 静态事件解耦：发布方（面板/服务）无需引用终端控件，订阅方自行调度到 UI 线程。
/// 命令窗公用单池；scope 表示日志来源 tab，Log 内部把 scope 映射为主机标签前缀
/// （如 "[主机A (1.2.3.4)]"）拼进正文，跨 tab 日志显示 "[A ↔ B]"；
/// scope 为 null = 全局日志（不加前缀）。未注册的 scope（本地 tab）兜底显示 "本地"。
/// </summary>
public static class TerminalLogService
{
    /// <summary>新日志条目到达（可能从后台线程发出，订阅方需调度到 UI 线程）。</summary>
    public static event Action<TerminalLogEntry>? LineAppended;

    /// <summary>忙状态变化（连接进行中时终端右上角显示 loading）。</summary>
    public static event Action<bool>? BusyChanged;

    /// <summary>scope → 主机标签（"主机名称 (IP)"）注册表。连接成功建立时注册、tab 关闭时注销。</summary>
    private static readonly ConcurrentDictionary<string, string> ScopeTitles = new();

    /// <summary>注册 scope 的主机标签（连接建立时调用，重连覆盖旧值）。</summary>
    public static void RegisterScopeTitle(string scope, string title)
        => ScopeTitles[scope] = title;

    /// <summary>注销 scope（tab 关闭时调用）。</summary>
    public static void UnregisterScope(string scope)
        => ScopeTitles.TryRemove(scope, out _);

    /// <summary>追加一条带时间戳的日志。scopes 缺省 = 全局日志（无前缀）；传 scope
    /// （FileView.LogScope）= 按注册表拼主机标签前缀；跨 tab 日志传多个 scope。</summary>
    public static void Log(string message, params string?[]? scopes)
    {
        string[]? normalized = null;
        if (scopes is { Length: > 0 })
        {
            normalized = scopes.Where(s => !string.IsNullOrEmpty(s)).Select(s => s!).Distinct().ToArray();
            if (normalized.Length == 0)
            {
                normalized = null; // 全空 (如未连接的会话): 退回全局
            }
        }

        if (normalized is { Length: > 0 })
        {
            var titles = normalized.Select(s => ScopeTitles.TryGetValue(s, out var t) ? t : "本地").ToArray();
            var prefix = titles.Length == 1 ? titles[0] : string.Join(" ↔ ", titles);
            message = $"[{prefix}] {message}";
        }

        LineAppended?.Invoke(
            new TerminalLogEntry(DateTime.Now.ToString("HH:mm:ss"), message));
    }

    /// <summary>设置忙状态（开始/结束）。</summary>
    public static void SetBusy(bool busy)
        => BusyChanged?.Invoke(busy);
}
