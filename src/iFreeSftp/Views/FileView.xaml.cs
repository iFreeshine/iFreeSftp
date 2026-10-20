using System.Collections.ObjectModel;
using System.Collections.Concurrent;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.CompilerServices;
using System.Runtime.InteropServices;
using System.Security.Cryptography;
using System.Text;
using iFreeSftp.Models;
using iFreeSftp.Services;
using iFreeSftp.WpfEditor;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Vanara.PInvoke;
using Windows.Graphics.Imaging;
using static Vanara.PInvoke.Shell32;

namespace iFreeSftp.Views;

/// <summary>
/// 单个 FileView 的列状态: 列宽 / 回涨目标宽 / 列显隐 / 隐藏文件开关。
/// 每个窗格持有独立实例——曾为静态共享: 两窗格可用宽度不同, UpdateColumnFit
/// 的压缩与回涨经共享状态互相触发 (A 压完 B 回涨, B 再触发 A), 拖拽完全失效,
/// 无限互调可致 StackOverflow 崩溃。
/// </summary>
public sealed class FileColumnState
{
    public bool ShowSize = true;
    public bool ShowType = true;
    public bool ShowModified = true;
    public bool ShowPermissions = true; // 权限列仅远程 (Linux) 生效, 本地模式由 Mode 整列隐藏
    public bool ShowHidden; // 是否显示隐藏文件 (本地 Hidden 属性 / 远程 . 开头)

    // 可拖动调节的列宽 (像素, 拖动列头右缘手柄调整)。
    // 名称列范围更宽 (60~1200), 其余列 40~400。
    // 默认值按"常规窗格宽度 (~570 DIP)"校准: 本地四列 Σ=470 (需求 560≤570);
    // 远程多权限列 90 (drwxr-xr-x 10 字符实需 ~77), 需求 662 → 远程自适应等比压缩。
    public const double MinColWidth = 40;
    public const double MaxColWidth = 400;
    public const double MinNameColWidth = 60;
    public const double MaxNameColWidth = 1200;

    // 列模板几何常量: XAML 侧经 StaticResource 同值引用 (FileView.xaml 资源
    // IconCellSize/ColSpacing), 改这里不会自动同步 XAML——两处是同一来源的镜像,
    // 调整任一侧需同步另一侧 (WinUI XAML 不支持 x:Static, 只能如此镜像)。
    /// <summary>图标格边长: 条目模板与列头首列同宽。</summary>
    public const double IconCellSize = 22;
    /// <summary>条目/列头网格的列间距 (ColumnSpacing)。</summary>
    public const double ColSpacing = 12;
    /// <summary>条目/列头水平内边距 (单侧, Padding="4,x")。</summary>
    public const double RowPaddingH = 4;
    /// <summary>名称列宽计算固定开销 = 左右内边距 + 图标列 + 名称列前间距。</summary>
    public const double NameColFixedOverhead = RowPaddingH * 2 + IconCellSize + ColSpacing;

    public double NameColWidth = 160;
    public double SizeColWidth = 90;
    public double TypeColWidth = 100;
    public double ModifiedColWidth = 120;
    public double PermissionsColWidth = 90;

    // 列宽自适应的"回涨目标宽": 初始 = 默认列宽; 用户拖拽结束时记录为拖拽后的宽度。
    // 空间富余时低于目标宽的列回涨, 最多涨到目标值——用户拖窄/拖宽都是明确意图, 不被推翻。
    public double NameColDef = 160;
    public double SizeColDef = 90;
    public double TypeColDef = 100;
    public double ModifiedColDef = 120;
    public double PermissionsColDef = 90;

    /// <summary>本窗格列状态变化时触发 (本 FileView 订阅刷新自身列表与列头)。</summary>
    public event Action? Changed;

    public void RaiseChanged() => Changed?.Invoke();
}

/// <summary>
/// 单个文件视图：一个 Tab 页的内容。支持本地浏览与远程(SFTP)浏览。
/// 垂直 5 行结构: 面包屑路径 / 小图标工具栏 / 栏目名称 / 文件列表(含树形目录) / 状态统计栏,
/// 行间以 1px 细线分割。
/// </summary>
public sealed partial class FileView : UserControl, IDisposable, INotifyPropertyChanged
{
    /// <summary>当前模式（本地/远程）。</summary>
    public FileViewMode Mode
    {
        get => _mode;
        private set
        {
            if (_mode != value)
            {
                _mode = value;
                OnPropertyChanged();
                UpdateModeUI();
                ModeChanged?.Invoke(value);
            }
        }
    }
    private FileViewMode _mode = FileViewMode.Local;

    /// <summary>模式变化事件，用于更新所属 Tab 标题。</summary>
    public event Action<FileViewMode>? ModeChanged;

    /// <summary>
    /// 主机 tab 标题 ("名称 (IP)")。新建连接时先行落位 (连接成败均保留 tab),
    /// 之后语言切换/模式变化一律优先显示本标题; null = 非主机 tab, 用通用标题。
    /// </summary>
    public string? HostTabTitle { get; set; }

    /// <summary>连接失败落位: 错误信息显示到状态栏 (tab 保留, 供重试前查看原因)。</summary>
    public void ShowConnectFailure(string message) => UpdateStatus(message);

    /// <summary>同步导航请求: 本侧进入/返回目录成功且路径相对基准根变化后发出,
    /// 参数为本侧新目录绝对路径; 由 FolderContainer 转发给对侧做镜像导航。</summary>
    public event Action<string>? SyncNavigationRequested;

    /// <summary>当前目录绝对路径 (本地根视图为空串; 供同步匹配判定)。</summary>
    public string CurrentPath => _currentPath;

    /// <summary>同步浏览基准根: 本侧起始目录 (本地=主机本地目录, 远程=主机远程目录)。
    /// 空 = 本视图不参与同步联动 (未开启同步的主机或普通浏览)。</summary>
    public string SyncBasePath { get; private set; } = string.Empty;

    private readonly ObservableCollection<FileItemViewModel> _items = new();
    private SftpSessionService _sftp = new(); // 自带空会话; 主机连接成功后由 AttachConnectedSessionAsync 接管注入
    private string _currentPath = string.Empty;
    private string _lastSyncPath = string.Empty; // 最近联动导航路径: 路径未变化的重载/刷新不触发联动
    private bool _disposed;
    private bool _editingPath;
    private bool _layoutReady; // 构造完成后置位, 之前的布局回调不可安全访问全部命名元素

    /// <summary>本窗格列状态 (列宽/显隐/隐藏文件开关)。左右窗格独立, 互不联动。</summary>
    private readonly FileColumnState _cols = new();

    // —— 面包屑段数据 ——
    private readonly List<CrumbItem> _crumbs = new();

    // —— 排序状态 ——
    private enum SortKey { Name, Size, Type, Modified }
    private SortKey _sortKey = SortKey.Name;
    private bool _sortAsc = true;

    public event PropertyChangedEventHandler? PropertyChanged;

    public FileView(string paneName, bool emptyState = false, bool remoteMode = false)
    {
        PaneName = paneName;
        // 空状态在构造期即确定: 宿主 AddNewTab 创建后立即读取 IsEmptyState
        // 决定是否隐藏标签条, 不能等 Loaded (那时检查早已错过)。
        // 初始 tab 与"关闭最后一个 tab"的重建 tab 都以空态引导出现 (左右窗格皆可)。
        _emptyState = emptyState;
        // 远程初始模式 (为主机连接新建的 tab): 直接以远程空列表呈现, 不先加载本地。
        _mode = remoteMode ? FileViewMode.Remote : FileViewMode.Local;
        InitializeComponent();
        FileList.ItemsSource = _items;
        // 列显隐菜单与条目右键菜单同款样式 (主题化 presenter 背景, 见 PaneMenuPresenterStyle);
        // 打开时按当前主题重取, 避免构造期 ActualTheme 未定取错变体。
        ColMenuFlyout.Opening += (_, _) => ColMenuFlyout.MenuFlyoutPresenterStyle = PaneMenuPresenterStyle();
        // 树宽拖手: 拖动调整左侧树面板宽度。
        TreeSplitter.DragDelta += TreeSplitter_DragDelta;
        // 列显隐为本窗格独立状态: 变化时刷新自身列表与列头。
        _cols.Changed += OnColumnStateChanged;
        // 列表布局变化 (尺寸/滚动条/条目增删) 时重算列宽自适应与贯穿竖线位置。
        // 构造期 XAML 解析中途也可能触发布局, 用 _layoutReady 挡住 (StatusBar 等尚未存在);
        // 注意回调内不得更新其他 UI (如状态栏文本), 会引发 XAML 重入 failfast。
        FileList.LayoutUpdated += (_, _) =>
        {
            if (_layoutReady)
            {
                UpdateColumnFit();
                UpdateColumnLines();
            }
        };
        // 面包屑默认完整显示全路径, 仅在总宽超出面包屑行时从中间折叠出 "..";
        // 折叠方案依赖行宽, 行尺寸变化 (窗格缩放/DPI 变化) 时按当前 _crumbs 重建。
        BreadcrumbRow.SizeChanged += (_, _) =>
        {
            if (!_editingPath)
            {
                RebuildCrumbPanel();
            }
        };
        Loaded += OnLoaded;
        // 注意: 不可在 Unloaded 中 Dispose——WinUI 的 Unloaded 在模式切换/布局重排把
        // 视图短暂摘出可视树时也会触发 (并非只发生在销毁), 曾把刚注入的远程会话连同
        // 列表加载一并杀掉。释放只走 tab 关闭的显式 Dispose (FolderContainer)。
        _layoutReady = true; // 全部命名元素就绪, 布局回调从此生效
    }

    public string PaneName { get; }

    /// <summary>承载本视图的窗格容器 (FolderContainer.AddNewTab 注入), 经其 OppositePane 定位对侧视图。</summary>
    public FolderContainer? HostPane { get; set; }

    /// <summary>对侧窗格的活动视图 ("传输到另一侧"目标); 无窗格/对侧无标签时为 null。</summary>
    public FileView? OppositeView => HostPane?.OppositePane?.ActiveView;

    /// <summary>本 tab 日志池的 scope (每 tab 独立命令窗): 视图自身与承载会话的日志
    /// 只进本 tab 池; 跨窗格传输把自身与对侧的 scope 都发给 TerminalLogService (双发)。</summary>
    public string LogScope { get; } = Guid.NewGuid().ToString("N");

    /// <summary>跨窗格传输日志的 scope 组合: 自身 + 对侧 (两个 tab 都发, 保留完整上下文)。</summary>
    private string[] TransferScopes(FileView opp)
        => LogScope == opp.LogScope ? new[] { LogScope } : new[] { LogScope, opp.LogScope };

    /// <summary>本视图远程会话 (远程模式连接判断/传输用; 本地模式实例存在但未连接)。</summary>
    public SftpSessionService SftpService => _sftp;

    /// <summary>本地当前浏览目录 (根视图为空串); 远程模式返回 null (对侧校验用)。</summary>
    public string? CurrentDirectory => Mode == FileViewMode.Local ? _currentPath : null;

    /// <summary>请求打开"打开内容"选择器（空状态引导页"选择主机"按钮触发）。</summary>
    public event Action? OpenPickerRequested;

    /// <summary>空状态变化通知 (true=进入空态/引导, false=退出)。FolderContainer 用于切换标签条可见性。</summary>
    public event Action<bool>? EmptyStateChanged;

    /// <summary>是否处于初始引导空态（窗格初始 tab / 关闭最后一个 tab 后的重建 tab 为 true, 收起后不再回到 true）。</summary>
    public bool IsEmptyState => _emptyState;

    /// <summary>右窗口初始引导态: 覆盖层挡住文件区, 首次连接/模式切换后收起。</summary>
    private bool _emptyState;

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        // 初始化主题与本地化。
        LocalizationService.CultureChanged += OnCultureChanged;
        ThemeService.ThemeChanged += OnThemeChanged;
        // 框选 (rubber band selection): 列表空白处按住左键拖出选择框多选,
        // 本地/远程两侧列表共用同一实现 (文件列表宿主层统一挂接)。
        FileListHost.PointerPressed += OnRubberBandPointerPressed;
        FileListHost.PointerMoved += OnRubberBandPointerMoved;
        FileListHost.PointerReleased += OnRubberBandPointerReleased;
        FileListHost.PointerCanceled += OnRubberBandPointerReleased;
        ApplyResources();
        UpdateModeUI();
        // 默认本地模式进入本地浏览; 远程模式 (主机连接 tab) 保持空列表等连接结果,
        // 连接失败也不回退本地 (不擅自建立本地连接)。
        if (Mode == FileViewMode.Local)
        {
            NavigateLocalRoot();
        }
        if (_emptyState)
        {
            EmptyStatePanel.Visibility = Visibility.Visible;
            // 空态只显示引导页: 5 行结构中的前 4 行与状态栏整体隐藏。
            BreadcrumbRow.Visibility = Visibility.Collapsed;
            ToolbarRow.Visibility = Visibility.Collapsed;
            HeaderRow.Visibility = Visibility.Collapsed;
            StatusBar.Visibility = Visibility.Collapsed;
            // 引导内容块中心定位在容器高度 40% 处；首帧布局后执行一次, 之后随尺寸变化跟随。
            EmptyStatePanel.SizeChanged += (_, _) => UpdateEmptyStateLayout();
            DispatcherQueue.TryEnqueue(
                Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, UpdateEmptyStateLayout);
        }
    }

    /// <summary>引导内容块垂直定位: 顶部边距 = 容器高 × 40% - 内容半高。</summary>
    private void UpdateEmptyStateLayout()
        => EmptyContent.Margin = new Thickness(
            0, Math.Max(0, EmptyStatePanel.ActualHeight * 0.4 - EmptyContent.ActualHeight / 2), 0, 0);

    /// <summary>收起空状态引导覆盖层（连接主机 / 手动切换模式 / 选择本地文件夹时）。</summary>
    public void DismissEmptyState()
    {
        if (!_emptyState)
        {
            return;
        }

        _emptyState = false;
        EmptyStatePanel.Visibility = Visibility.Collapsed;
        BreadcrumbRow.Visibility = Visibility.Visible;
        ToolbarRow.Visibility = Visibility.Visible;
        HeaderRow.Visibility = Visibility.Visible;
        StatusBar.Visibility = Visibility.Visible;
        UpdateModeUI();
        UpdateColumnLines();
        EmptyStateChanged?.Invoke(false);
    }

    /// <summary>
    /// 外部入口: 本地视图跳转到指定目录 (同步浏览: 连接主机后对侧窗格按主机
    /// 本地目录初始定位)。空态引导就地转本地; 远程会话视图不劫持 (调用方先开
    /// 本地 tab); 目录不存在时 EnterLocalPath 仅状态栏提示, 不破坏当前视图。
    /// 目标目录记为本侧同步基准根。
    /// </summary>
    public void NavigateLocalTo(string path)
    {
        if (string.IsNullOrWhiteSpace(path))
        {
            return;
        }

        DismissEmptyState();
        if (Mode != FileViewMode.Local)
        {
            return;
        }

        try
        {
            var full = Path.GetFullPath(path);
            SyncBasePath = full.Length > 3 && full.EndsWith('\\') ? full.TrimEnd('\\') : full;
        }
        catch
        {
            // 路径非法不设基准 (保持原值), EnterLocalPath 会给出状态栏提示。
        }

        _lastSyncPath = _currentPath; // 初始定位本身不触发联动
        EnterLocalPath(path);
    }

    /// <summary>
    /// 对侧镜像导航入口 (同步浏览): 进入指定目录——本地路径先归一化并比较,
    /// 目标与当前相同则跳过 (幂等, 天然终止联动回环); 远程直接加载。
    /// 返回是否进入成功: 本地目录不存在 / 远程列举失败时为 false (留在原地),
    /// 调用方据此提示"目录不一致"并关闭联动。
    /// </summary>
    public async Task<bool> NavigateFromSyncAsync(string target)
    {
        if (string.IsNullOrWhiteSpace(target) || IsEmptyState)
        {
            return false;
        }

        if (Mode == FileViewMode.Local)
        {
            string norm;
            try
            {
                norm = Path.GetFullPath(target);
            }
            catch
            {
                return false;
            }

            if (_currentPath == norm)
            {
                return true; // 已在目标, 幂等成功
            }

            if (!Directory.Exists(norm))
            {
                return false; // 对侧目录不存在 → 两侧目录不一致
            }

            EnterLocalPath(norm);
            return true;
        }

        if (_sftp.IsConnected)
        {
            var t = target.Length == 0 ? "/" : target;
            if (_currentPath == t)
            {
                return true; // 已在目标, 幂等成功
            }

            var previous = _currentPath;
            _currentPath = t;
            if (!await LoadRemoteAsync())
            {
                // 加载失败回滚路径, "留在原地"名实相符——否则路径指向不存在的
                // 目录, 失效恢复 (RestoreBoth) 的刷新会再次加载幽灵路径再次失败,
                // 比对占位空行停留无法清除。
                _currentPath = previous;
                return false;
            }

            return true;
        }

        return false;
    }

    /// <summary>关闭本视图的同步联动 (目录不一致时由所属容器调用): 清空基准根。</summary>
    public void DisableSync()
    {
        SyncBasePath = string.Empty;
        _lastSyncPath = string.Empty;
    }

    /// <summary>
    /// 发出同步导航 (本地进目录/远程加载成功后调用): 先通知窗格状态变化
    /// (Header 重算同步按钮的匹配态——无论联动是否激活, 路径变化都影响匹配),
    /// 再在参与联动 (有基准根) 且路径相对上次导航变化 (刷新/幂等回环不触发)
    /// 时镜像对侧。
    /// </summary>
    private void NotifySyncNavigation()
    {
        HostPane?.NotifySyncStateChanged();
        if (SyncBasePath.Length == 0 || _currentPath == _lastSyncPath)
        {
            return;
        }

        _lastSyncPath = _currentPath;
        SyncNavigationRequested?.Invoke(_currentPath);
    }

    private void EmptyStateBtn_Click(object sender, RoutedEventArgs e)
        => OpenPickerRequested?.Invoke();

    // 比对高亮画刷走 XAML ThemeResource (主题感知自动变体), 主题切换无需代码重取。
    private void OnThemeChanged(bool isDark) => ApplyResources();

    /// <summary>图标按钮提示文本 (小图标工具栏无标签, 用气泡提示)。</summary>
    private static void SetTip(FrameworkElement element, string key)
        => SetTipText(element, LocalizationService.GetString(key));

    /// <summary>设置元素 tooltip (直接文本, 面包屑段显示截止到该项的完整路径用)。
    /// 同步设置 AutomationProperties.Name: 图标按钮无文本内容, 不补名读屏器报不出控件。
    /// </summary>
    private static void SetTipText(FrameworkElement element, string text)
    {
        element.SetValue(ToolTipService.ToolTipProperty, App.MakeAccentTip(text));
        element.SetValue(
            Microsoft.UI.Xaml.Automation.AutomationProperties.NameProperty, text);
    }

    /// <summary>重新应用本地化文本。</summary>
    public void ApplyResources()
    {
        SetTip(OpenDirBtn, "OpenDirectory");
        SetTip(UpBtn, "UpOneLevel");
        SetTip(RefreshBtn, "Refresh");
        SetTip(TreeToggleBtn, "ToggleTree");
        SetTip(NewFolderBtn, "NewFolder");
        SetTip(NewFileBtn, "NewFile");
        SetTip(ShowHiddenBtn, "ToggleHidden");
        ColName.Text = LocalizationService.GetString("ColName");
        ColSize.Text = LocalizationService.GetString("ColSize");
        ColType.Text = LocalizationService.GetString("ColType");
        ColModified.Text = LocalizationService.GetString("ColModified");
        ColPermissions.Text = LocalizationService.GetString("ColPermissions");
        MenuShowSize.Text = LocalizationService.GetString("ColSize");
        MenuShowType.Text = LocalizationService.GetString("ColType");
        MenuShowModified.Text = LocalizationService.GetString("ColModified");
        MenuShowPermissions.Text = LocalizationService.GetString("ColPermissions");
        EmptyStateTitle.Text = LocalizationService.GetString("EmptyStateTitle");
        EmptyStateMessage.Text = LocalizationService.GetString("EmptyStateMessage");
        EmptyStateBtnText.Text = LocalizationService.GetString("EmptyStateButton");
        PermissionOverlay.ApplyResources();
        ChecksumOverlay.ApplyResources();
        UpdateBreadcrumb();
        UpdateStatus();
    }

    /// <summary>根据模式更新 UI（按钮可用性、树根）。</summary>
    private void UpdateModeUI()
    {
        bool isRemote = Mode == FileViewMode.Remote;
        OpenDirBtn.IsEnabled = !isRemote; // 系统文件夹选择器只能浏览本地
        MenuShowPermissions.IsEnabled = isRemote; // 权限列仅远程 (Linux) 有意义
        RebuildTree();
        UpdateNewButtons();
        UpdateHeaderColumnVisibility(); // 权限列显隐随模式切换

        if (isRemote)
        {
            if (_sftp.IsConnected)
            {
                _ = LoadRemoteAsync();
            }
            else
            {
                _items.Clear();
                UpdateBreadcrumb();
                UpdateStatus();
            }
        }
        else
        {
            NavigateLocalRoot();
        }
    }

    /// <summary>远程编辑外部编辑器会话 (按远端完整路径索引): 检测到编辑器保存后询问回传。
    /// 同文件再次编辑先结束旧会话——旧 watcher 会把新一次下载误判为用户保存。</summary>
    private readonly Dictionary<string, Services.ExternalEditSession> _editSessions = new(StringComparer.Ordinal);

    /// <summary>本视图连接的主机配置 (AttachConnectedSession 时记录), 取主机级外部编辑器用。</summary>
    private HostProfile? _connectedHost;

    /// <summary>
    /// 接管一个已连接的会话并加载远程列表（FolderContainer.ConnectAndOpenAsync 在
    /// 连接成功后调用——只有连接成功才会构建/转换 tab 与远程容器）。会话与保活
    /// 归属本视图（关 tab 随 Dispose 释放）；同步浏览从主机远程目录开始, 否则登录主目录。
    /// </summary>
    public void AttachConnectedSession(SftpSessionService session, HostProfile host)
    {
        _connectedHost = host;
        _sftp.Dispose(); // 弃用自带的未使用会话 (未连接, 释放无害)
        _sftp = session;
        // 同步浏览: 主机开启同步且填了远程目录时, 从该目录开始浏览 (归一化为绝对
        // 路径, 根保留 "/"); 否则清空路径, 远程从登录主目录开始。
        var remoteStart = host.SyncEnabled ? host.RemoteSyncPath?.Trim() : null;
        if (!string.IsNullOrEmpty(remoteStart))
        {
            if (!remoteStart.StartsWith('/'))
            {
                remoteStart = "/" + remoteStart;
            }
            _currentPath = remoteStart == "/" ? "/" : remoteStart.TrimEnd('/');
        }
        else
        {
            _currentPath = string.Empty; // 就地转换时清除本地浏览残留路径
        }
        SyncBasePath = string.IsNullOrEmpty(remoteStart) ? string.Empty : _currentPath;
        _lastSyncPath = _currentPath; // 初始定位不触发联动 (对侧由 OpenLocalSyncView 定位)

        if (AppSettingsService.KeepAliveEnabled)
        {
            // 连接成功后启动保活, 维持长连接稳定性 (全局设置: 开关+间隔)
            session.KeepAliveInterval = TimeSpan.FromSeconds(AppSettingsService.KeepAliveIntervalSeconds);
            session.StartKeepAlive();
        }
        // 服务端哈希探测延迟到首个远程列表加载完成后触发 (见 LoadRemoteAsync),
        // 避免探测命令与首次列表加载抢通道
        _hashProbePending = true;
        TerminalLogService.Log(string.Format(
            LocalizationService.GetString("LogConnected"), host.Host), LogScope);
        // 先切远程再收引导: 空态 tab 就地转远程, 全程不经过本地列表;
        // 会话已连接, Mode 切换即经 UpdateModeUI 重建远程树根并加载列表。
        Mode = FileViewMode.Remote;
        DismissEmptyState();
        ApplyResources();
    }

    /// <summary>远程加载防重入标志 (连接后 UpdateModeUI 会双触发, 后续调用直接跳过)。</summary>
    private bool _loadingRemote;

    /// <summary>待探测服务端哈希: 连接置位, 首个远程列表显示完成后消费一次。</summary>
    private bool _hashProbePending;

    /// <summary>加载远程目录（fire-and-forget 调用亦有兜底: 失败显示到状态栏）。
    /// 返回是否加载成功 (供同步浏览镜像导航判定)。</summary>
    private async Task<bool> LoadRemoteAsync()
    {
        if (!_sftp.IsConnected || _loadingRemote)
        {
            return false;
        }

        _loadingRemote = true;
        try
        {
            _items.Clear();
            // 首次 (路径空) 取登录主目录, 之后面包屑/上级/条目导航都基于绝对路径。
            if (_currentPath is not { Length: > 0 })
            {
                _currentPath = await _sftp.GetWorkingDirectoryAsync();
            }

            var baseDir = _currentPath;
            var entries = await _sftp.ListDirectoryAsync(baseDir);
            foreach (var entry in entries)
            {
                // 远程以 . 开头视为隐藏文件 (dotfile), 由"显示隐藏文件"开关控制。
                if (!_cols.ShowHidden && entry.Name.StartsWith('.'))
                {
                    continue;
                }
                // 符号链接不显示 (如 / 下的 bin -> usr/bin), 与隐藏文件开关无关。
                if (entry.IsSymbolicLink)
                {
                    continue;
                }
                // 相对路径列举的 FullName 是 "./name" 形式, 统一归一化为绝对路径,
                // 保证双击进目录/面包屑/状态栏拿到的都是完整路径。
                if (!entry.FullPath.StartsWith('/'))
                {
                    entry.FullPath = baseDir.EndsWith('/')
                        ? baseDir + entry.Name
                        : baseDir + "/" + entry.Name;
                }
                _items.Add(ToViewModel(entry));
            }
            ApplySort();
            UpdateBreadcrumb();
            UpdateStatus();
            SyncTreeIfVisible(); // 列表→树同步: 高亮当前目录
            NotifySyncNavigation(); // 路径相对上次变化 (导航而非刷新) 时镜像对侧
            // 列表已显示: 此刻才做服务端哈希命令探测 (每次连接一次, 结果缓存进会话,
            // 供校验码菜单/弹窗过滤; fire-and-forget 不阻塞交互)
            if (_hashProbePending)
            {
                _hashProbePending = false;
                _ = _sftp.ProbeHashSupportAsync();
            }

            return true;
        }
        catch (Exception ex)
        {
            // 服务器无响应 (连接配额被占/sftp 子系统挂起) 时操作超时抛
            // SshOperationTimeoutException —— 原始消息无上下文, 换成带处置建议的提示
            var message = ex is Renci.SshNet.Common.SshOperationTimeoutException or TimeoutException
                ? LocalizationService.GetString("ErrRemoteTimeout")
                : ex.Message;
            UpdateStatus(message);
            // 报错与处置同步进命令行面板 (状态栏提示易被错过, 终端留存可回看)
            TerminalLogService.Log(message, LogScope);
            return false;
        }
        finally
        {
            _loadingRemote = false;
        }
    }

    /// <summary>进入本地根（驱动器列表）。</summary>
    private void NavigateLocalRoot()
    {
        _items.Clear();
        _currentPath = string.Empty;
        foreach (var drive in DriveInfo.GetDrives())
        {
            var entry = new FileEntry
            {
                Name = drive.Name,
                FullPath = drive.Name,
                IsDirectory = true,
                LastModified = DateTimeOffset.MinValue,
            };
            _items.Add(ToViewModel(entry));
        }
        ApplySort();
        UpdateBreadcrumb();
        UpdateStatus();
        UpdateNewButtons(); // 根视图 (驱动器列表) 下新建按钮不可用
        SyncTreeIfVisible(); // 根态: 清除树选中
        NotifySyncNavigation(); // 参与联动时回根即越出基准根, 由容器判定关闭联动
    }

    /// <summary>进入目录（本地或远程，文件列表条目入口）。</summary>
    private void EnterDirectory(FileItemViewModel item)
    {
        if (Mode == FileViewMode.Local)
        {
            EnterLocalDirectory(item);
        }
        else if (_sftp.IsConnected)
        {
            _currentPath = item.FullPath;
            _ = LoadRemoteAsync();
        }
    }

    private void EnterLocalDirectory(FileItemViewModel item)
    {
        if (!item.IsDirectory) return;
        try
        {
            _currentPath = item.FullPath;
            _items.Clear();
            var dir = new DirectoryInfo(item.FullPath);
            foreach (var sub in dir.EnumerateFileSystemInfos())
            {
                // 默认不显示隐藏文件 (Hidden 属性), 由工具栏"显示隐藏文件"开关控制。
                if (!_cols.ShowHidden && (sub.Attributes & FileAttributes.Hidden) != 0)
                {
                    continue;
                }
                // .lnk 快捷方式不显示 (junction/OneDrive 等 ReparsePoint 不受影响)。
                if (sub is FileInfo && sub.Extension.Equals(".lnk", StringComparison.OrdinalIgnoreCase))
                {
                    continue;
                }
                var entry = new FileEntry
                {
                    Name = sub.Name,
                    FullPath = sub.FullName,
                    IsDirectory = sub is DirectoryInfo || (sub.Attributes & FileAttributes.Directory) != 0,
                    Length = sub is FileInfo f ? f.Length : 0,
                    LastModified = sub.LastWriteTimeUtc,
                };
                // Unix 本机 (应用未来跨平台编译): 按链接目标/权限位填充, 供类型列识别
                // (Windows 本地恒 false, 走扩展名链; 远程由 SFTP 属性填充)。
                if (!OperatingSystem.IsWindows())
                {
                    entry.IsSymbolicLink = sub.LinkTarget is not null;
                    try
                    {
                        entry.IsExecutable = (File.GetUnixFileMode(sub.FullName)
                            & (UnixFileMode.UserExecute | UnixFileMode.GroupExecute | UnixFileMode.OtherExecute)) != 0;
                    }
                    catch
                    {
                        // 取不到权限位 (权限拒绝等) 时按不可执行, 走扩展名链。
                    }
                }
                _items.Add(ToViewModel(entry));
            }
            ApplySort();
            UpdateBreadcrumb();
            UpdateStatus();
            UpdateNewButtons(); // 进入具体目录后新建按钮可用
            SyncTreeIfVisible(); // 列表→树同步: 高亮当前目录
            NotifySyncNavigation();
        }
        catch (UnauthorizedAccessException)
        {
            UpdateStatus($"{item.Name}: access denied");
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>按路径进入本地目录（面包屑 / 编辑框 / 树 / 打开目录对话框共用）。</summary>
    private void EnterLocalPath(string path)
    {
        try
        {
            // "C:" 会被解析为驱动器当前目录, 显式补全为根 "C:\"。
            if (path.Length == 2 && path[1] == ':')
            {
                path += "\\";
            }
            var full = Path.GetFullPath(path);
            if (!Directory.Exists(full))
            {
                UpdateStatus(string.Format(LocalizationService.GetString("DirNotFound"), full));
                return;
            }
            var name = full.EndsWith('\\') ? full : Path.GetFileName(full);
            EnterLocalDirectory(new FileItemViewModel(new FileEntry
            {
                Name = name,
                FullPath = full,
                IsDirectory = true,
            }, isLocalPath: true, _cols));
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>上一级。</summary>
    private void UpBtn_Click(object sender, RoutedEventArgs e)
    {
        if (Mode == FileViewMode.Local)
        {
            if (string.IsNullOrEmpty(_currentPath)) return;
            var parent = Directory.GetParent(_currentPath)?.FullName;
            if (parent is null)
            {
                NavigateLocalRoot();
            }
            else
            {
                EnterLocalDirectory(new FileItemViewModel(new FileEntry
                {
                    Name = Path.GetFileName(parent),
                    FullPath = parent,
                    IsDirectory = true,
                }, isLocalPath: true, _cols));
            }
        }
        else if (_sftp.IsConnected && !string.IsNullOrEmpty(_currentPath))
        {
            // 远程是 Unix 路径, 不能走 Windows 的 Path.GetFullPath (会污染当前盘符):
            // 截到最后一个 '/', 已在根 '/' 时保持原位。
            var trimmed = _currentPath.TrimEnd('/');
            var idx = trimmed.LastIndexOf('/');
            _currentPath = idx <= 0 ? "/" : trimmed[..idx];
            _ = LoadRemoteAsync();
        }
    }

    /// <summary>打开目录: 弹出系统文件夹选择对话框 (Vanara IFileOpenDialog) 并跳转。</summary>
    private void OpenDirBtn_Click(object sender, RoutedEventArgs e)
    {
        if (Mode != FileViewMode.Local)
        {
            return;
        }
        var picked = PickLocalFolder();
        if (!string.IsNullOrEmpty(picked))
        {
            DismissEmptyState();
            EnterLocalPath(picked);
        }
    }

    /// <summary>系统文件夹选择对话框 (IFileOpenDialog + FOS_PICKFOLDERS), 取消返回 null。</summary>
    private string? PickLocalFolder()
    {
        try
        {
            var dlg = (IFileOpenDialog)new CFileOpenDialog();
            dlg.SetOptions(dlg.GetOptions()
                | FILEOPENDIALOGOPTIONS.FOS_PICKFOLDERS
                | FILEOPENDIALOGOPTIONS.FOS_FORCEFILESYSTEM);
            if (_currentPath is { Length: > 0 })
            {
                try
                {
                    if (SHCreateItemFromParsingName<IShellItem>(_currentPath, null) is { } folderItem)
                    {
                        dlg.SetFolder(folderItem);
                    }
                }
                catch
                {
                    // 起始目录解析失败时使用对话框默认位置
                }
            }
            var hwnd = (HWND)WinRT.Interop.WindowNative.GetWindowHandle(App.MainAppWindow);
            if (dlg.Show(hwnd).Failed)
            {
                return null; // 用户取消
            }
            return dlg.GetResult().GetDisplayName(SIGDN.SIGDN_FILESYSPATH);
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
            return null;
        }
    }

    /// <summary>刷新当前目录。</summary>
    private void RefreshBtn_Click(object sender, RoutedEventArgs e)
    {
        if (Mode == FileViewMode.Local)
        {
            if (string.IsNullOrEmpty(_currentPath)) NavigateLocalRoot();
            else EnterLocalPath(_currentPath);
        }
        else if (_sftp.IsConnected)
        {
            _ = LoadRemoteAsync();
        }
    }

    /// <summary>切换树形目录侧栏显隐。</summary>
    private void TreeToggleBtn_Click(object sender, RoutedEventArgs e)
    {
        bool show = TreeToggleBtn.IsChecked == true;
        DirTree.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        TreeSeparator.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        TreeSplitter.Visibility = show ? Visibility.Visible : Visibility.Collapsed;
        if (show)
        {
            RebuildTree();
            SyncTreeIfVisible(); // 打开即高亮当前目录
        }
    }

    /// <summary>树可见时把文件列表当前位置同步到树 (展开祖先链 + 选中)。</summary>
    private void SyncTreeIfVisible()
    {
        if (TreeToggleBtn.IsChecked == true && !_disposed)
        {
            _ = ExpandTreePathAsync(_currentPath, select: true, expandTarget: false, gen: _treeGen);
        }
    }

    /// <summary>
    /// 树可见时重建树并保留已展开路径, 最后选回当前目录
    /// (刷新 / 新建 / 重命名 / 删除 / 隐藏开关切换后, 经 RefreshListKeepingPath 调用)。
    /// </summary>
    private void RefreshTreeIfVisible()
    {
        if (TreeToggleBtn.IsChecked == true && !_disposed)
        {
            _treeGen++;
            _ = RebuildTreePreservingAsync(_treeGen);
        }
    }

    private async Task RebuildTreePreservingAsync(int gen)
    {
        // 记录已展开节点路径 (含根), 重建后逐条重新展开
        var expanded = new List<string>();
        void Walk(IList<TreeViewNode> nodes)
        {
            foreach (var n in nodes)
            {
                if (n.Content is TreeDir d)
                {
                    if (n.IsExpanded)
                    {
                        expanded.Add(d.FullPath);
                    }
                    Walk(n.Children);
                }
            }
        }
        Walk(DirTree.RootNodes);
        RebuildTree();
        foreach (var path in expanded)
        {
            if (_disposed || gen != _treeGen)
            {
                return;
            }
            await ExpandTreePathAsync(path, select: false, expandTarget: true, gen: gen);
        }
        if (!_disposed && gen == _treeGen && _currentPath is { Length: > 0 })
        {
            await ExpandTreePathAsync(_currentPath, select: true, expandTarget: false, gen: gen);
        }
    }

    /// <summary>
    /// 逐层展开树到指定目录 (列表→树同步)。懒加载节点需等待 Expanding 填充完成再深入;
    /// 断链 (目录不存在/无权限/超时) 停在最近可达层。gen 丢弃被后续树重建作废的同步链。
    /// </summary>
    private async Task ExpandTreePathAsync(string path, bool select, bool expandTarget, int gen)
    {
        if (_disposed)
        {
            return;
        }
        if (path is not { Length: > 0 })
        {
            DirTree.SelectedNode = null; // 根态 (本地驱动器列表): 清除选中
            return;
        }
        var isLocal = Mode == FileViewMode.Local;
        TreeViewNode? node;
        string remain;
        if (isLocal)
        {
            var rootKey = path.Length >= 3 && path[1] == ':' ? path[..3] : path;
            node = FindTreeChild(DirTree.RootNodes, rootKey, isLocal);
            remain = path.Length > rootKey.Length ? path[rootKey.Length..] : string.Empty;
        }
        else
        {
            node = FindTreeChild(DirTree.RootNodes, "/", isLocal);
            remain = path == "/" ? string.Empty : path.TrimStart('/');
        }
        if (node is null)
        {
            return;
        }
        while (remain.Length > 0)
        {
            var idx = remain.IndexOf(isLocal ? '\\' : '/');
            string seg;
            if (idx < 0)
            {
                seg = remain;
                remain = string.Empty;
            }
            else
            {
                seg = remain[..idx];
                remain = remain[(idx + 1)..];
                if (seg.Length == 0)
                {
                    continue;
                }
            }
            if (!await WaitForTreeChildrenAsync(node) || _disposed || gen != _treeGen)
            {
                return;
            }
            if (node.Content is not TreeDir parentDir)
            {
                return;
            }
            var full = isLocal
                ? (parentDir.FullPath.EndsWith('\\') ? parentDir.FullPath + seg : parentDir.FullPath + "\\" + seg)
                : (parentDir.FullPath.EndsWith('/') ? parentDir.FullPath + seg : parentDir.FullPath + "/" + seg);
            var next = FindTreeChild(node.Children, full, isLocal);
            if (next is null)
            {
                break; // 断链: 停在最近可达节点
            }
            node = next;
        }
        if (_disposed || gen != _treeGen)
        {
            return;
        }
        if (expandTarget)
        {
            await WaitForTreeChildrenAsync(node);
        }
        if (select && gen == _treeGen)
        {
            DirTree.SelectedNode = node;
        }
    }

    /// <summary>
    /// 触发懒加载并等待 Expanding 填充真正完成 (3 秒超时按断链处理)。
    /// Expanding 处理器在枚举前就清掉 HasUnrealizedChildren (防重入), 所以"加载中"
    /// 要看 _expanding 集合; 两者都就绪才算完成, 否则深层路径同步会在空 Children 上断链。
    /// 另: RebuildTree 刚结束时容器尚未生成, 直接赋 IsExpanded=true 会被 WinUI 吞掉
    /// (不触发 Expanding), 检测到毫无动静就重新触发 (先折叠再展开, 避免同值赋值不走变更)。
    /// </summary>
    private async Task<bool> WaitForTreeChildrenAsync(TreeViewNode node)
    {
        if (!node.HasUnrealizedChildren && !_expanding.Contains(node))
        {
            return true;
        }
        if (node.HasUnrealizedChildren)
        {
            node.IsExpanded = true; // 触发 DirTree_Expanding 懒加载
        }
        for (var i = 0; i < 60 && (_expanding.Contains(node) || node.HasUnrealizedChildren); i++)
        {
            await Task.Delay(50);
            if (i % 4 == 3 && node.HasUnrealizedChildren && !_expanding.Contains(node))
            {
                node.IsExpanded = false;
                node.IsExpanded = true;
            }
        }
        return !node.HasUnrealizedChildren && !_expanding.Contains(node);
    }

    private static TreeViewNode? FindTreeChild(IList<TreeViewNode> nodes, string fullPath, bool isLocal)
    {
        foreach (var n in nodes)
        {
            if (n.Content is TreeDir d
                && (isLocal
                    ? string.Equals(d.FullPath.TrimEnd('\\'), fullPath.TrimEnd('\\'), StringComparison.OrdinalIgnoreCase)
                    : string.Equals(d.FullPath, fullPath, StringComparison.Ordinal)))
            {
                return n;
            }
        }
        return null;
    }

    /// <summary>拖动树宽拖手: 调整树面板宽度 (120~500)。</summary>
    private void TreeSplitter_DragDelta(double dx, double dy)
    {
        if (double.IsNaN(DirTree.Width))
        {
            DirTree.Width = 200;
        }
        DirTree.Width = Math.Clamp(DirTree.Width + dx, 120, 500);
    }

    /// <summary>切换显示隐藏文件 (本地 Hidden 属性 / 远程 . 开头, 本窗格重载)。</summary>
    private void ShowHiddenBtn_Click(object sender, RoutedEventArgs e)
    {
        _cols.ShowHidden = ShowHiddenBtn.IsChecked == true;
        _cols.RaiseChanged();
    }

    // ───────────────────────── 面包屑路径行 ─────────────────────────

    /// <summary>重建面包屑: 根段(此电脑//) + 逐级路径段, 最后一段高亮。</summary>
    private void UpdateBreadcrumb()
    {
        if (_editingPath)
        {
            return; // 编辑态不重建, 避免打断输入
        }
        _crumbs.Clear();
        if (Mode == FileViewMode.Local)
        {
            _crumbs.Add(new CrumbItem(LocalizationService.GetString("ThisPC"), string.Empty,
                _currentPath.Length == 0));
            if (_currentPath.Length > 0)
            {
                var root = Path.GetPathRoot(_currentPath) ?? string.Empty;
                if (root.Length > 0)
                {
                    _crumbs.Add(new CrumbItem(root.EndsWith('\\') ? root[..^1] : root, root,
                        _currentPath == root));
                }
                var acc = root;
                var rel = _currentPath.Length > root.Length
                    ? _currentPath[root.Length..].Trim('\\')
                    : string.Empty;
                if (rel.Length > 0)
                {
                    foreach (var part in rel.Split('\\'))
                    {
                        acc = Path.Combine(acc, part);
                        _crumbs.Add(new CrumbItem(part, acc, acc == _currentPath));
                    }
                }
            }
        }
        else
        {
            // 远程: 以 / 为根逐段拆分
            _crumbs.Add(new CrumbItem("/", "/", _currentPath is { Length: 0 } or "/"));
            if (_currentPath is { Length: > 1 })
            {
                var acc = string.Empty;
                foreach (var part in _currentPath.Split('/', StringSplitOptions.RemoveEmptyEntries))
                {
                    acc += "/" + part;
                    _crumbs.Add(new CrumbItem(part, acc, acc == _currentPath));
                }
            }
        }
        RebuildCrumbPanel();
    }

    /// <summary>
    /// 重建面包屑面板: 默认完整显示 根段(home 图标) + 全部路径段 (段间分隔线);
    /// 仅当总宽超出面包屑行可用宽度时, 才从目录链中间开始折叠出 ".." 按钮
    /// (保留根段与最后段, 折叠段进 ".." 下拉菜单), 折叠数量取能放下的最小值。
    /// 全部折叠仍放不下时维持 home &gt; .. &gt; 最后段, 单段超长由 MaxWidth 截断。
    /// DPI: 全程 DIP 域运算 (ActualWidth/DesiredSize 与布局值同域), 与缩放比无关。
    /// </summary>
    private void RebuildCrumbPanel()
    {
        BreadcrumbPanel.Children.Clear();
        if (_crumbs.Count == 0)
        {
            return;
        }
        int n = _crumbs.Count;
        // 预生成全部段按钮并测量理想宽 (未入树的元素 Measure 即可得期望宽)。
        var buttons = new Button[n];
        var widths = new double[n];
        double full = 0;
        for (int k = 0; k < n; k++)
        {
            buttons[k] = CreateCrumbButton(_crumbs[k]);
            widths[k] = MeasureWidth(buttons[k]);
            full += widths[k];
        }
        double sepW = MeasureWidth(CreateSeparator());
        double ovW = MeasureWidth(new Button
        {
            Style = (Style)Resources["CrumbButtonStyle"],
            Content = new TextBlock { Text = "..", Style = (Style)Resources["CrumbTextLastStyle"] },
        });
        int seps = n - 1;
        full += seps * sepW + 2.0 * (n + seps - 1); // StackPanel Spacing=2: 相邻子元素间 2
        // 可用宽 = 面包屑行宽 - 行内 Grid 左右 Padding (6+6); 首帧未布局时先全量显示,
        // 行 SizeChanged 后会重建收敛。
        double avail = BreadcrumbRow.ActualWidth - 12;
        if (n <= 2 || avail <= 0 || full <= avail)
        {
            // 放得下 (或仅根+一段无从折叠): 完整显示, 不出现 ".."。
            AddCrumbInline(buttons[0]);
            for (int k = 1; k < n; k++)
            {
                AddCrumbInline(CreateSeparator());
                AddCrumbInline(buttons[k]);
            }
            return;
        }
        // 从目录链中间开始折叠: 中间段 c1..c_{n-2}, 折叠窗口自中间向两侧扩张,
        // 取"能放下的最小折叠数"; 全部折叠仍放不下则取全折叠兜底。
        int midCount = n - 2;
        int foldFrom = 1, foldTo = n - 2; // 兜底: home > .. > 最后段
        for (int cnt = 1; cnt <= midCount; cnt++)
        {
            int i = 1 + (midCount - cnt) / 2; // 折叠窗口在中间段内居中
            int j = i + cnt - 1;
            if (CollapsedCrumbWidth(widths, i, j, sepW, ovW) <= avail)
            {
                foldFrom = i;
                foldTo = j;
                break;
            }
        }
        // 组装: 根段..c_{foldFrom-1} | ".." (下拉含被折叠段) | c_{foldTo+1}..最后段。
        AddCrumbInline(buttons[0]);
        for (int k = 1; k < foldFrom; k++)
        {
            AddCrumbInline(CreateSeparator());
            AddCrumbInline(buttons[k]);
        }
        AddCrumbOverflowButton(_crumbs.GetRange(foldFrom, foldTo - foldFrom + 1)); // 自带前置分隔线
        for (int k = foldTo + 1; k < n; k++)
        {
            AddCrumbInline(CreateSeparator());
            AddCrumbInline(buttons[k]);
        }
    }

    /// <summary>折叠方案 [i..j] (含) 的理想总宽: 保留段 + ".." + 分隔线 + 间距。</summary>
    private static double CollapsedCrumbWidth(double[] widths, int i, int j, double sepW, double ovW)
    {
        int n = widths.Length;
        int visButtons = i + 1 + (n - 1 - j); // 根..c_{i-1} + ".." + c_{j+1}..最后段
        int seps = visButtons - 1;
        double sum = ovW;
        for (int k = 0; k < i; k++)
        {
            sum += widths[k];
        }
        for (int k = j + 1; k < n; k++)
        {
            sum += widths[k];
        }
        return sum + seps * sepW + 2.0 * (visButtons + seps - 1);
    }

    private static double MeasureWidth(FrameworkElement element)
    {
        element.Measure(new Windows.Foundation.Size(double.PositiveInfinity, double.PositiveInfinity));
        return element.DesiredSize.Width;
    }

    private FontIcon CreateSeparator()
        => new() { Style = (Style)Resources["CrumbSeparatorStyle"] };

    private void AddCrumbInline(FrameworkElement element)
        => BreadcrumbPanel.Children.Add(element);

    private Button CreateCrumbButton(CrumbItem seg)
    {
        // 根段 (此电脑 / 远程根, Path 为空) 一律用 Home 图标替代文字, 其余段显示文本。
        bool isRoot = seg.Path.Length == 0;
        var btn = new Button
        {
            Style = (Style)Resources["CrumbButtonStyle"],
            Tag = seg.Path,
            Content = isRoot
                ? new FontIcon { Style = (Style)Resources[seg.Last ? "CrumbHomeIconLastStyle" : "CrumbHomeIconStyle"] }
                : new TextBlock
                {
                    Text = seg.Text,
                    Style = (Style)Resources[seg.Last ? "CrumbTextLastStyle" : "CrumbTextDefaultStyle"],
                },
        };
        btn.Click += Crumb_Click;
        // hover 提示 = 截止到该段的完整路径; 根段 (此电脑, Path 为空) 显示"此电脑"。
        SetTipText(btn, isRoot ? LocalizationService.GetString("ThisPC") : seg.Path);
        return btn;
    }

    /// <summary>".." 按钮: 中间层级折叠段, 点击弹出下拉导航菜单。</summary>
    private void AddCrumbOverflowButton(List<CrumbItem> hidden)
    {
        AddCrumbInline(CreateSeparator());
        var flyout = new MenuFlyout();
        foreach (var seg in hidden)
        {
            var item = new MenuFlyoutItem { Text = seg.Text };
            var path = seg.Path;
            item.Click += (_, _) => NavigateTo(path);
            flyout.Items.Add(item);
        }
        AddCrumbInline(new Button
        {
            Style = (Style)Resources["CrumbButtonStyle"],
            // ".." 字样醒目 (主色) 表示可点开中间层级
            Content = new TextBlock { Text = "..", Style = (Style)Resources["CrumbTextLastStyle"] },
            Flyout = flyout, // ButtonBase 点击自动弹出
        });
    }

    private void Crumb_Click(object sender, RoutedEventArgs e)
    {
        if (sender is Button { Tag: string target })
        {
            NavigateTo(target);
        }
    }

    /// <summary>按目标路径跳转 (面包屑段点击)。</summary>
    private void NavigateTo(string target)
    {
        if (Mode == FileViewMode.Local)
        {
            if (target.Length == 0)
            {
                NavigateLocalRoot();
            }
            else
            {
                EnterLocalPath(target);
            }
        }
        else if (_sftp.IsConnected)
        {
            _currentPath = target.Length == 0 ? "/" : target;
            _ = LoadRemoteAsync();
        }
    }

    /// <summary>点击面包屑空白区 → 切换为输入框编辑。</summary>
    private void Breadcrumb_Tapped(object sender, TappedRoutedEventArgs e) => BeginPathEdit();

    private void BeginPathEdit()
    {
        _editingPath = true;
        // 编辑框预填完整路径; 本地根(驱动器列表)预填空并显示"路径"占位。
        PathEditBox.Text = Mode == FileViewMode.Local
            ? _currentPath
            : (_currentPath.Length == 0 ? "/" : _currentPath);
        BreadcrumbPanel.Visibility = Visibility.Collapsed;
        PathEditBox.Visibility = Visibility.Visible;
        PathEditBox.Focus(FocusState.Programmatic);
        PathEditBox.SelectAll();
    }

    private void EndPathEdit()
    {
        _editingPath = false;
        PathEditBox.Visibility = Visibility.Collapsed;
        BreadcrumbPanel.Visibility = Visibility.Visible;
    }

    private void PathEditBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Enter)
        {
            e.Handled = true;
            var text = PathEditBox.Text.Trim();
            EndPathEdit();
            if (text.Length == 0)
            {
                return;
            }
            if (Mode == FileViewMode.Local)
            {
                EnterLocalPath(text);
            }
            else if (_sftp.IsConnected)
            {
                _currentPath = text;
                _ = LoadRemoteAsync();
            }
        }
        else if (e.Key == Windows.System.VirtualKey.Escape)
        {
            e.Handled = true;
            EndPathEdit();
        }
    }

    // 回车/ESC 提交后不主动挪键盘焦点: 折叠编辑框引发的系统焦点回收即使
    // 落在工具栏按钮上, 也已由 ToolButtonStyle 的 UseSystemFocusVisuals=False
    // 抑制白框, 视觉上焦点"哪里都没去"。

    private void PathEditBox_LostFocus(object sender, RoutedEventArgs e)
    {
        if (_editingPath)
        {
            EndPathEdit();
        }
    }

    // ───────────────────────── 排序与列显隐 ─────────────────────────

    /// <summary>对当前列表按 (目录优先, 所选键) 重新排序。</summary>
    private void ApplySort()
    {
        // 比对模式: 列表按两侧合并名清单重排 (占位行对齐 + 行高亮), 不做常规排序。
        // 挂钩点覆盖所有列表加载路径 (远程/本地/根/刷新) 与列头点击。
        if (FileCompareService.Active)
        {
            FileCompareService.ApplyBoth();
            return;
        }

        var ordered = _items.OrderByDescending(i => i.IsDirectory);
        ordered = _sortKey switch
        {
            SortKey.Name => _sortAsc
                ? ordered.ThenBy(i => i.Name, StringComparer.OrdinalIgnoreCase)
                : ordered.ThenByDescending(i => i.Name, StringComparer.OrdinalIgnoreCase),
            SortKey.Size => _sortAsc ? ordered.ThenBy(i => i.Size) : ordered.ThenByDescending(i => i.Size),
            SortKey.Type => _sortAsc
                ? ordered.ThenBy(i => i.TypeText, StringComparer.OrdinalIgnoreCase)
                : ordered.ThenByDescending(i => i.TypeText, StringComparer.OrdinalIgnoreCase),
            _ => _sortAsc ? ordered.ThenBy(i => i.Modified) : ordered.ThenByDescending(i => i.Modified),
        };
        var sorted = ordered.ToList();
        _items.Clear();
        foreach (var item in sorted)
        {
            _items.Add(item);
        }
        UpdateSortArrows();
    }

    // ───────────────────────── 文件比对 (与同步浏览联动) ─────────────────────────

    /// <summary>
    /// 比对模式重排本侧列表 (由 FileCompareService.ApplyBoth 驱动, 两窗格各一次):
    /// 取两侧条目快照求合并名清单, 按 "目录优先 + 名称" 排序——两侧用同一算法、
    /// 同一名集, 行序一致, 行 i 两两配对。本侧有 → 条目行, 缺 → 占位空行。
    /// 高亮 (仅文件, 目录参与配对但不染色): 单侧独有 → Caution; 两侧都有但
    /// 时间或大小不同 → Critical。
    /// </summary>
    internal void ApplyCompareLayout()
    {
        if (_disposed || _emptyState)
        {
            return;
        }

        var opposite = OppositeView;
        if (opposite is null || opposite.IsEmptyState)
        {
            return; // 对侧无活动视图无从比对 (同步点亮前提下两侧必有视图)
        }

        var mine = SnapshotCompareEntries();
        var others = opposite.SnapshotCompareEntries();

        // 合并名清单: 并集 (大小写不敏感, 两侧同口径) + 目录优先 + 名称序。
        // 目录标记取两侧任一侧 (若一侧是目录一侧是文件, 按目录处理: 不染色)。
        var names = new HashSet<string>(mine.Keys, StringComparer.OrdinalIgnoreCase);
        names.UnionWith(others.Keys);
        var merged = names
            .Select(n => (Name: n, IsDir: (mine.TryGetValue(n, out var a) && a.IsDirectory)
                                       || (others.TryGetValue(n, out var b) && b.IsDirectory)))
            .OrderByDescending(x => x.IsDir)
            .ThenBy(x => x.Name, StringComparer.OrdinalIgnoreCase);

        // 复用现有 VM (保系统图标/列状态); 快照经 VM.Entry 而来, 键即名称。
        var reused = new Dictionary<string, FileItemViewModel>(StringComparer.OrdinalIgnoreCase);
        foreach (var vm in _items)
        {
            if (!vm.IsPlaceholder && !reused.ContainsKey(vm.Name))
            {
                reused[vm.Name] = vm;
            }
        }

        _items.Clear();
        foreach (var (name, isDir) in merged)
        {
            FileItemViewModel vm;
            FileCompareState state;
            if (mine.TryGetValue(name, out var entry))
            {
                vm = reused.TryGetValue(name, out var exist) ? exist : ToViewModel(entry);
                state = !isDir
                    ? (!others.TryGetValue(name, out var other)
                        ? FileCompareState.Caution // 仅本侧有 (对侧为占位空行)
                        : FilesDiffer(entry, other) ? FileCompareState.Critical : FileCompareState.None)
                    : FileCompareState.None;
            }
            else
            {
                vm = FileItemViewModel.CreatePlaceholder(_cols); // 本侧缺: 空行对齐对侧
                state = FileCompareState.None;
            }
            vm.SetCompareState(state);
            _items.Add(vm);
        }
        UpdateStatus();
    }

    /// <summary>
    /// 比对熄灭后就地恢复常规布局 (FileCompareService.RestoreBoth 驱动): 剔除占位
    /// 行 + 清比对高亮 + 常规排序——重排只是改变 _items 顺序并插入占位 VM, 各侧
    /// 真实条目数据仍在, 无需重新加载。关键优势: 失效复位往往发生在某侧加载完成
    /// 的回调链内 (失配由导航触发), 该侧 _loadingRemote 仍为 true, 重新加载会被
    /// 防重入标志静默吞掉 (实测占位空行停留的根因); 就地重排零网络请求、同步生效。
    /// </summary>
    internal void RestoreNormalLayout()
    {
        if (_disposed || _emptyState)
        {
            return;
        }

        var keep = _items.Where(i => !i.IsPlaceholder).ToList();
        foreach (var vm in keep)
        {
            vm.SetCompareState(FileCompareState.None); // 清 Caution/Critical 高亮
        }

        _items.Clear();
        foreach (var vm in keep)
        {
            _items.Add(vm);
        }

        ApplySort(); // 比对此时已熄灭 → 走常规排序
        UpdateStatus();
    }

    /// <summary>比对快照: 名称 → 条目 (大小写不敏感; 占位行不参与)。</summary>
    internal Dictionary<string, FileEntry> SnapshotCompareEntries()
        => _items.Where(i => !i.IsPlaceholder)
                 .GroupBy(i => i.Name, StringComparer.OrdinalIgnoreCase)
                 .ToDictionary(g => g.Key, g => g.First().Entry, StringComparer.OrdinalIgnoreCase);

    /// <summary>条目比对: 大小不同, 或修改时间不同 (截断到秒, SFTP mtime 仅秒级精度) 即不同。</summary>
    private static bool FilesDiffer(FileEntry a, FileEntry b)
        => a.Length != b.Length
           || TruncSeconds(a.LastModified.UtcDateTime) != TruncSeconds(b.LastModified.UtcDateTime);

    private static DateTime TruncSeconds(DateTime t)
        => new(t.Year, t.Month, t.Day, t.Hour, t.Minute, t.Second, t.Kind);

    private void UpdateSortArrows()
    {
        // 排序指示用细线 chevron (Segoe Fluent Icons: E70E=ChevronUp / E70D=ChevronDown),
        // 避免实心三角 (\u25B2) 的块状观感。
        SortNameArrow.Text = _sortKey == SortKey.Name ? (_sortAsc ? "\uE70E" : "\uE70D") : string.Empty;
        SortSizeArrow.Text = _sortKey == SortKey.Size ? (_sortAsc ? "\uE70E" : "\uE70D") : string.Empty;
        SortTypeArrow.Text = _sortKey == SortKey.Type ? (_sortAsc ? "\uE70E" : "\uE70D") : string.Empty;
        SortModifiedArrow.Text = _sortKey == SortKey.Modified ? (_sortAsc ? "\uE70E" : "\uE70D") : string.Empty;
    }

    /// <summary>列头点击: 同列切换升降序, 换列默认升序。</summary>
    private void HeaderSortClick(SortKey key)
    {
        if (_sortKey == key)
        {
            _sortAsc = !_sortAsc;
        }
        else
        {
            _sortKey = key;
            _sortAsc = true;
        }
        ApplySort();
    }

    private void SortName_Tapped(object sender, TappedRoutedEventArgs e) => HeaderSortClick(SortKey.Name);
    private void SortSize_Tapped(object sender, TappedRoutedEventArgs e) => HeaderSortClick(SortKey.Size);
    private void SortType_Tapped(object sender, TappedRoutedEventArgs e) => HeaderSortClick(SortKey.Type);
    private void SortModified_Tapped(object sender, TappedRoutedEventArgs e) => HeaderSortClick(SortKey.Modified);

    /// <summary>列显隐/列宽/隐藏文件 开关变化: 同步列头单元格、全部条目单元格与列表过滤。</summary>
    private void OnColumnStateChanged()
    {
        if (_disposed)
        {
            return;
        }
        MenuShowSize.IsChecked = _cols.ShowSize;
        MenuShowType.IsChecked = _cols.ShowType;
        MenuShowModified.IsChecked = _cols.ShowModified;
        MenuShowPermissions.IsChecked = _cols.ShowPermissions;
        UpdateHeaderColumnVisibility();
        // 列头列宽经 x:Bind 到下面几个属性, 这里通知刷新。
        OnPropertyChanged(nameof(NameColWidth));
        OnPropertyChanged(nameof(SizeColWidth));
        OnPropertyChanged(nameof(TypeColWidth));
        OnPropertyChanged(nameof(ModifiedColWidth));
        OnPropertyChanged(nameof(PermissionsColWidth));
        foreach (var item in _items)
        {
            item.RefreshColumnVisibility();
        }
        UpdateColumnLines();
        // 列宽/显隐变化 → 重算列宽自适应 (超出可用宽度时等比压缩其他列到最小宽度兜底)。
        UpdateColumnFit();
        // "显示隐藏文件"开关变化 → 按当前路径重新过滤加载列表。
        if (_cols.ShowHidden != _appliedShowHidden)
        {
            _appliedShowHidden = _cols.ShowHidden;
            RefreshListKeepingPath();
        }
    }

    /// <summary>保持当前路径重新加载列表 (显隐开关切换 / 新建文件后刷新共用; 对侧复制完成后也调用)。</summary>
    public void RefreshListKeepingPath()
    {
        RefreshTreeIfVisible(); // 树内容可能过期 (新建/删除目录), 重建并保留展开状态
        if (Mode == FileViewMode.Local)
        {
            if (string.IsNullOrEmpty(_currentPath))
            {
                NavigateLocalRoot();
            }
            else
            {
                EnterLocalPath(_currentPath);
            }
        }
        else if (_sftp.IsConnected)
        {
            _ = LoadRemoteAsync();
        }
    }

    // —— 列宽拖动 (Thumb Tag 指明所调节的列) ——

    private bool _appliedShowHidden;

    /// <summary>列头暴露给 x:Bind 的列宽 (转发行模板同一状态)。</summary>
    public double NameColWidth => _cols.NameColWidth;
    public double SizeColWidth => _cols.SizeColWidth;
    public double TypeColWidth => _cols.TypeColWidth;
    public double ModifiedColWidth => _cols.ModifiedColWidth;
    public double PermissionsColWidth => _cols.PermissionsColWidth;

    // —— 列宽动态自适应 (超出时等比压缩 / 富余时回涨恢复, 每列保底最小宽度, 任何列不消失) ——
    // 旧版"整列舍弃"auto-hide (类型→修改时间→大小) 已废弃: 拖宽名称列时类型列最先
    // 消失, 不符合预期。现改为: 超出量由列按可压缩余量等比分摊——拖拽中其他列先被
    // 挤压 (各自到最小列宽为止), 拖拽列最后被压卡在上限; 全部压到最小值仍放不下
    // (窗格极窄) 时保持最小宽度, 允许尾部溢出裁剪, 列依旧不隐藏。
    // 反向恢复: 空间富余 (先窄后宽 / 此前被拖拽挤压) 时, 低于目标宽的列等比回涨到
    // 目标宽为止——保证空间充足时列宽完整显示, 不停在历史挤压宽度; 目标宽初始为
    // 默认列宽, 用户拖拽结束即记录为该列新目标 (用户拖窄不会被回涨推翻)。
    // DPI/分辨率: 全程 DIP 域运算 (ActualWidth 与列宽同为逻辑像素), 任意缩放比下
    // 行为一致, 无需按 DPI 换算。

    /// <summary>拖拽中的列标识 (ColumnThumb_DragDelta 置位, DragCompleted 清除); null=无拖拽。</summary>
    private string? _draggingCol;

    /// <summary>
    /// 列宽动态自适应: 可见列总宽超出列表可用宽度时, 按可压缩余量等比压缩
    /// (每列不低于最小列宽, 列不消失); 空间富余时低于目标宽的列等比回涨恢复
    /// (目标宽 = 默认值或用户最近一次拖拽后的宽度)。触发点: 拖拽列宽
    /// (RaiseChanged → OnColumnStateChanged) 与列表布局变化 (窗格缩放/滚动条出现)。
    /// 结果写回共享列宽状态, 列头/条目/竖线/手柄随实际布局自动跟随。
    /// </summary>
    private void UpdateColumnFit()
    {
        if (_disposed || _emptyState)
        {
            return;
        }
        // 可用宽度取第一个条目的实际宽 (已扣除垂直滚动条占位); 无条目时退化为列表宽。
        double avail = FileList.ContainerFromIndex(0) is ListViewItem first && first.ActualWidth > 0
            ? first.ActualWidth
            : FileList.ActualWidth;
        if (avail <= 0)
        {
            return; // 首帧布局前宽度未定, 等下次 LayoutUpdated 收敛
        }
        // 可见列 (名称列始终显示; 其余按用户显隐开关),
        // 元组: (列标识, 当前宽, 最小宽, 回涨目标宽, 写回)。
        var cols = new List<(string Col, double Width, double Min, double Def, Action<double> Set)>
        {
            ("Name", _cols.NameColWidth, FileColumnState.MinNameColWidth, _cols.NameColDef,
                w => _cols.NameColWidth = w),
        };
        if (_cols.ShowSize)
        {
            cols.Add(("Size", _cols.SizeColWidth, FileColumnState.MinColWidth, _cols.SizeColDef,
                w => _cols.SizeColWidth = w));
        }
        if (_cols.ShowType)
        {
            cols.Add(("Type", _cols.TypeColWidth, FileColumnState.MinColWidth, _cols.TypeColDef,
                w => _cols.TypeColWidth = w));
        }
        if (_cols.ShowModified)
        {
            cols.Add(("Modified", _cols.ModifiedColWidth, FileColumnState.MinColWidth,
                _cols.ModifiedColDef, w => _cols.ModifiedColWidth = w));
        }
        if (Mode == FileViewMode.Remote && _cols.ShowPermissions)
        {
            cols.Add(("Permissions", _cols.PermissionsColWidth, FileColumnState.MinColWidth,
                _cols.PermissionsColDef, w => _cols.PermissionsColWidth = w));
        }
        // 总需求 = 固定开销 (左右 Padding 8 + 图标列 22 + 名称列前间距 12)
        //          + Σ(每列宽 + 列间距 12); 几何常量与 XAML 列模板同源。
        double needed = FileColumnState.NameColFixedOverhead;
        foreach (var (_, width, _, _, _) in cols)
        {
            needed += width + FileColumnState.ColSpacing;
        }
        // 0.5 DIP 容差防抖动; 压缩/回涨互斥, 中间留 1 DIP 滞回带防震荡。
        bool changed = false;
        double excess = needed - avail;
        if (excess > 0.5)
        {
            // 两轮压缩: 第 1 轮压"非拖拽列" (拖拽列最后被压 → 手感为拖拽列先拿足增量,
            // 其他列被挤到最小值; 窗格缩放时无拖拽列, 即全部列等比压缩); 第 2 轮仍超宽
            // (其他列已全部到最小值) 才压拖拽列, 使其卡在剩余空间上限。
            for (int round = 0; round < 2 && excess > 0.5; round++)
            {
                // round 0 压非拖拽列; round 1 (仅拖拽中) 压拖拽列。两轮目标互斥,
                // 元组宽度快照在各轮内对其目标列始终准确。
                if (round == 1 && _draggingCol is null)
                {
                    break; // 无拖拽列: round 0 已按等比处理全部列
                }
                double squeezable = 0;
                foreach (var (col, width, min, _, _) in cols)
                {
                    bool isDragging = col == _draggingCol;
                    if (round == 0 ? isDragging : !isDragging)
                    {
                        continue;
                    }
                    squeezable += Math.Max(0, width - min);
                }
                if (squeezable <= 0)
                {
                    continue;
                }
                double ratio = Math.Min(1.0, excess / squeezable);
                foreach (var (col, width, min, _, set) in cols)
                {
                    bool isDragging = col == _draggingCol;
                    if (round == 0 ? isDragging : !isDragging)
                    {
                        continue;
                    }
                    double shrunk = Math.Max(min, width - (width - min) * ratio);
                    if (shrunk < width - 0.05)
                    {
                        set(shrunk);
                        excess -= width - shrunk;
                        changed = true;
                    }
                }
            }
        }
        else if (avail - needed > 0.5)
        {
            // 空间富余 → 回涨恢复: 先窄后宽的窗格 / 此前被挤压的列, 从历史挤压宽度
            // 等比回涨到各自目标宽为止, 保证空间充足时列宽完整显示。拖拽列与已达
            // 目标宽的列 deficit=0 不动 (回涨总量 = min(富余, 缺口), 单调收敛)。
            double deficit = 0;
            foreach (var (col, width, _, def, _) in cols)
            {
                if (col != _draggingCol)
                {
                    deficit += Math.Max(0, def - width);
                }
            }
            if (deficit > 0)
            {
                double ratio = Math.Min(1.0, (avail - needed) / deficit);
                foreach (var (col, width, _, def, set) in cols)
                {
                    if (col == _draggingCol || def <= width)
                    {
                        continue;
                    }
                    double grown = Math.Min(def, width + (def - width) * ratio);
                    if (grown > width + 0.05)
                    {
                        set(grown);
                        changed = true;
                    }
                }
            }
        }
        if (changed)
        {
            _cols.RaiseChanged(); // → OnColumnStateChanged → 再次进入本方法收敛
        }
    }

    // —— 列分隔贯穿竖线 (ListView 覆盖层 Rectangle, 位置随列宽/显隐/宽度动态重算) ——

    private double _sizeLineX = -1;
    private double _typeLineX = -1;
    private double _modLineX = -1;
    private double _permLineX = -1;
    // 树同步链代际号: 树重建 (RefreshTreeIfVisible) 自增, 旧的同步链检查到代际变化即放弃,
    // 防止针对旧树节点的选中/展开写进新树 (导航同步与重建链并发时的竞态)。
    private int _treeGen;

    // 正在懒加载枚举的树节点 (UI 线程集合)。Expanding 处理器先清 HasUnrealizedChildren
    // 再异步枚举 (早清防重入), 故该标记不能反映"加载中"; 等待方以此集合判断真正完成。
    private readonly HashSet<TreeViewNode> _expanding = new();

    /// <summary>
    /// 重算列表区 3 条贯穿竖线位置: 从第一个条目的真实 Border 元素测量 (与条目列天然对齐,
    /// 不受滚动条占位/容器内边距影响); 并把条目与列头行的宽度差补偿到列头右 Padding,
    /// 使列头竖线与列表竖线衔接贯穿。无条目时隐藏 (等 LayoutUpdated 再收敛)。
    /// </summary>
    private void UpdateColumnLines()
    {
        if (_disposed)
        {
            return;
        }
        // 空态引导时列表被引导层覆盖, 线不应浮现其上。
        if (_emptyState)
        {
            HideColumnLines();
            return;
        }
        if (FileList.ContainerFromIndex(0) is not ListViewItem first)
        {
            HideColumnLines();
            return;
        }
        // ListViewItem.Content 是数据项 (FileItemViewModel) 而非模板根;
        // 行模板根 Grid 需在容器视觉树中按 "含 Column>=2 的 Border" 特征搜索。
        Grid? rowGrid = FindRowGrid(first);
        if (rowGrid is null)
        {
            HideColumnLines();
            return;
        }

        Border? sizeAnchor = null, typeAnchor = null, modAnchor = null, permAnchor = null;
        foreach (var child in rowGrid.Children)
        {
            if (child is Border border)
            {
                switch (Grid.GetColumn(border))
                {
                    case 2: sizeAnchor = border; break;
                    case 3: typeAnchor = border; break;
                    case 4: modAnchor = border; break;
                    case 5: permAnchor = border; break;
                }
            }
        }

        // 条目区与列头行的宽度差 (列表竖向滚动条占位) 补到列头右 Padding, 列头线随之后移对齐。
        double scrollbarW = Math.Max(0, FileList.ActualWidth - first.ActualWidth);
        var headerPad = new Thickness(4, 6, 4 + scrollbarW, 6);
        if (HeaderGrid.Padding != headerPad)
        {
            HeaderGrid.Padding = headerPad;
        }

        PlaceColumnLine(SizeLine, sizeAnchor, _cols.ShowSize, ref _sizeLineX);
        PlaceColumnLine(TypeLine, typeAnchor, _cols.ShowType, ref _typeLineX);
        PlaceColumnLine(ModifiedLine, modAnchor, _cols.ShowModified, ref _modLineX);
        PlaceColumnLine(PermissionLine, permAnchor,
            Mode == FileViewMode.Remote && _cols.ShowPermissions, ref _permLineX);
    }

    private void HideColumnLines()
    {
        SizeLine.Visibility = Visibility.Collapsed;
        TypeLine.Visibility = Visibility.Collapsed;
        ModifiedLine.Visibility = Visibility.Collapsed;
        PermissionLine.Visibility = Visibility.Collapsed;
    }

    /// <summary>在条目容器视觉树中搜索行模板根 Grid (特征: 含 Grid.Column>=2 的 Border 子元素)。</summary>
    private static Grid? FindRowGrid(DependencyObject node)
    {
        if (node is Grid grid)
        {
            foreach (var child in grid.Children)
            {
                if (child is Border b && Grid.GetColumn(b) >= 2)
                {
                    return grid;
                }
            }
        }
        int count = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(node);
        for (int i = 0; i < count; i++)
        {
            if (FindRowGrid(Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(node, i)) is { } found)
            {
                return found;
            }
        }
        return null;
    }

    /// <summary>把竖线放到锚点列与前列间距的正中间 (列左缘 - 6, 相对 ContentRoot 坐标), 值变化才赋值避免布局循环。</summary>
    private void PlaceColumnLine(FrameworkElement line, FrameworkElement? anchor, bool show, ref double lastX)
    {
        if (!show || anchor is null || anchor.Visibility != Visibility.Visible)
        {
            line.Visibility = Visibility.Collapsed;
            return;
        }
        // 列间距 12px: 线画在正中间, 条目/列头文字 (各 6px 内边距) 与线互不侵入。
        // 坐标基准为列头+列表共同容器 (覆盖层跨两行贯穿)。
        double x = anchor.TransformToVisual(ContentRoot).TransformPoint(new Windows.Foundation.Point(0, 0)).X - 6;
        line.Visibility = Visibility.Visible;
        if (Math.Abs(lastX - x) > 0.5)
        {
            lastX = x;
            line.Margin = new Thickness(x, 0, 0, 0);
        }
    }

    // 列宽手柄 hover 光标 (WinUI 3 无公开 Cursor 属性, 反射设 ProtectedCursor;
    // 与 HandCursorRoot 同法, PublishTrimmed 必须为 False)
    private static readonly System.Reflection.PropertyInfo? ProtectedCursorProperty =
        typeof(UIElement).GetProperty("ProtectedCursor", System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
    private static readonly Microsoft.UI.Input.InputCursor SizeWestEastCursor =
        Microsoft.UI.Input.InputSystemCursor.Create(Microsoft.UI.Input.InputSystemCursorShape.SizeWestEast);

    private void ColumnThumb_PointerEntered(object sender, PointerRoutedEventArgs e)
        => ProtectedCursorProperty?.SetValue(sender, SizeWestEastCursor);

    private void ColumnThumb_PointerExited(object sender, PointerRoutedEventArgs e)
        => ProtectedCursorProperty?.SetValue(sender, null);

    private void ColumnThumb_DragDelta(object sender, DragDeltaEventArgs e)
    {
        // DragDelta.HorizontalChange 是距上一次事件的增量 (DragCompleted 的才是总增量),
        // 必须在当前列宽上累加; 用"起始宽+增量"会导致列宽只走一步就卡住。
        var tag = ((Thumb)sender).Tag as string;
        _draggingCol = tag; // 压缩适配时其他列先被挤压, 本列最后被压 (见 UpdateColumnFit)
        switch (tag)
        {
            case "Name":
                _cols.NameColWidth = Math.Clamp(
                    _cols.NameColWidth + e.HorizontalChange,
                    FileColumnState.MinNameColWidth, FileColumnState.MaxNameColWidth);
                break;
            case "Size":
                _cols.SizeColWidth = Math.Clamp(
                    _cols.SizeColWidth + e.HorizontalChange,
                    FileColumnState.MinColWidth, FileColumnState.MaxColWidth);
                break;
            case "Type":
                _cols.TypeColWidth = Math.Clamp(
                    _cols.TypeColWidth + e.HorizontalChange,
                    FileColumnState.MinColWidth, FileColumnState.MaxColWidth);
                break;
            case "Permissions":
                _cols.PermissionsColWidth = Math.Clamp(
                    _cols.PermissionsColWidth + e.HorizontalChange,
                    FileColumnState.MinColWidth, FileColumnState.MaxColWidth);
                break;
            default:
                _cols.ModifiedColWidth = Math.Clamp(
                    _cols.ModifiedColWidth + e.HorizontalChange,
                    FileColumnState.MinColWidth, FileColumnState.MaxColWidth);
                break;
        }
        _cols.RaiseChanged();
    }

    private void ColumnThumb_DragCompleted(object sender, DragCompletedEventArgs e)
    {
        // 拖拽结果即该列新的"回涨目标宽": 用户拖窄/拖宽都是明确意图,
        // 后续富余空间回涨最多回到此值, 不会推翻 (见 UpdateColumnFit 恢复分支)。
        var tag = ((Thumb)sender).Tag as string;
        switch (tag)
        {
            case "Name":
                _cols.NameColDef = _cols.NameColWidth;
                break;
            case "Size":
                _cols.SizeColDef = _cols.SizeColWidth;
                break;
            case "Type":
                _cols.TypeColDef = _cols.TypeColWidth;
                break;
            case "Permissions":
                _cols.PermissionsColDef = _cols.PermissionsColWidth;
                break;
            default:
                _cols.ModifiedColDef = _cols.ModifiedColWidth;
                break;
        }
        _draggingCol = null; // 拖拽结束, 后续自适应回到"全部列等比压缩/回涨"模式
    }

    // —— 新建文件夹 / 新建文件 (本地走 IO, 远程走 SFTP) ——

    private void NewFolderBtn_Click(object sender, RoutedEventArgs e)
        => _ = CreateNewEntryAsync(isFolder: true);

    private void NewFileBtn_Click(object sender, RoutedEventArgs e)
        => _ = CreateNewEntryAsync(isFolder: false);

    /// <summary>新建条目: 名字自动加序号避开重名; 完成刷新。本地走 IO, 远程走 SFTP。</summary>
    private async Task CreateNewEntryAsync(bool isFolder)
    {
        if (Mode == FileViewMode.Remote)
        {
            await CreateRemoteEntryAsync(isFolder);
            return;
        }
        if (_currentPath.Length == 0)
        {
            return;
        }
        try
        {
            var baseName = LocalizationService.GetString(isFolder ? "NewFolderName" : "NewFileName");
            var path = Path.Combine(_currentPath, baseName);
            for (int i = 2; File.Exists(path) || Directory.Exists(path); i++)
            {
                path = Path.Combine(_currentPath,
                    $"{Path.GetFileNameWithoutExtension(baseName)} ({i}){Path.GetExtension(baseName)}");
            }
            if (isFolder)
            {
                Directory.CreateDirectory(path);
            }
            else
            {
                File.Create(path).Dispose();
            }
            RefreshListKeepingPath();
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>远程新建: 当前目录自动加序号 → SFTP mkdir/touch → 刷新。
    /// 名字不做 Windows 非法字符限制 (Linux 允许 : \ 等, 由服务器自行校验)。</summary>
    private async Task CreateRemoteEntryAsync(bool isFolder)
    {
        try
        {
            var baseName = LocalizationService.GetString(isFolder ? "NewFolderName" : "NewFileName");
            var dir = _currentPath is { Length: > 0 } ? _currentPath : ".";
            var path = RemotePathJoin(dir, baseName);
            for (int i = 2; await _sftp.ExistsAsync(path); i++)
            {
                path = RemotePathJoin(dir,
                    $"{Path.GetFileNameWithoutExtension(baseName)} ({i}){Path.GetExtension(baseName)}");
            }
            if (isFolder)
            {
                await _sftp.CreateDirectoryAsync(path);
            }
            else
            {
                await _sftp.CreateFileAsync(path);
            }
            RefreshListKeepingPath();
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>远程路径拼接 ('/' 分隔; "." 与 "/" 两种当前目录形态均正确)。</summary>
    private static string RemotePathJoin(string dir, string name)
        => dir.EndsWith('/') ? dir + name : dir + "/" + name;

    /// <summary>按模式/当前层级更新新建按钮可用性 (根视图与远程模式不可用)。</summary>
    private void UpdateNewButtons()
    {
        bool canCreate = Mode == FileViewMode.Local && _currentPath.Length > 0;
        NewFolderBtn.IsEnabled = canCreate;
        NewFileBtn.IsEnabled = canCreate;
    }

    private void UpdateHeaderColumnVisibility()
    {
        // 列头可见性 = 用户显隐开关 (列宽自适应只压缩宽度不隐藏列, 见 UpdateColumnFit);
        // 权限列另受模式限制: 仅远程 (Linux) 显示。
        HeaderSizeCell.Visibility = _cols.ShowSize
            ? Visibility.Visible : Visibility.Collapsed;
        HeaderTypeCell.Visibility = _cols.ShowType
            ? Visibility.Visible : Visibility.Collapsed;
        HeaderModifiedCell.Visibility = _cols.ShowModified
            ? Visibility.Visible : Visibility.Collapsed;
        HeaderPermissionsCell.Visibility =
            Mode == FileViewMode.Remote && _cols.ShowPermissions
                ? Visibility.Visible : Visibility.Collapsed;
    }

    private void MenuShowSize_Toggled(object sender, RoutedEventArgs e)
    {
        if (_cols.ShowSize != MenuShowSize.IsChecked)
        {
            _cols.ShowSize = MenuShowSize.IsChecked;
            _cols.RaiseChanged();
        }
    }

    private void MenuShowType_Toggled(object sender, RoutedEventArgs e)
    {
        if (_cols.ShowType != MenuShowType.IsChecked)
        {
            _cols.ShowType = MenuShowType.IsChecked;
            _cols.RaiseChanged();
        }
    }

    private void MenuShowModified_Toggled(object sender, RoutedEventArgs e)
    {
        if (_cols.ShowModified != MenuShowModified.IsChecked)
        {
            _cols.ShowModified = MenuShowModified.IsChecked;
            _cols.RaiseChanged();
        }
    }

    private void MenuShowPermissions_Toggled(object sender, RoutedEventArgs e)
    {
        if (_cols.ShowPermissions != MenuShowPermissions.IsChecked)
        {
            _cols.ShowPermissions = MenuShowPermissions.IsChecked;
            _cols.RaiseChanged();
        }
    }

    // ───────────────────────── 树形目录侧栏 ─────────────────────────

    /// <summary>重建树根: 本地=驱动器, 远程=/ (子级懒展开)。</summary>
    private void RebuildTree()
    {
        DirTree.RootNodes.Clear();
        if (Mode == FileViewMode.Local)
        {
            foreach (var drive in DriveInfo.GetDrives())
            {
                DirTree.RootNodes.Add(NewDirNode(new TreeDir(drive.Name, drive.Name), lazy: true));
            }
        }
        else if (_sftp.IsConnected)
        {
            DirTree.RootNodes.Add(NewDirNode(new TreeDir("/", "/"), lazy: true));
        }
    }

    private static TreeViewNode NewDirNode(TreeDir dir, bool lazy)
        => new() { Content = dir, HasUnrealizedChildren = lazy, IsExpanded = false };

    /// <summary>树节点懒展开: 首次展开时才枚举子目录。</summary>
    private async void DirTree_Expanding(TreeView sender, TreeViewExpandingEventArgs args)
    {
        var node = args.Node;
        if (node?.Content is not TreeDir dir || !node.HasUnrealizedChildren)
        {
            return;
        }
        node.HasUnrealizedChildren = false;
        _expanding.Add(node); // 登记加载中 (早清标记后, 等待方靠此判断枚举真正结束)
        try
        {
            if (Mode == FileViewMode.Local)
            {
                var path = dir.FullPath;
                var names = await Task.Run(() =>
                {
                    try
                    {
                        return Directory.EnumerateDirectories(path)
                            .Where(p => _cols.ShowHidden
                                || (File.GetAttributes(p) & FileAttributes.Hidden) == 0)
                            .Select(p => new TreeDir(Path.GetFileName(p.TrimEnd('\\')) is { Length: > 0 } ? Path.GetFileName(p.TrimEnd('\\')) : p, p))
                            .ToList();
                    }
                    catch
                    {
                        return new List<TreeDir>();
                    }
                });
                foreach (var sub in names)
                {
                    node.Children.Add(NewDirNode(sub, lazy: true));
                }
            }
            else if (_sftp.IsConnected)
            {
                var entries = await _sftp.ListDirectoryAsync(dir.FullPath);
                foreach (var entry in entries.Where(e2 => e2.IsDirectory
                    && e2.Name is not "." and not ".."
                    && (_cols.ShowHidden || !e2.Name.StartsWith('.'))))
                {
                    node.Children.Add(NewDirNode(new TreeDir(entry.Name, entry.FullPath), lazy: true));
                }
            }
        }
        catch
        {
            // 无权限/网络异常: 该节点展开为空即可
        }
        finally
        {
            _expanding.Remove(node);
        }
    }

    /// <summary>点击树节点 → 文件列表跳转到该目录。</summary>
    private void DirTree_ItemInvoked(TreeView sender, TreeViewItemInvokedEventArgs args)
    {
        var content = (args.InvokedItem as TreeViewNode)?.Content ?? args.InvokedItem;
        if (content is not TreeDir dir)
        {
            return;
        }
        DismissEmptyState();
        if (Mode == FileViewMode.Local)
        {
            EnterLocalPath(dir.FullPath);
        }
        else if (_sftp.IsConnected)
        {
            _currentPath = dir.FullPath;
            _ = LoadRemoteAsync();
        }
    }

    // ───────────────────────── 列表交互与状态栏 ─────────────────────────

    // 单击 = 选中 (ListView 默认选择行为); 双击文件夹进入下一层, 文件的双击动作待定义。

    private void FileList_DoubleTapped(object sender, DoubleTappedRoutedEventArgs e)
    {
        if (FileList.SelectedItem is FileItemViewModel item && item.IsDirectory)
        {
            EnterDirectory(item);
        }
    }

    private void FileList_SelectionChanged(object sender, SelectionChangedEventArgs e)
        => UpdateStatus();

    // ───────────────────────── 框选 (rubber band selection) ─────────────────────────

    private Windows.Foundation.Point _rubberOrigin;
    private bool _rubberActive;
    private bool _rubberAdditive;
    private readonly List<FileItemViewModel> _rubberBase = new();

    /// <summary>
    /// 列表空白处按住左键拖出选择框, 框内条目多选 (与资源管理器一致)。条目上的
    /// 按下交由 ListView 常规单击/双击选择; Ctrl 框选在原有选择上合并。条目容器
    /// 虚拟化, 滚出可视区未实例化的不参与命中; 比对占位行无语义, 命中也排除。
    /// </summary>
    private void OnRubberBandPointerPressed(object sender, PointerRoutedEventArgs e)
    {
        var pt = e.GetCurrentPoint(FileListHost);
        if (e.Pointer.PointerDeviceType != Microsoft.UI.Input.PointerDeviceType.Mouse
            || !pt.Properties.IsLeftButtonPressed
            || _emptyState)
        {
            return;
        }

        // 滚动条等控件按下不框选; 条目上按下 (DataContext 为条目 VM) 交常规选择
        var source = e.OriginalSource as DependencyObject;
        if (IsInScrollBar(source) || (source as FrameworkElement)?.DataContext is FileItemViewModel)
        {
            return;
        }

        _rubberOrigin = pt.Position;
        _rubberActive = true;
        _rubberAdditive = e.KeyModifiers.HasFlag(Windows.System.VirtualKeyModifiers.Control);
        _rubberBase.Clear();
        if (_rubberAdditive)
        {
            _rubberBase.AddRange(FileList.SelectedItems.OfType<FileItemViewModel>());
        }

        FileListHost.CapturePointer(e.Pointer);
        RubberBand.Visibility = Visibility.Visible;
        UpdateRubberBand(pt.Position);
        ApplyRubberSelection(pt.Position); // 按下即应用: 无修饰时点击空白=清除选择
        e.Handled = true;
    }

    private void OnRubberBandPointerMoved(object sender, PointerRoutedEventArgs e)
    {
        if (!_rubberActive)
        {
            return;
        }

        var pos = e.GetCurrentPoint(FileListHost).Position;
        UpdateRubberBand(pos);
        ApplyRubberSelection(pos);
        e.Handled = true;
    }

    private void OnRubberBandPointerReleased(object sender, PointerRoutedEventArgs e)
    {
        if (!_rubberActive)
        {
            return;
        }

        _rubberActive = false;
        FileListHost.ReleasePointerCapture(e.Pointer);
        RubberBand.Visibility = Visibility.Collapsed;
        e.Handled = true;
    }

    /// <summary>按起点与当前点更新选择框矩形 (支持反方向拖动)。</summary>
    private void UpdateRubberBand(Windows.Foundation.Point pos)
    {
        Canvas.SetLeft(RubberBand, Math.Min(_rubberOrigin.X, pos.X));
        Canvas.SetTop(RubberBand, Math.Min(_rubberOrigin.Y, pos.Y));
        RubberBand.Width = Math.Abs(pos.X - _rubberOrigin.X);
        RubberBand.Height = Math.Abs(pos.Y - _rubberOrigin.Y);
    }

    /// <summary>按当前框选矩形重设列表选择: 无修饰=仅框内; Ctrl=框选前基准∪框内。</summary>
    private void ApplyRubberSelection(Windows.Foundation.Point pos)
    {
        var rect = new Windows.Foundation.Rect(
            Math.Min(_rubberOrigin.X, pos.X),
            Math.Min(_rubberOrigin.Y, pos.Y),
            Math.Abs(pos.X - _rubberOrigin.X),
            Math.Abs(pos.Y - _rubberOrigin.Y));

        var hit = new List<object>();
        for (int i = 0; i < FileList.Items.Count; i++)
        {
            if (FileList.ContainerFromIndex(i) is not ListViewItem container
                || container.Content is not FileItemViewModel vm
                || vm.IsPlaceholder)
            {
                continue; // 虚拟化未实例化 / 比对占位行不参与
            }

            var bounds = container.TransformToVisual(FileListHost).TransformBounds(
                new Windows.Foundation.Rect(default, new Windows.Foundation.Size(container.ActualWidth, container.ActualHeight)));
            var overlap = bounds;
            overlap.Intersect(rect);
            if (!overlap.IsEmpty)
            {
                hit.Add(vm);
            }
        }

        var target = _rubberAdditive ? _rubberBase.Cast<object>().Union(hit).ToList() : hit;
        FileList.SelectedItems.Clear();
        foreach (var item in target)
        {
            FileList.SelectedItems.Add(item);
        }
    }

    /// <summary>按下来源是否位于滚动条内部 (滚动条拖动不启动框选)。</summary>
    private static bool IsInScrollBar(DependencyObject? source)
    {
        for (var node = source; node is not null; node = VisualTreeHelper.GetParent(node))
        {
            if (node is ScrollBar)
            {
                return true;
            }
        }

        return false;
    }

    // ───────────────────────── 右键菜单 (本地全量 / 远程管理类子集) ─────────────────────────

    /// <summary>列表右键: 命中条目 → 先选中再弹条目菜单; 空白区 → 弹空白菜单。
    /// 远程模式须已连接 (断连时无菜单)。</summary>
    private void FileList_RightTapped(object sender, RightTappedRoutedEventArgs e)
    {
        if (_emptyState || (Mode != FileViewMode.Local && !_sftp.IsConnected))
        {
            return;
        }
        var hit = (e.OriginalSource as FrameworkElement)?.DataContext as FileItemViewModel;
        if (hit is not null && hit.IsPlaceholder)
        {
            hit = null; // 比对占位行无条目语义: 按空白区处理 (弹空白菜单)
        }
        if (hit is not null && !FileList.SelectedItems.Contains(hit))
        {
            FileList.SelectedItem = hit; // 右键未选中项时切换为单选; 命中已选集则保持多选
        }
        var flyout = hit is not null ? BuildItemMenu(hit) : BuildBlankMenu();
        flyout.ShowAt(FileList, e.GetPosition(FileList));
    }

    /// <summary>条目右键菜单: 本地 = 复制到另一侧 / 编辑 (仅文本文件, 头部探测) / 打开 ║ 新建 ║ 刷新 / 重命名 / 删除;
    /// 远程 = 编辑 (扩展名预启用, 未知扩展名点击时头部探测回退) ║ 刷新 / 重命名 / 删除。</summary>
    private MenuFlyout BuildItemMenu(FileItemViewModel item)
    {
        var menu = new MenuFlyout { MenuFlyoutPresenterStyle = PaneMenuPresenterStyle() };
        if (Mode == FileViewMode.Remote)
        {
            // 传输到另一侧: 四方向统一判定 (远程→本地 / 远程→远程)
            menu.Items.Add(MenuItem("MenuCopyToOpposite", () => _ = TransferToOppositeAsync(),
                isEnabled: CanTransferToOpposite()));
            menu.Items.Add(MenuItem("MenuEdit", () => _ = OpenRemoteWithEditorAsync(item),
                isEnabled: !item.IsDirectory && RemoteEditMenuEnabled(item)));
            menu.Items.Add(new MenuFlyoutSeparator());
            menu.Items.Add(MenuItem("Refresh", () => RefreshListKeepingPath()));
            menu.Items.Add(MenuItem("MenuRename", () => _ = RenameEntryAsync(item)));
            menu.Items.Add(MenuItem("MenuDelete", () => _ = DeleteSelectedAsync(), isDangerous: true));
            // 权限组: 修改属主/属组/其他的读/写/执行位 (本地 Windows 无 chmod 等价物, 仅远程提供)
            menu.Items.Add(new MenuFlyoutSeparator());
            menu.Items.Add(MenuItem("MenuPermissions", () => _ = ShowPermissionsDialogAsync(item)));
            // 校验码: 服务端命令优先 (sha256sum/sha512sum/openssl SM3, 秒级), 缺失时回退本机拉流;
            // 菜单可用性按探测结果联动: 三项均不支持 (或尚未探测出结果) → 置灰, 目录也置灰
            menu.Items.Add(MenuItem("MenuChecksum", () => ChecksumOverlay.OpenFor(item, _sftp),
                isEnabled: !item.IsDirectory && _sftp.ServerHashAny));
            return menu;
        }

        // 传输到另一侧: 四方向统一判定 (本地→本地复制 / 本地→远程上传)
        bool isFile = !item.IsDirectory;
        menu.Items.Add(MenuItem("MenuCopyToOpposite", () => _ = TransferToOppositeAsync(),
            isEnabled: CanTransferToOpposite()));
        menu.Items.Add(MenuItem("MenuEdit", () => OpenWithEditor(item), isEnabled: isFile && IsTextFile(item.FullPath)));
        menu.Items.Add(MenuItem("MenuOpen", () => OpenEntry(item)));
        menu.Items.Add(new MenuFlyoutSeparator());
        AppendNewEntries(menu);
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(MenuItem("Refresh", () => RefreshListKeepingPath()));
        menu.Items.Add(MenuItem("MenuRename", () => _ = RenameEntryAsync(item)));
        menu.Items.Add(MenuItem("MenuDelete", () => _ = DeleteSelectedAsync(), isDangerous: true));
        return menu;
    }

    /// <summary>空白区右键菜单: 新建文件夹 / 新建文件 ║ 刷新 (本地与远程一致, 远程须已连接)。</summary>
    private MenuFlyout BuildBlankMenu()
    {
        var menu = new MenuFlyout { MenuFlyoutPresenterStyle = PaneMenuPresenterStyle() };
        AppendNewEntries(menu, remote: Mode == FileViewMode.Remote);
        menu.Items.Add(new MenuFlyoutSeparator());
        menu.Items.Add(MenuItem("Refresh", () => RefreshListKeepingPath()));
        return menu;
    }

    /// <summary>追加"新建文件夹/新建文件"两项 (根视图下禁用, 与工具栏按钮一致; 远程须已连接)。</summary>
    private void AppendNewEntries(MenuFlyout menu, bool remote = false)
    {
        bool canCreate = remote ? _sftp.IsConnected : _currentPath.Length > 0;
        menu.Items.Add(MenuItem("MenuNewFolder", () => _ = CreateNewEntryAsync(isFolder: true), isEnabled: canCreate));
        menu.Items.Add(MenuItem("MenuNewFile", () => _ = CreateNewEntryAsync(isFolder: false), isEnabled: canCreate));
    }

    /// <summary>构建本地化菜单项 (text=资源 key; 命令经 DispatcherQueue 延后触发,
    /// 避免 flyout 关闭动画与列表重建重入)。isDangerous = 危险操作红色文字 (删除)。</summary>
    private MenuFlyoutItem MenuItem(string textKey, Action action, bool isEnabled = true, bool isDangerous = false)
    {
        var item = new MenuFlyoutItem { Text = LocalizationService.GetString(textKey), IsEnabled = isEnabled };
        if (isDangerous)
        {
            item.Foreground = new SolidColorBrush(Windows.UI.Color.FromArgb(255, 0xE8, 0x11, 0x23));
        }
        item.Click += (_, _) => DispatcherQueue.TryEnqueue(() =>
        {
            try
            {
                action();
            }
            catch (Exception ex)
            {
                // 菜单命令 fire-and-forget: 同步段异常静默吞掉会表现为"点击无反应"
                App.LogCrash("MenuClick:" + textKey, ex.ToString());
            }
        });
        return item;
    }

    /// <summary>重命名条目: 对话框输入新名 (实时校验非法字符/重名) → move → 刷新。本地走 IO, 远程走 SFTP。</summary>
    private async Task RenameEntryAsync(FileItemViewModel item)
    {
        if (Mode == FileViewMode.Remote)
        {
            await RenameRemoteEntryAsync(item);
            return;
        }
        string? newName;
        try
        {
            newName = await RenameInputOverlay.ShowAsync(item.Name, item.IsDirectory, n => EntryExists(n));
        }
        catch (Exception ex)
        {
            // 对话框阶段异常 (如 ContentDialog 单例冲突) 会静默表现为"不弹出"
            App.LogCrash("RenameDialog(local)", ex.ToString());
            UpdateStatus(ex.Message);
            return;
        }
        if (newName is null || newName == item.Name)
        {
            return;
        }
        try
        {
            var dst = Path.Combine(Path.GetDirectoryName(item.FullPath)!, newName);
            if (item.IsDirectory)
            {
                Directory.Move(item.FullPath, dst);
            }
            else
            {
                File.Move(item.FullPath, dst);
            }
            RefreshListKeepingPath();
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>远程重命名: 新名占用校验基于当前列表 (免逐键网络请求, 服务器冲突由错误信息兜底) → SFTP rename → 刷新。</summary>
    private async Task RenameRemoteEntryAsync(FileItemViewModel item)
    {
        string? newName;
        try
        {
            newName = await RenameInputOverlay.ShowAsync(item.Name, item.IsDirectory,
                n => _items.Any(i => string.Equals(i.Name, n, StringComparison.Ordinal)), isRemote: true);
        }
        catch (Exception ex)
        {
            App.LogCrash("RenameDialog(remote)", ex.ToString());
            UpdateStatus(ex.Message);
            return;
        }
        if (newName is null || newName == item.Name)
        {
            return;
        }
        try
        {
            var dir = _currentPath is { Length: > 0 } ? _currentPath : ".";
            await _sftp.RenameAsync(item.FullPath, RemotePathJoin(dir, newName));
            RefreshListKeepingPath();
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>当前目录下是否已存在同名条目 (重命名校验)。</summary>
    private bool EntryExists(string name)
        => Directory.Exists(Path.Combine(_currentPath, name))
           || File.Exists(Path.Combine(_currentPath, name));

    /// <summary>删除当前选中项 (含右键命中项, 支持多选): 确认 → 远程永久删除 (目录递归) /
    /// 本地移入回收站 → 刷新。</summary>
    private async Task DeleteSelectedAsync()
    {
        var items = FileList.SelectedItems.OfType<FileItemViewModel>().Where(i => !i.IsPlaceholder).ToList();
        if (items.Count == 0)
        {
            return;
        }

        var toRecycle = Mode != FileViewMode.Remote; // 本地删除进回收站可还原, 远程仍永久删除
        if (!await DeleteConfirmOverlay.ShowAsync(items.Count, items[0].Name, toRecycle))
        {
            return;
        }
        try
        {
            if (Mode == FileViewMode.Remote)
            {
                foreach (var it in items)
                {
                    await _sftp.DeleteAsync(it.FullPath, it.IsDirectory);
                }
            }
            else
            {
                // 本地: 一次 Shell 调用批量送回收站 (文件/目录可混合)
                if (!RecycleBin.Delete(items.Select(i => i.FullPath)))
                {
                    throw new InvalidOperationException(LocalizationService.GetString("ErrRecycleDelete"));
                }
            }

            RefreshListKeepingPath();
            UpdateStatus(string.Format(LocalizationService.GetString("StatusDeletedCount"), items.Count));
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>打开权限编辑弹窗: 先实时拉取远端当前权限 (列表缓存可能过期), 再弹九宫格编辑。</summary>
    private async Task ShowPermissionsDialogAsync(FileItemViewModel item)
    {
        int mode;
        try
        {
            mode = await _sftp.GetPermissionsAsync(item.FullPath);
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
            return;
        }

        PermissionOverlay.OpenFor(item, mode);
    }

    /// <summary>应用权限: SFTP chmod (仅 9 位 rwx, suid/sgid/sticky 保持原值) → 刷新列表与状态栏。</summary>
    private async Task ApplyPermissionsAsync(FileItemViewModel item, int mode)
    {
        try
        {
            await _sftp.SetPermissionsAsync(item.FullPath, mode);
            RefreshListKeepingPath();
            UpdateStatus(string.Format(LocalizationService.GetString("StatusPermUpdated"),
                Convert.ToString(mode, 8).PadLeft(3, '0')));
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>权限弹窗"应用"回调 (弹窗自行关闭后触发)。</summary>
    private void PermissionOverlay_ApplyRequested(FileItemViewModel item, int mode)
        => _ = ApplyPermissionsAsync(item, mode);

    /// <summary>右键菜单底色: 与父容器同色系的浅一档 (Popup 层资源查找不经过本控件,
    /// 须在构建时按当前主题显式指定 presenter 背景)。亮 = #FFFFFF (容器 #F9F9F9 之上),
    /// 暗 = #26303D (容器 #1B2430 之上, 同深蓝色系)。</summary>
    private Style PaneMenuPresenterStyle()
    {
        var bg = ActualTheme == ElementTheme.Light
            ? Windows.UI.Color.FromArgb(255, 0xFF, 0xFF, 0xFF)
            : Windows.UI.Color.FromArgb(255, 0x26, 0x30, 0x3D);
        var style = new Style(typeof(MenuFlyoutPresenter));
        style.Setters.Add(new Setter(MenuFlyoutPresenter.BackgroundProperty, new SolidColorBrush(bg)));
        return style;
    }

    /// <summary>打开条目: 文件夹进入下一层; 文件用系统默认关联程序打开。</summary>
    private void OpenEntry(FileItemViewModel item)
    {
        if (item.IsDirectory)
        {
            EnterDirectory(item);
            return;
        }
        try
        {
            Process.Start(new ProcessStartInfo(item.FullPath) { UseShellExecute = true });
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>用编辑器打开文件：外部编辑器模式（设置里已配置 exe）直接启动进程，
    /// 其余情况用应用内置编辑器（WPF AvalonEdit 窗口，独立线程）打开。</summary>
    private void OpenWithEditor(FileItemViewModel item)
    {
        if (AppSettingsService.EditorMode == "external"
            && !string.IsNullOrWhiteSpace(AppSettingsService.EditorPath))
        {
            try
            {
                Process.Start(new ProcessStartInfo(AppSettingsService.EditorPath, $"\"{item.FullPath}\""));
            }
            catch (Exception ex)
            {
                UpdateStatus(ex.Message);
            }
            return;
        }
        try
        {
            WpfEditorWindow.Open(item.FullPath, ThemeService.IsDark,
                LocalizationService.CurrentCulture != "en-US");
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>判定文本文件（本地右键"编辑"启用依据）：读头部 8KB 启发式探测（核心见 IsTextData）。
    /// 刻意不做严格 UTF-8 校验：GBK 等 ANSI 文本同样是可编辑目标（编辑器按 BOM/UTF-8/GBK 回退解码）。
    /// 读失败（锁定/权限）按非文本处理，与编辑器同样打不开的行为一致。头部 8KB 本地读盘亚毫秒，
    /// 菜单同步构建前完成无感知，故无需扩展名白名单（白名单会误伤 Makefile/.gitignore 等无扩展名文本）。</summary>
    private static bool IsTextFile(string path)
    {
        const int ProbeBytes = 8192;
        try
        {
            using var fs = File.OpenRead(path);
            var len = (int)Math.Min(fs.Length, ProbeBytes);
            if (len == 0)
            {
                return true; // 空文件可编辑
            }

            Span<byte> buf = stackalloc byte[ProbeBytes];
            fs.ReadExactly(buf[..len]);
            return IsTextData(buf[..len]);
        }
        catch
        {
            return false;
        }
    }

    /// <summary>文本探测核心（本地文件与远端头部共用）：UTF-16/UTF-8 BOM 视为文本；
    /// 含 NUL 字节视为二进制；控制字符（除 \t\n\r\f\v）占比 >5% 视为二进制。</summary>
    private static bool IsTextData(ReadOnlySpan<byte> data)
    {
        if (data.Length >= 2 && (data[0] == 0xFF && data[1] == 0xFE || data[0] == 0xFE && data[1] == 0xFF))
        {
            return true; // UTF-16 BOM (LE/BE), 内容含 NUL 属正常
        }

        if (data.Length >= 3 && data[0] == 0xEF && data[1] == 0xBB && data[2] == 0xBF)
        {
            return true; // UTF-8 BOM
        }

        var controls = 0;
        foreach (var b in data)
        {
            if (b == 0)
            {
                return false; // NUL 字节 → 二进制
            }

            if (b < 0x20 && b != 0x09 && b != 0x0A && b != 0x0D && b != 0x0B && b != 0x0C)
            {
                controls++;
            }
        }

        return controls * 100 <= data.Length * 5; // 控制字符占比 ≤5% 视为文本
    }

    // ───────────────────────── 远程编辑（下载 → 打开 → 保存自动回传） ─────────────────────────

    /// <summary>远程编辑允许的最大文件大小（与内置编辑器一致，10 MB）。</summary>
    private const long RemoteEditMaxBytes = 10 * 1024 * 1024;

    /// <summary>远程编辑扩展名白名单：命中即视为文本，点击时跳过内容探测（免一次网络往返）。</summary>
    private static readonly HashSet<string> RemoteTextExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        "txt", "md", "log", "csv", "tsv", "ini", "cfg", "conf", "config", "json", "xml",
        "yml", "yaml", "toml", "properties", "sql", "html", "htm", "css", "scss",
        "js", "mjs", "ts", "jsx", "tsx", "vue", "py", "rb", "pl", "php", "java", "kt",
        "swift", "c", "h", "cpp", "hpp", "cc", "cs", "go", "rs", "sh", "bash", "zsh",
        "bat", "cmd", "ps1", "lua", "r", "m", "dart",
        "gradle", "sln", "csproj", "props", "targets", "editorconfig", "gitignore", "gitattributes",
    };

    /// <summary>确定二进制的扩展名：菜单直接置灰（与 10 MB 上限双保险）。</summary>
    private static readonly HashSet<string> RemoteBinaryExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        "exe", "dll", "so", "dylib", "a", "lib", "obj", "o", "bin", "iso", "img",
        "zip", "rar", "7z", "gz", "tgz", "bz2", "xz", "zst", "tar", "jar", "war",
        "class", "msi", "apk", "jpg", "jpeg", "png", "gif", "bmp", "ico", "webp",
        "tif", "tiff", "svgz", "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx",
        "mp3", "wav", "flac", "ogg", "mp4", "avi", "mkv", "mov", "wmv", "webm",
        "ttf", "otf", "woff", "woff2", "eot", "db", "sqlite", "sqlite3", "pdb",
    };

    /// <summary>远程编辑菜单预启用：非二进制扩展名且 ≤10 MB 即启用（白名单/未知扩展名均可点，
    /// 未知扩展名点击时再做头部探测回退；二进制扩展名直接置灰）。</summary>
    private static bool RemoteEditMenuEnabled(FileItemViewModel item)
        => item.Size <= RemoteEditMaxBytes
           && !RemoteBinaryExtensions.Contains(Path.GetExtension(item.Name).TrimStart('.'));

    /// <summary>远程编辑：下载临时副本 → 编辑器打开 → 保存回传。
    /// 内置编辑器有保存回调（保存时 mtime 冲突比对 + 确认后上传回传 + 关窗清理临时目录）；
    /// 主机卡片/全局设置配置了外部编辑器时走外部编辑器——无保存回调，经文件监视会话
    /// （ExternalEditSession）检测保存后弹窗询问"是否保存编辑结果"。
    /// 菜单已按扩展名预启用；此处仅对未知扩展名（含无扩展名）补一次 8KB 头部内容探测。</summary>
    private async Task OpenRemoteWithEditorAsync(FileItemViewModel item)
    {
        // 同一文件再次编辑: 先结束上一次的外部编辑会话——旧 watcher 会把本次
        // 下载写入误判为用户保存而弹回传询问。
        if (_editSessions.Remove(item.FullPath, out var old))
        {
            old.Dispose();
        }

        if (!RemoteTextExtensions.Contains(Path.GetExtension(item.Name).TrimStart('.')))
        {
            try
            {
                var head = await _sftp.ReadHeadAsync(item.FullPath, 8192);
                if (!IsTextData(head))
                {
                    UpdateStatus(LocalizationService.GetString("RemoteEditNotText"));
                    return;
                }
            }
            catch (Exception ex)
            {
                UpdateStatus(ex.Message);
                return;
            }
        }

        // 临时目录: %TEMP%\iFreeSftp\RemoteEdit\<主机>\<远端完整路径哈希前16>\原名，
        // 按路径哈希隔离避免重名互踩；关窗即清理（残留也会被系统清理临时文件回收）。
        var dir = Path.Combine(
            Path.GetTempPath(), "iFreeSftp", "RemoteEdit", SanitizeFileName(_sftp.Host),
            Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(item.FullPath)))[..16]);
        var localPath = Path.Combine(dir, SanitizeFileName(item.Name));
        try
        {
            Directory.CreateDirectory(dir);
            await _sftp.DownloadAsync(item.FullPath, localPath);
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
            TryDeleteDir(dir);
            return;
        }

        // 回传基准 mtime（下载时的远端时间）; 取不到时用 MinValue 跳过冲突比对直接上传。
        DateTimeOffset baseMtime;
        try
        {
            baseMtime = await _sftp.GetLastWriteTimeAsync(item.FullPath);
        }
        catch
        {
            baseMtime = DateTimeOffset.MinValue;
        }

        // 编辑器决策: 主机卡片外部编辑器 → 全局外部编辑器 → 内置编辑器。
        // 外部编辑器无保存回调, 以文件监视会话检测保存（临时目录随视图关闭清理）。
        var editorPath = ResolveExternalEditor();
        if (editorPath is not null)
        {
            StartRemoteExternalEdit(item, localPath, baseMtime, editorPath);
            return;
        }

        try
        {
            WpfEditorWindow.Open(localPath, ThemeService.IsDark,
                LocalizationService.CurrentCulture != "en-US",
                onSaved: saved => DispatcherQueue.TryEnqueue(() =>
                    _ = UploadRemoteEditAsync(item, localPath, () => baseMtime, v => baseMtime = v)),
                onClosed: () => TryDeleteDir(dir));
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
            TryDeleteDir(dir);
        }
    }

    /// <summary>外部编辑器路径解析: 全局设置; 未配置返回 null（用内置）。</summary>
    private string? ResolveExternalEditor()
    {
        return AppSettingsService.EditorMode == "external"
            && !string.IsNullOrWhiteSpace(AppSettingsService.EditorPath)
                ? AppSettingsService.EditorPath
                : null;
    }

    /// <summary>启动外部编辑器远程编辑会话: 建文件监视（按远端路径索引, 重编辑覆盖旧会话）+
    /// 启动编辑器进程。启动失败即结束会话（临时副本留存 Temp 无害, 同路径重编辑会覆盖）;
    /// 成功后会话随视图关闭统一回收。</summary>
    private void StartRemoteExternalEdit(FileItemViewModel item, string localPath,
        DateTimeOffset baseMtime, string editorPath)
    {
        var session = new Services.ExternalEditSession(localPath, DispatcherQueue, () =>
            _ = OnExternalEditChangedAsync(item, localPath, () => baseMtime, v => baseMtime = v));
        _editSessions[item.FullPath] = session;
        try
        {
            Process.Start(new ProcessStartInfo(editorPath, $"\"{localPath}\""));
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
            _editSessions.Remove(item.FullPath);
            session.Dispose();
        }
    }

    /// <summary>外部编辑器保存检测回调（UI 线程）: 弹窗询问"文件缓存已经改变, 是否保存编辑结果"
    /// → 确认则复用内置编辑器的回传链路（远端 mtime 冲突比对 + 上传 + 刷新列表）;
    /// 取消则暂不回传——会话已更新基准快照, 编辑器再次保存时会再次询问。</summary>
    private async Task OnExternalEditChangedAsync(FileItemViewModel item, string localPath,
        Func<DateTimeOffset> getBase, Action<DateTimeOffset> setBase)
    {
        if (_disposed
            || !await FileOpsDialog.ShowRemoteEditChangedAsync(this, item.Name))
        {
            return;
        }

        await UploadRemoteEditAsync(item, localPath, getBase, setBase);
    }

    /// <summary>远程编辑保存回传（UI 线程）：远端 mtime 与下载基准差 ≥1 秒视为他人已改 →
    /// 确认弹窗（覆盖/取消）；上传成功后重读远端 mtime 作为新基准（避免下次保存误报冲突）并刷新列表。</summary>
    private async Task UploadRemoteEditAsync(FileItemViewModel item, string localPath,
        Func<DateTimeOffset> getBase, Action<DateTimeOffset> setBase)
    {
        try
        {
            var current = await _sftp.GetLastWriteTimeAsync(item.FullPath);
            if (Math.Abs((current - getBase()).TotalSeconds) >= 1
                && !await FileOpsDialog.ShowRemoteConflictAsync(this))
            {
                return;
            }

            await _sftp.UploadAsync(localPath, item.FullPath);
            setBase(await _sftp.GetLastWriteTimeAsync(item.FullPath));
            UpdateStatus($"\"{item.Name}\" {LocalizationService.GetString("RemoteEditUploaded")}");
            RefreshListKeepingPath();
        }
        catch (Exception ex)
        {
            UpdateStatus(ex.Message);
        }
    }

    /// <summary>远端文件名转本地安全名（Linux 允许 : \ 等 Windows 非法字符，替换为下划线）。</summary>
    private static string SanitizeFileName(string name)
    {
        var invalid = Path.GetInvalidFileNameChars();
        var sb = new StringBuilder(name.Length);
        foreach (var ch in name)
        {
            sb.Append(Array.IndexOf(invalid, ch) >= 0 ? '_' : ch);
        }

        return sb.Length == 0 ? "file" : sb.ToString();
    }

    /// <summary>尽力删除临时目录（编辑器线程可能仍持句柄，失败留下残留无害，目录按路径哈希可复用）。</summary>
    private static void TryDeleteDir(string dir)
    {
        try
        {
            Directory.Delete(dir, recursive: true);
        }
        catch
        {
            // 句柄占用等失败忽略。
        }
    }

    /// <summary>传输防重入 (传输期间重复点击菜单直接提示)。</summary>
    private bool _transferring;

    /// <summary>
    /// "传输到另一侧"菜单可用性 (四方向统一判定): 两侧均定位到有效目录;
    /// 远程源须本侧在线, 远程目标须对侧在线。
    /// </summary>
    private bool CanTransferToOpposite()
        => OppositeView is { } opp
           && !string.IsNullOrEmpty(_currentPath)
           && !string.IsNullOrEmpty(opp.CurrentPath)
           && (Mode == FileViewMode.Local || _sftp.IsConnected)
           && (opp.Mode == FileViewMode.Local || opp.SftpService.IsConnected);

    /// <summary>两侧远程会话是否同一主机 (host:port 相同 → 服务端 cp 直达, 零本地流量)。</summary>
    private bool SameRemoteHost(FileView opp)
        => string.Equals(_sftp.Host, opp.SftpService.Host, StringComparison.OrdinalIgnoreCase)
           && _sftp.Port == opp.SftpService.Port;

    /// <summary>
    /// 传输条目到对侧窗格当前目录 (标准方式): 逐项检查目标同名冲突 → 弹确认
    /// (覆盖/跳过/取消 + "应用到本批所有冲突项"勾选后对整批剩余冲突统一生效) →
    /// 整批入队后由并发 worker 池执行 (并发数在"传输"设置组配置)。四方向:
    /// 本地→本地=复制; 本地→远程=上传; 远程→本地=下载; 远程→远程=同主机服务端
    /// cp 直达 (失败/跨主机回退本地临时目录中转)。目录递归传输, 递归内部同名
    /// 合并/覆盖不再逐项询问。单项失败不影响其余任务 (失败项进"传输失败"页签)。
    /// 调用来源: 右键菜单 (取列表当前选择) 与跨窗格拖拽放置 (取被拖拽条目)。
    /// </summary>
    /// <param name="dragItems">
    /// 拖拽放置传入的被拖拽条目; null = 右键菜单, 取列表当前选择。
    /// </param>
    private async Task TransferToOppositeAsync(IReadOnlyList<FileItemViewModel>? dragItems = null)
    {
        var opp = OppositeView;
        if (opp is null || opp.CurrentPath is not { Length: > 0 } destDir
            || string.IsNullOrEmpty(_currentPath))
        {
            UpdateStatus(LocalizationService.GetString("StatusCopyTargetInvalid"));
            return;
        }

        var items = (dragItems ?? FileList.SelectedItems.OfType<FileItemViewModel>())
            .Where(i => !i.IsPlaceholder && !string.IsNullOrEmpty(_currentPath)).ToList();
        if (items.Count == 0)
        {
            return;
        }

        // 连接双保险 (菜单判定同条件, 状态变化后点击兜底)
        if ((Mode == FileViewMode.Remote && !_sftp.IsConnected)
            || (opp.Mode == FileViewMode.Remote && !opp.SftpService.IsConnected))
        {
            UpdateStatus(LocalizationService.GetString("StatusDisconnected"));
            return;
        }

        if (_transferring)
        {
            UpdateStatus(LocalizationService.GetString("StatusTransferBusy"));
            return;
        }

        bool destRemote = opp.Mode == FileViewMode.Remote;
        _transferring = true;
        // 跨窗格传输日志双发: 本 tab 与对侧 tab 的日志池都记 (硬拆会丢上下文)。
        var transferScopes = TransferScopes(opp);
        try
        {
            // —— 阶段一: 逐项冲突决策 (弹窗串行, 尚不入队; 取消则整批不入队) ——
            // 决策缓存: null = 尚未询问 (后续冲突逐项弹窗); true/false = 全部覆盖/全部跳过
            bool? overwriteAll = null;
            int skipped = 0;
            // ForceOverwrite: 用户对本条明确选了"覆盖"——执行体据此跳过续传追加决策,
            // 防"远端较小旧文件"被误判为中断半成品而缝合成损坏内容 (审核 N1)。
            // 目标不存在时恒 false: 首传语义, 任务中断重跑时目标侧半成品仍可正常续传。
            var accepted = new List<(FileItemViewModel Item, string Dest, bool DestExists, bool ForceOverwrite)>();
            foreach (var item in items)
            {
                string dest = destRemote
                    ? JoinRemotePath(destDir, item.Name)
                    : Path.Combine(destDir, item.Name);

                bool forceOverwrite = false;
                bool destExists = await TargetEntryExistsAsync(opp, dest);
                if (destExists)
                {
                    if (overwriteAll is null)
                    {
                        var source = (Size: NonEmpty(item.DisplaySize), Time: NonEmpty(item.DisplayModified));
                        var target = await TargetInfoAsync(opp, dest);
                        var (result, applyAll) = await FileOpsDialog.ShowTransferConflictAsync(this, item.Name, source, target);
                        if (result == TransferConflictResult.Cancel)
                        {
                            UpdateStatus(LocalizationService.GetString("StatusTransferCancelled"));
                            return;
                        }
                        if (applyAll)
                        {
                            overwriteAll = result == TransferConflictResult.Overwrite;
                        }
                        if (result == TransferConflictResult.Skip)
                        {
                            skipped++;
                            continue;
                        }

                        forceOverwrite = result == TransferConflictResult.Overwrite;
                    }
                    else if (overwriteAll == false)
                    {
                        skipped++;
                        continue;
                    }
                    else
                    {
                        forceOverwrite = true; // 命中"全部覆盖"缓存
                    }
                }

                accepted.Add((item, dest, destExists, forceOverwrite));
            }

            // —— 阶段二: 整批入队 (等待) 并构建执行体, worker 池并发执行 ——
            // (并发数 1 = 全部复用主连接串行; >1 每任务独立连接经批次连接池借还)
            // 目录条目展开为逐文件任务 (递归嵌套): 队列按文件可见进度/失败独立记账,
            // 并发池按文件并行 (目录单任务会占死一个 worker 串行递归)。空目录一条
            // 创建任务保证目标结构完整; 含文件的父目录由文件任务按需创建 (批级
            // 目录创建任务共享, 全批等待同一任务完成后再上传——防并发 "no such file")。
            var workItems = new List<(Guid QueueId, Func<TransferBatchConnections, Task> Run)>();
            using var connections = new TransferBatchConnections();
            var dirTasks = new ConcurrentDictionary<string, Lazy<Task>>();
            foreach (var (item, dest, destExists, forceOverwrite) in accepted)
            {
                if (!item.IsDirectory)
                {
                    var queueTask = TransferQueueService.Enqueue(item.FullPath, dest, NonEmpty(item.DisplaySize), item.Size);
                    queueTask.LogScopes = transferScopes;
                    var progress = new Action<long>(b => TransferQueueService.Report(queueTask.Id, b));
                    queueTask.Run = BuildTransferRunner(opp, item, dest, destExists, forceOverwrite, progress,
                        () => queueTask.IsResumed = true, transferScopes);
                    AttachVerifyHook(queueTask, opp, destRemote ? item.FullPath : dest, destRemote ? dest : item.FullPath);
                    workItems.Add((queueTask.Id, queueTask.Run));
                    continue;
                }

                // 官方构建 (完整传输核心) 同主机目录: 整树直达——单条 cp -a 服务端完成
                // 整棵子树, 免逐文件展开与逐文件 exec (大目录数量级提速); 队列单任务,
                // 失败回退本地中转 (递归)。Stub 构建不走此路径, 保持逐文件展开。
                if (Mode == FileViewMode.Remote && opp.Mode == FileViewMode.Remote
                    && SameRemoteHost(opp) && CoreServices.Transfer.IsFullImplementation)
                {
                    var treeTask = TransferQueueService.Enqueue(item.FullPath, dest, NonEmpty(item.DisplaySize));
                    treeTask.LogScopes = transferScopes;
                    treeTask.Run = BuildServerTreeRunner(opp, item, dest, destExists);
                    workItems.Add((treeTask.Id, treeTask.Run));
                    continue;
                }

                // tar 打包加速 (T12): 本地→远程 / 远程→本地 的目录传输, 文件数达阈值
                // 时走"单流打包"替代逐文件展开——整目录单任务 (队列单行, 无逐文件进度)。
                // 远程→远程不走 (cp -a / 中转已覆盖); 本地枚举廉价, 远端探一条 find
                // 计数; 判定不可用或执行失败都回退下面的逐文件展开, 结果语义不变。
                if ((destRemote && Mode == FileViewMode.Local)
                    || (!destRemote && Mode == FileViewMode.Remote))
                {
                    var fileCount = Mode == FileViewMode.Local
                        ? CountLocalFiles(item.FullPath)
                        : await TarAccelService.CountRemoteFilesAsync(_sftp, item.FullPath);
                    if (TarAccelService.ShouldUse(fileCount))
                    {
                        var tarTask = TransferQueueService.Enqueue(item.FullPath, dest, NonEmpty(item.DisplaySize));
                        tarTask.LogScopes = transferScopes;
                        tarTask.IsTarMode = true;
                        var tarProgress = new Action<long>(b => TransferQueueService.Report(tarTask.Id, b));
                        tarTask.Run = BuildTarRunner(opp, item, dest, destRemote, tarProgress, transferScopes);
                        workItems.Add((tarTask.Id, tarTask.Run));
                        TerminalLogService.Log(string.Format(
                            LocalizationService.GetString("LogTarMode"), item.Name, fileCount), transferScopes);
                        continue;
                    }
                }

                // 目录展开: 枚举全部子目录/文件 (相对路径), 失败跳过该目录并记命令窗
                List<string> subDirs;
                List<(string RelPath, long Length)> subFiles;
                try
                {
                    (subDirs, subFiles) = Mode == FileViewMode.Remote
                        ? await EnumerateRemoteDirTreeAsync(_sftp, item.FullPath)
                        : EnumerateLocalDirTree(item.FullPath);
                }
                catch (Exception ex)
                {
                    skipped++;
                    TerminalLogService.Log(string.Format(
                        LocalizationService.GetString("LogDirExpandFailed"), item.Name, ex.Message), transferScopes);
                    continue;
                }

                // 文件任务 (源/目标按相对路径拼接; 大小来自枚举)。
                // 顶层文件 (直接位于目录根下) 的目标父目录 = dest 本身——传空串会让
                // 下载/本地复制分支 Directory.CreateDirectory("") 抛"路径不能为空";
                // 传 dest 幂等创建已存在目录, 无害且统一三分支语义。
                foreach (var (rel, length) in subFiles)
                {
                    var src = JoinRemotePath(item.FullPath, rel);
                    var dst = JoinRemotePath(dest, rel);
                    var parentRel = ParentRel(rel);
                    var qt = TransferQueueService.Enqueue(src, dst, FileEntry.FormatSize(length), length);
                    qt.LogScopes = transferScopes;
                    var p = new Action<long>(b => TransferQueueService.Report(qt.Id, b));
                    qt.Run = BuildExpandedFileRunner(
                        opp, src, dst, parentRel.Length == 0 ? dest : JoinRemotePath(dest, parentRel),
                        destRemote, forceOverwrite, p, dirTasks, () => qt.IsResumed = true, transferScopes);
                    AttachVerifyHook(qt, opp, destRemote ? src : dst, destRemote ? dst : src);
                    workItems.Add((qt.Id, qt.Run));
                }

                // 空目录任务 (不直接含文件的目录——含文件目录由文件任务按需建, 根除外)
                var parentSet = subFiles.Select(f => ParentRel(f.RelPath)).ToHashSet();
                foreach (var dirRel in subDirs.Where(d => !parentSet.Contains(d)))
                {
                    var src = JoinRemotePath(item.FullPath, dirRel);
                    var dst = JoinRemotePath(dest, dirRel);
                    var qt = TransferQueueService.Enqueue(src, dst, NonEmpty(string.Empty));
                    qt.LogScopes = transferScopes;
                    qt.Run = BuildEnsureDirRunner(opp, dst, destRemote, dirTasks);
                    workItems.Add((qt.Id, qt.Run));
                }
            }

            // 命令行面板叙述: 批量传输开始 (与状态栏并存, 终端留存可回看; 全跳过无任务则不输出)
            if (workItems.Count > 0)
            {
                TerminalLogService.Log(string.Format(
                    LocalizationService.GetString("LogTransferStart"), workItems.Count, destDir), transferScopes);
            }

            // 传输中节流刷新目标面板 (FileZilla/WinSCP 惯例: 传输过程目标目录逐步可见,
            // 而非全部传完才出现); 批次结束取消循环后仍做最终刷新兜底
            using var refreshCts = new CancellationTokenSource();
            var refreshLoop = RefreshOppositeWhileTransferringAsync(opp, destDir, refreshCts.Token);

            var (done, failed) = await ConcurrentTransferService.RunAsync(
                workItems, AppSettingsService.ConcurrentTransfers, connections);

            refreshCts.Cancel();
            try
            {
                await refreshLoop;
            }
            catch (OperationCanceledException)
            {
                // 传输结束取消刷新循环: 正常退出
            }

            UpdateStatus(failed > 0
                ? string.Format(LocalizationService.GetString("StatusTransferDoneFailed"), done, skipped, failed)
                : string.Format(LocalizationService.GetString("StatusTransferDone"), done, skipped));

            // 命令行面板叙述: 批量传输结束汇总
            if (workItems.Count > 0)
            {
                TerminalLogService.Log(string.Format(
                    LocalizationService.GetString("LogTransferDone"), done, skipped, failed), transferScopes);
            }
            if (done > 0)
            {
                opp.RefreshListKeepingPath();
            }
        }
        catch (Exception ex)
        {
            // 阶段一 (冲突探测) 异常在此展示; 阶段二异常已由执行器按项记账
            UpdateStatus(ex.Message);
        }
        finally
        {
            _transferring = false;
        }
    }

    /// <summary>
    /// 传输期间节流刷新目标窗格 (阶段二并发执行期间): 每 3s 检查一次, 仅当自上次
    /// 刷新后有任务新完成 (目标侧真的多出了内容) 且用户仍停在本次传输的目标目录时
    /// 才重列列表——新目录/新文件陆续出现 (惯例行为), 又不干扰用户翻到其他目录浏览、
    /// 不给服务器打无谓的列目录请求 (大文件慢传期间完成计数不变即不刷)。批次结束
    /// 经令牌取消; 远程刷新经主会话列目录 (与传输连接池无冲突, 并发 1 时与传输
    /// 串行排队无害)。
    /// </summary>
    private async Task RefreshOppositeWhileTransferringAsync(FileView opp, string destDir, CancellationToken token)
    {
        var baseline = TransferQueueService.GetSummary();
        var lastSeen = 0;
        try
        {
            using var timer = new PeriodicTimer(TimeSpan.FromSeconds(3));
            while (await timer.WaitForNextTickAsync(token))
            {
                var summary = TransferQueueService.GetSummary();
                var finished = summary.Done + summary.Failed - (baseline.Done + baseline.Failed);
                if (finished == 0 || finished == lastSeen)
                {
                    continue;
                }

                lastSeen = finished;
                // 用户仍停在目标目录才刷 (远程路径大小写敏感, 本地按 Windows 宽松比较)
                var sameDir = opp.Mode == FileViewMode.Remote
                    ? string.Equals(opp.CurrentPath, destDir, StringComparison.Ordinal)
                    : string.Equals(opp.CurrentPath, destDir, StringComparison.OrdinalIgnoreCase);
                if (!sameDir)
                {
                    continue;
                }

                opp.RefreshListKeepingPath();
            }
        }
        catch (OperationCanceledException)
        {
            // 批次结束取消循环: 正常退出
        }
    }

    // ───────────────────── 跨窗格拖拽传输 (左右面板互拖) ─────────────────────

    /// <summary>
    /// 进行中的窗格间拖拽载荷 (静态: WinUI 原生拖拽的 DataPackage 不便承载
    /// 托管对象引用, 应用内拖拽用静态字段最稳; 拖拽由单一指针发起, 无并发)。
    /// </summary>
    private static (FileView Source, List<FileItemViewModel> Items)? _paneDrag;

    /// <summary>
    /// 条目拖拽发起 (ListView CanDragItems): 记录来源视图与被拖拽条目供对侧
    /// 放置判定。比对占位行无语义, 过滤后为空则取消拖拽。
    /// </summary>
    private void FileList_DragItemsStarting(object sender, DragItemsStartingEventArgs args)
    {
        var items = args.Items.OfType<FileItemViewModel>().Where(i => !i.IsPlaceholder).ToList();
        if (items.Count == 0 || _emptyState)
        {
            args.Cancel = true;
            return;
        }

        args.Data.RequestedOperation = Windows.ApplicationModel.DataTransfer.DataPackageOperation.Copy;
        _paneDrag = (this, items);
    }

    /// <summary>拖拽结束 (放置/取消/丢失): 清理载荷。</summary>
    private void FileList_DragItemsCompleted(ListViewBase sender, DragItemsCompletedEventArgs args)
    {
        if (_paneDrag?.Source == this)
        {
            _paneDrag = null;
        }
    }

    /// <summary>
    /// 对侧条目拖到本侧悬停: 仅接受来源为对侧窗格且两侧就绪的拖拽 (本侧自身
    /// 拖动 / 外部应用拖入一律显示禁止光标), 接受时高亮列表区并在光标旁
    /// 显示目标目录名。悬停期间条件可变, 每次 DragOver 重判。
    /// </summary>
    private void FileListHost_DragOver(object sender, DragEventArgs e)
    {
        e.Handled = true;
        if (_paneDrag is not { } drag
            || drag.Source == this
            || drag.Source.OppositeView != this
            || !CanTransferToOpposite())
        {
            e.AcceptedOperation = Windows.ApplicationModel.DataTransfer.DataPackageOperation.None;
            SetDropHint(false);
            return;
        }

        e.AcceptedOperation = Windows.ApplicationModel.DataTransfer.DataPackageOperation.Copy;
        // 光标旁标题: "传输到 <目标目录名>" (取末段路径, 根目录显示分隔符)
        var leaf = _currentPath.TrimEnd('/', '\\');
        var sep = leaf.LastIndexOfAny(new[] { '/', '\\' });
        leaf = sep >= 0 ? leaf[(sep + 1)..] : _currentPath;
        if (string.IsNullOrEmpty(leaf))
        {
            leaf = "/";
        }

        e.DragUIOverride.Caption = string.Format(
            LocalizationService.GetString("DragDropCaption"), leaf);
        e.DragUIOverride.IsCaptionVisible = true;
        e.DragUIOverride.IsContentVisible = true;
        SetDropHint(true);
    }

    private void FileListHost_DragLeave(object sender, DragEventArgs e)
    {
        SetDropHint(false);
    }

    /// <summary>
    /// 对侧条目放到本侧: 触发来源视图的"传输到另一侧"流程 (冲突确认/入队/
    /// 并发执行全部复用右键菜单同一路径), 传入被拖拽条目而非当前选择。
    /// </summary>
    private void FileListHost_Drop(object sender, DragEventArgs e)
    {
        e.Handled = true;
        SetDropHint(false);
        if (_paneDrag is not { } drag
            || drag.Source == this
            || drag.Source.OppositeView != this)
        {
            return;
        }

        var source = drag.Source;
        var items = drag.Items;
        _paneDrag = null;
        _ = source.TransferToOppositeAsync(items);
    }

    /// <summary>放置提示层显隐 (填充 + 描边两叠层)。</summary>
    private void SetDropHint(bool visible)
    {
        var state = visible ? Visibility.Visible : Visibility.Collapsed;
        DropHintFill.Visibility = state;
        DropHintBorder.Visibility = state;
    }

    /// <summary>
    /// 按传输方向构建单条目的传输执行体 (worker 领取任务时调用): 本地→远程上传 /
    /// 远程→本地下载 / 远程→远程 (同主机 cp 直达, 失败回退中转) / 本地→本地复制。
    /// 执行体接受批次连接池参数而非闭包绑定——任务可持久化重跑 ("重置并重新请求
    /// 所有"用新池重放)。仅服务顶层文件条目——顶层目录已在入队阶段展开为逐文件任务。
    /// onResumed: 实际走断点续传追加分支时回调 (队列行标记"续传中")。
    /// </summary>
    private Func<TransferBatchConnections, Task> BuildTransferRunner(
        FileView opp, FileItemViewModel item, string dest, bool destExists, bool forceOverwrite,
        Action<long> progress, Action? onResumed = null, string[]? transferScopes = null)
    {
        bool destRemote = opp.Mode == FileViewMode.Remote;

        if (destRemote && Mode == FileViewMode.Local)
        {
            // 本地→远程上传: 目标侧连接 (装饰器注入暂停检查点: 每传输块检查, 暂停
            // 中断当前文件留断点; 归还用原 ops——装饰器非 TransferConnection 不参与借还)
            return async connections =>
            {
                var ops = await connections.AcquireAsync(opp.SftpService);
                try
                {
                    await TransferFileWithResumeAsync(new PauseInterruptibleOps(ops), item.FullPath, dest,
                        upload: true, forceOverwrite, progress, onResumed, transferScopes);
                }
                finally
                {
                    connections.Release(opp.SftpService, ops);
                }
            };
        }

        if (!destRemote && Mode == FileViewMode.Remote)
        {
            // 远程→本地下载: 源侧连接 (暂停装饰器同上)
            return async connections =>
            {
                var ops = await connections.AcquireAsync(_sftp);
                try
                {
                    await TransferFileWithResumeAsync(new PauseInterruptibleOps(ops), item.FullPath, dest,
                        upload: false, forceOverwrite, progress, onResumed, transferScopes);
                }
                finally
                {
                    connections.Release(_sftp, ops);
                }
            };
        }

        if (destRemote)
        {
            // 远程→远程: 同主机服务端 cp 直达 (零本地流量, 命令策略在闭源核心, 经
            // 主会话命令通道下发, 多路自动排队); cp 不可用 (Windows 服务器/命令失败/
            // 跨主机) 回退本地临时目录中转——Stub 构建无服务端直达, 同主机也走中转
            if (SameRemoteHost(opp) && CoreServices.Transfer.IsFullImplementation)
            {
                return async connections =>
                {
                    if (await CoreServices.Transfer.ServerCopyAsync(
                            _sftp, item.FullPath, dest, item.IsDirectory, destExists))
                    {
                        return; // 服务端复制成功
                    }

                    await RelayRemoteToRemoteAsync(opp, item.FullPath, dest, item.IsDirectory, forceOverwrite, progress, connections, transferScopes);
                };
            }

            return connections =>
                RelayRemoteToRemoteAsync(opp, item.FullPath, dest, item.IsDirectory, forceOverwrite, progress, connections, transferScopes);
        }

        // 本地→本地复制 (无 SSH 参与, 线程池执行避免占 UI)
        return _ => Task.Run(() => CopyEntryRecursive(item.FullPath, dest));
    }

    /// <summary>按传输方向为单文件队列任务挂校验执行体（T2, 设置开启且为本地↔远程
    /// 方向才有; 远程→远程/本地→本地/树直达/tar/空目录无单文件语义, 不校验）。
    /// 远端侧会话按方向取: 上传 = 目标侧 opp.SftpService, 下载 = 源侧 _sftp。</summary>
    private void AttachVerifyHook(TransferTask task, FileView opp, string localPath, string remotePath)
    {
        if (!AppSettingsService.VerifyAfterTransfer)
        {
            return;
        }

        bool destRemote = opp.Mode == FileViewMode.Remote;
        var sftp = (destRemote, Mode) switch
        {
            (true, FileViewMode.Local) => opp.SftpService,   // 上传: 远端为目标侧
            (false, FileViewMode.Remote) => _sftp,           // 下载: 远端为源侧
            _ => null,                                       // 远程→远程 / 本地→本地: 不校验
        };
        if (sftp is not null)
        {
            task.VerifyHook = BuildVerifyHook(sftp, localPath, remotePath);
        }
    }

    private Func<Task<VerifyOutcome>> BuildVerifyHook(
        SftpSessionService sftp, string localPath, string remotePath)
        => async () =>
        {
            var algorithm = VerifyPolicy.PickAlgorithm(
                AppSettingsService.VerifyAlgorithm, sftp.ServerSha256, sftp.ServerSha512, sftp.ServerSm3);
            if (algorithm is null)
            {
                return new VerifyOutcome(VerifyResultKind.Skipped);
            }

            var localHex = await ComputeLocalHashAsync(localPath, algorithm);
            var remoteHex = await sftp.TryComputeServerHashAsync(remotePath, algorithm);
            return new VerifyOutcome(
                VerifyPolicy.Decide(localHex, remoteHex, algorithmAvailable: true),
                localHex ?? string.Empty, remoteHex ?? string.Empty, algorithm);
        };

    /// <summary>本地文件杂凑（分块读流, 线程池执行; SM3 走自带托管实现）。
    /// 读失败→null→校验跳过。</summary>
    private static async Task<string?> ComputeLocalHashAsync(string path, string algorithm)
    {
        try
        {
            return await Task.Run(async () =>
            {
                await using var fs = new FileStream(path, FileMode.Open, FileAccess.Read,
                    FileShare.Read, 81920, FileOptions.Asynchronous | FileOptions.SequentialScan);
                var buffer = new byte[81920];
                if (algorithm == "SM3")
                {
                    var sm3 = new Sm3();
                    int read;
                    while ((read = await fs.ReadAsync(buffer)) > 0)
                    {
                        sm3.AppendData(buffer, read);
                    }

                    return sm3.GetHashHex();
                }

                using var hash = IncrementalHash.CreateHash(
                    algorithm == "SHA512" ? HashAlgorithmName.SHA512 : HashAlgorithmName.SHA256);
                int n;
                while ((n = await fs.ReadAsync(buffer)) > 0)
                {
                    hash.AppendData(buffer, 0, n);
                }

                return Convert.ToHexString(hash.GetHashAndReset()).ToLowerInvariant();
            });
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// 同主机目录整树直达执行体 (官方构建): 单条 cp -a 服务端完成整棵子树,
    /// 失败 (Windows 服务器/命令通道异常) 回退本地临时目录中转 (递归)。
    /// 执行体接受批次连接池参数而非闭包绑定——任务可持久化重跑 (见 BuildTransferRunner)。
    /// </summary>
    private Func<TransferBatchConnections, Task> BuildServerTreeRunner(
        FileView opp, FileItemViewModel item, string dest, bool destExists)
    {
        return async connections =>
        {
            if (await CoreServices.Transfer.ServerCopyAsync(
                    _sftp, item.FullPath, dest, sourceIsDirectory: true, destExists))
            {
                return; // 服务端整树复制成功
            }

            await RelayRemoteToRemoteAsync(opp, item.FullPath, dest, isDirectory: true, forceOverwrite: false, progress: null, connections);
        };
    }

    /// <summary>
    /// tar 打包加速执行体 (T12): 整目录单任务。上传=本地 tar stdout 直写远端临时
    /// 文件后 exec 解包; 下载=远端 exec 打包经 SFTP 下载单文件后本地解包。tar 路径
    /// 不可用返回 false → 回退逐文件（现状行为, 普通失败语义）; tar 路径执行中抛错
    /// → 附"整体传输不支持文件级续传"提示（用户需知情: 失败只能整体重试）。
    /// </summary>
    private Func<TransferBatchConnections, Task> BuildTarRunner(
        FileView opp, FileItemViewModel item, string dest, bool destRemote, Action<long> progress,
        string[]? transferScopes = null)
    {
        if (destRemote && Mode == FileViewMode.Local)
        {
            return async connections =>
            {
                var ops = await connections.AcquireAsync(opp.SftpService);
                try
                {
                    bool tarOk;
                    try
                    {
                        tarOk = await TarAccelService.UploadDirAsync(
                            opp.SftpService, ops, item.FullPath, dest, progress, transferScopes);
                    }
                    catch (Exception ex)
                    {
                        throw TarFailWrapped(ex);
                    }

                    if (!tarOk)
                    {
                        await UploadDirAsync(ops, item.FullPath, dest);
                    }
                }
                finally
                {
                    connections.Release(opp.SftpService, ops);
                }
            };
        }

        return async connections =>
        {
            var ops = await connections.AcquireAsync(_sftp);
            try
            {
                bool tarOk;
                try
                {
                    tarOk = await TarAccelService.DownloadDirAsync(
                        _sftp, ops, item.FullPath, dest, progress, transferScopes);
                }
                catch (Exception ex)
                {
                    throw TarFailWrapped(ex);
                }

                if (!tarOk)
                {
                    await DownloadDirAsync(ops, item.FullPath, dest);
                }
            }
            finally
            {
                connections.Release(_sftp, ops);
            }
        };
    }

    /// <summary>tar 任务失败包装: 前置"整体传输, 失败只能整体重试"提示（T12 约束的用户知情）。</summary>
    private static Exception TarFailWrapped(Exception ex)
        => new InvalidOperationException(
            LocalizationService.GetString("QTarFailHint") + " " + ex.Message, ex);

    /// <summary>本地目录文件数（tar 判定用; 只数文件不建表, 失败返回 -1 不走 tar）。</summary>
    private static int CountLocalFiles(string dir)
    {
        try
        {
            return Directory.EnumerateFiles(dir, "*", SearchOption.AllDirectories).Count();
        }
        catch
        {
            return -1;
        }
    }

    /// <summary>
    /// 单文件传输（含断点续传决策, T1）: 按"续传"设置查对端已有大小, 纯函数
    /// <see cref="ResumePolicy.Decide"/> 判定追加/全量/冲突。低于续传阈值
    /// （设置-传输, 默认 1MB）的小文件只能全量重传。冲突（对端比源还大,
    /// 内容已变的强信号）不静默追加——记终端日志后按全量重传。forceOverwrite:
    /// 用户对该条目明确选了"覆盖"——跳过续传决策直接全量重传（全量上传/下载均
    /// 先截断再写, 天然覆盖语义）, 防"远端较小旧文件"被误判为中断半成品而追加
    /// 缝合成损坏内容 (审核 N1); 目标不存在时传 false, 首传与重跑续传语义不变。
    /// 追加分支本身抛异常（老服务器 SftpFileStream 定位不支持等）时回退全量:
    /// 两种回退目标（UploadFile/DownloadFile）都先截断, 不会留下缝合文件。
    /// </summary>
    private static async Task TransferFileWithResumeAsync(
        IRemoteTransferOps ops, string sourcePath, string destPath, bool upload, bool forceOverwrite,
        Action<long>? progress, Action? onResumed = null, string[]? logScopes = null)
    {
        if (!AppSettingsService.ResumeEnabled || forceOverwrite)
        {
            await TransferFilePlainAsync(ops, sourcePath, destPath, upload, progress);
            return;
        }

        try
        {
            if (upload)
            {
                long localSize = await Task.Run(() =>
                {
                    try
                    {
                        return new FileInfo(sourcePath).Length;
                    }
                    catch
                    {
                        return 0L; // 取不到大小: 不走续传, 全量上传照常报错
                    }
                });

                if (localSize > 0)
                {
                    var remoteSize = await ops.TryGetSizeAsync(destPath);
                    var action = ResumePolicy.Decide(localSize, remoteSize, remoteSize >= 0, resumeEnabled: true,
                        thresholdBytes: AppSettingsService.ResumeThresholdKb * 1024L);
                    if (action == ResumeAction.Append)
                    {
                        onResumed?.Invoke();
                        await ops.UploadResumeAsync(sourcePath, destPath, remoteSize, progress);
                        return;
                    }

                    if (action == ResumeAction.Conflict)
                    {
                        TerminalLogService.Log(string.Format(
                            LocalizationService.GetString("LogResumeConflict"), destPath, remoteSize, localSize), logScopes);
                    }
                }
            }
            else
            {
                var remoteSize = await ops.TryGetSizeAsync(sourcePath);
                long localSize = 0L;
                if (File.Exists(destPath))
                {
                    try
                    {
                        localSize = new FileInfo(destPath).Length;
                    }
                    catch
                    {
                        localSize = 0L;
                    }
                }

                if (remoteSize > 0 && localSize > 0)
                {
                    var action = ResumePolicy.Decide(remoteSize, localSize, destExists: true, resumeEnabled: true,
                        thresholdBytes: AppSettingsService.ResumeThresholdKb * 1024L);
                    if (action == ResumeAction.Append)
                    {
                        onResumed?.Invoke();
                        await ops.DownloadResumeAsync(sourcePath, destPath, localSize, progress);
                        return;
                    }

                    if (action == ResumeAction.Conflict)
                    {
                        TerminalLogService.Log(string.Format(
                            LocalizationService.GetString("LogResumeConflict"), sourcePath, localSize, remoteSize), logScopes);
                    }
                }
            }
        }
        catch (Exception ex) when (ex is not OperationCanceledException)
        {
            // 续传路径自身失败（定位不支持/半途断网）: 全量重传兜底（先截断, 无缝合风险）
            TerminalLogService.Log(string.Format(
                LocalizationService.GetString("LogResumeFallback"), destPath, ex.Message), logScopes);
        }

        await TransferFilePlainAsync(ops, sourcePath, destPath, upload, progress);
    }

    /// <summary>无续传的普通单文件传输（续传关闭/目标不存在/续传失败兜底共用）。</summary>
    private static Task TransferFilePlainAsync(
        IRemoteTransferOps ops, string sourcePath, string destPath, bool upload, Action<long>? progress)
        => upload
            ? ops.UploadAsync(sourcePath, destPath, progress)
            : ops.DownloadAsync(sourcePath, destPath, progress);

    // —— 目录展开 (传输队列逐文件可见, 见阶段二) ——

    /// <summary>相对路径取父目录相对路径 (顶层返回空串)。</summary>
    private static string ParentRel(string rel)
    {
        var idx = rel.LastIndexOf('/');
        return idx < 0 ? string.Empty : rel[..idx];
    }

    /// <summary>
    /// 本地目录树递归枚举: 返回全部子目录与文件的相对路径 ('/' 分隔, 根不计入) +
    /// 文件字节大小。无权限子目录跳过 (不中止整棵树)。
    /// </summary>
    private static (List<string> Dirs, List<(string RelPath, long Length)> Files) EnumerateLocalDirTree(string rootDir)
    {
        var dirs = new List<string>();
        var files = new List<(string RelPath, long Length)>();

        void Walk(string dir, string rel)
        {
            IEnumerable<string> subDirs = [], subFiles = [];
            try
            {
                subDirs = Directory.EnumerateDirectories(dir);
            }
            catch (UnauthorizedAccessException)
            {
            }

            foreach (var sub in subDirs)
            {
                var name = Path.GetFileName(sub);
                var relChild = rel.Length == 0 ? name : rel + "/" + name;
                dirs.Add(relChild);
                Walk(sub, relChild);
            }

            try
            {
                subFiles = Directory.EnumerateFiles(dir);
            }
            catch (UnauthorizedAccessException)
            {
            }

            foreach (var f in subFiles)
            {
                long length = 0;
                try
                {
                    length = new FileInfo(f).Length;
                }
                catch (IOException)
                {
                }

                var name = Path.GetFileName(f);
                files.Add((rel.Length == 0 ? name : rel + "/" + name, length));
            }
        }

        Walk(rootDir, string.Empty);
        return (dirs, files);
    }

    /// <summary>
    /// 远程目录树递归枚举 (经主会话 _sshLock 串行): 返回全部子目录与文件的相对路径
    /// ('/' 分隔, 根不计入) + 文件字节大小。ListDirectory 的 "./name" 相对路径统一
    /// 拼接归一化为绝对路径。异常上抛 (该目录跳过并记日志)。
    /// </summary>
    private static async Task<(List<string> Dirs, List<(string RelPath, long Length)> Files)> EnumerateRemoteDirTreeAsync(
        IRemoteTransferOps sftp, string rootDir)
    {
        var dirs = new List<string>();
        var files = new List<(string RelPath, long Length)>();

        async Task Walk(string dir, string rel)
        {
            foreach (var entry in await sftp.ListDirectoryAsync(dir))
            {
                var relChild = rel.Length == 0 ? entry.Name : rel + "/" + entry.Name;
                if (entry.IsDirectory)
                {
                    dirs.Add(relChild);
                    await Walk(JoinRemotePath(dir, entry.Name), relChild);
                }
                else
                {
                    files.Add((relChild, entry.Length));
                }
            }
        }

        await Walk(rootDir, string.Empty);
        return (dirs, files);
    }

    /// <summary>
    /// 确保远端目录就绪, 返回该目录的批级创建任务: 同一目录全批共享一个
    /// Lazy&lt;Task&gt;——谁先登记谁逐段 mkdir, 其余调用方 await 同一任务, 目录真正
    /// 建好后文件任务才开始上传。旧实现 TryAdd 去重即返回, 并发下后到者不等
    /// 先到者建完就上传, 命中 SSH.NET "no such file"。逐段自底向上 mkdir,
    /// 失败吞异常 (已存在/无权限——前者无害, 后者由文件操作显式报错); 段序
    /// 自洽: 循环走到某段时其父必已就位 (本任务刚建或"已存在")。
    /// </summary>
    private static Task EnsureRemoteDirReadyAsync(
        IRemoteTransferOps ops, string remoteDir,
        ConcurrentDictionary<string, Lazy<Task>> dirTasks)
    {
        if (remoteDir.Length == 0)
        {
            return Task.CompletedTask;
        }

        return dirTasks.GetOrAdd(remoteDir, _ => new Lazy<Task>(async () =>
        {
            var path = string.Empty;
            foreach (var seg in remoteDir.Split('/', StringSplitOptions.RemoveEmptyEntries))
            {
                path += "/" + seg;
                try
                {
                    await ops.CreateDirectoryAsync(path);
                }
                catch
                {
                    // 已存在 / 无权限: 前者无害; 后者由文件操作显式报错
                }
            }
        })).Value;
    }

    /// <summary>
    /// 目录展开出的单文件任务执行体 (worker 调用): 先确保目标父目录存在
    /// (远端 await 批级共享创建任务 / 本地 CreateDirectory), 再按方向传输文件。
    /// 执行体接受批次连接池参数 (可持久化重跑, 见 BuildTransferRunner)。
    /// 远程→远程: 同主机 cp 失败不回退中转 (与顶层条目不同): 逐文件中转开销大,
    /// 失败进"传输失败"页签可单独重试; 跨主机 cp 不可用, 逐文件经本地中转。
    /// </summary>
    private Func<TransferBatchConnections, Task> BuildExpandedFileRunner(
        FileView opp, string src, string dst, string destParentDir, bool destRemote, bool forceOverwrite,
        Action<long> progress, ConcurrentDictionary<string, Lazy<Task>> dirTasks, Action? onResumed = null,
        string[]? transferScopes = null)
    {
        if (destRemote && Mode == FileViewMode.Local)
        {
            // 本地→远程上传: 目标侧连接, 先建父目录 (暂停装饰器见 BuildTransferRunner)
            return async connections =>
            {
                var ops = await connections.AcquireAsync(opp.SftpService);
                try
                {
                    await EnsureRemoteDirReadyAsync(ops, destParentDir, dirTasks);
                    await TransferFileWithResumeAsync(new PauseInterruptibleOps(ops), src, dst,
                        upload: true, forceOverwrite, progress, onResumed, transferScopes);
                }
                finally
                {
                    connections.Release(opp.SftpService, ops);
                }
            };
        }

        if (!destRemote && Mode == FileViewMode.Remote)
        {
            // 远程→本地下载: 源侧连接, 先建本地父目录 (暂停装饰器见 BuildTransferRunner)
            return async connections =>
            {
                var ops = await connections.AcquireAsync(_sftp);
                try
                {
                    Directory.CreateDirectory(destParentDir);
                    await TransferFileWithResumeAsync(new PauseInterruptibleOps(ops), src, dst,
                        upload: false, forceOverwrite, progress, onResumed, transferScopes);
                }
                finally
                {
                    connections.Release(_sftp, ops);
                }
            };
        }

        if (destRemote)
        {
            // 远程→远程: 先经目标侧连接确保父目录就绪, 之后同主机 cp -f 直达 (零本地
            // 流量, 命令策略在闭源核心; 失败不回退中转——逐文件中转开销大, 进"传输
            // 失败"页签可单独重试), 跨主机或 Stub 构建经本地临时目录中转
            return async connections =>
            {
                var ops = await connections.AcquireAsync(opp.SftpService);
                try
                {
                    await EnsureRemoteDirReadyAsync(ops, destParentDir, dirTasks);
                }
                finally
                {
                    connections.Release(opp.SftpService, ops);
                }

                if (SameRemoteHost(opp) && CoreServices.Transfer.IsFullImplementation)
                {
                    if (!await CoreServices.Transfer.ServerCopyAsync(
                            _sftp, src, dst, sourceIsDirectory: false, destExists: false))
                    {
                        throw new InvalidOperationException(LocalizationService.GetString("ErrServerCopy"));
                    }
                }
                else
                {
                    await RelayRemoteToRemoteAsync(opp, src, dst, isDirectory: false, forceOverwrite, progress, connections, transferScopes);
                }
            };
        }

        // 本地→本地复制 (无 SSH 参与, 线程池执行避免占 UI)
        return _ => Task.Run(() =>
        {
            Directory.CreateDirectory(destParentDir);
            File.Copy(src, dst, overwrite: true);
        });
    }

    /// <summary>空目录创建任务执行体 (目录无文件也保证目标结构完整, 见阶段二)。
    /// 执行体接受批次连接池参数 (可持久化重跑, 见 BuildTransferRunner)。</summary>
    private static Func<TransferBatchConnections, Task> BuildEnsureDirRunner(
        FileView opp, string dstDir, bool destRemote,
        ConcurrentDictionary<string, Lazy<Task>> dirTasks)
    {
        if (!destRemote)
        {
            return _ => Task.Run(() => Directory.CreateDirectory(dstDir));
        }

        return async connections =>
        {
            var ops = await connections.AcquireAsync(opp.SftpService);
            try
            {
                await EnsureRemoteDirReadyAsync(ops, dstDir, dirTasks);
            }
            finally
            {
                connections.Release(opp.SftpService, ops);
            }
        };
    }

    /// <summary>
    /// 跨主机远程→远程经本地中转: 下载到 %TEMP%\iFreeSftp\RemoteTransfer\&lt;hash&gt;\ 再
    /// 上传到目标, 完成即清理临时目录。临时目录按 源+目标 路径哈希稳定命名 (非每次
    /// 随机): 暂停中断保留半成品, 继续后重跑同一任务可续传——temp 已达源大小跳过
    /// 下载, 目标侧半成品经续传决策追加; 完成/失败才清理 (暂停中断置 keepTemp 后
    /// 原样上抛交 RunOneAsync 回排"等待")。forceOverwrite 仅单文件 temp→目标段消费
    /// (用户选"覆盖"时禁用追加防缝合损坏, 审核 N1); 源→temp 段不消费——temp 任务
    /// 私有, 大小判定安全; 目录分支天然覆盖语义, 不消费。源/目标两侧各借一条传输
    /// 连接 (用毕归还), 两侧操作均经暂停装饰器 (每传输块检查)。按路径操作, 顶层
    /// 条目与目录展开的单文件任务共用。
    /// </summary>
    private async Task RelayRemoteToRemoteAsync(FileView opp, string srcPath, string dstPath,
        bool isDirectory, bool forceOverwrite, Action<long>? progress, TransferBatchConnections connections,
        string[]? transferScopes = null)
    {
        var tempRoot = Path.Combine(Path.GetTempPath(), "iFreeSftp", "RemoteTransfer",
            RelayTempDirName(srcPath, dstPath));
        var keepTemp = false;
        try
        {
            Directory.CreateDirectory(tempRoot);
            var localPath = Path.Combine(tempRoot, RemoteLeafName(srcPath));
            if (isDirectory)
            {
                var src = await connections.AcquireAsync(_sftp);
                try
                {
                    // temp 任务私有: 已完整下载 (大小一致) 的文件跳过重下, 按文件粒度续传
                    await DownloadDirAsync(new PauseInterruptibleOps(src), srcPath, localPath, skipComplete: true);
                }
                finally
                {
                    connections.Release(_sftp, src);
                }

                var dst = await connections.AcquireAsync(opp.SftpService);
                try
                {
                    await UploadDirAsync(new PauseInterruptibleOps(dst), localPath, dstPath);
                }
                finally
                {
                    connections.Release(opp.SftpService, dst);
                }
            }
            else
            {
                // 阶段一 源→temp: temp 已达源大小视为上次中断的完整下载, 跳过;
                // 半成品走续传决策追加 (temp 任务私有, 大小判定安全, 故不消费覆盖决策)
                var src = await connections.AcquireAsync(_sftp);
                try
                {
                    var srcOps = new PauseInterruptibleOps(src);
                    var srcSize = await srcOps.TryGetSizeAsync(srcPath);
                    if (srcSize < 0 || LocalFileSize(localPath) != srcSize)
                    {
                        await TransferFileWithResumeAsync(srcOps, srcPath, localPath,
                            upload: false, forceOverwrite: false, progress, logScopes: transferScopes);
                    }
                }
                finally
                {
                    connections.Release(_sftp, src);
                }

                // 阶段二 temp→目标: 目标侧非任务私有 (可能本就有同名旧文件), 不按
                // 大小跳过; 用户选"覆盖"时禁用追加直接全量重传 (防较小旧文件被误判
                // 为半成品缝合成损坏内容), 未选覆盖时半成品走续传决策追加
                var dst = await connections.AcquireAsync(opp.SftpService);
                try
                {
                    await TransferFileWithResumeAsync(new PauseInterruptibleOps(dst), localPath, dstPath,
                        upload: true, forceOverwrite, progress, logScopes: transferScopes);
                }
                finally
                {
                    connections.Release(opp.SftpService, dst);
                }
            }
        }
        catch (PauseRequestedException)
        {
            keepTemp = true; // 暂停中断: 保留半成品供重跑续传, 上抛交 RunOneAsync 回排"等待"
            throw;
        }
        finally
        {
            if (!keepTemp)
            {
                TryDeleteDir(tempRoot);
            }
        }
    }

    /// <summary>中转临时目录名：源+目标路径哈希 (SHA-256 前 24 个 hex 字符)。稳定
    /// 命名使暂停中断后的重跑命中同一 temp 完成续传; 源或目标不同的任务互不共享。</summary>
    private static string RelayTempDirName(string srcPath, string dstPath)
        => Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes($"{srcPath}|{dstPath}")))[..24];

    /// <summary>本地文件大小 (不存在/不可读返回 -1)。</summary>
    private static long LocalFileSize(string path)
    {
        try
        {
            return new FileInfo(path).Length;
        }
        catch
        {
            return -1L;
        }
    }

    /// <summary>远程路径取末段名称 ('/' 分隔; 无分隔符原样返回)。</summary>
    private static string RemoteLeafName(string path)
    {
        var idx = path.LastIndexOf('/');
        return idx < 0 ? path : path[(idx + 1)..];
    }

    /// <summary>显示文本兜底: 空串显示 "—" (覆盖确认信息列)。</summary>
    private static string NonEmpty(string text) => text.Length > 0 ? text : "—";

    /// <summary>目标条目是否已存在 (远程经会话探测, 本地查文件系统)。</summary>
    private static async Task<bool> TargetEntryExistsAsync(FileView opp, string dest)
        => opp.Mode == FileViewMode.Remote
            ? await opp.SftpService.ExistsAsync(dest)
            : File.Exists(dest) || Directory.Exists(dest);

    /// <summary>目标条目的覆盖确认对比信息 (大小/修改时间; 目录大小不适用显示 —)。</summary>
    private static async Task<(string Size, string Time)> TargetInfoAsync(FileView opp, string dest)
    {
        if (opp.Mode == FileViewMode.Remote)
        {
            try
            {
                var (size, mtime) = await opp.SftpService.GetAttributesAsync(dest);
                return (FileEntry.FormatSize(size), mtime.LocalDateTime.ToString("yyyy-MM-dd HH:mm"));
            }
            catch
            {
                return ("—", "—"); // 属性读取失败 (权限等): 存在性已确认, 信息留占位
            }
        }

        if (Directory.Exists(dest))
        {
            return ("—", Directory.GetLastWriteTime(dest).ToString("yyyy-MM-dd HH:mm"));
        }

        var fi = new FileInfo(dest);
        return (FileEntry.FormatSize(fi.Length), fi.LastWriteTime.ToString("yyyy-MM-dd HH:mm"));
    }

    /// <summary>远程路径拼接 (逻辑在 Models.RemotePath 纯函数, 可单测)。</summary>
    private static string JoinRemotePath(string dir, string name)
        => Models.RemotePath.Join(dir, name);

    /// <summary>递归上传目录 (顶层同名已获覆盖许可; 递归内部同名直接合并/覆盖, 不再逐项询问)。</summary>
    private static async Task UploadDirAsync(IRemoteTransferOps sftp, string localDir, string remoteDir)
    {
        await sftp.CreateDirectoryAsync(remoteDir);
        foreach (var sub in Directory.EnumerateFileSystemEntries(localDir))
        {
            var remote = JoinRemotePath(remoteDir, Path.GetFileName(sub));
            if (Directory.Exists(sub))
            {
                await UploadDirAsync(sftp, sub, remote);
            }
            else
            {
                await sftp.UploadAsync(sub, remote);
            }
        }
    }

    /// <summary>递归下载目录 (目录合并, 同名文件覆盖; 递归内部不再逐项询问)。
    /// skipComplete 仅限中转 temp (任务私有): 已存在且大小一致的文件视为上次暂停
    /// 中断的完整下载成果, 跳过重下 (按文件粒度续传); 真实目标目录不传——目标可能
    /// 本就有同名旧文件, 大小一致不代表内容一致, 照常覆盖下载。</summary>
    private static async Task DownloadDirAsync(IRemoteTransferOps sftp, string remoteDir, string localDir,
        bool skipComplete = false)
    {
        Directory.CreateDirectory(localDir);
        foreach (var entry in await sftp.ListDirectoryAsync(remoteDir))
        {
            var local = Path.Combine(localDir, entry.Name);
            if (entry.IsDirectory)
            {
                await DownloadDirAsync(sftp, JoinRemotePath(remoteDir, entry.Name), local, skipComplete);
            }
            else
            {
                if (skipComplete && LocalFileSize(local) == entry.Length)
                {
                    continue;
                }

                await sftp.DownloadAsync(JoinRemotePath(remoteDir, entry.Name), local);
            }
        }
    }

    /// <summary>递归复制文件/目录 (目录同名合并, 文件同名覆盖)。</summary>
    private static void CopyEntryRecursive(string src, string dst)
    {
        if (Directory.Exists(src))
        {
            Directory.CreateDirectory(dst);
            foreach (var sub in Directory.EnumerateFileSystemEntries(src))
            {
                CopyEntryRecursive(sub, Path.Combine(dst, Path.GetFileName(sub)));
            }
        }
        else
        {
            File.Copy(src, dst, overwrite: true);
        }
    }

    private FileItemViewModel ToViewModel(FileEntry entry)
    {
        // 类型列文本由 FileItemViewModel.UpdateType 统一解析 (资源管理器同款,
        // 见 SystemIconService.GetTypeName), 本地与远程条目一致。
        // 本地模式条目按真实路径取图标 (盘符/exe 自带图标); 远程条目只按扩展名。
        return new FileItemViewModel(entry, Mode == FileViewMode.Local, _cols);
    }

    private void UpdateStatus(string? extra = null)
    {
        if (extra is not null)
        {
            StatusText.Text = extra;
            return;
        }
        string text;
        bool hasList;
        if (Mode == FileViewMode.Remote && !_sftp.IsConnected)
        {
            text = LocalizationService.GetString("StatusDisconnected");
            hasList = false; // 断开时列表内容已失效, 不做文件统计。
        }
        else
        {
            // 比对占位行 (空条目) 不计入条目数/统计。
            text = string.Format(LocalizationService.GetString("ItemsCount"),
                _items.Count(i => !i.IsPlaceholder));
            hasList = true;
        }
        if (hasList)
        {
            // 目录数 / 文件数 / 总字节数 (字节只计文件), 各段条件显示:
            // 目录与文件数 > 0 才显示, 总字节非 0 才显示。
            long totalBytes = 0;
            int fileCount = 0, dirCount = 0;
            foreach (var it in _items)
            {
                if (it.IsPlaceholder)
                {
                    continue;
                }
                if (it.IsDirectory)
                {
                    dirCount++;
                }
                else
                {
                    fileCount++;
                    totalBytes += it.Size;
                }
            }
            var parts = new List<string>();
            if (dirCount > 0)
            {
                parts.Add(string.Format(LocalizationService.GetString("StatusDirCount"), dirCount));
            }
            if (fileCount > 0)
            {
                parts.Add(string.Format(LocalizationService.GetString("StatusFileCount"), fileCount));
            }
            if (totalBytes > 0)
            {
                parts.Add(string.Format(LocalizationService.GetString("StatusTotalBytes"), totalBytes.ToString("N0")));
            }
            if (parts.Count > 0)
            {
                text += " · " + string.Join(" · ", parts);
            }
        }
        // 选中统计 (比对占位行不计入)
        int selected = FileList.SelectedItems.OfType<FileItemViewModel>().Count(i => !i.IsPlaceholder);
        if (selected > 0)
        {
            text += " · " + string.Format(LocalizationService.GetString("StatusSelected"), selected);
        }
        StatusText.Text = text;
    }

    private void OnCultureChanged()
    {
        // 语言切换后，刷新已加载条目的类型文本。
        foreach (var item in _items)
        {
            item.UpdateType();
        }
        ApplyResources();
    }

    private void OnPropertyChanged([CallerMemberName] string? name = null)
        => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        foreach (var session in _editSessions.Values)
        {
            session.Dispose();
        }

        _editSessions.Clear();
        _cols.Changed -= OnColumnStateChanged;
        LocalizationService.CultureChanged -= OnCultureChanged;
        ThemeService.ThemeChanged -= OnThemeChanged;
        _sftp.Dispose();
    }
}

/// <summary>树形目录节点内容模型 (本地/远程共用)。</summary>
public sealed class TreeDir
{
    public TreeDir(string name, string fullPath)
    {
        Name = name;
        FullPath = fullPath;
    }

    /// <summary>显示名。</summary>
    public string Name { get; }

    /// <summary>目录完整路径。</summary>
    public string FullPath { get; }

    /// <summary>UIA/调试显示名 (TreeView 节点的可访问性 Name 由 Content.ToString() 推导)。</summary>
    public override string ToString() => Name;
}

/// <summary>面包屑段数据 (Text=显示名, Path=点击导航目标, Last=是否最后一段)。</summary>
internal sealed record CrumbItem(string Text, string Path, bool Last);

/// <summary>
/// 本地删除进回收站 (Windows Shell): SHFileOperationW + FO_DELETE + FOF_ALLOWUNDO，
/// 与资源管理器删除行为一致 (可从回收站还原)。一次调用支持批量文件/目录混合；
/// 用 Shell 而非 File.Delete/Directory.Delete——后者是永久删除，无法恢复。
/// </summary>
internal static class RecycleBin
{
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct FileOpStructW
    {
        public IntPtr hwnd;
        public uint wFunc;                                     // FO_DELETE = 3
        [MarshalAs(UnmanagedType.LPWStr)] public string pFrom; // 双 NUL 结尾路径列表
        public IntPtr pTo;
        public ushort fFlags;                                  // FOF_* 组合
        public int fAnyOperationsAborted;                      // BOOL
        public IntPtr hNameMappings;
        public IntPtr lpszProgressTitle;                       // FOF_SILENT 下不用
    }

    private const uint FO_DELETE = 3;
    private const ushort FOF_SILENT = 0x0004;
    private const ushort FOF_NOCONFIRMATION = 0x0010;
    private const ushort FOF_ALLOWUNDO = 0x0040;
    private const ushort FOF_NOERRORUI = 0x0400;

    [DllImport("shell32.dll", CharSet = CharSet.Unicode)]
    private static extern int SHFileOperationW(ref FileOpStructW op);

    /// <summary>把一组本地路径移入回收站；返回 false = Shell 报错或被中止
    /// (错误细节 Shell 不外传，按通用失败文案提示)。</summary>
    public static bool Delete(IEnumerable<string> paths)
    {
        // pFrom 要求以双 NUL 结尾的路径列表: 各路径间以 \0 分隔, 末尾补一个 \0,
        // LPWStr marshal 的字符串终止符即第二个 \0
        var op = new FileOpStructW
        {
            wFunc = FO_DELETE,
            pFrom = string.Join('\0', paths) + "\0",
            fFlags = FOF_ALLOWUNDO       // 进回收站
                   | FOF_NOCONFIRMATION  // 确认弹窗应用侧已做, 不再弹系统框
                   | FOF_SILENT          // 无 Shell 进度对话框
                   | FOF_NOERRORUI,      // 错误走返回值由状态栏提示
        };
        return SHFileOperationW(ref op) == 0 && op.fAnyOperationsAborted == 0;
    }
}

/// <summary>文件比对行高亮状态 (规则见 FileCompareService)。</summary>
public enum FileCompareState
{
    /// <summary>无高亮 (两侧一致, 或目录参与配对但不染色)。</summary>
    None,
    /// <summary>仅一侧有此文件 (有文件的一行, 缺侧为占位空行)。</summary>
    Caution,
    /// <summary>两侧都有但文件时间或大小不同。</summary>
    Critical,
}

/// <summary>文件条目视图模型，增加系统图标 (取不到回退通用字形) 与列显隐绑定属性。</summary>
public sealed class FileItemViewModel : INotifyPropertyChanged
{
    private readonly FileEntry _entry;
    private readonly bool _isPlaceholder;
    public string IconGlyph => _entry.IsDirectory ? "\uE188" : "\uE7C3"; // Folder / PageSideBySide
    public string Name => _entry.Name;

    /// <summary>大小列文本: 比对占位行 (空行) 显示空, 不显示 0 B。</summary>
    public string DisplaySize => _isPlaceholder ? string.Empty : _entry.DisplaySize;
    public string DisplayModified => _entry.DisplayModified;
    public string DisplayPermissions => _entry.Permissions;
    public bool IsDirectory => _entry.IsDirectory;
    public string FullPath => _entry.FullPath;

    /// <summary>底层条目 (文件比对快照用; 占位行为空条目)。</summary>
    public FileEntry Entry => _entry;

    /// <summary>是否为比对占位行 (本侧缺此文件, 空行与对侧行对齐)。</summary>
    public bool IsPlaceholder => _isPlaceholder;

    /// <summary>排序用原始值。</summary>
    public long Size => _entry.Length;
    public DateTimeOffset Modified => _entry.LastModified;

    public FileItemViewModel(FileEntry entry, bool isLocalPath, FileColumnState cols, bool placeholder = false)
    {
        _entry = entry;
        _cols = cols;
        _isLocal = isLocalPath;
        _isPlaceholder = placeholder;
        if (placeholder)
        {
            return; // 占位行不取图标/类型 (空名字解析无意义)
        }

        UpdateType();
        // 系统真实图标异步就绪后切换显示; 取不到 (或尚未就绪) 时模板显示通用字形。
        var bitmap = SystemIconService.GetBitmap(_entry.IsDirectory, _entry.Name, _entry.FullPath, isLocalPath);
        if (bitmap is not null)
        {
            _ = SetIconAsync(bitmap);
        }
    }

    /// <summary>比对占位行工厂: 空条目, 模板所有单元格为空、不显示图标。</summary>
    public static FileItemViewModel CreatePlaceholder(FileColumnState cols)
        => new(new FileEntry(), isLocalPath: false, cols, placeholder: true);

    private readonly bool _isLocal; // 本地条目恒隐藏权限列 (Windows 无 rwx 权限语义)

    /// <summary>系统图标 XAML 源 (就绪后通知切换; null 时模板显示 IconGlyph 字形)。</summary>
    public ImageSource? Icon { get; private set; }

    public Visibility SystemIconVisible => Icon is null ? Visibility.Collapsed : Visibility.Visible;
    public Visibility GlyphVisible => Icon is null && !_isPlaceholder ? Visibility.Visible : Visibility.Collapsed;

    // —— 列显隐/列宽 (所属窗格的 FileColumnState 实例, 模板单元格折叠与定宽实现) ——

    private readonly FileColumnState _cols;

    public Visibility SizeColVisible => _cols.ShowSize
        ? Visibility.Visible : Visibility.Collapsed;
    public Visibility TypeColVisible => _cols.ShowType
        ? Visibility.Visible : Visibility.Collapsed;
    public Visibility ModifiedColVisible => _cols.ShowModified
        ? Visibility.Visible : Visibility.Collapsed;
    public Visibility PermissionsColVisible => !_isLocal && _cols.ShowPermissions
        ? Visibility.Visible : Visibility.Collapsed;
    public double NameColWidth => _cols.NameColWidth;
    public double SizeColWidth => _cols.SizeColWidth;
    public double TypeColWidth => _cols.TypeColWidth;
    public double ModifiedColWidth => _cols.ModifiedColWidth;
    public double PermissionsColWidth => _cols.PermissionsColWidth;

    /// <summary>列显隐/列宽变化后刷新绑定。</summary>
    public void RefreshColumnVisibility()
    {
        OnPropertyChanged(nameof(SizeColVisible));
        OnPropertyChanged(nameof(TypeColVisible));
        OnPropertyChanged(nameof(ModifiedColVisible));
        OnPropertyChanged(nameof(PermissionsColVisible));
        OnPropertyChanged(nameof(NameColWidth));
        OnPropertyChanged(nameof(SizeColWidth));
        OnPropertyChanged(nameof(TypeColWidth));
        OnPropertyChanged(nameof(ModifiedColWidth));
        OnPropertyChanged(nameof(PermissionsColWidth));
    }

    private async Task SetIconAsync(SoftwareBitmap bitmap)
    {
        var source = await SystemIconService.ToImageSourceAsync(bitmap);
        if (source is null || Icon is not null)
        {
            return;
        }
        Icon = source;
        OnPropertyChanged(nameof(Icon));
        OnPropertyChanged(nameof(SystemIconVisible));
        OnPropertyChanged(nameof(GlyphVisible));
    }

    private string _typeText = string.Empty;
    public string TypeText
    {
        get => _typeText;
        private set { _typeText = value; OnPropertyChanged(); }
    }

    // —— 文件比对高亮 (规则见 FileCompareService) ——

    private FileCompareState _compareState;

    /// <summary>比对高亮状态 (行背景的语义来源, 随重排更新)。</summary>
    public FileCompareState CompareState => _compareState;

    /// <summary>
    /// 高亮底色层显隐 (行模板内两个叠层 Border 的开关)。画刷由 XAML 的
    /// ThemeResource 解析——真正的主题感知 (亮/暗字典自动变体, 主题切换自动重取)。
    /// 不可经代码 Application.Current.Resources 取主题画刷: 那按应用/系统级主题
    /// 解析, 不跟随本应用"根元素 RequestedTheme 覆盖"式的切换, 亮主题会取到暗色。
    /// </summary>
    public Visibility CautionVisible
        => _compareState == FileCompareState.Caution ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>Critical 底色层显隐 (两侧都有但时间或大小不同)。</summary>
    public Visibility CriticalVisible
        => _compareState == FileCompareState.Critical ? Visibility.Visible : Visibility.Collapsed;

    /// <summary>设置比对高亮 (恒发通知: 同状态也要随主题变化重取画刷)。</summary>
    public void SetCompareState(FileCompareState state)
    {
        _compareState = state;
        OnPropertyChanged(nameof(CompareState));
        OnPropertyChanged(nameof(CautionVisible));
        OnPropertyChanged(nameof(CriticalVisible));
    }

    public void UpdateType()
    {
        // 类型文本统一在此解析: 资源管理器同款类型名 (SystemIconService.GetTypeName,
        // 与系统显示语言一致, 不随应用内语言切换)。任何条目构造路径与语言切换都经此刷新。
        TypeText = SystemIconService.GetTypeName(_entry.IsDirectory, _entry.Name, _entry.IsExecutable, _entry.IsSymbolicLink);
    }

    public event PropertyChangedEventHandler? PropertyChanged;
    private void OnPropertyChanged([CallerMemberName] string? name = null)
        => PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(name));
}
