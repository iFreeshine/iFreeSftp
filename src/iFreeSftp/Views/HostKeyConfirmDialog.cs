using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using iFreeSftp.Services;

namespace iFreeSftp.Views;

/// <summary>
/// 主机指纹确认对话框：正式连接首次遇到未知主机密钥时展示 SHA256 指纹，
/// 用户信任后由调用方把指纹存入主机档案（记住，下次自动校验）。
/// </summary>
internal static class HostKeyConfirmDialog
{
    /// <summary>UI 线程直接调用：展示确认框，返回是否信任。</summary>
    public static async Task<bool> ShowAsync(UIElement owner, string host, string fingerprint)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = owner.XamlRoot,
            Title = LocalizationService.GetString("HostKeyTitle"),
            Content = string.Format(
                LocalizationService.GetString("HostKeyMessage"), host, fingerprint),
            PrimaryButtonText = LocalizationService.GetString("HostKeyTrust"),
            CloseButtonText = LocalizationService.GetString("Cancel"),
            DefaultButton = ContentDialogButton.Primary,
        };
        return await dialog.ShowAsync() == ContentDialogResult.Primary;
    }

    /// <summary>
    /// SSH.NET 握手线程调用：把确认框调度到 UI 线程并同步等待结果
    /// （服务在 HostKeyReceived 事件里对该 Task GetResult）。
    /// </summary>
    public static Task<bool> ShowFromWorkerAsync(UIElement owner, string host, string fingerprint)
    {
        var completion = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        owner.DispatcherQueue.TryEnqueue(async () =>
        {
            try
            {
                completion.SetResult(await ShowAsync(owner, host, fingerprint));
            }
            catch (Exception ex)
            {
                completion.SetException(ex);
            }
        });
        return completion.Task;
    }

    /// <summary>
    /// 主机密钥变更确认框：有存档指纹但本次不匹配时展示新旧指纹。
    /// 默认按钮落在「拒绝」——无法确认变更真实性时宁可断连 (防中间人)。
    /// </summary>
    public static async Task<bool> ShowChangeAsync(UIElement owner, string host, string stored, string presented)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = owner.XamlRoot,
            Title = LocalizationService.GetString("HostKeyChangedTitle"),
            Content = string.Format(
                LocalizationService.GetString("HostKeyChangedMessage"), host, stored, presented),
            PrimaryButtonText = LocalizationService.GetString("HostKeyChangedTrust"),
            CloseButtonText = LocalizationService.GetString("Cancel"),
            DefaultButton = ContentDialogButton.Close,
        };
        return await dialog.ShowAsync() == ContentDialogResult.Primary;
    }

    /// <summary>SSH.NET 握手线程调用：把密钥变更确认框调度到 UI 线程并同步等待结果。</summary>
    public static Task<bool> ShowChangeFromWorkerAsync(UIElement owner, string host, string stored, string presented)
    {
        var completion = new TaskCompletionSource<bool>(TaskCreationOptions.RunContinuationsAsynchronously);
        owner.DispatcherQueue.TryEnqueue(async () =>
        {
            try
            {
                completion.SetResult(await ShowChangeAsync(owner, host, stored, presented));
            }
            catch (Exception ex)
            {
                completion.SetException(ex);
            }
        });
        return completion.Task;
    }
}
