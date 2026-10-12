using System;
using System.Numerics;
using System.Threading.Tasks;
using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// 删除确认弹窗 (模态): 样式与 PermissionDialog/ChecksumDialog 同构
/// (README 3.9 弹框统一原则 + 小弹框字号口径)。单项显示该名称, 多项显示
/// "首名 + 等 N 项", 不罗列全部文件名。宿主经 <see cref="ShowAsync"/> 异步
/// 等待结果; ESC/遮罩/关闭钮均为取消。
/// </summary>
public sealed partial class DeleteConfirmDialog : UserControl
{
    // ThemeShadow: 遮罩作接收者, Z 平移 32 (PermissionDialog 同款)
    private readonly Microsoft.UI.Xaml.Media.ThemeShadow _shadow = new();

    private TaskCompletionSource<bool>? _tcs;
    private KeyboardAccelerator? _escAccelerator;
    private bool _toRecycleBin; // 本次删除是否进回收站 (语言切换重刷文案用)

    public DeleteConfirmDialog()
    {
        InitializeComponent();
        _shadow.Receivers.Add(Mask);
        Card.Shadow = _shadow;
        Card.Translation = new Vector3(0, 0, 32);
        ApplyResources();
        // 语言切换订阅与宿主生命周期配对: Loaded 订阅 / Unloaded 退订 (WinUI 布局
        // 重排会 Unloaded→Loaded 重放, 订阅随之恢复; 宿主销毁后保持退订防泄漏)。
        // Loaded 先退订再订阅, 防重放时重复累加订阅计数。
        Loaded += (_, _) =>
        {
            LocalizationService.CultureChanged -= ApplyResources;
            LocalizationService.CultureChanged += ApplyResources;
        };
        Unloaded += (_, _) => LocalizationService.CultureChanged -= ApplyResources;
    }

    /// <summary>打开弹窗并等待结果: true = 确认删除, false = 取消。
    /// <paramref name="toRecycleBin"/> = 本地删除进回收站 (文案提示可还原);
    /// false = 远程永久删除 (文案提示无法恢复)。</summary>
    public Task<bool> ShowAsync(int count, string firstName, bool toRecycleBin = false)
    {
        // 终结上一轮未 Close 的等待 (防御: 上次弹窗未关就再次 ShowAsync,
        // 旧 Task 若不终结则永远悬挂)
        _tcs?.TrySetResult(false);
        _tcs = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        _toRecycleBin = toRecycleBin;
        DragTransform.X = 0; // 打开时重置拖拽偏移 (居中)
        DragTransform.Y = 0;
        // 名称行: 多项 = "首名 等 N 项", 避免罗列一长串文件名
        EntryName.Text = count > 1
            ? firstName + " " + string.Format(LocalizationService.GetString("DeleteConfirmMore"), count)
            : firstName;
        BodyText.Text = LocalizationService.GetString(
            _toRecycleBin ? "DeleteConfirmBodyRecycle" : "DeleteConfirmBody");
        AttachEscape();
        Visibility = Visibility.Visible;
        return _tcs.Task;
    }

    private void Close(bool confirmed)
    {
        DetachEscape();
        Visibility = Visibility.Collapsed;
        _tcs?.TrySetResult(confirmed);
        _tcs = null;
    }

    private void DeleteBtn_Click(object sender, RoutedEventArgs e) => Close(true);

    private void CancelBtn_Click(object sender, RoutedEventArgs e) => Close(false);

    private void Mask_Tapped(object sender, TappedRoutedEventArgs e) => Close(false);

    private void CloseBtn_Click(object sender, RoutedEventArgs e) => Close(false);

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
            Close(false);
        };
        // 面板隐藏时移除, 避免隐藏元素的快捷键行为不确定 (HostPickerPanel 同款)
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

    // —— 标题栏拖拽卡片: 指针捕获 + RenderTransform 平移 (ChecksumDialog 同款) ——

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

    /// <summary>重应用本地化文本 (打开时与语言切换时)。</summary>
    private void ApplyResources()
    {
        TitleText.Text = LocalizationService.GetString("MenuDelete");
        BodyText.Text = LocalizationService.GetString("DeleteConfirmBody");
        DeleteBtn.Content = LocalizationService.GetString("MenuDelete");
        CancelBtn.Content = LocalizationService.GetString("Cancel");
    }
}
