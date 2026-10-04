//! 配对、成员与信任共享的 Core 安全领域类型。
//!
//! 这里只放与算法无关的值对象。具体密码学派生（SHA-256、公钥的 Base32 编码、KDF）
//! 属于 Infra 层的 `uc-infra-crypto`。

pub mod identity_fingerprint;

pub use identity_fingerprint::{FingerprintError, IdentityFingerprint};
