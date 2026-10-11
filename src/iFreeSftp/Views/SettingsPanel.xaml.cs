using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// 设置面板：外壳与主机面板一致（遮罩 + 居中卡片 + 标题栏 + ESC/遮罩关闭）。
/// 主体 = 左侧大类导航（外观/连接）+ 右侧设置表单（小号字体）。
/// 修改即时生效并落盘（AppSettingsService），无保存按钮。
/// </summary>
public sealed partial class SettingsPanel : UserControl
{
    /// <summary>避免初始化期间控件联动（事件在 x:Name 字段就绪前触发）。</summary>
    private bool _initialized;

    /// <summary>同步选中项时抑制事件（如 Open 时按当前设置回填）。</summary>
    private bool _syncing;

    /// <summary>请求关闭（Header 负责隐藏覆盖层）。</summary>
    public event Action? CloseRequested;

    public SettingsPanel()
    {
        InitializeComponent();
        PopulateLanguages(); // 语言下拉: 内置两种 + locales 目录发现的外置语言
        LocalizationService.CultureChanged += ApplyResources;
        RegistrationService.StateChanged += OnRegistrationStateChanged;
        // 本元素主题真正翻转后才触发, 无 RequestedTheme 传播时序问题
        ActualThemeChanged += (_, _) => ApplyCategoryVisuals(_currentCategory);
        Unloaded += (_, _) =>
        {
            LocalizationService.CultureChanged -= ApplyResources;
            RegistrationService.StateChanged -= OnRegistrationStateChanged;
        };
        ApplyResources();
        ApplyCategoryVisuals("Appearance");
        _initialized = true;
    }

    /// <summary>打开面板：按当前设置回填表单并显示。</summary>
    public void Open()
    {
        SyncFromSettings();
        // 主题可能在面板关闭期间被切换: 重应用导航画刷 (代码后置持有的画刷实例
        // 是上次解析的主题字典值, 不会随主题自动重新解析)
        ApplyCategoryVisuals(_currentCategory);
        UpdateRegistrationUi(); // 打开时重读注册状态 (远程吊销/勋章开关可能已变)
        DragTransform.X = 0; // 打开时卡片回居中位
        DragTransform.Y = 0;
        AttachEscape();
        Visibility = Visibility.Visible;
    }

    /// <summary>关闭面板：隐藏覆盖层并通知 Header（无未保存概念，直接关）。</summary>
    private void RequestClose()
    {
        DetachEscape();
        Visibility = Visibility.Collapsed;
        CloseRequested?.Invoke();
    }

    private void MaskBorder_Tapped(object sender, TappedRoutedEventArgs e) => RequestClose();

    private void CloseBtn_Click(object sender, RoutedEventArgs e) => RequestClose();

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

    // —— 左侧大类导航 ——

    /// <summary>当前选中的大类 (主题切换后重应用画刷用)。</summary>
    private string _currentCategory = "Appearance";

    /// <summary>大类切换：按 Tag 显示对应内容页并更新选中态。</summary>
    private void Category_Click(object sender, RoutedEventArgs e)
    {
        if (sender is not Button btn || btn.Tag is not string category)
        {
            return;
        }

        ApplyCategoryVisuals(category);
    }

    private void ApplyCategoryVisuals(string category)
    {
        _currentCategory = category;
        // 显式逐页映射 (第五页加入后弃用"非前三类"反推, 避免新增页时漏改)
        AppearancePanel.Visibility = category == "Appearance" ? Visibility.Visible : Visibility.Collapsed;
        ConnectionPanel.Visibility = category == "Connection" ? Visibility.Visible : Visibility.Collapsed;
        FileEditPanel.Visibility = category == "FileEdit" ? Visibility.Visible : Visibility.Collapsed;
        TransferPanel.Visibility = category == "Transfer" ? Visibility.Visible : Visibility.Collapsed;
        RegistrationPanel.Visibility = category == "Registration" ? Visibility.Visible : Visibility.Collapsed;
        AboutPanel.Visibility = category == "About" ? Visibility.Visible : Visibility.Collapsed;
        SetCategoryState(AppearanceCatBtn, category == "Appearance");
        SetCategoryState(ConnectionCatBtn, category == "Connection");
        SetCategoryState(FileEditCatBtn, category == "FileEdit");
        SetCategoryState(TransferCatBtn, category == "Transfer");
        SetCategoryState(RegistrationCatBtn, category == "Registration");
        SetCategoryState(AboutCatBtn, category == "About");
    }

    /// <summary>
    /// 大类按钮选中态: 选中加深——选中近黑底白字 (暗) / 深灰底黑字 (亮);
    /// 未选中基础背景随主题 (亮无块底 / 暗 #243040 块底)。
    /// 配色不经 ThemeDictionaries/资源索引器解析——实测 (2026-09-30 UIA 取证)
    /// Resources["key"] 取到的主题字典画刷实例在切主题后不重新解析 (亮主题拿到
    /// 暗色画刷), 故直读 ActualTheme 配色, 每次应用新建画刷。
    /// </summary>
    private void SetCategoryState(Button btn, bool selected)
    {
        bool dark = ActualTheme == ElementTheme.Dark;
        if (selected)
        {
            btn.Background = new Microsoft.UI.Xaml.Media.SolidColorBrush(dark
                ? Windows.UI.Color.FromArgb(255, 0x0D, 0x14, 0x20)
                : Windows.UI.Color.FromArgb(255, 0xDA, 0xDA, 0xDA));
            btn.Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(dark
                ? Windows.UI.Color.FromArgb(255, 0xFF, 0xFF, 0xFF)
                : Windows.UI.Color.FromArgb(255, 0x22, 0x22, 0x22));
        }
        else
        {
            btn.Background = dark
                ? new Microsoft.UI.Xaml.Media.SolidColorBrush(Windows.UI.Color.FromArgb(255, 0x24, 0x30, 0x40))
                : null;
            btn.ClearValue(ForegroundProperty);
        }
    }

    // —— 关于页 ——

    /// <summary>官网域名 (已注册); GitHub 组织与 Microsoft Store 上架后经官网/渠道卡直达。</summary>
    private const string WebsiteUrl = "https://ifrees.top";

    /// <summary>GitHub 组织主页 (渠道卡直链; 仓库上架后可改为仓库地址)。</summary>
    private const string GitHubUrl = "https://github.com/iFreeshine";

    private async void AppWebsiteBtn_Click(object sender, RoutedEventArgs e)
        => await LaunchInBrowserAsync(WebsiteUrl);

    private async void AppGitHubBtn_Click(object sender, RoutedEventArgs e)
        => await LaunchInBrowserAsync(GitHubUrl);

    private static async Task LaunchInBrowserAsync(string url)
    {
        try
        {
            await Windows.System.Launcher.LaunchUriAsync(new Uri(url));
        }
        catch
        {
            // 打开浏览器失败 (极端环境) 静默: 域名/组织名在卡片文本中可手动复制
        }
    }

    /// <summary>填充版本与构建日期: 版本取程序集版本 (csproj Version, 显示三段 x.y.z);
    /// 构建日期取 exe 修改时间——每次发布自动更新, 无需改代码。</summary>
    private void UpdateAboutVersion()
    {
        var ver = System.Reflection.Assembly.GetEntryAssembly()?.GetName().Version;
        var versionText = ver?.ToString(3) ?? "0.8.0";
        var exePath = Environment.ProcessPath;
        var build = exePath is null ? default : File.GetLastWriteTime(exePath);
        AppVersionDesc.Text = build.Year >= 2000
            ? string.Format(
                LocalizationService.GetString("AppVersionDescFormat"),
                versionText, build.ToString("yyyy-MM-dd"))
            : versionText; // 取不到构建时间 (极端环境) 只显示版本
    }

    // —— 设置表单 ——

    /// <summary>按当前应用设置回填表单选中项与数值。</summary>
    private void SyncFromSettings()
    {
        _syncing = true;
        ThemeOptions.SelectedIndex = ThemeService.IsDark ? 0 : 1;
        SelectCurrentLanguage(); // 语言列表动态 (含外置), 只重选中当前项
        // Slider.Value 超出 Min/Max 时自动 coerce 到边界 (兼容旧档存档值越界)
        TimeoutSlider.Value = AppSettingsService.ConnectTimeoutSeconds;
        MaxRetriesSlider.Value = AppSettingsService.MaxRetries;
        RetryDelaySlider.Value = AppSettingsService.RetryDelaySeconds;
        OperationTimeoutSlider.Value = AppSettingsService.OperationTimeoutSeconds;
        OpenModeBox.SelectedIndex = AppSettingsService.ConnectOpenMode switch
        {
            "current" => 1,
            "new" => 2,
            _ => 0, // ask 每次询问
        };
        ConcurrentSlider.Value = AppSettingsService.ConcurrentTransfers;
        EditorModeOptions.SelectedIndex = AppSettingsService.EditorMode == "external" ? 1 : 0;
        EditorPathBox.Text = AppSettingsService.EditorPath;
        DeleteServerCmdToggle.IsOn = AppSettingsService.RemoteDeleteMode != "recursive";
        KeepAliveToggle.IsOn = AppSettingsService.KeepAliveEnabled;
        KeepAliveIntervalSlider.Value = AppSettingsService.KeepAliveIntervalSeconds;
        UpdateKeepAliveIntervalEnabled();
        UpdateDeleteModeHint();
        UpdateEditorPathCardState();
        UpdateSliderValueTexts();
        _syncing = false;
    }

    private void ThemeOptions_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        ThemeService.SetTheme(ThemeOptions.SelectedIndex != 1); // 0=深色, 1=浅色
    }

    /// <summary>
    /// 语言下拉动态填充: 内置 zh-CN/en-US + exe 旁 locales\ 目录发现的 *.json
    /// (用户自建语言文件重启后出现在此)。填充期间事件未就绪, 直接抑制联动。
    /// </summary>
    private void PopulateLanguages()
    {
        _syncing = true;
        LangOptions.Items.Clear();
        foreach (var lang in LocalizationService.AvailableLanguages)
        {
            LangOptions.Items.Add(new ComboBoxItem
            {
                Content = LocalizationService.GetLanguageDisplayName(lang),
                Tag = lang,
            });
        }
        SelectCurrentLanguage();
        _syncing = false;
    }

    /// <summary>按当前语言选中下拉项 (调用方负责 _syncing 抑制联动)。</summary>
    private void SelectCurrentLanguage()
    {
        var current = LocalizationService.CurrentCulture;
        for (int i = 0; i < LangOptions.Items.Count; i++)
        {
            if (LangOptions.Items[i] is ComboBoxItem item &&
                item.Tag is string tag &&
                string.Equals(tag, current, System.StringComparison.OrdinalIgnoreCase))
            {
                LangOptions.SelectedIndex = i;
                return;
            }
        }

        if (LangOptions.Items.Count > 0)
        {
            LangOptions.SelectedIndex = 0; // 兜底: 列表非空但无匹配 (异常情况) 选第一项
        }
    }

    private void LangOptions_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        if (LangOptions.SelectedItem is ComboBoxItem item && item.Tag is string lang)
        {
            LocalizationService.SetCulture(lang);
        }
    }

    /// <summary>
    /// 连接默认设置（超时/重试/重试延迟/操作超时）Slider 拖动即时落盘并刷新值文本。
    /// 挡位: 超时 15~30 步进 5, 重试 1~3, 延迟 3~10 步进 1, 操作超时 5~15 步进 5。
    /// </summary>
    private void ConnectSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        switch ((sender as FrameworkElement)?.Name)
        {
            case nameof(TimeoutSlider):
                AppSettingsService.SetConnectTimeout((int)TimeoutSlider.Value);
                break;
            case nameof(MaxRetriesSlider):
                AppSettingsService.SetMaxRetries((int)MaxRetriesSlider.Value);
                break;
            case nameof(RetryDelaySlider):
                AppSettingsService.SetRetryDelay((int)RetryDelaySlider.Value);
                break;
            case nameof(OperationTimeoutSlider):
                AppSettingsService.SetOperationTimeout((int)OperationTimeoutSlider.Value);
                break;
        }

        UpdateSliderValueTexts();
    }

    /// <summary>按当前语言刷新五个 Slider 旁的当前值文本（秒/次/个）。</summary>
    private void UpdateSliderValueTexts()
    {
        var seconds = LocalizationService.GetString("ValueFormatSeconds");
        var times = LocalizationService.GetString("ValueFormatTimes");
        var tasks = LocalizationService.GetString("ValueFormatTasks");
        TimeoutValueText.Text = string.Format(seconds, (int)TimeoutSlider.Value);
        MaxRetriesValueText.Text = string.Format(times, (int)MaxRetriesSlider.Value);
        RetryDelayValueText.Text = string.Format(seconds, (int)RetryDelaySlider.Value);
        OperationTimeoutValueText.Text = string.Format(seconds, (int)OperationTimeoutSlider.Value);
        ConcurrentValueText.Text = string.Format(tasks, (int)ConcurrentSlider.Value);
        KeepAliveIntervalValueText.Text = string.Format(seconds, (int)KeepAliveIntervalSlider.Value);
    }

    /// <summary>连接打开方式修改即时落盘（每次询问 / 当前标签页 / 新标签页）。</summary>
    private void OpenModeBox_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        AppSettingsService.SetConnectOpenMode(OpenModeBox.SelectedIndex switch
        {
            1 => "current",
            2 => "new",
            _ => "ask",
        });
    }

    /// <summary>远程删除开关（开=服务端命令优先, 关=纯 SFTP 递归）即时落盘，联动说明文字。</summary>
    private void DeleteServerCmdToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        AppSettingsService.SetRemoteDeleteMode(DeleteServerCmdToggle.IsOn ? "exec" : "recursive");
        UpdateDeleteModeHint();
    }

    /// <summary>按开关状态刷新删除方式说明文字（语言切换时经 ApplyResources 重刷）。</summary>
    private void UpdateDeleteModeHint()
        => DeleteModeHint.Text = LocalizationService.GetString(
            DeleteServerCmdToggle.IsOn ? "DeleteModeHintExec" : "DeleteModeHintRecursive");

    /// <summary>保活开关即时落盘, 联动间隔卡片可用性。</summary>
    private void KeepAliveToggle_Toggled(object sender, RoutedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        AppSettingsService.SetKeepAliveEnabled(KeepAliveToggle.IsOn);
        UpdateKeepAliveIntervalEnabled();
    }

    /// <summary>保活间隔挡位拖动即时落盘（15/30/45/60 秒）。</summary>
    private void KeepAliveIntervalSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        AppSettingsService.SetKeepAliveInterval((int)KeepAliveIntervalSlider.Value);
        UpdateSliderValueTexts();
    }

    /// <summary>间隔卡片随保活开关启用/禁用（Border 无 IsEnabled, 用 slider 可用性+降透明度表达）。</summary>
    private void UpdateKeepAliveIntervalEnabled()
    {
        KeepAliveIntervalSlider.IsEnabled = KeepAliveToggle.IsOn;
        KeepAliveIntervalCard.Opacity = KeepAliveToggle.IsOn ? 1 : 0.4;
    }

    // —— 传输设置 ——

    /// <summary>传输-并发传输数（1~3 挡位）Slider 拖动即时落盘并刷新值文本。</summary>
    private void TransferSlider_ValueChanged(object sender, RangeBaseValueChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        AppSettingsService.SetConcurrentTransfers((int)ConcurrentSlider.Value);
        UpdateSliderValueTexts();
    }

    // —— 文件编辑设置 ——

    /// <summary>编辑方式切换（内置编辑器 / 外部编辑器）即时落盘，并联动路径行显隐。</summary>
    private void EditorModeOptions_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        AppSettingsService.SetEditorMode(EditorModeOptions.SelectedIndex == 1 ? "external" : "wpf");
        UpdateEditorPathCardState();
    }

    /// <summary>外部编辑器路径修改即时落盘。</summary>
    private void EditorPathBox_TextChanged(object sender, TextChangedEventArgs e)
    {
        if (!_initialized || _syncing)
        {
            return;
        }

        AppSettingsService.SetEditorPath(EditorPathBox.Text);
    }

    /// <summary>路径卡片随编辑方式启用/禁用：内置时整卡变灰（同保活间隔卡片效果）。</summary>
    private void UpdateEditorPathCardState()
    {
        var external = EditorModeOptions.SelectedIndex == 1;
        EditorPathBox.IsEnabled = external;
        EditorBrowseBtn.IsEnabled = external;
        EditorPathCard.Opacity = external ? 1 : 0.4;
    }

    /// <summary>浏览选择外部编辑器 exe（桌面应用需用主窗口句柄初始化选择器，非打包场景）。</summary>
    private async void EditorBrowseBtn_Click(object sender, RoutedEventArgs e)
    {
        if (App.MainAppWindow is null)
        {
            return;
        }

        var picker = new Windows.Storage.Pickers.FileOpenPicker();
        WinRT.Interop.InitializeWithWindow.Initialize(picker,
            WinRT.Interop.WindowNative.GetWindowHandle(App.MainAppWindow));
        picker.FileTypeFilter.Add(".exe");
        var file = await picker.PickSingleFileAsync();
        if (file is not null)
        {
            EditorPathBox.Text = file.Path; // TextChanged 落盘
        }
    }

    private void ApplyResources()
    {
        TitleText.Text = LocalizationService.GetString("SettingsTitle");
        AppearanceCatBtn.Content = LocalizationService.GetString("CategoryAppearance");
        ConnectionCatBtn.Content = LocalizationService.GetString("CategoryConnection");
        TransferCatBtn.Content = LocalizationService.GetString("CategoryTransfer");
        ThemeLabel.Text = LocalizationService.GetString("ThemeLabel");
        LanguageLabel.Text = LocalizationService.GetString("LanguageLabel");
        ThemeDarkOption.Content = LocalizationService.GetString("ThemeDark");
        ThemeLightOption.Content = LocalizationService.GetString("ThemeLight");
        TimeoutLabel.Text = LocalizationService.GetString("TimeoutSeconds");
        OperationTimeoutLabel.Text = LocalizationService.GetString("OperationTimeoutSeconds");
        MaxRetriesLabel.Text = LocalizationService.GetString("MaxRetries");
        RetryDelayLabel.Text = LocalizationService.GetString("RetryDelaySeconds");
        OpenModeLabel.Text = LocalizationService.GetString("ConnectOpenMode");
        OpenModeAskItem.Content = LocalizationService.GetString("OpenModeAsk");
        OpenModeCurrentItem.Content = LocalizationService.GetString("OpenModeCurrent");
        OpenModeNewItem.Content = LocalizationService.GetString("OpenModeNew");
        DeleteModeLabel.Text = LocalizationService.GetString("DeleteModeLabel");
        UpdateDeleteModeHint();
        ConcurrentLabel.Text = LocalizationService.GetString("ConcurrentTransfers");
        ConcurrentHint.Text = LocalizationService.GetString("ConcurrentTransfersHint");
        FileEditCatBtn.Content = LocalizationService.GetString("CategoryFileEdit");
        EditorModeLabel.Text = LocalizationService.GetString("EditorModeLabel");
        EditorBuiltinOption.Content = LocalizationService.GetString("EditorBuiltinOption");
        EditorExternalOption.Content = LocalizationService.GetString("EditorExternalOption");
        EditorPathLabel.Text = LocalizationService.GetString("EditorPathLabel");
        EditorBrowseBtn.Content = LocalizationService.GetString("EditorBrowseBtn");
        EditorPathHint.Text = LocalizationService.GetString("EditorPathHint");
        RegistrationCatBtn.Content = LocalizationService.GetString("CategoryRegistration");
        RegistrationTitle.Text = LocalizationService.GetString("SettingsRegistrationTitle");
        AboutCatBtn.Content = LocalizationService.GetString("CategoryAbout");
        AboutTitle.Text = LocalizationService.GetString("SettingsAboutTitle");
        AppWebsiteBtn.Content = LocalizationService.GetString("AppWebsiteBtn");
        AppChannelsLabel.Text = LocalizationService.GetString("AppChannelsLabel");
        AppChannelsDesc.Text = LocalizationService.GetString("AppChannelsDesc");
        AppLicenseLabel.Text = LocalizationService.GetString("AppLicenseLabel");
        AppLicenseDesc.Text = LocalizationService.GetString("AppLicenseDesc");
        AppCopyrightLabel.Text = LocalizationService.GetString("AppCopyrightLabel");
        AppCopyrightDesc.Text = LocalizationService.GetString("AppCopyrightDesc");
        RegFreeTitle.Text = LocalizationService.GetString("RegFreeTitle");
        RegFreeDesc.Text = LocalizationService.GetString("RegFreeDesc");
        AppSupportTitle.Text = LocalizationService.GetString("AppSupportTitle");
        AppSupportDesc.Text = LocalizationService.GetString("AppSupportDesc");
        UpdateAboutVersion();
        RegStatusLabel.Text = LocalizationService.GetString("RegStatusLabel");
        RegCodeLabel.Text = LocalizationService.GetString("RegCodeLabel");
        RegCodeHint.Text = LocalizationService.GetString("RegCodeHint");
        RegCodeHint.ClearValue(Microsoft.UI.Xaml.Controls.TextBlock.ForegroundProperty); // 清激活结果色
        RegActivateBtn.Content = LocalizationService.GetString("RegActivateBtn");
        RegBuyCnBtn.Content = LocalizationService.GetString("RegBuyCnBtn");
        RegBuyIntlBtn.Content = LocalizationService.GetString("RegBuyIntlBtn");
        RegNoticeLabel.Text = LocalizationService.GetString("RegNoticeLabel");

        // 官方 SettingsCard 排版: 页顶标题 + 各卡片"设置名下方的灰色描述"
        AppearanceTitle.Text = LocalizationService.GetString("SettingsAppearanceTitle");
        ConnectionTitle.Text = LocalizationService.GetString("SettingsConnectionTitle");
        FileEditTitle.Text = LocalizationService.GetString("SettingsFileEditTitle");
        TransferTitle.Text = LocalizationService.GetString("SettingsTransferTitle");
        ThemeDesc.Text = LocalizationService.GetString("ThemeDesc");
        LanguageDesc.Text = LocalizationService.GetString("LanguageDesc");
        TimeoutDesc.Text = LocalizationService.GetString("TimeoutDesc");
        MaxRetriesDesc.Text = LocalizationService.GetString("MaxRetriesDesc");
        RetryDelayDesc.Text = LocalizationService.GetString("RetryDelayDesc");
        OpenModeDesc.Text = LocalizationService.GetString("OpenModeDesc");
        OperationTimeoutDesc.Text = LocalizationService.GetString("OperationTimeoutDesc");
        KeepAliveLabel.Text = LocalizationService.GetString("KeepAliveLabel");
        KeepAliveDesc.Text = LocalizationService.GetString("KeepAliveDesc");
        KeepAliveIntervalLabel.Text = LocalizationService.GetString("KeepAliveIntervalLabel");
        KeepAliveIntervalDesc.Text = LocalizationService.GetString("KeepAliveIntervalDesc");
        EditorModeDesc.Text = LocalizationService.GetString("EditorModeDesc");
        UpdateKeepAliveIntervalEnabled();
        UpdateEditorPathCardState();
        UpdateSliderValueTexts();
        UpdateRegistrationUi();
    }

    // —— 注册设置 ——

    /// <summary>
    /// StateChanged 可能在后台线程触发 (远程刷新/激活上报回调), 统一 marshal 到 UI 线程。
    /// </summary>
    private void OnRegistrationStateChanged()
        => DispatcherQueue.TryEnqueue(UpdateRegistrationUi);

    /// <summary>按 RegistrationService 当前状态刷新注册页（状态行/勋章/公告/获取入口）。</summary>
    private void UpdateRegistrationUi()
    {
        var state = RegistrationService.Current;
        var badgeOn = state.Registered && !state.Revoked && RegistrationService.BadgeEnabled;
        RegBadgePill.Visibility = badgeOn ? Visibility.Visible : Visibility.Collapsed;
        if (badgeOn)
        {
            RegBadgeText.Text = string.IsNullOrWhiteSpace(RegistrationService.BadgeText)
                ? LocalizationService.GetString("RegBadgeFallback")
                : RegistrationService.BadgeText;
        }

        if (state.Revoked)
        {
            RegStatusDesc.Text = LocalizationService.GetString("RegStatusRevoked");
        }
        else if (state.Registered)
        {
            var expires = state.ExpiresAt == 0
                ? LocalizationService.GetString("RegPermanent")
                : string.Format(
                    LocalizationService.GetString("RegExpiresTo"),
                    DateTimeOffset.FromUnixTimeSeconds(state.ExpiresAt).ToLocalTime()
                        .ToString("yyyy-MM-dd"));
            RegStatusDesc.Text = string.Format(
                LocalizationService.GetString("RegStatusOk"), state.UserName, expires);
        }
        else
        {
            RegStatusDesc.Text = LocalizationService.GetString("RegStatusFree");
        }

        var notice = RegistrationService.RemoteNotice;
        RegNoticeCard.Visibility = string.IsNullOrEmpty(notice)
            ? Visibility.Collapsed
            : Visibility.Visible;
        RegNoticeText.Text = notice;

        // 获取注册码入口: PurchaseUrl 未配置时隐藏 (初期手动发码阶段)
        RegBuyCnBtn.Visibility = string.IsNullOrEmpty(RegistrationService.PurchaseUrlCn)
            ? Visibility.Collapsed
            : Visibility.Visible;
        RegBuyIntlBtn.Visibility = string.IsNullOrEmpty(RegistrationService.PurchaseUrlIntl)
            ? Visibility.Collapsed
            : Visibility.Visible;
    }

    /// <summary>激活注册码：结果 (成功绿/失败红) 写入 hint 行; 成功时 StateChanged 联动勋章。</summary>
    private void RegActivateBtn_Click(object sender, RoutedEventArgs e)
    {
        var err = RegistrationService.Activate(RegCodeBox.Text);
        RegCodeHint.Text = err ?? LocalizationService.GetString("RegActivateOk");
        RegCodeHint.Foreground = new Microsoft.UI.Xaml.Media.SolidColorBrush(
            err is null
                ? Windows.UI.Color.FromArgb(255, 0x2E, 0x99, 0x60)
                : Windows.UI.Color.FromArgb(255, 0xE0, 0x53, 0x53));
    }

    /// <summary>购买注册码 (双渠道按钮): 国内跳爱发电, 国际跳 Ko-fi; URL 由 Worker 下发。</summary>
    private async void RegBuyCnBtn_Click(object sender, RoutedEventArgs e)
    {
        if (Uri.TryCreate(RegistrationService.PurchaseUrlCn, UriKind.Absolute, out var uri))
        {
            _ = await Windows.System.Launcher.LaunchUriAsync(uri);
        }
    }

    private async void RegBuyIntlBtn_Click(object sender, RoutedEventArgs e)
    {
        if (Uri.TryCreate(RegistrationService.PurchaseUrlIntl, UriKind.Absolute, out var uri))
        {
            _ = await Windows.System.Launcher.LaunchUriAsync(uri);
        }
    }

    // —— ESC 关闭（与主机面板同款, 加速键必须挂在窗口根元素上） ——

    private void AttachEscape()
    {
        if (PanelRoot.XamlRoot?.Content is FrameworkElement root)
        {
            root.KeyDown += PanelRoot_KeyDown;
        }
    }

    private void DetachEscape()
    {
        if (PanelRoot.XamlRoot?.Content is FrameworkElement root)
        {
            root.KeyDown -= PanelRoot_KeyDown;
        }
    }

    private void PanelRoot_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Escape)
        {
            RequestClose();
            e.Handled = true;
        }
    }
}
