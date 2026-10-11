using System.ComponentModel;
using System.Runtime.CompilerServices;
using iFreeSftp.Models;

namespace iFreeSftp.Services;

/// <summary>传输任务状态：等待 → 传输中 → 完成/失败。</summary>
public enum TransferStatus
{
    /// <summary>排队等待（已入队，尚未开始）。</summary>
    Waiting,
    /// <summary>传输中。</summary>
    Transferring,
    /// <summary>传输完成。</summary>
    Done,
    /// <summary>传输失败。</summary>
    Failed,
}

/// <summary>队列页签：队列中（等待+传输中）/ 传输失败 / 传输完成。</summary>
public enum TransferQueueTab
{
    Active,
    Failed,
    Done,
}

/// <summary>
/// 队列变更类型（细粒度事件，替代旧"任何变更都全量重建"的单一信号）：
/// Enqueued/Completed/Failed = 集合结构变化（UI 增量增删行）；Reset/Cleared =
/// 全量重填（重置重跑/清空，手动低频）；Updated = 行内属性变化（INPC 自动推送，
/// UI 无需操作，保留类型便于扩展）。
/// </summary>
public enum QueueChangeKind
{
    Enqueued,
    Completed,
    Failed,
    Updated,
    Reset,
    Cleared,
}

/// <summary>
/// 单条传输任务（队列行模型）：源/目标路径、大小与第四列状态文案。
/// 第四列 = 状态语义：等待→"等待中"，传输中→实时速度（无进度回报时"传输中"），
/// 完成→"传输完成"，失败→"传输失败"。
/// 实现 INotifyPropertyChanged：状态/速度/时刻变化只推单行属性通知（{Binding}
/// 经引擎自动封送到 UI 线程），列表结构与其余行完全不动——这是队列窗口不闪烁的关键。
/// </summary>
public sealed class TransferTask : INotifyPropertyChanged
{
    public Guid Id { get; init; } = Guid.NewGuid();

    /// <summary>源条目完整路径。</summary>
    public string Source { get; init; } = string.Empty;

    /// <summary>目标完整路径。</summary>
    public string Dest { get; init; } = string.Empty;

    /// <summary>大小展示文本（目录为占位符）。</summary>
    public string SizeText { get; init; } = string.Empty;

    private TransferStatus _status = TransferStatus.Waiting;

    public TransferStatus Status
    {
        get => _status;
        internal set
        {
            if (_status == value)
            {
                return;
            }

            _status = value;
            Raise();
            Raise(nameof(StatusText));
        }
    }

    private string _speedText = string.Empty;

    /// <summary>实时速度文本（如 "1.2 MB/s"），仅传输中且收到进度回报时非空。</summary>
    public string SpeedText
    {
        get => _speedText;
        internal set
        {
            if (_speedText == value)
            {
                return;
            }

            _speedText = value;
            Raise();
            Raise(nameof(StatusText)); // 传输中第四列 = 速度
        }
    }

    private DateTime? _finishTime;

    /// <summary>完成/失败时刻（完成或失败页签的"时间"列）。</summary>
    public DateTime? FinishTime
    {
        get => _finishTime;
        internal set
        {
            _finishTime = value;
            Raise();
            Raise(nameof(TimeText));
            Raise(nameof(StatusText)); // 完成/失败第四列 = 时刻
        }
    }

    private string _errorText = string.Empty;

    /// <summary>失败原因（异常消息；仅失败任务非空）。</summary>
    public string ErrorText
    {
        get => _errorText;
        internal set
        {
            if (_errorText == value)
            {
                return;
            }

            _errorText = value;
            Raise();
        }
    }

    // 速度统计基线（仅 TransferQueueService 维护）
    internal long LastBytes { get; set; }
    internal long LastTick { get; set; }

    /// <summary>
    /// 任务执行体（FileView 构建，接受批次连接池借还传输连接）：正常传输与"重置并
    /// 重新请求所有"共用同一份——不绑定具体连接池实例，执行时由调用方传入。
    /// </summary>
    internal Func<TransferBatchConnections, Task>? Run { get; set; }

    /// <summary>"时间"列文本（完成/失败时刻，时分秒）。</summary>
    public string TimeText => FinishTime?.ToString("HH:mm:ss") ?? string.Empty;

    /// <summary>
    /// 第四列展示文本（语言切换后由容器刷新重读）：等待→"等待中"（全局暂停时
    /// "已暂停"），传输中→实时速度（无进度回报时"传输中"），完成/失败→完成时刻
    /// （状态由页签表达）。
    /// </summary>
    public string StatusText => Status switch
    {
        TransferStatus.Transferring when SpeedText.Length > 0 => SpeedText,
        TransferStatus.Transferring => LocalizationService.GetString("QStateTransferring"),
        TransferStatus.Done or TransferStatus.Failed => TimeText,
        TransferStatus.Waiting when TransferQueueService.IsPaused => LocalizationService.GetString("QStatePaused"),
        _ => LocalizationService.GetString("QStateWaiting"),
    };

    /// <summary>
    /// 通知订阅行重读第四列（无属性值变化但视觉语义变化时用）：全局暂停标志切换、
    /// 语言切换——等待/传输中的 StatusText 随其重算。
    /// </summary>
    internal void RefreshVisual() => Raise(nameof(StatusText));

    /// <inheritdoc/>
    public event PropertyChangedEventHandler? PropertyChanged;

    /// <summary>
    /// UI 调度器（QueueContainer 构造时注入）：本模型的 PropertyChanged 经 WinRT
    /// 投影桥接到 {Binding}——非 UI 线程触发时封送 PropertyChangedEventArgs 会抛
    /// RPC_E_WRONG_THREAD 致进程崩溃（多文件上传实测），故 Raise 前强制切回 UI 线程。
    /// </summary>
    internal static Microsoft.UI.Dispatching.DispatcherQueue? UiDispatcher;

    private void Raise([CallerMemberName] string? name = null)
    {
        var dispatcher = UiDispatcher;
        if (dispatcher is not null && !dispatcher.HasThreadAccess)
        {
            dispatcher.TryEnqueue(() => Raise(name));
            return;
        }

        PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
    }
}

/// <summary>
/// 传输队列静态服务（全局单例语义）：四方向的"传输到另一侧"任务统一入队，
/// 供主面板左下 QueueContainer 按"队列中（等待+传输中）/ 传输失败 / 传输完成"
/// 页签展示。任意线程可调用。刷新机制分两层（防闪烁）：
/// ① 行内变化（状态/速度/时刻）经 <see cref="TransferTask"/> 的 INPC 直接推送到
///   所在行（{Binding} 自动封送 UI 线程），列表结构与其余行不动；
/// ② 结构变化（入队/完成转出/失败转出/清空/重置）经 <see cref="Changed"/> 广播
///   （携带变更类型与任务），UI 侧对三个页签集合做增量增删——不再整体替换视图。
/// 速度统计：SSH.NET 进度回调上报累计字节，≥400ms 窗口计算增量速度（节流）。
/// </summary>
public static class TransferQueueService
{
    private static readonly object Lock = new();
    private static readonly List<TransferTask> Tasks = new();

    /// <summary>
    /// 队列结构变化（任意线程）：kind+task 描述增量操作；Reset/Cleared 时 task 为
    /// null（UI 全量重填）。行内属性变化不走本事件（INPC 直达所在行）。
    /// </summary>
    public static event Action<QueueChangeKind, TransferTask?>? Changed;

    /// <summary>全局暂停标志（右键"暂停"置位，"继续/停止"清除；等待行显示"已暂停"）。</summary>
    public static bool IsPaused { get; private set; }

    /// <summary>入队一条传输任务（初始"等待"，执行体由调用方随后赋值 Run）。</summary>
    public static TransferTask Enqueue(string source, string dest, string sizeText)
    {
        var task = new TransferTask { Source = source, Dest = dest, SizeText = sizeText };
        lock (Lock)
        {
            Tasks.Add(task);
        }

        RaiseChanged(QueueChangeKind.Enqueued, task);
        return task;
    }

    /// <summary>标记开始传输（行内状态变化，INPC 直达所在行）。</summary>
    public static void Begin(Guid id) => SetStatus(id, TransferStatus.Transferring);

    /// <summary>标记传输完成（记录完成时刻；行移出"队列中"、追加"传输完成"尾部）。</summary>
    public static void Complete(Guid id)
    {
        TransferTask? task;
        lock (Lock)
        {
            task = FindLocked(id);
            if (task is not null && task.Status != TransferStatus.Done)
            {
                task.Status = TransferStatus.Done;
                task.FinishTime = DateTime.Now;
            }
            else
            {
                task = null;
            }
        }

        if (task is not null)
        {
            RaiseChanged(QueueChangeKind.Completed, task);
        }
    }

    /// <summary>标记传输失败（记录失败时刻与原因文本；行移出"队列中"、追加"传输失败"尾部）。</summary>
    public static void Fail(Guid id, string error)
    {
        TransferTask? task;
        lock (Lock)
        {
            task = FindLocked(id);
            if (task is not null && task.Status != TransferStatus.Failed)
            {
                task.Status = TransferStatus.Failed;
                task.FinishTime = DateTime.Now;
                task.ErrorText = error ?? string.Empty;
            }
            else
            {
                task = null;
            }
        }

        if (task is not null)
        {
            RaiseChanged(QueueChangeKind.Failed, task);
        }
    }

    /// <summary>
    /// 上报累计传输字节（SSH.NET 回调线程高频触发）。首次记录基线；≥400ms 才重算
    /// 一次速度并经 INPC 推送所在行（不触发结构事件，列表其余行不动）；字节计数
    /// 回退（中转下载→上传阶段重置）时重立基线。
    /// </summary>
    public static void Report(Guid id, long bytes)
    {
        lock (Lock)
        {
            var task = FindLocked(id);
            if (task is null || task.Status != TransferStatus.Transferring)
            {
                return;
            }

            var now = Environment.TickCount64;
            if (task.LastTick == 0)
            {
                task.LastBytes = bytes;
                task.LastTick = now;
                return;
            }

            var delta = bytes - task.LastBytes;
            if (delta < 0)
            {
                task.LastBytes = bytes;
                task.LastTick = now;
                return;
            }

            var dt = now - task.LastTick;
            if (dt < 400)
            {
                return;
            }

            task.SpeedText = FileEntry.FormatSize((long)(delta * 1000.0 / dt)) + "/s";
            task.LastBytes = bytes;
            task.LastTick = now;
        }
    }

    /// <summary>按页签取任务视图（队列中 = 等待+传输中，按入队顺序）。</summary>
    public static IReadOnlyList<TransferTask> Get(TransferQueueTab tab)
    {
        lock (Lock)
        {
            return tab switch
            {
                TransferQueueTab.Failed => Tasks.Where(t => t.Status == TransferStatus.Failed).ToList(),
                TransferQueueTab.Done => Tasks.Where(t => t.Status == TransferStatus.Done).ToList(),
                _ => Tasks.Where(t => t.Status is TransferStatus.Waiting or TransferStatus.Transferring).ToList(),
            };
        }
    }

    private static void SetStatus(Guid id, TransferStatus status)
    {
        lock (Lock)
        {
            if (FindLocked(id) is { } task)
            {
                task.Status = status;
            }
        }
    }

    /// <summary>置全局暂停标志并刷新所有等待行第四列（"等待中"行重读为"已暂停"，
    /// 行内 INPC 直达，无结构变化）。</summary>
    public static void SetPaused(bool paused)
    {
        lock (Lock)
        {
            if (IsPaused == paused)
            {
                return;
            }

            IsPaused = paused;
            foreach (var task in Tasks.Where(t => t.Status == TransferStatus.Waiting))
            {
                task.RefreshVisual();
            }
        }
    }

    /// <summary>队列概览（右键菜单按状态显隐菜单项用）。</summary>
    public static (int Total, int Waiting, int Transferring, int Done, int Failed) GetSummary()
    {
        lock (Lock)
        {
            return (
                Tasks.Count,
                Tasks.Count(t => t.Status == TransferStatus.Waiting),
                Tasks.Count(t => t.Status == TransferStatus.Transferring),
                Tasks.Count(t => t.Status == TransferStatus.Done),
                Tasks.Count(t => t.Status == TransferStatus.Failed));
        }
    }

    /// <summary>删除所有任务。正在传输的任务执行完后记账时找不到条目，静默无害。
    /// UI 收到 Cleared 后清空三个页签集合。</summary>
    public static void ClearAll()
    {
        lock (Lock)
        {
            Tasks.Clear();
        }

        RaiseChanged(QueueChangeKind.Cleared, null);
    }

    /// <summary>
    /// 停止传输：解除暂停 + 取消后续任务令牌 + 所有等待中任务立即标记失败"已停止"
    /// （正在传输的让其传完，SSH.NET 同步 API 无法安全中断）。等待行逐行转入
    /// "传输失败"页签（逐行 Failed 结构事件）。停止后菜单进入
    /// "删除所有 / 重置并重新请求所有"分支。
    /// </summary>
    public static void StopAll()
    {
        ConcurrentTransferService.RequestStop();
        List<TransferTask> stopped;
        lock (Lock)
        {
            IsPaused = false;
            stopped = Tasks.Where(t => t.Status == TransferStatus.Waiting).ToList();
            foreach (var task in stopped)
            {
                task.Status = TransferStatus.Failed;
                task.FinishTime = DateTime.Now;
                task.ErrorText = LocalizationService.GetString("QStoppedReason");
            }
        }

        foreach (var task in stopped)
        {
            RaiseChanged(QueueChangeKind.Failed, task);
        }
    }

    /// <summary>
    /// 重置并重新请求所有：全部任务（含执行体）重置回"等待"（清速度/原因/时刻），
    /// 解除暂停后整批重跑——新建批次连接池（旧批次的连接早已随批次关闭，执行体
    /// 不绑定具体池实例，借还发生在本次新池上）。fire-and-forget 调用。
    /// 重置广播 Reset 结构事件，UI 三个页签集合全量重填（手动低频操作）。
    /// </summary>
    public static async Task RerunAllAsync()
    {
        var items = new List<(Guid QueueId, Func<TransferBatchConnections, Task> Run)>();
        lock (Lock)
        {
            if (Tasks.Any(t => t.Status == TransferStatus.Transferring))
            {
                return; // 防御: 有任务正在传输时忽略重传 (正常菜单已按状态隐藏)
            }

            foreach (var task in Tasks)
            {
                if (task.Run is null)
                {
                    continue;
                }

                task.Status = TransferStatus.Waiting;
                task.SpeedText = string.Empty;
                task.ErrorText = string.Empty;
                task.FinishTime = null;
                task.LastBytes = 0;
                task.LastTick = 0;
                items.Add((task.Id, task.Run));
            }
        }

        IsPaused = false;
        RaiseChanged(QueueChangeKind.Reset, null);
        if (items.Count == 0)
        {
            return;
        }

        // 命令行面板叙述: 重传开始与结束汇总 (跳过数为 0, 复用完成文案)
        TerminalLogService.Log(string.Format(
            LocalizationService.GetString("LogQueueRerun"), items.Count));

        using var connections = new TransferBatchConnections(AppSettingsService.ConcurrentTransfers);
        var (_, failed) = await ConcurrentTransferService.RunAsync(
            items, AppSettingsService.ConcurrentTransfers, connections);

        TerminalLogService.Log(string.Format(
            LocalizationService.GetString("LogTransferDone"), items.Count - failed, 0, failed));
    }

    // 调用方必须已持有 Lock
    private static TransferTask? FindLocked(Guid id)
        => Tasks.FirstOrDefault(t => t.Id == id);

    private static void RaiseChanged(QueueChangeKind kind, TransferTask? task)
        => Changed?.Invoke(kind, task);
}
