using System;
using System.Threading.Tasks;

namespace iFreeSftp.Core.Abstractions;

/// <summary>
/// 远程命令通道适配：核心传输逻辑（服务端 cp 直达等）经此向远程服务器下发 shell 命令，
/// 不与 SSH.NET 类型耦合——主程序由 SftpSessionService 适配实现。
/// </summary>
public interface IRemoteExec
{
    /// <summary>执行 shell 命令并等待退出（timeoutSeconds 为上限秒数；超时/通道异常抛异常）。</summary>
    Task<(int ExitStatus, string Output, string Error)> RunCommandAsync(string command, int timeoutSeconds = 10);
}

/// <summary>
/// 传输核心（闭源模块边界）：同主机服务端直达拷贝、后续的目录展开调度与
/// 跨主机中转等差异化传输逻辑由此接口承载。真实现位于本地闭源仓库 iFreeSftp.Core
/// （官方构建引用），开源构建使用 Core.Stub 降级实现。
/// </summary>
public interface ITransferCore
{
    /// <summary>当前实现是否为完整传输核心。Stub=false → 调用方直接走中转回退路径。</summary>
    bool IsFullImplementation { get; }

    /// <summary>
    /// 同主机服务端直达复制（走命令通道 cp，零本地流量）。
    /// 语义：文件覆盖 cp -f；目录新建副本 cp -a；与已有目录合并 cp -a src/. dst/。
    /// 返回 false = 服务器不支持（Windows 服务器等）或执行失败，由调用方回退本地中转。
    /// </summary>
    Task<bool> ServerCopyAsync(
        IRemoteExec exec, string remoteSrc, string remoteDst, bool sourceIsDirectory, bool destExists);
}

/// <summary>注册码验签失败种类（错误文案由调用方按种类映射到本地化资源）。</summary>
public enum RegistrationErrorKind
{
    None,
    Format,      // 前缀/分段/Base64/payload 布局非法
    Signature,   // 验签失败（公钥不匹配或载荷被改）
    Expired,     // 有效期已过
}

/// <summary>注册码 payload 解析结果（等级/有效期取自签名载荷，改本地文件无法伪造）。</summary>
public readonly record struct RegistrationVerifyResult(
    bool Ok,
    RegistrationErrorKind Error,
    string UserName,
    long IssuedAt,
    long ExpiresAt,
    string Tier);

/// <summary>
/// 注册核心（闭源模块边界）：ECDsa P-256 验签、注册码 payload 解析（公钥内嵌核心，离线可用）。
/// 注意：Stub 不提供激活能力——验签真实现若随开源分发，闭源即失去意义。
/// </summary>
public interface IRegistrationCore
{
    /// <summary>当前实现是否提供激活能力。Stub=false → 注册页提示使用官方构建。</summary>
    bool IsFullImplementation { get; }

    /// <summary>验签 + 解析注册码（IFREE1.&lt;签名&gt;.&lt;载荷&gt;）。失败时 Ok=false 并给出错误种类。</summary>
    RegistrationVerifyResult Verify(string code);
}

/// <summary>
/// Pro 功能挂载点（预留）：未来收费功能的闭源实现统一挂在此边界后，
/// 现仅保留目录骨架，随收费功能立项再细化。
/// </summary>
public interface IProFeature
{
    /// <summary>当前实现是否包含 Pro 功能。Stub=false。</summary>
    bool IsFullImplementation { get; }
}
