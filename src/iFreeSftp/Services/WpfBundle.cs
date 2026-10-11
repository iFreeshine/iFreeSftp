using System.Runtime.InteropServices;
using System.Runtime.Loader;

namespace iFreeSftp.Services;

/// <summary>
/// WPF 集群目录 (wpf\) 运行时挂载。
/// 发布形态见 csproj WpfPublishMode: dir(默认)=exe+wpf\ 子目录 / bundle=全内嵌主 exe。
/// dir 形态下挂两条解析路径:
///   托管 — AssemblyLoadContext.Default.Resolving 兜底从 wpf\ 加载 (默认探测失败后触发);
///   native — AddDllDirectory 把 wpf\ 加入 DLL 搜索 (wpfgfx_cor3 等 WPF DllImport)。
/// bundle 形态无 wpf\ 目录, 自然空转。
/// </summary>
public static class WpfBundle
{
    private static int _initialized;

    private const uint LOAD_LIBRARY_SEARCH_APPLICATION_DIR = 0x200;
    private const uint LOAD_LIBRARY_SEARCH_SYSTEM32 = 0x800;
    private const uint LOAD_LIBRARY_SEARCH_USER_DIRS = 0x400;

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetDefaultDllDirectories(uint flags);

    [DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
    private static extern nint AddDllDirectory(string directory);

    /// <summary>启动最早处调用一次 (OnLaunched, 单实例守卫之后); 重复调用空转。</summary>
    public static void Initialize()
    {
        if (Interlocked.Exchange(ref _initialized, 1) != 0)
        {
            return;
        }

        try
        {
            // Environment.ProcessPath = 真 exe 路径 (单文件模式 AppContext.BaseDirectory
            // 指向 %TEMP% 解压目录, 不能用)
            var exeDir = Path.GetDirectoryName(Environment.ProcessPath);
            if (string.IsNullOrEmpty(exeDir))
            {
                return;
            }

            var wpfDir = Path.Combine(exeDir, "wpf");
            if (!Directory.Exists(wpfDir))
            {
                return; // bundle 形态 (或 wpf\ 被误删) → 无需挂载
            }

            // native: WPF 集群 DllImport (wpfgfx_cor3 等) 默认只探应用目录/系统目录,
            // 加 USER_DIRS + 用户目录后才可从 wpf\ 解析
            SetDefaultDllDirectories(
                LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32 | LOAD_LIBRARY_SEARCH_USER_DIRS);
            AddDllDirectory(wpfDir);

            // 托管: 默认探测失败后的兜底 (WpfEditor/AvalonEdit/WPF 托管集群)
            AssemblyLoadContext.Default.Resolving += (_, name) =>
            {
                var path = Path.Combine(wpfDir, name.Name + ".dll");
                return File.Exists(path)
                    ? AssemblyLoadContext.Default.LoadFromAssemblyPath(path)
                    : null;
            };
        }
        catch (Exception ex)
        {
            // 挂载失败不阻塞启动; 打开编辑器时才表现为 WPF 加载错误
            App.LogCrash("WpfBundle", ex.ToString());
        }
    }
}
