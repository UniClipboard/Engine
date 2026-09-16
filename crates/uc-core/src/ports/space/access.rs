//! 空间访问端口。
//!
//! 以"用户口令视角"覆盖空间的初始化 / 解锁 / 加锁 / 就绪查询 /
//! 工厂重置 / 静默恢复 / 子密钥派生 / pairing offer 与 proof 派生。签名里
//! 只出现领域中性类型（`Passphrase` / `ActiveSpace` / `JoinOffer` /
//! `ProofDerivedKey`），密钥物料（KEK / MasterKey / KeySlot /
//! WrappedMasterKey 等）一律在 adapter 内部生成和持有，不穿过这一层。
//!
//! 本模块同时定义内层聚合 trait [`SpaceAccessStore`]（adapter 实现一次）
//! 与一组窄意图端口（每个表示一个业务动作）。应用层消费方只依赖窄端口，
//! 不依赖聚合 store（ports.md §12）。
//!
//! 合并了原先的 `EncryptionPort`（KDF + wrap/unwrap 部分）、
//! `EncryptionSessionPort`、`KeyMaterialPort`、以及已删除的 `space::CryptoPort`
//! 的三个方法（`generate_nonce32 / export_keyslot_blob / derive_master_key_from_keyslot`）。

use async_trait::async_trait;

use crate::crypto::domain::{ActiveSpace, Passphrase};
use crate::ids::SpaceId;
use crate::space_access::{JoinOffer, PreparedAdmissionTargetAccess, ProofDerivedKey};

/// 业务语义级的空间访问失败。
///
/// 故意不带密码学细节（algo / nonce / tag / blob 偏移等）——
/// adapter 内部可以保留更细的错误，这里只暴露调用方需要分支处理的类别。
#[derive(Debug, thiserror::Error)]
pub enum SpaceAccessError {
    /// 空间尚未初始化，无法解锁或换口令。
    #[error("space not initialized")]
    NotInitialized,

    /// 空间已初始化，不能再次 `initialize`。
    #[error("space already initialized")]
    AlreadyInitialized,

    /// 口令不匹配——unlock 时校验失败。
    #[error("wrong passphrase")]
    WrongPassphrase,

    /// 空间当前未解锁，无法执行需要会话的操作。
    #[error("space not unlocked")]
    NotUnlocked,

    /// 持久化的密钥物料损坏或版本不支持——属于数据层故障，不可恢复。
    #[error("space key material corrupted or unsupported")]
    CorruptedKeyMaterial,

    /// 已验证的 Space 安全状态无法完成耐久 catalog 与活动会话安装。
    #[error("space security state could not be activated")]
    SecurityState {
        #[source]
        source: anyhow::Error,
    },

    /// 其它内部故障（底层 IO / 算法实现异常等）。
    #[error("space access internal error: {0}")]
    Internal(String),
}

/// Inner aggregate surface for space access.
///
/// 所有方法都以"空间"为中心——adapter 需要把 `SpaceId` 作为内部
/// 会话/密钥物料查找的键，外部调用方不感知 KEK / MasterKey。
///
/// This is the low-level store (ports.md §5.1/§12): a single adapter
/// implements it once and the narrow space-access intent ports below delegate
/// to it. Application-layer consumers depend on the narrow ports, never on this
/// aggregate.
#[async_trait]
pub trait SpaceAccessStore: Send + Sync {
    /// 首次初始化一个空间。
    ///
    /// 语义：
    /// - 生成全新的密钥物料并用口令派生的 KEK 加以包装，持久化落盘。
    /// - 完成后内存会话进入"已解锁"状态，返回 `ActiveSpace` 作为凭据。
    /// - 若该空间已初始化过，应返回 [`SpaceAccessError::AlreadyInitialized`]。
    async fn initialize(
        &self,
        space_id: &SpaceId,
        passphrase: &Passphrase,
    ) -> Result<ActiveSpace, SpaceAccessError>;

    /// 用口令解锁一个已初始化的空间。
    ///
    /// 语义：
    /// - 从持久化中读出包装后的密钥物料，用 `passphrase` 解包。
    /// - 成功后内存会话持有解包后的密钥，返回 `ActiveSpace`。
    /// - 口令错误返回 [`SpaceAccessError::WrongPassphrase`]。
    /// - 空间未初始化返回 [`SpaceAccessError::NotInitialized`]。
    async fn unlock(
        &self,
        space_id: &SpaceId,
        passphrase: &Passphrase,
    ) -> Result<ActiveSpace, SpaceAccessError>;

    /// 查询当前是否已解锁。
    async fn is_unlocked(&self, space_id: &SpaceId) -> bool;

    /// 清除内存会话——持久化密钥物料不受影响,后续仍可 `unlock`。
    async fn lock(&self, space_id: &SpaceId) -> Result<(), SpaceAccessError>;

    /// 静默尝试从持久化层（keyring 缓存）恢复会话——startup 路径专用。
    ///
    /// 行为：
    /// - 空间从未初始化：返回 `Ok(None)`（不视为错误,调用方据此判断是否需要引导用户首次 setup）；
    /// - 已初始化且 keyring 命中：解锁成功,返回 `Ok(Some(ActiveSpace))`；
    /// - keyring 缓存丢失 / 权限不足 / 密钥物料损坏：返回相应 [`SpaceAccessError`]。
    ///
    /// 与 `unlock` 的区别：本方法**不接受口令**,完全依赖 keyring 缓存——
    /// 适合 startup 静默恢复;若 keyring 失效（如用户清除 / 跨设备导入）,
    /// 调用方应回退到带口令的 `unlock` 路径。
    async fn try_resume_session(
        &self,
        space_id: &SpaceId,
    ) -> Result<Option<ActiveSpace>, SpaceAccessError>;

    /// 从当前会话派生 32 字节子密钥（HKDF-SHA256,IKM = MasterKey）。
    ///
    /// 调用方决定 `salt`（一般是某种业务作用域,如 profile_id）和 `info`
    /// （区分用途的字符串,如 `"uniclipboard-search-index/v1"`）。adapter
    /// 内部用 IKM = master_key 派生,不暴露 master_key 字节。
    ///
    /// 用途：搜索索引 SearchKey 派生、未来其它需要派生密钥的场景。
    /// 会话未解锁时返回 [`SpaceAccessError::NotUnlocked`]。
    async fn derive_subkey(&self, salt: &[u8], info: &[u8]) -> Result<[u8; 32], SpaceAccessError>;

    /// 取出当前已解锁会话的 proof 链路凭据(包装的 master_key 字节)。
    ///
    /// 仅供 sponsor 侧 `ProofPort::verify_proof` 在 cache miss
    /// （进程重启 / cache 失效）时重新计算 HMAC——它需要拿到
    /// build_proof 时使用的同一份字节。
    ///
    /// 行为:
    /// - 会话已解锁: `Ok(Some(ProofDerivedKey))`
    /// - 会话未解锁: `Ok(None)` (调用方应当作 verify 失败处理)
    /// - 内部 IO 失败: `Err(Internal)`
    ///
    /// 注: 这是"读取已解锁会话的 master_key 字节"的窄接口,与 joiner 侧的
    /// `derive_master_key_for_proof` (从 JoinOffer + Passphrase 派生) 形成对称。
    async fn current_session_proof_key(&self) -> Result<Option<ProofDerivedKey>, SpaceAccessError>;

    /// Sponsor 侧：准备 pairing offer。
    ///
    /// 读取/生成该空间的 keyslot 序列化字节 + 产生 32 字节挑战 nonce,打包给 joiner。
    ///
    /// 注：签名保留 `passphrase` 参数以忠实反映当前 sponsor 侧"准备 offer =
    /// 顺带首次初始化（若未初始化）"的行为。若未来拆分"已初始化 sponsor 只读
    /// offer"vs"首次初始化 + 建 offer"两种语义,应在独立的清理阶段进行。
    async fn prepare_join_offer(
        &self,
        space_id: &SpaceId,
        passphrase: &Passphrase,
    ) -> Result<JoinOffer, SpaceAccessError>;

    /// Joiner 侧：用口令解开 offer 的 keyslot 字节,派生出构造 proof 所需的不透明凭据。
    ///
    /// 返回的 `ProofDerivedKey` 是只在本次 pairing proof 链路里有意义的
    /// 32 字节秘密——后续直接喂给 `ProofPort::build_proof` 计算 HMAC,
    /// 领域代码无需也无法把它当作 `MasterKey` 转用到其它路径。
    async fn derive_master_key_for_proof(
        &self,
        offer: &JoinOffer,
        passphrase: &Passphrase,
    ) -> Result<ProofDerivedKey, SpaceAccessError>;
}

// ─── space-access intent ports ───────────────────────────────────────────
//
// Narrow, single-responsibility views over space access. Each represents one
// business action and each consumer depends only on the slice it actually
// uses (ports.md §4.1/§8.1/§8.2). The concrete adapter implements every one
// of them and the composition root coerces a single instance into each
// (ports.md §8.3); `SpaceAccessStore` above remains the inner aggregate the
// impls delegate to (ports.md §12).

/// Prepare target-local access material without changing the active space.
#[async_trait]
pub trait PrepareAdmissionTargetAccessPort: Send + Sync {
    async fn prepare_target_access(
        &self,
        target_space_id: &SpaceId,
        passphrase: &Passphrase,
    ) -> Result<PreparedAdmissionTargetAccess, SpaceAccessError>;
}

/// Derive a purpose-scoped 32-byte subkey from the unlocked session.
#[async_trait]
pub trait DeriveSpaceSubkeyPort: Send + Sync {
    /// Derive a 32-byte subkey bound to the current session's key material,
    /// scoped by caller-chosen `salt` and `info`. The same inputs always
    /// yield the same bytes; a different `info` yields an independent subkey.
    ///
    /// Returns [`SpaceAccessError::NotUnlocked`] when the session is locked.
    async fn derive_subkey(&self, salt: &[u8], info: &[u8]) -> Result<[u8; 32], SpaceAccessError>;
}

/// Read the proof credential of the currently unlocked session.
#[async_trait]
pub trait CurrentSessionProofKeyPort: Send + Sync {
    /// Return the opaque proof credential derived from the currently unlocked
    /// session, or `None` when the session is locked.
    ///
    /// The returned [`ProofDerivedKey`] is the same secret a joiner derives
    /// from a [`JoinOffer`] via [`DeriveProofKeyPort`], so both sides compute
    /// matching proofs.
    async fn current_session_proof_key(&self) -> Result<Option<ProofDerivedKey>, SpaceAccessError>;
}

/// Build a pairing offer that lets another device join a space.
#[async_trait]
pub trait PrepareJoinOfferPort: Send + Sync {
    /// Build a [`JoinOffer`] for `space_id`: the serialized key material plus
    /// a fresh challenge nonce.
    ///
    /// When the space is not yet initialized this first-time-initializes it
    /// with `passphrase` (leaving the session unlocked); when it is already
    /// initialized the existing material is read and `passphrase` is ignored.
    async fn prepare_join_offer(
        &self,
        space_id: &SpaceId,
        passphrase: &Passphrase,
    ) -> Result<JoinOffer, SpaceAccessError>;
}

/// Derive the proof credential needed to join a space from an offer.
#[async_trait]
pub trait DeriveProofKeyPort: Send + Sync {
    /// Unwrap the key material carried by `offer` using `passphrase` and
    /// return the opaque [`ProofDerivedKey`] used to answer the offer's
    /// challenge. Symmetric counterpart to
    /// [`CurrentSessionProofKeyPort::current_session_proof_key`].
    ///
    /// Returns [`SpaceAccessError::WrongPassphrase`] when the passphrase does
    /// not match the offer, and [`SpaceAccessError::CorruptedKeyMaterial`]
    /// when the offer payload cannot be parsed.
    async fn derive_master_key_for_proof(
        &self,
        offer: &JoinOffer,
        passphrase: &Passphrase,
    ) -> Result<ProofDerivedKey, SpaceAccessError>;
}
