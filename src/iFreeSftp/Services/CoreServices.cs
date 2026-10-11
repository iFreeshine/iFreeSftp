using iFreeSftp.Core;
using iFreeSftp.Core.Abstractions;

namespace iFreeSftp.Services;

/// <summary>
/// 核心三件套实例持有：统一经 iFreeSftp.Core.CoreEntry 装配（stub/official 构建
/// 各自带同名工厂，调用点零条件编译）。社区构建为降级实现（IsFullImplementation=false）。
/// </summary>
public static class CoreServices
{
    /// <summary>传输核心：同主机服务端直达拷贝（官方构建）。</summary>
    public static ITransferCore Transfer { get; } = CoreEntry.CreateTransfer();

    /// <summary>注册核心：注册码验签与解析（官方构建）。</summary>
    public static IRegistrationCore Registration { get; } = CoreEntry.CreateRegistration();

    /// <summary>Pro 功能挂载点（预留）。</summary>
    public static IProFeature Pro { get; } = CoreEntry.CreateProFeature();
}
