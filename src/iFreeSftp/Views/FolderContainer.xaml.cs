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

    /// <summary>关闭确认对话框展示中（同窗 ContentDialog 单实例, 防并发弹窗重入）。</summary>
    private bool _confirmingOpen;

    public FolderContainer()
    {
        InitializeComponent();
        // + 按钮: 打开"打开目标"选择器 (本地文件系统 / 主机列表)。
        Strip.NewTabRequested += (_, _) => OpenPickerRequested?.Invoke(PaneName);
        Strip.TabCloseConfirming += OnTabCloseConfirming;
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
                // 主机 tab 标题优先 (连接时已写名称+IP), 不被语言切换的通用标题覆盖
                Strip.SetTabTitle(tab, view.HostTabTitle ?? GetTabTitle(view.Mode));
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

        // 标题与图标随视图模式变化（本地/远程）; 主机 tab 保持名称+IP 标题。
        view.ModeChanged += mode =>
        {
            Strip.SetTabTitle(tab, view.HostTabTitle ?? GetTabTitle(mode));
            Strip.SetTabIcon(tab, mode == FileViewMode.Remote
                ? IdeTabStrip.RemoteIconGlyph
                : IdeTabStrip.LocalIconGlyph);
        };

        TabCountChanged?.Invoke(this);
        NotifySyncStateChanged(); // tab 数变化影响同步按钮 enable 判定
    }

    /// <summary>
    /// 连接主机，标签先行 (tab-first): 新标签意图时立即落位远程占位 tab, 标题=
    /// 主机名称+IP, 连接成败均保留——失败仅把错误写进该 tab 状态栏与命令窗,
    /// 不撤销 tab (2026-10-17); 复用路径 (当前 tab 打开) 仍保持成功才接管。
    /// 连接阶段 (日志/loading/指纹确认弹窗锚定本容器) 不动已有标签页; 成功:
    /// 占位 tab 就地接管会话或活动 tab 转远程并加载列表。
    /// 返回 (是否成功, 用户本次确认的指纹)。
    /// </summary>
    public async Task<(bool Ok, string? Fingerprint)> ConnectAndOpenAsync(HostProfile host, bool newTab)
    {
        // 标签先行: 立即建远程占位 tab 并写主机标题, 失败不撤销。
        // 先落 tab 再建会话: 会话日志的 scope 归属 (含指纹审计) 需要 tab 的 LogScope。
        TabViewItem? pendingTab = null;
        FileView? pendingView = null;
        if (newTab || Strip.Tabs.TabItems.Count == 0)
        {
            (pendingTab, pendingView) = AddNewTabGetView(remote: true);
            pendingView.HostTabTitle = HostTabNaming.Build(host.Name, host.Host);
            Strip.SetTabTitle(pendingTab, pendingView.HostTabTitle);
        }

        var scope = (pendingView ?? ActiveView)!.LogScope;

        // 注册日志来源标签 (主机名称+IP): 本连接所有日志在公用命令窗带此前缀,
        // 直接用 host 参数注册, 不等 HostTabTitle, 保证 LogConnecting 也带标签。
        TerminalLogService.RegisterScopeTitle(scope, HostTabNaming.Build(host.Name, host.Host));

        var session = new SftpSessionService
        {
            LogScope = scope,
            ConfirmHostKey = fp =>
            {
                TerminalLogService.Log(LocalizationService.GetString("LogWaitFingerprint"), scope);
                return HostKeyConfirmDialog.ShowFromWorkerAsync(this, host.Host, fp);
            },
            ConfirmHostKeyChange = (stored, presented) =>
            {
                TerminalLogService.Log(LocalizationService.GetString("LogWaitFingerprint"), scope);
                return HostKeyConfirmDialog.ShowChangeFromWorkerAsync(this, host.Host, stored, presented);
            },
        };
        session.SetTrustedFingerprint(string.IsNullOrWhiteSpace(host.HostKeyFingerprint)
            ? null
            : host.HostKeyFingerprint);

        TerminalLogService.SetBusy(true);
        TerminalLogService.Log(string.Format(
            LocalizationService.GetString("LogConnecting"), host.Host, host.Port), scope);
        try
        {
            if (!await session.ConnectAsync(host))
            {
                ReportConnectFailure(scope, pendingView, string.Empty);
                session.Dispose();
                return (false, null);
            }
        }
        catch (Exception ex)
        {
            ReportConnectFailure(scope, pendingView, ex.Message);
            session.Dispose();
            return (false, null);
        }
        finally
        {
            TerminalLogService.SetBusy(false);
        }

        // 连接成功: 占位 tab 就地接管, 复用路径由活动 tab 接管 (补主机标题)。
        var view = pendingView ?? ActiveView!;
        view.HostTabTitle = HostTabNaming.Build(host.Name, host.Host);
        view.AttachConnectedSession(session, host);
        SyncHostTabTitle(view);

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

    /// <summary>AddNewTab 的带返回值变体: 返回新建的 tab 与其承载的 FileView
    /// (tab-first 连接需要 tab 引用, 建出后立即写主机标题)。</summary>
    private (TabViewItem Tab, FileView View) AddNewTabGetView(bool remote)
    {
        AddNewTab(remote: remote);
        var tab = (TabViewItem)Strip.Tabs.SelectedItem;
        return (tab, (FileView)tab.Content);
    }

    /// <summary>连接失败落位: 命令窗日志 + 占位 tab 状态栏显示错误 (tab 保留,
    /// 供用户重试前查看原因); 复用路径 (无占位 tab) 仅写命令窗。</summary>
    private void ReportConnectFailure(string scope, FileView? view, string error)
    {
        var msg = string.Format(
            LocalizationService.GetString("LogConnectFailed"),
            string.IsNullOrEmpty(error)
                ? LocalizationService.GetString("LogConnectUnknown")
                : error);
        TerminalLogService.Log(msg, scope);
        view?.ShowConnectFailure(msg);
    }

    /// <summary>把视图的主机标题同步到承载它的 tab (成功落位后调用;
    /// 复用路径的活动 tab 标题在此时才从通用标题更新为主机标题)。</summary>
    private void SyncHostTabTitle(FileView view)
    {
        if (view.HostTabTitle is null)
        {
            return;
        }

        var tab = Strip.Tabs.TabItems.OfType<TabViewItem>()
            .FirstOrDefault(t => ReferenceEquals(t.Content, view));
        if (tab is not null)
        {
            Strip.SetTabTitle(tab, view.HostTabTitle);
        }
    }

    /// <summary>
    /// 关 tab 前确认: 本 tab 有活动传输任务时弹三选——后台继续（传输走独立连接,
    /// tab 关闭后照常跑完）/ 取消队列并关闭（取消本 tab 任务, 等块边界退出再关）/
    /// 不关了。无活动任务（或引导页）静默放行。
    /// </summary>
    private async Task<bool> OnTabCloseConfirming(TabViewItem tab)
    {
        // 对话框展示/等待期间忽略后续关闭请求（返回 false 防止并发关闭绕过确认）
        if (_confirmingOpen)
        {
            return false;
        }

        if (tab.Content is not FileView view
            || !TransferQueueService.HasActiveForScope(view.LogScope))
        {
            return true;
        }

        if (XamlRoot is null)
        {
            return true; // 未入树无法弹窗, 放行避免 tab 卡死关不掉
        }

        _confirmingOpen = true;
        try
        {
            var dialog = new ContentDialog
            {
                XamlRoot = XamlRoot,
                Title = LocalizationService.GetString("TabCloseTitle"),
                Content = LocalizationService.GetString("TabCloseContent"),
                PrimaryButtonText = LocalizationService.GetString("TabCloseBackground"),
                SecondaryButtonText = LocalizationService.GetString("TabCloseCancelQueue"),
                CloseButtonText = LocalizationService.GetString("TabCloseStay"),
                DefaultButton = ContentDialogButton.Close,
            };
            var result = await dialog.ShowAsync();
            if (result == ContentDialogResult.Primary)
            {
                return true; // 后台继续: 传输连接独立于标签页, 直接关
            }

            if (result == ContentDialogResult.Secondary)
            {
                // 只取消本 tab 的活动任务（不动其他 tab, 不动全局暂停状态）
                foreach (var task in TransferQueueService.Get(TransferQueueTab.Active))
                {
                    if ((task.Status is TransferStatus.Waiting or TransferStatus.Transferring)
                        && Array.IndexOf(task.LogScopes, view.LogScope) >= 0)
                    {
                        TransferQueueService.CancelTask(task.Id);
                    }
                }

                // 轮询等传输中任务到块边界退出（结算为"已取消"）; 超时兜底放行——
                // 传输连接独立存活, 迟到的结算照常落库, 关 tab 不受影响。
                bool Running() => TransferQueueService.Get(TransferQueueTab.Active).Any(t =>
                    (t.Status is TransferStatus.Waiting or TransferStatus.Transferring)
                    && Array.IndexOf(t.LogScopes, view.LogScope) >= 0);
                for (var i = 0; i < 50 && Running(); i++)
                {
                    await Task.Delay(100);
                }

                return true;
            }

            return false; // 不关了
        }
        finally
        {
            _confirmingOpen = false;
        }
    }

    /// <summary>关闭标签页: IdeTabStrip 已把标签移出, 此处释放内容资源。</summary>
    private void OnTabCloseRequested(TabViewItem tab)
    {
        if (tab.Content is FileView view)
        {
            // 先注销日志来源标签, 再销毁
            TerminalLogService.UnregisterScope(view.LogScope);
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
