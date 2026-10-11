using System.Collections.ObjectModel;
using System.Collections.Specialized;
using System.Linq;
using iFreeSftp.Models;
using iFreeSftp.Services;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Windows.Storage.Pickers;

namespace iFreeSftp.Views;

/// <summary>
/// 主机设置面板：遮罩 + 居中卡片的窗口内覆盖层，由 Header 承载（RowSpan 覆盖全窗）。
/// 左列主机卡片列表（顶部居中新增按钮），右列 3 个设置 tab（常规/同步/传输）。
/// 显式保存语义：编辑只改内存工作集（HostStoreService.Hosts），点保存才落盘；
/// 未保存关闭时确认，选"不保存"则从盘重载丢弃修改。
/// </summary>
public sealed partial class HostPanel : UserControl
{
    /// <summary>请求关闭面板（Header 收到后隐藏覆盖层）。</summary>
    public event Action? CloseRequested;

    private readonly ObservableCollection<HostProfile> _list = new();
    private HostProfile? _selected;
    private bool _dirty;
    private bool _syncing;      // 程序性回填期间抑制 dirty 标记与事件响应
    private bool _initialized;  // InitializeComponent 期间事件早于字段就绪, 抑制响应
    private KeyboardAccelerator? _escAccelerator;
    private DispatcherQueueTimer? _saveFeedbackTimer;
    private DependencyObject? _focusBeforeSave; // 保存前焦点元素, 提醒浮层关闭后恢复以继续编辑

    public HostPanel()
    {
        InitializeComponent();
        _initialized = true;
        HostList.ItemsSource = _list;
        _list.CollectionChanged += OnListCollectionChanged;
        LocalizationService.CultureChanged += ApplyResources;
        Unloaded += (_, _) => LocalizationService.CultureChanged -= ApplyResources;
        // 右键菜单样式照搬主框架文件夹右键菜单 (README 3.9 第 7 条), 主题切换后重设底色
        if (Resources["CardMenuFlyout"] is MenuFlyout cardMenu)
        {
            cardMenu.MenuFlyoutPresenterStyle = PaneMenuPresenterStyle();
            ActualThemeChanged += (_, _) => cardMenu.MenuFlyoutPresenterStyle = PaneMenuPresenterStyle();
        }
        SettingsTabs.Loaded += (_, _) =>
            DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, HideTabSeparators);
        ApplyResources();
    }

    /// <summary>主机卡片右键菜单样式: 照搬主框架文件夹右键菜单 (FileView.PaneMenuPresenterStyle)——
    /// 仅 presenter 背景按主题显式指定 (Popup 层资源查找不经过本控件, 主题字典覆盖不可靠),
    /// 边框/条目 hover/行高走系统默认。亮 = #FFFFFF, 暗 = #26303D (与父容器同色系浅一档)。</summary>
    private Style PaneMenuPresenterStyle()
    {
        var bg = ActualTheme == ElementTheme.Light
            ? Windows.UI.Color.FromArgb(255, 0xFF, 0xFF, 0xFF)
            : Windows.UI.Color.FromArgb(255, 0x26, 0x30, 0x3D);
        var style = new Style(typeof(MenuFlyoutPresenter));
        style.Setters.Add(new Setter(MenuFlyoutPresenter.BackgroundProperty, new Microsoft.UI.Xaml.Media.SolidColorBrush(bg)));
        return style;
    }

    /// <summary>
    /// 隐藏 TabViewItem 头部的分隔竖线。WinUI 3 模板中分隔线是名为
    /// TabSeparator 的 Border (1x16), 其颜色走内部资源查找, 覆盖
    /// TabViewItemSeparator 主题刷无效, 故运行时按名称直接折叠。
    /// </summary>
    private void HideTabSeparators()
    {
        void Walk(Microsoft.UI.Xaml.DependencyObject node)
        {
            var count = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(node);
            for (var i = 0; i < count; i++)
            {
                var child = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(node, i);
                if (child is Border border && border.Name == "TabSeparator")
                {
                    border.Visibility = Visibility.Collapsed;
                }

                Walk(child);
            }
        }

        Walk(SettingsTabs);
    }

    /// <summary>打开面板：与存储工作集对齐列表、默认选中第一项、挂 ESC。</summary>
    public void Open()
    {
        ConnectPane = "Right";
        TabIntent = "auto";
        _selected = null;
        _list.Clear();
        foreach (var host in HostStoreService.Hosts)
        {
            _list.Add(host);
        }

        _dirty = false;
        TestAlert.IsOpen = false;   // 清掉上次打开时遗留的测试结果提示
        DragTransform.X = 0;        // 打开时卡片回居中位
        DragTransform.Y = 0;
        UpdateEmptyHint();
        HostList.SelectedIndex = _list.Count > 0 ? 0 : -1;
        SyncFromProfile();
        UpdateSelectionUI();
        AttachEscape();
        Visibility = Visibility.Visible;
        // 兜底: 面板显示后重应用一次选中配色 (容器可能早于面板可见时生成, 主题刷按可见态取更可靠)。
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, ApplyCardSelectionVisuals);
        // 兜底: 模板可能晚于首开应用, 每次打开后再折叠一次分隔线。
        HideTabSeparators();
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, HideTabSeparators);
    }

    // —— 标题栏拖拽卡片: 指针捕获 + RenderTransform 平移 (照搬 HostPickerPanel, 打开时重置回居中) ——

    private bool _dragging;
    private Windows.Foundation.Point _lastDragPos;

    private void TitleBar_PointerPressed(object sender, PointerRoutedEventArgs e)
    {
        var titleBar = (UIElement)sender;
        titleBar.CapturePointer(e.Pointer);
        _dragging = true;
        _lastDragPos = e.GetCurrentPoint(PanelRoot).Position;
    }

    private void TitleBar_PointerMoved(object sender, PointerRoutedEventArgs e)
    {
        if (!_dragging)
        {
            return;
        }

        var pos = e.GetCurrentPoint(PanelRoot).Position;
        DragTransform.X += pos.X - _lastDragPos.X;
        DragTransform.Y += pos.Y - _lastDragPos.Y;
        _lastDragPos = pos;
    }

    private void TitleBar_PointerReleased(object sender, PointerRoutedEventArgs e)
    {
        _dragging = false;
        ((UIElement)sender).ReleasePointerCapture(e.Pointer);
    }

    // —— 打开/关闭 ——

    /// <summary>
    /// 保存后继续当前编辑: 记录保存前的焦点元素 (OldFocusedElement), 并尝试取消
    /// 鼠标点击对保存按钮的焦点获取; 即使取消被忽略或焦点随后被提醒浮层抢占,
    /// 弹出后与自动关闭后都会恢复到保存前的元素, 编辑状态 (光标/选区) 保持原样。
    /// 键盘 Tab 导航与快捷键激活不受影响 (仅拦截 Mouse 来源)。
    /// </summary>
    private void SaveBtn_GettingFocus(object sender, GettingFocusEventArgs e)
    {
        _focusBeforeSave = e.OldFocusedElement;
        if (e.InputDevice == FocusInputDeviceKind.Mouse)
        {
            e.TryCancel();
        }
    }

    /// <summary>焦点已不在保存前元素上时程序化恢复 (TextBox 恢复焦点即恢复光标/选区)。</summary>
    private void RestoreEditingFocus()
    {
        if (_focusBeforeSave is not UIElement target)
        {
            return;
        }

        if (!ReferenceEquals(FocusManager.GetFocusedElement(), target))
        {
            _ = FocusManager.TryFocusAsync(target, FocusState.Programmatic);
        }
    }

    /// <summary>统一关闭入口：不弹确认，直接关闭且不保存（有修改则工作集回滚到磁盘状态）。</summary>
    private void RequestClose()
    {
        if (_dirty)
        {
            HostStoreService.Reload();
        }

        DetachEscape();
        Visibility = Visibility.Collapsed;
        CloseRequested?.Invoke();
    }

    private void AttachEscape()
    {
        if (_escAccelerator is not null)
        {
            return;
        }

        _escAccelerator = new KeyboardAccelerator { Key = Windows.System.VirtualKey.Escape };
        _escAccelerator.Invoked += (_, args) =>
        {
            args.Handled = true;
            RequestClose();
        };
        // 代码挂接: 面板隐藏时移除, 避免隐藏元素的快捷键行为不确定。
        PanelRoot.KeyboardAccelerators.Add(_escAccelerator);
    }

    private void DetachEscape()
    {
        if (_escAccelerator is null)
        {
            return;
        }

        PanelRoot.KeyboardAccelerators.Remove(_escAccelerator);
        _escAccelerator = null;
    }

    private void MaskBorder_Tapped(object sender, TappedRoutedEventArgs e) => RequestClose();

    private void CloseBtn_Click(object sender, RoutedEventArgs e) => RequestClose();

    private void SaveBtn_Click(object sender, RoutedEventArgs e) => SaveChanges();

    /// <summary>
    /// 测试连接：把表单写回当前项后，用 SSH.NET 按其参数做真实 SSH/SFTP 连接测试。
    /// </summary>
    private async void TestBtn_Click(object sender, RoutedEventArgs e)
    {
        if (_selected is null)
        {
            return;
        }

        SyncToProfile();
        var host = _selected;
        if (string.IsNullOrWhiteSpace(host.Host) || string.IsNullOrWhiteSpace(host.Username))
        {
            ShowTestAlert(false, LocalizationService.GetString("TestNeedFields"), string.Empty);
            return;
        }

        if (host.AuthMode == "KeyFile" && string.IsNullOrWhiteSpace(host.KeyFilePath))
        {
            ShowTestAlert(false, LocalizationService.GetString("TestNeedKeyFile"), string.Empty);
            return;
        }

        TestBtn.IsEnabled = false;
        TestBtn.Content = LocalizationService.GetString("Testing");
        bool ok;
        var detail = string.Empty;
        try
        {
            using var session = new SftpSessionService();
            ok = await session.ConnectAsync(host, retry: false); // 测试连接不重试, 快速反馈
        }
        catch (Exception ex)
        {
            ok = false;
            detail = ex.Message;
        }
        finally
        {
            TestBtn.IsEnabled = true;
            TestBtn.Content = LocalizationService.GetString("TestConnection");
        }

        ShowTestAlert(ok, LocalizationService.GetString(ok ? "TestOkTitle" : "TestFailTitle"), detail);
    }

    /// <summary>测试结果以浮层提示 (InfoBar) 显示在 footer 上方, 成功绿 / 失败红, 可手动关闭。</summary>
    private void ShowTestAlert(bool ok, string title, string detail)
    {
        _saveFeedbackTimer?.Stop(); // 保存提醒的自动关闭定时器不能误关测试结果提示
        TestAlert.Severity = ok ? InfoBarSeverity.Success : InfoBarSeverity.Error;
        TestAlert.Title = title;
        TestAlert.Message = string.IsNullOrEmpty(detail) ? null : detail;
        TestAlert.IsOpen = true;
    }

    /// <summary>
    /// 保存成功浮层提醒: 复用测试结果 InfoBar (绿色成功样式), 2.5s 自动消失。
    /// 提醒弹出后与自动消失后都把焦点恢复到保存前元素 (RestoreEditingFocus),
    /// 用户可无缝继续编辑; 未被抢占时恢复为空操作, 不做多余动作。
    /// </summary>
    private void ShowSaveAlert()
    {
        TestAlert.Severity = InfoBarSeverity.Success;
        TestAlert.Title = LocalizationService.GetString("SaveOkTitle");
        TestAlert.Message = null;
        TestAlert.IsOpen = true;

        // 低优先级投递: 等本次焦点迁移全部落定后再校验恢复
        DispatcherQueue.TryEnqueue(Microsoft.UI.Dispatching.DispatcherQueuePriority.Low, RestoreEditingFocus);

        _saveFeedbackTimer?.Stop();
        _saveFeedbackTimer = DispatcherQueue.CreateTimer();
        _saveFeedbackTimer.Interval = TimeSpan.FromSeconds(2.5);
        _saveFeedbackTimer.Tick += (_, _) =>
        {
            TestAlert.IsOpen = false;
            _saveFeedbackTimer?.Stop();
            // 仅当提醒消失后焦点落空才恢复; 期间用户已主动点击别处则不打扰
            if (FocusManager.GetFocusedElement() is null)
            {
                RestoreEditingFocus();
            }
        };
        _saveFeedbackTimer.Start();
    }

    /// <summary>把表单写回选中项后整体落盘；空名称补默认名并刷新卡片显示。</summary>
    private void SaveChanges()
    {
        SyncToProfile();
        foreach (var host in HostStoreService.Hosts)
        {
            if (string.IsNullOrWhiteSpace(host.Name))
            {
                host.Name = LocalizationService.GetString("NewHostName");
            }
        }

        // RefreshCards 的 Replace 会重挂卡片容器并可能丢掉 ListView 选中态,
        // 选中丢失会走 SelectionChanged→_selected=null→SyncFromProfile 清空表单
        // (变成"新增"态)。全程置 _syncing 抑制该链路, 刷新后恢复原选中,
        // 表单内容/选中卡片/编辑焦点保持原样。
        _syncing = true;
        try
        {
            var selectedIndex = HostList.SelectedIndex;
            RefreshCards();
            HostList.SelectedIndex = selectedIndex;
        }
        finally
        {
            _syncing = false;
        }

        HostStoreService.Save();
        _dirty = false;
        ShowSaveAlert();
    }

    // —— 卡片列表 ——

    private void AddBtn_Click(object sender, RoutedEventArgs e)
    {
        var host = new HostProfile { Name = LocalizationService.GetString("NewHostName") };
        HostStoreService.Hosts.Add(host);
        _list.Add(host);
        _dirty = true;
        UpdateEmptyHint();
        HostList.SelectedIndex = _list.Count - 1;
    }

    /// <summary>右键菜单文案：Tag 存 resw 键，Loading 时设本地化文本（模板实例无法按名查找）。</summary>
    private void MenuText_Loading(object sender, object args)
    {
        var item = (MenuFlyoutItem)sender;
        item.Text = LocalizationService.GetString((string)item.Tag);
    }

    /// <summary>
    /// 由宿主（Header）注入：在指定窗格（Left/Right）连接主机。newTab=true 连接成功后
    /// 新建远程 tab，否则活动标签页就地接管; 返回 (是否成功, 确认的指纹)。
    /// </summary>
    public Func<string, bool, HostProfile, Task<(bool Ok, string? Fingerprint)>>? ConnectRequested { get; set; }

    /// <summary>
    /// 由宿主（Header）注入：指定窗格是否处于初始空状态（仅一个空引导页）。
    /// 空状态下首个主机直接在唯一标签页打开, 跳过"当前/新标签页"询问。
    /// </summary>
    public Func<string, bool>? InitialEmptyState { get; set; }

    /// <summary>本次连接的目标窗格（Left/Right）。Open 时重置, 由打开入口覆盖。</summary>
    public string ConnectPane { get; set; } = "Right";

    /// <summary>本次连接的 tab 意图: auto=按设置与空态决策; new=强制新标签页 (tab 栏 + 菜单入口)。</summary>
    public string TabIntent { get; set; } = "auto";

    /// <summary>
    /// 连接指定主机：校验 → 打开方式（目标窗格空态=当前 tab, 加号入口=新 tab,
    /// 或按设置弹窗询问"当前/新标签页"并可勾选"总是"落盘）→ 定位目标标签页 →
    /// 关闭面板回主页 → FileView 发起连接并加载远程列表（终端窗输出日志、
    /// loading 转圈）→ 首次确认的指纹写回主机存档。
    /// 面板内卡片点击、右键"连接"与打开选择器都走这里
    /// (面板未打开时 RequestClose 为无害空操作)。
    /// </summary>
    public async Task ConnectToHostAsync(HostProfile host)
    {
        if (string.IsNullOrWhiteSpace(host.Host) || string.IsNullOrWhiteSpace(host.Username))
        {
            ShowTestAlert(false, LocalizationService.GetString("TestNeedFields"), string.Empty);
            return;
        }

        if (host.AuthMode == "KeyFile" && string.IsNullOrWhiteSpace(host.KeyFilePath))
        {
            ShowTestAlert(false, LocalizationService.GetString("TestNeedKeyFile"), string.Empty);
            return;
        }

        // 打开方式: 目标窗格初始空态时首个主机直接在唯一标签页打开, 不询问;
        // tab 栏加号入口强制新标签页; 其余按设置固定值, "ask" 则弹窗询问
        // (勾选"总是"后落盘不再询问)。
        var mode = AppSettingsService.ConnectOpenMode;
        if (InitialEmptyState?.Invoke(ConnectPane) == true)
        {
            mode = "current";
        }
        else if (TabIntent == "new")
        {
            mode = "new";
        }
        else if (mode is not ("current" or "new"))
        {
            var (choice, always) = await HostOpenDialog.ShowAsync(this, host.Host);
            if (choice is null)
            {
                return;
            }

            mode = choice;
            if (always)
            {
                AppSettingsService.SetConnectOpenMode(mode);
            }
        }

        // 关闭面板回主页; 连接过程与结果在右下角主机命令窗输出。
        // 只有连接成功才会构建 tab 与远程容器 (FolderContainer.ConnectAndOpenAsync),
        // 失败不建 tab, 保持窗格原状。
        RequestClose();

        var (ok, fingerprint) = await (ConnectRequested?.Invoke(ConnectPane, mode == "new", host)
            ?? Task.FromResult((false, (string?)null)));
        if (!ok)
        {
            return;
        }

        // 首次确认通过: 记住指纹并写入主机存档, 下次连接自动校验。
        // 面板关闭可能触发工作集回滚(Reload), 按引用失效时按 主机:端口+登录名 匹配当前工作集。
        if (fingerprint is not null && fingerprint != host.HostKeyFingerprint)
        {
            var target = HostStoreService.Hosts.FirstOrDefault(h => ReferenceEquals(h, host))
                ?? HostStoreService.Hosts.FirstOrDefault(h =>
                    h.Host == host.Host && h.Port == host.Port && h.Username == host.Username);
            if (target is not null)
            {
                target.HostKeyFingerprint = fingerprint;
                HostStoreService.Save();
                TerminalLogService.Log(LocalizationService.GetString("LogFingerprintSaved"));
            }
        }
    }

    /// <summary>右键菜单"连接"：连接该卡片主机。</summary>
    private void MenuConnect_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is HostProfile host)
        {
            _ = ConnectToHostAsync(host);
        }
    }

    /// <summary>
    /// 单击卡片：选中并载入编辑表单 (2026-09-30 用户要求点击进入编辑, 不再发起连接;
    /// 连接走右键菜单/主机选择弹窗)。
    /// 点击编辑钮时也冒泡到卡片，按来源是否为按钮排除，由编辑钮自身逻辑处理。
    /// </summary>
    private void CardRoot_Tapped(object sender, TappedRoutedEventArgs e)
    {
        for (var node = e.OriginalSource as Microsoft.UI.Xaml.DependencyObject; node is not null;
             node = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetParent(node))
        {
            if (node is Microsoft.UI.Xaml.Controls.Primitives.ButtonBase)
            {
                return;
            }
        }

        if ((sender as FrameworkElement)?.DataContext is not HostProfile host)
        {
            return;
        }

        HostList.SelectedItem = host;
    }

    /// <summary>编辑：选中该卡片，右侧表单随之载入（与 hover 编辑钮一致）。</summary>
    private void MenuEdit_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is HostProfile host)
        {
            HostList.SelectedItem = host;
        }
    }

    /// <summary>复制：克隆该主机插入其后、立即写入主机存档并选中新副本。</summary>
    private void MenuDuplicate_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not HostProfile host)
        {
            return;
        }

        var index = _list.IndexOf(host);
        if (index < 0)
        {
            return;
        }

        var copy = host.Clone();
        copy.Name = $"{host.Name} {LocalizationService.GetString("DuplicateSuffix")}";
        HostStoreService.Hosts.Insert(index + 1, copy);
        _list.Insert(index + 1, copy);
        HostStoreService.Save(); // 复制即落盘, 副本不会因未保存关闭而丢失
        _dirty = true;
        HostList.SelectedItem = copy;
    }

    /// <summary>删除：断开选中并从列表与工作集移除（不落盘, 保存才生效）。</summary>
    private void MenuDelete_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is not HostProfile host)
        {
            return;
        }

        var index = _list.IndexOf(host);
        if (index < 0)
        {
            return;
        }

        // 先断开选中, 避免列表变化触发的 SelectionChanged 把表单写回被删对象。
        _selected = null;
        _syncing = true;
        _list.RemoveAt(index);
        _syncing = false;
        HostStoreService.Hosts.Remove(host);
        _dirty = true;
        UpdateEmptyHint();

        if (_list.Count > 0)
        {
            HostList.SelectedIndex = Math.Min(index, _list.Count - 1);
        }
        else
        {
            SyncFromProfile();
            UpdateSelectionUI();
        }
    }

    private void EditBtn_Loading(object sender, object args)
        => ToolTipService.SetToolTip((FrameworkElement)sender, MakeAccentTip(LocalizationService.GetString("EditHost")));

    /// <summary>编辑按钮：选中该卡片，右侧表单随之载入。</summary>
    private void EditBtn_Click(object sender, RoutedEventArgs e)
    {
        if ((sender as FrameworkElement)?.DataContext is HostProfile host)
        {
            HostList.SelectedItem = host;
        }
    }

    private void CardRoot_PointerEntered(object sender, Microsoft.UI.Xaml.Input.PointerRoutedEventArgs e)
        => SetCardActionsVisible(sender, true);

    private void CardRoot_PointerExited(object sender, Microsoft.UI.Xaml.Input.PointerRoutedEventArgs e)
        => SetCardActionsVisible(sender, false);

    /// <summary>
    /// hover 显隐卡片操作钮。DataTemplate 实例无法按名查找, 按名遍历可视树。
    /// </summary>
    private static void SetCardActionsVisible(object sender, bool visible)
    {
        if (FindNamedDescendant((Microsoft.UI.Xaml.DependencyObject)sender, "CardActions") is { } actions)
        {
            actions.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
        }
    }

    /// <summary>按名在可视树向下查找元素（DataTemplate 实例无法 FindName, 统一走遍历）。</summary>
    private static Microsoft.UI.Xaml.FrameworkElement? FindNamedDescendant(Microsoft.UI.Xaml.DependencyObject root, string name)
    {
        var count = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(root);
        for (var i = 0; i < count; i++)
        {
            var child = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(root, i);
            if (child is Microsoft.UI.Xaml.FrameworkElement element && element.Name == name)
            {
                return element;
            }

            if (FindNamedDescendant(child, name) is { } found)
            {
                return found;
            }
        }

        return null;
    }

    // —— 卡片选中(active)配色 ——
    // 卡片底色与边框不透明, 盖住了 ListViewItem 的默认选中视觉, 因此自行应用 active 样式:
    // 选中 = active 背景 + 强调色边框, 未选中 = 默认卡片配色 + 无边框
    // (2026-09-30 用户要求未选中卡片不显示边框线); 配色定义在 ThemeDictionaries。

    /// <summary>容器生成/回收时按选择态应用卡片配色（覆盖初始填充、滚动重用、拖拽重排）。</summary>
    private void HostList_ContainerContentChanging(object sender, ContainerContentChangingEventArgs args)
        => ApplyCardSelectionVisuals(args.ItemContainer);

    /// <summary>把选择态配色应用到全部已实现的卡片容器。</summary>
    private void ApplyCardSelectionVisuals()
    {
        WalkContainers(HostList);

        void WalkContainers(Microsoft.UI.Xaml.DependencyObject node)
        {
            var count = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(node);
            for (var i = 0; i < count; i++)
            {
                var child = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(node, i);
                if (child is ListViewItem container)
                {
                    ApplyCardSelectionVisuals(container);
                }
                else
                {
                    WalkContainers(child);
                }
            }
        }
    }

    /// <summary>按容器选择态切换卡片配色；主题刷从面板 ThemeDictionaries 按当前主题取。</summary>
    private void ApplyCardSelectionVisuals(Microsoft.UI.Xaml.Controls.Primitives.SelectorItem container)
    {
        if (FindNamedDescendant(container, "CardRoot") is not Border root)
        {
            return;
        }

        var themeKey = ActualTheme == ElementTheme.Light ? "Light" : "Dark";
        if (Resources.ThemeDictionaries.TryGetValue(themeKey, out var themeRes) && themeRes is ResourceDictionary colors)
        {
            var bgKey = container.IsSelected ? "HostCardActiveBackgroundBrush" : "HostCardBackgroundBrush";
            var bdKey = container.IsSelected ? "HostCardActiveBorderBrush" : "HostCardBorderBrush";
            if (colors.TryGetValue(bgKey, out var bg) && bg is Microsoft.UI.Xaml.Media.Brush bgBrush)
            {
                root.Background = bgBrush;
            }

            if (colors.TryGetValue(bdKey, out var bd) && bd is Microsoft.UI.Xaml.Media.Brush bdBrush)
            {
                root.BorderBrush = bdBrush;
            }
        }

        // 边框仅选中卡片显示 (未选中 1px 边框线去除, 2026-09-30)
        root.BorderThickness = container.IsSelected
            ? new Microsoft.UI.Xaml.Thickness(1)
            : new Microsoft.UI.Xaml.Thickness(0);
    }

    /// <summary>拖拽排序：列表 Move 时把工作集顺序同步为列表顺序并标记脏（增删不用 Move, 不受影响）。</summary>
    private void OnListCollectionChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        if (e.Action != NotifyCollectionChangedAction.Move)
        {
            return;
        }

        var ordered = _list.ToList();
        HostStoreService.Hosts.Clear();
        HostStoreService.Hosts.AddRange(ordered);
        _dirty = true;
    }

    private void HostList_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        ApplyCardSelectionVisuals(); // 选择态配色, 与 _syncing 无关
        if (_syncing)
        {
            return;
        }

        SyncToProfile(); // 写回旧选中项
        _selected = HostList.SelectedItem as HostProfile;
        SyncFromProfile();
        UpdateSelectionUI();
    }

    /// <summary>HostProfile 无 INPC：重设同一引用触发 Replace，让卡片按最新数据重绘。</summary>
    private void RefreshCards()
    {
        for (int i = 0; i < _list.Count; i++)
        {
            _list[i] = _list[i];
        }
    }

    private void UpdateEmptyHint()
        => EmptyHint.Visibility = _list.Count == 0 ? Visibility.Visible : Visibility.Collapsed;

    private void UpdateSelectionUI()
        => SettingsTabs.IsEnabled = _selected is not null;

    // —— 表单 ↔ 模型 ——

    private void SyncToProfile()
    {
        if (_selected is null)
        {
            return;
        }

        _selected.Name = NameBox.Text.Trim();
        var newHost = HostBox.Text.Trim();
        // 主机地址变更 → 旧服务器指纹随之失效 (指纹与服务器绑定, 与登录 key 无关),
        // 清空待首次连接重新确认; 不清会因指纹不匹配直接拒绝且无恢复入口
        if (!string.Equals(newHost, _selected.Host, StringComparison.OrdinalIgnoreCase))
        {
            _selected.HostKeyFingerprint = string.Empty;
        }
        _selected.Host = newHost;
        _selected.Port = int.TryParse(PortBox.Text.Trim(), out var port) ? Math.Clamp(port, 1, 65535) : 22;
        _selected.AuthMode = AuthModeBox.SelectedIndex == 1 ? "KeyFile" : "Password";
        _selected.KeyFilePath = KeyPathBox.Text.Trim();
        _selected.Username = UsernameBox.Text.Trim();
        _selected.Password = LoginPwdBox.Password;
        _selected.LocalSyncPath = LocalSyncBox.Text.Trim();
        _selected.RemoteSyncPath = RemoteSyncBox.Text.Trim();
        _selected.SyncEnabled = SyncToggle.IsOn;
        _selected.CompareEnabled = CompareToggle.IsOn;
        _selected.KeepAliveEnabled = KeepAliveToggle.IsOn;
        _selected.KeepAliveIntervalSeconds = (int)Math.Clamp(KeepAliveSecondsBox.Value, 1, 3600);
        _selected.TimeoutSeconds = (int)Math.Clamp(TimeoutSlider.Value, 15, 30);
        _selected.MaxRetries = (int)Math.Clamp(MaxRetriesSlider.Value, 1, 3);
        _selected.RetryDelaySeconds = (int)Math.Clamp(RetryDelaySlider.Value, 3, 10);
    }

    private void SyncFromProfile()
    {
        _syncing = true;
        try
        {
            if (_selected is null)
            {
                NameBox.Text = string.Empty;
                HostBox.Text = string.Empty;
                PortBox.Text = "22";
                AuthModeBox.SelectedIndex = 0;
                LoginPwdBox.Password = string.Empty;
                KeyPathBox.Text = string.Empty;
                UsernameBox.Text = string.Empty;
                LocalSyncBox.Text = string.Empty;
                RemoteSyncBox.Text = string.Empty;
                SyncToggle.IsOn = false;
                CompareToggle.IsOn = false;
                KeepAliveToggle.IsOn = false;
                KeepAliveSecondsBox.Value = 15;
                TimeoutSlider.Value = 20;
                MaxRetriesSlider.Value = 2;
                RetryDelaySlider.Value = 5;
            }
            else
            {
                NameBox.Text = _selected.Name;
                HostBox.Text = _selected.Host;
                PortBox.Text = _selected.Port.ToString();
                AuthModeBox.SelectedIndex = _selected.AuthMode == "KeyFile" ? 1 : 0;
                LoginPwdBox.Password = _selected.Password;
                KeyPathBox.Text = _selected.KeyFilePath;
                UsernameBox.Text = _selected.Username;
                LocalSyncBox.Text = _selected.LocalSyncPath;
                RemoteSyncBox.Text = _selected.RemoteSyncPath;
                SyncToggle.IsOn = _selected.SyncEnabled;
                CompareToggle.IsOn = _selected.CompareEnabled;
                KeepAliveToggle.IsOn = _selected.KeepAliveEnabled;
                KeepAliveSecondsBox.Value = _selected.KeepAliveIntervalSeconds;
                // 旧档案越界值 (如 6000) 由 Slider coerce 到边界, 保存时按范围落盘
                TimeoutSlider.Value = _selected.TimeoutSeconds;
                MaxRetriesSlider.Value = _selected.MaxRetries;
                RetryDelaySlider.Value = _selected.RetryDelaySeconds;
            }
            UpdateAuthRows();
            UpdateSyncRowsEnabled();
            UpdateKeepAliveRowsEnabled();
            UpdateConnectSliderTexts();
        }
        finally
        {
            _syncing = false;
        }
    }

    /// <summary>登录方式切换：密钥方式隐藏密码行；密钥文件行常显，不随方式切换。</summary>
    private void UpdateAuthRows()
    {
        bool useKey = AuthModeBox.SelectedIndex == 1;
        PasswordRowLabel.Visibility = useKey ? Visibility.Collapsed : Visibility.Visible;
        LoginPwdBox.Visibility = useKey ? Visibility.Collapsed : Visibility.Visible;
    }

    /// <summary>开启同步切换：联动两个路径输入框的可用性，并按需标记脏。</summary>
    private void SyncToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (_initialized && !_syncing && _selected is not null && SyncToggle.IsOn != _selected.SyncEnabled)
        {
            _dirty = true;
        }

        UpdateSyncRowsEnabled();
    }

    /// <summary>同步关闭时禁用本地/远程路径输入框，开启时解锁。</summary>
    private void UpdateSyncRowsEnabled()
    {
        LocalSyncBox.IsEnabled = RemoteSyncBox.IsEnabled = SyncToggle.IsOn;
        // 文件比对依赖同步浏览 (基准根定位), 同步关闭时比对开关一并禁用。
        CompareToggle.IsEnabled = SyncToggle.IsOn;
    }

    /// <summary>keep-alive 开关切换：联动间隔输入框可用性，并按需标记脏。</summary>
    private void KeepAliveToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (_initialized && !_syncing && _selected is not null && KeepAliveToggle.IsOn != _selected.KeepAliveEnabled)
        {
            _dirty = true;
        }

        UpdateKeepAliveRowsEnabled();
    }

    /// <summary>keep-alive 关闭时禁用间隔秒数输入框，开启时激活。</summary>
    private void UpdateKeepAliveRowsEnabled()
        => KeepAliveSecondsBox.IsEnabled = KeepAliveToggle.IsOn;

    // —— 编辑事件: 标记脏 ——
    // 注意: WinUI 的 TextChanged 等事件可能延迟到回填(_syncing 复位)之后才触发,
    // 单靠 _syncing 防护会误标脏。因此一律与当前选中项的模型值比对, 值未变不算脏。

    private void FieldTextChanged(object sender, TextChangedEventArgs args)
    {
        if (!_initialized || _syncing || _selected is null)
        {
            return;
        }

        var box = (TextBox)sender;
        var current = box.Name switch
        {
            nameof(NameBox) => _selected.Name.Trim(),
            nameof(HostBox) => _selected.Host.Trim(),
            nameof(PortBox) => _selected.Port.ToString(),
            nameof(KeyPathBox) => _selected.KeyFilePath.Trim(),
            nameof(LocalSyncBox) => _selected.LocalSyncPath.Trim(),
            nameof(RemoteSyncBox) => _selected.RemoteSyncPath.Trim(),
            nameof(UsernameBox) => _selected.Username.Trim(),
            _ => null,
        };
        if (current is not null && box.Text.Trim() == current)
        {
            return; // 回填产生的延迟同值事件
        }

        _dirty = true;
    }

    private void FieldPasswordChanged(object sender, RoutedEventArgs e)
    {
        if (!_initialized || _syncing || _selected is null)
        {
            return;
        }

        if (((PasswordBox)sender).Password == _selected.Password)
        {
            return;
        }

        _dirty = true;
    }

    /// <summary>端口输入校验：只允许数字（允许清空），其余字符直接拒绝进入。</summary>
    private void PortBox_BeforeTextChanging(TextBox sender, TextBoxBeforeTextChangingEventArgs args)
    {
        if (args.NewText.Length > 0 && !uint.TryParse(args.NewText, out _))
        {
            args.Cancel = true;
        }
    }

    private void FieldValueChanged(object sender, NumberBoxValueChangedEventArgs args)
    {
        if (!_initialized || _syncing || _selected is null)
        {
            return;
        }

        var box = (NumberBox)sender;
        var current = box.Name switch
        {
            nameof(KeepAliveSecondsBox) => _selected.KeepAliveIntervalSeconds,
            _ => int.MinValue,
        };
        if ((int)box.Value == current)
        {
            return; // 回填产生的延迟同值事件
        }

        _dirty = true;
    }

    /// <summary>连接 slider（超时/重试/延迟）拖动标脏; 值由保存时统一读取。</summary>
    private void ConnectSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (!_initialized || _syncing || _selected is null)
        {
            return;
        }

        _dirty = true;
        UpdateConnectSliderTexts();
    }

    /// <summary>按当前语言刷新三个连接 slider 旁的当前值文本（秒/次）。</summary>
    private void UpdateConnectSliderTexts()
    {
        var seconds = LocalizationService.GetString("ValueFormatSeconds");
        var times = LocalizationService.GetString("ValueFormatTimes");
        TimeoutValueText.Text = string.Format(seconds, (int)TimeoutSlider.Value);
        MaxRetriesValueText.Text = string.Format(times, (int)MaxRetriesSlider.Value);
        RetryDelayValueText.Text = string.Format(seconds, (int)RetryDelaySlider.Value);
    }

    private void FieldToggled(object sender, RoutedEventArgs e)
    {
        if (!_initialized || _syncing || _selected is null)
        {
            return;
        }

        var toggle = (ToggleSwitch)sender;
        var current = toggle.Name switch
        {
            nameof(CompareToggle) => _selected.CompareEnabled,
            _ => false,
        };
        if (toggle.IsOn == current)
        {
            return; // 回填产生的延迟同值事件
        }

        _dirty = true;
    }

    private void AuthModeBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }
        UpdateAuthRows();
        _dirty = true;
    }

    /// <summary>密钥文件浏览：桌面应用需用主窗口句柄初始化选择器（非打包场景）。</summary>
    private void BrowseBtn_Click(object sender, RoutedEventArgs e)
    {
        if (App.MainAppWindow is null)
        {
            return;
        }

        var picker = new FileOpenPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker,
            WinRT.Interop.WindowNative.GetWindowHandle(App.MainAppWindow));
        picker.FileTypeFilter.Add(".ppk");
        picker.FileTypeFilter.Add(".pem");
        picker.FileTypeFilter.Add(".key");
        _ = PickKeyFileAsync(picker);
    }

    private async Task PickKeyFileAsync(FileOpenPicker picker)
    {
        var file = await picker.PickSingleFileAsync();
        if (file is not null && KeyPathBox.Text != file.Path)
        {
            KeyPathBox.Text = file.Path;
            _dirty = true;
        }
    }

    // —— 本地化 ——

    /// <summary>重应用全部文案（语言切换时由 LocalizationService 回调）。</summary>
    private void ApplyResources()
    {
        TitleText.Text = LocalizationService.GetString("HostPanelTitle");
        ToolTipService.SetToolTip(AddBtn, MakeAccentTip(LocalizationService.GetString("AddHost")));
        var closeText = LocalizationService.GetString("Close");
        Microsoft.UI.Xaml.Automation.AutomationProperties.SetName(TitleCloseBtn, closeText);
        ToolTipService.SetToolTip(TitleCloseBtn, MakeAccentTip(closeText));
        GeneralTab.Header = LocalizationService.GetString("TabGeneral");
        SyncTab.Header = LocalizationService.GetString("TabSync");
        TransferTab.Header = LocalizationService.GetString("TabTransfer");
        NameLabel.Text = LocalizationService.GetString("HostNameLabel");
        IpLabel.Text = LocalizationService.GetString("IpLabel");
        PortLabel.Text = LocalizationService.GetString("Port");
        LoginMethodLabel.Text = LocalizationService.GetString("LoginMethod");
        AuthPasswordItem.Content = LocalizationService.GetString("AuthModePassword");
        AuthKeyItem.Content = LocalizationService.GetString("AuthModeKeyFile");
        PasswordRowLabel.Text = LocalizationService.GetString("Password");
        KeyPathLabel.Text = LocalizationService.GetString("KeyPathLabel");
        BrowseBtn.Content = LocalizationService.GetString("Browse");
        UsernameLabel.Text = LocalizationService.GetString("Username");
        LocalSyncLabel.Text = LocalizationService.GetString("LocalSyncPath");
        RemoteSyncLabel.Text = LocalizationService.GetString("RemoteSyncPath");
        SyncLabel.Text = LocalizationService.GetString("EnableSync");
        CompareLabel.Text = LocalizationService.GetString("EnableCompare");
        KeepAliveLabel.Text = LocalizationService.GetString("EnableKeepAlive");
        IntervalLabel.Text = LocalizationService.GetString("KeepAliveInterval");
        TimeoutLabel.Text = LocalizationService.GetString("TimeoutSeconds");
        MaxRetriesLabel.Text = LocalizationService.GetString("MaxRetries");
        RetryDelayLabel.Text = LocalizationService.GetString("RetryDelaySeconds");
        TestBtn.Content = LocalizationService.GetString("TestConnection");
        SaveBtn.Content = LocalizationService.GetString("Save");
        CloseBtn.Content = LocalizationService.GetString("Close");
        EmptyHint.Text = LocalizationService.GetString("EmptyHostList");
    }

    private static ToolTip MakeAccentTip(string text) => new()
    {
        Content = text,
        Style = (Style)Application.Current.Resources["AccentToolTipStyle"],
    };
}
