using iFreeSftp.Models;
using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace iFreeSftp.Views;

/// <summary>
/// 文件夹容器: 承载一个 <see cref="IdeTabStrip"/>, 每个标签内为一个 <see cref="FileView"/>。
/// 主容器内左右并排两个文件夹容器, 各自独立多开标签, 均可连接远程。
/// 标签条 UI (模板/配色/基线/+ 按钮) 由 IdeTabStrip 封装, 此处只负责业务:
/// 创建/释放 FileView、本地化、本地-远程模式切换。
/// </summary>
public sealed partial class FolderContainer : UserControl
{
    /// <summary>窗格标识（Left/Right），用于本地化标题与默认模式。</summary>
    public string PaneName { get; set; } = "Left";

    /// <summary>对侧窗格 (MainContainer 构造后接线), 供"复制到另一侧"定位目标视图。</summary>
    public FolderContainer? OppositePane { get; set; }

    /// <summary>新建/关闭标签时触发，便于主窗口统计。</summary>
    public event Action<FolderContainer>? TabCountChanged;

    /// <summary>当前活动标签页的 FileView。</summary>
    public FileView? ActiveView => (Strip.Tabs.SelectedItem as TabViewItem)?.Content as FileView;

    /// <summary>
    /// 请求打开"打开内容"选择器 (tab 栏 + 按钮或空状态"选择主机"按钮触发,
    /// MainContainer 订阅)。参数为来源窗格 (Left/Right), 供落位。
    /// </summary>
    public event Action<string>? OpenPickerRequested;

    /// <summary>
    /// 本窗格是否处于初始空状态: 唯一的 tab 仍是引导页（尚未发生任何连接/浏览）。
    /// 此时首个主机连接直接在当前 tab 打开, 无需询问打开方式。
    /// </summary>
    public bool IsInitialEmptyState =>
        Strip.Tabs.TabItems.Count <= 1 && (ActiveView?.IsEmptyState ?? false);

    /// <summary>因空状态引导隐藏了标签条头部（恢复用）。</summary>
    private bool _headerHiddenForEmpty;

    public FolderContainer()
    {
        InitializeComponent();
        // + 按钮: 打开"打开目标"选择器 (本地文件系统 / 主机列表)。
        Strip.NewTabRequested += (_, _) => OpenPickerRequested?.Invoke(PaneName);
        Strip.TabCloseRequested += OnTabCloseRequested;
        // 换 tab 后活动视图变化: 比对激活中时新活动视图按合并名清单重排
        // (未激活的视图仍是常规列表, 重排幂等无害)。
        Strip.Tabs.SelectionChanged += (_, _) => FileCompareService.ApplyBoth();
        Loaded += OnLoaded;
        // 注册不放 ctor: PaneName 是 XAML 属性赋值, 发生在 ctor 之后,
        // ctor 里两个窗格都会以默认值 "Left" 注册而互相覆盖
    }

    /// <summary>本窗格状态变化 (tab 开关/连接/导航): Header 重算同步按钮。</summary>
    public void NotifySyncStateChanged() => SyncBrowseService.RaiseStateChanged();

    /// <summary>同步快照: 活动视图存在性 / 是否有同步基准根 (主机配置) / 相对基准根
    /// 的相对路径 (null = 无基准或越出基准根)。</summary>
    public SyncBrowseService.PaneSnapshot GetSyncSnapshot()
    {
        var view = ActiveView;
        if (view is null || view.IsEmptyState)
        {
            return new SyncBrowseService.PaneSnapshot(false, false, null);
        }

        return new SyncBrowseService.PaneSnapshot(
            true,
            !string.IsNullOrEmpty(view.SyncBasePath),
            RelToBase(view));
    }

    /// <summary>
    /// 视图当前目录相对其同步基准根的相对路径 ('/' 分隔归一; 大小写按本地/远程
    /// 各自规则); 无基准根或越出基准根返回 null。与镜像导航 (HandleSyncNavigation)
    /// 共用同一套边界规则, 保证"判定匹配"与"执行镜像"口径一致。
    /// </summary>
    internal static string? RelToBase(FileView view)
    {
        var basePath = view.SyncBasePath;
        if (string.IsNullOrEmpty(basePath))
        {
            return null;
        }

        var path = view.CurrentPath;
        if (view.Mode == FileViewMode.Local)
        {
            // 盘根本身以 '\' 结尾 ("E:\"), 其余基准根补尾分隔符后做前缀匹配
            if (string.Equals(path, basePath, StringComparison.OrdinalIgnoreCase))
            {
                return string.Empty;
            }

            var prefix = basePath.EndsWith('\\') ? basePath : basePath + '\\';
            return path.StartsWith(prefix, StringComparison.OrdinalIgnoreCase)
                ? path[prefix.Length..].Replace('\\', '/').TrimEnd('/')
                : null;
        }

        if (basePath == "/")
        {
            return path.Length <= 1 ? string.Empty : path.TrimStart('/');
        }

        if (path == basePath)
        {
            return string.Empty;
        }

        return path.StartsWith(basePath + '/')
            ? path[(basePath.Length + 1)..].TrimEnd('/')
            : null;
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        ApplyResources();
        // 供 Header 同步按钮状态判定; Loaded 时 XAML 已赋完 PaneName, 键值正确
        SyncBrowseService.Register(PaneName, this);
        // 初始 tab: 左窗格直接显示本地根目录, 右窗格显示空态引导 (选择目标)。
        if (Strip.Tabs.TabItems.Count == 0)
        {
            AddNewTab(emptyState: PaneName == "Right");
        }
    }

    /// <summary>重新应用本地化文本（所有标签页标题与内容）。</summary>
    public void ApplyResources()
    {
        foreach (var tab in Strip.Tabs.TabItems.OfType<TabViewItem>())
        {
            if (tab.Content is FileView view)
            {
                Strip.SetTabTitle(tab, GetTabTitle(view.Mode));
                Strip.SetTabIcon(tab, view.Mode == FileViewMode.Remote
                    ? IdeTabStrip.RemoteIconGlyph
                    : IdeTabStrip.LocalIconGlyph);
                view.ApplyResources();
            }
        }
    }

    private string GetTabTitle(FileViewMode mode) => LocalizationService.GetString(
        mode == FileViewMode.Remote ? "TabRemote" : "TabLocal");

    /// <summary>
    /// 在本窗格打开本地浏览 (选择器"本地文件系统"落位):
    /// 唯一 tab 还是空态引导时就地转为本地浏览, 否则新建标签页。
    /// </summary>
    public void OpenLocalView()
    {
        if (Strip.Tabs.TabItems.Count == 1 && ActiveView?.IsEmptyState == true)
        {
            ActiveView.DismissEmptyState(); // 空态引导 tab 就地转为本地浏览
        }
        else
        {
            AddNewTab();
        }
    }

    /// <summary>新建一个标签页, 内容为当前窗格的 FileView; emptyState=true 为空态引导页;
    /// remote=true 为远程模式 tab (主机连接落位, 不加载本地列表)。</summary>
    private void AddNewTab(bool emptyState = false, bool remote = false)
    {
        var view = new FileView(PaneName, emptyState, remote)
        {
            HostPane = this,
        };
        // 同步浏览: 本视图目录变更时, 把对侧窗格镜像到 "对侧基准根 + 相对路径"。
        view.SyncNavigationRequested += path => HandleSyncNavigation(view, path);
        var tab = Strip.AddTab(
            GetTabTitle(remote ? FileViewMode.Remote : FileViewMode.Local),
            remote ? IdeTabStrip.RemoteIconGlyph : IdeTabStrip.LocalIconGlyph, view);
        // 空状态引导"选择主机"按钮 → 打开"打开目标"选择器。
        view.OpenPickerRequested += () => OpenPickerRequested?.Invoke(PaneName);
        // 空状态时隐藏标签条头部 (引导页独占窗格), 首次连接收起引导后恢复。
        if (view.IsEmptyState)
        {
            _headerHiddenForEmpty = true;
            Strip.SetHeaderVisible(false);
        }

        view.EmptyStateChanged += isEmpty =>
        {
            if (!isEmpty && _headerHiddenForEmpty)
            {
                _headerHiddenForEmpty = false;
                Strip.SetHeaderVisible(true);
            }
        };

        // 标题与图标随视图模式变化（本地/远程）。
        view.ModeChanged += mode =>
        {
            Strip.SetTabTitle(tab, GetTabTitle(mode));
            Strip.SetTabIcon(tab, mode == FileViewMode.Remote
                ? IdeTabStrip.RemoteIconGlyph
                : IdeTabStrip.LocalIconGlyph);
        };

        TabCountChanged?.Invoke(this);
        NotifySyncStateChanged(); // tab 数变化影响同步按钮 enable 判定
    }

    /// <summary>
    /// 连接主机，成功后才落位（HostPanel 连接入口）。连接阶段（日志/loading/指纹
    /// 确认弹窗锚定本容器）不动任何标签页; 成功: newTab 新建远程模式 tab、否则活动
    /// tab 就地接管会话转远程并加载列表; 失败: 仅命令窗日志, 不构建 tab 与远程容器。
    /// 返回 (是否成功, 用户本次确认的指纹)。
    /// </summary>
    public async Task<(bool Ok, string? Fingerprint)> ConnectAndOpenAsync(HostProfile host, bool newTab)
    {
        var session = new SftpSessionService
        {
            ConfirmHostKey = fp =>
            {
                TerminalLogService.Log(LocalizationService.GetString("LogWaitFingerprint"));
                return HostKeyConfirmDialog.ShowFromWorkerAsync(this, host.Host, fp);
            },
            ConfirmHostKeyChange = (stored, presented) =>
            {
                TerminalLogService.Log(LocalizationService.GetString("LogWaitFingerprint"));
                return HostKeyConfirmDialog.ShowChangeFromWorkerAsync(this, host.Host, stored, presented);
            },
        };
        session.SetTrustedFingerprint(string.IsNullOrWhiteSpace(host.HostKeyFingerprint)
            ? null
            : host.HostKeyFingerprint);
        TerminalLogService.SetBusy(true);
        TerminalLogService.Log(string.Format(
            LocalizationService.GetString("LogConnecting"), host.Host, host.Port));
        try
        {
            if (!await session.ConnectAsync(host))
            {
                TerminalLogService.Log(string.Format(
                    LocalizationService.GetString("LogConnectFailed"), string.Empty));
                session.Dispose();
                return (false, null);
            }
        }
        catch (Exception ex)
        {
            TerminalLogService.Log(string.Format(
                LocalizationService.GetString("LogConnectFailed"), ex.Message));
            session.Dispose();
            return (false, null);
        }
        finally
        {
            TerminalLogService.SetBusy(false);
        }

        // 连接成功才构建/转换 tab 与远程容器。
        var view = newTab || Strip.Tabs.TabItems.Count == 0
            ? AddNewTabGetView(remote: true)
            : ActiveView!;
        view.AttachConnectedSession(session, host);

        // 同步浏览: 主机开启同步且填了本地目录时, 对侧窗格按该目录初始定位
        // (本地/空态视图就地导航, 远程会话视图新开本地 tab, 不劫持原会话)。
        // 对侧为空 (无视图/空态, 如应用刚启动) 时不自动开启本地面板:
        // 同步与文件比对因缺对侧基准一并取消, 只在连接侧打开主机 (2026-10-03)。
        if (host.SyncEnabled && !string.IsNullOrWhiteSpace(host.LocalSyncPath))
        {
            var oppositeView = OppositePane?.ActiveView;
            if (oppositeView is not null && !oppositeView.IsEmptyState)
            {
                OppositePane?.OpenLocalSyncView(host.LocalSyncPath);
                // 规则①: 主机配置了同步浏览, 连接落位后两侧均定位于基准根, 天然匹配
                // → 自动点亮 (之后的导航镜像由 HandleSyncNavigation 的 Active 门控执行)
                SyncBrowseService.SetActive(true);
                // 规则②: 主机卡片配置了文件比对 → 连接后自动激活 (同步已点亮满足规则①;
                // 列表加载完成经 ApplySort 挂钩会再重排, 此处先激活点亮按钮)。
                if (host.CompareEnabled)
                {
                    FileCompareService.SetActive(true);
                }
            }
            else
            {
                // 对侧为空: 显式取消同步与比对 (残留点亮状态一并熄灭)
                SyncBrowseService.SetActive(false);
                FileCompareService.SetActive(false);
            }
        }

        NotifySyncStateChanged(); // 连接落位改变快照, Header 重算按钮状态
        return (true, session.ConfirmedFingerprint);
    }

    /// <summary>
    /// 同步浏览定位: 活动视图为本地或空态时就地导航到指定目录; 活动视图是远程
    /// 会话时新开一个本地标签页导航 (保留原远程会话), 新标签自动成为活动标签。
    /// </summary>
    public void OpenLocalSyncView(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return;
        }

        var view = ActiveView;
        if (view is null || (view.Mode == FileViewMode.Remote && !view.IsEmptyState))
        {
            AddNewTab(); // 无视图或远程会话占位: 新建本地标签承载同步浏览
            view = ActiveView!;
        }

        view.NavigateLocalTo(path);
        NotifySyncStateChanged(); // 同步定位改变快照, Header 重算按钮状态
    }

    /// <summary>
    /// 同步浏览镜像导航: 一侧进入/返回目录时, 对侧按 "各自基准根 + 相对路径"
    /// 同时进入/返回 (含退回基准根本身)。发起侧越出基准根 (回驱动器列表/根之上)
    /// 判定两侧目录不一致; 对侧目标不存在或加载失败同样判定不一致——均提示并
    /// 关闭两侧联动。对侧无活动视图或未参与联动时仅跳过本次联动。
    /// </summary>
    private void HandleSyncNavigation(FileView source, string path)
    {
        // 联动只在同步按钮点亮 (Active) 时工作; 熄灭后导航互不影响
        if (!SyncBrowseService.Active)
        {
            return;
        }

        var target = OppositePane?.ActiveView;
        if (target is null)
        {
            return;
        }

        var targetBase = target.SyncBasePath;
        if (targetBase.Length == 0)
        {
            return;
        }

        // 发起侧相对基准根的相对路径 (与匹配判定共用 RelToBase, 口径一致);
        // null = 越出基准根 (回驱动器列表/基准根之上) → 两侧无法对应, 关闭联动
        var rel = RelToBase(source);
        if (rel is null)
        {
            SyncBrowseService.ReportMismatch("SyncMismatchMsg");
            return;
        }

        // 对侧目标 = 对侧基准根 + 相对路径 (用对侧自身的路径分隔符拼接)
        var targetPath = rel.Length == 0
            ? targetBase
            : target.Mode == FileViewMode.Local
                ? (targetBase.EndsWith('\\') ? targetBase : targetBase + '\\') + rel.Replace('/', '\\')
                : targetBase == "/" ? "/" + rel : targetBase + '/' + rel;

        // 异步镜像: 对侧进入失败 (目录不存在/列举失败) → 提示并关闭同步浏览
        _ = MirrorNavigateAsync(target, targetPath);
    }

    /// <summary>执行对侧镜像导航, 失败时提示目录不一致并关闭两侧联动。</summary>
    private async Task MirrorNavigateAsync(FileView target, string targetPath)
    {
        if (!await target.NavigateFromSyncAsync(targetPath))
        {
            SyncBrowseService.ReportMismatch("SyncMismatchMsg");
        }
    }

    /// <summary>AddNewTab 的带返回值变体: 返回新建 tab 承载的 FileView。</summary>
    private FileView AddNewTabGetView(bool remote)
    {
        AddNewTab(remote: remote);
        return ActiveView!;
    }

    /// <summary>关闭标签页: IdeTabStrip 已把标签移出, 此处释放内容资源。</summary>
    private void OnTabCloseRequested(TabViewItem tab)
    {
        if (tab.Content is FileView view)
        {
            view.Dispose();
        }

        // 最后一个 tab 关闭: 回到初始化空态 (引导页独占窗格, 标签条随之隐藏)。
        if (Strip.Tabs.TabItems.Count == 0)
        {
            AddNewTab(emptyState: true);
            return;
        }

        TabCountChanged?.Invoke(this);
        NotifySyncStateChanged(); // 活动视图可能换人, Header 重算按钮状态
    }
}

/// <summary>文件视图模式：本地或远程。</summary>
public enum FileViewMode
{
    Local,
    Remote,
}
