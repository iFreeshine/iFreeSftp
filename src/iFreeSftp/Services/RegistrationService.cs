using System.Security.Cryptography;
using System.Text;
using iFreeSftp.Core.Abstractions;

namespace iFreeSftp.Services;

/// <summary>
/// 注册体系: 签名注册码 (验签委托闭源 Core, 见 IRegistrationCore) + 远程信息控制 (吊销名单/公告/勋章开关)。
///
/// 注册码格式: IFREE1.&lt;Base64Url(DER签名)&gt;.&lt;Base64Url(payload)&gt;
/// payload (二进制): [0]版本=1 | [1]等级(1=supporter) | [2..5]签发时间 unix秒 BE |
/// [6..9]到期时间 unix秒 BE (0=永久) | [10]用户名字节长 | [11..]用户名 UTF-8 (≤24 字节)
///
/// 防破解边界 (按"一点防破解"设计, 非重壁垒):
/// - 码本身是签名, 私钥在作者手里——伪造码 = 不可行; 只能改程序绕过验证
/// - 验签真实现随闭源 Core 分发 (社区构建无激活能力, 见 CoreServices.Registration)
/// - 本地只存注册码原文 (DPAPI CurrentUser 加密), 每次加载重新验签——改存储文件 = 验签失败
/// - 等级/有效期取自签名 payload, 无法通过改本地文件升级
///
/// 远程信息: 激活时上报一次 (软绑定统计: 码哈希 + 机器哈希, 不限次数);
/// 启动后 24h 节流拉取 state (吊销名单/公告/勋章开关), 全部失败静默——
/// 离线不影响注册有效性, 远程只能控制勋章显示 (注册与免费版功能本无差别)。
/// </summary>
public static class RegistrationService
{
    /// <summary>
    /// 注册服务端点 (Cloudflare Worker 自定义域, tools/worker/reg-worker.js)。
    /// 不带尾部斜杠; 不可达时远程调用静默失败 (激活/验签为离线, 不受影响)。
    /// </summary>
    public const string EndpointBase = "https://reg.ifrees.top";

    private static readonly string? DataDir = ResolveDataDir();
    private static string RegFilePath => Path.Combine(DataDir!, "registration.dat");
    private static string RemoteCachePath => Path.Combine(DataDir!, "remote.json");

    private static readonly HttpClient Http = CreateHttpClient();

    public static RegistrationState Current { get; private set; } = new();

    /// <summary>远程公告文本 (空 = 无)。</summary>
    public static string RemoteNotice { get; private set; } = "";

    /// <summary>勋章总开关 (远程可关; 未拉到远程数据前默认开)。</summary>
    public static bool BadgeEnabled { get; private set; } = true;

    /// <summary>远程勋章文字 (Worker 下发; 空 = 客户端用本地默认文案)。</summary>
    public static string BadgeText { get; private set; } = "";

    /// <summary>购买/赞助页链接, 双渠道 (Worker 环境变量下发; 空 = UI 隐藏对应按钮):
    /// 国内=爱发电 (微信/支付宝), 国际=Ko-fi (PayPal/卡)。
    /// 走远程下发而非硬编码: 换收款平台只改 Worker 配置, 无需发版。</summary>
    public static string PurchaseUrlCn { get; private set; } = "";

    public static string PurchaseUrlIntl { get; private set; } = "";

    /// <summary>版本探测: 远程下发了更高版本号 (UI 还需看 AppSettingsService.UpdateCheckEnabled 开关)。</summary>
    public static bool UpdateAvailable { get; private set; }

    /// <summary>远程下发的最新版本号 (空 = 无信息)。</summary>
    public static string LatestVersion { get; private set; } = "";

    /// <summary>"有新版本"徽标跳转页 (远程未下发时用官网默认)。</summary>
    public static string UpdateUrl { get; private set; } = "https://ifrees.top";

    /// <summary>注册状态变化 (激活成功/远程吊销) 后 UI 刷新用。</summary>
    public static event Action? StateChanged;

    /// <summary>启动时加载: 无文件未注册; 有文件重验签 (文件被篡改则视为未注册)。</summary>
    public static void Load()
    {
        if (DataDir is null)
        {
            return;
        }

        try
        {
            if (!File.Exists(RegFilePath))
            {
                Current = new RegistrationState();
                return;
            }

            var code = ProtectedData.Unprotect(
                File.ReadAllBytes(RegFilePath), null, DataProtectionScope.CurrentUser);
            TryActivateCore(Encoding.UTF8.GetString(code), save: false);
        }
        catch
        {
            Current = new RegistrationState(); // 文件损坏/被改: 未注册
        }
    }

    /// <summary>激活: 验签 + 有效期检查 → DPAPI 存储 → 异步上报。返回错误消息 (null=成功)。</summary>
    public static string? Activate(string code)
    {
        var err = TryActivateCore(code?.Trim() ?? "", save: true);
        if (err is null)
        {
            _ = ReportActivationAsync();
        }

        return err;
    }

    /// <summary>远程刷新: 吊销/公告/勋章开关/版本探测 (24h 节流; 失败静默)。
    /// force=true 绕过节流 (设置页手动「检查更新」), 走网络刷新并覆盖缓存。
    /// 返回本次是否获得可用远程状态 (缓存命中也算 true; 网络失败 false)。</summary>
    public static async Task<bool> RefreshRemoteStateAsync(bool force = false)
    {
        if (string.IsNullOrEmpty(EndpointBase))
        {
            return false;
        }

        try
        {
            if (!force && File.Exists(RemoteCachePath))
            {
                var cached = File.ReadAllText(RemoteCachePath);
                if (DateTimeOffset.UtcNow - ParseCachedTime(cached) < TimeSpan.FromHours(24))
                {
                    // 节流命中: 仍要把缓存内容应用到内存 (重启后公告/勋章/更新徽标即刻恢复,
                    // 不必等 24h 过期), 只是不重新走网络。
                    ApplyRemoteState(cached[(cached.IndexOf('\n') + 1)..]);
                    return true;
                }
            }

            using var resp = await Http.GetAsync($"{EndpointBase}/v1/state");
            resp.EnsureSuccessStatusCode();
            var json = await resp.Content.ReadAsStringAsync();
            ApplyRemoteState(json);

            File.WriteAllText(
                RemoteCachePath,
                DateTimeOffset.UtcNow.ToUnixTimeSeconds().ToString() + "\n" + json);
            return true;
        }
        catch
        {
            // 远程不可达: 保持本地状态 (设计如此, 离线不影响注册)
            return false;
        }
    }

    /// <summary>构造机器指纹 (软绑定仅统计: 主机名+用户名哈希, 不限次数、不做硬限制)。
    /// SHA-256 前 16 hex (v0.8.0 曾用 MD5, 换哈希统计口径变化属预期, 老计数自然过期)。</summary>
    public static string MachineId()
    {
        var raw = Environment.MachineName + "|" + Environment.UserName;
        return Convert.ToHexString(SHA256.HashData(Encoding.UTF8.GetBytes(raw)))[..16].ToLowerInvariant();
    }

    /// <summary>注册码哈希 (远程吊销名单的 key; 不传码原文)。</summary>
    public static string CodeHash(string code)
    {
        return Convert.ToHexString(
            SHA256.HashData(Encoding.UTF8.GetBytes(code.Trim())))[..24].ToLowerInvariant();
    }

    // ───────────────────────── 内部 ─────────────────────────

    private static string? TryActivateCore(string code, bool save)
    {
        // 验签真实现随闭源 Core 分发; 社区构建 (Stub) 无激活能力
        if (!CoreServices.Registration.IsFullImplementation)
        {
            return LocalizationService.GetString("RegErrOfficialOnly");
        }

        var result = CoreServices.Registration.Verify(code);
        if (!result.Ok)
        {
            return LocalizationService.GetString(result.Error switch
            {
                RegistrationErrorKind.Signature => "RegErrSignature",
                RegistrationErrorKind.Expired => "RegErrExpired",
                _ => "RegErrFormat",
            });
        }

        Current = new RegistrationState
        {
            Registered = true,
            Revoked = false, // 重新激活即视为申诉; 吊销名单在下次远程刷新时再裁决
            UserName = result.UserName,
            IssuedAt = result.IssuedAt,
            ExpiresAt = result.ExpiresAt,
            Tier = result.Tier,
        };

        if (save && DataDir is not null)
        {
            Directory.CreateDirectory(DataDir);
            File.WriteAllBytes(
                RegFilePath,
                ProtectedData.Protect(
                    Encoding.UTF8.GetBytes(code), null, DataProtectionScope.CurrentUser));
        }

        StateChanged?.Invoke();
        return null;
    }

    private static async Task ReportActivationAsync()
    {
        if (string.IsNullOrEmpty(EndpointBase) || !Current.Registered)
        {
            return;
        }

        try
        {
            var body = new
            {
                h = CodeHash(LoadRawCode() ?? ""),
                m = MachineId(),
                u = Current.UserName,
                v = typeof(RegistrationService).Assembly.GetName().Version?.ToString(3) ?? "",
            };
            await Http.PostAsync($"{EndpointBase}/v1/activate", Json(body));
        }
        catch
        {
            // 上报失败不影响激活 (软绑定仅统计)
        }
    }

    private static string? LoadRawCode()
    {
        try
        {
            if (DataDir is null || !File.Exists(RegFilePath))
            {
                return null;
            }

            return Encoding.UTF8.GetString(
                ProtectedData.Unprotect(
                    File.ReadAllBytes(RegFilePath), null, DataProtectionScope.CurrentUser));
        }
        catch
        {
            return null;
        }
    }

    private static void ApplyRemoteState(string json)
    {
        try
        {
            using var doc = System.Text.Json.JsonDocument.Parse(json);
            var root = doc.RootElement;

            if (root.TryGetProperty("badge", out var badge) &&
                badge.TryGetProperty("enabled", out var enabled))
            {
                BadgeEnabled = enabled.GetBoolean();
            }

            if (root.TryGetProperty("badge", out var badge2) &&
                badge2.TryGetProperty("text", out var badgeText))
            {
                BadgeText = badgeText.GetString() ?? "";
            }

            if (root.TryGetProperty("notice", out var notice) &&
                notice.TryGetProperty("text", out var text))
            {
                // 截断超长公告, 防止异常数据撑爆 UI/内存。
                var noticeText = text.GetString() ?? "";
                RemoteNotice = noticeText.Length > 2000 ? noticeText[..2000] : noticeText;
            }

            if (root.TryGetProperty("purchase", out var purchase))
            {
                if (purchase.TryGetProperty("cn", out var pCn))
                {
                    PurchaseUrlCn = pCn.GetString() ?? "";
                }

                if (purchase.TryGetProperty("intl", out var pIntl))
                {
                    PurchaseUrlIntl = pIntl.GetString() ?? "";
                }
            }

            if (root.TryGetProperty("update", out var update))
            {
                if (update.TryGetProperty("latestVersion", out var lv))
                {
                    LatestVersion = lv.GetString() ?? "";
                }

                if (update.TryGetProperty("url", out var uu))
                {
                    var u = uu.GetString();
                    // 入口白名单: 仅接受 http/https, 非法值维持默认官网 (消费点另有 UrlGuard 拦截)。
                    if (UrlGuard.IsSafeWebUrl(u))
                    {
                        UpdateUrl = u!;
                    }
                }

                UpdateAvailable = IsNewerVersion(LatestVersion, LocalVersion());
            }

            if (root.TryGetProperty("revoked", out var revoked) &&
                revoked.ValueKind == System.Text.Json.JsonValueKind.Array)
            {
                var mine = CodeHash(LoadRawCode() ?? "");
                foreach (var item in revoked.EnumerateArray())
                {
                    if (item.GetString() == mine)
                    {
                        Current = Current with { Revoked = true };
                        break;
                    }
                }
            }

            StateChanged?.Invoke();
        }
        catch
        {
            // 响应格式异常: 忽略 (服务端归自己管)
        }
    }

    private static DateTimeOffset ParseCachedTime(string cached)
    {
        var idx = cached.IndexOf('\n');
        return long.TryParse(idx > 0 ? cached[..idx] : "", out var sec)
            ? DateTimeOffset.FromUnixTimeSeconds(sec)
            : DateTimeOffset.MinValue;
    }

    /// <summary>本地程序集版本 (csproj Version, 如 0.8.0 → "0.8.0")。</summary>
    private static string LocalVersion()
    {
        return typeof(RegistrationService).Assembly.GetName().Version?.ToString(3) ?? "0.0.0";
    }

    /// <summary>数字逐段比较, remote &gt; local 才算有更新 (逻辑在 Models.VersionCompare 纯函数, 可单测)。</summary>
    private static bool IsNewerVersion(string remote, string local)
        => Models.VersionCompare.IsNewer(remote, local);

    private static ByteArrayContent Json(object obj)
    {
        return new ByteArrayContent(
            Encoding.UTF8.GetBytes(System.Text.Json.JsonSerializer.Serialize(obj)));
    }

    private static HttpClient CreateHttpClient()
    {
        var c = new HttpClient { Timeout = TimeSpan.FromSeconds(10) };
        c.DefaultRequestHeaders.UserAgent.ParseAdd("iFreeSftp");
        return c;
    }

    /// <summary>数据目录: 有包身份 (MSIX/商店版) 走 ApplicationData, 裸 exe 走 %APPDATA%\iFreeSftp。</summary>
    private static string? ResolveDataDir()
    {
        // ApplicationData.Current 仅在有包身份时可用 (裸 exe 抛异常) —
        // 以可用性探测代替 Package.IsPackaged (部分 SDK 投影未提供该属性)
        try
        {
            return Windows.Storage.ApplicationData.Current.LocalFolder.Path;
        }
        catch
        {
            // 无包身份 → %APPDATA%
        }

        try
        {
            // LocalApplicationData: 与主机配置/设置 (HostStoreService/AppSettingsService) 同目录同命运;
            // 不用 Roaming — 域环境漫游同步过去的数据 DPAPI 也解不开, 徒增垃圾
            return Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "iFreeSftp");
        }
        catch
        {
            return null;
        }
    }
}

/// <summary>注册状态快照 (record + init 属性: 吊销裁决用 with 表达式整体替换)。</summary>
public sealed record RegistrationState
{
    public bool Registered { get; init; }
    public bool Revoked { get; init; }
    public string UserName { get; init; } = "";
    public long IssuedAt { get; init; }
    public long ExpiresAt { get; init; } // 0 = 永久
    public string Tier { get; init; } = "";
}
