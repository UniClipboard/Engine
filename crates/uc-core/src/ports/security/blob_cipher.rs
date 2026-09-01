//! 业务 blob 加解密 port。
//!
//! 对本机持久业务负载执行加解密。调用方只提供负载和业务实体 AAD；
//! adapter 独占活动写入上下文与密文自描述读取规则。签名里不出现 Space、
//! MasterKey、算法标签或版本号等上下文选择能力。
//!
//! 合并了原先 `EncryptionPort::{encrypt_blob, decrypt_blob}` 的职责。
//! 传输分片场景（chunked + 内置压缩 + per-chunk wire-format AAD）已经
//! 由独立的 `TransferCipherPort` 承担——AAD 模型与 wire format 无法与
//! 本 port 共享，因此保持两套 port 并行。

use async_trait::async_trait;

use crate::crypto::domain::{Aad, Ciphertext, Plaintext};

/// 业务语义级的数据加解密失败。
///
/// 故意保持粗粒度——调用方一般只需要区分"还能不能继续用这个空间"和
/// "数据本身坏了"。算法细节 / AEAD tag 失败 / nonce 结构问题全部
/// 归到 `InvalidCiphertext`，由 adapter 在日志里补更细的信息。
#[derive(Debug, thiserror::Error)]
pub enum BlobCipherError {
    /// 当前持久内容保护上下文不可用，例如会话已经被锁定。
    #[error("space session is no longer unlocked")]
    NotUnlocked,

    /// 密文本身损坏 / AAD 不匹配 / 解包失败——数据层故障。
    #[error("invalid ciphertext or aad mismatch")]
    InvalidCiphertext,

    /// 其它内部失败（底层算法库、IO 等）。
    #[error("blob cipher internal error: {0}")]
    Internal(String),
}

/// 业务 blob 加解密 port。
///
/// 方法契约：
/// - 加密成功返回 `Ciphertext`——不透明字节，包含 adapter 自描述的 nonce / tag 布局。
/// - 解密成功返回 `Plaintext`——drop 时自动清零。
/// - AAD 由调用方按业务实体规则构造，adapter 负责把它绑定到不可由调用方
///   选择的持久保护上下文。
#[async_trait]
pub trait BlobCipherPort: Send + Sync {
    async fn encrypt(
        &self,
        plaintext: &Plaintext,
        aad: &Aad,
    ) -> Result<Ciphertext, BlobCipherError>;

    async fn decrypt(
        &self,
        ciphertext: &Ciphertext,
        aad: &Aad,
    ) -> Result<Plaintext, BlobCipherError>;
}
