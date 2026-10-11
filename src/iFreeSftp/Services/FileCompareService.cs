using iFreeSftp.Views;

namespace iFreeSftp.Services;

/// <summary>
/// 文件比对状态总线 (规则, 用户口述):
/// ①只有同步浏览点亮 (SyncBrowseService.Active) 时文件比对才允许选中;
/// ②主机卡片配置了文件比对 → 连接落位后自动激活;
/// ③激活时对比左右两侧文件列表条目 (不对比文件内容);
/// ④某侧有另一侧没有的文件 → 有文件的一行 Caution 高亮, 缺文件一侧对应行空置
///   (占位行, 两侧行对齐);
/// ⑤两侧都有但文件时间或大小不同 → 该行 Critical 高亮。
/// 激活时两侧活动视图按"合并文件名清单"重排 (目录参与配对但不染色);
/// 熄灭或同步熄灭联动熄灭时恢复常规列表。
/// </summary>
public static class FileCompareService
{
    /// <summary>文件比对当前是否激活 (按钮点亮状态)。</summary>
    public static bool Active { get; private set; }

    /// <summary>激活状态变化 (Header 同步驱动按钮勾选态)。</summary>
    public static event Action? ActiveChanged;

    /// <summary>两侧重排防重入 (一侧加载触发重排会再次驱动对侧, 避免互调成环)。</summary>
    private static bool _applying;

    /// <summary>
    /// 点亮/熄灭比对 (Header 按钮或连接自动激活驱动)。点亮即对两侧活动视图重排,
    /// 熄灭即恢复常规列表 (重新加载当前目录)。
    /// </summary>
    public static void SetActive(bool on)
    {
        if (Active == on)
        {
            return;
        }

        Active = on;
        ActiveChanged?.Invoke();
        if (on)
        {
            ApplyBoth();
        }
        else
        {
            RestoreBoth();
        }
    }

    /// <summary>同步熄灭时联动熄灭比对 (幂等; 熄灭会恢复两侧常规列表)。</summary>
    public static void Reset()
    {
        if (Active)
        {
            SetActive(false);
        }
    }

    /// <summary>两侧活动视图重排比对 (一侧列表加载完成/开关切换/换 tab 时调用)。
    /// 失效守卫: 任一侧失去可配对视图 (tab 全关回空态引导页 / 窗格无视图) 时比对
    /// 已无法进行——主动熄灭并恢复有效侧常规列表。不守卫则两侧各自早退保留上次
    /// 重排结果, 占位空行与高亮永久停留。防重入保证一次触发两侧各排一次。</summary>
    public static void ApplyBoth()
    {
        if (!Active || _applying)
        {
            return;
        }

        var left = SyncBrowseService.GetPane("Left")?.ActiveView;
        var right = SyncBrowseService.GetPane("Right")?.ActiveView;
        if (left is null || right is null || left.IsEmptyState || right.IsEmptyState)
        {
            Reset(); // 内部走 SetActive(false) → RestoreBoth, 幂等
            return;
        }

        _applying = true;
        try
        {
            left.ApplyCompareLayout();
            right.ApplyCompareLayout();
        }
        finally
        {
            _applying = false;
        }
    }

    /// <summary>熄灭后就地恢复常规布局 (剔除占位行 + 常规排序, 不重新加载)。
    /// 旧实现用 RefreshListKeepingPath 重新加载, 会被"失配触发侧"自身的在途加载
    /// 防重入标志 (_loadingRemote) 静默吞掉, 占位空行停留 (实测)。空态引导页
    /// 跳过——它没有列表数据。</summary>
    private static void RestoreBoth()
    {
        foreach (var name in new[] { "Left", "Right" })
        {
            if (SyncBrowseService.GetPane(name)?.ActiveView is { IsEmptyState: false } view)
            {
                view.RestoreNormalLayout();
            }
        }
    }
}
