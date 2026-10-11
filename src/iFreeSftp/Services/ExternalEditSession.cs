using Microsoft.UI.Dispatching;

namespace iFreeSftp.Services;

/// <summary>
/// 外部编辑器远程编辑会话：外部编辑器没有保存回调（内置编辑器有），以
/// FileSystemWatcher 监视本地临时副本检测"编辑器保存了"——任意文件事件重置
/// 防抖定时器（编辑器安全保存多为临时文件改名/多段写入，事件成簇到达），静默
/// 期结束后确认写锁已释放、快照（长度+mtime）确实变化，才触发一次回调询问回传。
/// 回调在 UI 线程执行；确认与否都已在触发时更新基准快照——同一次保存不重复询问，
/// 之后再保存快照再次变化，会再次询问。
/// </summary>
public sealed class ExternalEditSession : IDisposable
{
    /// <summary>防抖静默期（毫秒）：编辑器保存的最后一簇事件后等待此时长再检测。</summary>
    private const int DebounceMs = 800;

    private readonly string _path;
    private readonly Action _onChange; // UI 线程回调（检测到保存）
    private readonly DispatcherQueue _dq;
    private readonly FileSystemWatcher _watcher;
    private readonly DispatcherQueueTimer _timer;
    private (long Length, long UtcTicks) _baseline;
    private bool _disposed;

    public ExternalEditSession(string localPath, DispatcherQueue dq, Action onChange)
    {
        _path = localPath;
        _dq = dq;
        _onChange = onChange;
        _baseline = Snapshot();

        _timer = dq.CreateTimer();
        _timer.Interval = TimeSpan.FromMilliseconds(DebounceMs);
        _timer.IsRepeating = false;
        _timer.Tick += (_, _) => _ = OnQuietAsync();

        _watcher = new FileSystemWatcher(Path.GetDirectoryName(localPath)!, Path.GetFileName(localPath))
        {
            NotifyFilter = NotifyFilters.LastWrite | NotifyFilters.Size | NotifyFilters.FileName,
        };
        // Changed/Created/Renamed/Deleted 都只作"有动静"信号: 保存 = 旧事件簇结束,
        // 最终状态以静默期后的快照为准 (编辑器安全保存的临时名改名也会触发 Renamed)
        _watcher.Changed += (_, _) => RestartTimer();
        _watcher.Created += (_, _) => RestartTimer();
        _watcher.Renamed += (_, _) => RestartTimer();
        _watcher.EnableRaisingEvents = true;
    }

    /// <summary>防抖到期：写锁已释放且快照变化 → 更新基准并回调（否则丢弃本次静默期）。</summary>
    private async Task OnQuietAsync()
    {
        if (_disposed)
        {
            return;
        }

        // 编辑器保存常持独占写锁: 打不开说明还在写, 推迟一个静默期再查 (自旋重排)
        try
        {
            using var _ = File.Open(_path, FileMode.Open, FileAccess.Read, FileShare.Read);
        }
        catch (IOException)
        {
            RestartTimer();
            return;
        }
        catch
        {
            return; // 文件暂不可读等非锁原因: 等下一簇事件
        }

        var now = Snapshot();
        if (now == _baseline)
        {
            return; // 快照未变 (无保存 / 取消保存), 不询问
        }

        _baseline = now; // 无论回传与否都以最新快照为基准: 同一次保存只询问一次
        _dq.TryEnqueue(() => _onChange());
    }

    /// <summary>重置防抖静默期（事件到达即推迟检测; 运行中重复 Start 无效, 须先 Stop）。</summary>
    private void RestartTimer()
    {
        if (_disposed)
        {
            return;
        }

        _timer.Stop();
        _timer.Start();
    }

    private (long, long) Snapshot()
    {
        try
        {
            var info = new FileInfo(_path);
            return (info.Length, info.LastWriteTimeUtc.Ticks);
        }
        catch
        {
            return (0, 0);
        }
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _timer.Stop();
        _watcher.Dispose();
    }
}
