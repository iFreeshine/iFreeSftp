using System.Security.Cryptography;
using iFreeSftp.Core.Abstractions;
using iFreeSftp.Models;
using Renci.SshNet;
using Renci.SshNet.Sftp;

namespace iFreeSftp.Services;

/// <summary>
/// SFTP 远程会话抽象：负责连接、保活(keepalive)、目录列举与文件管理操作。
/// 连接参数（超时/重试/重试延迟）由应用设置 AppSettingsService 提供。
/// 连接与全部操作基于 SSH.NET (SftpClient) 真实实现。
/// 实现 IRemoteExec 命令通道适配，供闭源传输核心下发服务端直达命令。
/// </summary>
public sealed class SftpSessionService : IDisposable, IRemoteTransferOps, IRemoteExec
{
    /// <summary>远端主机。</summary>
    public string Host { get; private set; } = string.Empty;

    /// <summary>端口，默认 22。</summary>
    public int Port { get; private set; } = 22;

    /// <summary>是否已连接。</summary>
    public bool IsConnected { get; private set; }

    /// <summary>保活间隔，默认 30 秒，参考 FileZilla 默认值。</summary>
    public TimeSpan KeepAliveInterval { get; set; } = TimeSpan.FromSeconds(30);

    /// <summary>保活是否在运行。</summary>
    public bool KeepAliveActive => _keepAliveTimer is not null;

    /// <summary>保活状态变化时触发。</summary>
    public event Action<bool>? KeepAliveChanged;

    /// <summary>
    /// 已信任的主机指纹（SHA256:xxx）。非空时连接自动校验：匹配才信任，
    /// 不匹配直接拒绝（防中间人）。为空时走 ConfirmHostKey 确认回调；
    /// 回调也为空时拒绝连接（fail-closed）。
    /// 外部只能经 <see cref="SetTrustedFingerprint"/> 设置（收口 + 审计）；
    /// 连接内部回填（重试/懒通道复用确认结果）不走该入口。
    /// </summary>
    public string? TrustedFingerprint { get; private set; }

    /// <summary>
    /// 设置信任指纹的唯一入口（调用方用主机档案的存档指纹预填）。
    /// 非空时记审计日志，便于排查"本次连接为何静默信任了该主机"。
    /// </summary>
    public void SetTrustedFingerprint(string? fingerprint)
    {
        TrustedFingerprint = fingerprint;
        if (!string.IsNullOrEmpty(fingerprint))
        {
            TerminalLogService.Log(string.Format(
                LocalizationService.GetString("LogTrustApplied"), fingerprint), LogScope);
        }
    }

    /// <summary>
    /// 主机指纹确认回调（首次遇到未知主机密钥时由 UI 提供指纹确认框）。
    /// 参数为 SHA256 指纹，返回是否信任。在 SSH.NET 握手线程上同步等待结果。
    /// 为空时无存档指纹的连接直接拒绝（fail-closed）。
    /// </summary>
    public Func<string, Task<bool>>? ConfirmHostKey { get; set; }

    /// <summary>
    /// 主机密钥变更确认回调（有存档指纹但本次不匹配时询问用户是否更新信任）。
    /// 参数为 (存档指纹, 本次指纹)，返回是否更新。在 SSH.NET 握手线程上同步等待结果。
    /// 为空时（测试连接）维持直接拒绝。
    /// </summary>
    public Func<string, string, Task<bool>>? ConfirmHostKeyChange { get; set; }

    /// <summary>本次连接中用户确认过的指纹（连接成功后由调用方持久化到主机档案）。</summary>
    public string? ConfirmedFingerprint { get; private set; }

    /// <summary>本会话日志归属的 tab scope（承载它的 FileView 创建会话时注入;
    /// null = 全局广播, 如"测试连接"的临时会话）。</summary>
    public string? LogScope { get; set; }

    /// <summary>用户在确认框拒绝了主机指纹（视为放弃, 重试循环不再继续）。</summary>
    private bool _fingerprintRejected;

    private Timer? _keepAliveTimer;
    private SftpClient? _client;
    // SSH.NET 的 SftpClient 非线程安全: 并发调用 (如连接后 UpdateModeUI 双触发 +
    // 手动刷新同时到) 会让协议请求交错挂死。所有 SFTP 操作经此信号量串行执行。
    private readonly SemaphoreSlim _sshLock = new(1, 1);

    // —— Exec 命令通道: 与 SFTP 独立的 SSH 会话 (SftpClient 无命令执行能力)。
    // 懒建立: 首次跑命令/探测时才连接, 失败不影响 SFTP 主链路 (命令类功能各自回退)。
    private SshClient? _execClient;
    private readonly SemaphoreSlim _execLock = new(1, 1);
    // 懒建 Exec 通道所需的登录参数 (ConnectAsync 成功时记录)
    private HostProfile? _lastProfile;
    // 服务端哈希命令探测缓存: null=未探测 (重连/换主机时清空)
    private bool? _hasSha256Sum;
    private bool? _hasSha512Sum;
    private bool? _hasOpensslSm3;
    // 服务端平台探测缓存: null=未探测 (重连/换主机时清空)。true=POSIX shell
    // (sh/bash), false=Windows (PowerShell/cmd 登录 shell, /tmp 等不存在, 命令语法不同)
    private bool? _isPosixServer;

    /// <summary>
    /// 建立连接：按登录方式取密钥文件或密码构造凭据，SSH.NET 真实连接。
    /// 连接参数用应用设置（AppSettingsService）：超时秒数（0=禁用），
    /// 失败后按最大重试次数/登录重试延迟自动重试（retry=false 或认证失败/用户拒绝指纹时不重试）。
    /// 失败抛异常（网络不可达 / 认证失败 / 密钥文件无效等），由调用方展示详情。
    /// </summary>
    public async Task<bool> ConnectAsync(HostProfile profile, bool retry = true)
    {
        Host = profile.Host;
        Port = profile.Port;
        ConfirmedFingerprint = null;
        _fingerprintRejected = false;
        // 重连/换主机: 旧 Exec 命令通道作废, 探测缓存清空
        DisconnectExec();
        _lastProfile = profile;
        _hasSha256Sum = _hasSha512Sum = _hasOpensslSm3 = null;
        _isPosixServer = null;

        var info = BuildConnectionInfo(profile);

        // 总尝试次数 = 首次 + 最大重试次数（重试关闭时仅一次）。
        var maxAttempts = retry ? 1 + Math.Max(0, AppSettingsService.MaxRetries) : 1;
        for (var attempt = 1; ; attempt++)
        {
            _client?.Dispose();
            _client = new SftpClient(info);
            // SSH.NET 内置自动保活 (周期 NOOP), 由应用设置的间隔驱动; StartKeepAlive 的
            // 定时器仅负责 KeepAliveActive 状态供状态栏显示。
            _client.KeepAliveInterval = KeepAliveInterval;
            _client.HostKeyReceived += OnHostKeyReceived;
            // 重试时复用本次会话已确认的指纹, 避免重复弹确认框。
            if (TrustedFingerprint is null && ConfirmedFingerprint is not null)
            {
                TrustedFingerprint = ConfirmedFingerprint;
            }

            try
            {
                var timeoutSeconds = AppSettingsService.ConnectTimeoutSeconds;
                if (timeoutSeconds > 0)
                {
                    using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(timeoutSeconds));
                    await _client.ConnectAsync(cts.Token);
                }
                else
                {
                    await _client.ConnectAsync(CancellationToken.None);
                }

                IsConnected = _client.IsConnected;
                // 操作超时: 服务器无响应 (连接配额被占/sftp 子系统挂起) 时, 列目录/
                // 传输等单次请求按上限报错, 不再无限挂起 (SSH.NET 默认无限等待)
                var opSeconds = AppSettingsService.OperationTimeoutSeconds;
                _client.OperationTimeout = opSeconds > 0
                    ? TimeSpan.FromSeconds(opSeconds)
                    : Timeout.InfiniteTimeSpan;
                return IsConnected;
            }
            catch (Exception ex) when (attempt < maxAttempts
                && ex is not Renci.SshNet.Common.SshAuthenticationException
                && !_fingerprintRejected)
            {
                var delay = Math.Max(0, AppSettingsService.RetryDelaySeconds);
                TerminalLogService.Log(string.Format(
                    LocalizationService.GetString("LogRetry"),
                    ex.Message, delay, attempt, AppSettingsService.MaxRetries), LogScope);
                if (delay > 0)
                {
                    await Task.Delay(TimeSpan.FromSeconds(delay));
                }
            }
        }
    }

    /// <summary>
    /// 主机密钥校验：有存档指纹时精确匹配才信任（不匹配拒绝）;
    /// 无存档时走确认回调询问用户（确认结果记录到 ConfirmedFingerprint 供调用方保存）;
    /// 无回调时拒绝（fail-closed：所有调用方都必须接入指纹确认）。
    /// </summary>
    private void OnHostKeyReceived(object? sender, Renci.SshNet.Common.HostKeyEventArgs e)
    {
        var fingerprint = e.FingerPrintSHA256;
        if (string.IsNullOrEmpty(TrustedFingerprint))
        {
            if (ConfirmHostKey is null)
            {
                // fail-closed: 无存档指纹且无确认回调时拒绝。不再自动放行, 防止任何
                // 未接入指纹确认的调用路径静默信任未知主机密钥 (中间人风险)。
                e.CanTrust = false;
                return;
            }

            // 握手线程同步等待 UI 确认结果。
            var confirm = ConfirmHostKey(fingerprint).GetAwaiter().GetResult();
            e.CanTrust = confirm;
            if (confirm)
            {
                ConfirmedFingerprint = fingerprint;
            }
            else
            {
                _fingerprintRejected = true;
                TerminalLogService.Log(LocalizationService.GetString("LogFingerprintRejected"), LogScope);
            }

            return;
        }

        e.CanTrust = string.Equals(fingerprint, TrustedFingerprint, StringComparison.Ordinal);
        if (!e.CanTrust)
        {
            // 拒绝时把两个指纹都打出来: 存档 ≠ 本次 → 服务器密钥变更或档案过期 (如卡片改了 IP)
            TerminalLogService.Log(string.Format(
                LocalizationService.GetString("LogFingerprintMismatch"),
                TrustedFingerprint, fingerprint), LogScope);

            // 变更确认兜底 (服务器重装/换机): 用户确认后更新信任, ConfirmedFingerprint
            // 会随连接成功由调用方写回主机档案 (复用首次确认的持久化链路)
            if (ConfirmHostKeyChange is not null)
            {
                var reConfirm = ConfirmHostKeyChange(TrustedFingerprint, fingerprint)
                    .GetAwaiter().GetResult();
                e.CanTrust = reConfirm;
                if (reConfirm)
                {
                    ConfirmedFingerprint = fingerprint;
                    TerminalLogService.Log(LocalizationService.GetString("LogFingerprintChangedTrusted"), LogScope);
                }
                else
                {
                    _fingerprintRejected = true;
                    TerminalLogService.Log(LocalizationService.GetString("LogFingerprintRejected"), LogScope);
                }
            }
        }
    }

    /// <summary>取远端当前工作目录（登录用户主目录；须已连接。
    /// SSH.NET 的 WorkingDirectory 属性是网络请求 (REALPATH), 须走后台线程）。</summary>
    public Task<string> GetWorkingDirectoryAsync()
        => RunAsync(client => client.WorkingDirectory);

    /// <summary>按登录方式构造认证凭据：密钥方式用私钥文件（不支持 .ppk），否则用密码。</summary>
    private static AuthenticationMethod CreateAuthMethod(HostProfile profile)
    {
        if (profile.AuthMode == AuthMode.KeyFile)
        {
            var keyFile = new PrivateKeyFile(profile.KeyFilePath);
            return new PrivateKeyAuthenticationMethod(profile.Username, keyFile);
        }

        return new PasswordAuthenticationMethod(profile.Username, profile.Password);
    }

    /// <summary>断开连接并停止保活。</summary>
    public void Disconnect()
    {
        StopKeepAlive();
        DisconnectExec();
        _client?.Disconnect();
        _client?.Dispose();
        _client = null;
        IsConnected = false;
    }

    /// <summary>启动保活定时器：周期性发送 NOOP 包维持长连接，避免被中间网关断开。</summary>
    public void StartKeepAlive()
    {
        if (!IsConnected || _keepAliveTimer is not null)
        {
            return;
        }

        _keepAliveTimer = new Timer(
            callback: _ => SendKeepAlive(),
            state: null,
            dueTime: KeepAliveInterval,
            period: KeepAliveInterval);

        KeepAliveChanged?.Invoke(true);
    }

    /// <summary>停止保活定时器。</summary>
    public void StopKeepAlive()
    {
        if (_keepAliveTimer is null)
        {
            return;
        }

        _keepAliveTimer.Dispose();
        _keepAliveTimer = null;
        KeepAliveChanged?.Invoke(false);
    }

    /// <summary>保活心跳定时器触发：真实 NOOP 由 SSH.NET KeepAliveInterval 自动发送 (见 ConnectAsync)，
    /// 此处仅维持状态栏"保活中"显示语义。若改回手动发送, 须显式实现 (SendKeepAlive() 已标记过时)。</summary>
    private void SendKeepAlive()
    {
    }

    /// <summary>
    /// SFTP 权限位 → "drwxr-xr-x" 形式的 10 字符串：1 位类型（d=目录 / l=符号链接 / -=文件）
    /// + 属主/属组/其他三组 rwx。与 ls -l 显示一致（不含 setuid/sticky 特殊位）。
    /// </summary>
    private static string BuildPermissionString(ISftpFile f)
        => string.Create(10, f, static (span, file) =>
        {
            span[0] = file.IsSymbolicLink ? 'l' : file.IsDirectory ? 'd' : '-';
            span[1] = file.OwnerCanRead ? 'r' : '-';
            span[2] = file.OwnerCanWrite ? 'w' : '-';
            span[3] = file.OwnerCanExecute ? 'x' : '-';
            span[4] = file.GroupCanRead ? 'r' : '-';
            span[5] = file.GroupCanWrite ? 'w' : '-';
            span[6] = file.GroupCanExecute ? 'x' : '-';
            span[7] = file.OthersCanRead ? 'r' : '-';
            span[8] = file.OthersCanWrite ? 'w' : '-';
            span[9] = file.OthersCanExecute ? 'x' : '-';
        });

    /// <summary>列举目录（过滤 "." ".."，隐藏文件过滤由调用方按开关处理）。
    /// SSH.NET 调用为同步阻塞，统一包 Task.Run 避免卡 UI（下同）。</summary>
    public Task<IReadOnlyList<FileEntry>> ListDirectoryAsync(string path)
        => RunAsync(client =>
        {
            var entries = new List<FileEntry>();
            foreach (var f in client.ListDirectory(path))
            {

                if (f.Name is "." or "..")
                {
                    continue;
                }

                entries.Add(new FileEntry
                {
                    Name = f.Name,
                    FullPath = f.FullName,
                    IsDirectory = f.IsDirectory,
                    IsExecutable = f.OwnerCanExecute || f.GroupCanExecute || f.OthersCanExecute,
                    IsSymbolicLink = f.IsSymbolicLink,
                    Permissions = BuildPermissionString(f),
                    Length = f.Length,
                    LastModified = f.LastWriteTime,
                });
            }

            return (IReadOnlyList<FileEntry>)entries;
        });

    /// <summary>远程路径是否存在（文件或目录）。</summary>
    public Task<bool> ExistsAsync(string path)
        => RunAsync(client => client.Exists(path));

    /// <summary>新建远程目录。</summary>
    public Task CreateDirectoryAsync(string path)
        => RunAsync<object?>(client => { client.CreateDirectory(path); return null; });

    /// <summary>新建远程空文件（已存在时无操作，由调用方保证名字可用）。</summary>
    public Task CreateFileAsync(string path)
        => RunAsync<object?>(client =>
        {
            if (!client.Exists(path))
            {
                client.WriteAllText(path, string.Empty);
            }

            return null;
        });

    /// <summary>远程重命名（SSH_FXP_RENAME，文件与目录通用）。</summary>
    public Task RenameAsync(string oldPath, string newPath)
        => RunAsync<object?>(client => { client.RenameFile(oldPath, newPath); return null; });

    /// <summary>远程删除：文件直接删（单条 REMOVE，无提速空间，不受设置影响）；目录按
    /// "远程删除方式"设置分流——"exec" 经命令通道 rm -rf 服务端本地递归删除（海量文件
    /// 秒级，失败自动回退 SFTP 递归），"recursive" 纯 SFTP 协议逐条递归。SSH.NET 2026
    /// 的 DeleteDirectory 经反编译确认仅发一个 RMDIR 请求（无递归），非空目录会被服务器
    /// 拒绝，须自行自上而下清空（README 3.12 案例）。</summary>
    public async Task DeleteAsync(string path, bool isDirectory)
    {
        if (isDirectory && AppSettingsService.RemoteDeleteMode == "exec"
            && await TryDeleteDirViaExecAsync(path))
        {
            return;
        }

        await RunAsync<object?>(client =>
        {
            if (isDirectory)
            {
                DeleteRemoteDirRecursive(client, path);
            }
            else
            {
                client.DeleteFile(path);
            }

            return null;
        });
    }

    /// <summary>命令通道 rm -rf 删除目录。成功判定严格：退出码 0 才算成功（防 Windows
    /// 服务器 shell 下 rm 非法被误判成功，与服务端 cp 直达同款）；超时/通道异常时命令
    /// 可能"迟到删净"，回退前先经 SFTP 核对——目录已不存在同样视为成功，仍存在则交由
    /// 调用方回退 SFTP 递归（rm 幂等，按现存条目重列，部分删除状态无影响）。</summary>
    private async Task<bool> TryDeleteDirViaExecAsync(string path)
    {
        try
        {
            var (exit, _, _) = await RunCommandAsync($"rm -rf -- {ShellQuote(path)}", 120);
            if (exit == 0)
            {
                return true;
            }
        }
        catch
        {
            // 通道不可用（仅 SFTP 主机/断连）: 走存在性核对后回退
        }

        try
        {
            if (await RunAsync(client => !client.Exists(path)))
            {
                return true; // 迟到删净: rm 已完成, 只是回执丢失/超时
            }

            TerminalLogService.Log(LocalizationService.GetString("LogDeleteFallback"), LogScope);
        }
        catch
        {
            // 核对失败（断连等）: 静默回退, 由 SFTP 递归路径给出明确报错
        }

        return false;
    }

    /// <summary>递归删除远程目录：先清空内容，最后删除目录本身。符号链接一律按文件删
    /// （删链接本身不跟随——IsDirectory 对指向目录的链接可能为真，递归进入会误删目标内容）。</summary>
    private static void DeleteRemoteDirRecursive(SftpClient client, string path)
    {
        foreach (var f in client.ListDirectory(path))
        {
            if (f.Name is "." or "..")
            {
                continue;
            }

            if (f.IsDirectory && !f.IsSymbolicLink)
            {
                DeleteRemoteDirRecursive(client, f.FullName);
            }
            else
            {
                client.DeleteFile(f.FullName);
            }
        }

        client.DeleteDirectory(path); // 至此必为空目录, RMDIR 成功
    }

    /// <summary>读取远端文件头部 N 字节（远程编辑的文本内容探测，避免整文件下载）。</summary>
    public Task<byte[]> ReadHeadAsync(string path, int count)
        => RunAsync(client =>
        {
            using var stream = client.OpenRead(path);
            var buffer = new byte[count];
            var read = 0;
            while (read < count)
            {
                var n = stream.Read(buffer, read, count - read);
                if (n <= 0)
                {
                    break;
                }

                read += n;
            }

            return read == count ? buffer : buffer[..read];
        });

    /// <summary>下载远端文件到本地路径（远程编辑临时副本 / 传输到另一侧）。
    /// progress = 累计已传输字节回调（SSH.NET 分块触发，任意线程，可为空）。</summary>
    public Task DownloadAsync(string remotePath, string localPath, Action<long>? progress = null)
        => RunAsync<object?>(client =>
        {
            using var stream = File.Create(localPath);
            if (progress is null)
            {
                client.DownloadFile(remotePath, stream);
            }
            else
            {
                client.DownloadFile(remotePath, stream, b => progress((long)b));
            }

            return null;
        });

    /// <summary>上传本地文件覆盖远端路径（远程编辑保存回传 / 传输到另一侧）。
    /// progress = 累计已传输字节回调（SSH.NET 分块触发，任意线程，可为空）。</summary>
    public Task UploadAsync(string localPath, string remotePath, Action<long>? progress = null)
        => RunAsync<object?>(client =>
        {
            using var stream = File.OpenRead(localPath);
            if (progress is null)
            {
                client.UploadFile(stream, remotePath);
            }
            else
            {
                client.UploadFile(stream, remotePath, b => progress((long)b));
            }

            return null;
        });

    /// <summary>查询远端文件大小；不存在返回 -1（SftpPathNotFoundException 归一,
    /// 权限等真实错误照常上抛——后续传输同样会失败, 不掩盖问题）。</summary>
    public Task<long> TryGetSizeAsync(string remotePath)
        => RunAsync(client =>
        {
            try
            {
                return client.GetAttributes(remotePath).Size;
            }
            catch (Renci.SshNet.Common.SftpPathNotFoundException)
            {
                return -1L;
            }
        });

    /// <summary>断点续传上传（主会话实现: 与全部 SFTP 操作同经 _sshLock 串行）。
    /// OpenOrCreate + 显式定位续写, 不截断远端半成品。</summary>
    public Task UploadResumeAsync(string localPath, string remotePath, long offset, Action<long>? progress = null)
        => RunAsync<object?>(client =>
        {
            using var input = File.OpenRead(localPath);
            input.Seek(offset, SeekOrigin.Begin);
            using var output = client.Open(remotePath, FileMode.OpenOrCreate, FileAccess.ReadWrite);
            output.Seek(offset, SeekOrigin.Begin);
            var buffer = new byte[32768];
            long total = offset;
            int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }

            output.Flush();
            return null;
        });

    /// <summary>断点续传下载（主会话实现）。远端读流定位, 本地追加写（不截断）。</summary>
    public Task DownloadResumeAsync(string remotePath, string localPath, long offset, Action<long>? progress = null)
        => RunAsync<object?>(client =>
        {
            using var output = new FileStream(localPath, FileMode.OpenOrCreate, FileAccess.ReadWrite);
            if (output.Length > offset)
            {
                output.SetLength(offset);
            }

            output.Seek(offset, SeekOrigin.Begin);
            using var input = client.OpenRead(remotePath);
            input.Seek(offset, SeekOrigin.Begin);
            var buffer = new byte[32768];
            long total = offset;
            int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }

            return null;
        });

    /// <summary>把本地流写入远端文件（覆盖创建）。tar 加速上传直写远端临时文件。</summary>
    public Task UploadStreamAsync(Stream source, string remotePath, Action<long>? progress = null)
        => RunAsync<object?>(client =>
        {
            using var output = client.Open(remotePath, FileMode.Create, FileAccess.Write);
            var buffer = new byte[32768];
            long total = 0;
            int read;
            while ((read = source.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }

            output.Flush();
            return null;
        });

    /// <summary>远端文件最后修改时间（远程编辑 mtime 冲突比对）。</summary>
    public Task<DateTimeOffset> GetLastWriteTimeAsync(string path)
        => RunAsync(client => new DateTimeOffset(client.GetAttributes(path).LastWriteTime));

    /// <summary>远端条目大小与修改时间（传输覆盖确认的源/目标对比信息）。</summary>
    public Task<(long Size, DateTimeOffset LastWriteTime)> GetAttributesAsync(string path)
        => RunAsync(client =>
        {
            var attrs = client.GetAttributes(path);
            return (attrs.Size, new DateTimeOffset(attrs.LastWriteTime));
        });

    /// <summary>远端条目当前权限（12 位 Unix mode，如 0o755；setgid 位 SSH.NET 未公开读取，忽略）。</summary>
    public Task<int> GetPermissionsAsync(string path)
        => RunAsync(client =>
        {
            var attrs = client.GetAttributes(path);
            int mode = 0;
            if (attrs.OwnerCanRead)
            {
                mode |= 0x100;
            }

            if (attrs.OwnerCanWrite)
            {
                mode |= 0x080;
            }

            if (attrs.OwnerCanExecute)
            {
                mode |= 0x040;
            }

            if (attrs.GroupCanRead)
            {
                mode |= 0x020;
            }

            if (attrs.GroupCanWrite)
            {
                mode |= 0x010;
            }

            if (attrs.GroupCanExecute)
            {
                mode |= 0x008;
            }

            if (attrs.OthersCanRead)
            {
                mode |= 0x004;
            }

            if (attrs.OthersCanWrite)
            {
                mode |= 0x002;
            }

            if (attrs.OthersCanExecute)
            {
                mode |= 0x001;
            }

            if (attrs.IsUIDBitSet)
            {
                mode |= 0x800;
            }

            if (attrs.IsStickyBitSet)
            {
                mode |= 0x200;
            }

            return mode;
        });

    /// <summary>远程 chmod：逐位回写 9 位属主/属组/其他的 rwx（布尔位只动各自位, suid/sgid/sticky
    /// 在属性字段中保持原值）。SSH.NET 属性对象自带变更跟踪, SetAttributes 只发送变更过的字段,
    /// 不会误写大小/时间。权限无变化时不产生任何 SETSTAT 请求。</summary>
    public Task SetPermissionsAsync(string path, int mode)
        => RunAsync<object?>(client =>
        {
            var attrs = client.GetAttributes(path);
            attrs.OwnerCanRead = (mode & 0x100) != 0;
            attrs.OwnerCanWrite = (mode & 0x080) != 0;
            attrs.OwnerCanExecute = (mode & 0x040) != 0;
            attrs.GroupCanRead = (mode & 0x020) != 0;
            attrs.GroupCanWrite = (mode & 0x010) != 0;
            attrs.GroupCanExecute = (mode & 0x008) != 0;
            attrs.OthersCanRead = (mode & 0x004) != 0;
            attrs.OthersCanWrite = (mode & 0x002) != 0;
            attrs.OthersCanExecute = (mode & 0x001) != 0;
            client.SetAttributes(path, attrs);
            return null;
        });

    // 校验码计算取消标记: 弹框关闭/切换算法时置位, 分块循环逐块检查, 尽快让出 SSH 通道
    private volatile bool _checksumCancel;

    /// <summary>请求中断正在进行的校验码计算（若在跑）; 下一次 ComputeChecksumAsync 开始时自动复位。</summary>
    public void CancelChecksum() => _checksumCancel = true;

    /// <summary>
    /// 计算远端文件哈希。algorithm: "SHA256"/"SHA512"/"SM3"。
    /// ① 服务端命令优先（探测可用时）: 服务器本地磁盘读 + 结果传回, 大文件秒级完成, 且在
    ///    独立 Exec 通道执行, 不占 SFTP 链路/不与文件操作抢锁; 不响应 CancelChecksum——
    ///    服务端执行由 RunCommandAsync 10 秒超时兜底, 弹窗侧凭 _runId 作废在途结果。
    /// ② 回退本机拉流（命令缺失/Exec 不可用）: 复用与下载同一条 DownloadFile 已验证路径
    ///    （SFTP 大块手动 Read 曾挂死）, 边读边算不落盘, 分块间隙检查取消标志。
    /// </summary>
    public async Task<string> ComputeChecksumAsync(string path, string algorithm)
    {
        var client = _client ?? throw new InvalidOperationException(LocalizationService.GetString("StatusDisconnected"));
        if (!client.IsConnected)
        {
            throw new InvalidOperationException(LocalizationService.GetString("StatusDisconnected"));
        }

        var serverHex = algorithm switch
        {
            "SHA256" when _hasSha256Sum == true => await ComputeServerHashAsync(path, "sha256sum", 64),
            "SHA512" when _hasSha512Sum == true => await ComputeServerHashAsync(path, "sha512sum", 128),
            "SM3" when _hasOpensslSm3 == true => await ComputeServerSm3Async(path),
            _ => null,
        };
        if (serverHex is not null)
        {
            return serverHex;
        }

        await _sshLock.WaitAsync();
        try
        {
            // 取消标志必须在锁内复位: 锁外复位会把仍在跑的旧计算"去取消"成完整双份下载
            _checksumCancel = false;
            return await Task.Run(() =>
            {
                using var sink = new HashSinkStream(this, algorithm);
                client.DownloadFile(path, sink);
                return sink.GetHashHex();
            });
        }
        finally
        {
            _sshLock.Release();
        }
    }

    /// <summary>POSIX shell 单引号转义（关闭-转义-重开）, 防路径含空格/引号/分号时被注入或截断。
    /// 实现集中在 <see cref="Models.ShellQuote"/>（可单测）, 此处仅作短名转发。</summary>
    private static string ShellQuote(string arg) => Models.ShellQuote.Quote(arg);

    /// <summary>服务端 sha256sum/sha512sum 计算哈希: 输出形如 "hex␣␣path", 取首段十六进制串并校验长度。</summary>
    private async Task<string?> ComputeServerHashAsync(string path, string tool, int hexLength)
    {
        var (exit, output, error) = await RunCommandAsync($"{tool} {ShellQuote(path)}");
        if (exit != 0)
        {
            throw new InvalidOperationException(string.IsNullOrWhiteSpace(error)
                ? $"{tool} 退出码 {exit}"
                : error.Trim());
        }

        var hex = output.Trim().Split(' ', '\t')[0];
        if (hex.Length != hexLength || !hex.All(Uri.IsHexDigit))
        {
            throw new InvalidOperationException("服务端哈希输出无法解析: " + output.Trim());
        }

        return hex.ToLowerInvariant();
    }

    /// <summary>服务端 openssl dgst -sm3 计算哈希: 输出形如 "SM3(path)= hex", 取最后一个 "=" 之后段并校验。</summary>
    private async Task<string?> ComputeServerSm3Async(string path)
    {
        var (exit, output, error) = await RunCommandAsync($"openssl dgst -sm3 {ShellQuote(path)}");
        if (exit != 0)
        {
            throw new InvalidOperationException(string.IsNullOrWhiteSpace(error)
                ? $"openssl 退出码 {exit}"
                : error.Trim());
        }

        var idx = output.LastIndexOf('=');
        var hex = (idx >= 0 ? output[(idx + 1)..] : output).Trim();
        if (hex.Length != 64 || !hex.All(Uri.IsHexDigit))
        {
            throw new InvalidOperationException("服务端 SM3 输出无法解析: " + output.Trim());
        }

        return hex.ToLowerInvariant();
    }

    /// <summary>传输后校验专用（T2）: 按 exec 探测位路由服务端杂凑命令。仅走 exec
    /// 通道——不回退本机拉流（等价把文件再传一遍且占 _sshLock 阻塞浏览）;
    /// exec 不可用/命令失败/探测未完成一律返回 null, 调用方按"校验跳过"处理。</summary>
    public async Task<string?> TryComputeServerHashAsync(string path, string algorithm)
    {
        try
        {
            return algorithm switch
            {
                "SHA256" when _hasSha256Sum == true => await ComputeServerHashAsync(path, "sha256sum", 64),
                "SHA512" when _hasSha512Sum == true => await ComputeServerHashAsync(path, "sha512sum", 128),
                "SM3" when _hasOpensslSm3 == true => await ComputeServerSm3Async(path),
                _ => null,
            };
        }
        catch
        {
            return null;
        }
    }

    /// <summary>
    /// 校验码哈希接收流: DownloadFile 每读到一块即写入本流喂给增量哈希（SHA256/SHA512 走
    /// IncrementalHash, SM3 走托管实现）。写入间隙检查取消标志, 弹框关闭/切换算法尽快让出通道。
    /// </summary>
    private sealed class HashSinkStream : Stream
    {
        private readonly SftpSessionService _owner;
        private readonly IncrementalHash? _incremental;
        private readonly Sm3? _sm3;
        private long _total;

        public HashSinkStream(SftpSessionService owner, string algorithm)
        {
            _owner = owner;
            if (algorithm == "SM3")
            {
                _sm3 = new Sm3();
            }
            else
            {
                _incremental = IncrementalHash.CreateHash(
                    algorithm == "SHA512" ? HashAlgorithmName.SHA512 : HashAlgorithmName.SHA256);
            }
        }

        public override bool CanRead => false;
        public override bool CanSeek => false;
        public override bool CanWrite => true;
        public override long Length => _total;
        public override long Position
        {
            get => _total;
            set => throw new NotSupportedException();
        }

        public override void Write(byte[] buffer, int offset, int count)
        {
            // 分块间隙检查取消标志: 弹框关闭/切换算法尽快让出 SSH 通道
            if (_owner._checksumCancel)
            {
                throw new OperationCanceledException("校验码计算已取消");
            }

            if (_sm3 is not null)
            {
                _sm3.AppendData(buffer[offset..(offset + count)], count);
            }
            else
            {
                _incremental!.AppendData(buffer, offset, count);
            }

            _total += count;
        }

        public string GetHashHex()
            => _sm3 is not null
                ? _sm3.GetHashHex()
                : Convert.ToHexString(_incremental!.GetHashAndReset()).ToLowerInvariant();

        public override void Flush() { }
        public override int Read(byte[] buffer, int offset, int count) => throw new NotSupportedException();
        public override long Seek(long offset, SeekOrigin origin) => throw new NotSupportedException();
        public override void SetLength(long value) => throw new NotSupportedException();
    }

    // ———— Exec 命令通道（服务端哈希等命令类功能的基础设施） ————

    /// <summary>按连接参数与登录方式构建 ConnectionInfo（SFTP 主连接与 Exec 命令通道共用）。</summary>
    private ConnectionInfo BuildConnectionInfo(HostProfile profile)
    {
        var timeoutSeconds = AppSettingsService.ConnectTimeoutSeconds;
        return new ConnectionInfo(Host, Port, profile.Username, CreateAuthMethod(profile))
        {
            // 0 = 禁用超时, TCP 层按系统默认处理; 其余按设置秒数。
            Timeout = timeoutSeconds > 0 ? TimeSpan.FromSeconds(timeoutSeconds) : Timeout.InfiniteTimeSpan,
        };
    }

    /// <summary>确保 Exec 命令通道已连接（懒建立: 首次命令才连, 闲置断开自动重建;
    /// 失败不影响 SFTP 主链路——命令类功能各自回退）。须在 _execLock 内调用。</summary>
    private async Task<SshClient> EnsureExecClientAsync()
    {
        if (_execClient is { IsConnected: true })
        {
            return _execClient;
        }

        var profile = _lastProfile
            ?? throw new InvalidOperationException(LocalizationService.GetString("StatusDisconnected"));
        _execClient?.Dispose();
        var client = new SshClient(BuildConnectionInfo(profile));
        // 懒连接发生在 SFTP 正式连接成功之后: 指纹已被档案预填或确认回填, 直接精确匹配。
        if (TrustedFingerprint is null && ConfirmedFingerprint is not null)
        {
            TrustedFingerprint = ConfirmedFingerprint;
        }

        client.HostKeyReceived += OnExecHostKeyReceived;
        await client.ConnectAsync(CancellationToken.None);
        _execClient = client;
        return client;
    }

    /// <summary>
    /// Exec/传输通道主机密钥校验: 仅当有已信任指纹且精确匹配时才信任, 无指纹一律拒绝
    /// (fail-closed)。懒连接前置条件是主连接已成功, 届时 TrustedFingerprint 必非空
    /// (档案预填或确认回填), 不会误伤正常路径。
    /// </summary>
    private void OnExecHostKeyReceived(object? sender, Renci.SshNet.Common.HostKeyEventArgs e)
        => e.CanTrust = !string.IsNullOrEmpty(TrustedFingerprint)
            && string.Equals(e.FingerPrintSHA256, TrustedFingerprint, StringComparison.Ordinal);

    /// <summary>
    /// 远程执行一条命令并取回结果（独立 Exec 通道, _execLock 串行, 默认 10 秒超时防挂;
    /// 服务端复制等长任务可传更大超时）。command 原样交给远端登录 shell——含用户
    /// 输入的参数须先经 ShellQuote 转义防注入。
    /// </summary>
    public async Task<(int ExitStatus, string Output, string Error)> RunCommandAsync(string command, int timeoutSeconds = 10)
    {
        await _execLock.WaitAsync();
        try
        {
            var client = await EnsureExecClientAsync();
            return await Task.Run(() =>
            {
                using var cmd = client.CreateCommand(command);
                cmd.CommandTimeout = TimeSpan.FromSeconds(timeoutSeconds);
                try
                {
                    cmd.Execute();
                    // ExitStatus 在命令未正常结束时为 null (超时/通道异常), 归一为 -1
                    return (cmd.ExitStatus ?? -1, cmd.Result, cmd.Error);
                }
                catch
                {
                    // 连接类异常: 丢弃通道, 下次命令自动重建 (自愈)
                    _execClient?.Dispose();
                    _execClient = null;
                    throw;
                }
            });
        }
        finally
        {
            _execLock.Release();
        }
    }

    /// <summary>关闭并释放 Exec 命令通道（重连/断开时调用; 懒建立, 下次命令自动重连）。</summary>
    private void DisconnectExec()
    {
        _execClient?.Dispose();
        _execClient = null;
    }

    /// <summary>
    /// 打开一条传输专用独立连接（并发传输 &gt;1 时每路 worker 各持一条, 绕开主会话
    /// _sshLock 串行瓶颈）。登录参数复用本次会话记录的档案, 指纹静默信任（主连接
    /// 建立时已确认）; 失败抛异常由调用方按任务失败处理。连接不参与保活（批次短命）。
    /// </summary>
    public async Task<TransferConnection> OpenTransferConnectionAsync()
    {
        var profile = _lastProfile
            ?? throw new InvalidOperationException(LocalizationService.GetString("StatusDisconnected"));

        var client = new SftpClient(BuildConnectionInfo(profile));
        if (TrustedFingerprint is null && ConfirmedFingerprint is not null)
        {
            TrustedFingerprint = ConfirmedFingerprint;
        }

        client.HostKeyReceived += OnExecHostKeyReceived;
        try
        {
            var timeoutSeconds = AppSettingsService.ConnectTimeoutSeconds;
            if (timeoutSeconds > 0)
            {
                using var cts = new CancellationTokenSource(TimeSpan.FromSeconds(timeoutSeconds));
                await client.ConnectAsync(cts.Token);
            }
            else
            {
                await client.ConnectAsync(CancellationToken.None);
            }
        }
        catch
        {
            client.Dispose();
            throw;
        }

        return new TransferConnection(client);
    }

    /// <summary>
    /// 探测服务端哈希命令可用性（sha256sum/sha512sum/openssl SM3）, 结果缓存并打终端日志。
    /// 列表加载完成后 fire-and-forget 调用; Exec 通道不可用（如服务器仅允许 SFTP）时日志说明,
    /// 哈希功能回退本机拉流。复合命令一次往返输出 "abc" 三位 0/1（POSIX sh 兼容;
    /// csh 登录 shell 下解析失败 → 视为全部不可用, 安全回退）。
    /// Windows 服务器（PowerShell/cmd 登录 shell）暂不支持服务端哈希: 三个支持位全部置否,
    /// 右键校验码菜单经 ServerHashAny 联动置灰; WSL/bash 登录 shell 走 POSIX 探测不受影响。
    /// </summary>
    public async Task ProbeHashSupportAsync()
    {
        try
        {
            // 先辨平台 (缓存, 与 tar 加速共用): Windows 服务器命令语法/路径均不同
            if (!await IsPosixServerAsync())
            {
                _hasSha256Sum = _hasSha512Sum = _hasOpensslSm3 = false;
                TerminalLogService.Log("[命令通道] Windows 服务器, 暂不支持服务端哈希, 校验码菜单已禁用", LogScope);
                return;
            }

            var (_, output, _) = await RunCommandAsync(
                "a=0; b=0; c=0; command -v sha256sum >/dev/null 2>&1 && a=1;" +
                " command -v sha512sum >/dev/null 2>&1 && b=1;" +
                " openssl dgst -sm3 /dev/null >/dev/null 2>&1 && c=1; echo \"$a$b$c\"");
            var flag = output.Trim();
            var parsed = flag.Length == 3;
            _hasSha256Sum = parsed && flag[0] == '1';
            _hasSha512Sum = parsed && flag[1] == '1';
            _hasOpensslSm3 = parsed && flag[2] == '1';
            TerminalLogService.Log(string.Format(
                "[命令通道] 服务端哈希探测: sha256sum={0} sha512sum={1} SM3(openssl)={2}",
                YesNo(_hasSha256Sum.Value), YesNo(_hasSha512Sum.Value), YesNo(_hasOpensslSm3.Value)), LogScope);
        }
        catch (Exception ex)
        {
            _hasSha256Sum = _hasSha512Sum = _hasOpensslSm3 = false;
            TerminalLogService.Log($"[命令通道] 不可用, 服务端哈希将回退本机拉流: {ex.Message}", LogScope);
        }
    }

    private static string YesNo(bool value) => value ? "可用" : "不可用";

    /// <summary>
    /// 服务端是否 POSIX shell（缓存一条 echo 探测, 重连/换主机清空）。
    /// PowerShell 双引号内插输出 "Windows_NT"; cmd 不认识 $env 语法, 输出字面
    /// "$env:OS"; POSIX 下 $env 未定义展开为空, 输出字面 ":OS"。探测失败
    /// （Exec 通道不可用）时按 false 处理——命令类功能各自回退, 不猜平台。
    /// </summary>
    public async Task<bool> IsPosixServerAsync()
    {
        if (_isPosixServer is { } cached)
        {
            return cached;
        }

        try
        {
            var (_, osOut, _) = await RunCommandAsync("echo $env:OS");
            var os = osOut.Trim();
            _isPosixServer = !os.Contains("Windows_NT") && !os.Contains("$env");
        }
        catch
        {
            _isPosixServer = false;
        }

        return _isPosixServer.Value;
    }

    /// <summary>服务端哈希命令支持快照 (探测未完成或均不可用时均为 false): 远程右键
    /// 校验码菜单可用性与弹窗算法过滤的判定源。ComputeChecksumAsync 另有本机拉流兜底。</summary>
    public bool ServerSha256 => _hasSha256Sum == true;
    public bool ServerSha512 => _hasSha512Sum == true;
    public bool ServerSm3 => _hasOpensslSm3 == true;

    /// <summary>任一算法服务端可用 (探测未完成/断开重置后 → false)。</summary>
    public bool ServerHashAny => ServerSha256 || ServerSha512 || ServerSm3;

    /// <summary>确保已连接后在后台线程串行执行 SSH.NET 同步调用（未连接抛出由调用方展示）。
    /// 同一会话的所有操作经 _sshLock 排队——SftpClient 非线程安全, 并发必挂。</summary>
    private async Task<T> RunAsync<T>(Func<SftpClient, T> action)
    {
        var client = _client ?? throw new InvalidOperationException(LocalizationService.GetString("StatusDisconnected"));
        if (!client.IsConnected)
        {
            throw new InvalidOperationException(LocalizationService.GetString("StatusDisconnected"));
        }

        await _sshLock.WaitAsync();
        try
        {
            return await Task.Run(() => action(client));
        }
        finally
        {
            _sshLock.Release();
        }
    }

    public void Dispose()
    {
        Disconnect();
    }
}
