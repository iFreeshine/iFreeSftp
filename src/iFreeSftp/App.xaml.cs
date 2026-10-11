using Windows.ApplicationModel;
using Windows.ApplicationModel.Activation;
using Windows.Foundation;
using Windows.Foundation.Collections;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using Microsoft.UI.Xaml.Controls.Primitives;
using Microsoft.UI.Xaml.Data;
using Microsoft.UI.Xaml.Input;
using Microsoft.UI.Xaml.Media;
using Microsoft.UI.Xaml.Navigation;
using Microsoft.UI.Xaml.Shapes;

// To learn more about WinUI, the WinUI project structure,
// and more about our project templates, see: http://aka.ms/winui-project-info.

namespace iFreeSftp;

/// <summary>
/// Provides application-specific behavior to supplement the default Application class.
/// </summary>
public partial class App : Application
{
    private Window? _window;

    /// <summary>主窗口引用，供 ThemeService 等服务访问。</summary>
    public static Window? MainAppWindow { get; private set; }

    // —— 单实例 ——
    // 字段保引用: Mutex 被 GC 回收会提前释放互斥, 单实例随之失效
    private static Mutex? _singleInstanceMutex;

    [System.Runtime.InteropServices.DllImport("user32.dll")]
    private static extern bool SetForegroundWindow(nint hWnd);

    [System.Runtime.InteropServices.DllImport("user32.dll")]
    private static extern bool ShowWindow(nint hWnd, int nCmdShow);

    /// <summary>MainWindowHandle 为 0 (如由最小化状态枚举时) 也先走一遍还原。</summary>
    private const int SW_RESTORE = 9;

    /// <summary>
    /// 单实例守卫: 会话级命名 Mutex (不同 Windows 用户可各开一个)。
    /// 已有实例时把其主窗口还原并置前台, 然后当前进程退出。
    /// </summary>
    private bool TryAcquireSingleInstance()
    {
        _singleInstanceMutex = new Mutex(true, @"Local\iFreeSftp.SingleInstance", out var createdNew);
        if (createdNew)
        {
            return true;
        }

        try
        {
            var other = System.Diagnostics.Process.GetProcessesByName("iFreeSftp")
                .FirstOrDefault(p => p.Id != Environment.ProcessId && p.MainWindowHandle != 0);
            if (other is not null)
            {
                ShowWindow(other.MainWindowHandle, SW_RESTORE);
                SetForegroundWindow(other.MainWindowHandle);
            }
        }
        catch
        {
            // 激活已有实例失败无碍: 静默退出新实例即可
        }

        return false;
    }

    /// <summary>
    /// Initializes the singleton application object.  This is the first line of authored code
    /// executed, and as such is the logical equivalent of main() or WinMain().
    /// </summary>
    public App()
    {
        InitializeComponent();
        UnhandledException += OnUnhandledException;
        AppDomain.CurrentDomain.UnhandledException += OnDomainUnhandledException;
    }

    internal static void LogCrash(string source, string detail)
    {
        try
        {
            File.AppendAllText(
                System.IO.Path.Combine(System.IO.Path.GetTempPath(), "ifree_crash.log"),
                $"[{DateTime.Now:HH:mm:ss.fff}] {source}{Environment.NewLine}{detail}{Environment.NewLine}{Environment.NewLine}");
        }
        catch
        {
            // 日志失败时忽略, 避免掩盖原始异常
        }
    }

    private void OnUnhandledException(object sender, Microsoft.UI.Xaml.UnhandledExceptionEventArgs e)
    {
        LogCrash("XAML UnhandledException", $"{e.Message}{Environment.NewLine}{e.Exception}");
    }

    private void OnDomainUnhandledException(object? sender, System.UnhandledExceptionEventArgs e)
    {
        LogCrash("AppDomain UnhandledException", e.ExceptionObject?.ToString() ?? "null");
    }

    /// <summary>
    /// Invoked when the application is launched.
    /// </summary>
    /// <param name="args">Details about the launch request and process.</param>
    protected override void OnLaunched(Microsoft.UI.Xaml.LaunchActivatedEventArgs args)
    {
        // 单实例守卫最先执行: 重复启动不初始化任何服务 (注册/主题订阅), 拉起已有实例后退出
        if (!TryAcquireSingleInstance())
        {
            Exit();
            return;
        }

        // WPF 集群目录 (wpf\) 挂载托管/native 解析路径 (bundle 形态无该目录, 空转)
        Services.WpfBundle.Initialize();

        // 主面板切换明暗主题时, 同步所有已打开的 WPF 编辑器窗口（各自 STA 线程上重放主题）。
        Services.ThemeService.ThemeChanged += dark => WpfEditor.WpfEditorWindow.ApplyThemeAll(dark);

        // 注册体系: 加载本地注册状态 (DPAPI 解密 + 重验签), 后台节流拉取远程配置
        // (吊销名单/公告/勋章开关, 失败静默, 不阻塞启动)
        Services.RegistrationService.Load();
        _ = Services.RegistrationService.RefreshRemoteStateAsync();

        _window = new Views.Header();
        MainAppWindow = _window;
        _window.Activate();
    }
}
