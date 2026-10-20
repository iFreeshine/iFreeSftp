using iFreeSftp.Models;
using Xunit;

namespace iFreeSftp.Tests;

// 断点续传决策: T1 的安全核心——"目标严格小于源才追加"防缝合文件
public class ResumePolicyTests
{
    [Theory]
    // 上传: 远端半成品 (0 < remote < local) → 追加
    [InlineData(1000, 400, true, true, ResumeAction.Append)]
    // 下载: 本地半成品——调用方把远端大小作源、本地大小作目标传入, 归一到同一判定
    [InlineData(5000, 4999, true, true, ResumeAction.Append)]
    // 目标不存在 → 全量
    [InlineData(1000, -1, false, true, ResumeAction.Overwrite)]
    [InlineData(1000, 0, false, true, ResumeAction.Overwrite)]
    // 续传关闭 → 全量（零行为变化）
    [InlineData(1000, 400, true, false, ResumeAction.Overwrite)]
    // 大小相等 → 全量: 无法区分"恰好传完"与"同尺寸的另一份内容", 重传是唯一不丢数据的答案
    [InlineData(1000, 1000, true, true, ResumeAction.Overwrite)]
    // 目标比源大 → 冲突（内容已变的强信号; 调用方按全量处理并记日志, 不静默追加）
    [InlineData(400, 1000, true, true, ResumeAction.Conflict)]
    // 空源文件: 目标存在任意大小都不追加
    [InlineData(0, 0, true, true, ResumeAction.Overwrite)]
    [InlineData(0, 5, true, true, ResumeAction.Conflict)]
    // 目标存在但为 0 字节: "从 0 追加"等于全量, 按全量走（队列不显示误导性"续传中"）
    [InlineData(1000, 0, true, true, ResumeAction.Overwrite)]
    public void Decide_ReturnsExpected(long source, long dest, bool destExists, bool enabled, ResumeAction expected)
        => Assert.Equal(expected, ResumePolicy.Decide(source, dest, destExists, enabled));

    // 1 字节之差也要追加: 半成品判定不放过任何可续传量
    [Fact]
    public void Decide_SingleByteSmaller_Appends()
        => Assert.Equal(ResumeAction.Append, ResumePolicy.Decide(101, 100, true, true));

    // 防御性: destExists=false 但 size>=0（调用方矛盾）时按不存在处理, 绝不追加
    [Fact]
    public void Decide_DestFlagFalse_OverwritesEvenWithPlausibleSize()
        => Assert.Equal(ResumeAction.Overwrite, ResumePolicy.Decide(1000, 500, destExists: false, resumeEnabled: true));

    // —— 续传阈值: 低于阈值的小文件只能全量重传（重传代价可忽略, 阈值宜取大不取小）——
    [Theory]
    // 源 999B < 阈值 1000B: 本可追加, 强制全量
    [InlineData(999, 500, ResumeAction.Overwrite)]
    // 源恰等于阈值: 不"小于"阈值, 正常追加
    [InlineData(1000, 500, ResumeAction.Append)]
    // 源大于阈值: 正常追加
    [InlineData(2000, 500, ResumeAction.Append)]
    // 阈值之下目标更大仍是冲突（内容已变信号优先, 不被阈值吞掉）
    [InlineData(100, 500, ResumeAction.Conflict)]
    public void Decide_Threshold_SmallSourceOverwrites(long source, long dest, ResumeAction expected)
        => Assert.Equal(expected, ResumePolicy.Decide(source, dest, true, true, thresholdBytes: 1000));

    // 默认阈值 0 = 功能关闭, 既有行为零变化
    [Fact]
    public void Decide_DefaultThreshold_KeepsLegacyBehavior()
        => Assert.Equal(ResumeAction.Append, ResumePolicy.Decide(101, 100, true, true));
}

// POSIX shell 单引号转义: exec 命令拼接用户可控路径前的唯一入口（T12 与 rm -rf 共用）
public class ShellQuoteTests
{
    [Theory]
    [InlineData("/var/www", "'/var/www'")]
    [InlineData("/var/my site", "'/var/my site'")]                 // 空格
    [InlineData("/var/a'b", "'/var/a'\\''b'")]                     // 单引号: 关闭-转义-重开
    [InlineData("/var/a;rm -rf /", "'/var/a;rm -rf /'")]           // 分号命令串: 引号内无效
    [InlineData("/var/$(id)", "'/var/$(id)'")]                     // 命令替换: 单引号不展开
    [InlineData("/var/a\"b", "'/var/a\"b'")]                       // 双引号无需转义
    [InlineData("/var/`id`", "'/var/`id`'")]                       // 反引号同理
    [InlineData("", "''")]                                         // 空串: 空参数
    public void Quote_NeutralizesInjection(string input, string expected)
        => Assert.Equal(expected, ShellQuote.Quote(input));

    // 转义结果自身必须仍是"单引号包裹的一段"——首尾为引号且内部无未转义裸引号
    [Fact]
    public void Quote_ResultIsSingleQuotedSegment()
    {
        var q = ShellQuote.Quote("/a'b'c");
        Assert.StartsWith("'", q, StringComparison.Ordinal);
        Assert.EndsWith("'", q, StringComparison.Ordinal);
        // 去掉首尾引号后, 剩下的引号必须都成对出现（'\'' 结构）
        var inner = q[1..^1];
        var count = inner.Count(c => c == '\'');
        Assert.Equal(0, count % 2);
    }
}
