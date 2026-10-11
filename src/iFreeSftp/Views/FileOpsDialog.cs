using Microsoft.UI.Text;
using Microsoft.UI.Xaml;
using Microsoft.UI.Xaml.Controls;
using iFreeSftp.Services;

namespace iFreeSftp.Views;

/// <summary>传输覆盖确认结果。</summary>
public enum TransferConflictResult
{
    /// <summary>覆盖目标 (仅当前这一个条目, 除非同时勾选了"应用到全部")。</summary>
    Overwrite,
    /// <summary>跳过该条目。</summary>
    Skip,
    /// <summary>取消, 终止本批传输。</summary>
    Cancel,
}

/// <summary>
/// 文件操作 ContentDialog 静态类：远程编辑冲突确认 / 传输覆盖确认（项目紧凑弹窗风格）。
/// 重命名已改为自绘卡片弹窗 RenameInputDialog（DeleteConfirmDialog 同构, 双窗格覆盖层）。
/// </summary>
internal static class FileOpsDialog
{
    /// <summary>项目紧凑弹窗风格（Header 失配弹窗同款）: 字阶整体较系统 ContentDialog 小一号——
    /// 标题 16 SemiBold、按钮 12; 正文由调用方以 Body() 或自带字号的复合控件传入。</summary>
    private static ContentDialog CreateCompactDialog(UIElement owner, string title, object content)
    {
        var dialog = new ContentDialog
        {
            XamlRoot = owner.XamlRoot,
            Title = new TextBlock { Text = title, FontSize = 16, FontWeight = FontWeights.SemiBold },
            Content = content,
        };
        // 按钮字号: 覆盖 dialog.Resources 的隐式样式键 (BasedOn 主题原样式保模板,
        // FontSizeProperty 定义在 Control 上), Primary/Secondary/Close 全覆盖。
        // 注意: ResourceDictionary 索引器对缺失键直接抛 "Cannot find resource with
        // given key" (Secondary/CloseButtonStyle 不在主题字典), 必须用 TryGetValue
        foreach (var key in new[] { "DefaultButtonStyle", "SecondaryButtonStyle", "CloseButtonStyle" })
        {
            if (Application.Current.Resources.TryGetValue(key, out var res) && res is Style baseStyle)
            {
                dialog.Resources[key] = new Style(typeof(Button))
                {
                    BasedOn = baseStyle,
                    Setters = { new Setter(Control.FontSizeProperty, 12d) },
                };
            }
        }

        return dialog;
    }

    /// <summary>12 号正文文本块（字符串内容的紧凑字号, 自动换行）。</summary>
    private static TextBlock Body(string text) => new()
    {
        Text = text,
        FontSize = 12,
        TextWrapping = TextWrapping.Wrap,
    };

    /// <summary>远程编辑保存回传冲突确认：远端 mtime 与下载基准不一致时询问是否仍覆盖。
    /// 返回 true = 覆盖上传，false = 取消（保留远端现状）。</summary>
    public static async Task<bool> ShowRemoteConflictAsync(UIElement owner)
    {
        var dialog = CreateCompactDialog(owner,
            LocalizationService.GetString("MenuEdit"),
            Body(LocalizationService.GetString("RemoteEditConflict")));
        dialog.PrimaryButtonText = LocalizationService.GetString("RemoteEditOverwrite");
        dialog.CloseButtonText = LocalizationService.GetString("Cancel");
        dialog.DefaultButton = ContentDialogButton.Primary;
        return await dialog.ShowAsync() == ContentDialogResult.Primary;
    }

    /// <summary>外部编辑器远程编辑：文件监视检测到编辑器已保存，询问是否回传远端。
    /// 返回 true = 保存编辑结果（上传），false = 暂不保存（之后再次保存会再询问）。</summary>
    public static async Task<bool> ShowRemoteEditChangedAsync(UIElement owner, string fileName)
    {
        var dialog = CreateCompactDialog(owner,
            LocalizationService.GetString("MenuEdit"),
            Body(string.Format(LocalizationService.GetString("RemoteEditChanged"), fileName)));
        dialog.PrimaryButtonText = LocalizationService.GetString("RemoteEditSave");
        dialog.CloseButtonText = LocalizationService.GetString("Cancel");
        dialog.DefaultButton = ContentDialogButton.Primary;
        return await dialog.ShowAsync() == ContentDialogResult.Primary;
    }

    /// <summary>
    /// 传输覆盖确认（标准方式）：展示源/目标的大小与修改时间对比，覆盖/跳过/取消
    /// 三选一；"应用到本批所有冲突项"勾选后，覆盖或跳过的决定对整批传输的剩余
    /// 冲突统一生效（队列级决策），否则仅作用于当前这一个文件（单文件操作）。
    /// </summary>
    public static async Task<(TransferConflictResult Result, bool ApplyAll)> ShowTransferConflictAsync(
        UIElement owner, string name, (string Size, string Time) source, (string Size, string Time) target)
    {
        var applyAll = new CheckBox { Content = LocalizationService.GetString("TransferApplyAll") };

        // 源/目标两列信息 (大小 + 修改时间)
        var grid = new Grid { ColumnSpacing = 24 };
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        grid.ColumnDefinitions.Add(new ColumnDefinition { Width = new GridLength(1, GridUnitType.Star) });
        var sourceCol = BuildTransferInfoColumn(LocalizationService.GetString("TransferSource"), source);
        var targetCol = BuildTransferInfoColumn(LocalizationService.GetString("TransferTarget"), target);
        Grid.SetColumn(targetCol, 1);
        grid.Children.Add(sourceCol);
        grid.Children.Add(targetCol);

        var panel = new StackPanel { Spacing = 12 };
        panel.Children.Add(new TextBlock
        {
            Text = string.Format(LocalizationService.GetString("TransferConflictQuestion"), name),
            FontSize = 12,
            TextWrapping = TextWrapping.Wrap,
        });
        panel.Children.Add(grid);
        panel.Children.Add(applyAll);

        var dialog = CreateCompactDialog(owner,
            LocalizationService.GetString("TransferConflictTitle"), panel);
        dialog.PrimaryButtonText = LocalizationService.GetString("TransferOverwrite");
        dialog.SecondaryButtonText = LocalizationService.GetString("TransferSkip");
        dialog.CloseButtonText = LocalizationService.GetString("Cancel");
        dialog.DefaultButton = ContentDialogButton.Primary;
        var result = await dialog.ShowAsync();
        return (result switch
        {
            ContentDialogResult.Primary => TransferConflictResult.Overwrite,
            ContentDialogResult.Secondary => TransferConflictResult.Skip,
            _ => TransferConflictResult.Cancel,
        }, applyAll.IsChecked == true);
    }

    /// <summary>覆盖确认的信息列: 列头 (源/目标) + 大小 + 修改时间。</summary>
    private static StackPanel BuildTransferInfoColumn(string header, (string Size, string Time) info)
    {
        var panel = new StackPanel { Spacing = 4 };
        panel.Children.Add(new TextBlock { Text = header, FontWeight = FontWeights.SemiBold, FontSize = 12 });
        panel.Children.Add(new TextBlock { Text = info.Size, FontSize = 12 });
        panel.Children.Add(new TextBlock { Text = info.Time, FontSize = 12 });
        return panel;
    }
}
