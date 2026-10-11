using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Windows;
using System.Windows.Controls;
using System.Windows.Controls.Primitives;
using System.Windows.Input;
using System.Windows.Interop;
using System.Windows.Media;
using System.Windows.Shell;
using ICSharpCode.AvalonEdit.Highlighting;
using ICSharpCode.AvalonEdit.Search;

namespace iFreeSftp.WpfEditor;

/// <summary>
/// WPF 代码高亮编辑器窗口（预览）：基于 AvalonEdit，按扩展名自动选择语法高亮定义。
/// 自绘无边框外框（WindowChrome），标题栏/工具栏/背景配色与主界面同款并跟随明暗主题；
/// 工具栏提供保存/撤消/查找/自动换行/行号开关。经 <see cref="Open"/> 在专用 STA 线程上
/// 承载（WinUI 3 窗口无法直接内嵌 WPF 内容），与主界面互不干扰，关窗即结束线程。
/// </summary>
public sealed class WpfEditorWindow : Window
{
    /// <summary>允许打开的最大文件大小（与内置编辑器一致，10 MB），防止大文件卡死 UI。</summary>
    private const long MaxFileSize = 10 * 1024 * 1024;

    // —— 与主界面同款的主题色（App.xaml ThemeDictionaries 中的 FolderArea 值） ——
    private static readonly Color BackDark = Color.FromRgb(0x1B, 0x24, 0x30);   // 窗口底色
    private static readonly Color BackLight = Color.FromRgb(0xF9, 0xF9, 0xF9);
    // —— hover/勾选态: Windows 系统风格不透明灰, 相对所在容器底色明暗各一档
    //    (标题栏 #1B2430/#F9F9F9 与工具栏 #2E3948/#DDDDDD 底色不同, 分别设值) ——
    private static readonly Color TitleHoverDark = Color.FromRgb(0x2C, 0x38, 0x46);
    private static readonly Color TitleHoverLight = Color.FromRgb(0xE4, 0xE4, 0xE4);
    private static readonly Color ToolHoverDark = Color.FromRgb(0x3A, 0x46, 0x56);
    private static readonly Color ToolHoverLight = Color.FromRgb(0xCC, 0xCC, 0xCC);
    private static readonly Color ToolCheckedDark = Color.FromRgb(0x46, 0x53, 0x64);
    private static readonly Color ToolCheckedLight = Color.FromRgb(0xBF, 0xBF, 0xBF);
    private static readonly Color ToolBarDark = Color.FromRgb(0x2E, 0x39, 0x48);  // 工具栏底色(暗更亮)
    private static readonly Color ToolBarLight = Color.FromRgb(0xDD, 0xDD, 0xDD); // 工具栏底色(亮更暗)

    // 已打开的编辑器窗口注册表: 主面板切换明暗主题时经 ApplyThemeAll 同步。
    private static readonly List<WpfEditorWindow> OpenWindows = new();

    // 高亮定义是全局共享单例, 暗色替换前按定义快照原始命名色, 切回亮色时还原。
    private static readonly object HighlightLock = new();
    private static readonly Dictionary<IHighlightingDefinition, Dictionary<string, HighlightingBrush?>> OriginalBrushes = new();

    // 暗色高亮命名色 → VS 风配色（各语言定义命名不完全一致, 未命中保持原色）。
    private static readonly Dictionary<string, Color> DarkColorMap = new(StringComparer.OrdinalIgnoreCase)
    {
        ["Comment"] = Color.FromRgb(0x6A, 0x99, 0x55),
        ["String"] = Color.FromRgb(0xCE, 0x91, 0x78),
        ["Number"] = Color.FromRgb(0xB5, 0xCE, 0xA8),
        ["Preprocessor"] = Color.FromRgb(0xC5, 0x86, 0xC0),
        ["Keywords"] = Color.FromRgb(0x56, 0x9C, 0xD6),
        ["Keyword"] = Color.FromRgb(0x56, 0x9C, 0xD6),
        ["Type"] = Color.FromRgb(0x4E, 0xC9, 0xB0),
        ["Types"] = Color.FromRgb(0x4E, 0xC9, 0xB0),
    };

    private readonly ICSharpCode.AvalonEdit.TextEditor _editor = new()
    {
        FontFamily = new FontFamily("Consolas"),
        FontSize = 13,
        ShowLineNumbers = true,
    };

    private readonly string _path;
    private readonly bool _zh; // 文案语言（跟随主界面）
    private bool _dark;        // 明暗主题（主面板切换时经 SetTheme 重放）
    private readonly string? _loadError; // 非 null = 打开失败（Loaded 后弹提示并关窗）
    private readonly SearchPanel? _searchPanel;
    private TextBlock _titleText = new();
    private Button _maxButton = new();
    private Action<string>? _onSaved; // 保存成功回调 (远程编辑回传), 编辑器线程上触发
    private StackPanel _toolbar = new(); // 工具栏（通栏背景随主题）
    private readonly List<(Button Btn, bool IsClose)> _windowButtons = new(); // 标题栏按钮（样式随主题重建）
    private readonly List<(Control Btn, bool IsToggle)> _toolButtons = new(); // 工具栏按钮（样式随主题重建）
    private Encoding _writeEncoding = new UTF8Encoding(false);
    private bool _loading;    // 程序性填充文本期间抑制 dirty 标记
    private bool _dirty;      // 有未保存修改（标题带 * 提示）
    private bool _forceClose; // 确认后强制关闭, 跳过再次确认

    static WpfEditorWindow()
    {
        // 注册 ANSI 代码页（GBK 等）支持；失败时回退 Latin-1（逐字节无损, 不破坏内容）。
        try
        {
            Encoding.RegisterProvider(System.Text.CodePagesEncodingProvider.Instance);
        }
        catch
        {
            // 重复注册等场景忽略。
        }
    }

    private WpfEditorWindow(string path, bool dark, bool zh)
    {
        _path = path;
        _zh = zh;
        _dark = dark;
        Title = Path.GetFileName(path);
        Width = 960;
        Height = 720;
        WindowStartupLocation = WindowStartupLocation.CenterScreen;
        WindowStyle = WindowStyle.None;
        ResizeMode = ResizeMode.CanResize;

        // 自绘外框: 无系统标题栏, 标题栏区域内仍可拖拽/双击最大化, 四周保留缩放边。
        WindowChrome.SetWindowChrome(this, new WindowChrome
        {
            CaptionHeight = 32,
            ResizeBorderThickness = new Thickness(5),
            GlassFrameThickness = new Thickness(0),
            CornerRadius = new CornerRadius(0),
            UseAeroCaptionButtons = false,
        });

        var text = string.Empty;
        try
        {
            if (new FileInfo(path).Length > MaxFileSize)
            {
                _loadError = S("文件超过 10 MB，无法在 WPF 编辑器中打开。",
                    "The file exceeds 10 MB and cannot be opened in the WPF editor.");
            }
            else
            {
                (text, _writeEncoding) = ReadFile(path);
            }
        }
        catch (Exception ex)
        {
            _loadError = ex.Message;
        }

        // 按扩展名取高亮定义（C#/XML/JS 等, AvalonEdit 内置）；无定义则纯文本。
        _editor.SyntaxHighlighting = HighlightingManager.Instance.GetDefinitionByExtension(Path.GetExtension(path));
        _editor.TextChanged += (_, _) =>
        {
            if (_loading || _dirty)
            {
                return;
            }

            _dirty = true;
            UpdateTitle();
        };

        _searchPanel = SearchPanel.Install(_editor.TextArea); // Ctrl+F / 工具栏共用

        Content = BuildLayout();
        ApplyScrollViewerStyle(); // 隐式 ScrollViewer 模板: 去掉右下角系统色角块
        SetTheme(dark);           // 底色/标题栏/工具栏/按钮/滚动条/编辑区整体按主题上色

        lock (OpenWindows) OpenWindows.Add(this); // 供主面板切主题时同步
        Closed += (_, _) => { lock (OpenWindows) OpenWindows.Remove(this); };

        SourceInitialized += (_, _) => ApplyWindowCorner(); // Win11 圆角（Win10 忽略）

        // 未保存确认：Yes=保存并关闭 No=直接关闭 Cancel=取消。
        Closing += (_, e) =>
        {
            if (_forceClose || !_dirty)
            {
                return;
            }

            var result = MessageBox.Show(this,
                S("当前文件有未保存的修改，是否保存？", "This file has unsaved changes. Save them?"),
                S("未保存的更改", "Unsaved Changes"),
                MessageBoxButton.YesNoCancel, MessageBoxImage.Question);
            if (result == MessageBoxResult.Cancel)
            {
                e.Cancel = true;
                return;
            }

            if (result == MessageBoxResult.Yes && !Save())
            {
                e.Cancel = true;
            }
        };

        // 打开失败：窗口就绪后弹提示并关窗。
        Loaded += (_, _) =>
        {
            if (_loadError is null)
            {
                return;
            }

            MessageBox.Show(this, _loadError, S("无法打开文件", "Cannot Open File"),
                MessageBoxButton.OK, MessageBoxImage.Warning);
            _forceClose = true;
            Close();
        };

        _loading = true;
        _editor.Text = text;
        _loading = false;
        UpdateTitle();
    }

    /// <summary>
    /// 在专用 STA 线程打开编辑器窗口（WPF 需要自己的消息循环），关窗即结束线程。
    /// onSaved = 保存成功后回调（参数为保存的文件路径, 编辑器线程上触发, 跨线程者自行投递,
    /// 远程编辑据此回传上传）；onClosed = 关窗后回调（编辑器线程上触发, 供调用方清理临时文件）。
    /// </summary>
    public static void Open(string path, bool darkTheme, bool chinese, Action<string>? onSaved = null, Action? onClosed = null)
    {
        var thread = new Thread(() =>
        {
            try
            {
                var window = new WpfEditorWindow(path, darkTheme, chinese)
                {
                    _onSaved = onSaved,
                };
                window.Show();
                window.Closed += (_, _) =>
                {
                    onClosed?.Invoke();
                    System.Windows.Threading.Dispatcher.CurrentDispatcher
                        .BeginInvokeShutdown(System.Windows.Threading.DispatcherPriority.Background);
                };
                System.Windows.Threading.Dispatcher.Run();
            }
            catch (Exception ex)
            {
                // 线程内异常无法回传主窗口状态栏; 总是落盘日志, 并尽量弹框可见化。
                try
                {
                    File.WriteAllText(Path.Combine(Path.GetTempPath(), "iFreeSftp-editor-error.log"),
                        $"[{DateTime.Now:yyyy-MM-dd HH:mm:ss}]\r\n{ex}");
                }
                catch
                {
                    // 日志失败忽略。
                }

                try
                {
                    MessageBox.Show(ex.Message, "iFreeSftp Editor Error",
                        MessageBoxButton.OK, MessageBoxImage.Error);
                }
                catch
                {
                    // 消息框失败忽略。
                }
            }
        });
        thread.SetApartmentState(ApartmentState.STA);
        thread.IsBackground = true;
        thread.Start();
    }

    /// <summary>主面板切换明暗主题时同步所有已打开的编辑器窗口（跨线程经 Dispatcher 投递）。</summary>
    public static void ApplyThemeAll(bool dark)
    {
        lock (OpenWindows)
        {
            foreach (var window in OpenWindows.ToArray())
            {
                window.Dispatcher.BeginInvoke(() => window.SetTheme(dark));
            }
        }
    }

    /// <summary>双语文案（独立线程无主应用的本地化服务, 跟随主界面语言取词）。</summary>
    private string S(string zh, string en) => _zh ? zh : en;

    private static SolidColorBrush Brush(Color color) => new(color);

    /// <summary>
    /// 扁平无边框按钮模板：背景/内边距 TemplateBinding 跟随样式触发器。必须替换默认模板——
    /// WPF 默认按钮模板内置悬停触发器会直接写死 Border 背景，优先级高于 Style Trigger,
    /// 导致样式里设置的 hover 背景色永不生效（表现为永远系统默认灰）。
    /// </summary>
    private static ControlTemplate FlatTemplate(Type controlType)
    {
        var template = new ControlTemplate(controlType);
        var border = new FrameworkElementFactory(typeof(Border));
        border.Name = "Bd";
        border.SetValue(Border.BackgroundProperty, new TemplateBindingExtension(Control.BackgroundProperty));
        border.SetValue(Border.PaddingProperty, new TemplateBindingExtension(Control.PaddingProperty));
        border.SetValue(Border.CornerRadiusProperty, new CornerRadius(4));
        // ContentPresenter 默认取 TemplatedParent 的 Content; 居中对齐与默认模板行为一致。
        var presenter = new FrameworkElementFactory(typeof(ContentPresenter));
        presenter.SetValue(FrameworkElement.HorizontalAlignmentProperty, HorizontalAlignment.Center);
        presenter.SetValue(FrameworkElement.VerticalAlignmentProperty, VerticalAlignment.Center);
        border.AppendChild(presenter);
        template.VisualTree = border;
        return template;
    }

    /// <summary>整体布局：自绘标题栏 + 工具栏 + 编辑器。</summary>
    private UIElement BuildLayout()
    {
        var root = new Grid { Background = Transparent };
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(32) });
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(30) });
        root.RowDefinitions.Add(new RowDefinition { Height = new GridLength(1, GridUnitType.Star) });

        var title = BuildTitleBar();
        Grid.SetRow(title, 0);
        var toolbar = BuildToolbar();
        Grid.SetRow(toolbar, 1);
        Grid.SetRow(_editor, 2);
        root.Children.Add(title);
        root.Children.Add(toolbar);
        root.Children.Add(_editor);
        return root;
    }

    private static readonly SolidColorBrush Transparent = Brushes.Transparent;

    /// <summary>自绘标题栏：标题居左（作为拖拽区），右上角最小化/最大化/关闭。</summary>
    private UIElement BuildTitleBar()
    {
        var bar = new Grid { Background = Transparent };

        _titleText = new TextBlock
        {
            VerticalAlignment = VerticalAlignment.Center,
            Margin = new Thickness(12, 0, 0, 0),
            FontSize = 12,
        };
        bar.Children.Add(_titleText);

        var buttons = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            HorizontalAlignment = HorizontalAlignment.Right,
            VerticalAlignment = VerticalAlignment.Top,
        };
        _maxButton = MakeWindowButton("\uE922", ToggleMaximize);
        buttons.Children.Add(MakeWindowButton("\uE921", () => WindowState = WindowState.Minimized));
        buttons.Children.Add(_maxButton);
        buttons.Children.Add(MakeWindowButton("\uE8BB", Close, closeButton: true));
        bar.Children.Add(buttons);
        return bar;
    }

    /// <summary>工具栏：保存 / 撤消 / 查找 / 自动换行 / 行号。背景与标题栏形成反差（暗更亮/亮更暗）。
    /// 通栏贴边：不留左右边距，背景触及窗口两侧边缘。</summary>
    private UIElement BuildToolbar()
    {
        var bar = new StackPanel
        {
            Orientation = Orientation.Horizontal,
            Margin = new Thickness(0, 0, 0, 4),
        };
        _toolbar = bar;

        bar.Children.Add(MakeToolButton("\uE74E", S("保存", "Save"), () => Save()));
        bar.Children.Add(MakeToolButton("\uE7A7", S("撤消", "Undo"), () => _editor.Document?.UndoStack.Undo()));
        bar.Children.Add(MakeToolButton("\uE721", S("查找", "Find"), () => _searchPanel?.Open()));

        var wrapToggle = MakeToolToggle(S("自动换行", "Wrap"));
        wrapToggle.IsChecked = _editor.WordWrap;
        wrapToggle.Click += (_, _) => _editor.WordWrap = wrapToggle.IsChecked == true;
        bar.Children.Add(wrapToggle);

        var lineNoToggle = MakeToolToggle(S("行号", "Line #s"));
        lineNoToggle.IsChecked = _editor.ShowLineNumbers;
        lineNoToggle.Click += (_, _) => _editor.ShowLineNumbers = lineNoToggle.IsChecked == true;
        bar.Children.Add(lineNoToggle);
        return bar;
    }

    /// <summary>窗口控制按钮（最小化/最大化/关闭），悬停变暗、关闭钮悬停红色。</summary>
    private Button MakeWindowButton(string glyph, Action click, bool closeButton = false)
    {
        var btn = new Button
        {
            Content = new TextBlock
            {
                Text = glyph,
                FontFamily = new FontFamily("Segoe MDL2 Assets"),
                FontSize = 10,
            },
            Width = 40,
            Height = 28,
            Focusable = false,
            BorderThickness = new Thickness(0),
        };
        WindowChrome.SetIsHitTestVisibleInChrome(btn, true); // 在标题栏拖拽区内仍可点击
        btn.Click += (_, _) => click();
        btn.Style = WindowButtonStyle(closeButton);
        _windowButtons.Add((btn, closeButton)); // 主题切换时重建样式
        return btn;
    }

    /// <summary>窗口控制按钮样式：扁平模板 + 系统风格悬停灰，随主题（切换时整体重建）。</summary>
    private Style WindowButtonStyle(bool closeButton)
    {
        var style = new Style(typeof(Button));
        // 背景必须走 Style 而非实例本地值: 本地值优先级高于样式触发器, 会导致 hover 色失效。
        style.Setters.Add(new Setter(Button.BackgroundProperty, Transparent));
        style.Setters.Add(new Setter(Button.ForegroundProperty,
            Brush(_dark ? Colors.White : Color.FromRgb(0x22, 0x22, 0x22))));
        var hover = new Trigger { Property = UIElement.IsMouseOverProperty, Value = true };
        hover.Setters.Add(new Setter(Button.BackgroundProperty,
            Brush(closeButton ? Color.FromRgb(0xE8, 0x11, 0x23) : _dark ? TitleHoverDark : TitleHoverLight)));
        hover.Setters.Add(new Setter(Button.ForegroundProperty, Brush(Colors.White)));
        style.Triggers.Add(hover);
        style.Setters.Add(new Setter(Button.TemplateProperty, FlatTemplate(typeof(Button))));
        return style;
    }

    /// <summary>工具栏普通按钮（图标 + 文字），悬停变暗。</summary>
    private Button MakeToolButton(string glyph, string text, Action click)
    {
        var btn = new Button
        {
            Content = ToolButtonContent(glyph, text),
            Focusable = false,
            BorderThickness = new Thickness(0),
            Padding = new Thickness(8, 3, 8, 3),
            Margin = new Thickness(0, 0, 6, 0),
            Cursor = Cursors.Hand,
        };
        btn.Click += (_, _) => click();
        btn.Style = ToolButtonStyle(toggle: false);
        _toolButtons.Add((btn, false)); // 主题切换时重建样式
        return btn;
    }

    /// <summary>工具栏开关按钮（图标 + 文字 + 勾选态背景）。</summary>
    private ToggleButton MakeToolToggle(string text)
    {
        var btn = new ToggleButton
        {
            Content = ToolButtonContent(string.Empty, text),
            Focusable = false,
            BorderThickness = new Thickness(0),
            Padding = new Thickness(8, 3, 8, 3),
            Margin = new Thickness(0, 0, 6, 0),
            Cursor = Cursors.Hand,
        };
        btn.Style = ToolButtonStyle(toggle: true);
        _toolButtons.Add((btn, true)); // 主题切换时重建样式
        return btn;
    }

    private static StackPanel ToolButtonContent(string glyph, string text)
    {
        var panel = new StackPanel { Orientation = Orientation.Horizontal };
        if (glyph.Length > 0)
        {
            panel.Children.Add(new TextBlock
            {
                Text = glyph,
                FontFamily = new FontFamily("Segoe MDL2 Assets"),
                FontSize = 12,
                VerticalAlignment = VerticalAlignment.Center,
            });
        }

        panel.Children.Add(new TextBlock
        {
            Text = text,
            FontSize = 12,
            Margin = new Thickness(glyph.Length > 0 ? 6 : 0, 0, 0, 0),
            VerticalAlignment = VerticalAlignment.Center,
        });
        return panel;
    }

    /// <summary>工具栏按钮样式：扁平模板 + 悬停/勾选态用系统风格灰，随主题。</summary>
    private Style ToolButtonStyle(bool toggle)
    {
        var style = new Style(toggle ? typeof(ToggleButton) : typeof(Button));
        style.Setters.Add(new Setter(Control.TemplateProperty,
            FlatTemplate(toggle ? typeof(ToggleButton) : typeof(Button))));
        // 背景必须走 Style 而非实例本地值: 本地值优先级高于样式触发器, 会导致 hover 色失效。
        style.Setters.Add(new Setter(Control.BackgroundProperty, Transparent));
        style.Setters.Add(new Setter(Control.ForegroundProperty,
            Brush(_dark ? Colors.White : Color.FromRgb(0x22, 0x22, 0x22))));
        var hover = new Trigger { Property = UIElement.IsMouseOverProperty, Value = true };
        hover.Setters.Add(new Setter(Control.BackgroundProperty, Brush(_dark ? ToolHoverDark : ToolHoverLight)));
        style.Triggers.Add(hover);
        if (toggle)
        {
            var check = new Trigger { Property = ToggleButton.IsCheckedProperty, Value = true };
            check.Setters.Add(new Setter(Control.BackgroundProperty, Brush(_dark ? ToolCheckedDark : ToolCheckedLight)));
            style.Triggers.Add(check);
        }

        return style;
    }

    /// <summary>
    /// 按明暗主题重放窗口整体配色：窗口底/标题栏文字/工具栏/按钮样式/滚动条/编辑区。
    /// 主面板切换主题时经 <see cref="ApplyThemeAll"/> 在各窗口线程上重放。
    /// </summary>
    private void SetTheme(bool dark)
    {
        _dark = dark;
        Background = Brush(dark ? BackDark : BackLight);
        _titleText.Foreground = Brush(dark ? Colors.White : Color.FromRgb(0x22, 0x22, 0x22));
        _toolbar.Background = Brush(dark ? ToolBarDark : ToolBarLight);
        foreach (var (btn, isClose) in _windowButtons)
        {
            btn.Style = WindowButtonStyle(isClose);
        }

        foreach (var (btn, isToggle) in _toolButtons)
        {
            btn.Style = ToolButtonStyle(isToggle);
        }

        ApplyScrollBarStyle();
        if (dark)
        {
            // 编辑区 VS 风暗色配色。
            _editor.Background = Brush(Color.FromRgb(0x1E, 0x1E, 0x1E));
            _editor.Foreground = Brush(Color.FromRgb(0xD4, 0xD4, 0xD4));
            _editor.LineNumbersForeground = Brush(Color.FromRgb(0x85, 0x85, 0x85));
            ApplyDarkHighlighting(_editor.SyntaxHighlighting);
        }
        else
        {
            // 亮色：回到系统默认编辑区配色 + 原始高亮命名色。
            _editor.Background = SystemColors.WindowBrush;
            _editor.Foreground = SystemColors.WindowTextBrush;
            _editor.LineNumbersForeground = SystemColors.WindowTextBrush;
            RestoreHighlighting(_editor.SyntaxHighlighting);
        }
    }

    private void UpdateTitle()
    {
        Title = $"{Path.GetFileName(_path)}{(_dirty ? " *" : string.Empty)} - iFreeSftp";
        _titleText.Text = Title;
    }

    /// <summary>
    /// WinUI 3 风格滚动条（隐式样式作用于编辑器内所有 ScrollBar）：细胶囊无箭头、
    /// 轨道透明，悬停加深加宽；颜色按明暗主题取半透明前景。XamlReader 动态构建。
    /// </summary>
    private void ApplyScrollBarStyle()
    {
        var thumb = _dark ? "#40FFFFFF" : "#28000000";
        var thumbHover = _dark ? "#70FFFFFF" : "#50000000";
        var xaml = """
            <Style xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
                   xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" TargetType="ScrollBar">
              <Style.Triggers>
                <Trigger Property="Orientation" Value="Vertical">
                  <Setter Property="Width" Value="12"/>
                  <Setter Property="Template">
                    <Setter.Value>
                      <ControlTemplate TargetType="ScrollBar">
                        <Grid Background="Transparent">
                          <Track x:Name="PART_Track" IsDirectionReversed="True">
                            <Track.Thumb>
                              <Thumb>
                                <Thumb.Template>
                                  <ControlTemplate TargetType="Thumb">
                                    <Border x:Name="Bd" Width="6" CornerRadius="3" Background="THUMB" HorizontalAlignment="Center"/>
                                    <ControlTemplate.Triggers>
                                      <Trigger Property="IsMouseOver" Value="True">
                                        <Setter TargetName="Bd" Property="Width" Value="8"/>
                                        <Setter TargetName="Bd" Property="Background" Value="THUMBHOVER"/>
                                      </Trigger>
                                    </ControlTemplate.Triggers>
                                  </ControlTemplate>
                                </Thumb.Template>
                              </Thumb>
                            </Track.Thumb>
                          </Track>
                        </Grid>
                      </ControlTemplate>
                    </Setter.Value>
                  </Setter>
                </Trigger>
                <Trigger Property="Orientation" Value="Horizontal">
                  <Setter Property="Height" Value="12"/>
                  <Setter Property="Template">
                    <Setter.Value>
                      <ControlTemplate TargetType="ScrollBar">
                        <Grid Background="Transparent">
                          <Track x:Name="PART_Track">
                            <Track.Thumb>
                              <Thumb>
                                <Thumb.Template>
                                  <ControlTemplate TargetType="Thumb">
                                    <Border x:Name="Bd" Height="6" CornerRadius="3" Background="THUMB" VerticalAlignment="Center"/>
                                    <ControlTemplate.Triggers>
                                      <Trigger Property="IsMouseOver" Value="True">
                                        <Setter TargetName="Bd" Property="Height" Value="8"/>
                                        <Setter TargetName="Bd" Property="Background" Value="THUMBHOVER"/>
                                      </Trigger>
                                    </ControlTemplate.Triggers>
                                  </ControlTemplate>
                                </Thumb.Template>
                              </Thumb>
                            </Track.Thumb>
                          </Track>
                        </Grid>
                      </ControlTemplate>
                    </Setter.Value>
                  </Setter>
                </Trigger>
              </Style.Triggers>
            </Style>
            """;
        Resources[typeof(ScrollBar)] = (Style)System.Windows.Markup.XamlReader.Parse(
            xaml.Replace("THUMBHOVER", thumbHover).Replace("THUMB", thumb));
    }

    /// <summary>最大化/还原切换，图标随之更换。</summary>
    private void ToggleMaximize()
    {
        WindowState = WindowState == WindowState.Maximized ? WindowState.Normal : WindowState.Maximized;
        ((TextBlock)_maxButton.Content).Text = WindowState == WindowState.Maximized ? "\uE923" : "\uE922";
    }

    /// <summary>保存：按探测到的原编码写回。成功返回 true（关闭确认用），并触发 onSaved 回调（远程回传）。</summary>
    private bool Save()
    {
        try
        {
            File.WriteAllText(_path, _editor.Text, _writeEncoding);
            _dirty = false;
            UpdateTitle();
            _onSaved?.Invoke(_path);
            return true;
        }
        catch (Exception ex)
        {
            MessageBox.Show(this, ex.Message, S("保存失败", "Save Failed"),
                MessageBoxButton.OK, MessageBoxImage.Error);
            return false;
        }
    }

    /// <summary>
    /// 隐式 ScrollViewer 模板：复刻 Aero2 默认模板但去掉右下角（垂直/水平滚动条交汇处）
    /// 的 SystemColors.ControlBrush 角矩形——暗色窗口上表现为右下角白色小块。
    /// 与主题无关, 只需在构造时应用一次。
    /// </summary>
    private void ApplyScrollViewerStyle()
    {
        var xaml = """
            <Style xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation"
                   xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" TargetType="ScrollViewer">
              <Setter Property="Template">
                <Setter.Value>
                  <ControlTemplate TargetType="ScrollViewer">
                    <Grid Background="{TemplateBinding Background}">
                      <Grid.ColumnDefinitions>
                        <ColumnDefinition Width="*"/>
                        <ColumnDefinition Width="Auto"/>
                      </Grid.ColumnDefinitions>
                      <Grid.RowDefinitions>
                        <RowDefinition Height="*"/>
                        <RowDefinition Height="Auto"/>
                      </Grid.RowDefinitions>
                      <ScrollContentPresenter Margin="{TemplateBinding Padding}"
                                              Content="{TemplateBinding Content}"
                                              ContentTemplate="{TemplateBinding ContentTemplate}"
                                              CanContentScroll="{TemplateBinding CanContentScroll}"/>
                      <ScrollBar Grid.Column="1" Minimum="0" Cursor="Arrow"
                                 Maximum="{TemplateBinding ScrollableHeight}"
                                 ViewportSize="{TemplateBinding ViewportHeight}"
                                 Value="{Binding VerticalOffset, Mode=OneWay, RelativeSource={RelativeSource TemplatedParent}}"
                                 Visibility="{TemplateBinding ComputedVerticalScrollBarVisibility}"/>
                      <ScrollBar Orientation="Horizontal" Grid.Row="1" Minimum="0" Cursor="Arrow"
                                 Maximum="{TemplateBinding ScrollableWidth}"
                                 ViewportSize="{TemplateBinding ViewportWidth}"
                                 Value="{Binding HorizontalOffset, Mode=OneWay, RelativeSource={RelativeSource TemplatedParent}}"
                                 Visibility="{TemplateBinding ComputedHorizontalScrollBarVisibility}"/>
                    </Grid>
                  </ControlTemplate>
                </Setter.Value>
              </Setter>
            </Style>
            """;
        Resources[typeof(ScrollViewer)] = (Style)System.Windows.Markup.XamlReader.Parse(xaml);
    }

    /// <summary>
    /// 暗色主题：把高亮定义里的命名色替换为暗色友好色（VS 风配色）。定义是全局共享单例，
    /// 首次替换前快照原始色供切回亮色还原（多窗口共享, 静态锁保护）。
    /// </summary>
    private void ApplyDarkHighlighting(IHighlightingDefinition? definition)
    {
        if (definition is null)
        {
            return;
        }

        lock (HighlightLock)
        {
            if (!OriginalBrushes.TryGetValue(definition, out var originals))
            {
                originals = new Dictionary<string, HighlightingBrush?>(StringComparer.OrdinalIgnoreCase);
                foreach (var named in definition.NamedHighlightingColors)
                {
                    if (named.Name is { } name && DarkColorMap.ContainsKey(name))
                    {
                        originals[name] = named.Foreground;
                    }
                }

                OriginalBrushes[definition] = originals;
            }

            foreach (var named in definition.NamedHighlightingColors)
            {
                if (named.Name is { } name && DarkColorMap.TryGetValue(name, out var color))
                {
                    named.Foreground = new SimpleHighlightingBrush(color);
                }
            }
        }
    }

    /// <summary>亮色主题：还原高亮定义的原始命名色（从未暗色化过则无操作）。</summary>
    private void RestoreHighlighting(IHighlightingDefinition? definition)
    {
        if (definition is null)
        {
            return;
        }

        lock (HighlightLock)
        {
            if (!OriginalBrushes.TryGetValue(definition, out var originals))
            {
                return;
            }

            foreach (var named in definition.NamedHighlightingColors)
            {
                if (named.Name is { } name && originals.TryGetValue(name, out var brush))
                {
                    named.Foreground = brush;
                }
            }
        }
    }

    /// <summary>Win11 圆角窗口（DWMWA_WINDOW_CORNER_PREFERENCE = 33, ROUND = 2；旧系统失败忽略）。</summary>
    private void ApplyWindowCorner()
    {
        try
        {
            var round = 2;
            _ = DwmSetWindowAttribute(new WindowInteropHelper(this).Handle, 33, ref round, sizeof(int));
        }
        catch
        {
            // 旧系统不支持圆角, 保持直角。
        }
    }

    /// <summary>
    /// 读取文件并探测写回编码（与主应用内置编辑器同款逻辑，两框架边界各自维护）：
    /// 有 BOM 按 BOM（UTF-8 / UTF-16 LE/BE）；无 BOM 先按严格 UTF-8 试解码，
    /// 失败则按系统 ANSI（中文环境为 GBK）。
    /// </summary>
    private static (string Text, Encoding Encoding) ReadFile(string path)
    {
        var bytes = File.ReadAllBytes(path);
        if (bytes.Length >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF)
        {
            return (new UTF8Encoding(false).GetString(bytes, 3, bytes.Length - 3), new UTF8Encoding(true));
        }

        if (bytes.Length >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE)
        {
            return (Encoding.Unicode.GetString(bytes, 2, bytes.Length - 2), Encoding.Unicode);
        }

        if (bytes.Length >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF)
        {
            return (Encoding.BigEndianUnicode.GetString(bytes, 2, bytes.Length - 2), Encoding.BigEndianUnicode);
        }

        try
        {
            var strict = new UTF8Encoding(false, throwOnInvalidBytes: true);
            return (strict.GetString(bytes), new UTF8Encoding(false));
        }
        catch (DecoderFallbackException)
        {
            var ansi = SystemAnsiEncoding();
            return (ansi.GetString(bytes), ansi);
        }
    }

    /// <summary>系统默认 ANSI 编码（中文环境为 GBK），不可用时回退 Latin-1。</summary>
    private static Encoding SystemAnsiEncoding()
    {
        try
        {
            return Encoding.GetEncoding(0);
        }
        catch
        {
            return Encoding.Latin1;
        }
    }

    [DllImport("dwmapi.dll")]
    private static extern int DwmSetWindowAttribute(IntPtr hwnd, int attribute, ref int value, int size);
}
