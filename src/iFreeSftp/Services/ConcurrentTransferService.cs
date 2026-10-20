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

    /// <summary>查询远端文件大小（字节）；不存在返回 -1（SftpPathNotFoundException 归一）。</summary>
    Task<long> TryGetSizeAsync(string remotePath);

    /// <summary>断点续传上传: 本地源 seek 到 offset, 远端定位续写（不截断）。
    /// offset 必须等于远端当前大小。progress = 含 offset 的累计字节。</summary>
    Task UploadResumeAsync(string localPath, string remotePath, long offset, Action<long>? progress = null);

    /// <summary>断点续传下载: 远端读流 seek 到 offset, 本地追加写（不截断）。
    /// offset 必须等于本地半成品当前大小。progress = 含 offset 的累计字节。</summary>
    Task DownloadResumeAsync(string remotePath, string localPath, long offset, Action<long>? progress = null);

    /// <summary>把本地流写入远端文件（覆盖创建）。tar 加速: 本地 tar 进程 stdout
    /// 直写远端临时文件, 免去"先落本地临时 tar 再整文件上传"的一道磁盘往返。</summary>
    Task UploadStreamAsync(Stream source, string remotePath, Action<long>? progress = null);

    /// <summary>远端重命名（POSIX rename, 文件与目录通用）。tar 加速: 临时目录原子切换。</summary>
    Task RenameAsync(string oldPath, string newPath);
}

/// <summary>
/// 队列任务暂停请求异常：暂停检查点在传输块边界抛出，中止当前文件并留下断点。
/// 必须继承 <see cref="OperationCanceledException"/>——续传包装层的失败兜底 catch
/// 过滤 OCE（当取消而非失败），本异常才能穿透直达 RunOneAsync 按重跑处理，
/// 而不是被当成传输失败立刻触发全量重传。
/// </summary>
public sealed class PauseRequestedException : OperationCanceledException
{
}

/// <summary>
/// 队列任务停止/取消请求异常：全局停止或单任务取消在块边界检查点抛出，
/// 中止当前文件并留下断点（续传包装层按断点结算）。必须继承
/// <see cref="OperationCanceledException"/>——理由同 <see cref="PauseRequestedException"/>，
/// 穿透续传包装层的失败兜底 catch 直达 RunOneAsync 按"已取消"结算。
/// </summary>
public sealed class StopRequestedException : OperationCanceledException
{
}

/// <summary>
/// 可暂停中断的传输操作装饰器（仅队列传输路径使用）：把进度回调统一包装为
/// "先过暂停检查点再转发"。SSH.NET Upload/Download 每传输一块（≤缓冲区大小）触发
/// 一次回调，自写续传循环每块同样触发——暂停置位后下一块内抛
/// <see cref="PauseRequestedException"/>，当前文件中止且对端半成品留作断点。
/// 瞬时操作（建目录/列举/查大小/重命名）直通不包——不构成可感知的传输时长。
/// </summary>
public sealed class PauseInterruptibleOps : IRemoteTransferOps
{
    private readonly IRemoteTransferOps _inner;

    public PauseInterruptibleOps(IRemoteTransferOps inner) => _inner = inner;

    private static Action<long>? Wrap(Action<long>? progress)
        => b =>
        {
            ConcurrentTransferService.ThrowIfCheckpoint();
            progress?.Invoke(b);
        };

    public Task CreateDirectoryAsync(string path) => _inner.CreateDirectoryAsync(path);

    public Task<IReadOnlyList<FileEntry>> ListDirectoryAsync(string path) => _inner.ListDirectoryAsync(path);

    public Task DownloadAsync(string remotePath, string localPath, Action<long>? progress = null)
        => _inner.DownloadAsync(remotePath, localPath, Wrap(progress));

    public Task UploadAsync(string localPath, string remotePath, Action<long>? progress = null)
        => _inner.UploadAsync(localPath, remotePath, Wrap(progress));

    public Task<long> TryGetSizeAsync(string remotePath) => _inner.TryGetSizeAsync(remotePath);

    public Task UploadResumeAsync(string localPath, string remotePath, long offset, Action<long>? progress = null)
        => _inner.UploadResumeAsync(localPath, remotePath, offset, Wrap(progress));

    public Task DownloadResumeAsync(string remotePath, string localPath, long offset, Action<long>? progress = null)
        => _inner.DownloadResumeAsync(remotePath, localPath, offset, Wrap(progress));

    public Task UploadStreamAsync(Stream source, string remotePath, Action<long>? progress = null)
        => _inner.UploadStreamAsync(source, remotePath, Wrap(progress));

    public Task RenameAsync(string oldPath, string newPath) => _inner.RenameAsync(oldPath, newPath);
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

    // 注意: 不用 SftpClient.DownloadFile/UploadFile —— SSH.NET 2026.0.0 在内部经
    // ThreadPool.QueueUserWorkItem 调度进度回调, 回调里抛 PauseRequestedException
    // 会逃逸出 await 链成为 AppDomain 未处理异常直接崩进程, 且传输循环收不到暂停
    // 信号。检查点必须在自有调用栈内抛: 自写读写循环, 每块内联调用 progress,
    // 异常沿本方法返回的 Task 传播到 RunOneAsync 的 catch, 暂停/停止语义才完整。

    public Task DownloadAsync(string remotePath, string localPath, Action<long>? progress = null)
        => Task.Run(() =>
        {
            // OpenRead 对不存在文件抛 SftpPathNotFoundException, 与 DownloadFile 语义一致。
            using var input = _client.OpenRead(remotePath);
            using var output = File.Create(localPath);
            var buffer = new byte[BufferSize];
            long total = 0;
            int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }
        });

    public Task UploadAsync(string localPath, string remotePath, Action<long>? progress = null)
        => Task.Run(() =>
        {
            using var input = File.OpenRead(localPath);
            // FileMode.Create 存在即截断, 与 UploadFile 语义一致。
            using var output = _client.Open(remotePath, FileMode.Create, FileAccess.Write);
            var buffer = new byte[BufferSize];
            long total = 0;
            int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }

            output.Flush();
        });

    public Task<long> TryGetSizeAsync(string remotePath)
        => Task.Run(() =>
        {
            try
            {
                return _client.GetAttributes(remotePath).Size;
            }
            catch (Renci.SshNet.Common.SftpPathNotFoundException)
            {
                return -1L; // 不存在: 归一为 -1 (ResumePolicy 判 Overwrite)
            }
        });

    public Task UploadResumeAsync(string localPath, string remotePath, long offset, Action<long>? progress = null)
        => Task.Run(() =>
        {
            using var input = File.OpenRead(localPath);
            input.Seek(offset, SeekOrigin.Begin);
            // OpenOrCreate + 显式定位续写: 不依赖 SSH_FXF_APPEND 标志（SftpFileStream 的
            // FileMode.Append 映射该标志, 部分老服务器不实现）; SFTP 协议无 FSEEK,
            // SftpFileStream 的 Seek 为客户端位置记账, READ/WRITE 均按位置发起, 是协议基线。
            using var output = _client.Open(remotePath, FileMode.OpenOrCreate, FileAccess.ReadWrite);
            if (output.Length > offset)
            {
                output.SetLength(offset); // 防御: 远端半成品比 offset 长（调用方保证相等, 与下载方向对称）
            }

            output.Seek(offset, SeekOrigin.Begin);
            var buffer = new byte[BufferSize];
            long total = offset;
            int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }

            output.Flush();
        });

    public Task DownloadResumeAsync(string remotePath, string localPath, long offset, Action<long>? progress = null)
        => Task.Run(() =>
        {
            using var output = new FileStream(localPath, FileMode.OpenOrCreate, FileAccess.ReadWrite);
            if (output.Length > offset)
            {
                output.SetLength(offset); // 防御: 本地半成品比 offset 长（调用方保证相等）
            }

            output.Seek(offset, SeekOrigin.Begin);
            using var input = _client.OpenRead(remotePath);
            input.Seek(offset, SeekOrigin.Begin);
            var buffer = new byte[BufferSize];
            long total = offset;
            int read;
            while ((read = input.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }
        });

    public Task UploadStreamAsync(Stream source, string remotePath, Action<long>? progress = null)
        => Task.Run(() =>
        {
            using var output = _client.Open(remotePath, FileMode.Create, FileAccess.Write);
            var buffer = new byte[BufferSize];
            long total = 0;
            int read;
            while ((read = source.Read(buffer, 0, buffer.Length)) > 0)
            {
                output.Write(buffer, 0, read);
                total += read;
                progress?.Invoke(total);
            }

            output.Flush();
        });

    public Task RenameAsync(string oldPath, string newPath)
        => Task.Run(() => _client.RenameFile(oldPath, newPath));

    public void Dispose() => _client.Dispose();
}

/// <summary>
/// 一次批量传输的传输连接池：每路并发各自持有一条独立连接（SftpClient 非线程安全，
/// 单连接内并发无收益且必挂），任务用毕归还复用（避免每条文件重做 SSH 握手），
/// 批次结束统一关闭。一律独立于主会话建连（含并发数为 1）：传输生命周期独立于
/// 标签页, 关 tab（主会话销毁）后传输照常跑完——关 tab 三选"后台继续"依赖此行为。
/// </summary>
public sealed class TransferBatchConnections : IDisposable
{
    private readonly SemaphoreSlim _gate = new(1, 1);
    // 按远端会话分组的空闲连接（借出即移出, 归还即放回）
    private readonly Dictionary<SftpSessionService, List<TransferConnection>> _idle = new();
    // 本批次打开过的全部连接（结束时统一关闭）
    private readonly List<TransferConnection> _all = new();

    /// <summary>借一条该侧远端的传输连接（空闲复用，无空闲则新建）。一律独立于主会话建连。</summary>
    public async Task<IRemoteTransferOps> AcquireAsync(SftpSessionService session)
    {
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

    /// <summary>归还连接回空闲池。</summary>
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
/// worker 在门上挂起（非轮询），不领取新任务；正在传输的任务经
/// <see cref="PauseInterruptibleOps"/> 的块边界检查点抛 <see cref="PauseRequestedException"/>
/// 中止（半成品留断点），任务回"等待"，继续后重跑同一任务自动断点续传；继续 = 归还
/// 许可；停止 = 置位停止标志 + 归还许可 + 取消令牌，传输中的任务在下一个块边界
/// 检查点抛 <see cref="StopRequestedException"/> 按"已取消"结算（半成品留断点，
/// 可重试续传），未领取的任务经取消令牌唤醒同样按"已取消"结算。
/// </summary>
public static class ConcurrentTransferService
{
    /// <summary>传输检查点：由 PauseInterruptibleOps 的进度回调按块边界调用，
    /// 依次检查全局暂停、全局停止、单任务取消，命中即抛对应异常中断当前传输
    /// （半成品留断点）。仅队列传输路径经过；远程编辑等非队列路径不感知。</summary>
    internal static void ThrowIfCheckpoint()
    {
        if (_paused)
        {
            throw new PauseRequestedException();
        }

        if (_stopRequested)
        {
            throw new StopRequestedException();
        }

        var taskId = CurrentTaskId.Value;
        if (taskId.HasValue && TransferQueueService.IsCancelRequested(taskId.Value))
        {
            throw new StopRequestedException();
        }
    }

    /// <summary>暂停门（全局）：开放态持有 1 个许可。暂停时取走，worker 挂起在
    /// 门上（0 CPU 空转）；worker 通过检查点后立即归还，一次仅串行通过一个
    /// （检查点只有几条指令，不构成吞吐瓶颈）。</summary>
    private static readonly SemaphoreSlim _gate = new(1, 1);

    /// <summary>暂停门标志（volatile 保证 worker 可见；Pause/Resume/Stop 仅
    /// UI 线程串行调用，标志本身无竞态）。暂停/停止入口按此防重复取还许可。</summary>
    private static volatile bool _paused;

    /// <summary>全局停止标志（volatile 保证 worker 在块边界检查点可见）。批次开始时
    /// 重置——批次串行（RunAsync 全量等待后才返回），重置不影响未跑完的检查点判定。</summary>
    private static volatile bool _stopRequested;

    /// <summary>当前 worker 正在执行的任务 id（AsyncLocal 随 ExecutionContext 流入
    /// Task.Run 闭包与进度回调）：单任务取消的检查点判定依据，RunOneAsync 领取后
    /// 设置、结算前清理。</summary>
    internal static readonly AsyncLocal<Guid?> CurrentTaskId = new();

    /// <summary>停止令牌：批次开始时快照，停止时换新 CTS 并取消旧的（跨批次全局）。</summary>
    private static CancellationTokenSource _cts = new();

    /// <summary>是否处于暂停（队列右键菜单"继续传输"判定用）。</summary>
    public static bool Paused => _paused;

    /// <summary>暂停：传输中的任务在下一个块边界检查点中止（半成品留断点，任务回
    /// "等待"），等待中的在门上挂起不开始。</summary>
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

    /// <summary>停止：置位停止标志，解除暂停（归还门让挂起 worker 醒来）并取消
    /// 令牌唤醒未领取任务。传输中的任务在下一个块边界检查点抛停止异常按
    /// "已取消"结算（半成品留断点，可重试续传）。</summary>
    public static void RequestStop()
    {
        _stopRequested = true;
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
        _stopRequested = false; // 批次开始时重置停止标志（上一批已全量结束, 串行无重入）
        var token = _cts.Token; // 批次开始时快照, 之后 RequestStop 经检查点/令牌影响本批
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
            while (true)
            {
                // 暂停门: 挂起等待而非轮询——暂停期间不领取新任务，恢复/停止时经
                // 许可归还或取消令牌唤醒。循环仅覆盖"暂停置位与取门之间"
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
                if (!TransferQueueService.Begin(queueId))
                {
                    // 领取前已被取消或移除（Begin 锁内原子判定, 消除 UI 标取消与
                    // worker 领取之间的竞态窗口）
                    return;
                }

                CurrentTaskId.Value = queueId;
                try
                {
                    await run(connections);
                    TransferQueueService.SettleAfterRun(queueId); // T2: 有校验 hook 时转"校验中"异步比对, 不阻塞后续传输
                    break;
                }
                catch (PauseRequestedException)
                {
                    // 块边界检查点中断: 任务回"等待"(显示"已暂停")，继续后重跑同一
                    // 任务——ResumePolicy 按对端半成品大小自动从断点续传。
                    TransferQueueService.RequeueAfterPause(queueId);
                }
                finally
                {
                    CurrentTaskId.Value = null;
                }
            }
        }
        catch (StopRequestedException)
        {
            // 全局停止或单任务取消经块边界检查点触发: 按"已取消"结算（半成品留
            // 断点, 可重试续传）。
            TransferQueueService.Cancel(queueId);
            onFailed();
        }
        catch (OperationCanceledException)
        {
            // 停止令牌取消: 未领取的等待任务经令牌唤醒走此分支, 同样按"已取消"结算。
            TransferQueueService.Cancel(queueId);
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
