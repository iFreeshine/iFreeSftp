using System.Text.Encodings.Web;
using System.Text.Json;

namespace iFreeSftp.Services;

/// <summary>
/// 应用设置存储：主题与语言，持久化为 %LOCALAPPDATA%\iFreeSftp\settings.json。
/// 修改后经 500ms 防抖合并落盘（Slider 拖动等高频改动只写末次）；应用退出前
/// 经 <see cref="FlushNow"/> 立即落盘，防最后 500ms 内改动丢失。
/// Theme/Language 为空表示"用户未配置过"，启动时分别按系统主题/默认语言处理。
/// </summary>
public static class AppSettingsService
{
    private static readonly string StorePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "iFreeSftp", "settings.json");

    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    };

    /// <summary>防抖写盘定时器：每次改动重置 500ms 到期，连续改动只触发一次落盘。</summary>
    private static readonly System.Threading.Timer _debounceTimer;

    /// <summary>防抖挂起标志：1 = 有未落盘改动（Interlocked 置零防 Timer 与
    /// 退出 FlushNow 并发时双写/漏写）。</summary>
    private static int _savePending;

    /// <summary>写盘互斥：Timer 回调线程与 UI 线程 FlushNow 可能并发到达。</summary>
    private static readonly object _writeLock = new();

    static AppSettingsService()
    {
        _debounceTimer = new System.Threading.Timer(_ => FlushNow());
    }

    /// <summary>主题设置："Dark" | "Light"，空 = 未配置（跟随系统）。</summary>
    public static string Theme { get; private set; } = string.Empty;

    /// <summary>语言设置："zh-CN" | "en-US"，空 = 未配置（默认中文）。</summary>
    public static string Language { get; private set; } = string.Empty;

    /// <summary>连接默认-超时秒数（0 = 禁用超时），默认 20。
    /// 参考 FileZilla 20s / WinSCP 15s: 正常网络握手几秒完成, 20s 失败尽早进入
    /// 重试; 上限 300s (连接超过 5 分钟未建立, 重试比干等有意义)。</summary>
    public static int ConnectTimeoutSeconds { get; private set; } = 20;

    /// <summary>连接默认-最大重试次数（0-99），默认 2。</summary>
    public static int MaxRetries { get; private set; } = 2;

    /// <summary>连接默认-登录重试延迟（秒，0-999），默认 5。</summary>
    public static int RetryDelaySeconds { get; private set; } = 5;

    /// <summary>连接默认-操作超时秒数（0 = 禁用），默认 15。SFTP 单次请求（列目录/
    /// 传输等）等待服务器响应的上限；服务器无响应（如连接配额被其他客户端占满）
    /// 时按上限报错，避免无限挂起表现为"已连接但列表永不显示"。</summary>
    public static int OperationTimeoutSeconds { get; private set; } = 15;

    /// <summary>连接打开方式: "ask" 每次询问（默认）| "current" 当前标签页 | "new" 新建标签页。</summary>
    public static string ConnectOpenMode { get; private set; } = "ask";

    /// <summary>传输-并发传输数（1-3），默认 2。1 = 串行并复用当前连接；
    /// &gt;1 时每个任务使用独立 SFTP 连接（SftpClient 非线程安全，单连接内并发无收益）。</summary>
    public static int ConcurrentTransfers { get; private set; } = 2;

    /// <summary>文件编辑器模式: "wpf" 应用内置编辑器（WPF AvalonEdit，代码高亮，默认）| "external" 外部编辑器。</summary>
    public static string EditorMode { get; private set; } = "wpf";

    /// <summary>外部编辑器可执行文件路径（EditorMode = "external" 时使用，空 = 回退内置编辑器）。</summary>
    public static string EditorPath { get; private set; } = string.Empty;

    /// <summary>远程删除方式: "exec" 服务端命令优先（经命令通道 rm -rf，失败自动回退
    /// SFTP 递归，默认）| "recursive" 纯 SFTP 协议逐条递归删除（兼容所有服务器）。</summary>
    public static string RemoteDeleteMode { get; private set; } = "exec";

    /// <summary>连接-保活开关（周期发送 NOOP 防长连接被中间网关断开），默认开启。
    /// 主机档案的同名设置从未生效（历史遗留）, 以全局为准。</summary>
    public static bool KeepAliveEnabled { get; private set; } = true;

    /// <summary>连接-保活间隔秒数，默认 30（参考 SftpSessionService 原默认）,
    /// 挡位 15/30/45/60。</summary>
    public static int KeepAliveIntervalSeconds { get; private set; } = 30;

    /// <summary>关于-有新版本时提醒（标题栏徽标），默认开启。关闭仅隐藏自动提醒,
    /// 「检查更新」按钮仍可用。</summary>
    public static bool UpdateCheckEnabled { get; private set; } = true;

    /// <summary>任一设置变更后触发 (跨面板联动, 如更新提醒开关 → 标题徽标重算;
    /// 订阅方需自行 marshal 到 UI 线程并保证处理幂等)。</summary>
    public static event Action? SettingsChanged;

    /// <summary>启动时从盘载入；文件缺失或损坏时保持默认（跟随系统/默认语言）。</summary>
    public static void Load()
    {
        try
        {
            if (!File.Exists(StorePath))
            {
                return;
            }

            var doc = JsonDocument.Parse(File.ReadAllText(StorePath)).RootElement;
            if (doc.TryGetProperty("theme", out var theme) && theme.ValueKind == JsonValueKind.String)
            {
                Theme = theme.GetString() ?? Theme;
            }

            if (doc.TryGetProperty("language", out var lang) && lang.ValueKind == JsonValueKind.String)
            {
                Language = lang.GetString() ?? Language;
            }

            if (doc.TryGetProperty("connectTimeout", out var timeout) && timeout.TryGetInt32(out var t))
            {
                ConnectTimeoutSeconds = Math.Clamp(t, 0, 300);
            }

            if (doc.TryGetProperty("maxRetries", out var retries) && retries.TryGetInt32(out var r))
            {
                MaxRetries = Math.Clamp(r, 0, 99);
            }

            if (doc.TryGetProperty("retryDelay", out var delay) && delay.TryGetInt32(out var d))
            {
                RetryDelaySeconds = Math.Clamp(d, 0, 999);
            }

            if (doc.TryGetProperty("operationTimeout", out var opTimeout) && opTimeout.TryGetInt32(out var ot))
            {
                OperationTimeoutSeconds = Math.Clamp(ot, 0, 86400);
            }

            if (doc.TryGetProperty("connectOpenMode", out var openMode) && openMode.ValueKind == JsonValueKind.String)
            {
                var mode = openMode.GetString();
                if (mode is "ask" or "current" or "new")
                {
                    ConnectOpenMode = mode;
                }
            }

            if (doc.TryGetProperty("concurrentTransfers", out var concurrent) && concurrent.TryGetInt32(out var cc))
            {
                ConcurrentTransfers = Math.Clamp(cc, 1, 3);
            }

            if (doc.TryGetProperty("editorMode", out var editorMode) && editorMode.ValueKind == JsonValueKind.String)
            {
                var mode = editorMode.GetString();
                if (mode is "wpf" or "external")
                {
                    EditorMode = mode;
                }
            }

            if (doc.TryGetProperty("editorPath", out var editorPath) && editorPath.ValueKind == JsonValueKind.String)
            {
                EditorPath = editorPath.GetString() ?? EditorPath;
            }

            if (doc.TryGetProperty("remoteDeleteMode", out var deleteMode) && deleteMode.ValueKind == JsonValueKind.String)
            {
                var mode = deleteMode.GetString();
                if (mode is "exec" or "recursive")
                {
                    RemoteDeleteMode = mode;
                }
            }

            if (doc.TryGetProperty("keepAliveEnabled", out var keepAlive)
                && keepAlive.ValueKind is JsonValueKind.True or JsonValueKind.False)
            {
                KeepAliveEnabled = keepAlive.GetBoolean();
            }

            if (doc.TryGetProperty("keepAliveInterval", out var keepAliveInterval) && keepAliveInterval.TryGetInt32(out var kv))
            {
                KeepAliveIntervalSeconds = Math.Clamp(kv, 15, 60);
            }

            if (doc.TryGetProperty("updateCheckEnabled", out var updateCheck)
                && updateCheck.ValueKind is JsonValueKind.True or JsonValueKind.False)
            {
                UpdateCheckEnabled = updateCheck.GetBoolean();
            }
        }
        catch
        {
            // 文件损坏等异常时保持默认, 不让应用启动失败。
        }
    }

    /// <summary>记录主题选择并落盘。</summary>
    public static void SetTheme(string theme)
    {
        Theme = theme;
        Save();
    }

    /// <summary>记录语言选择并落盘。</summary>
    public static void SetLanguage(string language)
    {
        Language = language;
        Save();
    }

    /// <summary>记录连接默认-超时秒数并落盘。</summary>
    public static void SetConnectTimeout(int seconds)
    {
        ConnectTimeoutSeconds = Math.Clamp(seconds, 0, 300);
        Save();
    }

    /// <summary>记录连接默认-最大重试次数并落盘。</summary>
    public static void SetMaxRetries(int retries)
    {
        MaxRetries = Math.Clamp(retries, 0, 99);
        Save();
    }

    /// <summary>记录连接默认-登录重试延迟并落盘。</summary>
    public static void SetRetryDelay(int seconds)
    {
        RetryDelaySeconds = Math.Clamp(seconds, 0, 999);
        Save();
    }

    /// <summary>记录连接默认-操作超时秒数并落盘。</summary>
    public static void SetOperationTimeout(int seconds)
    {
        OperationTimeoutSeconds = Math.Clamp(seconds, 0, 86400);
        Save();
    }

    /// <summary>记录连接打开方式（ask/current/new）并落盘。</summary>
    public static void SetConnectOpenMode(string mode)
    {
        ConnectOpenMode = mode is "current" or "new" ? mode : "ask";
        Save();
    }

    /// <summary>记录传输-并发传输数（1-3）并落盘。</summary>
    public static void SetConcurrentTransfers(int count)
    {
        ConcurrentTransfers = Math.Clamp(count, 1, 3);
        Save();
    }

    /// <summary>记录文件编辑器模式（wpf/external）并落盘。</summary>
    public static void SetEditorMode(string mode)
    {
        EditorMode = mode is "external" ? "external" : "wpf";
        Save();
    }

    /// <summary>记录外部编辑器路径并落盘。</summary>
    public static void SetEditorPath(string path)
    {
        EditorPath = path.Trim();
        Save();
    }

    /// <summary>记录远程删除方式（exec/recursive）并落盘。</summary>
    public static void SetRemoteDeleteMode(string mode)
    {
        RemoteDeleteMode = mode is "recursive" ? "recursive" : "exec";
        Save();
    }

    /// <summary>记录保活开关并落盘。</summary>
    public static void SetKeepAliveEnabled(bool enabled)
    {
        KeepAliveEnabled = enabled;
        Save();
    }

    /// <summary>记录保活间隔（15-60）并落盘。</summary>
    public static void SetKeepAliveInterval(int seconds)
    {
        KeepAliveIntervalSeconds = Math.Clamp(seconds, 15, 60);
        Save();
    }

    /// <summary>记录更新提醒开关并落盘 (触发 SettingsChanged 供标题徽标即时联动)。</summary>
    public static void SetUpdateCheckEnabled(bool enabled)
    {
        UpdateCheckEnabled = enabled;
        Save();
        SettingsChanged?.Invoke();
    }

    private static void Save()
    {
        Interlocked.Exchange(ref _savePending, 1);
        _debounceTimer.Change(500, Timeout.Infinite); // 重置到期: 连续改动只写末次
    }

    /// <summary>立即落盘挂起的设置改动（应用退出前调用，防最后 500ms 内改动丢失；
    /// 无挂起改动时空操作）。</summary>
    public static void FlushNow()
    {
        if (Interlocked.Exchange(ref _savePending, 0) != 1)
        {
            return;
        }

        lock (_writeLock)
        {
            try
            {
                WriteToDisk();
            }
            catch
            {
                // 写盘失败 (磁盘满/权限): 静默不打断退出流程, 下次改动会重试
            }
        }
    }

    private static void WriteToDisk()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(StorePath)!);
        File.WriteAllText(StorePath, JsonSerializer.Serialize(
            new
            {
                theme = Theme,
                language = Language,
                connectTimeout = ConnectTimeoutSeconds,
                maxRetries = MaxRetries,
                retryDelay = RetryDelaySeconds,
                operationTimeout = OperationTimeoutSeconds,
                connectOpenMode = ConnectOpenMode,
                concurrentTransfers = ConcurrentTransfers,
                editorMode = EditorMode,
                editorPath = EditorPath,
                remoteDeleteMode = RemoteDeleteMode,
                keepAliveEnabled = KeepAliveEnabled,
                keepAliveInterval = KeepAliveIntervalSeconds,
                updateCheckEnabled = UpdateCheckEnabled,
            }, JsonOptions));
    }
}
