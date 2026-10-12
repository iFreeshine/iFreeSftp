using iFreeSftp.Services;
using Xunit;

namespace iFreeSftp.Tests;

// SM3 (GB/T 32905-2016) 标准向量: 空串与附录 A 三条经 OpenSSL 独立实现复核
public class Sm3Tests
{
    [Theory]
    [InlineData("", "1ab21d8355cfa17f8e61194831e81a8f22bec8c728fefb747ed035eb5082aa2b")]
    [InlineData("abc", "66c7f0f462eeedd9d1f2d46bdc10e4e24167c4875cf2f7a2297da02b8f4ba8e0")]
    [InlineData("abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd",
        "debe9ff92275b8a138604889c18e5a4d6fdb70e5387e5765293dcba39c0c5732")]
    [InlineData("abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabc",
        "952eb84cacee9c10bde4d6882d29d63140ba72af6fe485085095dccd5b872453")]
    public void Hash_StandardVectors(string message, string expected)
        => Assert.Equal(expected, HashOf(message));

    [Fact]
    public void Hash_StreamingChunksMatchWhole()
    {
        var data = System.Text.Encoding.ASCII.GetBytes(
            "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabc");

        // 7B / 33B / 27B 三段追加, 覆盖块内缓冲、整块压缩、跨块收尾三条路径
        var sm3 = new Sm3();
        sm3.AppendData(data.AsSpan(0, 7).ToArray(), 7);
        sm3.AppendData(data.AsSpan(7, 33).ToArray(), 33);
        var rest = data.AsSpan(40).ToArray();
        sm3.AppendData(rest, rest.Length);

        Assert.Equal(HashOf(data), sm3.GetHashHex());
    }

    private static string HashOf(string message)
        => HashOf(System.Text.Encoding.UTF8.GetBytes(message));

    private static string HashOf(byte[] data)
    {
        var sm3 = new Sm3();
        sm3.AppendData(data, data.Length);
        return sm3.GetHashHex();
    }
}
