using iFreeSftp.Services;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Input;

namespace iFreeSftp.Views;

/// <summary>
/// 主容器：header 下面的整个内容区域。上排并排两个文件夹容器，下排并排
/// 上传队列容器与终端容器（初始高 60px）；中间竖/横分隔条均可拖拽，分别
/// 调节左右列宽与下排行高。标题栏（logo / 应用名 / 主题与语言切换）由
/// <see cref="Header"/> 承担。
/// </summary>
public sealed partial class MainContainer : Page
{
    /// <summary>下排容器最小/初始高度（逻辑像素）。</summary>
    private const double MinBottomHeight = 60;

    /// <summary>拖动横分隔条时上排保留的最小高度（逻辑像素）。</summary>
    private const double MinTopHeight = 200;

    /// <summary>左窗口文件夹容器（x:Name 字段是 private, 供 Header 注入主机连接目标视图）。</summary>
    public FolderContainer LeftView => LeftPane;

    /// <summary>右窗口文件夹容器（x:Name 字段是 private, 供 Header 注入主机连接目标视图）。</summary>
    public FolderContainer RightView => RightPane;

    public MainContainer()
    {
        InitializeComponent();
        Splitter.DragDelta += ColumnSplitter_DragDelta;
        RowSplitter.DragDelta += RowSplitter_DragDelta;
        Loaded += OnLoaded;
        // 对侧窗格互相接线 (FileView"复制到另一侧"定位目标)。
        LeftPane.OppositePane = RightPane;
        RightPane.OppositePane = LeftPane;
        // 任一窗格的 + 按钮/选择主机按钮请求打开选择器 → Header (带来源窗格)。
        LeftPane.OpenPickerRequested += pane => OpenPickerRequested?.Invoke(pane);
        RightPane.OpenPickerRequested += pane => OpenPickerRequested?.Invoke(pane);
    }

    /// <summary>请求打开"打开内容"选择器 (任一窗格触发, Header 订阅): 来源窗格 Left/Right。</summary>
    public event Action<string>? OpenPickerRequested;

    /// <summary>在指定窗格打开本地浏览 (选择器"本地文件系统"落位)。</summary>
    public void OpenLocalView(string pane)
    {
        if (pane == "Left")
        {
            LeftPane.OpenLocalView();
        }
        else
        {
            RightPane.OpenLocalView();
        }
    }

    private void OnLoaded(object sender, RoutedEventArgs e)
    {
        // 语言切换后刷新所有容器文本（主题切换无需刷新页面内容）。
        LocalizationService.CultureChanged += OnCultureChanged;
    }

    private void OnCultureChanged()
    {
        LeftPane.ApplyResources();
        RightPane.ApplyResources();
    }

    /// <summary>拖动竖分隔条，按水平位移调整左右列宽（上下两排同步）。</summary>
    private void ColumnSplitter_DragDelta(double dx, double dy)
    {
        var cols = ContentGrid.ColumnDefinitions;
        double left = cols[0].ActualWidth + dx;
        double right = cols[2].ActualWidth - dx;
        if (left < 120 || right < 120)
        {
            return;
        }
        cols[0].Width = new GridLength(left);
        cols[2].Width = new GridLength(right);
    }

    /// <summary>拖动横分隔条，按垂直位移调整下排行高（上移加高、下移收窄）。</summary>
    private void RowSplitter_DragDelta(double dx, double dy)
    {
        var rows = ContentGrid.RowDefinitions;
        double bottom = rows[2].ActualHeight - dy;
        double top = rows[0].ActualHeight + dy;
        if (bottom < MinBottomHeight || top < MinTopHeight)
        {
            return;
        }
        rows[2].Height = new GridLength(bottom);
    }
}

/// <summary>分隔条方向：Vertical=竖条（调列宽，左右手势），Horizontal=横条（调行高，上下手势）。</summary>
public enum SplitterOrientation
{
    Vertical,
    Horizontal,
}

/// <summary>
/// 分隔条控件：背景与 header 同色（hover 不变色），hover 时光标变拖拽手势，
/// 拖拽时通过 DragDelta 持续报告水平/垂直位移。WinUI 的 Thumb 是密封类，且
/// 页面 code-behind 无法访问受保护的 ProtectedCursor，故派生自 Control 自实现
/// 拖拽与光标；模板由使用处在 XAML 中内联提供。
/// </summary>
public sealed class SplitterThumb : Control
{
    private static readonly InputSystemCursor SizeWestEastCursor =
        InputSystemCursor.Create(InputSystemCursorShape.SizeWestEast);

    private static readonly InputSystemCursor SizeNorthSouthCursor =
        InputSystemCursor.Create(InputSystemCursorShape.SizeNorthSouth);

    /// <summary>拖拽中持续触发，参数为相对拖拽起点的水平/垂直位移（逻辑像素）。</summary>
    public event Action<double, double>? DragDelta;

    /// <summary>分隔条方向。</summary>
    public SplitterOrientation Orientation { get; set; } = SplitterOrientation.Vertical;

    private uint? _capturedPointerId;
    private double _startX;
    private double _startY;

    protected override void OnPointerEntered(PointerRoutedEventArgs e)
    {
        ProtectedCursor = Orientation == SplitterOrientation.Vertical ? SizeWestEastCursor : SizeNorthSouthCursor;
        base.OnPointerEntered(e);
    }

    protected override void OnPointerExited(PointerRoutedEventArgs e)
    {
        // 拖拽捕获期间光标保持手势，释放时统一清除。
        if (_capturedPointerId is null)
        {
            ProtectedCursor = null;
        }
        base.OnPointerExited(e);
    }

    protected override void OnPointerPressed(PointerRoutedEventArgs e)
    {
        var point = e.GetCurrentPoint(this);
        CapturePointer(e.Pointer);
        _capturedPointerId = e.Pointer.PointerId;
        _startX = point.Position.X;
        _startY = point.Position.Y;
        e.Handled = true;
        base.OnPointerPressed(e);
    }

    protected override void OnPointerMoved(PointerRoutedEventArgs e)
    {
        if (_capturedPointerId == e.Pointer.PointerId)
        {
            var point = e.GetCurrentPoint(this);
            DragDelta?.Invoke(point.Position.X - _startX, point.Position.Y - _startY);
            e.Handled = true;
        }
        base.OnPointerMoved(e);
    }

    protected override void OnPointerReleased(PointerRoutedEventArgs e)
    {
        if (_capturedPointerId == e.Pointer.PointerId)
        {
            ReleasePointerCapture(e.Pointer);
            _capturedPointerId = null;
            ProtectedCursor = null;
            e.Handled = true;
        }
        base.OnPointerReleased(e);
    }

    protected override void OnPointerCanceled(PointerRoutedEventArgs e)
    {
        _capturedPointerId = null;
        ProtectedCursor = null;
        base.OnPointerCanceled(e);
    }
}
