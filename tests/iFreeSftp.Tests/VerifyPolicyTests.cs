using iFreeSftp.Models;
using iFreeSftp.Services;
using Xunit;

namespace iFreeSftp.Tests;

// 传输后校验判定纯函数 (T2): 本地值 / 远端值 / 算法可用性 → 状态
public class VerifyPolicyTests
{
    [Theory]
    [InlineData("abc123", "abc123", true, VerifyResultKind.Match)]
    [InlineData("ABC123", "abc123", true, VerifyResultKind.Match)]   // 大小写不敏感
    [InlineData("abc123", "abc124", true, VerifyResultKind.Mismatch)]
    [InlineData("abc123", "abc123", false, VerifyResultKind.Skipped)] // 算法不可用
    [InlineData(null, "abc123", true, VerifyResultKind.Skipped)]
    [InlineData("abc123", null, true, VerifyResultKind.Skipped)]
    [InlineData(null, null, true, VerifyResultKind.Skipped)]
    [InlineData("", "abc123", true, VerifyResultKind.Skipped)]
    [InlineData("   ", "abc123", true, VerifyResultKind.Skipped)]
    public void Decide_ReturnsExpected(string? local, string? remote, bool available, VerifyResultKind expected)
        => Assert.Equal(expected, VerifyPolicy.Decide(local, remote, available));

    [Theory]
    [InlineData("Auto", true, true, true, "SHA256")]     // Auto 顺位回退
    [InlineData("Auto", false, true, true, "SHA512")]
    [InlineData("Auto", false, false, true, "SM3")]
    [InlineData("SM3", true, true, true, "SM3")]         // 显式指定且可用
    [InlineData("SHA256", false, true, true, null)]      // 显式指定不可用→null
    [InlineData("SM3", true, true, false, null)]
    [InlineData("Auto", false, false, false, null)]      // 全不可用
    [InlineData("SHA256", false, false, false, null)]
    [InlineData("MD5", true, true, true, null)]          // 未知值安全跳过
    public void PickAlgorithm_ReturnsExpected(string preferred, bool s256, bool s512, bool sm3, string? expected)
        => Assert.Equal(expected, VerifyPolicy.PickAlgorithm(preferred, s256, s512, sm3));

    // 设置默认值: 功能默认关闭 + 算法默认 Auto + 续传阈值默认 1MB（测试进程不调 Load, 默认值可靠）
    [Fact]
    public void Defaults_OffAndAuto()
    {
        Assert.False(AppSettingsService.VerifyAfterTransfer);
        Assert.Equal("Auto", AppSettingsService.VerifyAlgorithm);
        Assert.Equal(1024, AppSettingsService.ResumeThresholdKb);
    }
}
