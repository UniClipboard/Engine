# Issue #951 调研结论

## Issue 原文结论

- 标题：`backlog: reliable member revocation requires DEK envelope re-key`。
- 状态：打开；标签为 `security`、`architecture`；没有负责人、里程碑、评论、子任务或关联 PR。
- 目标是“前向撤销”，不是远程抹除：被撤销设备可保留已经下载的历史，但不得获得或解密撤销后的新内容。
- 验收要求包括：分层内容密钥、撤销触发换钥、历史不全量重加密、旧密钥无法重新自证加入。
- 在完整方案上线前，当前行为明确只是本地隐藏和停止主动同步。

## 当前实现

- 当前分支 `encry` 基于 `main` 的 `8f9d097`，工作区版本为 `0.20.0-rc.11`。
- `RemoveMember` 已存在，实际会依次删除成员、地址和信任记录，但不是原子操作，中途失败会留下部分状态。
- 网络接收、拉取和发送会查本机成员表，因此本地删除能停止正常同步，但不能让旧密钥失效。
- 当前一把 Space `MasterKey` 同时承担历史内容加密、传输加密、搜索派生和准入自证；会话也只保存这一把密钥。
- 本地 keyslot 已经用口令派生的 KEK 包装 `MasterKey`，但这只是单一共享内容密钥的本地包装，不是按成员和代次分发的密钥体系。
- 配对会发送当前 keyslot；加入方解出同一把 `MasterKey`，并直接用它生成 HMAC 自证。Sponsor 也用当前 `MasterKey` 验证，因此 issue 描述的重新入册风险真实存在。
- `HmacProofAdapter` 会记录密钥前四字节，违反仓库禁止记录密钥的规则，应先独立修复。
- 泄漏同时存在于 `build_proof` 的 debug 日志和 `verify_proof` 失败日志；回归测试需要真实触发两条路径并捕获日志，而不是只扫描源码文本。
- JSON 密文 V1、UCBL 文件和分块传输 V3 都没有内容密钥标识或群组代次，换钥后无法选择历史密钥。
- `SpaceMember` 只有活跃态；撤销直接删除，没有撤销记录、群组代次、成员 KeyPackage、分发确认或恢复状态。
- 当前设备身份接口只暴露设备 ID；成员和信任记录只有指纹，没有可用于离线密钥信封的独立成员加密公钥。
- 现有 switch-space 迁移有良好的崩溃恢复思路，但它会解密并重加密历史，只能作为状态机参考，不能作为本 issue 的数据迁移方案。

## 关键缺口

- 准入权与内容解密权必须分离；否则历史内容密钥仍能变成重新加入的凭据。
- 密文必须携带密钥标识和代次，同时保持旧格式只读兼容。
- 撤销必须是持久、可恢复、激活后只向前推进的流程，不能扩展当前“先删成员、再清其他表”的顺序写入。
- 离线保留成员必须先补齐群组提交再同步；被撤销成员不能进入新提交或新密钥目录。
- 旧客户端不能静默使用旧共享密钥继续写入，否则前向撤销承诺立即失效。
- 并发成员变更可能产生群组分叉，必须实测恢复，不能只假设最终一致。
- 当前 `SpaceMember` 明确把撤销建模为直接删除，领域层没有可保留撤销事实的类型。
- 当前 Diesel 成员仓储分别执行删除，缺少能把下一代群组状态、密钥目录、撤销事实和 outbox 放进同一事务的端口。
- 阶段 1 的数据库设计应把设备身份、群组状态、密钥目录、收件人和消息正文收进 AEAD 密文；明文仅保留随机流程标识、Space 标识、单调代次和状态枚举。
- OpenMLS 默认 RustCrypto 提供器使用内存存储；产品接入不能把其原始快照直接落盘，必须在 MasterKey AEAD 边界后持久化。
- 阶段 1 仓储已用同一个 SQLite 事务发布下一代 Space 状态与撤销激活状态；注入 Space 更新失败时两者会同时回滚。
- 设备身份、群组状态、密钥目录、撤销收件人和提交正文均只存在于 MasterKey 加密负载中；结构列只保留随机标识、Space、代次和状态。
- 阶段 2 直接读取单一 MasterKey 的数据面集中在 `BlobCipherAdapter`、`EncryptedBlobStore` 和 `TransferCipherAdapter`；搜索、文件元数据和其他持久化路径已统一经过 `DeriveSpaceSubkeyPort`。
- 本机 MasterKey 仍需作为旧 V1/UCBL/V3 读取密钥与加密密钥目录的本地保护根；当前代次的新随机内容密钥应单独进入会话目录，并按正文、传输、搜索等用途隔离。
- `InMemorySession::set_master_key` 是所有初始化、解锁和配对导入路径的共同汇合点，适合自动注册只读兼容的 `legacy-v1`，但生成并持久化新当前密钥仍需显式迁移步骤。
- 当前解锁适配器在初始化、口令解锁、静默恢复和配对导入四条路径写入会话；阶段 2 必须让这些路径在返回成功前恢复同一份持久密钥目录，不能每次解锁临时生成新密钥。
- 新密文的 AAD 需要在原业务 AAD 之外绑定格式、Space、代次、密钥标识和用途；旧格式只用 `legacy-v1` 与原 AAD 读取，缺失新密钥时必须直接失败，不能尝试旧主密钥。
- 阶段 2 已把会话扩展为持久密钥目录：`legacy-v1` 只用于旧格式读取，当前代次按正文和传输用途隔离；目录在解锁和静默恢复时从加密仓储重新装载。
- 普通业务密文新写入 V2、文件密文新写入新版 UCBL、分块传输新写入 V4；三类旧格式仍可读，新格式缺密钥、代次错误或 AAD 被篡改时均失败关闭。
- 搜索及既有本机派生字段仍使用与旧主密钥相同的派生结果，避免迁移后历史索引失效；独立派生测试已经证明字节兼容。
- 阶段 2 当前唯一回归失败是安全存储恢复后的强制搜索索引重建返回零条结果；关键词查询本身成功，说明需要继续核对重建时的正文读取或条目跳过路径。
- 上述失败的真实根因不是搜索派生：恢复入口分别传入固定占位 Space 和新生成的 Space，密钥目录把错误身份当成真实 Space 并生成了另一套当前密钥，导致普通历史投影和搜索重建都无法解开重启前的 V2 正文。
- 安全存储解锁和空间会话恢复现已统一读取 `SetupStatus.space_id`；端到端回归同时证明普通历史浏览和关键词搜索在重启后恢复。
- 旧安装 `SetupStatus.space_id == None` 的口令解锁路径仍会临时生成 Space；必须改为稳定兼容身份，否则首次迁移后的 V2 数据在下一次重启不可读。
- 当前配对顺序是 `Request -> KeyslotOffer -> HMAC(MasterKey) -> Confirm`：joiner 在 sponsor 确认前就持久化 sponsor 的共享 keyslot，内容主密钥同时承担历史解密与准入证明，必须整体替换而不是扩展字段。
- 正确边界是：每台设备拥有自己的本机保护根；可移植目录显式包含 `legacy-v1` 和历史内容密钥；目录通过 MLS Welcome 建立的新代次秘密包装后交付。
- 准入证明需要同时绑定口令派生结果和本次邀请码。只绑定旧内容主密钥会允许撤销设备自证；只绑定邀请码会丢失现有的用户口令确认。
- OpenMLS 验证代码已证明存储快照可序列化，签名私钥可随加密群组状态保存并从 MemoryStorage 读取；产品实现可把协议状态继续放在 `SpaceKeyMaterial.group_state` 的 MasterKey AEAD 边界内。
- 可移植目录升级为 v2 后显式携带 `legacy-v1`；本机 `MasterKey` 只保护本机持久状态，旧格式读取从目录取历史密钥。旧 v1 目录仍按本机根恢复，保持未发布迁移中间态的只读兼容。
- sponsor 与 joiner 安装同一份 v2 目录时，当前内容密钥和历史密钥一致，但两端本机保护根可以不同；这为后续 Welcome 分发目录提供了必要边界。
- 新准入 offer 只包含口令派生参数和随机盐，不再携带内容主密钥；证明密钥同时绑定邀请码、配对会话和 Space，旧内容密钥本身不能生成新证明。
- OpenMLS 产品模块已能创建 sponsor 群组、生成并校验设备 KeyPackage、生成 Welcome、导出双方一致的目录包装密钥，并把群组状态序列化后冷恢复继续加入。
- KeyPackage 中的设备身份与请求设备不一致时会拒绝；其他设备持有错误的待加入状态也无法打开 Welcome。
- 产品群组状态快照使用公开存储结构生成，不依赖生产构建中的测试功能；快照后续只能进入现有 MasterKey 加密仓储，不能明文落盘。
- 当前配对 wire 仍是 v3，领域消息固定为 `Request -> KeyslotOffer -> ChallengeResponse -> Confirm`；请求没有安全能力或 KeyPackage，确认也没有 Welcome、加密目录或群组代次。
- 当前 `RevocationRepositoryPort::save_space_material` 能把群组快照和目录放进同一加密负载，但加入流程还没有“保存成功后才激活会话”的专用组合入口；不能沿用旧流程在 Confirm 前安装共享 keyslot。
- 接线应由一个基础设施组合端口收口：joiner 只把 KeyPackage 放上 wire，私有待加入状态留在本机内存；sponsor 在证明通过后推进 MLS、换当前内容密钥、保存群组快照和目录，再生成 Confirm。
- joiner 安装必须先用 Welcome 导出包装密钥并校验 Space/代次/目录，再创建自己的本机保护根和 keyslot；任何校验或持久化失败都不得提前替换现有可用状态。
- 配对 wire 已升级到 v4：Request 明确携带可靠群组代次能力和 KeyPackage，Offer 只携带口令派生参数与挑战，Confirm 携带 Welcome、加密目录和当前代次；v3 及缺少能力的请求明确拒绝。
- Sponsor 只在新准入证明通过且成员/信任记录落地后推进 MLS、生成新内容密钥并保存新目录；Joiner 收到 Confirm 后才安装自己的本机保护根。
- 加入安装的持久化失败会恢复原 keyslot、本机 KEK 和会话；实测已有空间不会被失败的加入覆盖。
- 生产装配已删除 HMAC 校验对当前 Space 内容主密钥的 cache-miss fallback，旧内容主密钥不再是稳定准入凭据。
- 可靠撤销依赖所有保留成员处于同一群组代次：新增第三台设备时，较早加入的成员也必须收到并应用成员新增提交；因此成员新增提交和成员移除提交必须共用加密 outbox 与确认机制，不能只给新设备发送 Welcome。
- OpenMLS 产品模块现已能生成成员新增提交、生成成员移除提交并在保留成员上应用提交；目标成员应用移除提交时会失败关闭。
- 当前撤销迁移只有 `member_revocation_log` 和加密的 `encrypted_stage`；逐设备发送状态可继续封装在 `encrypted_stage` 内，结构列无需新增成员身份或消息字段，因而能保持敏感数据默认密文。
- 当前仓储只支持 Prepared、Staged、Activated；需要补充 Activated -> Distributing、逐收件人确认、全部确认后 Complete，以及对已激活流程的重启恢复。Complete 时可以清除暂存消息，撤销事实仍保留在加密记录中。
- 群组状态和密钥目录的产品实现集中在 `DefaultSpaceAccessAdapter`，并已持有撤销仓储；`MemberRosterFacade` 当前只持成员、地址、信任等仓储，仍执行“先删成员再级联清理”。可靠撤销应由安全适配器通过一个窄端口提供，门面只接收结构化结果，不直接操作 OpenMLS 或密钥。
- Ready 空间必须在安全适配器返回本机已激活后才能删除目标成员；Legacy/Migrating 空间只能执行并返回明确的 LocalOnly 结果，避免把旧的本机隐藏语义误报为可靠撤销。
- `DefaultSpaceAccessAdapter::admit_group_member` 已经调用 OpenMLS 生成成员新增 commit，但现有 `GroupAdmission` 只返回 Welcome、加密目录和代次，commit 被丢弃；必须扩展返回值并交给同一加密 outbox，否则真实三设备流程中的旧成员会落后。
- `InMemorySession::rotate_space_material` 可在保留全部历史内容密钥的同时新增当前内容密钥；可靠撤销可以复用它生成下一代目录，不需要扫描或重写历史正文与 blob。
- 可靠撤销端口不需要向应用层暴露 Space、群组状态或目录：适配器可从当前会话取得 Space，应用层只传目标设备和保留设备列表，返回撤销编号、真实状态和待确认数量。
- 恢复策略可按持久状态幂等推进：Prepared 重新生成暂存内容，Staged 原子激活，Activated 安装已持久的新目录并开始分发，Distributing 继续等待确认，Complete 直接返回。激活后不得重新生成或回退旧代次。
- Ready 但群组状态为空代表旧空间尚未完成群组升级，不能执行安全移除；这类空间必须走明确的 LocalOnly 结果。
- 生产装配通过 `SpaceAccessPorts` 把同一安全适配器拆成窄端口，目前缺少 `GroupRevocationPort` 字段；补上该字段后 `MemberRosterFacade` 才会在真实引擎中走可靠撤销，而不是测试通过但生产仍走 LocalOnly。
- `uc-engine` 当前 `OperationResult::MemberRemoved` 不携带信息，UniFFI `remove_member` 也返回空结果；阶段 5 必须同步升级为结构化的 LocalOnly、Applied(waiting N)、Complete 结果。
- UniFFI 的 WorkerCommand、公开方法和结果映射需要同时从 `Result<()>` 改为结构化结果；移动验收宿主与 `uc-engine` 公共契约测试当前都显式匹配空的 `MemberRemoved`，必须同步更新。
- HarmonyOS 目前没有独立的成员移除包装，依赖引擎通用结果；通用 JSON/值映射应包含 outcome、revocation_id 和 pending_recipients，与 UniFFI 保持一致。
- 当前 iroh 网络层没有通用成员控制通道，只有剪贴板、活动状态等专用协议；撤销更新不能复用业务负载协议，需要新增专用的群组代次更新协议与确认响应。
- 专用控制协议应直接发送仓储中的不透明加密待办，接收端成功应用群组 commit 和加密目录后才返回确认；离线或失败不确认，发送端保留 Distributing 状态并在重连后重试。
- 当前撤销调用会立即尝试一次发送；离线后的可恢复入口应按 `revocation_id` 查询持久状态并重试所有未确认待办，这样应用重启后无需重新执行成员删除也能继续向前。
- 成员新增更新必须在 sponsor 激活新群组状态的同一持久边界内进入加密待办；仅把更新放进 `GroupAdmission` 返回值仍可能在进程中断或网络失败时丢失。可以把通用群组更新待办封装进已加密的 `SpaceKeyMaterial`，不新增明文收件人字段。
- 成员新增更新已进入 `SpaceKeyMaterial` 的加密待办；数据库原始密文扫描确认收件人和提交内容均未明文出现，重启后仍可恢复，确认后会删除。
- Sponsor 会从真实成员仓储读取旧成员，并排除本机与新加入设备；加入完成后立即尝试发送，启动、周期重试和设备重新在线也会补发。
- 接收端以群组代次实现重复消息幂等：同一代次再次到达时直接确认成功，不会重复合并提交。
- 接收新代次和本机换代都必须继承尚未发送的通用待办；撤销激活同样保留其他成员待办，但必须剔除发给被撤销目标的旧待办。
- 加入待办和撤销待办使用同一发送通道但分开保存；恢复入口固定先发送加入待办，再发送撤销待办，避免离线成员收到越级提交。
- 本功能相关的端口测试替身统一使用 `mockall`，并由 helper 集中配置输入、调用次数、发送顺序和确认结果；真实数据库测试继续使用真实仓储。
- 配对双方的传输会话编号是各端本地句柄，不保证相同；准入证明应统一绑定 Sponsor 随 offer 发送的编号，不能把它与 Joiner 的本地拨号句柄比较。
- 真实配对端到端装配必须提供密钥代次仓储；继续使用不带仓储的旧适配器构造器会让 Sponsor 在确认前失败，Joiner 只能等待会话超时。该用例应使用临时真实 SQLite 仓储，不使用模拟仓储。

## 技术选择

| 选择 | 理由 |
|------|------|
| 默认验证 OpenMLS `0.8.1` | 它实现 RFC 9420，具备 KeyPackage、Welcome、成员移除、代次秘密导出和分叉恢复能力 |
| 先做四平台验证再接入 | 当前 crate 未声明 Rust 最低版本，移动目标、存储和体积都必须实测 |
| 建立一个统一的密钥代次模块 | 调用方只需开始、查询、恢复；协议、格式、存储和重试集中维护 |
| 将当前主密钥导入为 `legacy-v1` | 旧历史无需改写，同时新写入可使用新代次内容密钥 |
| 准入密钥与内容密钥分离 | 被撤销设备持有历史内容密钥也不能重新加入 |
| 新代次对旧客户端失败关闭 | 任何旧密钥或 LAN 自动回退都会破坏可靠撤销 |
| 撤销激活后只向前恢复 | 回滚到旧代次会重新授权被撤销设备 |
| 长期只保留一个移除入口 | 迁移期显式返回 `LocalOnly`，升级窗口结束后删除过渡分支 |

## 外部验证

- `gh` 已成功读取 issue 正文和时间线。
- crates.io 当前提供 OpenMLS `0.8.1`，源码可确认存在所需的成员移除、Welcome、秘密导出、提交合并和可选分叉恢复能力。
- OpenMLS 0.8.1 的三成员撤销、离线按代次补提交、并发分叉恢复和冷存储恢复均已由独立测试实跑通过。
- 根工作区已纳入独立验证 crate，桌面、iOS、Android、HarmonyOS Rust 目标均已重新编译通过，并受锁文件与架构检查约束。
- 模拟器与实体设备验证未执行，记为跳过，不计为通过。
- 开启分叉恢复和存储测试后的优化验收程序为 1,535,632 字节；这是含测试框架的基线，不是最终应用增量。

## 参考范围

- Issue：https://github.com/UniClipboard/UniClipboard/issues/951
- 当前移除流程：`crates/uc-application/src/facade/roster/facade.rs`
- 当前准入证明：`crates/uc-application/src/proof.rs`
- 当前密钥会话：`crates/uc-infra/src/security/session.rs`
- 当前 keyslot：`crates/uc-infra/src/security/crypto_model.rs`
- 当前密文格式：`crates/uc-infra/src/security/blob_cipher_adapter.rs`
- 当前 blob 格式：`crates/uc-infra/src/security/encrypted_blob_store.rs`
- 当前传输格式：`crates/uc-infra/src/clipboard/chunked_transfer.rs`
