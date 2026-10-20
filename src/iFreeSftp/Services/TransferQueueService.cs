using System.ComponentModel;
using System.Runtime.CompilerServices;
using iFreeSftp.Models;

namespace iFreeSftp.Services;

/// <summary>传输任务状态：等待 → 传输中 → （校验中）→ 完成/失败/已取消。</summary>
public enum TransferStatus
{
    /// <summary>排队等待（已入队，尚未开始）。</summary>
    Waiting,
    /// <summary>传输中。</summary>
    Transferring,
    /// <summary>传输已完成，正在校验（fire-and-forget，不占传输并发）。</summary>
    Verifying,
    /// <summary>传输完成。</summary>
    Done,
    /// <summary>传输失败。</summary>
    Failed,
    /// <summary>已取消（全局停止或单任务取消；行留"队列中"页签，可重试续传）。</summary>
    Cancelled,
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
/// Enqueued/Completed/Failed/Removed = 集合结构变化（UI 增量增删行）；Reset/Cleared =
/// 全量重填（重置重跑/清除已完结，手动低频）；Updated = 行内属性变化（INPC 自动推送，
/// UI 无需操作，保留类型便于扩展）。
/// </summary>
public enum QueueChangeKind
{
    Enqueued,
    Completed,
    Failed,
    Removed,
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

    /// <summary>文件总字节数（单文件任务入队时已知则填; 0 = 未知——目录/树/tar
    /// 任务不显示百分比，第四列维持纯速度文案）。</summary>
    public long TotalBytes { get; init; }

    /// <summary>日志归属的 tab scope 列表（入队时由发起视图填写; 跨窗格传输为
    /// 发起+对侧两个 scope, 两侧命令窗都保留上下文; 空 = 全局广播）。</summary>
    public string[] LogScopes { get; set; } = Array.Empty<string>();

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

    private bool _isResumed;

    /// <summary>本次执行走了断点续传追加分支（worker 决策后置位; 第四列显示"续传中"）。
    /// 重跑时复位重新决策——半成品大小每次执行都重新查询。</summary>
    public bool IsResumed
    {
        get => _isResumed;
        internal set
        {
            if (_isResumed == value)
            {
                return;
            }

            _isResumed = value;
            Raise();
            Raise(nameof(StatusText));
        }
    }

    /// <summary>已传输绝对偏移（续传时含断点前字节）：Report 更新, 不单独推通知——
    /// 百分比随 SpeedText 的 400ms 节流推送重算刷新。</summary>
    public long BytesDone { get; internal set; }

    private bool _isTarMode;

    /// <summary>tar 打包加速任务（入队时置位, 结构性标记, 重跑保留）:
    /// 第四列显示"tar 加速"; 失败时调用方附"整体传输不支持文件级续传"提示。</summary>
    public bool IsTarMode
    {
        get => _isTarMode;
        internal set
        {
            if (_isTarMode == value)
            {
                return;
            }

            _isTarMode = value;
            Raise();
            Raise(nameof(StatusText));
        }
    }

    private VerifyResultKind _verifyResult = VerifyResultKind.None;

    /// <summary>传输后校验结果（T2）：None = 未校验；变化时第四列文案随之重算——
    /// "已校验 / 校验跳过"追加在完成时刻后，"校验不匹配"追加在失败原因场景。</summary>
    public VerifyResultKind VerifyResult
    {
        get => _verifyResult;
        internal set
        {
            if (_verifyResult == value)
            {
                return;
            }

            _verifyResult = value;
            Raise();
            Raise(nameof(StatusText));
        }
    }

    /// <summary>单任务取消请求（volatile 保证块边界检查点可见）：行右键"取消"对
    /// 传输中任务置位，检查点命中后按"已取消"结算。重试时清零。</summary>
    internal volatile bool CancelRequested;

    // 速度统计基线（仅 TransferQueueService 维护）
    internal long LastBytes { get; set; }
    internal long LastTick { get; set; }

    /// <summary>
    /// 任务执行体（FileView 构建，接受批次连接池借还传输连接）：正常传输与"重置并
    /// 重新请求所有"共用同一份——不绑定具体连接池实例，执行时由调用方传入。
    /// </summary>
    internal Func<TransferBatchConnections, Task>? Run { get; set; }

    /// <summary>传输后校验执行体（FileView 挂接, 仅单文件 + 功能开启 + 本地↔远程方向）：
    /// 本地算杂凑 + exec 通道算远端杂凑 + 比对。null = 无校验, 结算直接完成。</summary>
    internal Func<Task<VerifyOutcome>>? VerifyHook { get; set; }

    /// <summary>"时间"列文本（完成/失败时刻，时分秒）。</summary>
    public string TimeText => FinishTime?.ToString("HH:mm:ss") ?? string.Empty;

    /// <summary>
    /// 第四列展示文本（语言切换后由容器刷新重读）：等待→"等待中"（全局暂停时
    /// "已暂停"），传输中→模式前缀 + 实时速度（无进度回报时仅模式文案: 续传中 /
    /// tar 加速 / 传输中），完成/失败→完成时刻（状态由页签表达）。
    /// </summary>
    public string StatusText => Status switch
    {
        TransferStatus.Cancelled => LocalizationService.GetString("QStateCancelled"),
        TransferStatus.Verifying => LocalizationService.GetString("QStateVerifying"),
        TransferStatus.Done when VerifyResult == VerifyResultKind.Match => VerifyTail("QVerifyVerified"),
        TransferStatus.Done when VerifyResult == VerifyResultKind.Skipped => VerifyTail("QVerifySkipped"),
        TransferStatus.Failed when VerifyResult == VerifyResultKind.Mismatch => VerifyTail("QVerifyMismatch"),
        TransferStatus.Transferring when IsResumed && SpeedText.Length > 0 =>
            ResumeLabel() + " " + SpeedText,
        TransferStatus.Transferring when IsResumed => ResumeLabel(),
        TransferStatus.Transferring when IsTarMode && SpeedText.Length > 0 =>
            LocalizationService.GetString("QStateTarMode") + " " + SpeedText,
        TransferStatus.Transferring when IsTarMode => LocalizationService.GetString("QStateTarMode"),
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

    /// <summary>续传中文案：已知总字节数时附百分比（BytesDone 为文件绝对偏移,
    /// 续传从断点起算; 钳制 0–100 防越界）。</summary>
    private string ResumeLabel()
    {
        var label = LocalizationService.GetString("QStateResuming");
        return TotalBytes > 0
            ? $"{label} {Math.Clamp(BytesDone * 100 / TotalBytes, 0, 100)}%"
            : label;
    }

    /// <summary>校验结果尾部文案：完成/失败时刻为空时仅文案，否则附时刻。</summary>
    private string VerifyTail(string key)
        => TimeText.Length == 0
            ? LocalizationService.GetString(key)
            : LocalizationService.GetString(key) + " " + TimeText;

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

    /// <summary>入队一条传输任务（初始"等待"，执行体由调用方随后赋值 Run）。
    /// totalBytes = 文件总字节数（单文件任务填, 供续传百分比; 目录级任务 0 不显示）。</summary>
    public static TransferTask Enqueue(string source, string dest, string sizeText, long totalBytes = 0)
    {
        var task = new TransferTask { Source = source, Dest = dest, SizeText = sizeText, TotalBytes = totalBytes };
        lock (Lock)
        {
            Tasks.Add(task);
        }

        RaiseChanged(QueueChangeKind.Enqueued, task);
        return task;
    }

    /// <summary>
    /// 领取任务开始传输：锁内原子判定——仅"等待中且未请求取消"的任务可领取并转
    /// "传输中"（true）；已取消/已完结/不存在（false）时调用方跳过。消除 UI 标记
    /// 取消与 worker 领取之间的竞态窗口。
    /// </summary>
    public static bool Begin(Guid id)
    {
        lock (Lock)
        {
            if (FindLocked(id) is not { } task
                || task.Status != TransferStatus.Waiting
                || task.CancelRequested)
            {
                return false;
            }

            task.Status = TransferStatus.Transferring; // 行内状态变化, INPC 直达所在行
            return true;
        }
    }

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
    /// 传输成功后的结算入口（替代直接 Complete，T2）：无校验执行体（功能关闭/
    /// 目录级任务/不支持方向）→ 立即完成；有 → 行转入"校验中"并 fire-and-forget
    /// 跑校验——worker 并发许可已在调用方 finally 释放，校验不占传输并发，
    /// 队列后续传输不被阻塞。校验中清空队列：任务移除后结算静默无害。
    /// </summary>
    public static void SettleAfterRun(Guid id)
    {
        TransferTask? completed = null;
        Func<Task<VerifyOutcome>>? hook = null;
        lock (Lock)
        {
            if (FindLocked(id) is { } task
                && task.Status is TransferStatus.Transferring or TransferStatus.Verifying)
            {
                hook = task.VerifyHook;
                if (hook is null)
                {
                    task.Status = TransferStatus.Done;
                    task.FinishTime = DateTime.Now;
                    completed = task;
                }
                else
                {
                    task.Status = TransferStatus.Verifying;
                }
            }
        }

        if (completed is not null)
        {
            RaiseChanged(QueueChangeKind.Completed, completed);
            return;
        }

        if (hook is not null)
        {
            _ = RunVerifyAsync(id, hook);
        }
    }

    /// <summary>跑校验并结算：不匹配 → 先记结果再失败（时序保证 StatusText 读取时
    /// 结果已就位，原因带两侧杂凑）；一致/跳过 → 记结果后完成。校验执行体抛异常
    /// 视同跳过（校验本身失败不得连累传输结果）。</summary>
    private static async Task RunVerifyAsync(Guid id, Func<Task<VerifyOutcome>> hook)
    {
        VerifyOutcome outcome;
        try
        {
            outcome = await hook().ConfigureAwait(false);
        }
        catch
        {
            outcome = new VerifyOutcome(VerifyResultKind.Skipped);
        }

        LogVerifyOutcome(id, outcome);

        switch (outcome.Kind)
        {
            case VerifyResultKind.Mismatch:
                SetVerifyResult(id, VerifyResultKind.Mismatch);
                Fail(id, string.Format(LocalizationService.GetString("QVerifyMismatchDetail"),
                    outcome.LocalHex, outcome.RemoteHex));
                break;
            default:
                SetVerifyResult(id, outcome.Kind);
                Complete(id);
                break;
        }
    }

    /// <summary>校验结算输出到命令行面板（[校验] 前缀, 与 [续传]/[tar 加速] 同风格）。
    /// 任务已被清队列移除时取不到路径, 静默跳过——队列面板已无该行, 日志无主。</summary>
    private static void LogVerifyOutcome(Guid id, VerifyOutcome outcome)
    {
        TransferTask? task;
        lock (Lock)
        {
            task = FindLocked(id);
        }

        if (task is null)
        {
            return;
        }

        var sourcePath = task.Source;
        var scopes = task.LogScopes;
        switch (outcome.Kind)
        {
            case VerifyResultKind.Mismatch:
                TerminalLogService.Log(string.Format(LocalizationService.GetString("LogVerifyMismatch"),
                    sourcePath, outcome.LocalHex, outcome.RemoteHex), scopes);
                break;
            case VerifyResultKind.Match:
                TerminalLogService.Log(string.Format(LocalizationService.GetString("LogVerifyMatch"),
                    sourcePath, outcome.Algorithm), scopes);
                break;
            default: // Skipped（None 理论不可达, 同按跳过记）
                TerminalLogService.Log(string.Format(
                    LocalizationService.GetString("LogVerifySkipped"), sourcePath), scopes);
                break;
        }
    }

    private static void SetVerifyResult(Guid id, VerifyResultKind kind)
    {
        lock (Lock)
        {
            if (FindLocked(id) is { } task)
            {
                task.VerifyResult = kind;
            }
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

            task.BytesDone = bytes;
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

    /// <summary>按页签取任务视图（队列中 = 等待+传输中+校验中+已取消）。</summary>
    public static IReadOnlyList<TransferTask> Get(TransferQueueTab tab)
    {
        lock (Lock)
        {
            return tab switch
            {
                TransferQueueTab.Failed => Tasks.Where(t => t.Status == TransferStatus.Failed).ToList(),
                TransferQueueTab.Done => Tasks.Where(t => t.Status == TransferStatus.Done).ToList(),
                _ => Tasks.Where(t => t.Status is TransferStatus.Waiting or TransferStatus.Transferring
                    or TransferStatus.Verifying or TransferStatus.Cancelled).ToList(),
            };
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

    /// <summary>暂停中断后把"传输中"任务回排"等待"（行内状态变化，无结构事件——
    /// 行不增不减；此时全局暂停标志已置位，行内第四列重读为"已暂停"）。继续后
    /// worker 重跑同一任务，ResumePolicy 按对端半成品自动断点续传。速度基线清零，
    /// 防止重跑后首窗口把断点前字节算进速度。</summary>
    public static void RequeueAfterPause(Guid id)
    {
        lock (Lock)
        {
            if (FindLocked(id) is { } task && task.Status == TransferStatus.Transferring)
            {
                task.Status = TransferStatus.Waiting;
                task.SpeedText = string.Empty;
                task.LastBytes = 0;
                task.LastTick = 0;
                task.IsResumed = false;
            }
        }
    }

    /// <summary>队列概览（右键菜单按状态显隐菜单项用）。</summary>
    public static (int Total, int Waiting, int Transferring, int Done, int Failed, int Cancelled) GetSummary()
    {
        lock (Lock)
        {
            return (
                Tasks.Count,
                Tasks.Count(t => t.Status == TransferStatus.Waiting),
                Tasks.Count(t => t.Status is TransferStatus.Transferring or TransferStatus.Verifying),
                Tasks.Count(t => t.Status == TransferStatus.Done),
                Tasks.Count(t => t.Status == TransferStatus.Failed),
                Tasks.Count(t => t.Status == TransferStatus.Cancelled));
        }
    }

    /// <summary>清除已完结任务（完成/失败/已取消）：排队与传输中的保留。UI 收到
    /// Reset 后全量重填三个页签集合（手动低频操作）。</summary>
    public static void ClearAll()
    {
        lock (Lock)
        {
            Tasks.RemoveAll(t => t.Status is TransferStatus.Done or TransferStatus.Failed
                or TransferStatus.Cancelled);
        }

        RaiseChanged(QueueChangeKind.Reset, null);
    }

    /// <summary>清除所有任务（队列右键"清除所有"）：排队/已暂停/传输中/已完结全清。
    /// 传输中的行删除后, worker 在下一个块边界检查点按"任务不存在 = 已取消"退出
    /// （见 IsCancelRequested）, 结算对已删任务无害, 不留幽灵传输。</summary>
    public static void RemoveAll()
    {
        lock (Lock)
        {
            Tasks.Clear();
        }

        RaiseChanged(QueueChangeKind.Reset, null);
    }

    /// <summary>
    /// 停止传输：置位停止标志 + 解除暂停 + 取消令牌（ConcurrentTransferService.RequestStop），
    /// 等待中任务立即标"已取消"（行留"队列中"页签，可重试）。传输中的任务在下一个
    /// 块边界检查点自行中止并结算"已取消"（半成品留断点，可重试续传）；校验中的
    /// 不干预，exec 10s 超时兜底自结算。行内状态变化，无结构事件。
    /// </summary>
    public static void StopAll()
    {
        ConcurrentTransferService.RequestStop();
        lock (Lock)
        {
            IsPaused = false;
            foreach (var task in Tasks.Where(t => t.Status == TransferStatus.Waiting))
            {
                task.Status = TransferStatus.Cancelled;
                task.SpeedText = string.Empty;
            }
        }
    }

    /// <summary>单任务取消（行右键）：等待中直接标"已取消"（worker 领取时 Begin
    /// 原子拒绝）；传输中置取消请求标志，块边界检查点命中后由 RunOneAsync 结算。
    /// 校验中/已完结不干预。行内状态变化，无结构事件。</summary>
    public static void CancelTask(Guid id)
    {
        lock (Lock)
        {
            if (FindLocked(id) is not { } task)
            {
                return;
            }

            if (task.Status == TransferStatus.Waiting)
            {
                task.Status = TransferStatus.Cancelled;
                task.SpeedText = string.Empty;
            }
            else if (task.Status == TransferStatus.Transferring)
            {
                task.CancelRequested = true;
            }
        }
    }

    /// <summary>检查点查询：该任务是否已请求取消（ConcurrentTransferService 的
    /// 块边界检查点按 AsyncLocal 任务 id 调用）。任务已移除（"清除所有"）同样返回
    /// true——行都没了传输理应终止, 防 worker 失去取消信号变成幽灵传输。</summary>
    public static bool IsCancelRequested(Guid id)
    {
        lock (Lock)
        {
            return FindLocked(id) is not { } task || task.CancelRequested;
        }
    }

    /// <summary>标记任务已取消（结算入口，RunOneAsync 在检查点停止/令牌取消时调用）：
    /// 行留"队列中"页签，第四列显示"已取消"，可右键重试。已完结不覆盖。</summary>
    public static void Cancel(Guid id)
    {
        lock (Lock)
        {
            if (FindLocked(id) is { } task
                && task.Status is TransferStatus.Waiting or TransferStatus.Transferring)
            {
                task.Status = TransferStatus.Cancelled;
                task.SpeedText = string.Empty;
            }
        }
    }

    /// <summary>某视图 scope 是否仍有活动任务（等待/传输中/校验中；跨窗格任务两侧
    /// scope 都记录，关任一侧都命中）：关闭 tab 前确认对话框的判定依据。</summary>
    public static bool HasActiveForScope(string scope)
    {
        lock (Lock)
        {
            return Tasks.Any(t => (t.Status is TransferStatus.Waiting or TransferStatus.Transferring
                or TransferStatus.Verifying)
                && Array.IndexOf(t.LogScopes, scope) >= 0);
        }
    }

    /// <summary>移除单条已完结任务（行右键）：等待/传输中不允许（UI 菜单显隐控制，
    /// 此处再防御）。UI 收到 Removed 后从三个页签集合各删除一次。</summary>
    public static void Remove(Guid id)
    {
        TransferTask? removed;
        lock (Lock)
        {
            removed = Tasks.FirstOrDefault(t => t.Id == id);
            if (removed is not null && removed.Status is TransferStatus.Done
                or TransferStatus.Failed or TransferStatus.Cancelled)
            {
                Tasks.Remove(removed);
            }
            else
            {
                removed = null;
            }
        }

        if (removed is not null)
        {
            RaiseChanged(QueueChangeKind.Removed, removed);
        }
    }

    /// <summary>
    /// 单任务重试（行右键，失败/已取消行）：重置该任务回"等待"后以单条目批次重跑
    /// （新建批次连接池，执行体不绑定池实例）。重置广播 Reset 结构事件——失败行
    /// 搬回"队列中"页签，UI 全量重填（手动低频操作）。
    /// </summary>
    public static async Task RerunOneAsync(Guid id)
    {
        var items = new List<(Guid QueueId, Func<TransferBatchConnections, Task> Run)>();
        lock (Lock)
        {
            if (Tasks.FirstOrDefault(t => t.Id == id) is not { } task
                || task.Run is null
                || task.Status is TransferStatus.Waiting or TransferStatus.Transferring
                    or TransferStatus.Verifying)
            {
                return; // 防御: 不存在/无执行体/仍活动时忽略 (正常菜单已按状态显隐)
            }

            task.CancelRequested = false;
            task.Status = TransferStatus.Waiting;
            task.SpeedText = string.Empty;
            task.ErrorText = string.Empty;
            task.FinishTime = null;
            task.LastBytes = 0;
            task.LastTick = 0;
            task.BytesDone = 0;
            task.IsResumed = false; // 续传是每次执行重新决策的, 复位
            task.VerifyResult = VerifyResultKind.None; // 校验结果随之清零
            items.Add((task.Id, task.Run));
        }

        RaiseChanged(QueueChangeKind.Reset, null);
        using var connections = new TransferBatchConnections();
        await ConcurrentTransferService.RunAsync(items, AppSettingsService.ConcurrentTransfers, connections);
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
        var scopeSet = new HashSet<string>();
        lock (Lock)
        {
            if (Tasks.Any(t => t.Status is TransferStatus.Transferring or TransferStatus.Verifying))
            {
                return; // 防御: 有任务正在传输/校验时忽略重传 (正常菜单已按状态隐藏)
            }

            foreach (var task in Tasks)
            {
                if (task.Run is null)
                {
                    continue;
                }

                scopeSet.UnionWith(task.LogScopes); // 任务可能跨多 tab, 汇总日志发全部相关池
                task.Status = TransferStatus.Waiting;
                task.SpeedText = string.Empty;
                task.ErrorText = string.Empty;
                task.FinishTime = null;
                task.LastBytes = 0;
                task.LastTick = 0;
                task.BytesDone = 0;
                task.IsResumed = false; // 续传是每次执行重新决策的, 复位
                task.VerifyResult = VerifyResultKind.None; // 校验结果随之清零
                items.Add((task.Id, task.Run));
            }
        }

        IsPaused = false;
        RaiseChanged(QueueChangeKind.Reset, null);
        if (items.Count == 0)
        {
            return;
        }

        var rerunScopes = scopeSet.ToArray();

        // 命令行面板叙述: 重传开始与结束汇总 (跳过数为 0, 复用完成文案)
        TerminalLogService.Log(string.Format(
            LocalizationService.GetString("LogQueueRerun"), items.Count), rerunScopes);

        using var connections = new TransferBatchConnections();
        var (_, failed) = await ConcurrentTransferService.RunAsync(
            items, AppSettingsService.ConcurrentTransfers, connections);

        TerminalLogService.Log(string.Format(
            LocalizationService.GetString("LogTransferDone"), items.Count - failed, 0, failed), rerunScopes);
    }

    // 调用方必须已持有 Lock
    private static TransferTask? FindLocked(Guid id)
        => Tasks.FirstOrDefault(t => t.Id == id);

    private static void RaiseChanged(QueueChangeKind kind, TransferTask? task)
        => Changed?.Invoke(kind, task);
}
