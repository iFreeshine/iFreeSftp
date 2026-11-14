//! 注册凭据存取（C# RegistrationService DataDir + ProtectedData 对应物）—— 装配 shim。
//!
//! 存取流程决策（enc: 前缀契约 / fail-closed / 原子写盘 / 禁止明文落盘）随闭源
//! core 分发（`ifree-core/src/registration/store.rs`）；本文件只做类型装配：
//! - 正式发布构建：trait 与 DpapiStore/NoopStore 自 core 再导出，DPAPI 加解密
//!   原语由宿主 hosts.rs 经函数指针注入（crypt32 FFI 本就在开源侧，hosts.json
//!   同用——core 保持零 FFI）；
//! - 源码编译版（无 ifree-core 目录）：钉子 NoopStore——无激活能力故无存取
//!   需求，不落盘、重启后未注册（与发布版「未激活」可观测行为一致）。

use std::sync::Arc;

/// 正式发布构建：凭据存取类型随闭源 core 分发（Windows DPAPI 后端）。
#[cfg(all(feature = "official", windows))]
pub(crate) use crate::ifree_core::registration::store::{CredentialStore, DpapiStore};

/// 正式发布构建（非 Windows）：占位后端随闭源 core 分发。
#[cfg(all(feature = "official", not(windows)))]
pub(crate) use crate::ifree_core::registration::store::{CredentialStore, NoopStore};

/// 源码编译版：trait 钉子（接口面，无逻辑）。
#[cfg(not(feature = "official"))]
pub(crate) trait CredentialStore: Send + Sync {
    /// 读取注册码原文。None = 未注册或解密失败（调用方一律按未注册处理）。
    fn load_code(&self) -> Option<String>;
    /// 保存注册码原文（实现负责加密落盘）。
    fn save_code(&self, code: &str) -> std::io::Result<()>;
}

/// 源码编译版：占位后端钉子——红线「无加密能力绝不落盘」：save 恒 Ok 且不写
/// 任何文件（激活态仅本会话有效），load 恒 None（重启后未注册，需重新激活）。
#[cfg(not(feature = "official"))]
pub(crate) struct NoopStore;

#[cfg(not(feature = "official"))]
impl CredentialStore for NoopStore {
    fn load_code(&self) -> Option<String> {
        None
    }
    fn save_code(&self, _code: &str) -> std::io::Result<()> {
        Ok(()) // 设计性不落盘（见 struct 注释）
    }
}

/// 生产装配（service 启动唯一入口）：Windows DPAPI 文件后端（路径 = 注册体系
/// 数据目录，与 hosts/settings 同目录；原语注入 hosts.rs）/ 其余形态 NoopStore。
#[cfg(feature = "official")]
pub(crate) fn default_store() -> Arc<dyn CredentialStore> {
    #[cfg(windows)]
    {
        Arc::new(DpapiStore::at_path(
            crate::app_paths::data_dir().join("registration.dat"),
            crate::hosts::protect_pub,
            crate::hosts::unprotect_pub,
        ))
    }
    #[cfg(not(windows))]
    {
        Arc::new(NoopStore)
    }
}

#[cfg(not(feature = "official"))]
pub(crate) fn default_store() -> Arc<dyn CredentialStore> {
    Arc::new(NoopStore)
}
