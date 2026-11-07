//! 注册/更新体系（迁移自 C# RegistrationService 体系，分层与契约见
//! 2026-11-06 注册更新体系交接意见；C# 源为准，逐项对齐）。
//!
//! 落地顺序：p2 core 分层（本文件 + core.rs）→ p3 machine/store →
//! p4 service（guard 随 p4 落地）→ p5 更新检查 → p6 UI 接线（全部已接线）。

pub mod core;
pub(crate) mod guard;
pub(crate) mod machine;
pub(crate) mod service;
pub(crate) mod store;
