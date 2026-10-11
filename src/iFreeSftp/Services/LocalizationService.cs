using Microsoft.Windows.ApplicationModel.Resources;
using System.Globalization;
using System.Text.Json;

namespace iFreeSftp.Services;

/// <summary>
/// 本地化服务：外置 locales 词典 + 内置 MRT Core 资源双层查找。
/// 内置层基于 MRT Core ResourceManager + 显式语言上下文，支持运行时切换中英文；
/// 注意：静态缓存的 ResourceLoader 不会跟随 PrimaryLanguageOverride 刷新，
/// 因此这里为每次语言创建独立 ResourceContext 并显式设置 language 限定符。
/// 外置层读取 exe 旁 locales\&lt;lang&gt;.json（发布时由 resw 导出），命中优先于内置——
/// 用户可自行编辑译文，或复制任一 json 改名翻译成新语言（重启生效）。
/// </summary>
public static class LocalizationService
{
    private static readonly ResourceManager _manager = new();
    private static readonly ResourceMap _map = _manager.MainResourceMap.GetSubtree("Resources");

    private static ResourceContext? _context;
    private static string _contextLanguage = string.Empty;

    // —— 外置词典 (locales\) ——

    /// <summary>
    /// 外置词典目录: exe 旁 locales\。单文件发布下 AppContext.BaseDirectory 是 %TEMP%
    /// 解压目录, 必须用 Environment.ProcessPath 定位 (同 WpfBundle); Debug 直跑无该
    /// 目录时为 null, 全部走内置资源。
    /// </summary>
    private static readonly string? _localesDir = ResolveLocalesDir();

    /// <summary>外置词典缓存: 语言代码 → 键值表。切换语言时读盘一次, 之后纯内存查找。</summary>
    private static readonly Dictionary<string, Dictionary<string, string>> _overrideCache = new(StringComparer.OrdinalIgnoreCase);

    /// <summary>可选语言 (内置 zh-CN/en-US + locales 目录发现的 *.json 文件名), 启动时扫描。</summary>
    private static readonly List<string> _languages = DiscoverLanguages();

    /// <summary>当前语言代码，例如 "zh-CN" / "en-US"。</summary>
    public static string CurrentCulture { get; private set; } = "zh-CN";

    /// <summary>语言切换后触发，UI 应在回调里重新应用字符串。</summary>
    public static event Action? CultureChanged;

    /// <summary>可选语言代码列表。locales 目录仅在启动时扫描, 新增语言文件重启后生效。</summary>
    public static IReadOnlyList<string> AvailableLanguages => _languages;

    /// <summary>
    /// 语言显示名: 取文化原生名去掉地域括号 (zh-CN → "中文", en-US → "English");
    /// 非标准语言代码 (CultureInfo 不识别) 原样显示。
    /// </summary>
    public static string GetLanguageDisplayName(string code)
    {
        try
        {
            var name = CultureInfo.GetCultureInfo(code).NativeName;
            var cut = name.IndexOfAny(['(', '（']);
            return cut > 0 ? name[..cut].Trim() : name;
        }
        catch
        {
            return code;
        }
    }

    private static string? ResolveLocalesDir()
    {
        try
        {
            var exeDir = Path.GetDirectoryName(Environment.ProcessPath);
            if (exeDir is null)
            {
                return null;
            }

            var dir = Path.Combine(exeDir, "locales");
            return Directory.Exists(dir) ? dir : null;
        }
        catch
        {
            return null;
        }
    }

    private static List<string> DiscoverLanguages()
    {
        // 内置语言固定两种 (pri 资源); locales 下任一 *.json 文件名即一种可选语言。
        var langs = new List<string> { "zh-CN", "en-US" };
        if (_localesDir is not null)
        {
            try
            {
                foreach (var file in Directory.GetFiles(_localesDir, "*.json"))
                {
                    var code = Path.GetFileNameWithoutExtension(file);
                    if (!string.IsNullOrWhiteSpace(code) &&
                        !langs.Contains(code, StringComparer.OrdinalIgnoreCase))
                    {
                        langs.Add(code);
                    }
                }
            }
            catch
            {
                // 目录扫描失败 (权限/IO 异常): 按仅内置处理, 不影响启动。
            }
        }
        return langs;
    }

    /// <summary>取某语言的外置词典 (无则空表)。首次访问读盘, 之后走缓存。</summary>
    private static Dictionary<string, string> GetOverride(string culture)
    {
        if (!_overrideCache.TryGetValue(culture, out var dict))
        {
            dict = LoadOverride(culture);
            _overrideCache[culture] = dict;
        }
        return dict;
    }

    private static Dictionary<string, string> LoadOverride(string culture)
    {
        var empty = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        if (_localesDir is null)
        {
            return empty;
        }

        try
        {
            var path = Path.Combine(_localesDir, culture + ".json");
            if (!File.Exists(path))
            {
                return empty;
            }

            using var stream = File.OpenRead(path);
            var parsed = JsonSerializer.Deserialize<Dictionary<string, string>>(stream);
            return parsed is null
                ? empty
                : new Dictionary<string, string>(parsed, StringComparer.OrdinalIgnoreCase);
        }
        catch
        {
            // 文件损坏/编码错误/格式非法: 忽略该词典, 回退内置资源。
            return empty;
        }
    }

    /// <summary>切换应用主语言。</summary>
    public static void SetCulture(string culture)
    {
        if (string.Equals(culture, CurrentCulture, StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        CurrentCulture = culture;
        AppSettingsService.SetLanguage(culture); // 语言选择即时落盘
        // 让 WinUI 框架层文本(日期格式等)同步切换；字符串读取走下方显式上下文。
        try
        {
            Windows.Globalization.ApplicationLanguages.PrimaryLanguageOverride = culture;
        }
        catch
        {
            // 非打包应用可能不支持 Override，忽略；不影响显式上下文读取。
        }

        CultureChanged?.Invoke();
    }

    /// <summary>
    /// 按 key 取本地化字符串。查找顺序: 外置 locales 词典 → 内置 pri 资源 → key 本身。
    /// 外置词典命中但值为空视为"待翻译", 回退内置, 避免出现空文案。
    /// </summary>
    public static string GetString(string key)
    {
        var dict = GetOverride(CurrentCulture);
        if (dict.Count > 0 && dict.TryGetValue(key, out var value) && value.Length > 0)
        {
            return value;
        }

        // 按需重建上下文：语言变化后旧上下文仍解析为旧语言。
        if (_context is null || _contextLanguage != CurrentCulture)
        {
            _context = _manager.CreateResourceContext();
            _context.QualifierValues["language"] = CurrentCulture;
            _contextLanguage = CurrentCulture;
        }

        return _map.TryGetValue(key, _context)?.ValueAsString ?? key;
    }
}
