using iFreeSftp.Core.Abstractions;

namespace iFreeSftp.Core.Stub;

/// <summary>
/// 社区构建降级实现（开源仓库可完整编译）：无闭源 Core DLL 时的占位实现。
/// 全部 IsFullImplementation=false，调用方据此走降级路径（直接回退/提示官方构建），
/// 保证开源构建可用但核心能力仅存在于官方发布物。
/// </summary>
public sealed class StubTransferCore : ITransferCore
{
    public bool IsFullImplementation => false;

    /// <summary>降级：不提供服务端直达复制，调用方直接回退本地中转。</summary>
    public Task<bool> ServerCopyAsync(
        IRemoteExec exec, string remoteSrc, string remoteDst, bool sourceIsDirectory, bool destExists)
        => Task.FromResult(false);
}

/// <summary>注册核心降级：不支持激活（验签真实现闭源，见技术文档分层方案）。</summary>
public sealed class StubRegistrationCore : IRegistrationCore
{
    public bool IsFullImplementation => false;

    /// <summary>降级：不支持验签（调用方先查 IsFullImplementation，不会走到这里）。</summary>
    public RegistrationVerifyResult Verify(string code)
        => new(false, RegistrationErrorKind.Format, string.Empty, 0, 0, string.Empty);
}

/// <summary>Pro 功能降级占位。</summary>
public sealed class StubProFeature : IProFeature
{
    public bool IsFullImplementation => false;
}
