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
