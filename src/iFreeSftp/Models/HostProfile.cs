using System.Text.Json;
using System.Text.Json.Serialization;

namespace iFreeSftp.Models;

/// <summary>登录方式（密码 / 私钥文件）。</summary>
public enum AuthMode
{
    /// <summary>密码登录（默认）。</summary>
    Password,

    /// <summary>私钥文件登录。</summary>
    KeyFile,
}

/// <summary>
/// AuthMode 的 JSON 转换：读写均为 "Password"/"KeyFile" 字符串（与旧版存档格式
/// 一致，老版本程序仍可读取新存档）；未知值/非字符串回退 Password，
/// 防止单条坏值让整个 hosts.json 反序列化失败。
/// </summary>
public sealed class AuthModeJsonConverter : JsonConverter<AuthMode>
{
    public override AuthMode Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
        => reader.TokenType == JsonTokenType.String
            && Enum.TryParse(reader.GetString(), out AuthMode mode)
            ? mode
            : AuthMode.Password;

    public override void Write(Utf8JsonWriter writer, AuthMode value, JsonSerializerOptions options)
        => writer.WriteStringValue(value.ToString());
}

/// <summary>
/// 主机配置：一个 SSH 主机的完整连接与同步/传输设置。
/// 持久化为 %LOCALAPPDATA%\iFreeSftp\hosts.json（见 HostStoreService）。
/// </summary>
public sealed class HostProfile
{
    /// <summary>项目名称（列表展示名）。</summary>
    public string Name { get; set; } = string.Empty;

    /// <summary>主机 IP / 域名。</summary>
    public string Host { get; set; } = string.Empty;

    /// <summary>端口，默认 22。</summary>
    public int Port { get; set; } = 22;

    /// <summary>登录方式（默认密码）。JSON 读写为字符串（见 <see cref="AuthModeJsonConverter"/>）。</summary>
    [JsonConverter(typeof(AuthModeJsonConverter))]
    public AuthMode AuthMode { get; set; } = AuthMode.Password;

    /// <summary>私钥文件路径（AuthMode = KeyFile 时生效）。</summary>
    public string KeyFilePath { get; set; } = string.Empty;

    /// <summary>登录名。</summary>
    public string Username { get; set; } = string.Empty;

    /// <summary>
    /// 登录密码。内存中为明文（运行时需要）；持久化时以 DPAPI（当前用户）加密落盘，
    /// 配置文件中不出现明文（见 HostStoreService）。
    /// </summary>
    public string Password { get; set; } = string.Empty;

    /// <summary>本地同步路径。</summary>
    public string LocalSyncPath { get; set; } = string.Empty;

    /// <summary>远程同步路径。</summary>
    public string RemoteSyncPath { get; set; } = string.Empty;

    /// <summary>是否开启同步（总开关，关闭时路径输入框禁用）。</summary>
    public bool SyncEnabled { get; set; }

    /// <summary>是否开启同步比较。</summary>
    public bool CompareEnabled { get; set; }

    /// <summary>是否开启保活（keep alive）。</summary>
    public bool KeepAliveEnabled { get; set; }

    /// <summary>保活间隔（秒），与 SftpSessionService 默认值一致。</summary>
    public int KeepAliveIntervalSeconds { get; set; } = 15;

    /// <summary>超时秒数：0 表示禁用超时，默认 6000。</summary>
    public int TimeoutSeconds { get; set; } = 6000;

    /// <summary>最大重试次数（0-99），默认 2。</summary>
    public int MaxRetries { get; set; } = 2;

    /// <summary>登录重试延迟（秒，0-999），默认 5。</summary>
    public int RetryDelaySeconds { get; set; } = 5;

    /// <summary>
    /// 已信任的主机密钥指纹（SHA256, ssh 命令同格式）。首次正式连接确认后记录，
    /// 后续连接自动校验；服务器密钥变化（指纹不匹配）时拒绝连接。
    /// </summary>
    public string HostKeyFingerprint { get; set; } = string.Empty;

    /// <summary>IP:端口 展示副标题（仅 UI 展示, 不序列化; 卡片用）。</summary>
    [System.Text.Json.Serialization.JsonIgnore]
    public string Subtitle => string.IsNullOrEmpty(Host) ? string.Empty : $"{Host}:{Port}";

    /// <summary>值语义浅拷贝：全部属性为字符串/数值/布尔，浅拷贝即为完整复制（卡片"复制"菜单用）。</summary>
    public HostProfile Clone() => (HostProfile)MemberwiseClone();
}
