using iFreeSftp.Services;
using Xunit;

namespace iFreeSftp.Tests;

/// <summary>
/// HostStoreService 密码保护纯函数回归：
/// 修复"DPAPI 解密失败 → Unprotect 返回 "" → Save 覆写原密文 → 密码永久丢失"。
/// 链接源码编译（零漂移），直调 internal Protect/Unprotect，不走 Load/Save 全流程
/// （StorePath 指向真实用户数据，避免污染）。
/// </summary>
public class HostStoreServiceTests
{
    [Fact]
    public void Protect_IsIdempotent()
    {
        var once = HostStoreService.Protect("秘密pw");
        Assert.StartsWith("enc:", once);

        // 核心回归：已带密文前缀的原样返回，不被二次加密
        Assert.Equal(once, HostStoreService.Protect(once));
    }

    [Theory]
    [InlineData("")]
    [InlineData(null)]
    public void Protect_EmptyOrNull_PassesThrough(string? password)
    {
        // 原样透传语义：null 进 null 出，"" 进 "" 出（null! 抑制可空告警，被测方契约按原样返回处理）
        Assert.Equal(password, HostStoreService.Protect(password!));
    }

    [Fact]
    public void Protect_Unprotect_Roundtrip()
    {
        Assert.Equal("pw123", HostStoreService.Unprotect(HostStoreService.Protect("pw123")));
    }

    [Theory]
    [InlineData("旧版明文pw")]
    [InlineData("")]
    [InlineData(null)]
    public void Unprotect_LegacyPlaintextOrEmpty_PassesThrough(string? stored)
    {
        Assert.Equal(stored ?? "", HostStoreService.Unprotect(stored));
    }

    [Theory]
    [InlineData("enc:AAAA")]  // 合法 Base64 但 DPAPI 解不开（CryptographicException）
    [InlineData("enc:!!!")]   // 非法 Base64（FormatException）
    [InlineData("enc:abc")]   // Base64 长度不对（FormatException）
    public void Unprotect_Undecryptable_ReturnsOriginalCipher(string stored)
    {
        // 修复前返回 ""，随后 Save 会把 "" 覆写回存档 → 密码永久丢失
        Assert.Equal(stored, HostStoreService.Unprotect(stored));
    }

    [Fact]
    public void UndecryptableCipher_FlowsThroughSave_WithoutLoss()
    {
        // 模拟 Load → Save 全链路（纯函数级）：解密失败保留在内存的原密文
        // 流经 Protect 落盘时必须原样写回，不得二次加密也不得清空
        const string corrupted = "enc:AAAA";
        var inMemory = HostStoreService.Unprotect(corrupted);
        Assert.Equal(corrupted, inMemory);
        Assert.Equal(corrupted, HostStoreService.Protect(inMemory));
    }
}
