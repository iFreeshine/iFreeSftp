using System.Security.Cryptography;
using System.Text;
using System.Text.Encodings.Web;
using System.Text.Json;
using iFreeSftp.Models;

namespace iFreeSftp.Services;

/// <summary>
/// 主机配置存储：内存工作集 + 显式 Save 落盘。
/// 编辑期间 Hosts 允许处于"脏"状态（与面板的显式保存语义配合）：
/// 面板保存时调 Save()，未保存关闭时调 Reload() 从盘重载丢弃修改。
/// </summary>
public static class HostStoreService
{
    private static readonly string StorePath = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "iFreeSftp", "hosts.json");

    /// <summary>串行化存档文件读写（Load/Save/SaveSingle），防止交错读到半写文件。</summary>
    private static readonly object _gate = new();

    /// <summary>
    /// 序列化选项：缩进 + 中文不转义，便于人工检查；反序列化缺字段时
    /// 取 HostProfile 属性初始化器的默认值。
    /// </summary>
    private static readonly JsonSerializerOptions JsonOptions = new()
    {
        WriteIndented = true,
        Encoder = JavaScriptEncoder.UnsafeRelaxedJsonEscaping,
    };

    /// <summary>内存工作集（可能包含未保存的修改）。</summary>
    public static List<HostProfile> Hosts { get; private set; } = new();

    /// <summary>
    /// hosts.json 读取失败时的时间戳备份文件路径（供 UI 提示数据去向）；读取正常为 null。
    /// </summary>
    public static string? LoadFailedBackupPath { get; private set; }

    /// <summary>从盘加载覆盖内存工作集；文件缺失或损坏时备份原文件并清空为初始状态。</summary>
    public static void Load()
    {
        lock (_gate)
        {
            LoadFailedBackupPath = null;
            try
            {
                if (File.Exists(StorePath))
                {
                    var doc = JsonDocument.Parse(File.ReadAllText(StorePath)).RootElement;
                    Hosts = doc.TryGetProperty("hosts", out var hosts)
                        ? JsonSerializer.Deserialize<List<HostProfile>>(hosts.GetRawText(), JsonOptions) ?? new()
                        : new();
                }
                else
                {
                    Hosts = new();
                }
            }
            catch (Exception ex)
            {
                // 文件损坏等异常时备份原文件、退回空列表，不让应用启动失败。
                Hosts = new();
                BackupCorruptedStore(ex);
            }

            // 密码解密进内存工作集（DPAPI 密文；旧版明文原样保留，下次保存时升级为密文）。
            foreach (var h in Hosts)
            {
                h.Password = Unprotect(h.Password);
            }
        }
    }

    /// <summary>把内存工作集落盘（面板"保存"按钮调用）。</summary>
    public static void Save()
    {
        lock (_gate)
        {
            Directory.CreateDirectory(Path.GetDirectoryName(StorePath)!);
            // 持久化快照: 密码以 DPAPI(当前用户) 密文落盘, 内存工作集保持明文。
            var snapshot = Hosts.Select(Snapshot).ToList();
            File.WriteAllText(StorePath, JsonSerializer.Serialize(new HostStore { hosts = snapshot }, JsonOptions));
        }
    }

    /// <summary>单条落盘快照：密码以 DPAPI(当前用户) 密文写入, 内存工作集保持明文。</summary>
    private static HostProfile Snapshot(HostProfile h) => new()
    {
        Name = h.Name,
        Host = h.Host,
        Port = h.Port,
        AuthMode = h.AuthMode,
        KeyFilePath = h.KeyFilePath,
        Username = h.Username,
        Password = Protect(h.Password),
        LocalSyncPath = h.LocalSyncPath,
        RemoteSyncPath = h.RemoteSyncPath,
        SyncEnabled = h.SyncEnabled,
        CompareEnabled = h.CompareEnabled,
        KeepAliveEnabled = h.KeepAliveEnabled,
        KeepAliveIntervalSeconds = h.KeepAliveIntervalSeconds,
        TimeoutSeconds = h.TimeoutSeconds,
        MaxRetries = h.MaxRetries,
        RetryDelaySeconds = h.RetryDelaySeconds,
        HostKeyFingerprint = h.HostKeyFingerprint,
    };

    /// <summary>
    /// 追加主机进盘上存档（复制/导入的"即存"路径）：把传入主机以全字段快照追加进
    /// hosts.json，不动存档中已有记录，也不提交内存工作集里其他未保存修改——
    /// 全量 Save 会把工作集里的脏编辑一并落盘, 使面板"未保存关闭"的 Reload 回滚失效。
    /// 内存工作集的追加由调用方先行完成, 本方法只管盘。返回是否成功写盘。
    /// </summary>
    public static bool AppendHosts(IEnumerable<HostProfile> hosts)
    {
        lock (_gate)
        {
            try
            {
                var items = hosts as ICollection<HostProfile> ?? hosts?.ToList() ?? new List<HostProfile>();
                if (items.Count == 0)
                {
                    return true;
                }

                var doc = File.Exists(StorePath)
                    ? JsonDocument.Parse(File.ReadAllText(StorePath)).RootElement
                    : (JsonElement?)null;
                var stored = doc is not null && doc.Value.TryGetProperty("hosts", out var hostsEl)
                    ? JsonSerializer.Deserialize<List<HostProfile>>(hostsEl.GetRawText(), JsonOptions) ?? new()
                    : new();
                stored.AddRange(items.Select(Snapshot));
                Directory.CreateDirectory(Path.GetDirectoryName(StorePath)!);
                File.WriteAllText(StorePath, JsonSerializer.Serialize(new HostStore { hosts = stored }, JsonOptions));
                return true;
            }
            catch
            {
                // 存档损坏/占用等异常：跳过落盘, 主机仍留在内存工作集, 随显式 Save 落盘。
                return false;
            }
        }
    }

    /// <summary>
    /// 单条落盘：只把该主机确认过的指纹写回存档中对应记录，不动工作集里其他未保存修改，
    /// 也不动该记录的其他字段（端口/密码等未保存编辑仍只存在于内存，随显式 Save 落盘）。
    /// 以 Host+Port+Username 在盘上定位记录（与连接路径的匹配口径一致）；
    /// 盘上找不到（新增未保存 / 关键字段已改未保存）或存档读取失败时不落盘返回 false，
    /// 指纹保留在内存工作集，随下次显式保存一起落盘。盘上密文原样流转，不经 Protect/Unprotect。
    /// </summary>
    public static bool SaveSingle(HostProfile host)
    {
        lock (_gate)
        {
            try
            {
                if (host is null || !File.Exists(StorePath))
                {
                    return false;
                }

                var doc = JsonDocument.Parse(File.ReadAllText(StorePath)).RootElement;
                var stored = doc.TryGetProperty("hosts", out var hostsEl)
                    ? JsonSerializer.Deserialize<List<HostProfile>>(hostsEl.GetRawText(), JsonOptions) ?? new()
                    : new();
                var target = stored.FirstOrDefault(h =>
                    h.Host == host.Host && h.Port == host.Port && h.Username == host.Username);
                if (target is null)
                {
                    return false;
                }

                target.HostKeyFingerprint = host.HostKeyFingerprint;
                File.WriteAllText(StorePath, JsonSerializer.Serialize(new HostStore { hosts = stored }, JsonOptions));
                return true;
            }
            catch
            {
                // 存档损坏/占用等异常：跳过落盘不打断测试流程，指纹留在内存随显式保存落盘。
                return false;
            }
        }
    }

    /// <summary>丢弃未保存修改，从盘重载（面板未保存关闭时调用）。</summary>
    public static void Reload() => Load();

    /// <summary>密文前缀，标识 DPAPI 密文（Base64）。</summary>
    private const string EncPrefix = "enc:";

    /// <summary>
    /// 损坏的存档文件复制为时间戳备份并记录路径与原因（终端日志 + UI 提示）；
    /// 备份失败（权限/占用）时静默，不影响启动。
    /// </summary>
    private static void BackupCorruptedStore(Exception ex)
    {
        try
        {
            if (!File.Exists(StorePath))
            {
                return;
            }

            var backupPath = StorePath + $".corrupt-{DateTime.Now:yyyyMMdd-HHmmss}.bak";
            File.Copy(StorePath, backupPath);
            LoadFailedBackupPath = backupPath;
            TerminalLogService.Log(string.Format(
                LocalizationService.GetString("LogHostStoreCorrupt"), backupPath, ex.Message));
        }
        catch
        {
            // 备份失败也照常启动
        }
    }

    /// <summary>
    /// DPAPI（当前用户）加密；空值原样返回。仅用于落盘快照，内存保持明文。
    /// 幂等：已带密文前缀的原样返回——解密失败而保留在内存里的原密文（见 Unprotect）
    /// 流经任何一次 Save 都不会被二次加密或清空，杜绝"解不开 → 存空 → 永久丢失"。
    /// internal 供 tests 链接编译后直测（零漂移）。
    /// </summary>
    internal static string Protect(string password)
    {
        if (string.IsNullOrEmpty(password))
        {
            return password;
        }

        if (password.StartsWith(EncPrefix, StringComparison.Ordinal))
        {
            return password;
        }

        var cipher = ProtectedData.Protect(
            Encoding.UTF8.GetBytes(password), null, DataProtectionScope.CurrentUser);
        return EncPrefix + Convert.ToBase64String(cipher);
    }

    /// <summary>
    /// 解密 DPAPI 密文；无前缀视为旧版明文原样返回（兼容升级）；
    /// 密文破损或换用户/换机无法解密时保留原密文并记日志（原样流经 Save 落盘，
    /// 换回原 Windows 账户仍可恢复；连接时认证失败属 fail-closed），不让启动失败。
    /// internal 供 tests 链接编译后直测（零漂移）。
    /// </summary>
    internal static string Unprotect(string? stored)
    {
        if (string.IsNullOrEmpty(stored) || !stored.StartsWith(EncPrefix, StringComparison.Ordinal))
        {
            return stored ?? "";
        }

        try
        {
            var plain = ProtectedData.Unprotect(
                Convert.FromBase64String(stored[EncPrefix.Length..]), null, DataProtectionScope.CurrentUser);
            return Encoding.UTF8.GetString(plain);
        }
        catch (Exception ex) when (ex is FormatException or CryptographicException)
        {
            TerminalLogService.Log(string.Format(
                LocalizationService.GetString("LogPasswordUndecryptable"), ex.Message));
            return stored;
        }
    }

    private sealed class HostStore
    {
        public List<HostProfile> hosts { get; set; } = new();
    }
}
