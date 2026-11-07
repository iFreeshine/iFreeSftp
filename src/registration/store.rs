//! 注册凭据存取（C# RegistrationService DataDir + ProtectedData 对应物）。
//!
//! - registration.dat：文件内容 = DPAPI(当前用户) 加密的注册码原文
//!   （复用 hosts.rs crypt32 FFI，`enc:` 前缀文本；C# 是裸 DPAPI 字节，
//!   格式不同但无需考虑存量迁移，Rust 侧自成一体）；
//! - 解密失败/无文件/空码 = 未注册（fail-closed，与 C# Load 的 catch 回退一致）；
//! - 目录：app_paths::data_dir（与 hosts.json/settings.json 同目录，
//!   C# bare-exe 分支的 DataDir 一致）；
//! - 平台抽象：CredentialStore trait（Windows DPAPI；非 Windows NoopStore
//!   不落盘 + 每次输入 + 可见提示，Keychain/libsecret 后续平台接入时再加）。

use std::path::PathBuf;

/// 凭据存取后端（当前用户范围内的机密持久化；上报线程经 Arc 跨线程访问）。
pub(crate) trait CredentialStore: Send + Sync {
    /// 读取注册码原文。None = 未注册或解密失败（调用方一律按未注册处理）。
    fn load_code(&self) -> Option<String>;
    /// 保存注册码原文（实现负责加密落盘）。
    fn save_code(&self, code: &str) -> std::io::Result<()>;
}

/// Windows DPAPI 文件后端（路径可注入便于测试；生产用 `default_store`）。
pub(crate) struct DpapiStore {
    path: PathBuf,
}

impl DpapiStore {
    pub(crate) fn default_store() -> Self {
        Self { path: dat_path() }
    }

    /// 指定路径构造（service 测试注入用）。
    #[allow(dead_code)] // 仅测试路径消费
    pub(crate) fn at_path(path: PathBuf) -> Self {
        Self { path }
    }
}

impl CredentialStore for DpapiStore {
    fn load_code(&self) -> Option<String> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        if text.is_empty() {
            return None;
        }
        // 解密失败 unprotect 回原样：仍是 "enc:" 密文 → 按未注册（换机/换用户场景）
        let plain = crate::hosts::unprotect_pub(&text);
        if plain.is_empty() || plain.starts_with(crate::hosts::ENC_PREFIX_PUB) {
            return None;
        }
        Some(plain)
    }

    fn save_code(&self, code: &str) -> std::io::Result<()> {
        // S1：加密失败 → Err（禁止明文落盘）；H4：原子写盘 + 目录/写盘错误上抛。
        let stored = crate::hosts::protect_pub(code)
            .map_err(|e| std::io::Error::other(format!("注册码加密失败：{e}")))?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        crate::app_paths::write_atomic(&self.path, stored.as_bytes())
    }
}

/// 非 Windows 占位后端（本期不接 Keychain/libsecret）：红线「无加密能力绝不
/// 落盘」—— save 设计性恒 Ok 且不写任何文件（激活态仅本会话有效），load 恒
/// None（重启后未注册，需重新激活）；激活页有静态可见提示（dialogs.rs cfg 块）。
#[cfg(not(windows))]
pub(crate) struct NoopStore;

#[cfg(not(windows))]
impl CredentialStore for NoopStore {
    fn load_code(&self) -> Option<String> {
        None
    }
    fn save_code(&self, _code: &str) -> std::io::Result<()> {
        Ok(()) // 设计性不落盘（见 struct 注释）
    }
}

/// registration.dat 路径（app_paths::data_dir，与 hosts/settings 同目录）。
fn dat_path() -> PathBuf {
    crate::app_paths::data_dir().join("registration.dat")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> DpapiStore {
        let mut p = std::env::temp_dir();
        p.push(format!("ifree-reg-test-{tag}.dat"));
        let _ = std::fs::remove_file(&p);
        DpapiStore { path: p }
    }

    #[test]
    #[cfg(windows)]
    fn roundtrip_and_fail_closed() {
        // 与 hosts.rs DPAPI 失败桩互斥（进程级全局，防并行用例误伤真 DPAPI）
        let _g = crate::hosts::TEST_DPAPI_LOCK.lock().unwrap();
        let s = temp_store("rt");
        // 无文件 = 未注册
        assert!(s.load_code().is_none());
        // 存取往返
        s.save_code("IFREE1.abc.def").unwrap();
        assert_eq!(s.load_code().as_deref(), Some("IFREE1.abc.def"));
        // 密文文件确实带 enc: 前缀（DPAPI 已加密，非明文）
        let raw = std::fs::read_to_string(&s.path).unwrap();
        assert!(raw.starts_with("enc:"));
        // 篡改密文 → 解密失败按未注册（fail-closed）
        std::fs::write(&s.path, "enc:broken!!").unwrap();
        assert!(s.load_code().is_none());
        // 空文件 = 未注册
        std::fs::write(&s.path, "").unwrap();
        assert!(s.load_code().is_none());
        let _ = std::fs::remove_file(&s.path);
    }

    #[test]
    #[cfg(not(windows))]
    fn noop_store_never_persists() {
        let s = NoopStore;
        assert!(s.load_code().is_none());
        // save 恒 Ok，但不落盘、load 恒 None（激活态仅本会话有效）
        s.save_code("IFREE1.abc.def").unwrap();
        assert!(s.load_code().is_none());
    }
}
