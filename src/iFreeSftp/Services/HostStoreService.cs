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

    /// <summary>从盘加载覆盖内存工作集；文件缺失或损坏时清空为初始状态。</summary>
    public static void Load()
    {
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
        catch
        {
            // 文件损坏等异常时退回空列表，不让应用启动失败。
            Hosts = new();
        }
    }

    /// <summary>把内存工作集落盘（面板"保存"按钮调用）。</summary>
    public static void Save()
    {
        Directory.CreateDirectory(Path.GetDirectoryName(StorePath)!);
        File.WriteAllText(StorePath, JsonSerializer.Serialize(new HostStore { hosts = Hosts }, JsonOptions));
    }

    /// <summary>丢弃未保存修改，从盘重载（面板未保存关闭时调用）。</summary>
    public static void Reload() => Load();

    private sealed class HostStore
    {
        public List<HostProfile> hosts { get; set; } = new();
    }
}
