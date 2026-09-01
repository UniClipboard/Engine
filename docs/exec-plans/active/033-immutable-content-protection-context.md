# 033 不可变内容保护上下文与一次性密文升级

状态：实施中；前三个基础切片、第四步的安全会话切片、第五步的持久密码接口收窄与 V3 payload 目标格式、第六步的多保护组搜索目标模块，以及第七步截至 production profile payload adapter family 原子选择已完成；启动 gate、双 generation 运行期、真实 promotion 与 CrossSpace clean cutover 尚未完成，032 在本规格完成前暂停实施

本规格取代规格 023 中“CrossSpace 必须把本机历史重封装到目标 MasterKey”的规则，以及规格 028 中“切换前普通本机内容沿用 CrossSpace rebuild/rewrap”的规则。规格 023/028 的准入提交、门禁、恢复和 MLS 成员语义继续有效。

# 1. Overview

当前持久化密文虽然已经携带 `content_key_id` 和 `group_epoch`，但没有携带其不可变的保护组标识。`BlobCipherAdapter`、`EncryptedBlobStore` 和多类派生负载在解密时从唯一 `InMemorySession` 取得当前 `SpaceId` 与当前 content key catalog；V2 AAD 也绑定当前 `SpaceId`。因此 `CrossSpace` 若要让旧本机历史在新 Space 激活后继续可读，只能遍历 SQLite、blob 和派生密文，执行“旧上下文解密、内存明文、新上下文加密”。该行为把可变的活动 Space 变成了不可变历史密文的解释条件。

MLS/OpenMLS 负责群组成员关系、epoch 演进和 content key 分发，不负责改写本机历史。已经取得某个保护组历史密钥的本机，在切换活动 Space 后仍可保留并读取自己的历史；目标 Space 不应因此取得来源 Space 的历史密钥或内容。当前全量 CrossSpace rewrap 既没有增强对已持有历史的撤销能力，又使切换耗时、磁盘占用和失败面随历史规模增长。

本规格把内容保护改为不可变 `ProtectionContextV1`：每份持久密文通过随机 `ContentKeyId`、epoch 和格式版本引用唯一保护上下文，解密由加密 vault 把该引用解析为 `ProtectionGroupId`，purpose 由所属 adapter 的固定类型决定，不根据当前活动 Space 选择密钥。`ProtectionGroupId`、`SpaceId` 和 purpose 不作为新增明文字段写进密文头。本机使用受 profile 稳定密钥保护的 `ProfileContentKeyVault` 保存多个历史保护组的 content key catalog；活动 Space session 只决定新写入和网络发送使用哪个保护组。

当前主 SQLite 同时保存本机历史数据和 membership ledger、关系、准入凭据等 Space 控制面状态。只改变密文格式仍无法在不复制整库的情况下原子切换 Space。因此 033 同时把 profile 数据面与 Space 控制面拆成两个 generation：前者保存跨 Space 保留的本机历史，后者保存只属于一个活动 Space 的成员、凭据、MLS 和协议状态。active manifest 同时引用稳定的 profile data generation 和可替换的 Space control generation。

已有 V1/V2 资料在软件升级时通过一次独立、原子、可恢复的存储升级整体转换为 V3。升级完成后，CrossSpace 只切换 MLS、安全状态和新写入上下文，复用同一 profile 数据 generation，不再扫描、复制或重加密历史业务数据。

# 2. Goals

- 所有新持久化业务密文都通过不透明 `ContentKeyId` 引用并认证不可变 `ProtectionContextV1`，正常读取不依赖当前 `ActiveSpace`，新增明文字段不暴露 `ProtectionGroupId` 或 `SpaceId`。
- 使用 profile 稳定、与 MLS exporter 和活动 Space MasterKey 分离的密钥，加密保存多个 `ProtectionGroupId` 的历史 content key catalog。
- 将 V1/V2 SQLite 密文、受管加密 blob、搜索字段和适用的派生负载通过一次性 V3 升级转换；全过程不把明文写入磁盘，并可在任意持久阶段崩溃后继续。
- 升级后的 CrossSpace 不改变 profile 数据 generation，不遍历历史条目或 blob，不调用 payload rewrap，执行成本不随历史条目数和 blob 总量增长。
- 切换后旧历史继续在本机可读，但不自动进入目标 Space 的同步、重发或成员可见范围；只有用户产生新的分享动作时才使用目标保护组发送。
- 把 profile 数据 generation 与活动 `SpaceControlGeneration` 分开，把现有主 SQLite 中的 Space-scoped 表迁出，使 active manifest 原子切换整个控制面时可以复用同一份 profile 数据。
- 保持持久化默认密文、固定脱敏错误上下文和完整 source chain；任何损坏、缺钥或空间不足都不能通过删除历史继续升级。
- 完成 clean cutover：升级成功后正常运行路径只写 V3，V1/V2 读取只存在于一次性升级模块，旧版本打开 V3 profile 必须在写入前明确失败。

# 3. Non-Goals

- 不让目标 Space、目标成员或网络 peer 自动取得来源保护组的历史 catalog 或本机旧历史。
- 不承诺通过退出、移除成员或切换 Space 撤销设备已经取得的历史明文或历史密钥；MLS 后继 epoch 只约束后继权限。
- 不把网络传输密文与本机持久化密文强制为相同字节格式；接收入库仍可在内存中完成 wire 解密和本地 V3 加密。
- 不在本规格实现历史 key catalog 的自动垃圾回收。033 默认保留已安装历史 catalog，直到 Factory Reset 或后续明确的数据删除与密钥回收规格证明无引用。
- 不自动把旧历史重新归属到目标 Space，也不改变用户主动复制、恢复或再次分享内容时创建新事件的业务语义。
- 不借本次工作重构全部 admission、Reset 或 membership branch 内部结构；规格 032 必须在 033 完成后按新代码重新基线化。
- 不允许长期并存两套正常存储实现，不允许用 lazy per-row migration 把升级成本和失败留给普通读取。
- 不改变 `uc-engine` 对外稳定入口或移动绑定版本关系；内部 Core/Application port 可以为正确的保护 interface 做 clean cutover。

# 4. Current Architecture Context

```text
Component: InMemorySession
Path: crates/uc-infra/src/space/security/session.rs
Responsibility: 同时保存当前 SpaceId、当前 MasterKey、当前 content key id/epoch 和单个 catalog，并为内容、传输、搜索及派生负载解析密钥。
Relationship: BlobCipherAdapter、TransferCipherAdapter、EncryptedBlobStore 与 SpaceAccessAdapter 共享同一 Arc；切换 session 会同时替换活动安全状态和历史内容解密上下文。
```

```text
Component: BlobCipherPort / BlobCipherAdapter
Path: crates/uc-core/src/ports/security/blob_cipher.rs
Path: crates/uc-infra/src/security/blob_cipher_adapter.rs
Responsibility: V2 密文保存 content_key_id 与 group_epoch，AAD 通过 key_epoch_aad 绑定从当前 session 取得的 SpaceId。
Relationship: encrypt/decrypt 都接收 ActiveSpace；decrypt 无法从密文本身选择历史保护组。
```

```text
Component: EncryptedBlobStore / TransferCipherAdapter
Path: crates/uc-infra/src/security/encrypted_blob_store.rs
Path: crates/uc-infra/src/clipboard/chunked_transfer.rs
Responsibility: 前者负责本机受管 blob 的持久密文，后者负责当前 Space 的网络分片密文。
Relationship: 两者当前共享同一 session，但 033 后只有本机持久化读取需要历史 key resolver；网络传输继续只使用活动 MLS/Space 上下文。
```

```text
Component: SqliteSpaceGenerationStore::rewrap_finalized_source
Path: crates/uc-infra/src/security/admission_space_transition.rs
Responsibility: CrossSpace/Reset 的最终快照复制、inline representation、blob、文件路径、传输状态、搜索渲染、active register、目录发布和接收记录重加密。
Relationship: 当前 CrossSpace 的数据面切换成本与本机历史总量线性相关；规格 032 原计划把该实现移动到 payload_rewrap 私有模块。
```

```text
Component: ActiveSpaceGenerationManifestV2
Path: crates/uc-core/src/membership/active_space_generation_manifest.rs
Path: crates/uc-infra/src/security/active_space_generation_manifest_store.rs
Responsibility: 把 SpaceId、keyslot、数据库和安全 generation 绑定为唯一活动指针。
Relationship: 当前 generation 路径同时由 SpaceId 和 generation 派生，导致数据库/blob 物理归属与活动 Space 绑定。
```

```text
Component: SqliteMembershipLedger / Space-scoped repositories
Path: crates/uc-infra/src/space/membership_ledger.rs
Path: crates/uc-infra/src/space/admission/
Path: crates/uc-infra/src/space/security/
Responsibility: 保存成员历史、关系、admission credential、MLS/security 与相关恢复状态。
Relationship: 当前部分控制面表与剪贴板历史共用活动 SQLite executor；若不迁出，CrossSpace 仍必须复制或原地改写整库，无法让 profile data generation 保持不变。
```

```text
Component: AdmissionKeyManager / ProfileKeyWiper
Path: crates/uc-infra/src/security/admission_key_manager.rs
Path: crates/uc-infra/src/security/profile_reset.rs
Responsibility: 使用 secure storage 中的 profile 稳定 admission key 保护恢复和准入 payload；Factory Reset 删除 profile 密钥。
Relationship: 证明仓库已有 profile 稳定密钥范式，但内容 key vault 必须使用独立 key name 和用途，不能直接复用 admission key 或 MLS exporter。
```

当前 CrossSpace 数据流：

1. 暂停来源运行入口并形成来源最终快照。
2. 用来源 session 解密所有受保护业务负载。
3. 用目标 session 重加密到目标 Space generation。
4. 提升同时绑定目标 Space、数据库和安全状态的 V2 manifest。
5. 重绑数据库、blob root 和唯一 session，清理来源 generation。

# 5. Proposed Design

## Components

### `ContentProtection`

- 职责：成为本机持久业务负载加解密的唯一深模块；创建 V3 envelope、认证完整保护上下文、从历史 key vault 解析密钥，并隐藏算法、catalog 查找和 AAD 组合。
- 输入：加密时输入明文、固定业务 AAD 和活动写入上下文；解密时输入 V3 密文和固定业务 AAD。
- 输出：不透明 `Ciphertext`/`Plaintext` 或稳定、保留 source 的错误。
- 关系：加密从 `ActiveSpaceSecuritySession` 取得当前写入上下文；解密只从密文读取上下文并调用 `ProfileContentKeyVault`，不得读取当前 SpaceId。Repository、blob store 和搜索渲染 adapter 不自行解析 envelope 或选择 key。

### `ProfileContentKeyVault`

- 职责：耐久、加密、原子地保存多个保护组的 content key catalog，并按全 profile 唯一 key identity 与精确 epoch 解析原始 content key 及所属保护组。
- 输入：已通过 MLS/admission 验证的 `SpaceKeyMaterial`，或密文携带的 `ContentKeyId + GroupEpoch`。
- 输出：catalog 安装摘要，或包含所属 `ProtectionGroupId` 与原始 key 的进程内零化 `ResolvedContentKey`。
- 关系：vault 文件由独立 `ProfileContentVaultKey` 使用 MasterKey AEAD 保护；该 key 存于 `SecureStoragePort`，生命周期属于 profile，只由 Factory Reset 删除。vault 不保存 OpenMLS 私有 group state，不决定哪个 Space 活动，也不向 peer 导出历史 catalog。

### `ActiveSpaceSecuritySession`

- 职责：保存当前 Space 的 OpenMLS/security state、当前保护组和“新写入使用的 content key/epoch”。
- 输入：已验证并已激活的目标 `SpaceKeyMaterial`。
- 输出：当前写入 `ProtectionContextV1`、当前 transport key 能力和活动安全诊断分类。
- 关系：从现有 `InMemorySession` 中移出历史 catalog 解析职责。已取得完整 material 时先把 catalog 持久安装进 vault，再切换 session；从 MasterKey 加密 repository 恢复时，由本模块在互斥区内临时装入目标密钥以读取 material，随后验证归属、安装 vault 并完成 session，任一步失败恢复旧 session 快照。切换不能清空 vault 中的旧保护组。

### `ProfileStorageUpgrade`

- 职责：唯一拥有 V1/V2 到 V3 的检测、排空、staging、全量转换、验证、manifest promotion、重启恢复和旧 generation 清理。
- 输入：旧 active manifest、旧 Space session/material、profile 路径与数据库/blob 能力。
- 输出：`UpToDate`、`Upgraded`、`Pending`，或稳定失败；不向调用方暴露逐表 rewrap 步骤。
- 关系：启动时在任何 Space、搜索、内容或网络运行期之前执行。旧格式解析器和旧 AAD 规则只能存在于此模块内部；CrossSpace transition 不得调用该模块。

### `ProfileDataGeneration`

- 职责：保存跨 Space 保留的本机历史 SQLite、受管 blob 和 `ProfileContentKeyVault`，物理路径只由 profile 与 data generation 派生，不由活动 SpaceId 派生。
- 输入：profile generation 与随机 data generation。
- 输出：经验证的 profile 数据引用。
- 关系：活动 Space manifest 引用它，但不拥有它。普通 CrossSpace 重用同一个 data generation；只有一次性存储升级、明确的数据修复或 Factory Reset 可以替换它。membership ledger、关系、Space admission credential、MLS 与 Space 恢复 journal 不得继续留在该 store。

### `SpaceControlGeneration`

- 职责：原子保存且只保存一个 Space 的成员账本、关系、OPAQUE/admission credential、MLS/security state、Space-scoped outbox/checkpoint 和恢复状态。
- 输入：已经验证的目标 Space material、成员历史与准入提交结果。
- 输出：经生产读取入口完整验证的不可变 control generation 引用。
- 关系：物理路径由 `SpaceId + space_control_generation` 派生；CrossSpace、SameSpace security promotion、Reset 和 membership branch 可以创建目标 control generation，但都复用 profile data generation。profile 级 admission/recovery intent 若必须跨活动 Space 存活，应继续由 profile 加密 repository 保存，不能错误下沉到目标 control generation。
- 约束：调用方只提交完整目标控制面，不逐表拼装。删除该 module 后，成员、凭据、MLS、关系与恢复状态的原子安装知识会重新散落到多个 transition，因而它必须是深模块。

### `SpaceTransitionActivation`

- 职责：在 profile data generation 不变的前提下，原子提升完整目标 `SpaceControlGeneration`，重绑活动 membership/credential/MLS/session repository，并恢复目标运行入口。
- 输入：已验证目标安全状态、已经安装到 vault 的目标 catalog、当前 profile data generation。
- 输出：活动目标 Space 或可恢复错误。
- 关系：继续是 manifest promotion 的唯一负责人；不接收 source/target payload cipher，不扫描数据库，不复制 blob，不拥有 payload rewrap。

## Data Model

### `ProtectionContextV1`

```rust
struct ProtectionContextV1 {
    protection_group_id: ProtectionGroupId,
    content_key_id: ContentKeyId,
    group_epoch: GroupEpoch,
    purpose: ContentKeyPurpose,
}
```

- `protection_group_id`：真正的密码学保护域，来自 `SpaceKeyState::protection_group_id()`；不得另造与其重复的 `protection_space_id`。
- `content_key_id`：在对应保护组 catalog 内唯一选择 content key。
- `group_epoch`：必须与 catalog entry 精确相等，不能按“最近”或当前 epoch 回退。
- `purpose`：固定枚举，参与派生与 AAD，防止 content/search/transport 等用途互换。
- 生命周期：创建密文时固定；切换活动 Space、成员变更或后续 epoch 轮换都不改写既有上下文。
- 这是 vault 解析后的进程内模型，不整体明文序列化。V3 密文头只保存随机 `ContentKeyId` 和 epoch；vault 以全局唯一 `ContentKeyId` 找到保护组，所属 adapter 提供编译期固定 purpose，共同重建完整 context。
- `legacy-v1` 或任何在多个保护组重复的 key id 不得用于 V3 新写入。升级器必须使用来源 catalog 中唯一的非 legacy current key；若不存在，则在来源保护组内生成并耐久安装一个新 key 后再转换。

### `PersistedCiphertextV3`

```rust
struct PersistedCiphertextV3 {
    version: u16,                 // 固定为 3
    aead: AeadAlgorithm,
    content_key_id: ContentKeyId, // 随机、不透明的 vault 查找引用
    group_epoch: GroupEpoch,
    nonce: [u8; 24],
    ciphertext: Vec<u8>,
}
```

V3 open 先用 `content_key_id` 在加密 vault 中唯一解析保护组与 key，再结合所属 adapter 的固定 purpose 重建完整 `ProtectionContextV1`。AAD 必须由固定 domain tag、完整 context、持久头中的 key id/epoch、业务实体 AAD 和明确长度编码组成。`ProtectionGroupId` 与 purpose 参与认证但不在密文头明文保存；`SpaceId` 若作为业务实体字段出现，只能位于已加密 payload 或由上层已批准的业务 AAD 中，密码模块不得用当前活动 SpaceId 覆盖 vault 解析出的保护组。V3 不保存明文文件名、路径、设备名或其他业务负载。

inline 密文与 UCBL blob 共享紧凑二进制 `PersistedCiphertextV3` envelope；UCBL 只在该 envelope 外增加 magic/version 并负责 zstd 压缩。各 adapter 仍使用自己的业务 AAD，但完整 `ProtectionContextV1`、purpose 派生和规范 AAD 组合只能由 `ContentProtection` 构造。未知 version、algorithm、purpose、保护组、key id 或 epoch 一律失败关闭。

### `ProfileContentKeyVaultV1`

```rust
struct ProfileContentKeyVaultV1 {
    format_version: u16,
    revision: u64,
    groups: Vec<ProtectedGroupCatalogV1>,
}

struct ProtectedGroupCatalogV1 {
    protection_group_id: ProtectionGroupId,
    space_id: SpaceId,
    entries: Vec<ContentKeyEntryV1>,
    catalog_digest: [u8; 32],
}
```

- 外层文件整体使用独立 `ProfileContentVaultKey` AEAD 加密，并绑定 profile generation、vault revision 和固定 purpose。
- `space_id` 只用于验证安装材料的归属和本机管理，不作为正常解密时的活动状态。
- `ProtectionGroupId`、`SpaceId`、key bytes 和 catalog 映射全部位于外层 AEAD 密文内；vault 文件的明文 framing 只能包含格式、nonce、长度和算法所需的非业务信息。
- `ContentKeyId` 在整个 profile vault 内必须唯一映射到一个 protection group；同一 key id 出现在不同组，或同一组 + key id 的 epoch/key bytes 不一致，均为损坏/冲突，不允许覆盖。
- catalog 合并必须规范排序、去重、幂等并有大小上限；不得记录原始业务标识或 payload。
- 033 不自动删除 group；未知引用关系时宁可保留加密 catalog，不能使历史静默不可读。

### `ActiveRuntimeLayout` 与 V3 持久 manifest

Core 只表达活动运行期的领域所有权：

```rust
struct ActiveRuntimeLayout {
    space_id: SpaceId,
    profile_data_generation: [u8; 16],
    space_control_generation: [u8; 16],
}
```

- `space_id` 不得为空，两个 generation 不得使用全零保留值，也不得互相别名。
- `profile_data_generation` 可在 CrossSpace 前后保持不变；`space_control_generation` 必须随目标控制面替换。
- Core 不拥有格式版本、serde、digest、keyslot 或密码实现；这些技术格式只属于 Infra。

Infra 的 V3 持久格式为：

```rust
struct PersistedActiveRuntimeManifestV3 {
    format_version: u16,
    space_id: String,
    keyslot_generation: [u8; 16],
    profile_data_generation: [u8; 16],
    space_control_generation: [u8; 16],
    manifest_digest: [u8; 32],
}
```

- `profile_data_generation` 取代“数据库 generation 属于当前 Space”的语义；SQLite、blob 与 content key vault 位于 profile data generation 下。
- `space_control_generation` 绑定当前 Space 的成员 SQLite、关系、凭据、MLS、安全与恢复状态；这些表不得继续混在 profile history SQLite 中。
- CrossSpace 创建新的 `space_control_generation`，但必须原样复用 `profile_data_generation`。
- manifest digest 绑定全部字段。第一切片只增加 V3 codec、领域映射和只读版本门禁，生产 `promote` 仍只写 V2；V2 到 V3 的真实提升只能由后续 `ProfileStorageUpgrade` 完成。旧 binary 识别到 V3 必须在任何写入前返回不支持版本。

### `ProfileStorageUpgradeJournalV1`

```rust
enum ProfileStorageUpgradePhaseV1 {
    Detected,
    TargetStaged,
    PayloadsConverted,
    Verified,
    Promoted,
    CleanupPending,
}
```

Journal 至少绑定 source manifest digest、source/target generation、当前保护组、目标 vault digest、转换计数和验证摘要，并使用 profile 稳定密钥 AEAD 保存。阶段只前进；每个阶段重复执行必须产生同一目标或验证已有目标，不能重新生成不一致的 generation/key。

### 搜索 key

搜索词项改用从 `ProfileContentVaultKey` 域分离派生的 profile 稳定 SearchKey，并把 `ProtectionGroupId` 纳入 HMAC 输入。一次查询对 vault 中实际有可搜索文档的保护组生成 token；切换 Space 不重建旧索引，新 Space 首次写入只增加该组 token。渲染字段仍使用对应记录的 V3 protection context 加密。

## API / Interface

密码学细节保留在 Infra。目标内部 interface 表达完整能力，而不是把 parse、resolve、derive、seal 步骤交给调用方：

```rust
impl ContentProtection {
    // 构造时固定 purpose；不由业务调用方逐次传入。
    async fn seal_for_active(
        &self,
        plaintext: &Plaintext,
        aad: &Aad,
    ) -> Result<Ciphertext, ContentProtectionError>;

    async fn open(
        &self,
        ciphertext: &Ciphertext,
        aad: &Aad,
    ) -> Result<Plaintext, ContentProtectionError>;
}
```

- `seal_for_active` 在模块内部取得当前写入上下文；没有已激活 Space 时返回稳定 `NotActive`。
- `open` 只要求 profile content vault 已解锁；它不接收 `ActiveSpace`，也不读取当前 SpaceId。
- 调用方继续提供实体级 AAD，但不能提供 protection group、key id、epoch、purpose、nonce 或成功布尔值；purpose 由具体持久化 adapter 构造时固定。
- `BlobCipherPort`、持久 blob store 和专用 repository adapter 必须统一委托该 module；若保留 Core port，需 clean cutover 到上述语义并删除旧 `decrypt(space, ...)` interface。
- `TransferCipherPort` 继续显式依赖活动 Space/MLS 上下文，不复用历史内容 `open` 作为网络授权。

```rust
impl ProfileContentKeyVault {
    fn install_verified_space_material(
        &self,
        material: &SpaceKeyMaterial,
    ) -> Result<InstalledCatalog, ContentKeyVaultError>;

    fn resolve(
        &self,
        content_key_id: &ContentKeyId,
        epoch: GroupEpoch,
    ) -> Result<ResolvedContentKey, ContentKeyVaultError>;
}
```

- `install_verified_space_material` 是完整原子动作；调用方不能逐 entry 写入。
- `resolve` 精确验证 key id 与 epoch 并返回所属保护组和零化原始 key；purpose 派生及完整 `ProtectionContextV1` 认证只由 `ContentProtection` 负责，不能散落到 vault 或调用方。
- vault 的持久化 adapter 不进入 Core，不增加公开 `uc-engine` operation。

```rust
impl ProfileStorageUpgrade {
    async fn ensure_v3(&self) -> Result<StorageUpgradeOutcome, StorageUpgradeError>;
}
```

- Engine 启动只调用一次 `ensure_v3`；模块内部完成检测、恢复和最终结果。
- 返回 `Pending` 时普通 Space/内容/搜索/网络运行期不得启动。
- 下层存储、密码、数据库和文件失败必须通过 `#[source] source: anyhow::Error` 或具体 source 保留完整错误链；只允许增加固定、脱敏动作上下文。

## Workflow

### 软件升级：V1/V2 到 V3

1. Engine 解锁 profile 级安全存储，但不启动 Space、搜索、内容、文件或网络运行期。
2. `ProfileStorageUpgrade::ensure_v3` 取得跨进程 profile upgrade lock，读取 V2 active manifest 或已有 upgrade journal。
3. 从 V2 manifest 加载来源 Space 安全材料，验证 `ProtectionGroupId`、完整 content key catalog、数据库和 blob root；任何缺失在写 target 前失败。
4. 生成并持久化唯一 target profile data generation 和独立 `ProfileContentVaultKey`；把来源 catalog 原子安装到新 vault。
5. 从一致性 SQLite 快照和 blob root 创建 target staging；把本机历史/搜索/文件数据迁入 profile data store，把 membership ledger、关系、Space credential、MLS/security 和 Space-scoped 恢复状态迁入当前 Space control store。逐项用旧 reader 解密，在内存中以同一实际保护组的 V3 context 和新 nonce 重新加密；允许明文的文件内容本体按现有规则复制。
6. 文件路径、transfer/receive 持久负载、active register 和搜索渲染字段按其记录保护上下文转换。只属于已停止运行期且没有历史价值的临时操作状态必须按既有终态规则取消/关闭，不能伪装成成功迁移。
7. 使用 profile SearchKey 重建搜索 token；不把明文词项、标题、文件名或路径写入中间文件。
8. 用生产 V3 读取入口分别重开 profile data store 和 Space control store，核对表归属、行数、blob 引用、catalog 引用、成员/凭据/MLS 一致性、搜索文档、摘要和代表性解密。验证不能调用旧 reader，且两类 store 不得残留对方所有的表。
9. 写入同时引用 target profile data generation 和 target Space control generation 的 `ActiveSpaceGenerationManifestV3`，原子替换 active manifest。此替换是唯一生效点。
10. 从 V3 manifest 重开 profile 数据和当前 Space security session；成功后进入 `CleanupPending`。旧 generation 清理失败只重试清理，不回滚 V3。
11. 删除旧 reader 的运行期接线。后续启动看到 V3 直接返回 `UpToDate`，不得再次扫描 payload。

### 升级后的 CrossSpace

1. Application 完成现有 admission Commit/Complete，并取得唯一切换租约；暂停新本地操作、旧 Space 网络发送和后台写入，排空已经开始的写事务。
2. Infra 验证目标 MLS/security generation 和 `SpaceKeyMaterial`，原子把目标 catalog 安装进 `ProfileContentKeyVault`。此时来源仍活动；额外的未引用 catalog 不扩大网络权限。
3. 结束、取消或隔离属于来源 Space 的在途 transfer、directory publish 和 receive attempt；不得把它们改挂到目标 Space。
4. 构造 `ActiveSpaceGenerationManifestV3`：目标 `space_id`、目标 keyslot/control generation、原样复用当前 `profile_data_generation`。
5. `SpaceTransitionActivation` 原子提升 manifest，重绑目标 membership/credential/MLS/security control store 与 session，并把目标 protection group 设置为新写入上下文。profile history 数据库连接和 blob root 保持同一 data generation，不执行 replace/copy/rewrap。
6. 恢复目标 Space 的接收、成员和发送运行期；本机历史查询通过各自 V3 context 继续读取。
7. 后台只清理来源 Space 的安全 generation、transition 暂存和在途临时状态；不得删除 vault 中仍可能解密历史的来源 catalog。

### 读取混合保护组历史

1. Repository 读取不透明 V3 ciphertext 和业务 AAD。
2. `ContentProtection::open` 解析并验证 context。
3. vault 以 `ProtectionGroupId + ContentKeyId + GroupEpoch` 精确解析 key，并按 `purpose` 派生。
4. AEAD 成功后只把零化 `Plaintext` 返回给业务读取路径。
5. 当前活动 Space 与密文保护组不同不构成错误；缺 catalog、epoch 不符或 tag 失败分别稳定分类并失败关闭。

### 旧历史再次分享

1. 本机可以读取并展示 vault 可解密的历史，不因此产生任何目标 Space outbox。
2. 用户明确执行复制、恢复后广播或再次分享时，Application 创建一个新的目标 Space 事件。
3. 旧历史在内存中打开，新事件通过当前 MLS/transport 上下文发送，并以当前活动保护组写入新的 V3 持久记录。
4. 原记录和原保护上下文保持不变；不得原地改写或只修改归属字段。

# 6. Implementation Plan

Step 1（已完成）:
File: `crates/uc-core/src/membership/active_runtime_layout.rs`、`crates/uc-infra/src/security/active_space_generation_manifest_store.rs` 及版本门禁错误映射
Change: Core 定义不携带持久格式的 `ActiveRuntimeLayout`，固定 profile 数据世代与 Space 控制世代的独立所有权和合法组合；Infra 定义 V3 持久 manifest、规范 digest、领域映射和只读版本识别。生产 store 继续只提升 V2，识别到合法 V3 时失败关闭为 `UnsupportedVersion`，不提前激活半完成运行期。
Risk: 此切片只建立模型与格式边界，不宣称 V3 可运行；任何 V3 写入和 promotion 必须等 vault、控制面 store、升级器与 production reader 一起完成。

Step 2（已完成）:
File: `crates/uc-infra/src/security/profile_content_key_vault/`、`crates/uc-infra/src/space/security/content_key_catalog.rs`、`crates/uc-infra/src/security/profile_reset.rs`
Change: 新增使用独立 secure-storage key 的 profile 加密 vault 深模块。外部 interface 只暴露完整 material 安装和 `ContentKeyId + exact GroupEpoch` 解析；内部按规范 catalog 与加密 persistence 两类知识组织，统一复用 Space security 的 V2 catalog codec，完成多保护组合并、全 profile key identity 冲突拒绝、未知 framing/缺钥/篡改失败关闭、耐久原子替换和 Factory Reset 擦除。解析结果同时返回所属 `ProtectionGroupId`，后续 `ContentProtection` 必须据此认证 V3 context。本切片不接入 production session、不提升 V3 manifest，也不改变 CrossSpace。
Risk: vault 丢失会使历史永久不可读；存在历史时绝不能自动再生缺失 key。

Step 3（已完成）:
File: `crates/uc-infra/src/security/content_protection/`、`crates/uc-infra/src/space/security/session.rs`
Change: 新增未接 production repository 的 V3 `ContentProtection` 深模块。构造时固定 at-rest purpose；`seal_for_active` 只从 session 取得当前保护组、非 legacy key id、精确 epoch 与原始 key，`open` 只从 V3 header 和 profile vault 重建历史上下文，不读取当前 Space。模块内部独占 purpose HKDF、规范长度编码 AAD、严格 V3 envelope 和稳定 source-preserving 错误；明文头不保存保护组、Space 或 purpose。session 本切片只增加当前写入保护组 seam，仍保留 V2 reader 所需历史 catalog，生产写入继续为 V2。
Risk: 在 V2 reader 尚未 clean cutover 前删除 session 历史 catalog 会破坏现有读取；必须先让所有 at-rest adapter 统一委托 `ContentProtection`，再完成职责拆除。

Step 4（进行中；激活与当前 material 推进子切片已完成）:
File: `crates/uc-infra/src/space/security/session.rs`、`crates/uc-infra/src/space/security/access.rs`、Engine assembly
Change: 第一子切片新增 `ActiveSpaceSecuritySession` 深模块，并由 Engine 为正常 Space security runtime 组装唯一 profile vault。已取得目标 material 的 Fresh group join 先验证归属并耐久安装完整 catalog，成功后才切换 `InMemorySession`。普通启动恢复面对由同一 MasterKey 加密的 repository，由该模块在互斥事务中临时装入目标 Space 密钥、读取并验证 material、安装 vault 后完成 session；repository、vault 或 session 任一步失败都恢复旧 snapshot。Legacy 无 material 激活不伪造 catalog，已成功追加但尚未被 session 引用的 catalog 保留并供幂等重试；`SpaceAccessError::SecurityState` 保留完整 source chain。

Change: 第二子切片把当前 Space 的 material 推进收口为 `install_current_material`；成员加入、epoch/revocation、legacy bootstrap、Sponsor/Helper 准入和 membership branch recovery 在已持久真实安全状态后，统一先幂等安装 vault catalog，再推进活动 session。安装失败通过各业务边界的稳定 `SecurityState` 分类保留 source，原恢复流程重试已提交 material；不回滚或删除已安全写入的 catalog。仅校验候选 material 的临时 `InMemorySession` 不得写 vault；旧 `admission_space_transition` 的 CrossSpace target session 留给 Step 8 整体删除，不在此处制造过渡接线。Step 5 的 V3 reader clean cutover 后仍需从 `InMemorySession` 删除持久历史解析职责。本步仍不接 production V3 payload、不删除 V2 历史 catalog，也不改变 V2 manifest 或旧 CrossSpace transition。
Risk: transport 与 at-rest purpose 混用会扩大旧组网络权限；使用不同具体模块，不创建万能 session trait。

Step 5（进行中；持久密码 interface 收窄与 V3 inline/UCBL 目标格式子切片已完成）:
File: `crates/uc-core/src/ports/security/blob_cipher.rs`、相关 Application port/错误模块、`crates/uc-infra/src/security/blob_cipher_adapter.rs`、`key_epoch_aad.rs`、`encrypted_blob_store.rs`、各加密 repository adapter
Change: 定义共享 V3 protection context/AAD，更新 inline 和 UCBL 格式；将持久内容解密 interface 一次性改为不接收 `ActiveSpace`，所有持久化新写只产生 V3，open 自描述选择 key。把派生字段的 context、常量、读写和验证收口到所属 repository/module，并以 contract 测试证明调用方不能选择 protection context。

Change: 第一子切片先从 `BlobCipherPort::encrypt/decrypt` 同时删除 `ActiveSpace` 参数，清除四类 inline decorator、旧 migration recovery 和待删 CrossSpace 中伪造的占位 Space。调用方现在只提交 payload 与业务实体 AAD；当前 V1/V2 兼容 adapter 仍从共享 session 内部选取上下文，wire format 和生产行为不变。此切片不启用 V3 writer、不增加 production 双 reader，也不修改 UCBL、搜索或专用 repository codec；profile upgrade gate 就绪后，inline production clean cutover 只替换 adapter 内部实现，不再改调用方 interface。

Change: 第二子切片把原 JSON `Vec<u8>` V3 envelope 收紧为共享的紧凑二进制格式，避免大 UCBL payload 被 JSON 数字数组放大；新增只读写 V3 的 inline `BlobCipherPort` adapter 与 UCBL store。两者均把 key resolution、purpose HKDF、完整 context AAD 与 AEAD framing 委托给同一个 `ContentProtection`，UCBL 只拥有压缩和外层 framing。跨 Space tracer 证明切换活动 session 后 inline 与 blob 历史仍从 vault 打开，密文不出现 payload、Space、保护组或 purpose，错误转换保留稳定分类与 source。本子切片仍不接 production，也不增加 V1/V2 正常 reader；搜索留给 Step 6。active register、file-set path、transfer/receive 与 directory publish 等专用字段不复制第二套密码 header，其业务序列化和 AAD 继续由所属模块拥有，并在 Step 7 全量转换与 clean cutover 时统一委托 `ContentProtection`。

Change: 第三子切片在 active register、file-set path、file transfer metadata/event、directory publish root map 与 receive artifact 所属模块内分别增加 V3 codec。各 owner 继续独占私有 payload schema、路径编码和实体 AAD，V3 codec 只把序列化后的内存字节交给共享 `ContentProtection`，没有在升级器内复制业务格式或另造密码 header。跨字段 contract 覆盖 round trip 与错列、错 transfer、错 attempt 的认证失败。当前 codec 仅供下一升级子切片编排，production repository 仍使用 V1/V2，不能提前形成双 reader。
Risk: 任一字段遗漏都会在切换后不可读；建立持久负载清单测试，不用一个字符串驱动的万能 rewrapper 隐藏差异。

Step 6（进行中；多保护组搜索目标模块子切片已完成）:
File: 搜索 key derivation、search repository/runtime 与索引版本
Change: 引入 profile 稳定且按 protection group 域分离的 SearchKey，提升索引版本；查询对实际存在文档的保护组生成 token，渲染字段委托 V3 `ContentProtection`。

Change: 第一子切片新增未接 production 的 `V3SearchProtection` 深模块。它从 `ProfileContentVaultKey` 域分离派生 profile 稳定搜索根；索引调用只提交规范词项，活动保护组由 session 固定，输出 opaque group ref 与组隔离 term tags。查询调用只提交索引 `DISTINCT` 得到的 group refs，模块在 vault 已安装组中验证并按“每个查询词一组跨保护组 alternatives”返回 tags，AND 语义不得把所有 tags 扁平后按 `词数 × 组数` 计数。render JSON 与实体 AAD 仍由搜索模块拥有，AEAD、purpose 和历史 key resolution 委托 Search purpose 的 V3 `ContentProtection`。测试证明同词跨组不等、重启/Space 切换稳定、只派生实际索引组、未知 ref 失败关闭且旧组 render 可读。本子切片不修改 v11 production schema/port/装配，也不触发普通重建；group-ref 列、索引版本提升和生产切换必须与 Step 7 target conversion 及最终 clean cutover 同时生效。
Risk: 多历史保护组增加查询 token 数；对 group 数和 token 批次设明确上限，并以性能测试固定预算。

Step 7（进行中；升级协调基础子切片已完成）:
File: 新增 `crates/uc-infra/src/security/profile_storage_upgrade/`、profile lifecycle/startup assembly
Change: 实现 journal、独占锁、source snapshot、数据面/控制面表拆分、全量转换、两类 V3 production-reader 验证、manifest promotion 与清理；将 V1/V2 reader 和旧 rewrap 常量移动为 upgrade-private 实现。升级是一个深模块，Engine 只调用 `ensure_v3`。

Change: 第一子切片新增 `ProfileStorageUpgrade::ensure_v3()` 唯一外部 seam，并在模块内部实现 profile 级进程内串行化、标准库跨进程非阻塞文件租约、独立 purpose 的 profile AEAD journal 和 source identity 恢复校验。Journal 使用规范 phase 集合，显式绑定 V2 manifest digest 与 keyslot/database/security generation，并一次生成非零、互异且不复用 source 的目标 profile data/control generation；重复启动必须复用相同密文 journal。V2 与零行/空 profile 统一返回 `Pending`，锁竞争返回 `Busy`，source 变化、journal 篡改、缺钥和持久化失败均失败关闭并保留下层 source。本子切片不接 Engine 启动，不创建 source snapshot 或 staging，不转换 payload，也不提升 V3 manifest；这些动作继续由同一深模块的后续子切片完成。
Change: 第二子切片让同一 `ensure_v3()` 在下一次单调推进时执行 SQLite `VACUUM INTO` 一致性快照，并按 journal 中唯一 data/control generation 创建不含 Space 明文的 `profile.sqlite` 与 `control.sqlite` target。Journal 在两个文件均原子落盘且 digest 相等后才进入 `TargetStaged`，同时绑定 source database revision；恢复会验证 target digest 与 source revision，任何升级期间写入、target 缺失或篡改均失败关闭，V2 source 与 active manifest 不变。此阶段的两个 target 仍是同一 snapshot，尚未宣称完成表归属拆分或 V3 密文转换；下一子切片必须在这些 target 内按领域所有者分离并转换，不能把“复制两份整库”当作最终架构。
Change: 第三子切片新增 `StoresSeparated` 耐久边界与穷尽 table ownership registry。每张 production SQLite 表必须唯一归入 profile data、Space control、profile coordination 或两库技术 schema；未声明的新表、重复声明或 schema 缺失都会在修改 target 前以 `Corrupt` 失败关闭。升级器只从 profile target 清除 control rows，只从 control target 清除 profile data/coordination rows，随后执行 `VACUUM` 物理清除并把两个独立 digest 写入加密 journal；source revision 在操作前后都必须不变。两库暂时保留完整技术 schema 以支持后续 repository 路由，但业务 row 已唯一归属；`clipboard_entry_delivery` 作为历史投递事实保留在 profile data，未来发送资格仍只来自活动 control/session。本子切片仍未转换 V3 payload、blob 或搜索 token，也不接 production。
Change: 第四子切片新增 `PrimaryPayloadsConverted` 耐久边界，只转换已经有正式 V3 adapter 的 inline 与 UCBL。`StoresSeparated` 数据库保持只读；升级器在同 generation 的唯一临时目录中复制 profile database，逐项用 V1/V2 reader 打开、在内存中交给 `ContentProtection` 以当前实际保护组和新 nonce 写为 V3，并把 blob 写入独立 tree。数据库与 blob 全部通过正式 `V3InlinePayloadCipher` / `V3EncryptedBlobStore` 回读、行 identity 对齐及 digest 后，才以目录 rename 发布 `v3-primary` 并写 journal；journal 保存数据库/blob-tree digest 与两类计数，恢复验证使用不会切换 WAL 或改写介质的直接 SQLite 连接。崩溃在 rename 前只留下未引用临时目录，rename 后 journal 未写则验证并复用完整输出；损坏输出失败关闭。本子切片不复制密码 header，也尚未转换 file-set、transfer/receive、active register、directory publish 与搜索字段，因此不进入最终 `PayloadsConverted`、不接 production。
Change: 第五子切片新增最终 `PayloadsConverted` 耐久边界。升级器从不可变 `v3-primary` 复制新的原子候选目录，只调用各 owner 提供的 legacy open 与 V3 seal，转换 file-set path、transfer metadata/event、active register、directory publish、receive artifact 和 search render；旧 subkey label 也留在 owner helper，升级器不复制私有 schema、路径编码、AAD 或 HKDF 常量。搜索 target 增加 32-byte opaque `protection_group_ref`；旧 HMAC postings 无法反推出规范词项，因而明确删除 postings/tags、把 `search-v12` 标为 blocked，等待下一 production V3 rebuild，而不是伪装完成 token 重建。所有专用字段经正式 V3 reader 回读，搜索 gate、数据库/blob-tree digest 与计数验证后才 rename 发布并推进 journal；重启可验证复用完整输出。当前仍未接 production reader、manifest promotion 或启动 gate。
Change: 第六子切片新增独立 `Verified` 耐久边界。升级器不以转换函数成功代替最终验证，而是按目标 profile/control repository 路由重新打开 `v3-payloads/profile.sqlite` 与 `control.sqlite`，执行 SQLite integrity、foreign-key 和业务 row 唯一归属检查，并以规范 `sqlite_master` 计算两份 schema fingerprint 写入加密 journal。重启会重新执行完整验证并比对 fingerprint，target 缺失、跨库残留、schema 漂移或 source revision 变化均失败关闭；只有 `Verified` 才能作为后续 manifest promotion 前置条件。本子切片仍不接 production 启动或提升 manifest。
Change: 第七子切片在 active manifest store 内增加 V2→V3 的完整 compare-and-promote 操作与显式 V3 loader。调用方提交已验证 source 和一个封装 `ActiveRuntimeLayout`/keyslot 的目标；store 在同一写锁内重读并认证当前 manifest，只有当前仍精确等于 source 才原子替换，重启已看到同一 target 返回 `AlreadyActive`，其他合法 manifest 返回 `SourceChanged`。升级不得借此改变 Space 或 keyslot identity，V3 ciphertext 继续隐藏标识；既有 V2 loader 面对 V3 仍失败关闭。当前只准备 promotion capability，`ensure_v3` 与 Engine 尚未调用，避免 production repository 路由就绪前提前提升。
Change: 第八子切片把 active register、directory publish 与 receive artifact 三个不要求“解密后继续同一 SQLite 事务”的 production repository 改为 owner-private protection strategy。既有构造器固定 legacy strategy，V3 构造器只接收共享 `ContentProtection`；调用点不能选择字段 AAD、格式或 reader，repository 也不做逐 envelope 双读。V3 open 在数据库读取完成后异步解析 profile vault，随后再返回领域记录；组合 contract 通过真实 SQLite 写读、密文探针与 owner AAD 回读证明三条路径。file-set、transfer/event 和 inbound atomic commit 仍含事务内同步密码操作，留给下一子切片整体重构，不能复制 SQL 建立平行 repository。
Change: 第九子切片将 entry file-set 的路径保护提取为 owner-private `EntryFileSetProtection` 完整策略，并由普通 repository 与 inbound atomic commit 复用。SQL replace/load、领域行编码和枚举校验仍只有一份；legacy/V3 只分别实现路径列 seal/open，V3 在进入 SQLite 事务前完成异步 vault 解析与密文准备，事务内只原子提交已准备 rows。真实 SQLite contract 覆盖 V3 original/root/relative 路径写读与明文探针，既有 legacy、锁定、FK、批量替换及 inbound 原子提交测试保持通过；没有建立平行 V3 repository 或逐行双 reader。transfer event/projection 的 read-modify-write 事务仍待下一子切片处理。

Change: 第十子切片把 file-transfer projection metadata 收口为 owner-private `TransferPersistenceProtection` 完整策略。普通构造器固定 legacy，V3 构造器只接收共享 `ContentProtection`；SQL 查询、领域投影和 port 实现保持单份。所有 metadata open/seal 均移出 Diesel closure；provisional path、单行失败与 bulk fail 以旧密文为 compare-and-swap 条件，bulk 任一行并发变化则同事务整体回滚。真实 SQLite contract 覆盖 V3 provisional metadata 写读与明文探针，既有 transfer 测试保持通过；event sequence append 与 receiver projection 联合事务由下一子切片负责。

Change: 第十一子切片完成 file-transfer event log 与 receiver projection 的同一 protection strategy。读取先取密文 rows、再在数据库外异步 open；append 先在一致 snapshot 上取得下一 sequence 与 projection 状态，在内存完成 event/metadata seal，最后单一 SQLite 事务重新验证 sequence、status 与旧 metadata 密文并同时提交 event 和 projection。CAS 冲突由 owner 有界重取 snapshot，调用方仍只执行一次 `append(event)`；发送侧无 projection 与终止态 no-op 语义保持不变。V3 联合真实数据库 contract 覆盖 event round trip、projection 更新与明文探针；未增加平行 event store 或逐 envelope 双 reader。

Change: 第十二子切片只建立 production search schema 的 V12 前置边界：`search_document` 新增 nullable 32-byte opaque `protection_group_ref` 与 profile/group 索引，Diesel row 明确 V11 行和升级 blocked 行可为空；`CURRENT_INDEX_VERSION` 仍为 V11，不提前开放 V12 查询或 writer。升级器复用正式 migration 后存在的列，仅为历史 fixture 缺列时补齐，避免把 schema 迁移复制成第二事实来源。真实 migration、V11 search 21/21 与 derived payload conversion contract 通过；下一子切片必须一次完成 V3 posting/render/query strategy 后才能提升版本。

Change: 第十三子切片修正 search pipeline 丢失保护组的 interface 反模式：`SearchKeyDerivationPort` 返回不可拆分的 `SearchKeyContext`，其中 V3 key 与 32-byte opaque `SearchProtectionRef` 同生共灭；`SearchPipeline` 把同一 ref 附在每个内存 posting 上，V11 context 明确为无 ref。V3 term tag 改为 group-specific tagging key 后再对规范词项 HMAC，使 Application 现有纯 CPU pipeline 与 `V3SearchProtection::query_terms` 使用完全同一算法；测试证明 context key 生成的 tag/ref 与 owner 深模块输出逐字节一致。此 ref 尚未由 SQLite V12 writer 接收，版本仍不提升。

Change: 第十四子切片在唯一 `SqliteSearchIndex` 内引入 V11/V12 protection strategy，不建立平行 repository。V12 writer 把 pipeline 生成的 opaque group ref 与 render seal 的活动保护组再次比对后才落库，render 密码操作在 SQLite closure 外完成；查询按每个规范词项生成跨实际索引保护组的 alternatives，保留 AND 语义。重建沿用同一临时 schema 与原子 cutover，真实 SQLite contract 覆盖多组查询、重建、明文探针和切换竞态。Engine 此时尚未选择 V12 构造。

Change: 第十五子切片新增 `ProfilePayloadAdapters` 与 Engine `ProfilePayloadRuntime`，把 inline 与 UCBL 组成不可拆分的 primary adapter family，并以同一 runtime 选择 active register、file-set、transfer/receive、directory records 和 search 的 legacy/V3 strategy。调用方不能单独选择某个 payload adapter 的格式，避免 V3 manifest 下出现混合 writer；网络 `TransferCipherPort` 仍独立使用活动 Space session，不属于历史 at-rest family。production 当前仍显式选择 legacy，下一子切片必须由启动 manifest/gate 构造 V3 runtime，不能在普通 wire 中猜测格式。

Change: 第十六子切片新增显式 `ActiveRuntimeManifest` 版本和及 `ProfileRuntimeLayout` 路径深模块。旧 V2 loader 面对 V3 继续返回 `UnsupportedVersion`，只有 V3-aware 启动入口可取得已认证 V2/V3 选择；profile database、blob root 与 control database 只由两个 opaque generation 派生，路径不编码 SpaceId。升级 target staging 改为复用同一 generation directory、文件名与 payload output 规则，消除升级器和 production 各自计算路径的双事实来源。本子切片尚未让 Engine 打开双 pool，下一子切片负责对象图路由。

Change: 第十七子切片由 Engine `RuntimeStorageSelection` 把一个已认证 manifest 原子解析为 profile database、control database、blob root 与 payload format。V2/无 manifest 继续让两个 executor 共享原 pool；V3 打开独立双 pool，并把 clipboard/history/search/transfer 仓储只接到 profile executor，把 membership、relationship、credential、Space security 与 LAN device 仓储只接到 control executor，同时选择完整 V3 payload runtime。`CurrentSpaceResolver` 可从 V3 manifest 读取当前 Space identity；旧 `DurableAdmissionSpaceTransition` 不得拿双库运行，V3 admission/reset/branch transition 暂由同一 fail-closed adapter 拒绝，等待 Step 8 的 `SpaceControlGeneration` owner 替换。启动升级 gate、promotion journal 和 control credential V3 scope 仍未完成。

Change: 第十八子切片让 `ProfileStorageUpgrade` 从 `Verified` 真正执行 V2 source compare-and-promote，并把目标 profile/control generations 与原 keyslot 组装成唯一 V3 manifest。promotion 成功或幂等发现同一 target 后才把加密 journal 推进到 `Promoted`；若进程在 manifest 替换后、journal 保存前崩溃，重启通过 target generation/keyslot 反向匹配已活动 V3，重新验证双库和 payload 后补写 `Promoted`。下一次调用单调进入 `CleanupPending`，之后返回 `UpToDate`；不同 V3 target、提前 promotion 或后来 source 均失败关闭。无活动 Space 的空 profile 不伪造 manifest，继续等待首次 activation。Engine 启动/解锁 gate 与旧 source 清理仍属后续子切片。

Risk: 磁盘不足、移动端短进程和崩溃会留下 staging；每个 phase 先耐久记录再执行可重复动作，promotion 前绝不改 source。

Step 8:
File: `crates/uc-infra/src/security/admission_space_transition.rs`、Application transition tests、active manifest store
Change: 新增完整 `SpaceControlGeneration` store；将 CrossSpace 改为复用 profile data generation，只安装 target catalog、提升 target control manifest 和重绑 control repositories/session；删除正常 CrossSpace 的 `rewrap_finalized_source`、source backup、payload rewrap 和 profile DB/blob replace 路径。Reset/branch 必须按各自语义单独验证，不能机械套用 CrossSpace。
Risk: 旧 transition checkpoint 可能跨版本存在；启动时先由升级器识别并完成或稳定拒绝，不能用新状态机误读旧 phase。

Step 9:
File: `docs/exec-plans/completed/023-durable-membership-proof-and-admission-activation.md`、`docs/exec-plans/completed/028-single-space-admission-protocol.md`、`docs/exec-plans/active/032-admission-space-transition-internal-refactor.md`、安全文档与架构检查脚本
Change: 完成实现后删除被 033 取代的旧行为正文，按最终代码重新撰写 032 的 transition 深模块边界；增加负向架构检查，禁止 CrossSpace 引用 payload upgrade/rewrap、旧 reader 或 source/target cipher pair。
Risk: 只改代码不移除旧规范会导致后续 Agent 恢复错误实现；文档和检查必须与 clean cutover 同提交完成。

# 7. Edge Cases

```text
Scenario: 空 profile 或没有任何历史 payload。
Expected behavior: 创建空 V3 profile data generation 和空加密 vault；若有活动 Space 则安装其 catalog，否则等待首次激活；不伪造历史组。
Implementation: 同一 upgrade journal 走完整验证和 promotion，零行不是特殊旁路。
```

```text
Scenario: V1/V2 payload 损坏、AAD 不匹配或旧 catalog 缺 key。
Expected behavior: 整体升级失败关闭，source generation 和 V2 manifest 保持不变；不删除、不标记“已迁移”、不把异常密文复制到正常 V3 读取路径。
Implementation: 返回稳定 RecoveryRequired/Corrupt 分类并保留底层 source；修复或明确产品恢复方案不属于自动升级。
```

```text
Scenario: 升级中磁盘空间不足或文件系统写入失败。
Expected behavior: 不提升 manifest；重启复用或安全重建同一 staging，原资料继续完整存在。
Implementation: promotion 前校验容量，所有文件使用临时文件、sync、原子替换和父目录同步；错误上下文不记录路径。
```

```text
Scenario: 在任意 upgrade phase 崩溃或移动端进程被终止。
Expected behavior: 重启从已耐久 phase 单调继续；Promoted 前只读 source，Promoted 后只恢复 target，绝不反向回退。
Implementation: journal 绑定 source/target digest、计数和 generation，重复步骤先验证已有产物。
```

```text
Scenario: 两个宿主进程或扩展同时尝试升级/写入。
Expected behavior: 只有持锁进程执行；其他实例返回 Pending/Busy 且不启动普通运行期。
Implementation: 使用现有 profile 跨进程锁范式，先取得锁再读取 journal 和 manifest。
```

```text
Scenario: CrossSpace 发生时来源仍有写事务或后台发送。
Expected behavior: 切换租约阻止新操作并等待已开始事务结束；超时保持来源活动，不提升目标 manifest。
Implementation: 复用现有暂停/排空门禁，但删除最终 source snapshot 和 payload rewrap。
```

```text
Scenario: target catalog 已安装，但 manifest promotion 前崩溃或准入失败。
Expected behavior: 来源继续活动；vault 中多出的未引用 target catalog 不授予网络权限，重试幂等复用；033 不自动清理该 catalog。
Implementation: catalog install 按 protection group 和 digest 幂等，活动授权仍只来自 active security manifest/session。
```

```text
Scenario: 同一 protection group 重复安装不同 key bytes、不同 epoch 或不同 Space 绑定。
Expected behavior: 视为损坏或协议冲突并失败关闭，不能 last-write-wins。
Implementation: vault 在一次原子提交前规范比较既有 entry 和 incoming digest。
```

```text
Scenario: 当前活动 Space 与待读历史的 protection group 不同。
Expected behavior: 本机读取成功；任何自动同步、重发、目录发布或 outbox 建立均保持关闭。
Implementation: read path 使用历史 resolver；send path 独立验证当前活动成员资格并只接受显式新事件。
```

```text
Scenario: 旧 Space 已移除本机或设备主动离开。
Expected behavior: 已在本机持有的历史仍可读，后继网络权限关闭；不得声称通过删除 catalog 实现对过去数据的撤销。
Implementation: MLS/session 移除与 vault retention 分离，只有 Factory Reset 或未来有引用证明的显式清理可擦除历史 key。
```

```text
Scenario: 搜索跨越大量历史保护组。
Expected behavior: 结果正确且不跨组复用相同 HMAC token；达到安全上限时分页/分批，不分配无界内存。
Implementation: 只为有索引文档的 group 生成域分离 token，固定批次与可观测计数，不记录查询词或 group 标识。
```

```text
Scenario: 旧 binary 打开 V3 profile。
Expected behavior: 在任何 SQLite、manifest、vault 或 security 写入前返回 UnsupportedVersion，不能创建 V2 文件或清空状态。
Implementation: 最外层 profile format gate 先于 repository 和 runtime 构造。
```

```text
Scenario: 网络不可用。
Expected behavior: 本地升级可以完成；CrossSpace admission 在目标安全材料完整前保持 Pending，但不会提前触碰本机历史。
Implementation: upgrade module 零网络依赖；transition 只在现有 Complete 门禁后安装目标 material。
```

# 8. Testing Strategy

## Unit Test

- 输入：固定 group/key/epoch/purpose 和业务 AAD；操作：V3 seal/open；预期：round trip 成功，修改任一 context 字段、AAD、nonce 或 ciphertext 均失败。
- 输入：当前活动为 Space B、密文 context 属于 Group A；操作：`ContentProtection::open`；预期：使用 Group A catalog 成功，测试证明没有读取当前 SpaceId。
- 输入：相同 catalog、补充历史 entry、冲突 entry 和超限 catalog；操作：vault install；预期：相同输入幂等、合法补充合并、冲突/超限失败且旧 vault 字节不变。
- 输入：缺 group、缺 key、epoch 不符、purpose 不符；操作：vault resolve；预期：稳定分类、`source()` 非空且日志/Debug 不含 key、Space、文件名或路径。
- 输入：V3 inline JSON 与 UCBL blob；操作：跨 adapter 打开；预期：共享 context/AAD 语义一致，未知版本和截断头失败关闭。
- 输入：两个 protection group 的相同词项；操作：派生搜索 token；预期：token 不同，重启和 Space 切换后各自稳定。

## Integration Test

- 输入：包含 inline 正文、标题、搜索渲染、标签、文件名/路径、active register、transfer/receive 状态和多个 blob 的真实 V2 fixture；操作：执行 upgrade；预期：所有应迁移字段只产生 V3，允许明文例外保持规则，production V3 reader 可完整读取，明文探针扫描无命中。
- 输入：在 `Detected` 到 `CleanupPending` 每个持久边界注入崩溃；操作：重复启动 `ensure_v3`；预期：最终只有一个活动 V3 generation，source 在 promotion 前完整、promotion 后不回退，计数与摘要不重复增长。
- 输入：升级后的大历史 profile；操作：Space A 加入 Space B；预期：active Space 和 security generation 切换，`profile_data_generation` 不变，数据库/blob inode 或内容摘要不因 transition 重写，rewrap 调用计数为零。
- 输入：Space A 与 B 各自产生数据后多次 A→B→A 切换；操作：读取、搜索和重启；预期：两组历史均可读可搜，新写入 context 精确属于当时活动组，历史 ciphertext 字节保持不变。
- 输入：旧 Space 历史和目标 Space 可达 peer；操作：仅切换、不执行分享；预期：目标 outbox/peer 不出现旧记录。随后显式再次分享；预期：产生目标组新事件，原记录不变。
- 输入：target catalog 安装后、manifest promotion 前故障；操作：重启/取消/重试；预期：来源仍可运行，相同 catalog 安装幂等，不能仅凭 vault entry 打开目标网络权限。
- 输入：V3 profile；操作：旧格式 gate 模拟旧 binary 启动；预期：首次写前 UnsupportedVersion，磁盘摘要不变。

## Regression Test

- SameSpace admission：不重写历史，当前 catalog history 合并与 epoch 轮换继续可读。
- MLS member removal/branch recovery：后继通信权限仍按目标 epoch 失败关闭，历史 vault 不参与网络授权。
- Fresh/initial activation：建立首个 profile data generation、vault catalog 和 V3 manifest 后才能开放运行入口。
- Reset：明确区分设备关系 Reset 与 Factory Reset；前者不得隐式擦除历史 key，后者必须删除 `ProfileContentVaultKey`、vault、数据库和 blob。
- 错误转换：所有新增/修改转换同时断言稳定 variant 和 `source()` 非空，禁止字符串化下层错误。
- 架构负向检查：CrossSpace 模块不能依赖 `ProfileStorageUpgrade`、旧 reader、source cipher、target cipher 或 `rewrap_finalized_source`；Application/Engine 不能组合逐表升级步骤。
- 交付检查：运行仓库 AGENTS.md 要求的 metadata、workspace check、fmt、architecture script、`git diff --check` 和密文明文探针扫描。

# 9. Acceptance Criteria

* [ ] V3 持久密文通过不透明且全 profile 唯一的 `ContentKeyId` 解析并认证完整 `ProtectionGroupId + ContentKeyId + GroupEpoch + Purpose`，purpose 由所属 adapter 固定，解密不读取当前 `ActiveSpace`，密文头不明文保存 `ProtectionGroupId`、`SpaceId` 或 purpose。
* [x] `ProfileContentKeyVault` 使用独立 profile 稳定密钥 AEAD 保存多个历史保护组 catalog，冲突安装失败关闭，Factory Reset 完整擦除。
* [ ] profile 数据 generation 的路径和生命周期不再由活动 SpaceId 决定；membership/credential/MLS 等 Space-scoped 表已迁入独立 control store，V3 active manifest 可在切换 Space 时复用同一 `profile_data_generation`。
* [ ] V1/V2 正常读取只存在于一次性 upgrade module；升级后所有新写只产生 V3，没有 lazy migration 或双写。
* [ ] 完整 V2 fixture 一次升级后，SQLite、blob、搜索和派生负载均可由 production V3 入口读取，磁盘无受保护明文。
* [ ] 任意升级阶段崩溃都能单调恢复；promotion 前 source 不变，promotion 后不回退，损坏/缺钥/空间不足不删除数据。
* [ ] CrossSpace 不创建 source final snapshot、不复制数据库/blob、不遍历历史、不执行 payload rewrap，且历史规模扩大不会线性增加切换密码操作数。
* [ ] A→B→A 多次切换后，既有 ciphertext 字节不变，各保护组历史仍可读可搜，新写入使用精确的活动保护组。
* [ ] 仅切换 Space 不会把旧历史加入目标 outbox；明确再次分享会创建目标组新事件且不改写原记录。
* [ ] 活动 MLS/transport 权限只来自 active security session，历史 vault entry 不能恢复旧 Space 的网络发送资格。
* [ ] 旧 binary 在写入前拒绝 V3 profile；新 binary 不会重新生成 V2 profile 状态。
* [ ] 新增或修改的错误转换验证稳定分类与非空 `source()`，观测和日志不包含业务负载、密钥、Space/设备标识、文件名或路径。
* [ ] 规格 023、028、032 和架构圣经在实现完成时删除或标明所有被 033 取代的 rewrap 语义，仓库只保留一个当前事实来源。

# 10. Risks and Trade-offs

- 历史密钥保留：切换后继续读取历史必然要求本机保留旧 catalog。这不扩大 peer 权限，也不能撤销设备已经拥有的过去数据；代价是 secure storage/vault 损坏影响所有历史组。通过独立 profile key、AEAD、原子文件和 Factory Reset 擦除降低风险。
- 一次性升级成本：首次升级仍是 O(受保护 payload + blob) 的全量 I/O 和 AEAD，且需要 staging 额外空间。收益是此成本只支付一次，后续 Space 切换与历史规模无关。
- 混合保护组复杂度：同一数据库可包含多个 protection group，读取和搜索必须自描述选钥。该复杂度集中在 `ContentProtection`、vault 和搜索 module，不能泄漏给每个 repository 或调用方。
- 搜索性能：按 group 域分离 token 避免跨 Space 等值关联，但查询成本随实际含搜索文档的历史组数增长。使用有界批次和索引元数据控制，不能退回跨组共用 token。
- Manifest 与存储拆分：把 profile 数据与 Space control generation 分开会触及主 SQLite 表归属、启动、Reset、凭据 scope 和恢复代码。替代方案是在 V2 manifest 中复用同一 database generation，但现有路径与 Space-scoped 表仍绑定 SpaceId，会保留隐含所有权和整库复制，因此拒绝。
- 本机 vault 模型：替代方案是把所有内容一次性改为本机统一 Data Key。它也能消除 CrossSpace rewrap，但会丢失现有 protection group/content key 语义，并把每次接收入库固定成新的保护域转换；本规格选择保留 MLS protection context 和历史 catalog。
- Lazy migration：可以降低升级首启时延，但会长期保留双格式 reader、让普通读取失败并产生混合 generation，违反单一事实来源和可恢复升级要求，因此拒绝。
- 自动清理旧 catalog：没有可靠引用证明时会造成不可恢复历史损失。本规格选择暂不自动回收，接受小规模加密 key 元数据增长。

# 11. Open Questions

无。以下决定已经固定，实施不得再次默认改写：

- 旧历史在本机保留可读，但不会因 Space 切换自动共享给目标组。
- 保护域使用仓库现有 `ProtectionGroupId`，不创建平行的 protection-space 标识。
- 一次性升级是 eager、全量、原子且可恢复的 clean cutover，不采用 lazy migration。
- CrossSpace 复用 profile data generation，只切换目标 Space control generation 和新写入上下文。
- 历史 catalog 在 033 中不自动垃圾回收；需要回收时另行定义引用证明与用户语义。
