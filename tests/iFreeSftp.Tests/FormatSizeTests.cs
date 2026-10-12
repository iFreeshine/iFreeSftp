using iFreeSftp.Models;
using Xunit;

namespace iFreeSftp.Tests;

// 字节大小格式化: 文件列表与状态栏统计共用
public class FormatSizeTests
{
    [Theory]
    [InlineData(0, "0 B")]
    [InlineData(1, "1 B")]
    [InlineData(1023, "1023 B")]                   // 单位边界内保留整数
    [InlineData(1024, "1 KB")]
    [InlineData(1536, "1.5 KB")]
    [InlineData(1048576, "1 MB")]
    [InlineData(1610612736, "1.5 GB")]
    public void FormatSize_ReturnsExpected(long bytes, string expected)
        => Assert.Equal(expected, FileEntry.FormatSize(bytes));

    [Fact]
    public void DisplaySize_DirectoryIsEmpty()
    {
        var dir = new FileEntry { IsDirectory = true, Length = 4096 };
        Assert.Equal(string.Empty, dir.DisplaySize);
    }

    [Fact]
    public void DisplaySize_FileIsFormatted()
    {
        var file = new FileEntry { IsDirectory = false, Length = 1024 };
        Assert.Equal("1 KB", file.DisplaySize);
    }
}
