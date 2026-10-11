using System;
using System.Numerics;
using iFreeSftp.Models;
using iFreeSftp.Services;
using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// 远程文件校验码弹窗 (模态): 样式与 PermissionDialog 同构 (README 3.9 + 小弹框字号口径)。
/// 算法下拉 SHA-256/SHA-512/SM3 选中即触发流式计算 (SFTP 拉流边读边算, 服务层经
/// _sshLock 串行); 大文件计算期间独占 SSH 通道, 关闭弹窗/切换算法时经 CancelChecksum()
/// 在分块间隙尽快中断。复制按钮回弹"已复制"1.5 秒。
/// 本地化文本由宿主 FileView 在打开与语言切换时调 ApplyResources 刷新。
/// </summary>
public sealed partial class ChecksumDialog : UserControl
{
    private FileItemViewModel? _item;
    private SftpSessionService? _sftp;
    private KeyboardAccelerator? _escAccelerator;
    // 计算请求代际号: 切换算法/关闭弹窗后, 在途结果凭 id 作废
    private int _runId;
    // OpenFor 显式启动默认算法时抑制 ComboBox 事件 (避免赋值连带重复计算)
    private bool _suppressAlgoEvent;
    // 成功算出的码值 (非计算中/错误占位), 复制按钮唯一合法来源
    private string? _currentHex;
    private string _computingText = string.Empty;
    private string _copyText = string.Empty;
    private string _copiedText = string.Empty;
    private DispatcherQueueTimer? _copyFeedbackTimer;
    // ThemeShadow: 遮罩作接收者, Z 平移 32 (PermissionDialog 同款)
    private readonly Microsoft.UI.Xaml.Media.ThemeShadow _shadow = new();

    public ChecksumDialog()
    {
        InitializeComponent();
        _shadow.Receivers.Add(Mask);
        Card.Shadow = _shadow;
        Card.Translation = new Vector3(0, 0, 32);
        ApplyResources();
    }

    /// <summary>打开弹窗: 按服务端探测结果禁用不支持的算法项, 默认选第一个可用项立即计算。</summary>
    public void OpenFor(FileItemViewModel item, SftpSessionService sftp)
    {
        _item = item;
        _sftp = sftp;
        DragTransform.X = 0;
        DragTransform.Y = 0;
        EntryIcon.Glyph = item.IconGlyph;
        EntryName.Text = item.Name;
        ApplyResources();
        AttachEscape();
        Visibility = Visibility.Visible;
        // 算法过滤: 菜单入口已保证至少一项服务端可用 (ServerHashAny), 置灰不支持项
        AlgoSha256.IsEnabled = sftp.ServerSha256;
        AlgoSha512.IsEnabled = sftp.ServerSha512;
        AlgoSm3.IsEnabled = sftp.ServerSm3;
        int firstEnabled = sftp.ServerSha256 ? 0 : sftp.ServerSha512 ? 1 : 2;
        string[] algorithms = { "SHA256", "SHA512", "SM3" };
        // 显式启动 (抑制赋值连带的重复事件), 避免双份计算
        _suppressAlgoEvent = true;
        AlgoCombo.SelectedIndex = firstEnabled;
        _suppressAlgoEvent = false;
        StartCompute(algorithms[firstEnabled]);
    }

    private void Close()
    {
        DetachEscape();
        _runId++; // 在途结果作废
        _sftp?.CancelChecksum();
        _item = null;
        Visibility = Visibility.Collapsed;
    }

    /// <summary>启动一次计算: 显示"计算中", 旧计算 (若有) 经取消标记尽快让出 SSH 通道。</summary>
    private async void StartCompute(string algorithm)
    {
        if (_item is null || _sftp is null)
        {
            return;
        }

        int id = ++_runId;
        _sftp.CancelChecksum();
        _currentHex = null;
        CopyBtn.IsEnabled = false;
        ValueBox.Text = _computingText;
        try
        {
            string hex = await _sftp.ComputeChecksumAsync(_item.FullPath, algorithm);
            if (id != _runId)
            {
                return; // 已被新算法/关闭作废
            }

            _currentHex = hex;
            ValueBox.Text = hex;
            CopyBtn.IsEnabled = true;
        }
        catch (OperationCanceledException)
        {
            // 被切换算法/关闭取消: 静默 (新计算已在跑或弹窗已关)
        }
        catch (Exception ex)
        {
            if (id != _runId)
            {
                return;
            }

            ValueBox.Text = ex.Message; // 错误信息复用码值框展示 (等宽折行)
        }
    }

    private void AlgoCombo_SelectionChanged(object sender, SelectionChangedEventArgs e)
    {
        if (_suppressAlgoEvent)
        {
            return;
        }

        // "SHA-256" → "SHA256", "SM3" 原样; 服务层按字符串路由
        string name = (AlgoCombo.SelectedItem as ComboBoxItem)?.Content?.ToString() ?? "SHA-256";
        StartCompute(name.Replace("-", string.Empty));
    }

    private void CopyBtn_Click(object sender, RoutedEventArgs e)
    {
        if (_currentHex is null)
        {
            return;
        }

        var package = new Windows.ApplicationModel.DataTransfer.DataPackage();
        package.SetText(_currentHex);
        Windows.ApplicationModel.DataTransfer.Clipboard.SetContent(package);

        // 回弹反馈: 已复制 → 1.5s 后还原
        CopyBtn.Content = _copiedText;
        _copyFeedbackTimer?.Stop();
        _copyFeedbackTimer = DispatcherQueue.CreateTimer();
        _copyFeedbackTimer.Interval = TimeSpan.FromSeconds(1.5);
        _copyFeedbackTimer.Tick += (_, _) =>
        {
            CopyBtn.Content = _copyText;
            _copyFeedbackTimer?.Stop();
        };
        _copyFeedbackTimer.Start();
    }

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
        TitleText.Text = LocalizationService.GetString("MenuChecksum");
        AlgoLabel.Text = LocalizationService.GetString("ChkAlgorithm");
        _computingText = LocalizationService.GetString("ChkComputing");
        _copyText = LocalizationService.GetString("ChkCopy");
        _copiedText = LocalizationService.GetString("ChkCopied");
        CopyBtn.Content = _copyText;
        CloseActionBtn.Content = LocalizationService.GetString("Close");
    }
}
