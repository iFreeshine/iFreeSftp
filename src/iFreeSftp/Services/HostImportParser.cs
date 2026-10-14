using System.Globalization;
using System.Runtime.CompilerServices;
using System.Text;
using System.Text.Json;
using System.Xml;
using iFreeSftp.Models;

// 注册表键值映射（FromPuTTYSessionMap 等）为 internal：纯函数形态直测，
// 免去注册表依赖；仅对测试程序集开放。
[assembly: InternalsVisibleTo("iFreeSftp.Tests")]

namespace iFreeSftp.Services;

// =============================================================================
// 会话导入解析器：从其他 SFTP 客户端读取"连接元数据"并转为 HostProfile。
//
// 三条红线（实现层固化，任何改动不得违反）：
//   红线 1 —— 口令绝不导入。其他客户端保存的密码一律不读取、不解密：
//             WinSCP 未设主密码时密码为 XOR 0xA3 可逆混淆，解密行为与
//             EDR 标记的凭据窃取特征一致；FileZilla 的 <Pass> 明文同样不读。
//             唯一例外是本软件自有存档（iFreeSftp hosts.json）：口令本就是
//             DPAPI 密文（enc: 前缀），随记录整体迁移走 ToHostProfile 的
//             fail-safe 通道（换机/换用户解密失败即清空）。
//   红线 2 —— 命令类字段绝不导入。PuTTY/WinSCP 的 RemoteCommand、FileZilla 的
//             POST_LOGIN_COMMANDS、OpenSSH 的 ProxyCommand/LocalCommand 一律
//             丢弃：本应用存在命令执行通道（SftpSessionService.RunCommandAsync），
//             导入这些字段等于引入最大的命令注入面。
//   红线 3 —— 只导连接元数据：名称/主机/端口/用户名/认证方式/密钥路径。
//             代理、跳板等字段不导（HostProfile 无承载处，导入即丢）。
//
// 隐私红线：仅在本机读取用户显式选择的来源；单文件 ≤ 8MB、单来源会话 ≤ 5000 条；
// 解析失败/日志只记机器可读 code 与来源路径，绝不落会话内容；不外传任何数据。
// =============================================================================

/// <summary>导入来源种类。</summary>
public enum ImportSourceKind
{
    /// <summary>WinSCP INI 文件（%APPDATA%\WinSCP.ini）。</summary>
    WinScpIni,

    /// <summary>WinSCP 注册表会话（HKCU\Software\Martin Prikryl\WinSCP 2\Sessions）。</summary>
    WinScpRegistry,

    /// <summary>FileZilla 站点管理器（%APPDATA%\FileZilla\sitemanager.xml）。</summary>
    FileZilla,

    /// <summary>PuTTY 注册表会话（HKCU\Software\SimonTatham\PuTTY\Sessions）。</summary>
    PuTTY,

    /// <summary>OpenSSH 客户端配置（~\.ssh\config）。</summary>
    OpenSsh,

    /// <summary>本软件自有存档（%LOCALAPPDATA%\iFreeSftp\hosts.json，跨机迁移场景）。</summary>
    OwnStore,
}

/// <summary>
/// 单条导入会话（纯元数据）。口令字段不存在——除 OwnStore 的 DPAPI 密文载体
/// <see cref="OwnStorePassword"/> 外，任何来源都不携带口令。
/// </summary>
public sealed class ImportedSession
{
    public ImportSourceKind Kind { get; set; }

    /// <summary>站点显示名（来源缺失时由解析器回退为主机名）。</summary>
    public string Name { get; set; } = string.Empty;

    public string Host { get; set; } = string.Empty;

    public int Port { get; set; } = 22;

    public string Username { get; set; } = string.Empty;

    public AuthMode AuthMode { get; set; } = AuthMode.Password;

    /// <summary>私钥/密钥文件路径（原样导入，UI 预览表供用户确认）。</summary>
    public string KeyFilePath { get; set; } = string.Empty;

    /// <summary>机器可读警告码：keyNotFound（密钥路径不存在）/ uncPath（UNC 路径已拒收）。</summary>
    public string Warning { get; set; } = string.Empty;

    /// <summary>
    /// 仅 OwnStore 来源：存档中的密码 DPAPI 密文（enc: 前缀）。
    /// 交由 <see cref="HostImportParser.ToHostProfile"/> 走 fail-safe 解密。
    /// </summary>
    public string OwnStorePassword { get; set; } = string.Empty;

    /// <summary>基本有效性（解析器已过滤无效项，此属性为防御性判定）。</summary>
    public bool IsValid => !string.IsNullOrWhiteSpace(Host) && Port >= 1 && Port <= 65535;
}

/// <summary>单个来源的扫描统计（UI 结果页展示，只有计数，无内容）。</summary>
public sealed class ImportSourceStat
{
    public ImportSourceKind Kind { get; set; }

    /// <summary>发现的会话条数（含跳过项）。</summary>
    public int Found { get; set; }

    /// <summary>实际导入条数。</summary>
    public int Imported { get; set; }

    /// <summary>非 SFTP 协议跳过条数。</summary>
    public int SkippedNonSftp { get; set; }

    /// <summary>字段无效（缺主机/端口越界等）跳过条数。</summary>
    public int SkippedInvalid { get; set; }

    /// <summary>超出单来源上限被丢弃条数。</summary>
    public int Truncated { get; set; }
}

/// <summary>整体扫描结果。</summary>
public sealed class ImportScanResult
{
    public List<ImportedSession> Sessions { get; } = new();

    /// <summary>每个已探测来源一条统计（含未发现的来源）。</summary>
    public List<ImportSourceStat> Stats { get; } = new();

    /// <summary>本机未发现的来源（kind 机器名，UI 映射为"未检测到"提示）。</summary>
    public List<string> MissingSources { get; } = new();

    /// <summary>单来源失败记录。code：fileTooLarge / fileNotFound / fileReadFailed / parseFailed / registryReadFailed。</summary>
    public List<(ImportSourceKind Kind, string Code)> Errors { get; } = new();

    /// <summary>超出全局总量上限（MaxSessions）被丢弃的条数。</summary>
    public int TotalTruncated { get; set; }
}

/// <summary>解析层异常：携带机器可读 code，UI 层映射本地化文案。</summary>
public sealed class ImportScanException : Exception
{
    public string Code { get; }

    public ImportScanException(string code, string message, Exception? inner = null)
        : base(message, inner) => Code = code;
}

/// <summary>扫描来源选项；路径为 null 时使用默认探测路径。</summary>
public sealed class ImportScanOptions
{
    public string? WinScpIniPath { get; set; }
    public string? FileZillaSitesPath { get; set; }
    public string? OpenSshConfigPath { get; set; }
    public string? OwnStorePath { get; set; }

    /// <summary>是否扫描 HKCU 注册表来源（WinSCP / PuTTY）。</summary>
    public bool IncludeRegistry { get; set; } = true;

    /// <summary>
    /// 允许读取的来源集合；null 表示全部。UI 传入用户勾选的集合，
    /// 未列入的来源一个字节都不读（与隐私公示一致，见技术文档 3.3）。
    /// </summary>
    public HashSet<ImportSourceKind>? Sources { get; set; }
}

/// <summary>
/// 导入解析主类。文本解析函数为纯函数（字符串进、对象出，不触碰文件系统与注册表），
/// 便于单测；<see cref="ScanSources"/> 为编排层（非纯，含注册表与文件读取）。
/// </summary>
public static class HostImportParser
{
    /// <summary>单来源会话数上限。</summary>
    public const int MaxSessions = 5000;

    /// <summary>单文件字节上限。</summary>
    public const long MaxFileBytes = 8 * 1024 * 1024;

    /// <summary>
    /// 存档密码密文前缀。与 HostStoreService.EncPrefix 语义一致（其为 private，
    /// 解析器自定义等值常量；两处必须同步修改）。
    /// </summary>
    private const string EncPrefixConst = "enc:";

    static HostImportParser()
    {
        // ANSI 回退编码（GBK 等）。CodePages 编码提供器自 .NET Core 3.0 起内置于
        // 共享框架，无需（也不得）添加 NuGet 包引用，否则触发 NU1510。
        Encoding.RegisterProvider(CodePagesEncodingProvider.Instance);
    }

    // =========================================================================
    // 字节 → 文本：编码探测链
    // =========================================================================

    /// <summary>
    /// 编码探测链：UTF-8 BOM → UTF-16 LE BOM（WinSCP INI 默认）→ UTF-16 BE BOM
    /// → 严格 UTF-8（非法字节即失败）→ 系统 ANSI 代码页回退。
    /// </summary>
    public static string DecodeBytes(byte[] bytes)
    {
        if (bytes.Length >= 3 && bytes[0] == 0xEF && bytes[1] == 0xBB && bytes[2] == 0xBF)
            return new UTF8Encoding(false).GetString(bytes, 3, bytes.Length - 3);
        if (bytes.Length >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE)
            return Encoding.Unicode.GetString(bytes, 2, bytes.Length - 2);
        if (bytes.Length >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF)
            return Encoding.BigEndianUnicode.GetString(bytes, 2, bytes.Length - 2);

        var strictUtf8 = new UTF8Encoding(encoderShouldEmitUTF8Identifier: false, throwOnInvalidBytes: true);
        try
        {
            return strictUtf8.GetString(bytes);
        }
        catch (DecoderFallbackException)
        {
            return Encoding.GetEncoding(0).GetString(bytes);
        }
    }

    // =========================================================================
    // WinSCP INI（[Sessions\folder\site] 节 + key=value）
    // =========================================================================

    /// <summary>
    /// 解析 WinSCP INI 文本。仅导入 Protocol 为 SFTP 的会话（键缺省视为 SFTP）。
    /// 红线 1：Password 键绝不读取。
    /// </summary>
    public static List<ImportedSession> ParseWinScpIni(string text, ImportSourceStat stat)
    {
        var result = new List<ImportedSession>();
        var values = new Dictionary<string, string>(StringComparer.OrdinalIgnoreCase);
        var siteName = string.Empty;
        var inSession = false;

        void Flush()
        {
            if (!inSession) return;
            inSession = false;
            stat.Found++;
            var s = FromWinScpValues(values, siteName, stat, ImportSourceKind.WinScpIni);
            if (s != null) AddCapped(result, s, stat);
            values.Clear();
        }

        foreach (var rawLine in text.Split('\n'))
        {
            var line = rawLine.TrimEnd('\r').Trim();
            if (line.Length == 0) continue;

            if (line[0] == '[')
            {
                Flush();
                var end = line.IndexOf(']');
                var section = end > 0 ? line.Substring(1, end - 1) : line.Trim('[', ']');
                if (section.StartsWith("Sessions\\", StringComparison.OrdinalIgnoreCase))
                {
                    inSession = true;
                    // 站点名 = 节名最后一段（WinSCP 对特殊字符做 %XX 转义）
                    siteName = SafeUnescape(LastSegment(section, '\\'));
                }
            }
            else if (inSession)
            {
                var eq = line.IndexOf('=');
                if (eq <= 0) continue;
                values[line.Substring(0, eq).Trim()] = line.Substring(eq + 1).Trim();
            }
        }
        Flush();
        return result;
    }

    /// <summary>WinSCP 键值字典（INI 或注册表）→ 会话。端口无效返回 null（已计入 stat）。</summary>
    private static ImportedSession? FromWinScpValues(
        Dictionary<string, string> v, string name, ImportSourceStat stat, ImportSourceKind kind)
    {
        // 红线 1：Password（XOR 0xA3 混淆）绝不读取，也不出现在任何中间结构里。
        var protocol = Get(v, "Protocol");
        if (!string.IsNullOrEmpty(protocol) && !protocol.Equals("SFTP", StringComparison.OrdinalIgnoreCase))
        {
            stat.SkippedNonSftp++;
            return null;
        }

        var host = Get(v, "HostName").Trim();
        if (host.Length == 0)
        {
            stat.SkippedInvalid++;
            return null;
        }

        var port = ParsePort(Get(v, "PortNumber"), stat);
        if (port == 0) return null;

        var keyPath = Get(v, "PublicKeyFile").Trim();
        return new ImportedSession
        {
            Kind = kind,
            Name = name.Length > 0 ? name : host,
            Host = host,
            Port = port,
            Username = Get(v, "UserName").Trim(),
            AuthMode = keyPath.Length > 0 ? AuthMode.KeyFile : AuthMode.Password,
            KeyFilePath = keyPath,
        };
    }

    // =========================================================================
    // 注册表来源（WinSCP / PuTTY，均只读 HKCU 指定子键）
    // =========================================================================

    /// <summary>WinSCP 注册表键值 → 会话（DWORD 端口转字符串后复用 INI 映射）。</summary>
    private static ImportedSession? FromWinScpRegistryValues(
        Dictionary<string, object> v, string name, ImportSourceStat stat)
    {
        var asStrings = new Dictionary<string, string>(v.Count, StringComparer.OrdinalIgnoreCase);
        foreach (var kv in v)
        {
            asStrings[kv.Key] = kv.Value is int dword
                ? dword.ToString(CultureInfo.InvariantCulture)
                : kv.Value as string ?? string.Empty;
        }
        return FromWinScpValues(asStrings, name, stat, ImportSourceKind.WinScpRegistry);
    }

    /// <summary>
    /// PuTTY 注册表键值 → 会话。Protocol 非空且非 ssh 跳过；HostName 可能是 user@host。
    /// 键名 URL 转义由编排层（ScanRegistrySessions）统一还原。internal 供单测直测。
    /// </summary>
    internal static ImportedSession? FromPuTTYSessionMap(
        Dictionary<string, object> v, string name, ImportSourceStat stat)
    {
        // 红线 1：PuTTY 注册表本身不存密码，防御性不读取任何疑似口令键。
        var protocol = GetString(v, "Protocol");
        if (!string.IsNullOrEmpty(protocol) && !protocol.Equals("ssh", StringComparison.OrdinalIgnoreCase))
        {
            stat.SkippedNonSftp++;
            return null;
        }

        // N2 修复：PuTTY 注册表存独立的 UserName 键，优先读取；为空时回退到
        // HostName 的 user@host 前缀拆分（兼容旧会话写法）。
        var (userFromHost, host) = SplitUserHost(GetString(v, "HostName").Trim());
        if (host.Length == 0)
        {
            stat.SkippedInvalid++;
            return null;
        }
        var user = GetString(v, "UserName").Trim();
        if (user.Length == 0) user = userFromHost;

        var port = 22;
        if (v.TryGetValue("PortNumber", out var portObj) && portObj is int dword
            && dword >= 1 && dword <= 65535)
        {
            port = dword;
        }

        var keyPath = GetString(v, "PublicKeyFile").Trim();
        return new ImportedSession
        {
            Kind = ImportSourceKind.PuTTY,
            Name = name.Length > 0 ? name : host,
            Host = host,
            Port = port,
            Username = user,
            AuthMode = keyPath.Length > 0 ? AuthMode.KeyFile : AuthMode.Password,
            KeyFilePath = keyPath,
        };
    }

    /// <summary>
    /// 遍历 HKCU 下指定注册表路径的会话子键，逐条映射。
    /// 只读（OpenSubKey），不写注册表、不触碰 HKLM。
    /// </summary>
    private static void ScanRegistrySessions(
        ImportScanResult result,
        ImportSourceKind kind,
        string basePath,
        Func<Dictionary<string, object>, string, ImportSourceStat, ImportedSession?> map)
    {
        var stat = new ImportSourceStat { Kind = kind };
        try
        {
            using var key = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(basePath);
            if (key == null)
            {
                result.MissingSources.Add(kind.ToString());
                result.Stats.Add(stat);
                return;
            }

            foreach (var subName in key.GetSubKeyNames())
            {
                using var sub = key.OpenSubKey(subName);
                if (sub == null) continue;

                var values = new Dictionary<string, object>(StringComparer.OrdinalIgnoreCase);
                foreach (var valName in sub.GetValueNames())
                {
                    if (valName.Length == 0) continue;
                    // PuTTY 对键名做 URL 转义（空格 %20 等）
                    values[SafeUnescape(valName)] = sub.GetValue(valName) ?? string.Empty;
                }

                stat.Found++;
                var s = map(values, SafeUnescape(LastSegment(subName, '\\')), stat);
                if (s != null) AddCapped(result.Sessions, s, stat);
            }
        }
        catch (Exception)
        {
            // 只记来源与机器码，绝不落会话内容
            result.Errors.Add((kind, "registryReadFailed"));
        }
        result.Stats.Add(stat);
    }

    // =========================================================================
    // FileZilla sitemanager.xml
    // =========================================================================

    /// <summary>
    /// 解析 FileZilla 站点管理器 XML。Protocol=="1"（SFTP）才导；Logontype=="5"
    ///（密钥文件）读取 Keyfile 元素。
    /// 红线 1/2：Pass/Password/Passkey（明文口令）与 POST_LOGIN_COMMANDS（命令）
    /// 显式 Skip，绝不读取内容。
    /// </summary>
    public static List<ImportedSession> ParseFileZillaXml(string text, ImportSourceStat stat)
    {
        var result = new List<ImportedSession>();
        var settings = new XmlReaderSettings
        {
            DtdProcessing = DtdProcessing.Prohibit, // XXE 防护
            XmlResolver = null,                     // 禁止外部实体
            IgnoreComments = true,
            IgnoreWhitespace = true,
            MaxCharactersInDocument = MaxFileBytes * 4L,
        };

        string? host = null, port = null, user = null, name = null, keyfile = null, logontype = null, protocol = null;
        var inServer = false;

        void Flush()
        {
            if (!inServer) return;
            inServer = false;
            stat.Found++;

            if (!string.IsNullOrEmpty(protocol) && protocol.Trim() != "1")
            {
                stat.SkippedNonSftp++;
                return;
            }
            var hostTrimmed = (host ?? string.Empty).Trim();
            if (hostTrimmed.Length == 0)
            {
                stat.SkippedInvalid++;
                return;
            }
            var portNum = ParsePort(port, stat);
            if (portNum == 0) return;

            // FileZilla Logontype: 0=Anonymous 1=Normal 2=Ask 3=Interactive 4=Account 5=Key file
            var isKey = string.Equals(logontype?.Trim(), "5", StringComparison.Ordinal)
                && !string.IsNullOrWhiteSpace(keyfile);
            AddCapped(result, new ImportedSession
            {
                Kind = ImportSourceKind.FileZilla,
                Name = string.IsNullOrWhiteSpace(name) ? hostTrimmed : name.Trim(),
                Host = hostTrimmed,
                Port = portNum,
                Username = (user ?? string.Empty).Trim(),
                AuthMode = isKey ? AuthMode.KeyFile : AuthMode.Password,
                KeyFilePath = isKey ? keyfile!.Trim() : string.Empty,
            }, stat);
        }

        using var sr = new StringReader(text);
        using var reader = XmlReader.Create(sr, settings);
        while (reader.Read())
        {
            if (reader.NodeType == XmlNodeType.Element)
            {
                if (reader.LocalName.Equals("Server", StringComparison.OrdinalIgnoreCase))
                {
                    Flush(); // 防御：嵌套/未闭合的 Server 提前截断上一条
                    inServer = true;
                    host = port = user = name = keyfile = logontype = protocol = null;
                    continue;
                }
                if (!inServer) continue;

                var el = reader.LocalName;
                if (IsForbiddenFileZillaElement(el))
                {
                    SkipSubtree(reader); // 红线 1/2：口令与命令元素整块跳过
                    continue;
                }
                switch (el)
                {
                    case "Host": host = ReadSimpleElement(reader); break;
                    case "Port": port = ReadSimpleElement(reader); break;
                    case "User": user = ReadSimpleElement(reader); break;
                    case "Name": name = ReadSimpleElement(reader); break;
                    case "Keyfile": keyfile = ReadSimpleElement(reader); break;
                    case "Logontype": logontype = ReadSimpleElement(reader); break;
                    case "Protocol": protocol = ReadSimpleElement(reader); break;
                    default:
                        SkipSubtree(reader);
                        break;
                }
            }
            else if (reader.NodeType == XmlNodeType.EndElement
                && reader.LocalName.Equals("Server", StringComparison.OrdinalIgnoreCase))
            {
                Flush();
            }
        }
        Flush(); // 防御：文档截断时的末条
        return result;
    }

    private static bool IsForbiddenFileZillaElement(string el)
        => el.Equals("Pass", StringComparison.OrdinalIgnoreCase)
        || el.Equals("Password", StringComparison.OrdinalIgnoreCase)
        || el.Equals("Passkey", StringComparison.OrdinalIgnoreCase)
        || el.Equals("POST_LOGIN_COMMANDS", StringComparison.OrdinalIgnoreCase);

    /// <summary>
    /// 读取当前简单元素（仅文本内容）的值；复杂内容一律忽略。
    /// 注意：XmlReader.ReadElementContentAsString 会把定位推到结束标签之后的节点，
    /// 与外层 while (reader.Read()) 组合会跳过下一个元素的开始标签导致字段交错丢失；
    /// 本方法读完内容后停在内容节点上（不越过 EndElement），由外层循环自然推进。
    /// </summary>
    private static string? ReadSimpleElement(XmlReader reader)
    {
        if (reader.IsEmptyElement) return null;
        if (!reader.Read()) return null;
        return reader.NodeType is XmlNodeType.Text or XmlNodeType.CDATA
            ? reader.Value
            : null;
    }

    /// <summary>
    /// 跳过当前元素整个子树，停在对应 EndElement 上（不越过），由外层 while 推进越过。
    /// 语义等同 reader.Skip()，但修正了与外层 Read 循环组合时的定位偏移问题。
    /// </summary>
    private static void SkipSubtree(XmlReader reader)
    {
        if (reader.IsEmptyElement) return;
        var depth = reader.Depth;
        while (reader.Read())
        {
            if (reader.NodeType == XmlNodeType.EndElement && reader.Depth == depth) return;
        }
    }

    // =========================================================================
    // OpenSSH ~/.ssh/config
    // =========================================================================

    /// <summary>
    /// 解析 OpenSSH 客户端配置。通配符（* ?）Host 块与 Match 块整体跳过；
    /// IdentityFile 取首个有效值（~ 展开、%VAR% 展开、去引号）；Include 不递归。
    /// 红线 2：ProxyCommand/LocalCommand/RemoteCommand 绝不读取（本地命令注入面）。
    /// </summary>
    public static List<ImportedSession> ParseOpenSshConfig(string text, ImportSourceStat stat)
    {
        var result = new List<ImportedSession>();
        var inBlock = false;   // 处于某个 Host/Match 块内
        var skipBlock = false; // 通配符或 Match 块（不导入）
        string? pattern = null;
        string? hostName = null;
        string? user = null;
        string? identity = null;
        var port = 0;

        void Flush()
        {
            if (!inBlock) return;
            inBlock = false;
            if (skipBlock) return; // 通配符/Match 块不是具体会话，不计数

            stat.Found++;
            var host = hostName ?? pattern;
            if (string.IsNullOrWhiteSpace(host))
            {
                stat.SkippedInvalid++;
                return;
            }
            AddCapped(result, new ImportedSession
            {
                Kind = ImportSourceKind.OpenSsh,
                Name = pattern ?? host,
                Host = host.Trim(),
                Port = port == 0 ? 22 : port,
                Username = user ?? string.Empty,
                AuthMode = identity != null ? AuthMode.KeyFile : AuthMode.Password,
                KeyFilePath = identity ?? string.Empty,
            }, stat);
        }

        foreach (var rawLine in text.Split('\n'))
        {
            var line = rawLine.TrimEnd('\r').Trim();
            if (line.Length == 0 || line[0] == '#') continue;

            var sep = line.IndexOfAny(new[] { ' ', '\t', '=' });
            if (sep < 0) continue;
            var keyword = line.Substring(0, sep).Trim();
            var rest = line.Substring(sep + 1).Trim();

            if (keyword.Equals("Host", StringComparison.OrdinalIgnoreCase))
            {
                Flush();
                var patterns = SplitTokens(rest);
                inBlock = true;
                skipBlock = patterns.Count == 0 || patterns.Any(p => p.Contains('*') || p.Contains('?'));
                pattern = patterns.Count > 0 ? patterns[0] : null;
                hostName = null;
                user = null;
                identity = null;
                port = 0;
            }
            else if (keyword.Equals("Match", StringComparison.OrdinalIgnoreCase))
            {
                Flush();
                inBlock = true;
                skipBlock = true; // 条件块，整体不导入
                pattern = null;
                hostName = null;
                user = null;
                identity = null;
                port = 0;
            }
            else if (!inBlock || skipBlock)
            {
                continue; // 块外全局选项 / 被跳过块内的命令类键（红线 2）不处理
            }
            else if (keyword.Equals("HostName", StringComparison.OrdinalIgnoreCase))
            {
                hostName ??= FirstToken(rest);
            }
            else if (keyword.Equals("Port", StringComparison.OrdinalIgnoreCase))
            {
                if (port == 0 && int.TryParse(FirstToken(rest), NumberStyles.Integer,
                    CultureInfo.InvariantCulture, out var p) && p >= 1 && p <= 65535)
                {
                    port = p;
                }
            }
            else if (keyword.Equals("User", StringComparison.OrdinalIgnoreCase))
            {
                user ??= FirstToken(rest);
            }
            else if (keyword.Equals("IdentityFile", StringComparison.OrdinalIgnoreCase))
            {
                identity ??= ExpandIdentityFile(rest);
            }
            // 其余键（含 Include/ProxyCommand 等）一律忽略；Include 按设计不递归
        }
        Flush();
        return result;
    }

    /// <summary>IdentityFile 值规范化：去引号、~ → 用户主目录、%VAR% 环境变量展开。</summary>
    private static string? ExpandIdentityFile(string raw)
    {
        var s = raw.Trim();
        if (s.Length >= 2 && s[0] == '"' && s[s.Length - 1] == '"')
            s = s.Substring(1, s.Length - 2).Trim();
        if (s.Length == 0) return null;

        var home = Environment.GetFolderPath(Environment.SpecialFolder.UserProfile);
        if (s == "~")
            s = home;
        else if (s.StartsWith("~/", StringComparison.Ordinal) || s.StartsWith("~\\", StringComparison.Ordinal))
            s = Path.Combine(home, s.Substring(2).Replace('/', Path.DirectorySeparatorChar));
        if (s.Contains('%'))
            s = Environment.ExpandEnvironmentVariables(s);
        return s.Length == 0 ? null : s;
    }

    // =========================================================================
    // 自有存档 hosts.json（跨机迁移）
    // =========================================================================

    /// <summary>
    /// 解析本软件自有存档（根对象 { "hosts": [HostProfile...] }，与
    /// HostStoreService 落盘结构一致）。属性名大小写不敏感，兼容历史存档。
    /// 红线 1 特例：密码 DPAPI 密文（enc: 前缀）放入 OwnStorePassword，
    /// 由 ToHostProfile 走 fail-safe 解密；明文口令（无前缀）一律丢弃。
    /// </summary>
    public static List<ImportedSession> ParseOwnStoreJson(string json, ImportSourceStat stat)
    {
        var result = new List<ImportedSession>();
        JsonDocument doc;
        try
        {
            doc = JsonDocument.Parse(json);
        }
        catch (JsonException ex)
        {
            throw new ImportScanException("parseFailed", "own store json parse failed", ex);
        }

        using (doc)
        {
            if (!doc.RootElement.TryGetProperty("hosts", out var hosts)
                || hosts.ValueKind != JsonValueKind.Array)
            {
                stat.SkippedInvalid++;
                return result;
            }

            foreach (var el in hosts.EnumerateArray())
            {
                if (el.ValueKind != JsonValueKind.Object) continue;
                stat.Found++;

                var props = new Dictionary<string, JsonElement>(StringComparer.OrdinalIgnoreCase);
                foreach (var prop in el.EnumerateObject()) props[prop.Name] = prop.Value;

                var host = GetStringProp(props, "Host").Trim();
                if (host.Length == 0)
                {
                    stat.SkippedInvalid++;
                    continue;
                }
                var port = 22;
                if (props.TryGetValue("Port", out var portEl) && portEl.TryGetInt32(out var p))
                {
                    if (p < 1 || p > 65535)
                    {
                        stat.SkippedInvalid++;
                        continue;
                    }
                    port = p;
                }

                // 密码只接受 enc: 前缀密文；明文（历史遗留/异常存档）绝不迁移
                var storedPwd = GetStringProp(props, "Password");
                var cipherPwd = storedPwd.StartsWith(EncPrefixConst, StringComparison.Ordinal)
                    ? storedPwd
                    : string.Empty;

                AddCapped(result, new ImportedSession
                {
                    Kind = ImportSourceKind.OwnStore,
                    Name = GetStringProp(props, "Name"),
                    Host = host,
                    Port = port,
                    Username = GetStringProp(props, "Username"),
                    AuthMode = GetStringProp(props, "AuthMode").Equals("KeyFile", StringComparison.OrdinalIgnoreCase)
                        ? AuthMode.KeyFile
                        : AuthMode.Password,
                    KeyFilePath = GetStringProp(props, "KeyFilePath"),
                    OwnStorePassword = cipherPwd,
                }, stat);
            }
        }
        return result;
    }

    private static string GetStringProp(Dictionary<string, JsonElement> props, string name)
        => props.TryGetValue(name, out var el) && el.ValueKind == JsonValueKind.String
            ? el.GetString() ?? string.Empty
            : string.Empty;

    // =========================================================================
    // 编排层（非纯）：默认路径探测、文件/注册表读取、校验、合并
    // =========================================================================

    /// <summary>按默认路径扫描勾选的来源并合并结果；单来源失败不中断整体。</summary>
    public static ImportScanResult ScanSources(ImportScanOptions? options = null)
    {
        options ??= new ImportScanOptions();
        var result = new ImportScanResult();

        // 来源白名单（N1 修复）：未勾选的来源不做文件探测、不读注册表，Stats/MissingSources 也不产生
        if (Wanted(options, ImportSourceKind.WinScpIni))
            ScanFileSource(result, ImportSourceKind.WinScpIni,
                options.WinScpIniPath ?? DefaultWinScpIniPath(), ParseWinScpIni);
        if (Wanted(options, ImportSourceKind.FileZilla))
            ScanFileSource(result, ImportSourceKind.FileZilla,
                options.FileZillaSitesPath ?? DefaultFileZillaPath(), ParseFileZillaXml);
        if (Wanted(options, ImportSourceKind.OpenSsh))
            ScanFileSource(result, ImportSourceKind.OpenSsh,
                options.OpenSshConfigPath ?? DefaultOpenSshPath(), ParseOpenSshConfig);
        if (Wanted(options, ImportSourceKind.OwnStore))
            ScanFileSource(result, ImportSourceKind.OwnStore,
                options.OwnStorePath ?? DefaultOwnStorePath(), ParseOwnStoreJson);

        if (options.IncludeRegistry)
        {
            if (Wanted(options, ImportSourceKind.WinScpRegistry))
                ScanRegistrySessions(result, ImportSourceKind.WinScpRegistry,
                    @"Software\Martin Prikryl\WinSCP 2\Sessions", FromWinScpRegistryValues);
            if (Wanted(options, ImportSourceKind.PuTTY))
                ScanRegistrySessions(result, ImportSourceKind.PuTTY,
                    @"Software\SimonTatham\PuTTY\Sessions", FromPuTTYSessionMap);
        }

        // 密钥路径校验（UNC 拒收 / 不存在告警），供预览表展示
        foreach (var s in result.Sessions)
            Validate(s, File.Exists);

        // 全局总量上限（隐私红线：总会话 ≤ MaxSessions）
        if (result.Sessions.Count > MaxSessions)
        {
            result.TotalTruncated = result.Sessions.Count - MaxSessions;
            result.Sessions.RemoveRange(MaxSessions, result.Sessions.Count - MaxSessions);
        }
        return result;
    }

    /// <summary>来源白名单判定：未指定来源集合（null）表示全部允许。</summary>
    private static bool Wanted(ImportScanOptions o, ImportSourceKind kind)
        => o.Sources is null || o.Sources.Contains(kind);

    private static void ScanFileSource(
        ImportScanResult result,
        ImportSourceKind kind,
        string path,
        Func<string, ImportSourceStat, List<ImportedSession>> parse)
    {
        var stat = new ImportSourceStat { Kind = kind };
        try
        {
            if (!File.Exists(path))
            {
                result.MissingSources.Add(kind.ToString());
                result.Stats.Add(stat);
                return;
            }
            var fi = new FileInfo(path);
            if (fi.Length > MaxFileBytes)
            {
                result.Errors.Add((kind, "fileTooLarge"));
                result.Stats.Add(stat);
                return;
            }

            byte[] bytes;
            try
            {
                bytes = File.ReadAllBytes(path);
            }
            catch (Exception)
            {
                result.Errors.Add((kind, "fileReadFailed"));
                result.Stats.Add(stat);
                return;
            }

            var sessions = parse(DecodeBytes(bytes), stat);
            result.Sessions.AddRange(sessions);
        }
        catch (ImportScanException ex)
        {
            result.Errors.Add((kind, ex.Code));
        }
        catch (Exception)
        {
            result.Errors.Add((kind, "parseFailed"));
        }
        result.Stats.Add(stat);
    }

    private static string DefaultWinScpIniPath()
        => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "WinSCP.ini");

    private static string DefaultFileZillaPath()
        => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "FileZilla", "sitemanager.xml");

    private static string DefaultOpenSshPath()
        => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
            ".ssh", "config");

    private static string DefaultOwnStorePath()
        => Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
            "iFreeSftp", "hosts.json");

    // =========================================================================
    // 校验、转换、判重
    // =========================================================================

    /// <summary>
    /// 密钥路径校验：UNC 网络路径拒收（红线 3，内网凭据外泄面），回退密码认证并标记
    /// uncPath；路径不存在保留原值并标记 keyNotFound（用户可在预览表修正）。
    /// </summary>
    public static void Validate(ImportedSession s, Func<string, bool> keyFileExists)
    {
        if (s.AuthMode != AuthMode.KeyFile || string.IsNullOrWhiteSpace(s.KeyFilePath)) return;
        if (s.KeyFilePath.StartsWith(@"\\", StringComparison.Ordinal)
            || s.KeyFilePath.StartsWith("//", StringComparison.Ordinal))
        {
            s.KeyFilePath = string.Empty;
            s.AuthMode = AuthMode.Password;
            s.Warning = "uncPath";
            return;
        }
        if (!keyFileExists(s.KeyFilePath)) s.Warning = "keyNotFound";
    }

    /// <summary>ImportedSession → HostProfile（不写存储，落库由 UI 层走 HostStoreService）。</summary>
    public static HostProfile ToHostProfile(ImportedSession s)
        => ToHostProfile(s, HostStoreService.Unprotect);

    /// <summary>
    /// 可注入解密委托的转换实现（单测用）。OwnStorePassword 走 DPAPI 解密：
    /// 结果仍带 enc: 前缀说明解密失败（换机/换用户），fail-safe 清空口令；
    /// 解密成功则赋明文（同机落盘时 Save() 会重新 Protect）。
    /// </summary>
    public static HostProfile ToHostProfile(ImportedSession s, Func<string, string?> unprotect)
    {
        var p = new HostProfile
        {
            Name = string.IsNullOrWhiteSpace(s.Name) ? s.Host : s.Name,
            Host = s.Host,
            Port = s.Port,
            Username = s.Username,
            AuthMode = s.AuthMode,
            KeyFilePath = s.KeyFilePath,
        };
        if (s.OwnStorePassword.Length > 0)
        {
            var plain = unprotect(s.OwnStorePassword) ?? string.Empty;
            p.Password = plain.StartsWith(EncPrefixConst, StringComparison.Ordinal) ? string.Empty : plain;
        }
        return p;
    }

    /// <summary>预览表判重口径：Host:Port:Username（与 HostStoreService.SaveSingle 定位语义一致，区分大小写）。</summary>
    public static string DedupKey(ImportedSession s)
        => DedupKey(s.Host, s.Port, s.Username);

    public static string DedupKey(string host, int port, string username)
        => $"{host.Trim()}:{port}:{username.Trim()}";

    // =========================================================================
    // 私有工具
    // =========================================================================

    private static void AddCapped(List<ImportedSession> list, ImportedSession s, ImportSourceStat stat)
    {
        if (list.Count >= MaxSessions)
        {
            stat.Truncated++;
            return;
        }
        list.Add(s);
        stat.Imported++;
    }

    private static int ParsePort(string? raw, ImportSourceStat stat)
    {
        if (string.IsNullOrWhiteSpace(raw)) return 22;
        if (int.TryParse(raw, NumberStyles.Integer, CultureInfo.InvariantCulture, out var p)
            && p >= 1 && p <= 65535) return p;
        stat.SkippedInvalid++;
        return 0;
    }

    private static string Get(Dictionary<string, string> v, string key)
        => v.TryGetValue(key, out var s) ? s : string.Empty;

    private static string GetString(Dictionary<string, object> v, string key)
        => v.TryGetValue(key, out var o) ? o as string ?? string.Empty : string.Empty;

    /// <summary>"user@host" 拆分；无 @ 或拆分无效时整体视为主机。</summary>
    private static (string User, string Host) SplitUserHost(string raw)
    {
        var at = raw.IndexOf('@');
        if (at <= 0 || at == raw.Length - 1) return (string.Empty, raw);
        return (raw.Substring(0, at), raw.Substring(at + 1));
    }

    private static string LastSegment(string s, char sep)
    {
        var idx = s.LastIndexOf(sep);
        return idx >= 0 ? s.Substring(idx + 1) : s;
    }

    private static string SafeUnescape(string s)
    {
        try
        {
            return Uri.UnescapeDataString(s);
        }
        catch (UriFormatException)
        {
            return s;
        }
    }

    private static string FirstToken(string s)
    {
        var tokens = SplitTokens(s);
        return tokens.Count > 0 ? tokens[0] : string.Empty;
    }

    /// <summary>空白分词，支持双引号包裹（OpenSSH Host "name with space"）。</summary>
    private static List<string> SplitTokens(string s)
    {
        var tokens = new List<string>();
        var inQuote = false;
        var cur = new StringBuilder();
        foreach (var ch in s)
        {
            if (ch == '"')
            {
                inQuote = !inQuote;
                continue;
            }
            if (!inQuote && (ch == ' ' || ch == '\t'))
            {
                if (cur.Length > 0)
                {
                    tokens.Add(cur.ToString());
                    cur.Clear();
                }
                continue;
            }
            cur.Append(ch);
        }
        if (cur.Length > 0) tokens.Add(cur.ToString());
        return tokens;
    }
}
