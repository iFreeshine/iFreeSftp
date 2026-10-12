using iFreeSftp.Models;
using Renci.SshNet;

namespace iFreeSftp.Services;

/// <summary>
/// 文件传输所需的最小远端操作集（并发传输的抽象：主会话或独立连接均可充当）。
/// SftpSessionService 直接实现该接口（其内部 _sshLock 串行）；并发 &gt;1 时由
/// <see cref="TransferConnection"/>（每任务独立连接）实现，绕开主会话串行瓶颈。
/// </summary>
public interface IRemoteTransferOps
{
    /// <summary>新建远程目录（目录上传递归用）。</summary>
    Task CreateDirectoryAsync(string path);

    /// <summary>列举远程目录（过滤 "." ".."，目录下载递归用）。</summary>
    Task<IReadOnlyList<FileEntry>> ListDirectoryAsync(string path);

    /// <summary>下载远端文件到本地路径。progress = 累计已传输字节回调（任意线程，可为空）。</summary>
    Task DownloadAsync(string remotePath, string localPath, Action<long>? progress = null);

    /// <summary>上传本地文件覆盖远端路径。progress = 累计已传输字节回调（任意线程，可为空）。</summary>
    Task UploadAsync(string localPath, string remotePath, Action<long>? progress = null);
}

/// <summary>
/// 传输专用独立 SFTP 连接（并发传输每路一条）：不参与主会话的浏览/保活/命令通道，
/// 仅承载文件传输操作。SftpClient 非线程安全，连接内操作由所属 worker 串行调用，
/// 恰好一个 worker 持有一条连接，无需加锁。传输缓冲调大（默认 32KB）提升吞吐。
/// </summary>
public sealed class TransferConnection : IRemoteTransferOps, IDisposable
{
    /// <summary>传输缓冲区大小 256KB（默认 32KB，调大减少往返次数提升吞吐）。</summary>
    private const int BufferSize = 256 * 1024;

    private readonly SftpClient _client;

    internal TransferConnection(SftpClient client)
    {
        _client = client;
        client.BufferSize = BufferSize;
    }

    public Task CreateDirectoryAsync(string path)
        => Task.Run(() => _client.CreateDirectory(path));

    public Task<IReadOnlyList<FileEntry>> ListDirectoryAsync(string path)
        => Task.Run<IReadOnlyList<FileEntry>>(() =>
        {
            var entries = new List<FileEntry>();
            foreach (var f in _client.ListDirectory(path))
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
                    IsSymbolicLink = f.IsSymbolicLink,
                    Length = f.Length,
                    LastModified = f.LastWriteTime,
                });
            }

            return (IReadOnlyList<FileEntry>)entries;
        });

    public Task DownloadAsync(string remotePath, string localPath, Action<long>? progress = null)
        => Task.Run(() =>
        {
            using var stream = File.Create(localPath);
            if (progress is null)
            {
                _client.DownloadFile(remotePath, stream);
            }
            else
            {
                _client.DownloadFile(remotePath, stream, b => progress((long)b));
            }
        });

    public Task UploadAsync(string localPath, string remotePath, Action<long>? progress = null)
        => Task.Run(() =>
        {
            using var stream = File.OpenRead(localPath);
            if (progress is null)
            {
                _client.UploadFile(stream, remotePath);
            }
            else
            {
                _client.UploadFile(stream, remotePath, b => progress((long)b));
            }
        });

    public void Dispose() => _client.Dispose();
}

/// <summary>
/// 一次批量传输的传输连接池：每路并发各自持有一条独立连接（SftpClient 非线程安全，
/// 单连接内并发无收益且必挂），任务用毕归还复用（避免每条文件重做 SSH 握手），
/// 批次结束统一关闭。并发数为 1 时直接复用主会话（其内部 _sshLock 串行），不额外建连接。
/// </summary>
public sealed class TransferBatchConnections : IDisposable
{
    private readonly int _max;
    private readonly SemaphoreSlim _gate = new(1, 1);
    // 按远端会话分组的空闲连接（借出即移出, 归还即放回）
    private readonly Dictionary<SftpSessionService, List<TransferConnection>> _idle = new();
    // 本批次打开过的全部连接（结束时统一关闭）
    private readonly List<TransferConnection> _all = new();

    public TransferBatchConnections(int maxConcurrent)
    {
        _max = Math.Max(1, maxConcurrent);
    }

    /// <summary>借一条该侧远端的传输连接（空闲复用，无空闲则新建）；串行模式直接返回主会话。</summary>
    public async Task<IRemoteTransferOps> AcquireAsync(SftpSessionService session)
    {
        if (_max <= 1)
        {
            return session;
        }

        TransferConnection? conn = null;
        await _gate.WaitAsync();
        try
        {
            if (_idle.TryGetValue(session, out var list) && list.Count > 0)
            {
                conn = list[^1];
                list.RemoveAt(list.Count - 1);
            }
        }
        finally
        {
            _gate.Release();
        }

        if (conn is null)
        {
            // 建连在锁外（SSH 握手可达数秒, 不阻塞其他 worker 借还）
            conn = await session.OpenTransferConnectionAsync();
            await _gate.WaitAsync();
            try
            {
                _all.Add(conn);
            }
            finally
            {
                _gate.Release();
            }
        }

        return conn;
    }

    /// <summary>归还连接回空闲池（串行模式传回主会话时为空操作）。</summary>
    public void Release(SftpSessionService session, IRemoteTransferOps ops)
    {
        if (ops is not TransferConnection conn)
        {
            return;
        }

        _gate.Wait();
        try
        {
            if (!_idle.TryGetValue(session, out var list))
            {
                _idle[session] = list = new List<TransferConnection>();
            }

            list.Add(conn);
        }
        finally
        {
            _gate.Release();
        }
    }

    public void Dispose()
    {
        foreach (var conn in _all)
        {
            conn.Dispose();
        }

        _all.Clear();
        _idle.Clear();
    }
}

/// <summary>
/// 并发传输执行器：worker 池（SemaphoreSlim 限流）消费一批已入队的传输任务。
/// 冲突决策阶段完成后整批提交；Begin/Complete/Fail 由执行器统一推进——入队后保持
/// "等待"直到 worker 领取，单项失败不影响其余任务（失败项进"传输失败"页签）。
/// 全局控制（队列右键菜单，跨窗格/跨批次共享单例语义）：暂停 = 取走暂停门许可，
/// worker 在门上挂起（非轮询），不领取新任务（正在传输的让其传完）；继续 = 归还
/// 许可；停止 = 归还许可 + 取消令牌，未领取的任务按"已停止"记失败，正在传输的
/// 让其传完（SSH.NET 同步 API 无法安全中断）。
/// </summary>
public static class ConcurrentTransferService
{
    /// <summary>暂停门（全局）：开放态持有 1 个许可。暂停时取走，worker 挂起在
    /// 门上（0 CPU 空转）；worker 通过检查点后立即归还，一次仅串行通过一个
    /// （检查点只有几条指令，不构成吞吐瓶颈）。</summary>
    private static readonly SemaphoreSlim _gate = new(1, 1);

    /// <summary>暂停门标志（volatile 保证 worker 可见；Pause/Resume/Stop 仅
    /// UI 线程串行调用，标志本身无竞态）。暂停/停止入口按此防重复取还许可。</summary>
    private static volatile bool _paused;

    /// <summary>停止令牌：批次开始时快照，停止时换新 CTS 并取消旧的（跨批次全局）。</summary>
    private static CancellationTokenSource _cts = new();

    /// <summary>是否处于暂停（队列右键菜单"继续传输"判定用）。</summary>
    public static bool Paused => _paused;

    /// <summary>暂停：正在传输的任务传完，等待中的在门上挂起不开始。</summary>
    public static void Pause()
    {
        if (_paused)
        {
            return;
        }

        _paused = true;
        _gate.Wait(); // 取走许可: 最多等一个正通过检查点的 worker 归还（瞬时）
    }

    /// <summary>继续：归还暂停门许可，worker 恢复领取任务。</summary>
    public static void Resume()
    {
        if (!_paused)
        {
            return;
        }

        _paused = false;
        _gate.Release();
    }

    /// <summary>停止：解除暂停（归还门让挂起 worker 醒来）并取消后续任务
    /// （正在传输的让其传完；挂起的 worker 经取消令牌唤醒后按"已停止"记失败）。</summary>
    public static void RequestStop()
    {
        if (_paused)
        {
            _paused = false;
            try
            {
                _gate.Release();
            }
            catch (SemaphoreFullException)
            {
                // 门已被并发 Resume 归还: 不影响停止语义（令牌取消仍生效）
            }
        }

        Interlocked.Exchange(ref _cts, new CancellationTokenSource())?.Cancel();
    }

    /// <summary>
    /// 并发执行一批传输任务。item.Run 接受批次连接池（借还传输连接）；任一异常按
    /// 单项失败记账。返回 (完成数, 失败数)。
    /// </summary>
    public static async Task<(int Done, int Failed)> RunAsync(
        IReadOnlyList<(Guid QueueId, Func<TransferBatchConnections, Task> Run)> items,
        int maxConcurrency, TransferBatchConnections connections)
    {
        var token = _cts.Token; // 批次开始时快照, 之后 RequestStop 不影响本批已领取判定
        var failed = 0;
        using var throttle = new SemaphoreSlim(Math.Max(1, maxConcurrency));
        var workers = new Task[items.Count];
        for (var i = 0; i < items.Count; i++)
        {
            var (queueId, run) = items[i];
            workers[i] = RunOneAsync(queueId, run, throttle, connections, token,
                () => Interlocked.Increment(ref failed));
        }

        await Task.WhenAll(workers);
        return (items.Count - failed, failed);
    }

    private static async Task RunOneAsync(
        Guid queueId, Func<TransferBatchConnections, Task> run, SemaphoreSlim throttle,
        TransferBatchConnections connections, CancellationToken token, Action onFailed)
    {
        await throttle.WaitAsync();
        try
        {
            // 暂停门: 挂起等待而非轮询——暂停期间不领取新任务 (正在传输的让其传完)，
            // 恢复/停止时经许可归还或取消令牌唤醒。循环仅覆盖"暂停置位与取门之间"
            // 的边界竞态 (worker 已归还门但 Pause 尚未取走)，常态一次通过。
            while (true)
            {
                await _gate.WaitAsync(token);
                _gate.Release();
                if (!_paused)
                {
                    break;
                }
            }

            token.ThrowIfCancellationRequested();
            TransferQueueService.Begin(queueId);
            await run(connections);
            TransferQueueService.Complete(queueId);
        }
        catch (OperationCanceledException)
        {
            TransferQueueService.Fail(queueId, LocalizationService.GetString("QStoppedReason"));
            onFailed();
        }
        catch (Exception ex)
        {
            TransferQueueService.Fail(queueId, ex.Message);
            onFailed();
        }
        finally
        {
            throttle.Release();
        }
    }
}
