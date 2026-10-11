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
/// </summary>
public static class TerminalLogService
{
    /// <summary>新日志条目到达（可能从后台线程发出，订阅方需调度到 UI 线程）。</summary>
    public static event Action<TerminalLogEntry>? LineAppended;

    /// <summary>忙状态变化（连接进行中时终端右上角显示 loading）。</summary>
    public static event Action<bool>? BusyChanged;

    /// <summary>追加一条带时间戳的日志。</summary>
    public static void Log(string message)
        => LineAppended?.Invoke(new TerminalLogEntry(DateTime.Now.ToString("HH:mm:ss"), message));

    /// <summary>设置忙状态（开始/结束）。</summary>
    public static void SetBusy(bool busy)
        => BusyChanged?.Invoke(busy);
}
