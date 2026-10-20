using System.Text.Json;
using iFreeSftp.Services;
using Xunit;

namespace iFreeSftp.Tests;

/// <summary>
/// AppSettingsService.MicaEnabled 持久化回归：Mica 开关是窗口骨架层透明化的总开关,
/// 读写错一拍就出现"设置页开了、重启又关了"或"旧存档被误判开启"。覆盖三条路径：
/// Load 对缺失文件/无键旧存档保持现值（默认 false 不被覆盖）/ Save 落盘内容正确且可往返 /
/// SetMicaEnabled 同步触发 SettingsChanged 供窗口骨架层联动。
/// 隔离策略：链接源码编译（零漂移），经 internal SetStorePathForTests 把存档重定向到
/// 每测试独立的临时目录，不触碰真实 %LOCALAPPDATA% 存档；Save 是 500ms 防抖，
/// 测试内一律显式 FlushNow 落盘并在收尾清掉挂起标志（防 Timer 回调在临时目录删除后
/// 重建目录写盘）。AppSettingsService 是进程级静态单例，各测试以"写存档 → Load"自建
/// 起点，断言不依赖执行顺序。
/// </summary>
public class AppSettingsServiceTests
{
    [Fact]
    public void Load_MissingFileOrKey_KeepsExplicitFalseTrue()
    {
        using var store = new TempStore();
        AppSettingsService.SetStorePathForTests(store.StorePath);

        // 存档不存在（全新安装）: Load 直接返回, 不改当前值
        AppSettingsService.Load();
        Assert.False(AppSettingsService.MicaEnabled);

        // 正向对照: 键存在且为布尔时 Load 确实解析（同时把现值拉到 true）
        File.WriteAllText(store.StorePath, """{"micaEnabled": true}""");
        AppSettingsService.Load();
        Assert.True(AppSettingsService.MicaEnabled);

        // 旧版存档无 micaEnabled 键: Load 不覆盖现值（现值已为 true,
        // 若 Load 擅自回置默认 false 即在此暴露）
        File.WriteAllText(store.StorePath, """{"theme":"Dark"}""");
        AppSettingsService.Load();
        Assert.True(AppSettingsService.MicaEnabled);

        // false 同样正确解析
        File.WriteAllText(store.StorePath, """{"micaEnabled": false}""");
        AppSettingsService.Load();
        Assert.False(AppSettingsService.MicaEnabled);

        // 键值非布尔（如字符串）: 按解析分支约定忽略, 保持现值
        File.WriteAllText(store.StorePath, """{"micaEnabled": "yes"}""");
        AppSettingsService.Load();
        Assert.False(AppSettingsService.MicaEnabled);

        AppSettingsService.FlushNow(); // 收尾清防抖挂起, 防残留 Timer 回调写盘
    }

    [Fact]
    public void Save_PersistsMicaEnabled_AndRoundtripsThroughLoad()
    {
        using var store = new TempStore();
        AppSettingsService.SetStorePathForTests(store.StorePath);

        // SetMicaEnabled 走 500ms 防抖 Save, 测试内 FlushNow 强制落盘
        AppSettingsService.SetMicaEnabled(true);
        AppSettingsService.FlushNow();
        using (var doc = JsonDocument.Parse(File.ReadAllText(store.StorePath)))
        {
            Assert.Equal(JsonValueKind.True, doc.RootElement.GetProperty("micaEnabled").ValueKind);
        }

        AppSettingsService.SetMicaEnabled(false);
        AppSettingsService.FlushNow();
        using (var doc = JsonDocument.Parse(File.ReadAllText(store.StorePath)))
        {
            Assert.Equal(JsonValueKind.False, doc.RootElement.GetProperty("micaEnabled").ValueKind);
        }

        // 往返: 落盘结果经 Load 重读还原（重启后设置不丢的核心链路）
        AppSettingsService.SetMicaEnabled(true);
        AppSettingsService.FlushNow();
        AppSettingsService.Load();
        Assert.True(AppSettingsService.MicaEnabled);

        AppSettingsService.FlushNow();
    }

    [Fact]
    public void SetMicaEnabled_RaisesSettingsChanged()
    {
        using var store = new TempStore();
        AppSettingsService.SetStorePathForTests(store.StorePath);

        // 静态事件: 委托存变量, finally 退订, 防 lambda 长存污染其他测试
        var fired = 0;
        Action onChange = () => fired++;
        AppSettingsService.SettingsChanged += onChange;
        try
        {
            // setter 同步触发 (订阅方自行 marshal 到 UI 线程), 两次设置触发两次
            AppSettingsService.SetMicaEnabled(true);
            Assert.Equal(1, fired);

            AppSettingsService.SetMicaEnabled(false);
            Assert.Equal(2, fired);
        }
        finally
        {
            AppSettingsService.SettingsChanged -= onChange;
        }

        AppSettingsService.FlushNow();
    }

    /// <summary>每测试独立的临时存档目录：测试结束整目录删除，不残留临时文件。</summary>
    private sealed class TempStore : IDisposable
    {
        public TempStore()
        {
            Dir = Directory.CreateTempSubdirectory("ifreesftp-settings-").FullName;
        }

        private string Dir { get; }

        /// <summary>存档文件路径（与真实存档同名，语义对应）。</summary>
        public string StorePath => Path.Combine(Dir, "settings.json");

        public void Dispose()
        {
            try
            {
                Directory.Delete(Dir, true);
            }
            catch
            {
                // 删除失败不致命, 残留交给系统 TEMP 清理
            }
        }
    }
}
