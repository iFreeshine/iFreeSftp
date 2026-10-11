using Microsoft.UI;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Shapes;
using Windows.UI;

namespace iFreeSftp.Views;

/// <summary>
/// IDE 编辑器式多标签条复合控件：平贴式标签项（选中蓝下划线、关闭列定宽、
/// subtle 色系背景层次）+ 贯穿基线 + 条右端固定新建 (+) 按钮。纯 UI 封装,
/// 标签内容与业务由使用方通过 <see cref="AddTab"/> 与事件注入。
/// </summary>
public sealed partial class IdeTabStrip : UserControl
{
    /// <summary>点击 + 号（外置按钮或内置入口）请求新建标签。</summary>
    public event EventHandler? NewTabRequested;

    /// <summary>标签关闭请求。内部已把标签移出标签条, 使用方在此清理内容资源。</summary>
    public event Action<TabViewItem>? TabCloseRequested;

    // 标签画刷: 透明指示条 / 选中指示条 (强调蓝 #0078D4, 与 tooltip、主机按钮一致)。
    // 其余配色 (分隔线、常态、悬停、选中底色) 均为 XAML 主题资源, 随主题自动切换。
    private static readonly SolidColorBrush TransparentBrush = new(Colors.Transparent);
    private static readonly SolidColorBrush AccentBrush = new(Color.FromArgb(0xFF, 0x00, 0x78, 0xD4));

    /// <summary>蓝色标签图标字形 (GDI+ 渲染验证): 显示器=本地, 服务器=远程。</summary>
    public const string LocalIconGlyph = "\uE7F4";
    public const string RemoteIconGlyph = "\uE968";

    // 标签字号三参数: 图标 / 文字 / 关闭图标, 三个独立可调。
    private const double IconFontSize = 9;
    private const double TitleFontSize = 12;
    private const double CloseFontSize = 9;

    // 垂直光学对齐微调 (px, 负值上移): 通过 RenderTransform 实现, 不参与
    // 布局, 与左右间距 (Margin) 完全独立。
    private const double IconVerticalOffset = -1;
    private const double TitleVerticalOffset = -1;

    public IdeTabStrip() => InitializeComponent();

    /// <summary>
    /// 显示/隐藏标签条头部（标签行 + 基线 + 新建按钮）。右窗口空状态引导时
    /// 隐藏头部让引导页独占窗格, 首次连接后恢复。TabView 模板的头部区是名为
    /// TabContainerGrid 的元素, 模板名称域隔离, 用视觉树递归查找后直接折叠,
    /// 内容区行(*)自动占满。
    /// </summary>
    public void SetHeaderVisible(bool visible)
    {
        var v = visible ? Visibility.Visible : Visibility.Collapsed;
        TabStripBaseline.Visibility = v;
        NewTabBtn.Visibility = v;
        var head = FindNamedDescendant(TabsPart, "TabContainerGrid");
        if (head is not null)
        {
            head.Visibility = v;
        }
    }

    /// <summary>视觉树递归按名查找（模板内元素 FindName 不可用）。</summary>
    private static FrameworkElement? FindNamedDescendant(DependencyObject root, string name)
    {
        if (root is FrameworkElement fe && fe.Name == name)
        {
            return fe;
        }

        var count = Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChildrenCount(root);
        for (var i = 0; i < count; i++)
        {
            var hit = FindNamedDescendant(Microsoft.UI.Xaml.Media.VisualTreeHelper.GetChild(root, i), name);
            if (hit is not null)
            {
                return hit;
            }
        }

        return null;
    }

    /// <summary>内部 TabView, 供高级场景 (遍历标签、拖拽配置等) 使用。</summary>
    public TabView Tabs => TabsPart;

    /// <summary>当前选中标签的内容。</summary>
    public object? SelectedContent => (TabsPart.SelectedItem as TabViewItem)?.Content;

    /// <summary>
    /// 新建标签页: 标签头为 IDE 编辑器式 (蓝色模式图标 + 标题, 垂直光学
    /// 对齐), 选中时背景提亮且底部显示贯穿整宽的蓝色下划线 (模板绘制)。
    /// </summary>
    /// <returns>新建的 <see cref="TabViewItem"/>, 便于使用方跟踪内容状态。</returns>
    public TabViewItem AddTab(string title, string iconGlyph, object content)
    {
        // 标签头: 图标列 + 标题列 (必须定义列, 否则同格重叠)。选中下划线在
        // 模板层 (SelectionIndicator), 因此标签头自身无需再放指示条。
        var icon = new FontIcon
        {
            Glyph = iconGlyph,
            FontSize = IconFontSize,
            Foreground = AccentBrush,
            FontFamily = new FontFamily("Segoe Fluent Icons"),
            VerticalAlignment = VerticalAlignment.Center,
            Margin = new Thickness(8, 0, 0, 0), // 仅水平: 图标距标签左缘
            RenderTransform = new TranslateTransform { Y = IconVerticalOffset }, // 垂直微调, 不影响布局
        };
        var titleBlock = new TextBlock
        {
            Text = title,
            VerticalAlignment = VerticalAlignment.Center,
            FontSize = TitleFontSize,
            Margin = new Thickness(8, 0, 0, 0), // 仅水平: 文字与图标的间距
            RenderTransform = new TranslateTransform { Y = TitleVerticalOffset }, // 垂直微调, 不影响布局
        };
        var header = new Grid
        {
            ColumnDefinitions =
            {
                new ColumnDefinition { Width = GridLength.Auto },
                new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) },
            },
            Children = { icon, titleBlock },
        };
        Grid.SetColumn(icon, 0);
        Grid.SetColumn(titleBlock, 1);
        header.Tag = icon; // 供 SetTabIcon 更新字形

        var tab = new TabViewItem
        {
            Header = header,
            Content = content,
            Tag = titleBlock, // 供 SetTabTitle 更新标题
        };

        // 模板在元素入树后才应用: Loaded 时解析模板层下划线与关闭按钮,
        // 设定关闭图标字号与初始下划线颜色, 并订阅后续选中态变化。
        tab.Loaded += (s, _) =>
        {
            var t = (TabViewItem)s;
            if (FindDescendant<Button>(t, "CloseButton") is { } close)
            {
                close.FontSize = CloseFontSize;
            }
            if (FindDescendant<Rectangle>(t, "SelectionIndicator") is { } indicator)
            {
                indicator.Fill = t.IsSelected ? AccentBrush : TransparentBrush;
                t.RegisterPropertyChangedCallback(
                    TabViewItem.IsSelectedProperty,
                    (sender, _) => indicator.Fill =
                        ((TabViewItem)sender).IsSelected ? AccentBrush : TransparentBrush);
            }
        };

        TabsPart.TabItems.Add(tab);
        TabsPart.SelectedItem = tab;
        return tab;
    }

    /// <summary>更新标签标题。</summary>
    public void SetTabTitle(TabViewItem tab, string title)
    {
        if (tab.Tag is TextBlock titleBlock)
        {
            titleBlock.Text = title;
        }
    }

    /// <summary>更新标签图标字形。</summary>
    public void SetTabIcon(TabViewItem tab, string glyph)
    {
        if (tab.Header is Grid header && header.Tag is FontIcon icon)
        {
            icon.Glyph = glyph;
        }
    }

    private void NewTabBtn_Click(object sender, RoutedEventArgs e)
        => NewTabRequested?.Invoke(this, EventArgs.Empty);

    private void Tabs_AddTabButtonClick(TabView sender, object args)
        => NewTabRequested?.Invoke(this, EventArgs.Empty);

    private void Tabs_TabCloseRequested(TabView sender, TabViewTabCloseRequestedEventArgs args)
    {
        if (args.Item is TabViewItem tab && sender.TabItems.Remove(tab))
        {
            TabCloseRequested?.Invoke(tab);
        }
    }

    private void Tabs_TabTearOutRequested(TabView sender, TabViewTabTearOutRequestedEventArgs args)
    {
        // 占位：后续可实现拖出标签成为独立窗口。
    }

    /// <summary>按名称在可视化树中查找指定类型的后代元素 (模板部件解析)。</summary>
    private static T? FindDescendant<T>(DependencyObject root, string name)
        where T : FrameworkElement
    {
        if (root is FrameworkElement fe && fe.Name == name && fe is T typed)
        {
            return typed;
        }
        int count = VisualTreeHelper.GetChildrenCount(root);
        for (int i = 0; i < count; i++)
        {
            var found = FindDescendant<T>(VisualTreeHelper.GetChild(root, i), name);
            if (found != null)
            {
                return found;
            }
        }
        return null;
    }
}
