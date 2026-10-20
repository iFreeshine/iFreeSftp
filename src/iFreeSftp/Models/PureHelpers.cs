namespace iFreeSftp.Models;

/// <summary>
/// 版本号比较纯函数: 逐段数字比较 ("0.9.0" &gt; "0.8.0"), 段数不足补 0,
/// 非数字段按 0。更新提醒 (RegistrationService) 使用。
/// </summary>
public static class VersionCompare
{
    /// <summary>remote 版本是否严格新于 local。</summary>
    public static bool IsNewer(string remote, string local)
    {
        var r = Parse(remote);
        var l = Parse(local);
        for (var i = 0; i < Math.Max(r.Length, l.Length); i++)
        {
            var rv = i < r.Length ? r[i] : 0;
            var lv = i < l.Length ? l[i] : 0;
            if (rv != lv)
            {
                return rv > lv;
            }
        }

        return false;
    }

    private static int[] Parse(string v)
    {
        return v.Split('.', StringSplitOptions.RemoveEmptyEntries)
            .Select(s => int.TryParse(s, out var n) ? n : 0)
            .ToArray();
    }
}

/// <summary>
/// 主机 tab 标题纯函数: "主机名称 (IP)" 组合。名称为空或与地址相同时只显示
/// 地址; tab 宽度不足由 UI 层省略号截断, hover tooltip 显示本函数全文。
/// </summary>
public static class HostTabNaming
{
    public static string Build(string? name, string? host)
    {
        var n = name?.Trim();
        var a = host?.Trim();
        if (string.IsNullOrEmpty(a))
        {
            a = n ?? string.Empty;
        }

        return string.IsNullOrEmpty(n) || n == a ? a : $"{n} ({a})";
    }
}

/// <summary>Unix 权限位 ↔ 符号串纯函数 (权限对话框显示用)。</summary>
public static class PermissionText
{
    /// <summary>9 位权限 → "rwxr-xr-x" 符号串 (rwx 顺序, 无位补 '-')。</summary>
    public static string Symbolic(int mode)
    {
        var sb = new System.Text.StringBuilder(9);
        (int bit, char c)[] specs =
        {
            (0x100, 'r'), (0x080, 'w'), (0x040, 'x'),
            (0x020, 'r'), (0x010, 'w'), (0x008, 'x'),
            (0x004, 'r'), (0x002, 'w'), (0x001, 'x'),
        };
        foreach (var (bit, c) in specs)
        {
            sb.Append((mode & bit) != 0 ? c : '-');
        }

        return sb.ToString();
    }
}

/// <summary>远程 (SFTP) 路径纯函数。</summary>
public static class RemotePath
{
    /// <summary>远程路径拼接 (根 '/' 兼容: dir TrimEnd 后补分隔符)。</summary>
    public static string Join(string dir, string name)
        => dir.TrimEnd('/') + "/" + name;
}

/// <summary>
/// POSIX shell 单引号转义（关闭-转义-重开）: 防路径含空格/引号/分号时被命令通道
/// 注入或截断。exec 命令模板拼接用户可控参数（路径等）前的唯一入口——
/// SftpSessionService 与 TarAccelService 共用, 集中一处便于单测覆盖恶意输入。
/// </summary>
public static class ShellQuote
{
    /// <summary>单引号包裹并转义内部单引号（' → '\''）。</summary>
    public static string Quote(string arg) => "'" + arg.Replace("'", "'\\''") + "'";
}

/// <summary>断点续传决策动作。</summary>
public enum ResumeAction
{
    /// <summary>从断点追加: 目标已存在且 0 &lt; 目标 &lt; 源（上传=远端半成品 / 下载=本地半成品）。</summary>
    Append,

    /// <summary>全量重传: 目标不存在 / 大小相等 / 续传关闭（保持既有覆盖语义, 零行为变化）。</summary>
    Overwrite,

    /// <summary>目标比源还大: 内容已变的强信号。不静默追加（避免把旧文件缝进新文件），
    /// 也不静默丢弃——本期无覆盖策略(T4)时调用方按 Overwrite 处理并记终端日志，
    /// T4 落地后此分支改交覆盖策略。</summary>
    Conflict,
}

/// <summary>
/// 断点续传决策纯函数（方向无关, 按"源/目标"大小关系判定）:
/// 上传 = Decide(本地大小, 远端大小, 远端存在); 下载 = Decide(远端大小, 本地大小, 本地存在)。
/// 判定保守: 仅"目标严格小于源"才追加, 相等或更大都全量重传——相等时无法区分
/// "恰好传完" 与 "同尺寸的另一份内容", 重传是唯一不丢数据的答案。
/// </summary>
public static class ResumePolicy
{
    /// <summary>按源/目标大小关系决定续传动作。destSize &lt; 0 表示目标不存在。
    /// <paramref name="thresholdBytes"/> 为续传阈值（0 = 不启用）：源低于阈值的小文件
    /// 一律全量重传——重传代价可忽略、续传收益趋零，阈值宜取大不取小（200KB-1MB）。</summary>
    public static ResumeAction Decide(
        long sourceSize, long destSize, bool destExists, bool resumeEnabled, long thresholdBytes = 0)
    {
        if (!resumeEnabled || !destExists || destSize <= 0)
        {
            // 目标不存在 / 空文件 / 续传关闭: 全量。目标为 0 字节时"从 0 追加"等于
            // 全量, 走续传路径只会让队列显示误导性的"续传中"。
            return ResumeAction.Overwrite;
        }

        if (destSize > sourceSize)
        {
            return ResumeAction.Conflict;
        }

        // 小文件阈值: 低于阈值不做断点续传, 全量重传（目标更大仍按 Conflict 记日志）。
        if (sourceSize < thresholdBytes)
        {
            return ResumeAction.Overwrite;
        }

        return destSize == sourceSize ? ResumeAction.Overwrite : ResumeAction.Append;
    }
}

/// <summary>
/// tar 打包加速的纯函数集（命令构造 / 阈值 / 临时命名）: 命令模板与用户可控路径
/// 的拼接集中在此, 每条命令的参数必经 <see cref="ShellQuote"/>; 编排（exec/SFTP/
/// 本地进程）在 Services.TarAccelService, 本类可脱离 IO 单测。
/// </summary>
public static class TarPlan
{
    /// <summary>临时 tar 文件名（远端/本地同名, guid 防碰撞; 前导点隐藏）。</summary>
    public static string TempTarName(string guid) => ".ifree-tar-" + guid + ".tar";

    /// <summary>临时解包目录名（与临时 tar 同 guid, 便于成对清理）。</summary>
    public static string TempDirName(string guid) => ".ifree-tar-" + guid;

    /// <summary>远端路径取父目录（"/" 与顶层路径均安全: "/a/b"→"/a", "/a"→"/"）。</summary>
    public static string RemoteParent(string path)
    {
        var trimmed = path.TrimEnd('/');
        var idx = trimmed.LastIndexOf('/');
        return idx <= 0 ? "/" : trimmed[..idx];
    }

    /// <summary>是否值得走 tar: 开启 + 有文件可传（0 文件空目录由逐文件路径的建目录
    /// 任务保证, 打包无收益）+ 文件数达到阈值。未知计数 -1 一律不走。</summary>
    public static bool ShouldUse(bool enabled, int fileCount, int minFiles)
        => enabled && fileCount > 0 && fileCount >= minFiles;

    /// <summary>服务端打包: tar -cf &lt;tar&gt; -C &lt;dir&gt; .（GNU/bsdtar/busybox 通用, -C 在文件列表前）。</summary>
    public static string BuildPackCommand(string tarPath, string dir)
        => $"tar -cf {ShellQuote.Quote(tarPath)} -C {ShellQuote.Quote(dir)} .";

    /// <summary>服务端解包: mkdir -p &lt;dest&gt; &amp;&amp; tar -xf &lt;tar&gt; -C &lt;dest&gt;
    /// （mkdir 幂等: 目标已存在时无害, 不存在时免 tar 因 -C 目录缺失失败）。</summary>
    public static string BuildUnpackCommand(string tarPath, string destDir)
        => $"mkdir -p {ShellQuote.Quote(destDir)} && tar -xf {ShellQuote.Quote(tarPath)} -C {ShellQuote.Quote(destDir)}";

    /// <summary>清理临时 tar: rm -f（幂等, 清理失败不影响传输结果）。</summary>
    public static string BuildRemoveCommand(string tarPath)
        => $"rm -f {ShellQuote.Quote(tarPath)}";

    /// <summary>清理临时解包目录: rm -rf（上传路径 rename 成功后为误调, 幂等无害）。</summary>
    public static string BuildRemoveDirCommand(string dirPath)
        => $"rm -rf {ShellQuote.Quote(dirPath)}";
}

/// <summary>传输后校验结果（T2）: None = 未校验（功能关闭/非单文件任务）。</summary>
public enum VerifyResultKind { None, Match, Mismatch, Skipped }

/// <summary>校验 hook 返回: 判定结果 + 两侧 hex（Mismatch 详情展示用, 小写）。</summary>
public sealed record VerifyOutcome(
    VerifyResultKind Kind, string LocalHex = "", string RemoteHex = "", string Algorithm = "");

public static class VerifyPolicy
{
    /// <summary>比对判定: 算法不可用或任一侧取不到 → 跳过; 忽略大小写相等 → 一致。</summary>
    public static VerifyResultKind Decide(string? localHex, string? remoteHex, bool algorithmAvailable)
    {
        if (!algorithmAvailable || string.IsNullOrWhiteSpace(localHex) || string.IsNullOrWhiteSpace(remoteHex))
        {
            return VerifyResultKind.Skipped;
        }
        return string.Equals(localHex, remoteHex, StringComparison.OrdinalIgnoreCase)
            ? VerifyResultKind.Match : VerifyResultKind.Mismatch;
    }

    /// <summary>选服务端校验算法: Auto 按 SHA256→SHA512→SM3 顺位回退;
    /// 显式指定不可用返回 null（不擅自换算法, 校验跳过）; 全不可用 null。</summary>
    public static string? PickAlgorithm(string preferred, bool sha256, bool sha512, bool sm3)
    {
        if (preferred != "Auto")
        {
            var ok = preferred switch
            {
                "SHA256" => sha256, "SHA512" => sha512, "SM3" => sm3, _ => false,
            };
            return ok ? preferred : null;
        }
        return (sha256, sha512, sm3) switch
        {
            (true, _, _) => "SHA256", (_, true, _) => "SHA512",
            (_, _, true) => "SM3", _ => null,
        };
    }
}
