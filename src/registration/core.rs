//! 注册体系分层（迁移自 C# Core.Abstractions / Core.Stub / core-src 装配）。
//!
//! 判据（交接 §1）：社区源码（无 ifree-core 目录、不带 official feature）
//! `cargo build` 完整可编译，行为 = 源码编译版（激活提示"仅官方构建"、更新检查照常）。
//! 注意：optional path dep 方案不可行——cargo 对缺席的 path 依赖在 feature
//! 关闭时仍会解析报错（2026-11-06 实测）；故 official 经 `#[path]` 源码并入。
//!
//! - trait + Stub 在开源侧：Stub 不提供激活能力——验签真实现若随开源分发，
//!   闭源即失去意义（C# CoreInterfaces.IRegistrationCore 注释同义）；
//! - 官方构建 `--features official`：闭源 ifree-core 源码经 #[path] 并入真验签；
//! - 装配点 make_core() 镜像 C# CoreEntry.CreateRegistration / CoreServices.Registration。

/// 验签失败种类（错误文案由调用方按种类映射本地化资源 —— C# RegistrationErrorKind）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // 社区构建仅构造 Format；None/Signature/Expired 随 official 验签路径与 C# 对齐保留
pub enum ErrorKind {
    None,
    /// 前缀/分段/Base64/载荷布局非法
    Format,
    /// 验签失败（公钥不匹配或载荷被改）
    Signature,
    /// 有效期已过
    Expired,
}

/// 注册码 payload 解析结果（等级/有效期取自签名载荷，改本地文件无法伪造）。
#[derive(Debug, Clone)]
pub struct VerifyResult {
    pub ok: bool,
    pub error: ErrorKind,
    pub user_name: String,
    /// u32 秒（UTC）
    pub issued_at: u32,
    /// u32 秒（UTC）；0 = 永久
    pub expires_at: u32,
    pub tier: String,
}

/// 注册核心（闭源模块边界）：ECDsa P-256 验签、注册码 payload 解析
/// （公钥内嵌核心，离线可用 —— C# IRegistrationCore）。
pub trait RegistrationCore: Send + Sync {
    /// 当前实现是否提供激活能力。Stub=false → 注册页提示使用官方构建。
    fn is_full(&self) -> bool;
    /// 验签 + 解析注册码（IFREE1.<签名>.<载荷>）。失败时 ok=false 并给出错误种类。
    fn verify(&self, code: &str) -> VerifyResult;
}

/// 社区构建（无 official）：验签恒失败 Format，激活不可用 —— C# StubRegistrationCore。
#[cfg(not(feature = "official"))]
struct StubCore;

#[cfg(not(feature = "official"))]
impl RegistrationCore for StubCore {
    fn is_full(&self) -> bool {
        false
    }
    fn verify(&self, _code: &str) -> VerifyResult {
        VerifyResult {
            ok: false,
            error: ErrorKind::Format,
            user_name: String::new(),
            issued_at: 0,
            expires_at: 0,
            tier: String::new(),
        }
    }
}

/// 闭源 ifree-core 并入点已提至 main.rs 顶层（registration/transfer 共用唯一副本）；
/// 本模块经 `crate::ifree_core` 引用。

#[cfg(feature = "official")]
struct OfficialCore;

#[cfg(feature = "official")]
impl RegistrationCore for OfficialCore {
    fn is_full(&self) -> bool {
        true
    }
    fn verify(&self, code: &str) -> VerifyResult {
        // now 由系统时间注入（C# Verify 内取 DateTimeOffset.UtcNow 同语义；
        // ifree-core 以参数收时便于单测）
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let r = crate::ifree_core::verify(code, now);
        VerifyResult {
            ok: r.ok,
            error: match r.error {
                crate::ifree_core::ErrorKind::None => ErrorKind::None,
                crate::ifree_core::ErrorKind::Format => ErrorKind::Format,
                crate::ifree_core::ErrorKind::Signature => ErrorKind::Signature,
                crate::ifree_core::ErrorKind::Expired => ErrorKind::Expired,
            },
            user_name: r.user_name,
            issued_at: r.issued_at,
            expires_at: r.expires_at,
            tier: r.tier,
        }
    }
}

/// 装配点（镜像 C# CoreEntry.CreateRegistration；全程唯一实例由 service 层持有）。
pub fn make_core() -> Box<dyn RegistrationCore> {
    #[cfg(feature = "official")]
    {
        Box::new(OfficialCore)
    }
    #[cfg(not(feature = "official"))]
    {
        Box::new(StubCore)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stub 行为判据：is_full=false，任意输入 Format。
    #[cfg(not(feature = "official"))]
    #[test]
    fn stub_rejects_everything() {
        let c = make_core();
        assert!(!c.is_full());
        let r = c.verify("IFREE1.aaa.bbb");
        assert!(!r.ok);
        assert_eq!(r.error, ErrorKind::Format);
    }

    /// official 判据：真验签通路（正例 + 篡改反例），now 由本测试注入。
    #[cfg(feature = "official")]
    #[test]
    fn official_verifies_roundtrip() {
        use base64::Engine as _;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use ring::rand::SystemRandom;
        use ring::signature::KeyPair;

        let rng = SystemRandom::new();
        let pkcs8 = ring::signature::EcdsaKeyPair::generate_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &rng,
        )
        .unwrap();
        let kp = ring::signature::EcdsaKeyPair::from_pkcs8(
            &ring::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            pkcs8.as_ref(),
            &rng,
        )
        .unwrap();
        let mut payload = vec![1u8, 1];
        payload.extend_from_slice(&1_700_000_000u32.to_be_bytes());
        payload.extend_from_slice(&0u32.to_be_bytes());
        let ub = b"tester";
        payload.push(ub.len() as u8);
        payload.extend_from_slice(ub);
        let sig = kp.sign(&rng, &payload).unwrap();
        let code = format!(
            "IFREE1.{}.{}",
            URL_SAFE_NO_PAD.encode(sig.as_ref()),
            URL_SAFE_NO_PAD.encode(&payload)
        );
        // 经 ifree-core 同一验签通路（公钥注入版），确认 include 装配可达
        let raw = crate::ifree_core::verify_with_key(kp.public_key().as_ref(), &code, 3_000_000_000);
        assert!(raw.ok);
        assert_eq!(raw.user_name, "tester");
        // make_core 走生产公钥（生成密钥与其不匹配 → Signature）
        let c = make_core();
        assert!(c.is_full());
        assert_eq!(c.verify(&code).error, ErrorKind::Signature);
    }
}
