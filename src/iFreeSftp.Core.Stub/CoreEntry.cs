using iFreeSftp.Core.Abstractions;
using iFreeSftp.Core.Stub;

namespace iFreeSftp.Core;

/// <summary>
/// 核心装配入口（与官方闭源 iFreeSftp.Core.dll 内同名同类、签名一致）：
/// 主程序统一经 CoreEntry 取三件套实例，调用点零条件编译——
/// 社区构建（引用本程序集）解析到降级实现，官方构建（引用 Core.dll）解析到完整实现。
/// </summary>
public static class CoreEntry
{
    public static ITransferCore CreateTransfer() => new StubTransferCore();

    public static IRegistrationCore CreateRegistration() => new StubRegistrationCore();

    public static IProFeature CreateProFeature() => new StubProFeature();
}
