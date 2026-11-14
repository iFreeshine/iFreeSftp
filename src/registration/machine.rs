//! 机器标识与哈希（C# MachineId / CodeHash 对应物）—— 装配 shim。
//!
//! 真实现（SHA-256 截断哈希 + 平台机器名/用户名 FFI）随闭源 core 分发：
//! - 正式发布构建（--features official）：委托 `crate::ifree_core::registration::machine`；
//! - 源码编译版（无 ifree-core 目录）：钉子恒空串——本形态无激活/上报能力，
//!   machine_id 消费点（激活上报）不可达；code_hash 仅吊销名单比对用，
//!   恒为空串哈希口径，永不命中真实吊销名单（与「未注册」语义一致）。

/// 机器标识（激活上报 m 字段）。
pub(crate) fn machine_id() -> String {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::registration::machine::machine_id()
    }
    #[cfg(not(feature = "official"))]
    {
        String::new()
    }
}

/// 注册码哈希（h 字段 / 吊销名单 key）。
pub(crate) fn code_hash(code: &str) -> String {
    #[cfg(feature = "official")]
    {
        crate::ifree_core::registration::machine::code_hash(code)
    }
    #[cfg(not(feature = "official"))]
    {
        let _ = code;
        String::new()
    }
}
