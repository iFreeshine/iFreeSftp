using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;

namespace iFreeSftp.Services;

/// <summary>
/// 主题服务：在亮色 / 暗色之间切换。
/// 主题必须应用在 Window 内容的根元素上：页面级 RequestedTheme 只影响自身子树，
/// 不会影响 Mica 背景与 TitleBar（它们跟随 Window 根部的主题）。
/// </summary>
public static class ThemeService
{
    private static FrameworkElement? _themeRoot;

    /// <summary>当前是否为暗色主题。</summary>
    public static bool IsDark { get; private set; } = true;

    /// <summary>主题切换后触发，参数为新的 IsDark 值。</summary>
    public static event Action<bool>? ThemeChanged;

    /// <summary>初始化：由 Header（主窗口）注入根元素，按已保存设置（未配置过则按 Windows
    /// 系统主题）设定初始值。注意不要用 ActualTheme 判断 —— 加载早期它可能尚未反映系统主题。</summary>
    public static void Initialize(FrameworkElement themeRoot)
    {
        _themeRoot = themeRoot;
        AppSettingsService.Load();
        IsDark = AppSettingsService.Theme switch
        {
            "Light" => false,
            "Dark" => true,
            _ => !IsSystemThemeLight(), // 未配置过时跟随系统主题
        };
        Apply();
        // 通知所有监听者按当前主题刷新（如图标）。
        ThemeChanged?.Invoke(IsDark);
        // 应用已保存的语言（在事件订阅前切换的话, 各视图构造时会按当前语言取文案）。
        if (!string.IsNullOrEmpty(AppSettingsService.Language))
        {
            LocalizationService.SetCulture(AppSettingsService.Language);
        }
    }

    /// <summary>切换明暗主题。</summary>
    public static void Toggle()
    {
        IsDark = !IsDark;
        AppSettingsService.SetTheme(IsDark ? "Dark" : "Light"); // 主题选择即时落盘
        Apply();
        ThemeChanged?.Invoke(IsDark);
    }

    /// <summary>按明暗直接设置主题（设置面板用），与当前一致时无操作。</summary>
    public static void SetTheme(bool dark)
    {
        if (IsDark != dark)
        {
            Toggle();
        }
    }

    private static void Apply()
    {
        if (_themeRoot is not null)
        {
            _themeRoot.RequestedTheme = IsDark ? ElementTheme.Dark : ElementTheme.Light;
        }

        // WinUI 3 已知问题：切换主题时 Mica 背景的合成层不会自动重建，会残留旧底色（"遮罩层"）。
        // 需重新挂载 SystemBackdrop 强制重建；且必须延迟到 RequestedTheme 传播完成之后，
        // 否则重建出的背景仍使用旧主题，造成背景与内容颜色相反。
        var queue = _themeRoot?.DispatcherQueue;
        queue?.TryEnqueue(DispatcherQueuePriority.Low, () =>
        {
            if (App.MainAppWindow is { SystemBackdrop: not null } window)
            {
                var backdrop = window.SystemBackdrop;
                window.SystemBackdrop = null;
                window.SystemBackdrop = backdrop;
            }
        });
    }

    /// <summary>读取 Windows 系统应用主题（注册表），true = 亮色。</summary>
    private static bool IsSystemThemeLight()
    {
        try
        {
            var value = Microsoft.Win32.Registry.GetValue(
                @"HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize",
                "AppsUseLightTheme", 1);
            return Convert.ToInt32(value) == 1;
        }
        catch
        {
            return false;
        }
    }
}
