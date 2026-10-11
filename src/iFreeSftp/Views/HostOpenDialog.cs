using iFreeSftp.Services;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;

namespace iFreeSftp.Views;

/// <summary>
/// 主机打开方式询问框：连接主机前询问在右窗口的"当前标签页"还是"新标签页"
/// 打开主机文件列表，可勾选"总是"将所选方式写入设置不再询问。
/// UI 线程直接调用（HostPanel 连接编排流程内），取消时返回 Mode=null。
/// </summary>
public static class HostOpenDialog
{
    /// <summary>弹出询问框。返回 (打开方式 "current"/"new", 是否总是), 取消时 Mode 为 null。</summary>
    public static async Task<(string? Mode, bool Always)> ShowAsync(UIElement owner, string host)
    {
        var alwaysBox = new CheckBox
        {
            Content = LocalizationService.GetString("OpenTabAlways"),
            FontSize = 12,
        };
        var dialog = new ContentDialog
        {
            XamlRoot = owner.XamlRoot,
            Title = string.Format(LocalizationService.GetString("OpenTabTitle"), host),
            Content = new StackPanel
            {
                Spacing = 12,
                Children =
                {
                    new TextBlock
                    {
                        Text = LocalizationService.GetString("OpenTabMessage"),
                        TextWrapping = TextWrapping.Wrap,
                        FontSize = 12,
                    },
                    alwaysBox,
                },
            },
            PrimaryButtonText = LocalizationService.GetString("OpenTabCurrent"),
            SecondaryButtonText = LocalizationService.GetString("OpenTabNew"),
            CloseButtonText = LocalizationService.GetString("Cancel"),
            DefaultButton = ContentDialogButton.Primary,
        };

        var result = await dialog.ShowAsync();
        return result switch
        {
            ContentDialogResult.Primary => ("current", alwaysBox.IsChecked == true),
            ContentDialogResult.Secondary => ("new", alwaysBox.IsChecked == true),
            _ => (null, false),
        };
    }
}
