using iFreeSftp.Models;
using Xunit;

namespace iFreeSftp.Tests;

// tar 打包加速的命令构造与阈值（T12）: 用户可控路径（目录名）拼进 exec 命令的
// 唯一入口, 与 ShellQuote 同一套转义; 编排（exec/SFTP/本地进程）需实机验证
public class TarPlanTests
{
    [Theory]
    [InlineData("/var/www", "/tmp/.ifree-tar-abc.tar", "/var/www", "tar -cf '/tmp/.ifree-tar-abc.tar' -C '/var/www' .")]
    [InlineData("/var/my site", "/tmp/x.tar", "/var/my site", "tar -cf '/tmp/x.tar' -C '/var/my site' .")]
    [InlineData("/var/a'b", "/tmp/x.tar", "/var/a'b", "tar -cf '/tmp/x.tar' -C '/var/a'\\''b' .")]
    [InlineData("/var/a;rm -rf /", "/tmp/x.tar", "/var/a;rm -rf /", "tar -cf '/tmp/x.tar' -C '/var/a;rm -rf /' .")]
    public void BuildPackCommand_QuotesAllPaths(string dir, string tarPath, string _, string expected)
        => Assert.Equal(expected, TarPlan.BuildPackCommand(tarPath, dir));

    [Theory]
    [InlineData("/tmp/x.tar", "/var/www", "mkdir -p '/var/www' && tar -xf '/tmp/x.tar' -C '/var/www'")]
    [InlineData("/tmp/x.tar", "/var/a'b", "mkdir -p '/var/a'\\''b' && tar -xf '/tmp/x.tar' -C '/var/a'\\''b'")]
    [InlineData("/tmp/x.tar", "/var/$(id)", "mkdir -p '/var/$(id)' && tar -xf '/tmp/x.tar' -C '/var/$(id)'")]
    public void BuildUnpackCommand_MkdirIdempotentAndQuoted(string tarPath, string destDir, string expected)
        => Assert.Equal(expected, TarPlan.BuildUnpackCommand(tarPath, destDir));

    [Fact]
    public void BuildRemoveCommand_QuotesPath()
        => Assert.Equal("rm -f '/tmp/x y.tar'", TarPlan.BuildRemoveCommand("/tmp/x y.tar"));

    [Fact]
    public void BuildRemoveDirCommand_QuotesPath()
        => Assert.Equal("rm -rf '/var/www/.ifree-tar-abc'", TarPlan.BuildRemoveDirCommand("/var/www/.ifree-tar-abc"));

    [Theory]
    [InlineData("/var/www/site", "/var/www")]      // 常规两级
    [InlineData("/var/www/site/", "/var/www")]     // 尾斜杠
    [InlineData("/site", "/")]                     // 顶层路径 → 根
    [InlineData("/", "/")]                         // 根本身
    [InlineData("/a/b/c/d", "/a/b/c")]
    public void RemoteParent_ReturnsExpected(string path, string expected)
        => Assert.Equal(expected, TarPlan.RemoteParent(path));

    [Theory]
    [InlineData("abc123", ".ifree-tar-abc123.tar")]
    [InlineData("", ".ifree-tar-.tar")]
    public void TempTarName_HidesAndNamespacesByGuid(string guid, string expected)
        => Assert.Equal(expected, TarPlan.TempTarName(guid));

    [Theory]
    [InlineData("abc123", ".ifree-tar-abc123")]
    public void TempDirName_HidesAndNamespacesByGuid(string guid, string expected)
        => Assert.Equal(expected, TarPlan.TempDirName(guid));

    [Theory]
    // 阈值边界: 达到才走, 差一个不走
    [InlineData(true, 50, 50, true)]
    [InlineData(true, 49, 50, false)]
    [InlineData(true, 51, 50, true)]
    // 关闭: 任何数量都不走
    [InlineData(false, 1000, 50, false)]
    [InlineData(false, 50, 50, false)]
    // 未知计数（探测失败 -1）: 一律不走, 不猜
    [InlineData(true, -1, 50, false)]
    [InlineData(true, -1, 0, false)]
    // 0 文件（空目录）: 打包无收益, 不走（空目录由逐文件路径的建目录任务保证）
    [InlineData(true, 0, 50, false)]
    [InlineData(true, 0, 0, false)]
    public void ShouldUse_ReturnsExpected(bool enabled, int fileCount, int minFiles, bool expected)
        => Assert.Equal(expected, TarPlan.ShouldUse(enabled, fileCount, minFiles));

    // 临时 tar 与临时目录同 guid: 成对出现便于清理（上传路径 rename 后 rm -rf 幂等）
    [Fact]
    public void TempNames_ShareGuidForPairedCleanup()
    {
        const string guid = "deadbeef";
        Assert.Contains(guid, TarPlan.TempTarName(guid));
        Assert.Contains(guid, TarPlan.TempDirName(guid));
        Assert.NotEqual(TarPlan.TempTarName(guid), TarPlan.TempDirName(guid));
    }
}
