# 1. Overview

本规格是“资料密钥丢失恢复与旧设备迁移”的执行记录。状态：**Engine 实现与本地验收已完成；Desktop 自动界面验收受现有启动画面遮挡问题阻塞；实体平台和旧版本程序回退验收跳过**。

问题最初在 Desktop 指定的 `faaceb1865ade14987c485c52ac4036a7e4c0610` 上复现。完成后按用户要求将修复重新放到 `origin/main` 的 `76f0356ac652b42bf55bf2c779c0441b24821cfe` 之上，并重跑全部验收。实现保留了修复前复现，新增恢复、旧设备迁移、清理重试、导入导出和恢复出厂覆盖；没有修改 Desktop，也没有推送、创建 PR 或发布。

当前 Engine 在 `ensure_profile_storage_v3` 中依赖安全存储解开活动清单或恢复源安全会话。丢失 KEK 或整个 keyring 时，依赖装配失败，宿主拿不到可执行 `UnlockSpace` 的 Engine。另有两个独立随机密钥分别保护本机准入/控制资料与历史密钥目录；只找回空间 MasterKey 不等于找回全部历史。

已执行的修复前证据：Desktop 原 `passphrase_recovery` 测试为 1 通过、2 失败，71.22 秒；两个移钥测试中的完整密钥重启对照通过。Engine `host_contract::key_loss` 两项对应复现均失败，9.27 秒。这些不是修复后通过。证据保存在本线程 `desktop-repro.log` 与 `repro.log`，交付报告负责记录实际命令和产物来源。

用户确认的方向是保留独立随机密钥，包装后持久存入 userdata 的 vault；本资料实例的系统钥匙串最终只保留一份自动解锁材料。口令是独立于原机器钥匙串的恢复途径。旧设备必须同样迁移，不能只对新安装启用。

# 2. Goals

- 新建和成功迁移的资料：完整 userdata 加正确的当前恢复口令，可在空安全存储环境恢复旧历史、保存新内容、再次启动。
- 钥匙串仅保存本资料实例的一个自动解锁条目；正文、独立密钥、恢复元数据不明文落盘。
- 旧密钥按原字节迁移，不生成替代密钥，不扫描重加密全部历史来完成密钥搬迁。
- 先将新密文副本耐久保存并验证，再删除旧钥匙串独立条目；中断与清理失败可继续。
- 缺钥时提供可访问的最小恢复服务；资料和后台未就绪时状态必须真实。
- 错误口令不产生破坏性写入，不影响已 ready 会话；错钥、损坏、不兼容和依赖错误保持准确分类。
- 修改口令、切换空间、工厂重置、导入导出和升级备份共同遵循新的密钥事实来源。
- 新恢复承诺不依赖原绝对路径、原系统账户或原钥匙串；跨环境恢复不默认为可同时运行两个相同设备身份。

# 3. Non-Goals

- 不修改 Desktop/Mobile 产品 UI；提供 Engine 公开契约与接入说明。绑定映射仅随 Engine 契约同步。
- 不提交、推送、创建 PR、发布或自动更新下游依赖。
- 不取消内容保护组、MLS 和本机历史之间的隔离，不把旧历史自动共享给新空间。
- 不恢复迁移前已经丢失、没有副本的随机密钥，不把重新配对等同于历史恢复。
- 不提供云端托管、找回忘记的口令、任意文件在线复制一致性或两个克隆身份并行使用能力。
- 不修改原有升级备份的明文例外范围；不把密钥导出到该原样副本中。
- 不承诺旧程序能直接读取新格式，不承诺更改口令能使攻击者已持有的旧备份或密钥失效。

# 4. Current Architecture Context

以下是当前已核对的代码事实；本节不能当作新方案已实现的说明。

```text
Component: Engine 启动与资料装配
Path: crates/uc-engine/src/engine/startup_owner.rs
      crates/uc-engine/src/runtime/mod.rs
      crates/uc-engine/src/assembly/host.rs
      crates/uc-engine/src/assembly/wire/mod.rs
Responsibility: 启动所有权、宿主能力适配、资料准备、正常后台装配。
Relationship: 当前正常 runtime 构造失败便无法执行公开操作；现有启动观察通道仍可查询失败。

Component: 资料启动准备
Path: crates/uc-application/src/profile/startup/use_case.rs
      crates/uc-infra/src/security/profile_startup_storage.rs
      crates/uc-infra/src/security/profile_lifecycle.rs
Responsibility: 先备份，再处理旧布局、待导入资料和生命周期。
Relationship: lifecycle marker 保存 profile generation；缺失时创建新 generation 可能使已有密文认证上下文失配。

Component: 空间解锁
Path: crates/uc-infra/src/space/security/access.rs
      crates/uc-infra/src/space/security/key_material.rs
      crates/uc-infra/src/fs/key_slot_store.rs
Responsibility: 从 keyslot 派生/读取 KEK，解开空间 MasterKey，激活安全会话。
Relationship: 原 unlock 会相信 kek_observed 并跳过写回；未完成工作区补丁正尝试修正，但尚未通过验证。

Component: 独立本机密钥
Path: crates/uc-infra/src/security/admission_key_manager.rs
      crates/uc-infra/src/security/profile_content_key_vault/
Responsibility: 前者保护活动清单、加入与恢复资料；后者保护跨空间历史密钥目录与搜索根。
Relationship: 两把随机密钥目前依赖 SecureStoragePort；历史目录还绑定原 profile generation。
              AdmissionKeyManager 缺钥时自动生成的行为不能用于已有密文恢复。

Component: 资料存储升级
Path: crates/uc-infra/src/security/profile_storage_upgrade/
Responsibility: V1/V2 到 V3 数据布局升级及中断恢复。
Relationship: 解锁前读取清单/源安全会话形成启动阻塞；不能绕过验证强行打开旧数据库。

Component: 改口令与空间切换
Path: crates/uc-application/src/space/lifecycle/change_encryption_passphrase/
      crates/uc-infra/src/space/security/encryption_passphrase_change.rs
      crates/uc-infra/src/security/space_transition_activation/mod.rs
      crates/uc-infra/src/security/active_space_generation_manifest_store.rs
Responsibility: 验证改口令资格、保存耐久事务、完成 registration/keyslot 安装与活动资料提升。
Relationship: 新恢复包装必须加入同一个可恢复提交；不能后台异步补写。

Component: 现有安全材料清单及清除
Path: crates/uc-infra/src/config_migration/secret_keys.rs
      crates/uc-infra/src/security/profile_upgrade_backup/inventory.rs
      crates/uc-infra/src/security/profile_reset.rs
Responsibility: 导入导出、升级备份与清除的密钥枚举。
Relationship: config migration 清单只含当前 KEK，升级备份另含 admission、content vault、lifecycle、identity
              和存在时的迁移密钥；前者不能直接作为本次完整迁移清单。
```

历史理由见[计划 023](../completed/023-durable-membership-proof-and-admission-activation.md)和[计划 033](../completed/033-immutable-content-protection-context.md)：加入空间之前也有资料需要加密；跨空间历史不能依赖当前空间的密钥。保留这些隔离，补齐恢复链，而不是复用一把空间密钥加密所有本机资料。

# 5. Proposed Design

## Components

| 组件（新增名为设计名） | 职责、输入与输出 | 所属与边界 |
| --- | --- | --- |
| `ProfileKeyRecovery` | 接收启动检查、口令恢复和旧设备迁移意图；输出完整恢复结果，拥有迁移、重试及清理顺序 | Application `profile/key_recovery/`；按 model/error/ports/use_case/tests 拆分 |
| `ProfileSecretVault` | 密文读取、认证、原子提交、修订检查；接收不透明受保护材料，返回已验证能力 | Infra `security/profile_secret_vault/`，不向 Engine 暴露密钥字节 |
| `ProfileSecretStore` | 为已有密钥消费者提供用途明确的读取/写入；已迁移时统一读 vault，不再读旧系统条目 | Infra；旧布局仅用于一次迁移，禁止长期“新格式失败再读旧格式” |
| `ProfileAutoUnlockStore` | 本 profile 唯一系统条目的读取、写入、复读、清除 | 只由恢复/安全生命周期负责人调用，复用 `SecureStorageAccess` |
| Engine 恢复运行期 | 提供受限公开操作，持有宿主资源，恢复完成后交接正常运行期 | Engine 管生命周期，不编排包装、迁移和删除步骤 |
| 既有空间事务负责人 | 改口令/切换空间时把新恢复包装纳入耐久事务 | 复用现有完整业务入口，不增加宿主步骤式调用 |

**规则与机制分离：** 原材料必须保留、旧条目最后删除、当前口令可恢复是业务规则；AEAD、文件同步、互斥和 OS keychain 是基础设施。恢复可用性不得用“某个文件存在”“缓存曾见过密钥”或错误字符串猜测，必须从结构化检查与认证结果确定，不引入 heuristic。

## Data Model

### 密钥关系

```text
用户口令 + vault 内保存的 KDF 参数
             ↓ 派生包装密钥
       解开被包装的 ProfileRootKey
                         ↑
系统钥匙串唯一条目 ──────┘ 自动解锁同一 ProfileRootKey
                         ↓
           解开 ProfileSecretPayload
                         ↓
   原 admission key / 原 content vault key / 原 profile generation / 其他必要材料
                         ↓
              原控制资料和原历史目录、正文
```

`ProfileRootKey` 为新生成的随机 32 字节本机根密钥，不替换任何旧历史密钥；钥匙串保存这把根密钥及最小非敏感选择信息。它是加速入口，不是恢复的唯一事实来源。不同用途继续使用原有独立密钥。

复用已有 Argon2id、XChaCha20-Poly1305、随机数源与 zeroize，不自制算法或新增密码库。用户口令仅在本次认证局部存在。KDF 参数和输入长度必须有明确边界，读取文件先限制大小，不能让篡改参数造成无界内存/CPU 消耗。

### 最终文件 `vault/profile-secrets-v1`

这是**拟新增格式**，名称需纳入仓库最终格式注册；整个功能只新增此最终版本，不保留开发中间版本。

| 字段 | 内容与限制 |
| --- | --- |
| `format_version`、算法标识 | 固定版本与允许的算法集合；未知版本失败关闭 |
| `kdf`、`salt` | 恢复根密钥所需参数；不是业务信息，不能含空间名或身份 |
| `wrapped_root` | 使用口令派生材料 AEAD 包装的 ProfileRootKey；独立 nonce |
| `encrypted_payload` | 由根密钥 AEAD 保护的整个 payload；独立 nonce |
| 认证上下文 | 固定用途、格式及包装头绑定；不得依赖原路径、机器标识或只有钥匙串才知道的 generation |

公开头仅含密码格式必要信息。`profile_generation`、内部身份、修订、迁移清单、活动恢复凭据修订和提交状态全部位于密文 payload。不同用途域分离；替换头、交叉拼接 wrapper/payload 必须认证失败。通过保留上一文件回滚的攻击不能仅靠本地文件完全阻止，见风险。

### `ProfileSecretPayload`

必须保存以下既有原值，不凭新默认值补齐：

| 材料 | 迁移处理 |
| --- | --- |
| `profile_admission_master_key:v1` | 原值保存；已有受保护控制资料而缺钥则部分不可恢复 |
| `profile_content_vault_key:v1` | 原值保存；已有历史目录而缺钥禁止重新生成 |
| `profile_lifecycle_marker:v1` | 原 generation 及合法阶段；恢复出厂中交回原负责人，不能重新开成 Ready |
| 当前 KEK 与 keyslot 的必要关联 | 空间恢复仍需要的原材料保存到受保护存储；不能继续藏在第二个系统条目里；keyslot 与恢复口令修订必须一致 |
| 设备身份材料 | `network/iroh/identity_store.rs` 的 `IDENTITY_STORE_KEY`（`iroh-identity:v1`）保存设备私钥本体，必须原值迁移；同时核对旧文件身份布局，不能在移机恢复中随机重建设备身份 |
| 未完成迁移/加入/改口令所需材料 | 按既有耐久记录准确枚举；要么完成旧事务再迁移，要么将完整恢复材料一并迁移，不丢弃进行中的业务 |
| 修订和清理记录 | `revision`、源材料集合认证摘要、`cleanup_pending`、待删除的受控旧条目分类；不在日志输出摘要或名称 |

清单集中于唯一 Infra 定义，供迁移、备份、导入导出和重置复用。旧名称仅在兼容适配器出现；不做任意 keychain 全量扫描。启动尚未初始化、没有历史的阶段不得伪造缺钥损失，也不能承诺尚未设置口令的资料已经具备口令恢复能力。

### 耐久状态

- `Legacy`：没有已提交的新格式，旧条目仍为事实来源。
- `Prepared`：准备文件已写但未发布；旧条目不动，正常路径仍用旧格式。
- `CommittedCleanupPending`：已发布完整可认证文件，新格式为唯一事实来源；旧条目只是待清理副本。
- `Committed`：旧条目清理并复查完成。

准备文件与事务日志也必须加密。文件提交必须复用仓库文件同步、同目录原子替换和平台写穿机制。跨文件操作不能假装一个 rename 就是全事务；以唯一耐久事务及其提交决策重放未完成步骤。

## API / Interface

以下名称是拟新增/扩展契约，不代表已存在。仅从 `uc-engine` 导出；内部类型通过 facade/deps 白名单组装。

- 继续使用 `Operation::UnlockSpace(UnlockSpaceInput { passphrase })` 作为宿主唯一口令提交动作。普通解锁保留现有成功行为；恢复服务中由同一入口完成认证、材料恢复、必要迁移及正常运行期交接。
- 新增 `Operation::QueryProfileRecovery`，返回 `ProfileRecoverySummary`；不得包含路径、密钥名称、空间标识或内部迁移步骤。
- `ProfileRecoverySummary`：`state`、`can_submit_passphrase`、`restart_required`、`background_ready`、`cleanup_pending`、`losses`。`losses` 仅为 `LocalHistory`、`LocalControlState`、`DeviceIdentity` 等稳定用户影响分类，不暴露内部密钥枚举。
- `state`：`NotRequired`、`AwaitingPassphrase`、`Recovering`、`Recovered`、`PartiallyRecoverable`、`Failed`。已有 ready 会话丢失自动解锁材料时可要求修复，但不能将其当前后台就绪事实置假；`background_ready` 独立表达真实能力。
- 恢复状态变化发 `ProfileRecoveryChanged`，并可重新查询快照；事件丢失不影响事实。
- `EncryptionStateSummary.session_ready` 在最小服务中必须为 false。新增 `StartupState::RecoveryAvailable` 表示已交付可访问恢复服务，不能沿用“正常后台 Ready”；启动交接后以恢复查询为状态真相，不让一次性启动通道永久承担恢复会话。
- 恢复服务允许恢复查询、加密状态查询、UnlockSpace、受控诊断、生命周期关闭；其他操作明确返回 `PROFILE_RECOVERY_REQUIRED`。诊断不得依赖尚未解开的业务数据库。

错误约定：

| 情况 | 对外结果 | 写入与重试 |
| --- | --- | --- |
| 用户输入认证失败 | 沿用 `UNLOCK_SPACE_UNAUTHORIZED_CODE`（1212） | 无持久业务或密钥写入，可重新输入 |
| 可恢复缺钥 | `AwaitingPassphrase`，非启动失败 | 不生成替代旧钥 |
| 结构损坏 | 沿用或映射 `UNLOCK_SPACE_CORRUPTED_CODE`（1216）/稳定 corrupt 分类 | 保留文件，不默默回退 |
| 未支持格式 | 独立 `PROFILE_RECOVERY_UNSUPPORTED` | 不尝试初始化或降级覆盖 |
| 旧资料永久缺少独立材料 | `PartiallyRecoverable` + 明确 loss；UnlockSpace 不返回 SpaceUnlocked | 不把“口令正确”当作后台已恢复 |
| 保存自动解锁材料失败 | `PROFILE_RECOVERY_PERSISTENCE_FAILED` | 不报告持久恢复完成；已有 ready 会话不清理 |
| 口令通过后其他启动依赖失败 | 保留对应启动错误，状态 Failed，`restart_required=true` | 同一实例不可重试；重启 Engine 后继续 |
| 升级备份存在但其保护材料永久缺失 | `PROFILE_UPGRADE_BACKUP_KEY_MISSING_CODE`（1224） | 不可重试，不生成替代材料，不绕过升级安全门槛 |
| 旧条目删除失败 | 新格式可用，`cleanup_pending=true` | 自动有界重试/下次启动继续，不报告完全迁移完成 |

新增数字错误码实施时在 `contract/error_codes.rs` 分配并通过唯一性测试；不得复用其他语义已有编号。Application/Infra 错误保留 source chain，对外只输出脱敏稳定分类。AEAD 失败本身无法区分错误口令与结构完整的密文篡改，不声称具备这种辨别能力。

## Workflow

### 启动与恢复

1. 完成只读布局检查与必要原样备份，不先生成 profile generation、admission key 或内容目录密钥。
2. 已提交新格式存在：先尝试唯一自动解锁条目并认证 payload；条目缺失或错误且口令包装结构合法时进入恢复服务。访问权限/存储错误单独分类，不能当作条目不存在。
3. 无新格式：按旧布局读取。存在旧资料却缺独立材料时报告旧格式损失；只有 KEK 缺失时允许原口令认证。完整旧材料进入迁移负责人。
4. 用户提交口令后认证根包装与整个 payload；认证失败不写磁盘或钥匙串。
5. 通过后恢复原材料能力，执行未完成事务，验证历史目录及活动资料一致性，写入并复读唯一自动解锁条目。
6. Application 完成恢复后由 Engine 启动原有后台。历史读取、新写入与接收就绪完成必要本地检查后报告 Recovered；网络不可达不能冒充本地资料不可恢复。
7. 运行期查询不反复读取钥匙串；显式恢复必须真实核实写回。锁定、暂停和关闭沿用现有可撤销会话与任务排空，不新增长期无人管理的任务。

### 新安装与旧设备迁移

1. 新安装尚无口令时只进入未初始化流程；设置/成功验证首个空间口令后才能提交可恢复格式。加入之前的本机临时安全记录仍需加密；该阶段的关闭恢复由现有加入负责人持有，不提前声明永久历史可恢复。
2. 旧设备取得独占迁移权限，完成或接管旧事务，读取受控清单及原值，认证现有 keyslot、活动清单和历史目录。
3. 正确口令为旧设备最终清理的验证条件；可以提前准备密文副本，但没有实际证明口令恢复链就不能清理旧条目。正常 ready 会话可继续由既有策略运行，迁移状态不能冒充 GUI 解锁授权。
4. 生成根密钥，原值装入 payload，写完整包装及密文；同步准备文件。
5. 从磁盘复读，在禁止访问旧独立条目的验证环境中以口令解开新副本，比较材料并读取代表性原历史；确认正文仍为原密文。
6. 原子发布新格式及耐久清理状态；此后新格式是唯一事实来源。写唯一自动解锁条目并复读确认。
7. 逐项删除明确清单中的旧条目并复查；不得清理系统或其他 profile 的条目。失败保留待清理状态，新格式继续使用。
8. 重新启动，验证历史读取和新内容保存。只有此后才能把“旧设备迁移完成”记为通过。

### 改口令和切换空间

- 当前恢复口令定义为本机最后一次**成功提交**的空间加密口令。改口令成功后用新口令；切换到目标空间成功后用目标口令。失败或未提交时继续用来源口令；无活动空间时保留最后成功的本机恢复口令，不因退出删除历史恢复能力。
- 保持根密钥和独立历史密钥稳定，仅重包根密钥，更新与空间 keyslot/registration 的关联；不遍历历史正文。
- 新包装在现有完整事务提交前准备，旧包装在事务未决定前保留可恢复能力。加密事务内保存恢复必要的旧/新材料及唯一提交决策；两条恢复路径只能用于重放该事务，不能直接绕过提交决策进入后台。
- 复用现有改口令 journal 与空间 activation 的完整事务所有权，纳入 vault 修订；Engine 不协调两个彼此独立的“成功”。提交后断电，以已提交目标完成重放，不能回滚成另一个活动空间。
- 清理完成后正常 API 只接受当前包装，不遍历历史 wrapper；旧备份已经被人复制的情况无法远程撤销，必须在说明中保留此边界。
- 工厂重置必须同时删除 vault、自动解锁材料和受控旧条目；现有“只删 keychain 就等于销毁密钥”的假设不再成立。重置中断也由既有完整负责人持有。

### userdata 搬迁

正常关闭或使用已有一致性导出，再复制完整 userdata（数据库、vault、受管 blob/文件内容与设备资料；仅复制 vault 不够）。当前 `crates/uc-core/src/app_dirs/mod.rs` 将 `file-cache` 放在 userdata 内，仍须在清单验收中覆盖实际文件内容。只有外部原路径、尚未下载或未保存的文件不属于已保存副本，不能承诺恢复这些缺失字节。新环境从持久资料身份选择 vault，绝对路径不参与解密。正确口令恢复后建立本机自动解锁入口，核实旧历史与新写入；不自动删除原副本。保留设备身份的迁移要求原实例停止，不能将同一身份的复制解释为新增设备；新身份配对属于另一条现有产品流程。

# 6. Implementation Plan

每步必须先完成最小可运行切片再扩展，不以拆掉现有可运行链换取未完成的复杂架构。

| Step | File / 范围 | Change | 主要风险与退出条件 |
| --- | --- | --- | --- |
| 0. 基线及材料闭合 | 本节列出的 inventory、secret_keys、profile_reset；现有 host_contract/key_loss 测试 | 对比 HEAD 与未完成补丁；枚举所有安全存储读写与持久依赖，记录哪些是必需材料、可重建缓存、其他 profile 或临时事务 | 清单缺项会造成假恢复；必须用代表性旧样本说明每项去向，完成 §11 中格式前置项 |
| 1. 最小恢复端到端 | Application profile/key_recovery；Infra 新 vault；Engine runtime/contract | 用一个独立样本创建新格式，完全清空安全存储，通过公开入口输入口令，读取历史并写入、重启 | 必须覆盖真正 Engine 装配，不只测包装函数；错误口令不写入 |
| 2. 旧设备原值迁移 | ProfileSecretStore、现有 KeyMaterialStore/AdmissionKeyManager/content vault/lifecycle | 将旧材料原值导入新密文，所有现有消费者收敛到唯一受保护来源，新增格式门禁 | 旧程序兼容/回退边界与迁移前备份先落实；提交前旧钥不能删 |
| 3. 耐久清理与中断 | Application 恢复负责人、Infra 原子 persistence、SecureStorageAccess | 独占迁移、修订检查、提交后逐项清理、复读、故障注入 | 每个断电点重启可继续；删除失败不生成新密钥或回退 |
| 4. 运行期状态与自动解锁修复 | space/security/access.rs、Engine recovery/operations、startup_owner | 移除 kek_observed 对显式写回的权威；正常会话与恢复状态分开；完善受限服务/事件/关闭 | 错误口令不能影响 ready 会话，取消不能丢失执行方的清理责任 |
| 5. 改口令/切换/重置 | change_encryption_passphrase、space_transition_activation、profile_reset | 包装与既有事务一起提交，覆盖来源/目标材料及重启；重置擦除新持久副本 | 无凭据与活动资料的混合提交；完整保留原有成员资格限制 |
| 6. 备份、导入导出与绑定 | config_migration、profile_upgrade_backup、bindings | 清单统一、新格式完整进入受控备份和导入；绑定映射状态及错误 | 升级原样备份不是单独恢复保证；其他机器不依赖原钥匙串 |
| 7. 完整验收及交付 | Engine/Infra/Application 测试；架构圣经、接口文档 | 全矩阵、明文探针、结构与编译检查；输出可接入契约和清晰变更边界 | 无提交授权，不创建干净提交；记录 base SHA、diff、检查日志和平台跳过项 |

完成后把稳定规则回写 `docs/security/encrypted-persistence.md`、`docs/design-docs/uc-engine-interface.md` 与架构圣经，再按文档规则将本计划移入 completed。实施期间不把设计表述为当前产品已具备的能力。

# 7. Edge Cases

| Scenario | Expected behavior | Implementation |
| --- | --- | --- |
| 空安装、尚无口令 | 正常首次设置，不显示“历史丢失” | 显式未初始化状态，不能凭一个空目录推断有资料 |
| 旧资料丢失一个/全部独立密钥 | 保留旧密文，如实报告影响范围 | 在生成任何同名材料前检查旧布局及认证结果 |
| 钥匙串值长度合法但错误 | 若新 vault 完整可进入口令恢复；不能用错误值修改文件 | 分开结构验证、自动解锁失败、口令认证 |
| vault 缺失、截断或被拼接 | 非空新格式资料失败关闭，不能当作首次安装 | 版本、上限、AEAD 与提交修订检查 |
| 错误口令 | 拒绝且不写材料、不锁后台 | 先认证后提交，运行中快照不变 |
| 并发恢复与切换/改口令 | 一个负责人串行或返回明确冲突，不相互覆盖 | 共享 profile 安全事务门禁及修订比较 |
| 写失败/磁盘满/断电 | 至少保留旧材料或已提交新副本 | 同目录原子提交、文件及目录同步、分阶段故障注入 |
| 旧条目删除失败 | 新格式继续使用，清理可重试 | 耐久 cleanup_pending，逐项复读 |
| 数据已解开但自动解锁写回失败 | 不报告持久恢复完成 | 写入复读；已有 ready 会话不清理 |
| 网络离线 | 本机历史恢复不依赖远端 | 正常远端重连独立处理；本地与网络就绪分开 |
| 极端口令/KDF/文件大小 | 有界拒绝，不泄露输入 | 验证边界后调用成熟密码实现 |
| 旧程序读取新格式 | 不作为支持的回退；不能把旧程序当验收成功 | 正式支持范围及旧二进制探针见 §11；不能仅新增旧程序不认识的标记就声称安全 |
| 只复制 userdata 的一部分 | 报告缺失/损坏，不声称可恢复全部资料 | 清单与真实历史验证；复制一致性前提明确 |
| 两台机器同时运行同一副本 | 不承诺安全新设备语义 | 冷迁移与设备克隆区分，测试保持原实例停止 |
| 工厂重置中断 | 不通过恢复入口复活已提交重置的资料 | 重置状态优先、恢复负责人服从耐久删除决策 |

# 8. Testing Strategy

## Unit Test

| 输入 | 操作 | 预期结果 |
| --- | --- | --- |
| 固定测试材料、正确/错误口令 | 包装后分别解开 | 正确字节一致；错误认证失败、写计数为零 |
| 头/nonce/payload 篡改，未知版本 | 读取 | 稳定分类，无 panic，无自动初始化 |
| 各迁移状态及故障点 | 推进、重复、重启重放 | 只提交一次；未验证新副本绝不删除旧条目 |
| storage 下层错误 | 跨层转换 | 稳定类别正确，source chain 可追溯且对外脱敏 |
| 持久清理部分完成 | 再次清理 | 不触碰范围外条目，缺失旧条目幂等处理 |

## Integration Test

使用真实临时目录、SQLite、现有密码库和可记录/注入失败的安全存储。只在独立测试资料移走条目，保留备份，不操作正式系统钥匙串。

| 输入 | 操作 | 预期结果 |
| --- | --- | --- |
| 含正文、标题、文件元数据与跨空间历史的旧样本 | 迁移并删除旧条目 | 原内容与密钥一致；新写入/搜索/重启可用 |
| 新格式完整 userdata | 关闭、移走整个 keyring、重新启动、错误/正确口令 | 恢复入口可用；错误无改动；正确恢复正常服务 |
| 新格式副本、全新根目录及空安全存储 | 独立进程启动并恢复 | 不读取原路径/钥匙串，旧历史与新写入通过 |
| 原 ready 会话 | 移走自动解锁条目，错误后正确口令 | 错误不影响后台；正确真实保存，重启可用 |
| 各 fsync/rename/keychain set/delete 边界 | 注入失败或终止子进程，再重启 | 没有唯一副本丢失，没有未提交混合状态 |
| 原口令 A | 改口令到 B，各步骤终止重启 | 按提交决策完成；最终 B 可恢复，正常接口拒绝 A |
| 空间 A 历史、加入空间 B | 成功/失败/中断切换并清空 keyring | 成功后用 B，失败前用 A；旧历史不重写、不自动外发 |
| 旧 admission/content vault 两把密钥分别缺失、长度错误、合法长度但错误值 | 逐个启动、提交口令并迁移 | 分类和影响范围准确；不生成替代旧钥，不删除旧条目，不修改原密文 |
| 旧格式独立材料已丢失 | 提交正确空间口令 | 明确部分不可恢复，不返回 SpaceUnlocked，不生成替代钥 |
| 正确密钥但数据库损坏/权限失败 | 启动或恢复 | 保留真实错误，不误归为等待口令 |

新增测试应通过真实公开 Engine 入口，沿用 `tests/host_contract` fixture；现有内存安全存储测试不能代替文件型安全存储与独立进程重启。校验磁盘/安全存储前后摘要时不记录敏感原值。明文探针覆盖 vault、准备文件、事务日志、数据库和诊断；输出仅为命中计数或受控路径，不输出探针正文。

## Regression Test

完整密钥重启；普通初始化/加入/失败恢复；已 ready 会话错误口令；锁定/暂停/关闭；升级备份；Factory Reset；现有改口令成员资格限制；历史搜索与跨空间历史保留；宿主契约及 iOS/Android/HarmonyOS 契约映射。

先执行聚焦测试，再串行执行仓库交付检查。Cargo 使用本仓共享 target，不创建临时完整编译树，不绕过现有编译缓存。

```bash
cargo test -p uc-engine --test host_contract key_loss --locked
cargo test -p uc-infra --lib --locked
cargo test -p uc-application --lib --locked
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-rust-style.mjs
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

最新主线上的验证结果：密钥丢失宿主场景 10 项通过；完整宿主契约 20 项通过、1 项按设计跳过；Infra 单元测试 980 项通过、8 项按设计跳过；Application 单元测试 949 项通过、1 项按设计跳过；公开契约 51 项、资料导入导出 2 项通过。Engine 单元测试中 234 项通过、4 项按设计跳过，另有 1 项跨进程传输测试超时；该测试在未修改基线上也能复现相同超时。完整 workspace 编译、格式、Rust 规范、仓库结构和差异检查均通过。新恢复文件测试确认原独立密钥名称和字节不会以明文出现。

Desktop 命令行程序使用本地 Engine 覆盖构建通过。0.19.1 样本启动后，后台稳定进入等待恢复口令状态，但 Desktop 当前仍显示启动画面并遮住已经渲染的恢复页，自动界面测试因此无法输入口令；按任务边界没有修改 Desktop 产品代码。新建资料自动界面流程未重复执行同一已知阻塞。实体系统钥匙串、iOS、Android、HarmonyOS 和旧版本程序回退记为跳过；Windows 已通过交叉编译检查，未执行真实系统运行验收。

# 9. Acceptance Criteria

* [x] 完整安全材料清单与所有消费者闭合；本 profile 的正常系统钥匙串条目只有一个。
* [x] 新格式未新增明文业务负载，所有密钥和事务材料加密保存，明文探针通过。
* [ ] Engine 测试已覆盖新安装与旧设备两条路径；Desktop 自动界面验收被现有启动画面遮挡问题阻塞，尚未完成最终界面证明。
* [x] 已落盘的新副本通过口令独立验证后才删除旧条目，范围外条目保持不变。
* [ ] 所有文件写入断点的独立进程终止注入未执行；删除失败、重启续清理和无伪成功已通过。
* [x] 单个/整个钥匙串丢失时仍可调用公开口令恢复入口。
* [ ] Engine 宿主测试已覆盖完整恢复、旧历史、新写入和重启；Desktop 0.19.1 样本尚未越过现有界面遮挡问题。
* [x] 运行期丢钥后错误口令不影响 ready 会话，正确口令真实保存自动解锁材料。
* [x] 恢复状态/错误完整区分，非密钥故障不被掩盖，不能把口令正确等同于服务就绪。
* [x] 旧格式不可恢复样本明确报告损失并保留原密文。
* [x] 改口令后仅新口令可恢复，旧历史保持可读。
* [ ] 空间切换各持久步骤的终止注入未单独执行。
* [x] 恢复出厂同时处理 userdata 密钥副本及系统条目，重启不复活已重置资料。
* [x] 备份和导入导出不遗漏新恢复材料，不依赖独立旧设备钥匙串条目。
* [ ] 老版本回退边界有真实二进制证据；未验证不能清理旧条目后声称可回退。
* [ ] 相关恢复测试、完整编译、格式、Rust 风格、仓库结构与差异检查通过；Desktop 自动界面验收受阻，Engine 另有一个可在未修改基线上复现的跨进程传输超时。
* [x] 设备/平台/系统钥匙串未执行项逐项记为“跳过”；构建、安装、启动与实际恢复不混为一项。
* [x] 交付明确 base SHA、变更清单、状态/错误约定与验证边界；按用户要求创建本地提交，无推送、发布或修改 Desktop。

# 10. Risks and Trade-offs

- **恢复便利与离线猜口令：** userdata 持有者可离线尝试口令，这是可移植口令恢复的固有代价。复用有成本的 KDF 并校验参数；不能用“加密了”代替口令强度与成本说明。
- **扩大根密钥影响范围：** 根密钥泄露可打开本机受保护材料，因此限制长期副本、复用可撤销会话、清理敏感临时值；保持内部用途隔离。
- **首次迁移的额外步骤：** 清理前需要实际口令验证，可能比仅依赖钥匙串的静默升级多一次输入。换取不删除唯一可恢复副本；不强制已 ready 会话停机等待输入。
- **性能：** 迁移仅搬密钥和必要元数据，成本不随历史正文总量增长；验证代表性内容及目录一致性，完整历史验收在测试中执行。正常读取不得每次 KDF 或 keychain 访问。
- **旧备份不能撤销：** 新口令使当前接口拒绝旧口令，但不能消除已复制旧 userdata/包装所授予的能力。本地修订不能提供对持有完整旧快照者的可靠防回滚。
- **销毁语义变化：** 删除钥匙串不再等于永久销毁资料；Factory Reset 必须删除新 vault 和恢复包装，文档/测试必须同步。
- **“只保留旧钥匙串方案”被否决：** 不满足口令恢复要求。“把所有历史改用当前空间密钥”被否决：破坏空间独立与切换成本。“每用途重新派生替代旧钥”被否决：会使旧密文失效。
- **层次泄露：** 不以通用字符串 key-value 大口暴露给 Application/Engine；具体旧名称和 AEAD 文件结构由 Infra 隐藏，Application 只拥有完整动作和结果。

# 11. Open Questions

以下是实施前必须用代码/样本或产品支持范围补齐的事项，不以猜测当作已解决，也不阻止先写只读清单与测试。

1. **旧版本兼容矩阵：** 需确认要覆盖的最老 Engine/产品版本和旧布局样本。已核对复现版本不代表覆盖全部历史版本。旧程序不会自动理解新标记；Step 2 清理前必须用支持范围内的旧二进制实测拒绝/回退行为，无法保证时明确禁止原地降级并提供独立备份恢复步骤，不能声称“加个新文件即可保护”。
2. **首个口令尚未建立的加入记录：** 必须核实已有 Fresh/Pending 路径的真实材料生命周期与撤销边界。产品的“凭口令恢复历史”从成功建立保护关系开始；不得让临时加入状态迫使新建第二个永久钥匙串入口。
3. **材料清单完整性：** 已发现 KEK、两把独立钥、lifecycle、identity 与条件性迁移钥；尚未逐个验证所有历史布局、备份保护钥及平台私有材料的恢复依赖。Step 0 必须完成读写调用审计和真实样本闭合后冻结最终 payload。
4. **跨文件最终提交细节：** 新包装与现有改口令/activation journal 的精确提交点须在 Step 0 基于真实事务锁及错误行为写出状态转换表，特别验证两份文件更新间断电、清空钥匙串后重放。该项未完成前不能进行破坏性旧条目清理；不得由两个独立成功回调替代。
5. **平台证明：** 当前只有 macOS 文件型安全存储原故障证据。实体 keychain 访问、Windows/Android 等文件替换语义与移机行为需要后续测试环境，未执行不能计为通过。

本规格已固定目标、材料保护关系、所有权、公开意图、迁移顺序、错误与验收；上述前置证据必须在对应步骤中补齐并更新本文，不能留到交付后让用户逐项检查。
