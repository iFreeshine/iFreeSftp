using System.Runtime.InteropServices;
using System.Text.Json;
using iFreeSftp.Services;
using Microsoft.UI;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Windowing;
using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Windows.Graphics;
using Windows.UI;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace iFreeSftp.Views;

/// <summary>
/// 标题栏（Header）：应用头部区域，承载 logo / 应用名 / 系统标题按钮与
/// 主机/设置/主题/语言切换按钮，并通过 Frame 显示主容器。
/// 页面 UI 与逻辑放 MainContainer.xaml / MainContainer.xaml.cs，
/// 以便使用 Page 的导航事件与 Loaded 生命周期。
/// </summary>
public sealed partial class Header : Window
{
    /// <summary>同步浏览不匹配弹窗防重入标记。</summary>
    private bool _mismatchDialogOpen;

    public Header()
    {
        InitializeComponent();

        // 沉浸式标题栏: TitleBar 控件自动处理拖拽区域与系统 caption 按钮,
        // 无需再调用 SetTitleBar。
        ExtendsContentIntoTitleBar = true;
        AppWindow.TitleBar.PreferredHeightOption = TitleBarHeightOption.Tall;

        // 用 BaseDirectory 解析: 文件夹发布=exe 目录, 单文件发布=首次启动解压目录,
        // 两种模式下 Assets 均位于此, 相对路径在单文件模式下会解析失败导致任务栏无图标。
        AppWindow.SetIcon(Path.Combine(AppContext.BaseDirectory, "Assets", "AppIcon.ico"));

        // 首次启动用较大的默认窗口; 之后恢复上次关闭时的尺寸。
        // 落盘时机用 Closed, 但窗口指标 (AppWindow.Size/hwnd DPI) 在 Closed 时已
        // 不可读, 故期间经 AppWindow.Changed 持续缓存最近一次正常态的逻辑尺寸。
        // (本 SDK 无 Window.Closing / ApplicationData.GetDefault, 不能用常规方案。)
        ApplyWindowSize();
        AppWindow.Changed += OnAppWindowChanged;
        Closed += (_, _) => SaveWindowSize();

        // 主机配置: 启动即从 hosts.json 载入内存工作集 (面板打开时直接使用)。
        HostStoreService.Load();
        // 面板关闭请求 (面板内部处理完保存/丢弃后通知隐藏覆盖层)。
        HostPanelOverlay.CloseRequested += () => HostPanelOverlay.Visibility = Visibility.Collapsed;
        // 主机连接: 按面板指定的窗格 (Left/Right) 与 tab 意图发起连接,
        // 只有连接成功才会构建 tab 与远程容器 (失败仅命令窗日志)。
        HostPanelOverlay.ConnectRequested = (pane, newTab, host) =>
        {
            var main = RootFrame.Content as MainContainer;
            var paneView = pane == "Left" ? main?.LeftView : main?.RightView;
            return paneView is not null
                ? paneView.ConnectAndOpenAsync(host, newTab)
                : Task.FromResult((false, (string?)null));
        };
        // 目标窗格初始空态判定: 引导页独占时首个主机免询问直接打开。
        HostPanelOverlay.InitialEmptyState = pane =>
        {
            var main = RootFrame.Content as MainContainer;
            return (pane == "Left" ? main?.LeftView : main?.RightView)?.IsInitialEmptyState == true;
        };
        // 设置面板 (右上角设置按钮): 关闭请求同样只负责隐藏覆盖层。
        SettingsPanelOverlay.CloseRequested += () => SettingsPanelOverlay.Visibility = Visibility.Collapsed;

        // 打开内容选择器: 关闭仅隐藏; 本地 → 触发窗格打开本地浏览;
        // 主机 → 直连该主机 (强制新标签页, 空态窗格由连接决策转当前唯一 tab)。
        HostPickerOverlay.CloseRequested += () => HostPickerOverlay.Visibility = Visibility.Collapsed;
        HostPickerOverlay.LocalPicked += pane =>
            (RootFrame.Content as MainContainer)?.OpenLocalView(pane);
        HostPickerOverlay.HostPicked += (pane, host) =>
        {
            HostPanelOverlay.ConnectPane = pane;
            HostPanelOverlay.TabIntent = "new";
            _ = HostPanelOverlay.ConnectToHostAsync(host);
        };

        // 主题应用在 Window 内容根元素上, 才能同时影响 Mica 背景与所有控件。
        // 在构造函数初始化(按注册表系统主题), 首帧即正确无闪烁。
        ThemeService.Initialize(RootGrid);

        // 标题栏按钮文本随主题/语言变化。
        ThemeService.ThemeChanged += _ => ApplyTitleBarResources();
        LocalizationService.CultureChanged += ApplyTitleBarResources;
        ApplyTitleBarResources();

        // 同步浏览状态机: 按钮状态随两侧文件夹快照重算 (StateChanged),
        // 不匹配时终端日志 + 弹窗 (MismatchDetected)。
        // 文件比对按钮同场驱动: 同步点亮才可选 (规则①), 勾选态随比对激活态。
        SyncBrowseService.StateChanged += UpdateSyncButton;
        SyncBrowseService.MismatchDetected += OnSyncMismatch;
        FileCompareService.ActiveChanged += UpdateSyncButton;
        Closed += (_, _) =>
        {
            SyncBrowseService.StateChanged -= UpdateSyncButton;
            SyncBrowseService.MismatchDetected -= OnSyncMismatch;
            FileCompareService.ActiveChanged -= UpdateSyncButton;
        };

        // 右边距依赖 XamlRoot, 在面板 Loaded 后计算。
        TitleButtonsPanel.Loaded += OnTitleButtonsPanelLoaded;

        // Navigate the root frame to the main container on startup.
        RootFrame.Navigate(typeof(MainContainer));
        // 任一窗格的 + 按钮/选择主机按钮 → 打开"打开内容"选择器 (带来源窗格)。
        if (RootFrame.Content is MainContainer main)
        {
            main.OpenPickerRequested += pane => HostPickerOverlay.OpenFor(pane);
        }
    }

    // —— 窗口尺寸: 首启默认值 + 记忆上次关闭尺寸 ——
    // 保存/恢复均用"逻辑尺寸" (物理像素 ÷ DPI 缩放), 跨 DPI 会话仍按比例还原。
    // AppWindow.Size/Resize 均为窗口外框 (含边框), 两端自洽。
    // 持久化用 LocalAppData 下的 JSON 文件: Windows.Storage.ApplicationData 在
    // 本 SDK 非打包场景不可用 (无 GetDefault, Current 需包标识)。
    private const double DefaultWindowWidth = 1280;
    private const double DefaultWindowHeight = 800;
    private double _lastLogicalWidth = DefaultWindowWidth;
    private double _lastLogicalHeight = DefaultWindowHeight;
    private static readonly string WindowSettingsPath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "iFreeSftp", "window.json");

    [DllImport("user32.dll")]
    private static extern uint GetDpiForWindow(IntPtr hwnd);

    [DllImport("user32.dll")]
    private static extern bool IsZoomed(IntPtr hwnd);

    [DllImport("user32.dll")]
    private static extern bool IsIconic(IntPtr hwnd);

    [DllImport("user32.dll")]
    private static extern bool GetClientRect(IntPtr hwnd, out RECT rect);

    [StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
    private struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    /// <summary>按上次记忆 (或首次默认) 的逻辑尺寸调整窗口, 显示前完成无跳变。</summary>
    private void ApplyWindowSize()
    {
        var width = DefaultWindowWidth;
        var height = DefaultWindowHeight;

        try
        {
            if (File.Exists(WindowSettingsPath))
            {
                var json = JsonDocument.Parse(File.ReadAllText(WindowSettingsPath)).RootElement;
                if (json.TryGetProperty("width", out var w) && w.GetDouble() > 0)
                {
                    width = w.GetDouble();
                }

                if (json.TryGetProperty("height", out var h) && h.GetDouble() > 0)
                {
                    height = h.GetDouble();
                }
            }
        }
        catch
        {
            // 文件缺失或损坏时退回默认尺寸。
        }

        var hwnd = Win32Interop.GetWindowFromWindowId(AppWindow.Id);
        var scale = GetDpiForWindow(hwnd) / 96.0;
        _lastLogicalWidth = width;
        _lastLogicalHeight = height;
        AppWindow.Resize(new SizeInt32(
            (int)Math.Round(width * scale),
            (int)Math.Round(height * scale)));
    }

    /// <summary>
    /// 持续缓存最近一次正常态 (非最大化/最小化) 的窗口逻辑尺寸, 供关闭时落盘。
    /// 最大化/最小化引起的尺寸变化不覆盖缓存 —— 关闭时保留的是正常态尺寸。
    /// </summary>
    private void OnAppWindowChanged(AppWindow sender, AppWindowChangedEventArgs args)
    {
        if (!args.DidSizeChange)
        {
            return;
        }

        try
        {
            var hwnd = Win32Interop.GetWindowFromWindowId(sender.Id);
            if (IsZoomed(hwnd) || IsIconic(hwnd))
            {
                return;
            }

            var scale = GetDpiForWindow(hwnd) / 96.0;
            var size = sender.Size;
            if (scale > 0 && size.Width > 0 && size.Height > 0)
            {
                _lastLogicalWidth = size.Width / scale;
                _lastLogicalHeight = size.Height / scale;
            }

            // 窗口宽度变了, 标题条拖拽区域宽度随之变化, 同步重注册。
            UpdateDragRegions();
        }
        catch
        {
            // 单次事件读取失败忽略, 等下次尺寸变化再缓存。
        }
    }

    /// <summary>关闭时把缓存的逻辑尺寸写入本地设置 (不读已失效的窗口指标)。</summary>
    private void SaveWindowSize()
    {
        try
        {
            Directory.CreateDirectory(Path.GetDirectoryName(WindowSettingsPath)!);
            File.WriteAllText(WindowSettingsPath, JsonSerializer.Serialize(new
            {
                width = _lastLogicalWidth,
                height = _lastLogicalHeight,
            }));
        }
        catch
        {
            // 持久化失败不影响窗口关闭。
        }
    }

    /// <summary>刷新标题栏按钮的图标与提示文本。</summary>
    private void ApplyTitleBarResources()
    {
        UpdateCaptionButtonColors();

        // 主题图标: 暗色显示月亮(E708), 亮色显示太阳(E706)。
        // 注意 Segoe Fluent Icons: E706=太阳, E708=月亮 (与直觉相反, 已实测字形)。
        ThemeIcon.Glyph = ThemeService.IsDark ? "\uE708" : "\uE706";
        ToolTipService.SetToolTip(ThemeToggleBtn, CreateAccentToolTip(LocalizationService.GetString("ThemeToggleLabel")));

        // 主机文字按钮与设置图标按钮。
        HostBtnText.Text = LocalizationService.GetString("Host");
        ToolTipService.SetToolTip(SettingsBtn, CreateAccentToolTip(LocalizationService.GetString("SettingsLabel")));

        // 模式胶囊 (主机按钮左侧): 同步浏览/文件比对, 文字显示在 tooltip。
        // tooltip 挂透明宿主 (Grid) 而非按钮: 按钮禁用时不参与命中测试, 挂按钮上 disable 无 tooltip
        ToolTipService.SetToolTip(SyncBrowseToggleHost, CreateAccentToolTip(LocalizationService.GetString("HeaderSyncBrowse")));
        ToolTipService.SetToolTip(CompareToggleHost, CreateAccentToolTip(LocalizationService.GetString("HeaderCompare")));

        // 语言按钮 tooltip 显示即将切换到的目标语言。
        string langName = LocalizationService.CurrentCulture.StartsWith("zh", System.StringComparison.OrdinalIgnoreCase)
            ? LocalizationService.GetString("LangEnglish")
            : LocalizationService.GetString("LangChinese");
        ToolTipService.SetToolTip(LangToggleBtn, CreateAccentToolTip(langName));

        // 延迟再刷一次 caption 按钮颜色: TitleBar 控件可能在主题传播完成后
        // 才自行更新系统按钮, 需在其之后覆盖 (与 Mica 重挂同为低优先级时机)。
        RootGrid.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, UpdateCaptionButtonColors);
    }

    /// <summary>创建图标按钮的 tooltip: 蓝色背景、显示在图标下方 (样式定义见 App.xaml)。</summary>
    private static ToolTip CreateAccentToolTip(string text) => new()
    {
        Content = text,
        Style = (Style)Application.Current.Resources["AccentToolTipStyle"],
    };

    /// <summary>
    /// 右侧按钮组紧贴系统 caption 按钮, 且整体与 caption 按钮垂直居中。
    /// RightInset/Height 为物理像素 (客户区坐标), 除以光栅化缩放转为逻辑像素。
    /// TitleBar 控件自然高度比系统 caption 区高约 8 逻辑像素, Row 0 的垂直居中
    /// 内容会整体偏低; 将控件高度钳制为 caption 区高度后, Row 0 即 caption 条,
    /// 左侧品牌区 (logo/应用名) 与右侧按钮组 (均为 Row 0 居中) 的中心都精确
    /// 落在 caption 按钮中心线上。
    /// (品牌区不放 TitleBar.LeftHeader: 其模板内容区最小高度 48 逻辑px, 高 DPI
    /// 下钳制后内部仍按 48px 居中会偏低, 故 overlay 到 Row 0, 见 Header.xaml。)
    /// </summary>
    private void OnTitleButtonsPanelLoaded(object sender, RoutedEventArgs e)
    {
        UpdateTitleBarMetrics();
        UpdateDragRegions();

        // 跨显示器拖动等 DPI 变化会同时改变缩放与 OS caption 指标
        // (本机实测 caption 条为 41.14 逻辑px 而非 48, 随 DPI 上下文浮动),
        // 一次性计算会失准, 故监听缩放变化重算。
        TitleButtonsPanel.XamlRoot.Changed += OnXamlRootChanged;
    }

    private void OnXamlRootChanged(XamlRoot sender, XamlRootChangedEventArgs args)
    {
        // Changed 仅在主题/缩放/背景等低频变化时触发, 重算开销极小且幂等,
        // 无需区分变化来源 (该版本事件参数也没有区分 DPI 的属性)。

        // 立即重算一次; OS caption 指标可能随 DPI 传播稍后才稳定, 低优先级
        // 再补算一次 (与 UpdateCaptionButtonColors 的双刷模式相同)。
        UpdateTitleBarMetrics();
        UpdateDragRegions();
        RootGrid.DispatcherQueue.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            UpdateTitleBarMetrics();
            UpdateDragRegions();
        });
    }

    /// <summary>按当前 DPI 重算: 右侧按钮组避开 caption 区的边距 + TitleBar 高度钳制。</summary>
    private void UpdateTitleBarMetrics()
    {
        var scale = TitleButtonsPanel.XamlRoot.RasterizationScale;
        if (scale <= 0)
        {
            return;
        }

        // RightInset/Height 在窗口最小化或指标未就绪时会瞬时读出 0,
        // 直接采用会把错误值持久化 (实测按钮组被推出窗口右缘), 故仅在
        // 有效时更新, 无效则保留上一次的正确值, 等下次事件自愈。
        var rightInset = AppWindow.TitleBar.RightInset;
        if (rightInset > 0)
        {
            TitleButtonsPanel.Margin = new Thickness(0, 0, rightInset / scale + 2, 0);
        }

        var captionHeight = AppWindow.TitleBar.Height;
        if (captionHeight > 0)
        {
            var captionLogical = captionHeight / scale;
            AppTitleBar.Height = captionLogical;

            // 按钮上下各留 5 逻辑px: 高度 = caption 条高 − 10, 随系统条自适应。
            // 在保持与系统 caption 按钮同轴的前提下间距恒为 5px
            // (caption 条实际高度随机器/DPI 浮动, 不能用固定值)。
            var buttonHeight = captionLogical - 10;
            foreach (var child in TitleButtonsPanel.Children)
            {
                if (child is Button button)
                {
                    button.Height = buttonHeight;
                }
            }
        }
    }

    /// <summary>
    /// 显式注册标题条非客户区拖拽区域。
    /// 背景: 标题条拖拽区域本应由 TitleBar 控件自动注册, 但窗口内容树中存在
    /// 后声明的 HostPanel 覆盖层 (即使 Collapsed) 时, 该控件的区域注册会失效
    /// (实测整条标题条 NCHITTEST 全部返回 HTCLIENT, 窗口无法拖动)。故由本
    /// 方法接管: Caption 区域 = 标题条整条 (避开系统 caption 按钮 RightInset),
    /// Passthrough 区域 = 右侧自定义按钮组 (保证按钮可点击)。
    /// 坐标为窗口客户区物理像素, 与 InputNonClientPointerSource 的坐标基准一致。
    /// </summary>
    private void UpdateDragRegions()
    {
        try
        {
            var scale = TitleButtonsPanel.XamlRoot.RasterizationScale;
            var captionHeight = AppWindow.TitleBar.Height;
            var rightInset = AppWindow.TitleBar.RightInset;
            if (scale <= 0 || captionHeight <= 0)
            {
                return; // 指标未就绪/最小化瞬时 0, 等下次事件重算
            }

            var hwnd = Win32Interop.GetWindowFromWindowId(AppWindow.Id);
            if (!GetClientRect(hwnd, out var rc))
            {
                return;
            }

            int clientWidth = rc.Right - rc.Left;
            if (clientWidth <= 0)
            {
                return;
            }

            var stripHeight = (int)Math.Round((double)captionHeight);
            var captionRight = Math.Max(0, clientWidth - rightInset);

            var origin = TitleButtonsPanel.TransformToVisual(null).TransformPoint(new Windows.Foundation.Point(0, 0));
            var passthrough = new Windows.Graphics.RectInt32(
                (int)Math.Floor(origin.X * scale) - 2,
                (int)Math.Floor(origin.Y * scale) - 2,
                (int)Math.Ceiling(TitleButtonsPanel.ActualWidth * scale) + 4,
                (int)Math.Ceiling(TitleButtonsPanel.ActualHeight * scale) + 4);

            // Caption 与 Passthrough 不能重叠: SDK 命中 Caption 优先, 按钮会被拖拽吞掉。
            // 故把 Caption 拆为绕开按钮面板的左右两段。
            var passLeft = Math.Clamp(passthrough.X, 0, captionRight);
            var passRight = Math.Clamp(passthrough.X + passthrough.Width, 0, captionRight);
            var captions = new List<Windows.Graphics.RectInt32>(2);
            if (passLeft > 0)
            {
                captions.Add(new Windows.Graphics.RectInt32(0, 0, passLeft, stripHeight));
            }
            if (passRight < captionRight)
            {
                captions.Add(new Windows.Graphics.RectInt32(passRight, 0, captionRight - passRight, stripHeight));
            }

            var source = Microsoft.UI.Input.InputNonClientPointerSource.GetForWindowId(AppWindow.Id);
            source.SetRegionRects(Microsoft.UI.Input.NonClientRegionKind.Caption, captions.ToArray());
            source.SetRegionRects(Microsoft.UI.Input.NonClientRegionKind.Passthrough, new[] { passthrough });
        }
        catch
        {
            // 窗口句柄/指标失效的瞬间忽略, 等下次事件重算。
        }
    }

    /// <summary>
    /// 显式设置系统 caption 按钮 (最小化/最大化/关闭) 的颜色。
    /// ExtendsContentIntoTitleBar 下这些按钮由系统绘制, 字形颜色不随运行时
    /// RequestedTheme 切换 —— 暗色启动后切亮色会残留白色字形, 在亮背景上不可见。
    /// </summary>
    private void UpdateCaptionButtonColors()
    {
        var tb = AppWindow.TitleBar;
        bool dark = ThemeService.IsDark;
        Color fg = dark ? Colors.White : Colors.Black;

        tb.ButtonBackgroundColor = Colors.Transparent;
        tb.ButtonForegroundColor = fg;
        tb.ButtonInactiveBackgroundColor = Colors.Transparent;
        tb.ButtonInactiveForegroundColor = Color.FromArgb(0x80, fg.R, fg.G, fg.B);
        tb.ButtonHoverBackgroundColor = dark
            ? Color.FromArgb(0x1F, 0xFF, 0xFF, 0xFF)
            : Color.FromArgb(0x14, 0x00, 0x00, 0x00);
        tb.ButtonHoverForegroundColor = fg;
        tb.ButtonPressedBackgroundColor = dark
            ? Color.FromArgb(0x0F, 0xFF, 0xFF, 0xFF)
            : Color.FromArgb(0x0D, 0x00, 0x00, 0x00);
        tb.ButtonPressedForegroundColor = fg;
        // 关闭按钮 hover 的红色底色由系统保留, 不覆盖。
    }

    /// <summary>
    /// 主机管理入口：显示主机设置面板覆盖层。
    /// 面板在 XAML 中声明（随窗口启动时构造，勿再懒创建第二个实例——
    /// 点击时二次构造同一 XAML 类型会触发 themeresources 懒加载异常，
    /// 且 Open 会作用在不在可视树中的孤儿实例上）。
    /// </summary>
    private void HostBtn_Click(object sender, RoutedEventArgs e)
    {
        HostPanelOverlay.Open();
        HostPanelOverlay.Visibility = Visibility.Visible;
    }

    /// <summary>
    /// 同步按钮状态重算 (StateChanged): 两侧都有文件夹 tab 才可点击 (规则②),
    /// 点亮态 = 联动激活中。自动点亮只在连接配置了同步浏览的主机时发生,
    /// 此处只反映状态, 不反向激活——避免用户手动熄灭后被立即重新点亮。
    /// </summary>
    private void UpdateSyncButton()
    {
        if (SyncBrowseToggle is null)
        {
            return;
        }

        var (both, _, _) = SyncBrowseService.Evaluate();
        SyncBrowseToggle.IsEnabled = both;
        SyncBrowseToggle.IsChecked = SyncBrowseService.Active;

        // 规则①: 文件比对只在同步浏览点亮后允许选中; 同步熄灭联动熄灭比对
        // (Reset 内部恢复两侧常规列表, 已灭则幂等无副作用)。
        if (!SyncBrowseService.Active)
        {
            FileCompareService.Reset();
        }
        CompareToggle.IsEnabled = SyncBrowseService.Active;
        // 程序赋值 IsChecked 不触发 Click, 不会反向重入。
        CompareToggle.IsChecked = FileCompareService.Active;
    }

    /// <summary>
    /// 同步浏览不匹配 (规则③④): 终端日志 + 弹窗提示; 防重入避免连续导航叠弹窗。
    /// </summary>
    private async void OnSyncMismatch(string reasonKey)
    {
        var msg = LocalizationService.GetString(reasonKey);
        TerminalLogService.Log(msg);
        if (_mismatchDialogOpen || Content?.XamlRoot is not { } root)
        {
            return;
        }

        _mismatchDialogOpen = true;
        try
        {
            // 字号整体较系统 ContentDialog 小一号 (字阶 20→16, 14→12):
            // 标题/正文用显式 TextBlock 压字号, 按钮经 ButtonStyle 覆盖。
            var dialog = new ContentDialog
            {
                XamlRoot = root,
                Title = new TextBlock
                {
                    Text = LocalizationService.GetString("SyncMismatchTitle"),
                    FontSize = 16,
                    FontWeight = FontWeights.SemiBold,
                },
                Content = new TextBlock
                {
                    Text = msg,
                    FontSize = 12,
                    TextWrapping = TextWrapping.Wrap,
                },
                CloseButtonText = LocalizationService.GetString("SyncMismatchClose"),
                DefaultButton = ContentDialogButton.Close,
            };
            // 按钮字号: 覆盖 dialog.Resources 的隐式样式键 "DefaultButtonStyle"
            // (BasedOn 主题原样式保模板, FontSizeProperty 定义在 Control 上)。
            if (Application.Current.Resources["DefaultButtonStyle"] is Style baseBtnStyle)
            {
                dialog.Resources["DefaultButtonStyle"] = new Style(typeof(Button))
                {
                    BasedOn = baseBtnStyle,
                    Setters = { new Setter(Control.FontSizeProperty, 12d) },
                };
            }

            await dialog.ShowAsync();
        }
        finally
        {
            _mismatchDialogOpen = false;
        }
    }

    /// <summary>
    /// 模式胶囊: 同步浏览开关 (规则③)——勾选时两侧快照匹配才点亮; 匹配不上
    /// 回滚勾选并弹不匹配信息, 不予点亮; 取消勾选熄灭联动。
    /// </summary>
    private void SyncBrowseToggle_Click(object sender, RoutedEventArgs e)
    {
        if (SyncBrowseToggle.IsChecked == true)
        {
            var (_, matched, reasonKey) = SyncBrowseService.Evaluate();
            if (matched)
            {
                SyncBrowseService.SetActive(true);
            }
            else
            {
                // 程序赋值 IsChecked 不触发 Click, 回滚安全
                SyncBrowseToggle.IsChecked = false;
                SyncBrowseService.ReportMismatch(reasonKey);
            }
        }
        else
        {
            SyncBrowseService.SetActive(false);
        }
    }

    /// <summary>
    /// 模式胶囊: 文件比对开关 (规则③④⑤)——点亮即激活比对: 两侧活动视图按合并
    /// 名清单重排, 单侧独有文件行 Caution 高亮 (缺侧空置占位), 两侧时间或大小
    /// 不同行 Critical 高亮; 取消即恢复常规列表。同步浏览未点亮时按钮已禁用
    /// (规则①), 此处仅防御性回滚。
    /// </summary>
    private void CompareToggle_Click(object sender, RoutedEventArgs e)
    {
        if (CompareToggle.IsChecked == true)
        {
            if (!SyncBrowseService.Active)
            {
                // 程序赋值 IsChecked 不触发 Click, 回滚安全
                CompareToggle.IsChecked = false;
                return;
            }
            FileCompareService.SetActive(true);
        }
        else
        {
            FileCompareService.SetActive(false);
        }
    }

    /// <summary>右上角设置按钮：打开设置面板（样式与主机面板同款, 内容为应用设置表单）。</summary>
    private void SettingsBtn_Click(object sender, RoutedEventArgs e)
    {
        SettingsPanelOverlay.Open();
        SettingsPanelOverlay.Visibility = Visibility.Visible;
    }

    private void ThemeToggleBtn_Click(object sender, RoutedEventArgs e)
        => ThemeService.Toggle();

    /// <summary>切换中英文。</summary>
    private void LangToggleBtn_Click(object sender, RoutedEventArgs e)
    {
        string next = LocalizationService.CurrentCulture.StartsWith("zh", System.StringComparison.OrdinalIgnoreCase)
            ? "en-US"
            : "zh-CN";
        LocalizationService.SetCulture(next);
    }
}
