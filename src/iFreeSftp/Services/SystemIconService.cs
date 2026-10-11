using System.Runtime.InteropServices;
using Microsoft.UI.Xaml.Media.Imaging;
using Vanara.PInvoke;
using Windows.Graphics.Imaging;
using Windows.Storage.Streams;
using static Vanara.PInvoke.Gdi32;
using static Vanara.PInvoke.Shell32;
using static Vanara.PInvoke.User32;

namespace iFreeSftp.Services;

/// <summary>
/// 系统文件图标服务: 经 Vanara SHGetFileInfo 取 Windows 真实文件图标,
/// HICON → 32bpp DIB → SoftwareBitmap(预乘 α) → SoftwareBitmapSource, 双层缓存
/// (位图按扩展名/路径, 源按位图实例)。仅文件列表条目使用, UI 装饰图标不受影响;
/// 任意一步失败返回 null, 由调用方回退通用字形。查询均发生在 UI 线程, 用普通 Dictionary 即可。
/// </summary>
public static class SystemIconService
{
    // 位图缓存: 键 → 图标位图 (null = 取不到, 也记住, 避免反复查询失败)。
    private static readonly Dictionary<string, SoftwareBitmap?> _bitmapCache = new();

    // 类型名缓存: 键 ("dir" / "ext:<小写扩展名>") → SHGFI_TYPENAME 文本 (空 = 取不到)。
    private static readonly Dictionary<string, string> _typeNameCache = new();

    // 源缓存: 每份位图只创建一个 SoftwareBitmapSource, 供同键的所有条目共享。
    private static readonly Dictionary<SoftwareBitmap, SoftwareBitmapSource> _sourceCache = new();

    // 本地图标随文件不同而不同的扩展名 (exe 自带图标等), 按完整路径取与缓存。
    private static readonly HashSet<string> _perFileIconExtensions = new(StringComparer.OrdinalIgnoreCase)
    {
        ".exe", ".dll", ".ico", ".msi", ".scr", ".cpl",
    };

    /// <summary>
    /// 取条目的系统图标位图。isLocalPath=条目来自本地文件系统 (FullPath 为本机真实路径);
    /// 远程条目只能按扩展名取 (SHGFI_USEFILEATTRIBUTES, 文件无需存在)。
    /// 跨平台: 非 Windows 上壳层图标不可用, 返回 null 由调用方回退通用字形。
    /// </summary>
    public static SoftwareBitmap? GetBitmap(bool isDirectory, string? name, string fullPath, bool isLocalPath)
    {
        if (!OperatingSystem.IsWindows())
        {
            return null; // 非 Windows 无壳层图标, 调用方回退通用字形。
        }
        name ??= string.Empty;
        // 键: 盘符根目录 (各盘图标不同) 与自带图标的本地文件按完整路径; 其余按扩展名。
        bool perPath = isLocalPath
            && (fullPath.EndsWith(":\\", StringComparison.Ordinal)
                || (!isDirectory && _perFileIconExtensions.Contains(Path.GetExtension(name))));
        string key = perPath
            ? "path:" + fullPath.ToUpperInvariant()
            : isDirectory ? "dir" : "ext:" + Path.GetExtension(name).ToLowerInvariant();

        if (_bitmapCache.TryGetValue(key, out var cached))
        {
            return cached;
        }

        SoftwareBitmap? bitmap = null;
        try
        {
            // 本地条目传完整路径 (文件真实存在, 壳层按真实属性/扩展名解析出准确图标;
            // 若只传文件名且不带 SHGFI_USEFILEATTRIBUTES, 壳层会到进程当前目录找文件而失败);
            // 远程条目只传文件名并带 SHGFI_USEFILEATTRIBUTES, 按扩展名解析 (文件无需存在)。
            bitmap = QueryBitmap(isLocalPath ? fullPath : name, isDirectory, !isLocalPath);
        }
        catch
        {
            // 图标获取失败 (异常路径字符等) 静默降级, 让调用方回退通用字形。
        }
        _bitmapCache[key] = bitmap;
        return bitmap;
    }

    // 非 Windows 平台 (Linux/macOS 本机) 无壳层与注册表, 内置类别映射表解析类型名。
    // 文案按类别粒度 (资源管理器逐扩展名精确名仅在 Windows 链可得), key 全小写。
    private static readonly Dictionary<string, string> _unixExtTypeMap = new(StringComparer.OrdinalIgnoreCase)
    {
        [".txt"] = "TextFileTypeName", [".md"] = "TextFileTypeName", [".log"] = "TextFileTypeName",
        [".ini"] = "TextFileTypeName", [".cfg"] = "TextFileTypeName", [".conf"] = "TextFileTypeName",
        [".rst"] = "TextFileTypeName",
        [".pdf"] = "PdfFileTypeName",
        [".png"] = "ImageFileTypeName", [".jpg"] = "ImageFileTypeName", [".jpeg"] = "ImageFileTypeName",
        [".gif"] = "ImageFileTypeName", [".bmp"] = "ImageFileTypeName", [".webp"] = "ImageFileTypeName",
        [".svg"] = "ImageFileTypeName", [".ico"] = "ImageFileTypeName", [".tif"] = "ImageFileTypeName",
        [".tiff"] = "ImageFileTypeName", [".heic"] = "ImageFileTypeName",
        [".mp4"] = "VideoFileTypeName", [".mkv"] = "VideoFileTypeName", [".avi"] = "VideoFileTypeName",
        [".mov"] = "VideoFileTypeName", [".webm"] = "VideoFileTypeName", [".wmv"] = "VideoFileTypeName",
        [".flv"] = "VideoFileTypeName", [".m4v"] = "VideoFileTypeName",
        [".mp3"] = "AudioFileTypeName", [".wav"] = "AudioFileTypeName", [".flac"] = "AudioFileTypeName",
        [".ogg"] = "AudioFileTypeName", [".aac"] = "AudioFileTypeName", [".m4a"] = "AudioFileTypeName",
        [".opus"] = "AudioFileTypeName",
        [".zip"] = "ArchiveFileTypeName", [".rar"] = "ArchiveFileTypeName", [".7z"] = "ArchiveFileTypeName",
        [".tar"] = "ArchiveFileTypeName", [".gz"] = "ArchiveFileTypeName", [".bz2"] = "ArchiveFileTypeName",
        [".xz"] = "ArchiveFileTypeName", [".tgz"] = "ArchiveFileTypeName",
        [".sh"] = "ScriptFileTypeName", [".bash"] = "ScriptFileTypeName", [".zsh"] = "ScriptFileTypeName",
        [".fish"] = "ScriptFileTypeName", [".py"] = "ScriptFileTypeName", [".pl"] = "ScriptFileTypeName",
        [".rb"] = "ScriptFileTypeName", [".ps1"] = "ScriptFileTypeName",
        [".c"] = "SourceCodeFileTypeName", [".cpp"] = "SourceCodeFileTypeName", [".h"] = "SourceCodeFileTypeName",
        [".hpp"] = "SourceCodeFileTypeName", [".cs"] = "SourceCodeFileTypeName", [".java"] = "SourceCodeFileTypeName",
        [".go"] = "SourceCodeFileTypeName", [".rs"] = "SourceCodeFileTypeName", [".swift"] = "SourceCodeFileTypeName",
        [".kt"] = "SourceCodeFileTypeName", [".ts"] = "SourceCodeFileTypeName", [".js"] = "SourceCodeFileTypeName",
        [".html"] = "SourceCodeFileTypeName", [".css"] = "SourceCodeFileTypeName", [".json"] = "SourceCodeFileTypeName",
        [".xml"] = "SourceCodeFileTypeName", [".sql"] = "SourceCodeFileTypeName", [".lua"] = "SourceCodeFileTypeName",
        [".php"] = "SourceCodeFileTypeName",
        [".doc"] = "DocumentFileTypeName", [".docx"] = "DocumentFileTypeName", [".xls"] = "DocumentFileTypeName",
        [".xlsx"] = "DocumentFileTypeName", [".ppt"] = "DocumentFileTypeName", [".pptx"] = "DocumentFileTypeName",
        [".odt"] = "DocumentFileTypeName", [".ods"] = "DocumentFileTypeName", [".odp"] = "DocumentFileTypeName",
        [".rtf"] = "DocumentFileTypeName",
        [".so"] = "LibraryFileTypeName", [".dylib"] = "LibraryFileTypeName", [".a"] = "LibraryFileTypeName",
        [".lnk"] = "SymlinkTypeName",
    };

    // Unix 惯例无扩展名约定文件名 (精确名) → 资源 key。
    private static readonly Dictionary<string, string> _unixNameTypeMap = new(StringComparer.Ordinal)
    {
        ["README"] = "TextFileTypeName", ["LICENSE"] = "TextFileTypeName", ["COPYING"] = "TextFileTypeName",
        ["CHANGELOG"] = "TextFileTypeName", ["NOTICE"] = "TextFileTypeName", ["INSTALL"] = "TextFileTypeName",
        ["TODO"] = "TextFileTypeName", ["VERSION"] = "TextFileTypeName", ["AUTHORS"] = "TextFileTypeName",
        ["NEWS"] = "TextFileTypeName",
        ["Makefile"] = "ScriptFileTypeName", ["Dockerfile"] = "ScriptFileTypeName",
    };

    /// <summary>
    /// 取资源管理器同款"类型"列文本: 文件夹 = "文件文件夹"; 符号链接 = "快捷方式"
    /// (远程 Linux/macOS 语义, 对齐 .lnk); 无扩展名可执行文件 = "应用程序" (权限位,
    /// Linux 惯例的无扩展名程序); 其余文件按扩展名解析类型名。
    /// Windows 链: SHGetFileInfo(SHGFI_TYPENAME, 部分精简系统返回空) → 注册表直查
    /// (UserChoice ProgID → 扩展名默认 ProgID → FriendlyTypeName [SHLoadIndirectString]
    /// → ProgID 默认值, 即资源管理器的真实数据源), 结果按扩展名缓存; 均取不到时回退
    /// "文件"文案。本地与远程条目统一走此路径。
    /// 跨平台兼容 (应用本体未来可能编译到 Linux/macOS): 非 Windows 无壳层/注册表,
    /// 走内置类别映射表 (Unix 约定文件名 → 扩展名 → 兜底), 不触达 Windows 链。
    /// symlink/exe 分支在缓存查询前短路 (缓存键只有扩展名, 否则会污染)。
    /// </summary>
    public static string GetTypeName(bool isDirectory, string name, bool isExecutable = false, bool isSymlink = false)
    {
        if (isDirectory)
        {
            return LocalizationService.GetString("FolderTypeName");
        }
        if (isSymlink)
        {
            return LocalizationService.GetString("SymlinkTypeName");
        }
        string ext = Path.GetExtension(name).ToLowerInvariant();
        if (isExecutable && ext.Length == 0)
        {
            return LocalizationService.GetString("ExecutableTypeName");
        }
        if (!OperatingSystem.IsWindows())
        {
            // 非 Windows: 无 SHGetFileInfo/注册表, 内置映射表 + 兜底 (无缓存, 查表即结果)。
            if (_unixNameTypeMap.TryGetValue(name, out var byName))
            {
                return LocalizationService.GetString(byName);
            }
            if (_unixExtTypeMap.TryGetValue(ext, out var byExt))
            {
                return LocalizationService.GetString(byExt);
            }
            return ext.Length == 0
                ? LocalizationService.GetString("FileType")
                : string.Format(LocalizationService.GetString("ExtFileFormat"), ext.TrimStart('.').ToUpperInvariant());
        }
        if (!_typeNameCache.TryGetValue(ext, out var shellName))
        {
            shellName = QueryTypeNameFromShell(name);
            if (shellName.Length == 0)
            {
                shellName = QueryTypeNameFromRegistry(ext) ?? string.Empty;
            }
            _typeNameCache[ext] = shellName;
        }
        // 全链路取不到: 资源管理器惯例显示 "<EXT> 文件" (无扩展名显示"文件")。
        return shellName.Length > 0
            ? shellName
            : ext.Length == 0
                ? LocalizationService.GetString("FileType")
                : string.Format(LocalizationService.GetString("ExtFileFormat"), ext.TrimStart('.').ToUpperInvariant());
    }

    /// <summary>SHGetFileInfo 取类型名 (SHGFI_TYPENAME + USEFILEATTRIBUTES, 文件无需存在)。</summary>
    private static string QueryTypeNameFromShell(string name)
    {
        try
        {
            var sfi = new SHFILEINFO();
            if (SHGetFileInfo(
                    name,
                    FileAttributes.Normal,
                    ref sfi,
                    SHFILEINFO.Size,
                    SHGFI.SHGFI_TYPENAME | SHGFI.SHGFI_USEFILEATTRIBUTES) != IntPtr.Zero)
            {
                return sfi.szDisplayName?.Trim() ?? string.Empty;
            }
        }
        catch
        {
            // 异常路径字符等静默降级, 交注册表路径兜底。
        }
        return string.Empty;
    }

    /// <summary>SHLoadIndirectString: 解析 "@%SystemRoot%\..., -资源ID" 形式的间接字符串。</summary>
    [DllImport("shlwapi.dll", CharSet = CharSet.Unicode)]
    private static extern int SHLoadIndirectString(string pszSource, System.Text.StringBuilder pszOut, uint cchOut, IntPtr ppvReserved);

    /// <summary>注册表直查类型名 (资源管理器"类型"列的真实数据源), 取不到返回 null。</summary>
    private static string? QueryTypeNameFromRegistry(string ext)
    {
        try
        {
            if (ext.Length == 0)
            {
                return null;
            }
            // 1) 用户自选关联 (UserChoice ProgID) → 解析不出显示名时 (如指向无显示名的
            //    Applications\xxx) → 2) 扩展名默认 ProgID。
            string? progid;
            string? name = null;
            using (var uc = Microsoft.Win32.Registry.CurrentUser.OpenSubKey(
                @"Software\Microsoft\Windows\CurrentVersion\Explorer\FileExts\" + ext + @"\UserChoice"))
            {
                progid = uc?.GetValue("Progid") as string;
            }
            if (!string.IsNullOrEmpty(progid))
            {
                name = ReadProgIdTypeName(progid);
            }
            if (string.IsNullOrEmpty(name))
            {
                using var ek = Microsoft.Win32.Registry.ClassesRoot.OpenSubKey(ext);
                progid = ek?.GetValue(null) as string;
                if (!string.IsNullOrEmpty(progid))
                {
                    name = ReadProgIdTypeName(progid);
                }
            }
            return name;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>读 ProgID 的显示类型名: FriendlyTypeName ("@dll,-资源" 间接串本地化解析)
    /// → ProgID 默认值 (旧式直接为显示名)。取不到返回 null。</summary>
    private static string? ReadProgIdTypeName(string progid)
    {
        try
        {
            using var pk = Microsoft.Win32.Registry.ClassesRoot.OpenSubKey(progid);
            if (pk is null)
            {
                return null;
            }
            if (pk.GetValue("FriendlyTypeName") is string friendly && friendly.Length > 0)
            {
                if (friendly.StartsWith('@'))
                {
                    var sb = new System.Text.StringBuilder(1024);
                    return SHLoadIndirectString(friendly, sb, (uint)sb.Capacity, IntPtr.Zero) == 0 && sb.Length > 0
                        ? sb.ToString()
                        : null;
                }
                return friendly;
            }
            return pk.GetValue(null) as string;
        }
        catch
        {
            return null;
        }
    }

    /// <summary>位图 → 可绑定 Image 的共享 SoftwareBitmapSource (每份位图只 SetBitmapAsync 一次)。</summary>
    public static async Task<SoftwareBitmapSource?> ToImageSourceAsync(SoftwareBitmap bitmap)
    {
        if (_sourceCache.TryGetValue(bitmap, out var ready))
        {
            return ready;
        }

        var source = new SoftwareBitmapSource();
        try
        {
            await source.SetBitmapAsync(bitmap);
        }
        catch
        {
            return null;
        }
        _sourceCache[bitmap] = source;
        return source;
    }

    /// <summary>SHGetFileInfo 取 HICON → 解出 32bpp 预乘位图。</summary>
    private static SoftwareBitmap? QueryBitmap(string path, bool isDirectory, bool useFileAttributes)
    {
        var flags = SHGFI.SHGFI_ICON | SHGFI.SHGFI_LARGEICON;
        var attributes = isDirectory ? FileAttributes.Directory : FileAttributes.Normal;
        if (useFileAttributes)
        {
            flags |= SHGFI.SHGFI_USEFILEATTRIBUTES;
        }

        var sfi = new SHFILEINFO();
        if (SHGetFileInfo(path, attributes, ref sfi, SHFILEINFO.Size, flags) == IntPtr.Zero
            || sfi.hIcon.IsInvalid)
        {
            return null;
        }

        // HICON 为裸句柄包装 (非 SafeHandle), 用完需显式 DestroyIcon。
        var hicon = sfi.hIcon;
        try
        {
            return HiconToBitmap(hicon);
        }
        finally
        {
            _ = DestroyIcon(hicon);
        }
    }

    /// <summary>HICON → ICONINFO 色位图 → GetDIBits 32bpp 顶行在前 → 修复 α 并预乘 → SoftwareBitmap。</summary>
    private static SoftwareBitmap? HiconToBitmap(HICON hicon)
    {
        var info = new User32.ICONINFO(); // Vanara 5 中 ICONINFO 为类, 传入实例由 API 填充
        GetIconInfo(hicon, info);
        using (info) // ICONINFO.Dispose 负责删除色位图/掩码位图句柄
        {
            if (info.hbmColor.IsInvalid)
            {
                return null; // 纯掩码单色图标, 交给字形兜底
            }

            using var hdc = GetDC();
            var bmi = new BITMAPINFO();
            bmi.bmiHeader.biSize = (uint)Marshal.SizeOf<BITMAPINFOHEADER>();
            // 第一次调用不取像素, 由 GDI 回填真实宽高 (随进程 DPI, "大图标"不一定是 32×32)。
            if (GetDIBits(hdc, info.hbmColor, 0, 0, null, ref bmi, DIBColorMode.DIB_RGB_COLORS) == 0)
            {
                return null;
            }
            int width = (int)bmi.bmiHeader.biWidth;
            int height = Math.Abs((int)bmi.bmiHeader.biHeight);
            if (width <= 0 || height <= 0)
            {
                return null;
            }

            bmi.bmiHeader.biHeight = -height; // 负高 = 顶行在前, 与 XAML 像素序一致
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = 0; // BI_RGB
            var pixels = new byte[width * height * 4];
            if (GetDIBits(hdc, info.hbmColor, 0, (uint)height, pixels, ref bmi, DIBColorMode.DIB_RGB_COLORS) != height)
            {
                return null;
            }

            // 旧式掩码图标所有 α=0 → 视为全不透明; 否则按直通 α 预乘 (SoftwareBitmapSource 要求预乘)。
            bool hasAlpha = false;
            for (int i = 3; i < pixels.Length; i += 4)
            {
                if (pixels[i] != 0)
                {
                    hasAlpha = true;
                    break;
                }
            }
            if (!hasAlpha)
            {
                for (int i = 3; i < pixels.Length; i += 4)
                {
                    pixels[i] = 255;
                }
            }
            for (int i = 0; i < pixels.Length; i += 4)
            {
                int a = pixels[i + 3];
                if (a == 255)
                {
                    continue;
                }
                pixels[i] = (byte)((pixels[i] * a + 127) / 255);
                pixels[i + 1] = (byte)((pixels[i + 1] * a + 127) / 255);
                pixels[i + 2] = (byte)((pixels[i + 2] * a + 127) / 255);
            }

            using var writer = new DataWriter();
            writer.WriteBytes(pixels);
            return SoftwareBitmap.CreateCopyFromBuffer(
                writer.DetachBuffer(), BitmapPixelFormat.Bgra8, width, height, BitmapAlphaMode.Premultiplied);
        }
    }
}
