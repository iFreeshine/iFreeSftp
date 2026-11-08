//! 传输核心三层（范式照 registration/core.rs）：开源侧只留 trait 定义 + Stub +
//! 装配，决策逻辑在 ifree-core 闭源层（official feature 并入）。
//!
//! C# 同构：本文件 = core-src iFreeSftp.Core.Abstractions（ITransferCore）；
//! ifree-core/src/transfer.rs = iFreeSftp.Core FullTransferCore（闭源）；
//! Stub（源码编译版）= Core.Stub——服务端直达不可用，调度侧显式提示「不含服务端直达复制组件」，
//! 远程→远程保底路径 = 本地中转（RelayFile/RelayDir，开源，C# 同形态）。
//!
//! trait 零 async：ServerCopy 的执行（exec 调用 + 600s 掐表）留在调度点
//! （transfer.rs run_task，tokio 上下文），trait 只收发纯决策——ifree-core
//! 因此保持零依赖、可独立 cargo test。

/// 传输核心抽象（C# ITransferCore）。返回值语义见各方法。
pub trait TransferCore: Send + Sync {
    /// 是否满血实现（C# IsFullImplementation）：false = Stub，调度侧跳过直达。
    fn is_full(&self) -> bool;

    /// 组装服务端 cp 命令（C# BuildServerCopyCommand）。Stub 恒 Err。
    fn server_copy_cmd(
        &self,
        src: &str,
        dst: &str,
        is_dir: bool,
        dest_exists: bool,
    ) -> Result<String, &'static str>;

    /// 成功判定（C# JudgeServerCopy）：exit 0 且 CP_OK 回执。
    fn server_copy_ok(&self, exit: i32, output: &str) -> bool;

    /// 命令超时秒数（C# ServerCopyTimeoutSeconds=600）。
    fn server_copy_timeout_secs(&self) -> u64;
}

/// 源码编译版存根（C# Core.Stub）：全部决策不可用，调度侧显式提示「不含服务端直达复制组件」，
/// 远程→远程任务走本地中转保底路径。
pub struct StubTransferCore;

impl TransferCore for StubTransferCore {
    fn is_full(&self) -> bool {
        false
    }
    fn server_copy_cmd(&self, _: &str, _: &str, _: bool, _: bool) -> Result<String, &'static str> {
        Err("server-copy-gated")
    }
    fn server_copy_ok(&self, _: i32, _: &str) -> bool {
        false
    }
    fn server_copy_timeout_secs(&self) -> u64 {
        0
    }
}

/// 正式发布构建真实现：决策逻辑委托 ifree-core（C# FullTransferCore）。
/// 仅 official 编译（源码编译版无 ifree-core 目录，零引用完整可编译——registration 同范式）。
#[cfg(feature = "official")]
pub struct OfficialTransferCore;

#[cfg(feature = "official")]
impl TransferCore for OfficialTransferCore {
    fn is_full(&self) -> bool {
        true
    }
    fn server_copy_cmd(
        &self,
        src: &str,
        dst: &str,
        is_dir: bool,
        dest_exists: bool,
    ) -> Result<String, &'static str> {
        Ok(crate::ifree_core::transfer::server_copy_cmd(
            src,
            dst,
            is_dir,
            dest_exists,
        ))
    }
    fn server_copy_ok(&self, exit: i32, output: &str) -> bool {
        crate::ifree_core::transfer::server_copy_ok(exit, output)
    }
    fn server_copy_timeout_secs(&self) -> u64 {
        crate::ifree_core::transfer::SERVER_COPY_TIMEOUT_SECS
    }
}

/// 装配入口（C# CoreEntry：源码编译版=Stub / 正式发布构建=Full）。cfg! 分支在编译期折叠，
/// 未选中的结构体名同样经 cfg 门控引用（源码编译版无 OfficialTransferCore 符号）。
pub fn make_transfer_core() -> Box<dyn TransferCore> {
    #[cfg(feature = "official")]
    {
        Box::new(OfficialTransferCore)
    }
    #[cfg(not(feature = "official"))]
    {
        Box::new(StubTransferCore)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 无 official/有 official 通用：Stub 语义（源码编译版下 make_transfer_core 必装 Stub）。
    /// Stub 不含任何 cp 命令字面量——源码编译版源码/二进制零出现 cp 命令串。
    #[test]
    fn stub_gates_server_copy() {
        let core = make_transfer_core();
        if cfg!(feature = "official") {
            assert!(core.is_full());
        } else {
            // 显式门禁 + 恒不判定成功（调度侧据此走「不含服务端直达复制组件」提示）
            assert!(!core.is_full());
            assert_eq!(
                core.server_copy_cmd("/a", "/b", true, false),
                Err("server-copy-gated")
            );
            assert!(!core.server_copy_ok(0, "CP_OK"));
        }
    }

    /// 正式发布构建专用：装配层拿到的是闭源决策逻辑（C# FullTransferCore 委托）。
    /// 无 official 时整个 cfg 掉，命令串不出现在源码编译版产物中。
    #[cfg(feature = "official")]
    #[test]
    fn official_offers_server_copy() {
        let core = make_transfer_core();
        assert_eq!(core.server_copy_timeout_secs(), 600);
        assert_eq!(
            core.server_copy_cmd("/a", "/b", true, false).unwrap(),
            "cp -a -- '/a' '/b' && echo CP_OK"
        );
        assert!(core.server_copy_ok(0, "CP_OK"));
        assert!(!core.server_copy_ok(0, ""));
    }
}
