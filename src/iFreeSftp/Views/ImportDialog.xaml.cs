using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Linq;
using iFreeSftp.Models;
using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// 预览表行视图模型 (顶层类便于 x:DataType 引用):
/// IsChecked 用 bool? 规避 CheckBox.IsChecked(x:Bind TwoWay) 的 bool?→bool 编译错误,
/// setter 内归一化为 bool。Session 只含连接元数据, 不含任何口令字段。
/// </summary>
internal sealed class ImportRow : INotifyPropertyChanged
{
    public event PropertyChangedEventHandler? PropertyChanged;

    public ImportRow(ImportedSession session, bool inStore, bool dupInBatch)
    {
        Session = session;
        InStore = inStore;
        DupInBatch = dupInBatch;
    }

    public ImportedSession Session { get; }

    /// <summary>与现有存档 DedupKey 重复 (不含批内重复)。</summary>
    public bool InStore { get; }

    /// <summary>本次扫描批内重复 (N3: 同一主机多来源只导入第一条)。</summary>
    public bool DupInBatch { get; }

    /// <summary>DedupKey 重复 (库内或批内) (默认不勾选, 也不参与导入计数)。</summary>
    public bool Exists => InStore || DupInBatch;

    private bool? _isChecked;
    public bool? IsChecked
    {
        get => _isChecked;
        set
        {
            var v = value == true;
            if (_isChecked == v)
            {
                return;
            }

            _isChecked = v;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(IsChecked)));
        }
    }

    // 一次性绑定字段 (扫描结果只出现一次)
    public string Name => Session.Name;
    public string HostText => Session.Host;
    public string PortText => Session.Port.ToString();
    public string UsernameText => Session.Username;

    // 动态文案 (OneWay + INPC, 语言切换时由 ApplyResources 重赋值)
    private string _authText = string.Empty;
    public string AuthText
    {
        get => _authText;
        set
        {
            if (_authText == value)
            {
                return;
            }

            _authText = value;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(AuthText)));
        }
    }

    private string _statusText = string.Empty;
    public string StatusText
    {
        get => _statusText;
        set
        {
            if (_statusText == value)
            {
                return;
            }

            _statusText = value;
            PropertyChanged?.Invoke(this, new PropertyChangedEventArgs(nameof(StatusText)));
        }
    }
}

/// <summary>
/// 会话导入向导弹窗 (模态): 同意页 → 预览表(判重) → 结果页 三态。
/// 扫描走 HostImportParser (三条红线: 口令不导、命令字段不导、只导连接元数据),
/// 勾选条目经 ImportRequested 回调交由宿主 HostPanel 写回 HostStoreService;
/// 弹窗自身不碰存储。Close 即丢弃扫描结果 (隐私最小化)。
/// 本地化文本由宿主在打开与语言切换时调 ApplyResources 刷新。
/// </summary>
public sealed partial class ImportDialog : UserControl
{
    /// <summary>点击"导入": 参数 = 用户勾选且非重复的会话列表 (不含任何口令字段)。</summary>
    public event Action<List<ImportedSession>>? ImportRequested;

    private enum DialogState { Consent, Preview, Result }

    private HashSet<string> _existingKeys = new(StringComparer.Ordinal);
    private List<ImportRow> _rows = new();
    private ImportScanResult? _lastResult;
    private DialogState _state = DialogState.Consent;
    private int _importedCount;
    private bool _hasImported;
    private KeyboardAccelerator? _escAccelerator;
    // 卡片投影: ThemeShadow 不能投到 caster 的祖先上, 用兄弟节点遮罩作接收者,
    // Z 平移 32px 产生柔和下坠阴影 (PermissionDialog 同款)。
    private readonly Microsoft.UI.Xaml.Media.ThemeShadow _shadow = new();

    public ImportDialog()
    {
        InitializeComponent();
        _shadow.Receivers.Add(Mask);
        Card.Shadow = _shadow;
        Card.Translation = new System.Numerics.Vector3(0, 0, 32);
        ApplyResources();
    }

    /// <summary>打开弹窗: 记录现有会话判重键, 重置到同意页 (每次打开重新取得显式同意)。</summary>
    public void OpenFor(IReadOnlyList<HostProfile> existingHosts)
    {
        _existingKeys = new HashSet<string>(
            existingHosts.Select(h => HostImportParser.DedupKey(h.Host, h.Port, h.Username)),
            StringComparer.Ordinal);
        _rows.Clear();
        PreviewList.ItemsSource = null;
        _lastResult = null;
        _importedCount = 0;
        _hasImported = false;
        DragTransform.X = 0;
        DragTransform.Y = 0;
        ResetSourceChecks();
        ConsentCb.IsChecked = false;
        ScanBtn.IsEnabled = false;
        ShowState(DialogState.Consent);
        ApplyResources();
        AttachEscape();
        Visibility = Visibility.Visible;
    }

    /// <summary>public: 宿主 HostPanel 的 ESC 兜底在焦点不在弹窗内时也需要能关闭导入弹窗。</summary>
    public void Close()
    {
        DetachEscape();
        _rows.Clear();
        PreviewList.ItemsSource = null;
        _lastResult = null;
        Visibility = Visibility.Collapsed;
    }

    private async void ScanBtn_Click(object sender, RoutedEventArgs e)
    {
        ScanBtn.IsEnabled = false;
        ScanBtn.Content = LocalizationService.GetString("ImportScanning");
        ImportScanResult result;
        try
        {
            // 磁盘/注册表 IO 放后台线程; ScanSources 内部已按来源捕获异常, 不会抛出。
            // 只把用户勾选的来源传给解析层: 未勾选的来源不做文件探测、不读注册表 (N1 修复)
            var selected = CollectSelectedKinds();
            result = await Task.Run(() => HostImportParser.ScanSources(
                new ImportScanOptions { Sources = selected }));
        }
        catch (Exception)
        {
            // 纯防御: 理论不可达, 回到同意页并给出无结果提示
            ScanBtn.Content = LocalizationService.GetString("ImportScan");
            ScanBtn.IsEnabled = ConsentCb.IsChecked == true;
            ShowResult(false);
            return;
        }

        ScanBtn.Content = LocalizationService.GetString("ImportScan");
        ScanBtn.IsEnabled = ConsentCb.IsChecked == true;

        _lastResult = result;

        if (result.Sessions.Count == 0)
        {
            ShowResult(false);
            return;
        }

        BuildPreview(result);
        ShowState(DialogState.Preview);
    }

    private void BuildPreview(ImportScanResult result)
    {
        // 批内判重 (N3 修复): 同一主机可能同时出现在多个来源 (如 WinSCP INI + 注册表),
        // 只有第一条视为可导入, 后续重复标记 Duplicate 且不可勾选
        var seen = new HashSet<string>(StringComparer.Ordinal);
        _rows = result.Sessions.Select(s =>
        {
            var key = HostImportParser.DedupKey(s);
            var inStore = _existingKeys.Contains(key);
            var dupInBatch = !seen.Add(key);
            var exists = inStore || dupInBatch;
            return new ImportRow(s, inStore, dupInBatch)
            {
                IsChecked = !exists,
                AuthText = AuthTextFor(s),
                StatusText = StatusTextFor(s, inStore, dupInBatch),
            };
        }).ToList();
        PreviewList.ItemsSource = _rows;
        UpdatePreviewTexts();
    }

    private static string StatusTextFor(ImportedSession s, bool exists, bool dupInBatch) => exists
        ? LocalizationService.GetString("ImportStatusExisting")
        : dupInBatch ? LocalizationService.GetString("ImportStatusDuplicate")
        : s.Warning switch
        {
            "keyNotFound" => LocalizationService.GetString("ImportStatusKeyNotFound"),
            "uncPath" => LocalizationService.GetString("ImportStatusUncPath"),
            _ => LocalizationService.GetString("ImportStatusNew"),
        };

    private static string AuthTextFor(ImportedSession s)
        => LocalizationService.GetString(s.AuthMode == AuthMode.KeyFile
            ? "AuthModeKeyFile" : "AuthModePassword");

    private void UpdatePreviewTexts()
    {
        SummaryText.Text = string.Format(LocalizationService.GetString("ImportFoundSummary"),
            _rows.Count, _rows.Count(r => r.Exists));
        ImportBtn.Content = string.Format(LocalizationService.GetString("ImportImportN"),
            _rows.Count(r => r.IsChecked == true && !r.Exists));
        var warn = BuildDetailText(_lastResult);
        WarnText.Text = warn;
        WarnText.Visibility = warn.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    private void ImportBtn_Click(object sender, RoutedEventArgs e)
    {
        // 已存在条目不可再导 (勾选态也不计入), 写回由宿主完成
        var selected = _rows.Where(r => r.IsChecked == true && !r.Exists)
            .Select(r => r.Session).ToList();
        if (selected.Count == 0)
        {
            return;
        }

        _importedCount = selected.Count;
        ImportRequested?.Invoke(selected);
        ShowResult(true);
    }

    private void BackBtn_Click(object sender, RoutedEventArgs e) => ShowState(DialogState.Consent);

    private void ShowResult(bool hasImported)
    {
        _hasImported = hasImported;
        ResultIcon.Glyph = hasImported ? "\uE73E" : "\uE946";
        ResultIcon.Foreground = (Microsoft.UI.Xaml.Media.Brush)Application.Current.Resources[
            hasImported ? "SystemFillColorSuccessBrush" : "TextFillColorSecondaryBrush"];
        UpdateResultTexts();
        ShowState(DialogState.Result);
    }

    private void UpdateResultTexts()
    {
        ResultText.Text = _hasImported
            ? string.Format(LocalizationService.GetString("ImportResultSuccess"), _importedCount)
            : LocalizationService.GetString("ImportResultNone");
        var detail = BuildDetailText(_lastResult);
        ResultDetailText.Text = detail;
        ResultDetailText.Visibility = detail.Length > 0 ? Visibility.Visible : Visibility.Collapsed;
    }

    /// <summary>截断与错误明细: 只含来源名与机器可读 code 的文案, 不落任何会话内容。</summary>
    private string BuildDetailText(ImportScanResult? result)
    {
        if (result is null)
        {
            return string.Empty;
        }

        var lines = new List<string>();
        if (result.TotalTruncated > 0)
        {
            lines.Add(string.Format(LocalizationService.GetString("ImportResultTruncated"),
                result.TotalTruncated));
        }

        foreach (var (kind, code) in result.Errors)
        {
            var text = code switch
            {
                "fileTooLarge" => LocalizationService.GetString("ImportErrFileTooLarge"),
                "fileReadFailed" => LocalizationService.GetString("ImportErrFileReadFailed"),
                "parseFailed" => LocalizationService.GetString("ImportErrParseFailed"),
                "registryReadFailed" => LocalizationService.GetString("ImportErrRegistryReadFailed"),
                _ => code,
            };
            lines.Add($"{SourceName(kind)}: {text}");
        }

        return string.Join(Environment.NewLine, lines);
    }

    private static string SourceName(ImportSourceKind kind) => LocalizationService.GetString(kind switch
    {
        ImportSourceKind.WinScpIni => "ImportSourceWinScpIni",
        ImportSourceKind.WinScpRegistry => "ImportSourceWinScpRegistry",
        ImportSourceKind.FileZilla => "ImportSourceFileZilla",
        ImportSourceKind.PuTTY => "ImportSourcePuTTY",
        ImportSourceKind.OpenSsh => "ImportSourceOpenSsh",
        _ => "ImportSourceOwnStore",
    });

    private HashSet<ImportSourceKind> CollectSelectedKinds()
    {
        var kinds = new HashSet<ImportSourceKind>();
        if (WinScpIniCb.IsChecked == true) kinds.Add(ImportSourceKind.WinScpIni);
        if (WinScpRegCb.IsChecked == true) kinds.Add(ImportSourceKind.WinScpRegistry);
        if (FileZillaCb.IsChecked == true) kinds.Add(ImportSourceKind.FileZilla);
        if (PuTTYCb.IsChecked == true) kinds.Add(ImportSourceKind.PuTTY);
        if (OpenSshCb.IsChecked == true) kinds.Add(ImportSourceKind.OpenSsh);
        if (OwnStoreCb.IsChecked == true) kinds.Add(ImportSourceKind.OwnStore);
        return kinds;
    }

    private void ResetSourceChecks()
    {
        WinScpIniCb.IsChecked = true;
        WinScpRegCb.IsChecked = true;
        FileZillaCb.IsChecked = true;
        PuTTYCb.IsChecked = true;
        OpenSshCb.IsChecked = true;
        OwnStoreCb.IsChecked = true;
    }

    private void ConsentCb_Changed(object sender, RoutedEventArgs e)
        => ScanBtn.IsEnabled = ConsentCb.IsChecked == true;

    private void CancelBtn_Click(object sender, RoutedEventArgs e) => Close();

    private void DoneBtn_Click(object sender, RoutedEventArgs e) => Close();

    private void Mask_Tapped(object sender, TappedRoutedEventArgs e) => Close();

    private void CloseBtn_Click(object sender, RoutedEventArgs e) => Close();

    private void ShowState(DialogState state)
    {
        _state = state;
        ConsentView.Visibility = state == DialogState.Consent ? Visibility.Visible : Visibility.Collapsed;
        PreviewView.Visibility = state == DialogState.Preview ? Visibility.Visible : Visibility.Collapsed;
        ResultView.Visibility = state == DialogState.Result ? Visibility.Visible : Visibility.Collapsed;
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
        // 代码挂接: 面板隐藏时移除, 避免隐藏元素的快捷键行为不确定 (PermissionDialog 同款)。
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
        TitleText.Text = LocalizationService.GetString("ImportDialogTitle");
        PrivacyNote.Text = LocalizationService.GetString("ImportPrivacyNote");
        SourcesHeader.Text = LocalizationService.GetString("ImportSourcesHeader");
        WinScpIniCb.Content = SourceName(ImportSourceKind.WinScpIni);
        WinScpRegCb.Content = SourceName(ImportSourceKind.WinScpRegistry);
        FileZillaCb.Content = SourceName(ImportSourceKind.FileZilla);
        PuTTYCb.Content = SourceName(ImportSourceKind.PuTTY);
        OpenSshCb.Content = SourceName(ImportSourceKind.OpenSsh);
        OwnStoreCb.Content = SourceName(ImportSourceKind.OwnStore);
        ConsentCb.Content = LocalizationService.GetString("ImportConsent");
        CancelBtn.Content = LocalizationService.GetString("Cancel");
        BackBtn.Content = LocalizationService.GetString("ImportBack");
        DoneBtn.Content = LocalizationService.GetString("ImportDone");
        ColName.Text = LocalizationService.GetString("ImportColName");
        ColHost.Text = LocalizationService.GetString("ImportColHost");
        ColPort.Text = LocalizationService.GetString("ImportColPort");
        ColUser.Text = LocalizationService.GetString("ImportColUser");
        ColAuth.Text = LocalizationService.GetString("ImportColAuth");
        ColStatus.Text = LocalizationService.GetString("ImportColStatus");
        ScanBtn.Content = LocalizationService.GetString("ImportScan");
        foreach (var row in _rows)
        {
            row.AuthText = AuthTextFor(row.Session);
            row.StatusText = StatusTextFor(row.Session, row.InStore, row.DupInBatch);
        }

        if (_state == DialogState.Preview)
        {
            UpdatePreviewTexts();
        }
        else if (_state == DialogState.Result)
        {
            UpdateResultTexts();
        }
    }
}
