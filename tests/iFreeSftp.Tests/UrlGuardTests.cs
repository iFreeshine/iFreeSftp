using iFreeSftp.Services;
using Xunit;

namespace iFreeSftp.Tests;

// 外部 URL 启动守卫: 远程通道下发的链接必须过 http/https 白名单
public class UrlGuardTests
{
    [Theory]
    [InlineData("https://example.com/path?x=1", true)]
    [InlineData("http://example.com", true)]
    [InlineData("file:///C:/Windows/System32/calc.exe", false)]   // 诱导拉起本地文件
    [InlineData("ftp://example.com", false)]
    [InlineData("javascript:alert(1)", false)]
    [InlineData("customproto://payload", false)]                  // 任意自定义协议
    [InlineData("example.com/path", false)]                       // 相对地址
    [InlineData("", false)]
    [InlineData(null, false)]
    public void IsSafeWebUrl_WhitelistOnly(string? url, bool expected)
        => Assert.Equal(expected, UrlGuard.IsSafeWebUrl(url));

    [Fact]
    public async Task TryLaunchAsync_UnsafeUrlReturnsFalse()
        => Assert.False(await UrlGuard.TryLaunchAsync("file:///C:/x"));
}
