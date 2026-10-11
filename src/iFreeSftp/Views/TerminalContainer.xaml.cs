using System.Collections.ObjectModel;
using System.Linq;
using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace iFreeSftp.Views;

/// <summary>
/// 终端容器：主机命令窗，位于右文件夹容器下方。
/// 暗绿终端风格，无标题栏；每条日志独立段落（时间戳固定列 + 正文自动换行，
/// 续行对齐时间戳之后）；日志来自 TerminalLogService，忙状态由 BusyChanged 驱动。
/// </summary>
public sealed partial class TerminalContainer : UserControl
{
    /// <summary>日志条数上限：超出后丢弃最早的条目，防止长会话内存/渲染膨胀。</summary>
    private const int MaxLines = 400;

    /// <summary>日志集合（x:Bind 数据源；仅增删条目, 属性本身不更新）。</summary>
    private ObservableCollection<TerminalLogEntry> Lines { get; } = new();

    public TerminalContainer()
    {
        InitializeComponent();
        TerminalLogService.LineAppended += OnLineAppended;
        TerminalLogService.BusyChanged += OnBusyChanged;

        // 右键菜单样式照搬主框架文件夹右键菜单 (README 3.9 第 7 条), 主题切换后重设底色
        TerminalMenuFlyout.MenuFlyoutPresenterStyle = PaneMenuPresenterStyle();
        ActualThemeChanged += (_, _) => TerminalMenuFlyout.MenuFlyoutPresenterStyle = PaneMenuPresenterStyle();
        LocalizationService.CultureChanged += ApplyResources;
        Unloaded += (_, _) => LocalizationService.CultureChanged -= ApplyResources;
        ApplyResources();
    }

    /// <summary>追加日志条目（发布方可能在后台线程，统一调度到 UI 线程）。</summary>
    private void OnLineAppended(TerminalLogEntry entry)
    {
        DispatcherQueue.TryEnqueue(() =>
        {
            Lines.Add(entry);
            while (Lines.Count > MaxLines)
            {
                Lines.RemoveAt(0);
            }

            // 自动滚动到底。
            LogScroll.UpdateLayout();
            LogScroll.ChangeView(null, LogScroll.ScrollableHeight, null);
        });
    }

    private void OnBusyChanged(bool busy)
        => DispatcherQueue.TryEnqueue(() => BusyRing.IsActive = busy);

    /// <summary>右键菜单 presenter 样式: 仅背景按主题显式指定 (Popup 层资源查找不经过本控件,
    /// 主题字典覆盖不可靠), 其余走系统默认。亮 = #FFFFFF, 暗 = #26303D。</summary>
    private Style PaneMenuPresenterStyle()
    {
        var bg = ActualTheme == ElementTheme.Light
            ? Windows.UI.Color.FromArgb(255, 0xFF, 0xFF, 0xFF)
            : Windows.UI.Color.FromArgb(255, 0x26, 0x30, 0x3D);
        var style = new Style(typeof(MenuFlyoutPresenter));
        style.Setters.Add(new Setter(MenuFlyoutPresenter.BackgroundProperty, new Microsoft.UI.Xaml.Media.SolidColorBrush(bg)));
        return style;
    }

    /// <summary>清除全部日志。</summary>
    private void ClearAll_Click(object sender, RoutedEventArgs e) => Lines.Clear();

    /// <summary>复制全部日志到粘贴板 (每行 = 时间戳 + 正文); 无内容时不动作。</summary>
    private void CopyAll_Click(object sender, RoutedEventArgs e)
    {
        if (Lines.Count == 0)
        {
            return;
        }

        var text = string.Join(Environment.NewLine, Lines.Select(l => $"{l.Time}  {l.Text}"));
        var package = new Windows.ApplicationModel.DataTransfer.DataPackage();
        package.SetText(text);
        Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(package);
    }

    private void ApplyResources()
    {
        ClearAllItem.Text = LocalizationService.GetString("TerminalClearAll");
        CopyAllItem.Text = LocalizationService.GetString("TerminalCopyAll");
    }
}
