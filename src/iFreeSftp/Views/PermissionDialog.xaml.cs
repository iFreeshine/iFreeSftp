using System;
using System.Text;
using iFreeSftp.Models;
using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// 远程文件/目录权限编辑弹窗 (初始版, 模态): 遮罩 + 居中卡片。
/// 九宫格复选框编辑 属主/属组/其他 × 读/写/执行, 底部实时显示八进制与符号值;
/// "应用"经 ApplyRequested 回调交由 FileView 走 SFTP chmod 并刷新列表。
/// 本地化文本由宿主 FileView 在打开与语言切换时调 ApplyResources 刷新。
/// </summary>
public sealed partial class PermissionDialog : UserControl
{
    /// <summary>点击"应用": 参数 = (目标条目, 九宫格整理出的 9 位权限 0o000~0o777, 不含特殊位)。</summary>
    public event Action<FileItemViewModel, int>? ApplyRequested;

    private FileItemViewModel? _item;
    private KeyboardAccelerator? _escAccelerator;
    // OpenFor 批量赋勾选态时不逐次重算数值文本 (打开末尾统一算一次)。
    private bool _suppressValueChanged;
    // 卡片投影: ThemeShadow 不能投到 caster 的祖先上, 用兄弟节点遮罩作接收者,
    // Z 平移 32px 产生柔和下坠阴影 (HostPickerPanel 同款)。
    private readonly Microsoft.UI.Xaml.Media.ThemeShadow _shadow = new();

    public PermissionDialog()
    {
        InitializeComponent();
        _shadow.Receivers.Add(Mask);
        Card.Shadow = _shadow;
        Card.Translation = new System.Numerics.Vector3(0, 0, 32);
        ApplyResources();
    }

    /// <summary>打开弹窗: 记录条目, 按当前权限 mode (12 位 Unix 位) 勾选九宫格。
    /// 拖拽偏移重置, 卡片回到居中位置。</summary>
    public void OpenFor(FileItemViewModel item, int mode)
    {
        _item = item;
        DragTransform.X = 0;
        DragTransform.Y = 0;
        EntryIcon.Glyph = item.IconGlyph;
        EntryName.Text = item.Name;
        _suppressValueChanged = true;
        OwnerReadCb.IsChecked = (mode & 0x100) != 0;
        OwnerWriteCb.IsChecked = (mode & 0x080) != 0;
        OwnerExecCb.IsChecked = (mode & 0x040) != 0;
        GroupReadCb.IsChecked = (mode & 0x020) != 0;
        GroupWriteCb.IsChecked = (mode & 0x010) != 0;
        GroupExecCb.IsChecked = (mode & 0x008) != 0;
        OthersReadCb.IsChecked = (mode & 0x004) != 0;
        OthersWriteCb.IsChecked = (mode & 0x002) != 0;
        OthersExecCb.IsChecked = (mode & 0x001) != 0;
        _suppressValueChanged = false;
        UpdateValueText();
        ApplyResources();
        AttachEscape();
        Visibility = Visibility.Visible;
    }

    private void Close()
    {
        DetachEscape();
        _item = null;
        Visibility = Visibility.Collapsed;
    }

    /// <summary>收集九宫格勾选态为 9 位权限值 (属主 0o400 ~ 其他 0o001)。</summary>
    private int CollectMode()
        => (OwnerReadCb.IsChecked == true ? 0x100 : 0)
           | (OwnerWriteCb.IsChecked == true ? 0x080 : 0)
           | (OwnerExecCb.IsChecked == true ? 0x040 : 0)
           | (GroupReadCb.IsChecked == true ? 0x020 : 0)
           | (GroupWriteCb.IsChecked == true ? 0x010 : 0)
           | (GroupExecCb.IsChecked == true ? 0x008 : 0)
           | (OthersReadCb.IsChecked == true ? 0x004 : 0)
           | (OthersWriteCb.IsChecked == true ? 0x002 : 0)
           | (OthersExecCb.IsChecked == true ? 0x001 : 0);

    /// <summary>9 位权限 → "rwxr-xr-x" 符号串 (rwx 顺序, 无位补 '-')。</summary>
    private static string Symbolic(int mode)
    {
        var sb = new StringBuilder(9);
        (int bit, char c)[] specs =
        {
            (0x100, 'r'), (0x080, 'w'), (0x040, 'x'),
            (0x020, 'r'), (0x010, 'w'), (0x008, 'x'),
            (0x004, 'r'), (0x002, 'w'), (0x001, 'x'),
        };
        foreach (var (bit, c) in specs)
        {
            sb.Append((mode & bit) != 0 ? c : '-');
        }

        return sb.ToString();
    }

    private void UpdateValueText()
    {
        int mode = CollectMode();
        ValueText.Text = $"{Convert.ToString(mode, 8).PadLeft(3, '0')} ({Symbolic(mode)})";
    }

    private void PermBox_ValueChanged(object sender, RoutedEventArgs e)
    {
        if (!_suppressValueChanged)
        {
            UpdateValueText();
        }
    }

    private void ApplyBtn_Click(object sender, RoutedEventArgs e)
    {
        if (_item is { } item)
        {
            ApplyRequested?.Invoke(item, CollectMode());
        }

        Close();
    }

    private void CancelBtn_Click(object sender, RoutedEventArgs e) => Close();

    private void Mask_Tapped(object sender, TappedRoutedEventArgs e) => Close();

    private void CloseBtn_Click(object sender, RoutedEventArgs e) => Close();

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
        // 代码挂接: 面板隐藏时移除, 避免隐藏元素的快捷键行为不确定 (HostPickerPanel 同款)。
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

    /// <summary>重应用本地化文本 (打开时与语言切换时由宿主调用)。</summary>
    public void ApplyResources()
    {
        TitleText.Text = LocalizationService.GetString("MenuPermissions");
        ColRead.Text = LocalizationService.GetString("PermRead");
        ColWrite.Text = LocalizationService.GetString("PermWrite");
        ColExec.Text = LocalizationService.GetString("PermExecute");
        OwnerLabel.Text = LocalizationService.GetString("PermOwner");
        GroupLabel.Text = LocalizationService.GetString("PermGroup");
        OthersLabel.Text = LocalizationService.GetString("PermOthers");
        ApplyBtn.Content = LocalizationService.GetString("PermApply");
        CancelBtn.Content = LocalizationService.GetString("Cancel");
    }
}
