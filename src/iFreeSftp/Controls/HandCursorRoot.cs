using System;
using System.Reflection;
using System.Runtime.InteropServices;
using Microsoft.UI.Input;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;

namespace iFreeSftp.Controls;

/// <summary>
/// 可移植的"手型光标根容器": 用作窗口根元素的自定义 Grid, 让窗口内所有
/// 按钮 (ButtonBase) 与 TabViewItem 悬停时显示手型光标; 非 ButtonBase 的
/// 可点元素 (如主机卡片 Border) 可设附加属性 HandCursorRoot.IsHand="True" 声明。
///
/// 用法 (替代原根 Grid, 其余 XAML 与代码零改动):
///   xmlns:controls="using:iFreeSftp.Controls"
///   &lt;controls:HandCursorRoot x:Name="RootGrid"&gt; ... &lt;/controls:HandCursorRoot&gt;
///
/// 覆盖两类区域:
///   1. 客户区: 根级动态光标 —— PointerMoved 经 FindElementsInHostCoordinates
///      命中 ButtonBase/TabViewItem 时, 反射设置 ProtectedCursor 为手型
///      (WinUI 3 无公开 Cursor 属性, ProtectedCursor 为 protected);
///      FindElementsInHostCoordinates 返回完整命中链 (含祖先), 无需上溯。
///   2. 标题条: TitleBar 控件把整个标题条注册为 OS 级 caption 拖拽区
///      (NCHITTEST=HTCAPTION, 包括自定义标题栏按钮上方), 该区域光标由窗口
///      过程 WM_SETCURSOR 决定, XAML 的 ProtectedCursor 会被无视 —— 故在
///      Loaded 时解析所属窗口并子类化其窗口过程: 标题条上
///      命中 ButtonBase/TabViewItem 显示手型, 其余保持默认箭头 (系统
///      最小化/最大化/关闭按钮由 OS 绘制, 按需忽略, 不拦截)。
///
/// 移植注意:
///   · PublishTrimmed 必须为 False (反射访问 ProtectedCursor);
///   · 无 SDK 版本要求: 所属窗口由"当前线程顶层窗口 + 客户区尺寸匹配"解析;
///   · 客户区命中按控件自身坐标系, 放在窗口任意层级均有效 (窗口根之外
///     使用时仅标题条命中测试以窗口客户区为准);
///   · SplitterThumb 等在自身元素上设置光标的控件按就近原则优先, 不受影响;
///   · SetCursor 前用 GetCursor 比对, 光标已是手型则不再设置 —— 否则系统会
///     因光标变化重发 WM_SETCURSOR, 悬停时形成每秒百次的消息循环。
/// </summary>
public class HandCursorRoot : Grid
{
    public HandCursorRoot()
    {
        // 客户区: 指针在根上移动时按命中结果切换光标, 离开根时还原。
        PointerMoved += OnSelfPointerMoved;
        PointerExited += OnSelfPointerExited;

        // 标题条: 需可视树建成后才能反查所属窗口, 在 Loaded 中子类化窗口过程。
        Loaded += OnSelfLoaded;
        Unloaded += OnSelfUnloaded;
    }

    // —— 客户区: 根级动态手型光标 ——

    private static readonly InputCursor HandCursor = InputSystemCursor.Create(InputSystemCursorShape.Hand);
    private static readonly PropertyInfo? ProtectedCursorProperty =
        typeof(UIElement).GetProperty("ProtectedCursor", BindingFlags.NonPublic | BindingFlags.Instance);
    private bool _handCursorActive;

    private void OnSelfPointerMoved(object sender, PointerRoutedEventArgs e)
    {
        SetHandCursor(IsHandCursorTarget(e.GetCurrentPoint(this).Position));
    }

    private void OnSelfPointerExited(object sender, PointerRoutedEventArgs e)
    {
        SetHandCursor(false);
    }

    // —— 附加属性: 非 ButtonBase 可点元素声明手型 ——

    /// <summary>附加属性: 元素悬停显示手型光标。供非 ButtonBase 的可点元素声明
    /// (如主机卡片 Border——单击进编辑), 命中测试链含祖先, 设在可点元素本身即可。</summary>
    public static readonly DependencyProperty IsHandProperty = DependencyProperty.RegisterAttached(
        "IsHand", typeof(bool), typeof(HandCursorRoot), new PropertyMetadata(false));

    public static bool GetIsHand(DependencyObject obj) => (bool)obj.GetValue(IsHandProperty);

    public static void SetIsHand(DependencyObject obj, bool value) => obj.SetValue(IsHandProperty, value);

    private bool IsHandCursorTarget(Windows.Foundation.Point point)
    {
        foreach (var element in VisualTreeHelper.FindElementsInHostCoordinates(point, this))
        {
            if (element is ButtonBase or TabViewItem || GetIsHand(element))
            {
                return true;
            }
        }

        return false;
    }

    private void SetHandCursor(bool hand)
    {
        if (_handCursorActive == hand || ProtectedCursorProperty is null)
        {
            return;
        }

        _handCursorActive = hand;
        ProtectedCursorProperty.SetValue(this, hand ? HandCursor : null);
    }

    // —— 标题条 (非客户区): WM_SETCURSOR 子类化 ——

    private const int WM_SETCURSOR = 0x0020;
    private const int HTCAPTION = 2;
    private static readonly IntPtr SystemHandCursor = LoadCursor(IntPtr.Zero, 32649); // IDC_HAND
    private static uint NextSubclassId = 1;

    private IntPtr _hwnd;
    private uint _subclassId;
    private SubclassProc? _subclassProc; // 持有委托引用防 GC 回收

    private delegate IntPtr SubclassProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, uint id, IntPtr refData);

    [DllImport("comctl32.dll")]
    private static extern bool SetWindowSubclass(IntPtr hWnd, SubclassProc proc, uint id, IntPtr refData);

    [DllImport("comctl32.dll")]
    private static extern bool RemoveWindowSubclass(IntPtr hWnd, SubclassProc proc, uint id);

    [DllImport("comctl32.dll")]
    private static extern IntPtr DefSubclassProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern IntPtr LoadCursor(IntPtr hInstance, int id);

    [DllImport("user32.dll")]
    private static extern IntPtr SetCursor(IntPtr cursor);

    [DllImport("user32.dll")]
    private static extern IntPtr GetCursor();

    [StructLayout(LayoutKind.Sequential)]
    private struct POINT
    {
        public int X;
        public int Y;
    }

    [DllImport("user32.dll")]
    private static extern bool GetCursorPos(out POINT pt);

    [DllImport("user32.dll")]
    private static extern bool ScreenToClient(IntPtr hWnd, ref POINT pt);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumWindowsProc proc, IntPtr lParam);

    private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    [DllImport("kernel32.dll")]
    private static extern uint GetCurrentThreadId();

    [DllImport("user32.dll")]
    private static extern bool GetClientRect(IntPtr hWnd, out RECT rect);

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    private void OnSelfLoaded(object sender, RoutedEventArgs e)
    {
        EnsureCaptionCursorHook();
    }

    private void OnSelfUnloaded(object sender, RoutedEventArgs e)
    {
        SetHandCursor(false);

        if (_hwnd == IntPtr.Zero || _subclassProc is null)
        {
            return;
        }

        try
        {
            RemoveWindowSubclass(_hwnd, _subclassProc, _subclassId);
        }
        catch
        {
        }

        _hwnd = IntPtr.Zero;
        _subclassProc = null;
    }

    /// <summary>
    /// 解析所属窗口并子类化其窗口过程: 枚举当前线程的顶层窗口 (WinUI 窗口与
    /// 其 XAML 内容运行在同一线程), 用客户区尺寸匹配可视树根的实际尺寸
    /// (根元素充满客户区, 逻辑尺寸 × 缩放 = 物理客户区) 定位所属窗口;
    /// 仅一个候选时直接采用。不依赖窗口如何持有本元素, 控件放在窗口任意
    /// 层级均有效。
    /// </summary>
    private void EnsureCaptionCursorHook()
    {
        if (_hwnd != IntPtr.Zero)
        {
            return;
        }

        try
        {
            DependencyObject root = this;
            while (VisualTreeHelper.GetParent(root) is DependencyObject parent)
            {
                root = parent;
            }

            var rootElement = (UIElement)root;
            var scale = XamlRoot?.RasterizationScale ?? 1.0;
            var candidates = new List<IntPtr>();
            EnumWindows((enumHwnd, _) =>
            {
                if (GetWindowThreadProcessId(enumHwnd, out var ownerThread) == GetCurrentThreadId())
                {
                    candidates.Add(enumHwnd);
                }

                return true;
            }, IntPtr.Zero);

            foreach (var hwnd in candidates)
            {
                if (GetClientRect(hwnd, out var rect)
                    && Math.Abs(rect.Right - rect.Left - rootElement.ActualSize.X * scale) < 4
                    && Math.Abs(rect.Bottom - rect.Top - rootElement.ActualSize.Y * scale) < 4)
                {
                    InstallSubclass(hwnd);
                    return;
                }
            }

            if (candidates.Count == 1)
            {
                InstallSubclass(candidates[0]);
            }
        }
        catch
        {
            // 解析失败仅影响标题条手型光标, 客户区逻辑不受影响。
        }
    }

    private void InstallSubclass(IntPtr hwnd)
    {
        _hwnd = hwnd;
        _subclassProc = CaptionSubclassProc;
        _subclassId = NextSubclassId++;
        SetWindowSubclass(_hwnd, _subclassProc, _subclassId, IntPtr.Zero);
    }

    private IntPtr CaptionSubclassProc(IntPtr hWnd, uint msg, IntPtr wParam, IntPtr lParam, uint id, IntPtr refData)
    {
        if (msg == WM_SETCURSOR)
        {
            var ht = (int)(lParam.ToInt64() & 0xFFFF);
            if (ht == HTCAPTION && IsCursorOverHandTarget())
            {
                if (GetCursor() != SystemHandCursor)
                {
                    SetCursor(SystemHandCursor);
                }

                return (IntPtr)1;
            }
        }

        return DefSubclassProc(hWnd, msg, wParam, lParam);
    }

    /// <summary>把当前光标位置换算成本控件逻辑坐标并做 XAML 命中测试。</summary>
    private bool IsCursorOverHandTarget()
    {
        try
        {
            if (_hwnd == IntPtr.Zero || !GetCursorPos(out var pt) || !ScreenToClient(_hwnd, ref pt))
            {
                return false;
            }

            var scale = XamlRoot?.RasterizationScale ?? 1.0;
            return IsHandCursorTarget(new Windows.Foundation.Point(pt.X / scale, pt.Y / scale));
        }
        catch
        {
            return false;
        }
    }
}
