using System.Collections.ObjectModel;
using iFreeSftp.Services;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;

namespace iFreeSftp.Views;

/// <summary>
/// 传输队列容器（左文件夹容器下方）: head 表头 + 队列列表 + foot 状态页签。
/// 表头整面作指针命中面（橡皮筋同款机制）：悬停靠近列分割线 ±6px 显示 ↔ 光标，
/// 按住拖拽即调宽——星号列在两侧间转移宽度、定宽列调像素，分割线始终跟随鼠标；
/// 每个页签独立记忆列宽。行网格与表头同构，宽度/可见性由代码同步应用
/// （ContainerContentChanging + 拖拽时遍历已实例化容器）。
/// 防闪烁刷新机制（两层）：
/// ① 行内变化（状态/速度/时刻）经 TransferTask 的 INPC 由 {Binding} 直达所在行，
///   列表结构与其余行完全不动（传输中每 400ms 的速度刷新不再整表重绑）；
/// ② 结构变化（入队/完成转出/失败转出/移除/清除已完结/重置）按变更类型对三个页签集合
///   （ObservableCollection）做增量增删——ItemsSource 引用仅在页签切换时更换，
///   传输全程零整体替换。变更事件来自任意线程，先攒批再经 DispatcherQueue
///   一次调度逐条应用（入队风暴合并为一个 UI 调度周期）。
/// </summary>
public sealed partial class QueueContainer : UserControl
{
    /// <summary>单列配置：星号列（占比）或定宽列（像素）。</summary>
    private sealed class ColCfg
    {
        public ColCfg(bool star, double val)
        {
            Star = star;
            Val = val;
        }

        public bool Star { get; }
        public double Val { get; set; }
    }

    // 各页签列配置（每页签独立记忆）: 5 列 = 源/目标/大小/速度或时间/原因;
    // 第 5 列（原因）仅失败页签可见（其余页签宽 0 隐藏）。失败页签把"原因"
    // 也作为星号列与源/目标分摊弹性宽度。
    private readonly Dictionary<TransferQueueTab, ColCfg[]> _cols = new()
    {
        [TransferQueueTab.Active] = new[]
        {
            NewCols(true, 2), NewCols(true, 2), NewCols(false, 90), NewCols(false, 90), NewCols(false, 0),
        },
        [TransferQueueTab.Failed] = new[]
        {
            NewCols(true, 1.5), NewCols(true, 1.5), NewCols(false, 90), NewCols(false, 90), NewCols(true, 2),
        },
        [TransferQueueTab.Done] = new[]
        {
            NewCols(true, 2), NewCols(true, 2), NewCols(false, 90), NewCols(false, 90), NewCols(false, 0),
        },
    };

    private static ColCfg NewCols(bool star, double val) => new(star, val);

    // 三个页签的视图集合（长期存在、增量增删；ItemsSource 仅在页签切换时换引用）。
    // 结构变更事件先攒批（_pending）再单次调度逐条应用, 入队风暴合并为一个调度周期。
    private readonly ObservableCollection<TransferTask> _activeView = [];
    private readonly ObservableCollection<TransferTask> _failedView = [];
    private readonly ObservableCollection<TransferTask> _doneView = [];
    private TransferQueueTab _tab = TransferQueueTab.Active;

    /// <summary>当前页签对应的视图集合（ApplyAll 遍历已实例化行用）。</summary>
    private ObservableCollection<TransferTask> CurrentView => _tab switch
    {
        TransferQueueTab.Failed => _failedView,
        TransferQueueTab.Done => _doneView,
        _ => _activeView,
    };

    // —— 表头列宽拖拽（整面命中: FileView 橡皮筋同款原始指针机制）——

    /// <summary>分割线命中判定半径（逻辑像素）。</summary>
    private const double GrabTolerance = 6;

    // WinUI 3 无公开 Cursor 属性: 反射设 ProtectedCursor（FileView 列手柄同款）
    private static readonly System.Reflection.PropertyInfo? ProtectedCursorProperty =
        typeof(UIElement).GetProperty("ProtectedCursor",
            System.Reflection.BindingFlags.NonPublic | System.Reflection.BindingFlags.Instance);
    private static readonly InputCursor SizeWestEastCursor =
        InputSystemCursor.Create(InputSystemCursorShape.SizeWestEast);

    // 拖拽状态: 按下分割线瞬间快照各列配置值, 之后 Moved 上报的累计位移直接
    // 在快照上叠加（不用增量累加, 避免布局回读误差逐步放大）
    private int? _dragCol;
    private double _dragStartX;
    private double[]? _dragStartVals;
    private double _perStarAtPress;
    private bool _captured;
    private bool _cursorOn;

    public QueueContainer()
    {
        InitializeComponent();
        TransferTask.UiDispatcher = DispatcherQueue; // INPC 通知强制封送 UI 线程（跨线程触发即崩）
        QueueList.ItemsSource = _activeView; // 初始"队列中"页签, 之后仅页签切换换引用
        QueueList.ContainerContentChanging += Row_Changing;
        LocalizationService.CultureChanged += OnCultureChanged;
        TransferQueueService.Changed += OnQueueChanged;
        // 捕获意外丢失（按下后失焦/弹窗等）时复位拖拽状态
        HeaderBar.PointerCaptureLost += (_, e) => EndDrag(e);
        // 右键菜单样式照搬主框架文件夹右键菜单 (README 3.9 第 7 条), 主题切换后重设底色
        QueueMenuFlyout.MenuFlyoutPresenterStyle = PaneMenuPresenterStyle();
        ActualThemeChanged += (_, _) => QueueMenuFlyout.MenuFlyoutPresenterStyle = PaneMenuPresenterStyle();
        ApplyResources();
    }

    /// <summary>队列右键菜单样式: 与终端命令窗右键菜单同款 (仅 presenter 背景按主题
    /// 显式指定, Popup 层主题字典覆盖不可靠), 其余走系统默认。亮 = #FFFFFF, 暗 = #26303D。</summary>
    private Style PaneMenuPresenterStyle()
    {
        var bg = ActualTheme == ElementTheme.Light
            ? Windows.UI.Color.FromArgb(255, 0xFF, 0xFF, 0xFF)
            : Windows.UI.Color.FromArgb(255, 0x26, 0x30, 0x3D);
        var style = new Style(typeof(MenuFlyoutPresenter));
        style.Setters.Add(new Setter(MenuFlyoutPresenter.BackgroundProperty, new SolidColorBrush(bg)));
        return style;
    }

    // —— 队列右键菜单: 按队列状态显隐 (Opening 时实时计算) ——

    /// <summary>
    /// 队列右键弹出菜单前按状态刷新各菜单项可见性：传输中（未暂停）= 暂停；暂停中 =
    /// 继续传输；停止在仍有等待或传输中任务时可用（大文件单任务传输中也可见）；
    /// 清除已完成在有完成/失败/已取消任务时可用；清除所有在队列非空时可用
    /// （连排队/传输中一起清, 传输中行删除后 worker 在块边界自动取消退出）；
    /// 重置并重新请求所有仅在队列静止（无等待/传输中）且有已完结任务时可用
    /// （避免与运行中的批次并行重跑）。队列空时不弹出。
    /// (用 RightTapped 手动 ShowAt 而非 ContextFlyout——WinUI 3 的 FlyoutBase.Opening
    /// 事件参数为 object, 无法在打开前按状态取消)
    /// </summary>
    private void Root_RightTapped(object sender, RightTappedRoutedEventArgs e)
    {
        var (total, waiting, transferring, done, failed, cancelled) = TransferQueueService.GetSummary();
        if (total == 0)
        {
            return;
        }

        var paused = ConcurrentTransferService.Paused;
        // 暂停/继续互斥: 暂停标志优先 (暂停中仍有任务在传完也显示"继续传输")
        PauseItem.Visibility = !paused && transferring > 0 ? Visibility.Visible : Visibility.Collapsed;
        ResumeItem.Visibility = paused ? Visibility.Visible : Visibility.Collapsed;
        StopItem.Visibility = waiting + transferring > 0 ? Visibility.Visible : Visibility.Collapsed;
        ClearDoneItem.Visibility = done + failed + cancelled > 0 ? Visibility.Visible : Visibility.Collapsed;
        ClearAllItem.Visibility = total > 0 ? Visibility.Visible : Visibility.Collapsed;
        RetryAllItem.Visibility = waiting + transferring == 0 && done + failed + cancelled > 0
            ? Visibility.Visible
            : Visibility.Collapsed;

        if (sender is FrameworkElement fe)
        {
            QueueMenuFlyout.ShowAt(fe, e.GetPosition(fe));
        }

        e.Handled = true;
    }

    private void PauseItem_Click(object sender, RoutedEventArgs e)
    {
        ConcurrentTransferService.Pause();
        TransferQueueService.SetPaused(true); // "等待中"行重读为"已暂停"
        TerminalLogService.Log(LocalizationService.GetString("LogQueuePaused"));
    }

    private void ResumeItem_Click(object sender, RoutedEventArgs e)
    {
        TransferQueueService.SetPaused(false);
        ConcurrentTransferService.Resume();
        TerminalLogService.Log(LocalizationService.GetString("LogQueueResumed"));
    }

    private void StopItem_Click(object sender, RoutedEventArgs e)
    {
        TransferQueueService.StopAll();
        TerminalLogService.Log(LocalizationService.GetString("LogQueueStopped"));
    }

    private void ClearDoneItem_Click(object sender, RoutedEventArgs e)
        => TransferQueueService.ClearAll();

    private void ClearAllItem_Click(object sender, RoutedEventArgs e)
        => TransferQueueService.RemoveAll();

    private void RetryAllItem_Click(object sender, RoutedEventArgs e)
        => _ = TransferQueueService.RerunAllAsync();

    // —— 行右键菜单: 按行任务状态显隐 (取消/重试/移除) ——

    /// <summary>行右键弹出菜单前按行任务状态刷新菜单项可见性：取消 = 等待/传输中；
    /// 重试 = 失败/已取消；移除 = 已完结（完成/失败/已取消）。三项均不可用（校验中）
    /// 不弹出。点击处理经 RowMenuFlyout.Target 取行任务（ShowAt 的目标元素）。</summary>
    private void Row_RightTapped(object sender, RightTappedRoutedEventArgs e)
    {
        if (sender is not FrameworkElement { DataContext: TransferTask task } row)
        {
            return;
        }

        var status = task.Status;
        var cancel = status is TransferStatus.Waiting or TransferStatus.Transferring;
        var retry = status is TransferStatus.Failed or TransferStatus.Cancelled;
        var remove = status is TransferStatus.Done or TransferStatus.Failed or TransferStatus.Cancelled;
        RowCancelItem.Visibility = cancel ? Visibility.Visible : Visibility.Collapsed;
        RowRetryItem.Visibility = retry ? Visibility.Visible : Visibility.Collapsed;
        RowRemoveItem.Visibility = remove ? Visibility.Visible : Visibility.Collapsed;
        if (!cancel && !retry && !remove)
        {
            return; // 校验中: 三项均不可用, 不弹菜单
        }

        RowMenuFlyout.ShowAt(row, e.GetPosition(row));
        e.Handled = true;
    }

    private void RowCancelItem_Click(object sender, RoutedEventArgs e)
    {
        if (RowMenuFlyout.Target is FrameworkElement { DataContext: TransferTask task })
        {
            TransferQueueService.CancelTask(task.Id);
        }
    }

    private void RowRetryItem_Click(object sender, RoutedEventArgs e)
    {
        if (RowMenuFlyout.Target is FrameworkElement { DataContext: TransferTask task })
        {
            _ = TransferQueueService.RerunOneAsync(task.Id);
        }
    }

    private void RowRemoveItem_Click(object sender, RoutedEventArgs e)
    {
        if (RowMenuFlyout.Target is FrameworkElement { DataContext: TransferTask task })
        {
            TransferQueueService.Remove(task.Id);
        }
    }

    private void OnCultureChanged()
    {
        ApplyResources();
        // StatusText 随语言重读: 三集合全部行刷新第四列（行内 INPC, 无结构变化）
        foreach (var task in _activeView.Concat(_failedView).Concat(_doneView))
        {
            task.RefreshVisual();
        }
    }

    // 队列结构变更在任意线程广播: 攒批后单次调度到 UI 线程逐条应用。
    // 防抖合并保留（目录展开批量入队时 Changed 同周期可达数百次, 逐次调度会淹死
    // 消息泵）, 但增量操作不能"只执行最后一次"——改为先攒进 _pending, 一个调度
    // 周期 drain 一次, 周期内所有增删在同一批里按顺序生效。
    private readonly List<(QueueChangeKind Kind, TransferTask? Task)> _pending = [];
    private bool _pendingScheduled;

    private void OnQueueChanged(QueueChangeKind kind, TransferTask? task)
    {
        lock (_pending)
        {
            _pending.Add((kind, task));
        }

        if (_pendingScheduled)
        {
            return;
        }

        _pendingScheduled = true;
        DispatcherQueue.TryEnqueue(() =>
        {
            _pendingScheduled = false;
            List<(QueueChangeKind Kind, TransferTask? Task)> batch;
            lock (_pending)
            {
                batch = new List<(QueueChangeKind, TransferTask?)>(_pending);
                _pending.Clear();
            }

            foreach (var (kind, task) in batch)
            {
                ApplyChange(kind, task);
            }

            UpdateTabCounts(); // 每批一次: 页签计数随结构变化刷新
        });
    }

    /// <summary>
    /// 应用一条结构变更（UI 线程）：入队=队列中尾部追加；完成/失败=队列中移除 +
    /// 对应页签尾部追加（其余行容器原样复用, 不重建视图）；移除=从三集合各删除
    /// （行已在其中之一, 其余为空操作）；清空=三集合清空；重置=三集合按服务端
    /// 快照全量重填（手动低频）。行内属性变化（状态/速度/时刻）不走此路径——
    /// INPC 直达所在行。
    /// </summary>
    private void ApplyChange(QueueChangeKind kind, TransferTask? task)
    {
        switch (kind)
        {
            case QueueChangeKind.Enqueued:
                if (task is not null)
                {
                    _activeView.Add(task);
                }

                break;

            case QueueChangeKind.Completed:
                if (task is not null)
                {
                    _activeView.Remove(task);
                    _doneView.Add(task);
                }

                break;

            case QueueChangeKind.Failed:
                if (task is not null)
                {
                    _activeView.Remove(task);
                    _failedView.Add(task);
                }

                break;

            case QueueChangeKind.Removed:
                if (task is not null)
                {
                    _activeView.Remove(task);
                    _failedView.Remove(task);
                    _doneView.Remove(task);
                }

                break;

            case QueueChangeKind.Cleared:
                _activeView.Clear();
                _failedView.Clear();
                _doneView.Clear();
                break;

            case QueueChangeKind.Reset:
                _activeView.Clear();
                _failedView.Clear();
                _doneView.Clear();
                foreach (var t in TransferQueueService.Get(TransferQueueTab.Active))
                {
                    _activeView.Add(t);
                }

                foreach (var t in TransferQueueService.Get(TransferQueueTab.Failed))
                {
                    _failedView.Add(t);
                }

                foreach (var t in TransferQueueService.Get(TransferQueueTab.Done))
                {
                    _doneView.Add(t);
                }

                break;
        }
    }

    /// <summary>重应用本地化文本（构造时与语言切换时）。</summary>
    private void ApplyResources()
    {
        ColSource.Text = LocalizationService.GetString("QColSource");
        ColDest.Text = LocalizationService.GetString("QColDest");
        ColSize.Text = LocalizationService.GetString("QColSize");
        ColReason.Text = LocalizationService.GetString("QColReason");
        TabActive.Content = LocalizationService.GetString("QTabActive");
        TabFailed.Content = LocalizationService.GetString("QTabFailed");
        TabDone.Content = LocalizationService.GetString("QTabDone");
        UpdateTabCounts(); // 计数随标题一起就位 (构造初始化 / 语言切换)
        // 第 4 列标签随页签变化: 队列中="速度", 失败/完成="时间"
        ColSpeed.Text = LocalizationService.GetString(
            _tab == TransferQueueTab.Active ? "QColSpeed" : "QColTime");
        // 右键菜单文案
        PauseItem.Text = LocalizationService.GetString("QMenuPause");
        ResumeItem.Text = LocalizationService.GetString("QMenuResume");
        StopItem.Text = LocalizationService.GetString("QMenuStop");
        ClearDoneItem.Text = LocalizationService.GetString("QMenuClearDone");
        ClearAllItem.Text = LocalizationService.GetString("QMenuClearAll");
        RetryAllItem.Text = LocalizationService.GetString("QMenuRetryAll");
        RowCancelItem.Text = LocalizationService.GetString("QMenuRowCancel");
        RowRetryItem.Text = LocalizationService.GetString("QMenuRowRetry");
        RowRemoveItem.Text = LocalizationService.GetString("QMenuRowRemove");
    }

    /// <summary>刷新三个状态页签的计数徽标（文字后跟实时数量: 队列/失败/成功）。
    /// 计数读自三个视图集合（与服务端状态天然一致），经 RadioButton.Tag 传入
    /// 页签模板的 TabCount 文本块。</summary>
    private void UpdateTabCounts()
    {
        TabActive.Tag = _activeView.Count.ToString();
        TabFailed.Tag = _failedView.Count.ToString();
        TabDone.Tag = _doneView.Count.ToString();
    }

    /// <summary>页签切换（队列中/传输失败/传输完成）: 记录页签、切换表头列语义与
    /// 第 5 列可见性、应用该页签记忆的列宽，并把列表绑到该页签的视图集合
    /// （ItemsSource 引用仅在页签切换时更换——传输全程零整体替换）。</summary>
    private void Tab_Checked(object sender, RoutedEventArgs e)
    {
        _tab = ReferenceEquals(sender, TabFailed) ? TransferQueueTab.Failed
            : ReferenceEquals(sender, TabDone) ? TransferQueueTab.Done
            : TransferQueueTab.Active;
        ApplyAll();
        QueueList.ItemsSource = CurrentView;
    }

    /// <summary>把当前页签的列配置与第 5 列可见性应用到表头和所有已实例化行。</summary>
    private void ApplyAll()
    {
        var cfg = _cols[_tab];
        var reasonVisible = _tab == TransferQueueTab.Failed;
        // 第 4 列标签随页签切换: 队列中="速度", 失败/完成="时间"
        ColSpeed.Text = LocalizationService.GetString(
            _tab == TransferQueueTab.Active ? "QColSpeed" : "QColTime");
        ApplyColumns(HeaderGrid, cfg);
        ColReason.Visibility = reasonVisible ? Visibility.Visible : Visibility.Collapsed;
        // 第 4 条分割线（原因列左缘）仅失败页签显示
        Div3.Visibility = reasonVisible ? Visibility.Visible : Visibility.Collapsed;
        var view = CurrentView;
        for (var i = 0; i < view.Count; i++)
        {
            if (QueueList.ContainerFromIndex(i) is DependencyObject container
                && FindRowRoot(container) is { } row)
            {
                ApplyColumns(row, cfg);
                SetRowReasonVisible(row, reasonVisible);
            }
        }
    }

    /// <summary>把列宽配置应用到网格（表头与行网格同构 5 列）。</summary>
    private static void ApplyColumns(Grid grid, ColCfg[] cfg)
    {
        var defs = grid.ColumnDefinitions;
        for (var i = 0; i < defs.Count && i < cfg.Length; i++)
        {
            defs[i].Width = cfg[i].Star
                ? new GridLength(cfg[i].Val, GridUnitType.Star)
                : new GridLength(cfg[i].Val);
        }
    }

    private static void SetRowReasonVisible(Grid row, bool visible)
    {
        if (row.Children.Count > 4 && row.Children[4] is TextBlock reason)
        {
            reason.Visibility = visible ? Visibility.Visible : Visibility.Collapsed;
        }
    }

    /// <summary>列表容器实例化/复用: 应用当前页签的列宽与第 5 列可见性。</summary>
    private void Row_Changing(ListViewBase sender, ContainerContentChangingEventArgs args)
    {
        if (args.InRecycleQueue
            || args.ItemContainer is not DependencyObject container
            || FindRowRoot(container) is not { } row)
        {
            return;
        }

        ApplyColumns(row, _cols[_tab]);
        SetRowReasonVisible(row, _tab == TransferQueueTab.Failed);
    }

    // —— 表头列宽拖拽（整面命中）——

    /// <summary>按下表头: 命中分割线则快照各列值并捕获指针, 开始拖拽。</summary>
    private void HeaderBar_Pressed(object sender, PointerRoutedEventArgs e)
    {
        var x = e.GetCurrentPoint(HeaderGrid).Position.X;
        if (HitBoundary(x) is not { } col)
        {
            return;
        }

        var cfgs = _cols[_tab];
        _dragStartVals = cfgs.Select(c => c.Val).ToArray();
        // 星|星分割线需要"像素/星"换算: 用按下瞬间该列实际宽 / 占比
        if (cfgs[col].Star && col + 1 < cfgs.Length && cfgs[col + 1].Star)
        {
            _perStarAtPress = HeaderGrid.ColumnDefinitions[col].ActualWidth / Math.Max(0.1, cfgs[col].Val);
            if (_perStarAtPress <= 0.1)
            {
                _dragStartVals = null; // 尚未布局, 放弃本次拖拽
                return;
            }
        }

        _dragCol = col;
        _dragStartX = x;
        HeaderBar.CapturePointer(e.Pointer);
        _captured = true;
        SetCursor(true);
        e.Handled = true;
    }

    /// <summary>表头上移动: 拖拽中按累计位移调列宽; 否则按是否命中分割线切光标。</summary>
    private void HeaderBar_Moved(object sender, PointerRoutedEventArgs e)
    {
        var x = e.GetCurrentPoint(HeaderGrid).Position.X;
        if (_dragCol is { } col)
        {
            DragColumn(col, x - _dragStartX);
            e.Handled = true;
        }
        else
        {
            SetCursor(HitBoundary(x) is not null);
        }
    }

    /// <summary>释放/取消: 结束拖拽并复位光标。</summary>
    private void HeaderBar_Released(object sender, PointerRoutedEventArgs e) => EndDrag(e);

    private void EndDrag(PointerRoutedEventArgs e)
    {
        if (_dragCol is null && !_captured)
        {
            return;
        }

        _dragCol = null;
        _dragStartVals = null;
        if (_captured)
        {
            _captured = false;
            HeaderBar.ReleasePointerCapture(e.Pointer);
        }
        SetCursor(false);
    }

    private void HeaderBar_Exited(object sender, PointerRoutedEventArgs e)
    {
        if (_dragCol is null)
        {
            SetCursor(false);
        }
    }

    private void SetCursor(bool on)
    {
        if (_cursorOn == on)
        {
            return;
        }

        _cursorOn = on;
        ProtectedCursorProperty?.SetValue(HeaderBar, on ? SizeWestEastCursor : null);
    }

    /// <summary>命中检测: x（HeaderGrid 坐标）落在某条分割线 ±6px 内返回左侧列索引。
    /// 分割线位于列间距（12px）正中, 仅当两侧列均可见（Val&gt;0）时可命中。</summary>
    private int? HitBoundary(double x)
    {
        var defs = HeaderGrid.ColumnDefinitions;
        var cfgs = _cols[_tab];
        double spacing = HeaderGrid.ColumnSpacing;
        double acc = 0;
        for (var i = 0; i < defs.Count - 1; i++)
        {
            acc += defs[i].ActualWidth;
            if (cfgs[i].Val > 0 && cfgs[i + 1].Val > 0
                && Math.Abs(x - (acc + (spacing / 2))) <= GrabTolerance)
            {
                return i;
            }
            acc += spacing;
        }

        return null;
    }

    /// <summary>拖动分割线（col 为左侧列索引）: 在按下快照上叠加累计位移。
    /// 星|星 → 左加右减（星号总量不变, 分割线精确跟随鼠标）; 星|定 → 右侧定宽列
    /// 让出/占用空间; 定|定 → 左加右减, 两侧定宽列均可调（速度列仅能经大小|速度
    /// 分割线调宽——其右缘是隐藏原因列, 不可命中）。调整后同步应用到表头与已实例化行。</summary>
    private void DragColumn(int col, double dxTotal)
    {
        if (_dragStartVals is null || col >= _dragStartVals.Length)
        {
            return;
        }

        var cfgs = _cols[_tab];
        var start = _dragStartVals;
        if (cfgs[col].Star)
        {
            if (col + 1 < cfgs.Length && cfgs[col + 1].Star)
            {
                var shift = dxTotal / _perStarAtPress;
                cfgs[col].Val = Math.Max(0.4, start[col] + shift);
                cfgs[col + 1].Val = Math.Max(0.4, start[col + 1] - shift);
            }
            else if (col + 1 < cfgs.Length)
            {
                cfgs[col + 1].Val = Math.Clamp(start[col + 1] - dxTotal, 50, 420);
            }
        }
        else
        {
            cfgs[col].Val = Math.Clamp(start[col] + dxTotal, 50, 420);
            // 定|定: 左加右减, 分割线跟随鼠标且两侧列宽均随拖拽变化
            if (col + 1 < cfgs.Length && !cfgs[col + 1].Star)
            {
                cfgs[col + 1].Val = Math.Clamp(start[col + 1] - dxTotal, 50, 420);
            }
        }

        ApplyAll();
    }

    /// <summary>在容器可视化树中递归查找行网格（DataTemplate 内 x:Name="RowRoot"）。</summary>
    private static Grid? FindRowRoot(DependencyObject node)
    {
        var count = VisualTreeHelper.GetChildrenCount(node);
        for (var i = 0; i < count; i++)
        {
            if (VisualTreeHelper.GetChild(node, i) is not DependencyObject child)
            {
                continue;
            }

            if (child is Grid { Name: "RowRoot" } row)
            {
                return row;
            }

            var found = FindRowRoot(child);
            if (found is not null)
            {
                return found;
            }
        }

        return null;
    }

    // —— 源/目标列路径覆盖提示 ——

    /// <summary>当前打开的路径覆盖浮层（同一时刻至多一个）。</summary>
    private Popup? _pathPopup;

    /// <summary>
    /// 源/目标列 hover：原位覆盖显示完整路径。不用系统 ToolTip——它贴着指针出现、
    /// 随悬停位置漂移且离文字块远；这里用 Popup 按文字块窗口坐标定位（浮层 Border
    /// 内边距反扣偏移，浮层文字与原文字重合、外框恰好罩住），位置稳定、完整路径
    /// 一次看全。浮层 IsHitTestVisible=false 不参与命中，否则盖住原块触发
    /// Exited→关闭→再 Entered 抖动循环。（WinUI 3 Popup 无 PlacementTarget 定位,
    /// 经 TransformToVisual 自算; 无父级宿主须显式指定 XamlRoot）
    /// </summary>
    private void PathText_PointerEntered(object sender, PointerRoutedEventArgs e)
    {
        if (sender is not TextBlock text || text.Text is not { Length: > 0 })
        {
            return;
        }

        ClosePathPopup();

        bool dark = ActualTheme == ElementTheme.Dark;
        const double padX = 8, padY = 5; // 浮层 Border 内边距 (定位反扣, 文字与原块重合)
        // 配色不经 ThemeResource（Popup 顶层资源解析不可靠），按 ActualTheme 直配:
        // 底=行容器色深/浅一档, 字=行内文字色 (与 SettingsPanel 导航按钮同款做法)
        var tip = new Border
        {
            Background = new SolidColorBrush(dark
                ? Windows.UI.Color.FromArgb(255, 0x2A, 0x35, 0x45)
                : Windows.UI.Color.FromArgb(255, 0xFF, 0xFF, 0xFF)),
            BorderBrush = new SolidColorBrush(dark
                ? Windows.UI.Color.FromArgb(255, 0x4A, 0x58, 0x6C)
                : Windows.UI.Color.FromArgb(255, 0xD0, 0xD0, 0xD0)),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(4),
            Padding = new Thickness(padX, padY, padX, padY),
            Child = new TextBlock
            {
                Text = text.Text,
                FontSize = 12,
                MaxWidth = 720, // 超长路径兜底: 超窗部分无法显示, 省略号截断
                TextTrimming = TextTrimming.CharacterEllipsis,
                Foreground = new SolidColorBrush(dark
                    ? Windows.UI.Color.FromArgb(255, 0xE8, 0xE8, 0xE8)
                    : Windows.UI.Color.FromArgb(255, 0x22, 0x22, 0x22)),
            },
        };

        // 文字块左上角 → 窗口坐标 (DIP, 与 Popup 偏移同坐标系), 再反扣内边距
        var topLeft = text.TransformToVisual(null).TransformPoint(new Windows.Foundation.Point(0, 0));
        _pathPopup = new Popup
        {
            XamlRoot = XamlRoot,
            IsHitTestVisible = false,
            HorizontalOffset = topLeft.X - padX,
            VerticalOffset = topLeft.Y - padY,
            Child = tip,
        };
        _pathPopup.IsOpen = true;
    }

    /// <summary>离开源/目标列：关闭覆盖浮层（先于下一块的 Entered，移列切换无残留）。</summary>
    private void PathText_PointerExited(object sender, PointerRoutedEventArgs e) => ClosePathPopup();

    private void ClosePathPopup()
    {
        if (_pathPopup is not null)
        {
            _pathPopup.IsOpen = false;
            _pathPopup = null;
        }
    }
}
