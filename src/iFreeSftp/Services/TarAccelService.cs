using System.Diagnostics;
using iFreeSftp.Models;

namespace iFreeSftp.Services;

/// <summary>
/// tar 打包加速（目录传输的快速路径）: 逐文件 SFTP 的协议往返在海量小文件下
/// dominated（每文件 open/close/stat + 一次数据流），改走"单流打包"——
///
/// 上传（本地→远程）: 本地 tar 进程 stdout **直写**远端临时 .tar（经传输连接，
///   不在本地落临时文件），再 exec 解包——目标不存在时先解到同级隐藏临时目录
///   再 rename（原子切换）; 已存在时直接解包覆盖（与逐文件语义一致）。
/// 下载（远程→本地）: 远端 exec tar 打包到 /tmp 临时文件，SFTP 下载该**单文件**,
///   本地 tar 解到临时目录后 rename/合并，最后删远端临时文件。
///
/// 任一环节不可用——本地无 tar / Windows 服务器 / 服务端无 tar / 空间不足 /
/// 命令失败——返回 false，调用方回退逐文件传输（既有行为，结果语义不变）。
///
/// ⚠️ tar 模式是**整目录单任务**: 不支持文件级断点续传（T1），失败整体重试;
/// 调用方（FileView）在失败信息中附此提示。语义差异（均为改进方向）:
/// 保留符号链接（不跟随）、保留 mtime 与权限位（SFTP 逐文件上传两者皆丢失）。
/// </summary>
public static class TarAccelService
{
    /// <summary>服务端 tar 打包/解包命令超时（大目录可达分钟级; 10 分钟封顶）。</summary>
    private const int ExecTimeoutSeconds = 600;

    /// <summary>远端文件计数 / 清理命令超时。</summary>
    private const int ExecQuickTimeoutSeconds = 60;

    /// <summary>远端临时 tar 存放目录（POSIX 标准临时目录; 不可写时命令失败自动回退）。</summary>
    private const string RemoteTempDir = "/tmp";

    /// <summary>本地 tar 可用性缓存（进程级探测一次; Win10 1803+ / macOS / Linux 均有）。</summary>
    private static bool? _localTarAvailable;

    /// <summary>本地 tar 是否可用（`tar --version` 探测一次并缓存）。</summary>
    public static bool IsLocalTarAvailable()
    {
        if (_localTarAvailable is { } cached)
        {
            return cached;
        }

        try
        {
            using var process = StartLocalTar("--version");
            process.WaitForExit(5000);
            _localTarAvailable = process.ExitCode == 0;
        }
        catch
        {
            // tar 不在 PATH（老版本 Windows / 精简镜像）: 不可用
            _localTarAvailable = false;
        }

        return _localTarAvailable.Value;
    }

    /// <summary>目录传输是否值得走 tar（设置开关 + 文件数阈值; 未知计数 -1 不走）。</summary>
    public static bool ShouldUse(int fileCount)
        => TarPlan.ShouldUse(AppSettingsService.TarAccelEnabled, fileCount, AppSettingsService.TarAccelMinFiles);

    /// <summary>
    /// 统计远端目录文件数（一条 find | wc -l; 探测失败/非 POSIX/解析失败返回 -1）。
    /// 下载方向的 tar 判定用: 避免为"一个大文件"白付打包与双倍磁盘开销。
    /// </summary>
    public static async Task<int> CountRemoteFilesAsync(SftpSessionService session, string remoteDir)
    {
        if (!await session.IsPosixServerAsync())
        {
            return -1;
        }

        try
        {
            var (exit, output, _) = await session.RunCommandAsync(
                $"find {ShellQuote.Quote(remoteDir)} -type f | wc -l", ExecQuickTimeoutSeconds);
            if (exit != 0)
            {
                return -1;
            }

            return int.TryParse(output.Trim(), out var count) && count >= 0 ? count : -1;
        }
        catch
        {
            return -1;
        }
    }

    /// <summary>
    /// tar 加速上传目录。返回 false = 走不通（调用方回退逐文件）; 抛出 = 真失败
    /// （命令成功判定后仍出错, 调用方按任务失败记账）。
    /// </summary>
    public static async Task<bool> UploadDirAsync(
        SftpSessionService remoteSession, IRemoteTransferOps ops,
        string localDir, string remoteDir, Action<long>? progress, string[]? logScopes = null)
    {
        if (!IsLocalTarAvailable())
        {
            TerminalLogService.Log(LocalizationService.GetString("LogTarNoLocalTar"), logScopes);
            return false;
        }

        // Windows 等非 POSIX 服务器无 tar/exec 语义, 打包命令必然失败——提前回退
        // 逐文件 (与 CountRemoteFilesAsync 的静默回退同风格; IsPosixServerAsync 有
        // 会话级缓存, 仅首次探测一次)。
        if (!await remoteSession.IsPosixServerAsync())
        {
            return false;
        }

        var guid = Guid.NewGuid().ToString("N");
        var parent = TarPlan.RemoteParent(remoteDir);
        var tarPath = Models.RemotePath.Join(parent, TarPlan.TempTarName(guid));
        var tmpDir = Models.RemotePath.Join(parent, TarPlan.TempDirName(guid));

        // 目标已存在?（一条 STAT; 决定 rename 原子切换还是直接解包覆盖）
        var destExists = await ops.TryGetSizeAsync(remoteDir) >= 0;

        try
        {
            // 本地 tar stdout 直写远端临时文件（不在本地落临时 tar）
            using (var process = StartLocalTarPack(localDir))
            {
                var stderrTask = process.StandardError.ReadToEndAsync();
                try
                {
                    await ops.UploadStreamAsync(process.StandardOutput.BaseStream, tarPath, progress);
                    var stderr = await stderrTask;
                    process.WaitForExit();
                    if (process.ExitCode != 0)
                    {
                        throw new InvalidOperationException(
                            string.Format(LocalizationService.GetString("LogTarLocalFail"),
                                process.ExitCode, stderr.Trim()));
                    }
                }
                catch
                {
                    TryKill(process);
                    throw;
                }
            }

            if (!destExists)
            {
                // 先解到同级隐藏临时目录, 再 rename —— 目标位置要么原样要么完整新目录
                await ExecAsync(remoteSession, TarPlan.BuildUnpackCommand(tarPath, tmpDir));
                try
                {
                    await ops.RenameAsync(tmpDir, remoteDir);
                    return true;
                }
                catch (Exception ex)
                {
                    // rename 失败（目标在探测后出现等）: 退回直接解包覆盖, 语义不丢
                    TerminalLogService.Log(string.Format(
                        LocalizationService.GetString("LogTarRenameFailed"), remoteDir, ex.Message), logScopes);
                    await ExecAsync(remoteSession, TarPlan.BuildUnpackCommand(tarPath, remoteDir));
                    return true;
                }
            }

            await ExecAsync(remoteSession, TarPlan.BuildUnpackCommand(tarPath, remoteDir));
            return true;
        }
        finally
        {
            await CleanupRemoteAsync(remoteSession, tarPath, tmpDir, logScopes);
        }
    }

    /// <summary>
    /// tar 加速下载目录。返回 false = 走不通（调用方回退逐文件）; 抛出 = 真失败。
    /// </summary>
    public static async Task<bool> DownloadDirAsync(
        SftpSessionService srcSession, IRemoteTransferOps ops,
        string remoteDir, string localDir, Action<long>? progress, string[]? logScopes = null)
    {
        if (!IsLocalTarAvailable())
        {
            TerminalLogService.Log(LocalizationService.GetString("LogTarNoLocalTar"), logScopes);
            return false;
        }

        var guid = Guid.NewGuid().ToString("N");
        var remoteTar = RemoteTempDir + "/" + TarPlan.TempTarName(guid);
        // 盘根 (如 "E:\") 的 GetDirectoryName 返回 null——无父目录可放临时文件,
        // 提前回退逐文件 (静默, 与上传方向非 POSIX 回退同风格; 审核 N9)
        var localParent = Path.GetDirectoryName(Path.GetFullPath(localDir));
        if (localParent is null)
        {
            return false;
        }

        // 临时 tar 与解包临时目录同级放目标父目录, 不放解包目录内部——解包成功后
        // 整目录 Move/Merge 搬迁进目标, tar 放内部会一并泄入目标且任何清理路径都
        // 删不到 (审核 N3)
        var localTmpDir = Path.Combine(localParent, TarPlan.TempDirName(guid));
        var localTar = Path.Combine(localParent, TarPlan.TempTarName(guid));

        try
        {
            // 服务端打包成单文件（exec 通道; /tmp 不可写或磁盘不足则失败回退）
            await ExecAsync(srcSession, TarPlan.BuildPackCommand(remoteTar, remoteDir));

            Directory.CreateDirectory(localTmpDir);
            await ops.DownloadAsync(remoteTar, localTar, progress);

            // 本地解包到临时目录, 再 rename/合并 —— 目标位置要么原样要么完整
            RunLocalTarExtract(localTar, localTmpDir);
            if (!Directory.Exists(localDir))
            {
                Directory.Move(localTmpDir, localDir);
            }
            else
            {
                MergeDirectory(localTmpDir, localDir);
            }

            return true;
        }
        finally
        {
            await CleanupRemoteAsync(srcSession, remoteTar, null, logScopes);
            TryDeleteFile(localTar);
            TryDeleteDirectory(localTmpDir); // rename 成功后已不存在, 幂等
        }
    }

    /// <summary>exec 一条命令并严格判定退出码（非 0 抛, 由调用方回退或记账）。</summary>
    private static async Task ExecAsync(SftpSessionService session, string command)
    {
        var (exit, _, error) = await session.RunCommandAsync(command, ExecTimeoutSeconds);
        if (exit != 0)
        {
            throw new InvalidOperationException(string.Format(
                LocalizationService.GetString("LogTarExecFail"),
                command, exit, string.IsNullOrWhiteSpace(error) ? "-" : error.Trim()));
        }
    }

    /// <summary>清理远端临时 tar / 临时目录（幂等; 失败只记日志, 不影响传输结果）。</summary>
    private static async Task CleanupRemoteAsync(SftpSessionService session, string tarPath, string? tmpDir, string[]? logScopes = null)
    {
        try
        {
            await session.RunCommandAsync(TarPlan.BuildRemoveCommand(tarPath), ExecQuickTimeoutSeconds);
            if (tmpDir is not null)
            {
                await session.RunCommandAsync(TarPlan.BuildRemoveDirCommand(tmpDir), ExecQuickTimeoutSeconds);
            }
        }
        catch (Exception ex)
        {
            // 清理失败（通道断等）: 临时文件残留在 /tmp 或目标父目录, 不致命
            TerminalLogService.Log(string.Format(
                LocalizationService.GetString("LogTarCleanupFailed"), tarPath, ex.Message), logScopes);
        }
    }

    private static Process StartLocalTar(string argument)
    {
        var psi = new ProcessStartInfo("tar")
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        psi.ArgumentList.Add(argument);
        return Process.Start(psi)
            ?? throw new InvalidOperationException(LocalizationService.GetString("LogTarNoLocalTar"));
    }

    /// <summary>本地 tar 打包到 stdout: tar -cf - -C &lt;dir&gt; .（参数走 ArgumentList, 无 shell  quoting 问题）。</summary>
    private static Process StartLocalTarPack(string localDir)
    {
        var psi = new ProcessStartInfo("tar")
        {
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };
        psi.ArgumentList.Add("-cf");
        psi.ArgumentList.Add("-");
        psi.ArgumentList.Add("-C");
        psi.ArgumentList.Add(localDir);
        psi.ArgumentList.Add(".");
        return Process.Start(psi)
            ?? throw new InvalidOperationException(LocalizationService.GetString("LogTarNoLocalTar"));
    }

    /// <summary>本地 tar 解包: tar -xf &lt;file&gt; -C &lt;dir&gt;（目录须已存在）。</summary>
    private static void RunLocalTarExtract(string tarFile, string destDir)
    {
        using var process = new Process();
        var psi = process.StartInfo;
        psi.FileName = "tar";
        psi.UseShellExecute = false;
        psi.CreateNoWindow = true;
        psi.RedirectStandardError = true;
        psi.ArgumentList.Add("-xf");
        psi.ArgumentList.Add(tarFile);
        psi.ArgumentList.Add("-C");
        psi.ArgumentList.Add(destDir);
        try
        {
            process.Start();
            var stderr = process.StandardError.ReadToEnd();
            process.WaitForExit();
            if (process.ExitCode != 0)
            {
                throw new InvalidOperationException(string.Format(
                    LocalizationService.GetString("LogTarLocalFail"), process.ExitCode, stderr.Trim()));
            }
        }
        catch
        {
            TryKill(process);
            throw;
        }
    }

    private static void TryKill(Process process)
    {
        try
        {
            if (!process.HasExited)
            {
                process.Kill(entireProcessTree: true);
            }
        }
        catch
        {
            // 进程已退出/无法击杀: 忽略
        }
    }

    private static void TryDeleteDirectory(string dir)
    {
        try
        {
            if (Directory.Exists(dir))
            {
                Directory.Delete(dir, recursive: true);
            }
        }
        catch
        {
            // 清理失败: 临时目录残留在目标父目录, 不致命
        }
    }

    private static void TryDeleteFile(string file)
    {
        try
        {
            if (File.Exists(file))
            {
                File.Delete(file);
            }
        }
        catch
        {
            // 清理失败: 临时 tar 残留在目标父目录, 不致命
        }
    }

    /// <summary>把临时目录内容合并进已存在的目标目录（文件覆盖, 目录幂等创建）。</summary>
    private static void MergeDirectory(string source, string dest)
    {
        foreach (var dir in Directory.EnumerateDirectories(source, "*", SearchOption.AllDirectories))
        {
            Directory.CreateDirectory(Path.Combine(dest, Path.GetRelativePath(source, dir)));
        }

        foreach (var file in Directory.EnumerateFiles(source, "*", SearchOption.AllDirectories))
        {
            var target = Path.Combine(dest, Path.GetRelativePath(source, file));
            Directory.CreateDirectory(Path.GetDirectoryName(target)!);
            File.Copy(file, target, overwrite: true);
        }
    }
}
