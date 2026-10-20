using iFreeSftp.Models;
using Xunit;

namespace iFreeSftp.Tests;

// 版本比较: RegistrationService 更新提醒的判定核心
public class VersionCompareTests
{
    [Theory]
    [InlineData("0.9.0", "0.8.0", true)]
    [InlineData("0.8.0", "0.9.0", false)]
    [InlineData("0.8.0", "0.8.0", false)]          // 相等不算新
    [InlineData("0.10.0", "0.9.0", true)]          // 逐段数字比较, 非字符串序
    [InlineData("1.0", "1.0.0", false)]            // 段数不足补 0
    [InlineData("1.0.0.1", "1.0.0", true)]
    [InlineData("0.8.0", "1.0", false)]
    [InlineData("abc", "0.0.1", false)]            // 非数字段按 0
    [InlineData("v1.2", "1.1", false)]             // 前缀污染段按 0, 不误报更新
    public void IsNewer_ReturnsExpected(string remote, string local, bool expected)
        => Assert.Equal(expected, VersionCompare.IsNewer(remote, local));
}

// Unix 权限位 → 符号串: 权限对话框显示
public class PermissionTextTests
{
    [Theory]
    [InlineData(0b111_101_101, "rwxr-xr-x")]       // 755
    [InlineData(0b110_100_100, "rw-r--r--")]       // 644
    [InlineData(0b111_111_111, "rwxrwxrwx")]       // 777
    [InlineData(0b000_000_000, "---------")]       // 000
    [InlineData(0b100_000_000, "r--------")]       // 400
    [InlineData(0b000_000_001, "--------x")]       // 001
    [InlineData(0b100_111_101_101, "rwxr-xr-x")]   // 4755: 仅取 9 位权限, setuid 位忽略
    public void Symbolic_ReturnsExpected(int mode, string expected)
        => Assert.Equal(expected, PermissionText.Symbolic(mode));
}

// 远程 (SFTP) 路径拼接: 文件列表进入子目录
public class RemotePathTests
{
    [Theory]
    [InlineData("/home/user", "a.txt", "/home/user/a.txt")]
    [InlineData("/", "a.txt", "/a.txt")]           // 根目录不产生双斜杠
    [InlineData("/home/user/", "a.txt", "/home/user/a.txt")]  // 尾斜杠去除
    [InlineData("/home//", "a.txt", "/home/a.txt")]           // 多个尾斜杠去除
    public void Join_ReturnsExpected(string dir, string name, string expected)
        => Assert.Equal(expected, RemotePath.Join(dir, name));
}

// 主机 tab 标题 ("名称 (IP)"): 连接落位即写入, hover tooltip 显示全文
public class HostTabNamingTests
{
    [Theory]
    [InlineData("我的服务器", "192.168.1.10", "我的服务器 (192.168.1.10)")]
    [InlineData("  web  ", "  example.com  ", "web (example.com)")]  // 两端空白裁剪
    [InlineData("", "10.0.0.1", "10.0.0.1")]                          // 名称空只显示地址
    [InlineData(null, "10.0.0.1", "10.0.0.1")]
    [InlineData("10.0.0.1", "10.0.0.1", "10.0.0.1")]                  // 名称与地址相同不重复
    [InlineData("vps", null, "vps")]                                  // 地址缺失回退名称
    [InlineData(null, null, "")]                                      // 双空: 空标题
    [InlineData("   ", "", "")]
    public void Build_ReturnsExpected(string? name, string? host, string expected)
        => Assert.Equal(expected, HostTabNaming.Build(name, host));
}
