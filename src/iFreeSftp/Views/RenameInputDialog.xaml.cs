using System;
using System.Numerics;
using System.Threading.Tasks;
using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// 重命名输入弹窗 (模态): 样式与 DeleteConfirmDialog/PermissionDialog/ChecksumDialog
/// 同构 (README 3.9 弹框统一原则 + 小弹框字号口径)。本地与远程共用, 宿主经
/// <see cref="ShowAsync"/> 异步等待结果; 校验通过且名字有改动时确认钮可用,
/// 回车即确认; ESC/遮罩/关闭钮/取消均为放弃 (返回 null)。
/// </summary>
public sealed partial class RenameInputDialog : UserControl
{
    // ThemeShadow: 遮罩作接收者, Z 平移 32 (DeleteConfirmDialog 同款)
    private readonly Microsoft.UI.Xaml.Media.ThemeShadow _shadow = new();

    private TaskCompletionSource<string?>? _tcs;
    private KeyboardAccelerator? _escAccelerator;
    private string _oldName = string.Empty;
    private bool _isRemote;
    private Func<string, bool> _isTaken = _ => false;

    public RenameInputDialog()
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

    /// <summary>打开弹窗并等待结果: 返回新名 (已 trim), 取消返回 null。
    /// isTaken = 目标名是否已占用 (本地 EntryExists / 远程列表比对)。
    /// isRemote = 远程条目: 不做 Windows 非法字符限制 (Linux 允许 : \ 等,
    /// 冲突由服务器校验兜底)。</summary>
    public Task<string?> ShowAsync(string oldName, bool isDirectory, Func<string, bool> isTaken, bool isRemote = false)
    {
        // 终结上一轮未 Close 的等待 (防御: 上次弹窗未关就再次 ShowAsync,
        // 旧 Task 若不终结则永远悬挂)
        _tcs?.TrySetResult(null);
        _tcs = new TaskCompletionSource<string?>(TaskCreationOptions.RunContinuationsAsynchronously);
        _oldName = oldName;
        _isTaken = isTaken;
        _isRemote = isRemote;
        DragTransform.X = 0; // 打开时重置拖拽偏移 (居中)
        DragTransform.Y = 0;
        NameBox.Text = oldName;
        ConfirmBtn.IsEnabled = false; // 初始为旧名, 需改动才可确认
        Visibility = Visibility.Visible;
        // 资源管理器习惯: 打开时选中主名 (目录全名 / 文件去扩展名部分), 直接键入即覆盖。
        // 焦点须在 Visible 之后申请, Collapsed 状态下设置无效
        var stemLength = isDirectory ? oldName.Length : Path.GetFileNameWithoutExtension(oldName).Length;
        NameBox.Focus(FocusState.Programmatic);
        NameBox.Select(0, Math.Min(stemLength, oldName.Length));
        AttachEscape();
        return _tcs.Task;
    }

    private void Close(string? newName)
    {
        DetachEscape();
        Visibility = Visibility.Collapsed;
        _tcs?.TrySetResult(newName);
        _tcs = null;
    }

    private void ConfirmBtn_Click(object sender, RoutedEventArgs e) => Close(NameBox.Text.Trim());

    private void CancelBtn_Click(object sender, RoutedEventArgs e) => Close(null);

    private void Mask_Tapped(object sender, TappedRoutedEventArgs e) => Close(null);

    private void CloseBtn_Click(object sender, RoutedEventArgs e) => Close(null);

    // 回车即确认 (校验未通过时无视)
    private void NameBox_KeyDown(object sender, KeyRoutedEventArgs e)
    {
        if (e.Key == Windows.System.VirtualKey.Enter && ConfirmBtn.IsEnabled)
        {
            Close(NameBox.Text.Trim());
        }
    }

    private void NameBox_TextChanged(object sender, TextChangedEventArgs e)
        => ConfirmBtn.IsEnabled = IsValidNewName();

    /// <summary>校验: 非空 / (本地)无 Windows 非法字符 / 与旧名不同 (忽略大小写) / 未占用。</summary>
    private bool IsValidNewName()
    {
        var name = NameBox.Text.Trim();
        return name.Length > 0
            && (_isRemote || name.IndexOfAny(System.IO.Path.GetInvalidFileNameChars()) < 0)
            && !string.Equals(name, _oldName, StringComparison.OrdinalIgnoreCase)
            && !_isTaken(name);
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
            Close(null);
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

    // —— 标题栏拖拽卡片: 指针捕获 + RenderTransform 平移 (DeleteConfirmDialog 同款) ——

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
        TitleText.Text = LocalizationService.GetString("MenuRename");
        ConfirmBtn.Content = LocalizationService.GetString("MenuRename");
        CancelBtn.Content = LocalizationService.GetString("Cancel");
    }
}
