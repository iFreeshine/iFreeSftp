using iFreeSftp.Views;

namespace iFreeSftp.Services;

/// <summary>
/// 同步浏览状态总线: 左右 FolderContainer 注册自身, Header 模式胶囊的同步按钮
/// 据此计算可用/点亮状态。状态机规则 (用户口述):
/// ①主机设置了同步浏览且两侧目录同步匹配上 → 点亮;
/// ②只有一侧文件夹 tab 打开 → 按钮 disable;
/// ③两侧都有文件夹 → 可点击, 匹配不上 → 弹不匹配信息, 不予点亮;
/// ④同步过程中出现不匹配 → 弹不匹配信息, 按钮熄灭 (联动关闭)。
/// 匹配判定 = 两侧视图各有同步基准根 (SyncBasePath, 连接定位时设置) 且
/// 当前目录相对各自基准根的相对路径一致 (本地大小写不敏感)。
/// </summary>
public static class SyncBrowseService
{
    private static readonly Dictionary<string, FolderContainer> _panes = new();

    /// <summary>同步联动当前是否激活 (按钮点亮状态)。</summary>
    public static bool Active { get; private set; }

    /// <summary>两侧窗格状态变化 (tab 开关/连接/导航/定位), Header 重算按钮状态。</summary>
    public static event Action? StateChanged;

    /// <summary>同步不匹配 (点击建立失败或联动中失配), 参数为 resw 原因文案 key。
    /// 首次失配才发出 (防双弹), 同时自动熄灭联动。</summary>
    public static event Action<string>? MismatchDetected;

    public static void Register(string paneName, FolderContainer pane) => _panes[paneName] = pane;

    public static FolderContainer? GetPane(string paneName) =>
        _panes.TryGetValue(paneName, out var pane) ? pane : null;

    /// <summary>点亮/熄灭联动 (Header 按钮点击驱动)。程序化熄灭不重复广播。</summary>
    public static void SetActive(bool on)
    {
        if (Active == on)
        {
            return;
        }

        Active = on;
        StateChanged?.Invoke();
    }

    /// <summary>窗格状态变化通知 (FolderContainer 视图/tab 变化时调用)。</summary>
    public static void RaiseStateChanged() => StateChanged?.Invoke();

    /// <summary>
    /// 报告同步不匹配: 熄灭联动 (幂等, 已灭不重复广播 StateChanged) 并广播
    /// 弹窗事件; 弹窗防重入由订阅方 (Header) 守卫。
    /// </summary>
    public static void ReportMismatch(string reasonKey)
    {
        SetActive(false);
        MismatchDetected?.Invoke(reasonKey);
    }

    /// <summary>单侧窗格同步快照: 是否有活动视图 / 是否有同步基准根 (主机配置,
    /// 非空即有) / 相对基准根的相对路径 ('/' 归一; null = 无基准或越出基准根)。</summary>
    public readonly record struct PaneSnapshot(bool HasView, bool HasBase, string? RelPath);

    /// <summary>
    /// 评估当前同步状态。HasBothPanes=两侧都有文件夹视图 (按钮可用的前提);
    /// Matched=主机有同步基准 (两侧 SyncBasePath 非空) 且相对路径一致;
    /// ReasonKey=不匹配时的 resw 文案键。
    /// </summary>
    public static (bool HasBothPanes, bool Matched, string ReasonKey) Evaluate()
    {
        var left = GetPane("Left")?.GetSyncSnapshot() ?? default;
        var right = GetPane("Right")?.GetSyncSnapshot() ?? default;
        var both = left.HasView && right.HasView;
        if (!both)
        {
            return (false, false, "SyncBrowseMismatch");
        }

        if (!left.HasBase || !right.HasBase)
        {
            // 一侧未连接/未定位同步基准 (主机未配置同步浏览或一侧未连接)
            return (true, false, "SyncMismatchNotConfigured");
        }

        if (left.RelPath is null || right.RelPath is null)
        {
            // 一侧越出基准根 (本地回驱动器列表 / 远程回基准根之上)
            return (true, false, "SyncMismatchMsg");
        }

        var same = string.Equals(left.RelPath, right.RelPath, StringComparison.OrdinalIgnoreCase);
        return same
            ? (true, true, string.Empty)
            : (true, false, "SyncMismatchMsg");
    }
}
