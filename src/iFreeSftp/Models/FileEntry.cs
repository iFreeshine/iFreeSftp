namespace iFreeSftp.Models;

/// <summary>
/// 文件列表条目模型，本地与远程共用。
/// </summary>
public sealed class FileEntry
{
    /// <summary>显示名称。</summary>
    public string Name { get; set; } = string.Empty;

    /// <summary>完整路径（本地绝对路径或远程绝对路径）。</summary>
    public string FullPath { get; set; } = string.Empty;

    /// <summary>是否为目录。</summary>
    public bool IsDirectory { get; set; }

    /// <summary>是否可执行（远程按 SFTP 权限位解析；Windows 本地恒 false，走扩展名链；Unix 本机按权限位解析）。</summary>
    public bool IsExecutable { get; set; }

    /// <summary>是否为符号链接（远程按 SFTP 属性解析；Windows 本地恒 false，.lnk 由扩展名链解析；Unix 本机按 LinkTarget 解析）。</summary>
    public bool IsSymbolicLink { get; set; }

    /// <summary>
    /// 权限属性串，远程 (Linux) 为 "drwxr-xr-x" 形式的 10 字符串（类型位 + 三组 rwx），
    /// 本地 Windows 为空——权限列仅远程模式显示。
    /// </summary>
    public string Permissions { get; set; } = string.Empty;

    /// <summary>字节大小，目录为 0。</summary>
    public long Length { get; set; }

    /// <summary>最后修改时间，本地为 UTC，远程取决于服务器时区。</summary>
    public DateTimeOffset LastModified { get; set; }

    /// <summary>人类可读大小，例如 "1.2 MB"。</summary>
    public string DisplaySize => IsDirectory ? string.Empty : FormatSize(Length);

    /// <summary>
    /// 修改时间字符串。无有效时间戳时显示空 (盘符根列表没有单一修改时间,
    /// 与资源管理器对驱动器留空一致), 避免把默认值格式化成 0001-01-01。
    /// </summary>
    public string DisplayModified => LastModified.Year <= 1
        ? string.Empty
        : LastModified.LocalDateTime.ToString("yyyy-MM-dd HH:mm");

    /// <summary>类型文本，由页面本地化后设置。</summary>
    public string TypeText { get; set; } = string.Empty;

    /// <summary>字节大小 → 人类可读文本 ("1.2 MB"), 列表条目与状态栏统计共用。</summary>
    public static string FormatSize(long bytes)
    {
        string[] units = { "B", "KB", "MB", "GB", "TB" };
        double size = bytes;
        int unit = 0;
        while (size >= 1024 && unit < units.Length - 1)
        {
            size /= 1024;
            unit++;
        }
        return $"{size:0.##} {units[unit]}";
    }
}
