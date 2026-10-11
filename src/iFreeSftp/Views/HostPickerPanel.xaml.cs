using System;
using System.Linq;
using iFreeSftp.Models;
using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// "打开内容"选择器 (模态): 遮罩 + 居中卡片。"本地"区单行本地文件系统,
/// "主机"区逐行列出存档主机。由 tab 栏 + 按钮与空状态"选择主机"按钮触发
/// (携带来源窗格); 点击行即选择并关闭, 落位由 Header 编排:
/// 本地 → FolderContainer.OpenLocalView, 主机 → HostPanel.ConnectToHostAsync。
/// </summary>
public sealed partial class HostPickerPanel : UserControl
{
    public sealed record LocalPick(string Title, string Hint);

    public sealed record HostPick(HostProfile Host, string Title, string Detail);

    /// <summary>选择"本地文件系统"。参数为来源窗格 (Left/Right)。</summary>
    public event Action<string>? LocalPicked;

    /// <summary>选择某主机直接连接。参数为 (来源窗格, 主机档案)。</summary>
    public event Action<string, HostProfile>? HostPicked;

    /// <summary>请求关闭 (遮罩/ESC), Header 收到后隐藏覆盖层。</summary>
    public event Action? CloseRequested;

    private string _pane = "Right";
    private KeyboardAccelerator? _escAccelerator;
    // 卡片投影: ThemeShadow 不能投到 caster 的祖先上, 用兄弟节点遮罩作接收者,
    // Z 平移 32px 产生柔和下坠阴影。
    private readonly Microsoft.UI.Xaml.Media.ThemeShadow _shadow = new();

    public HostPickerPanel()
    {
        InitializeComponent();
        _shadow.Receivers.Add(Mask);
        Card.Shadow = _shadow;
        Card.Translation = new System.Numerics.Vector3(0, 0, 32);
        LocalizationService.CultureChanged += ApplyResources;
        Unloaded += (_, _) => LocalizationService.CultureChanged -= ApplyResources;
    }

    /// <summary>打开选择器: 记录来源窗格并填充列表。拖拽偏移重置, 卡片回到居中位置。</summary>
    public void OpenFor(string pane)
    {
        _pane = pane;
        DragTransform.X = 0;
        DragTransform.Y = 0;
        LocalList.ItemsSource = new[] { new LocalPick(
            LocalizationService.GetString("ChooserLocalTitle"),
            LocalizationService.GetString("ChooserBrowseLocal")) };
        HostRows.ItemsSource = HostStoreService.Hosts
            .Select(h => new HostPick(h, h.Name,
                string.IsNullOrEmpty(h.Username) ? h.Host : $"{h.Username}@{h.Host}"))
            .ToList();
        EmptyHint.Visibility = HostStoreService.Hosts.Count == 0
            ? Visibility.Visible
            : Visibility.Collapsed;
        ApplyResources();
        AttachEscape();
        Visibility = Visibility.Visible;
    }

    private void Close()
    {
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
            Close();
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

    private void Mask_Tapped(object sender, TappedRoutedEventArgs e) => Close();

    private void CloseBtn_Click(object sender, RoutedEventArgs e) => Close();

    // —— 标题栏拖拽卡片: 指针捕获 + RenderTransform 平移 (打开时重置回居中) ——

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

    private void LocalList_ItemClick(object sender, ItemClickEventArgs e)
    {
        Close();
        LocalPicked?.Invoke(_pane);
    }

    private void HostRows_ItemClick(object sender, ItemClickEventArgs e)
    {
        Close();
        if ((e.ClickedItem as HostPick)?.Host is { } host)
        {
            HostPicked?.Invoke(_pane, host);
        }
    }

    /// <summary>重应用本地化文本 (打开时与语言切换时)。</summary>
    private void ApplyResources()
    {
        PickerTitle.Text = LocalizationService.GetString("ChooserTitle");
        LocalHeader.Text = LocalizationService.GetString("ChooserLocalSection");
        HostsHeader.Text = LocalizationService.GetString("ChooserHostsSection");
        EmptyHint.Text = LocalizationService.GetString("EmptyHostList");
    }
}
