# 032 退役 Legacy Space Transition

状态：待实施；已按完成后的规格 033 重新基线化

关联事实来源：

- [不可变内容保护上下文实施记录](../completed/033-immutable-content-protection-context.md)
- [安全架构](../../SECURITY.md)
- [工程与模块设计原则](../../design-docs/engineering-principles.md)

# 1. Overview

规格 033 已经完成 V1/V2 到 V3 的一次性存储升级，并为普通 admission、Device Reset、membership branch、Fresh 和 SameSpace 分别建立了 control-only V3 transition owner。Ready profile 在任何普通运行期对象图构造前必须完成 `ProfileStorageUpgrade::ensure_v3()`；后续切换只替换 Space control generation，不再复制或重包 profile payload。

仓库仍保留 `crates/uc-infra/src/security/admission_space_transition.rs`。该文件约 5,201 行，完整包含已被 033 取代的 V2 source backup、`target.sqlite`、payload rewrap、整库/blob 切换和四个 port implementation。它的唯一生产构造点是 Engine 的 maintenance-only storage 分支；Ready profile 已无法进入该分支。`RuntimeStorageSelection` 同时仍允许普通运行期直接选择 V2 manifest，`space_generation_directory` 也继续从旧 transition 模块公开，虽然该路径知识现在只属于一次性升级器。

原 032 计划把旧文件机械拆成 `generation_store`、`payload_rewrap`、`generation_activation` 等子模块。这会延长已退休行为的寿命，并与 033 的 clean cutover 形成第二套事实来源。删除检查表明：删除旧模块后，旧 rewrap 复杂度不会散回调用方，而是直接消失；因此正确方案是退役 legacy executor，而不是重构保留它。

本计划先让 maintenance-only 对象图通过显式 adapter 失败关闭，再删除 legacy transition，把 V2 generation 定位知识收回 `ProfileStorageUpgrade`，并增加架构检查防止旧运行路径复活。现有 V3 owners、Application ports 和 Core checkpoint codec 保持不变。

# 2. Goals

- Ready profile 的四类 Space transition port 只能由 V3 owners 提供；任何 V2 manifest 都必须先经 `ProfileStorageUpgrade`，不能被普通运行期直接打开。
- maintenance-only profile 仍能构造用于 Factory Reset 恢复的最小对象图，但所有 admission、Device Reset、initial Space activation 和 membership branch transition 调用都在写入前稳定失败关闭。
- 删除 `DurableAdmissionSpaceTransition` 及其 V2 snapshot、payload rewrap、整库/blob generation 切换和自有测试，不保留改名副本或兼容 facade。
- 将旧 V2 generation 目录定位收口到 `ProfileStorageUpgrade` 私有实现；Engine 和 `uc_infra::security` 不再公开或消费该 helper。
- 保持 V3 admission、Reset、branch、Fresh、SameSpace 的 manifest promotion、control pool/keyslot/session 恢复、错误分类和磁盘格式不变。
- 旧 transition checkpoint 仍能由 Core codec 识别；V3 runtime 对旧 state variant 稳定返回失败且不执行 legacy 行为、不修改任何 generation。
- 架构预检必须拒绝重新引入 legacy transition concrete type、Ready-V2 runtime selection 或 V3 payload rewrap 依赖。

# 3. Non-Goals

- 不拆分、合并或重命名 `V3AdmissionSpaceTransition`、`V3DeviceManagementReset`、`V3MembershipBranchTransition`、`V3InitialSpaceActivation`、`SpaceControlGeneration` 或 `SpaceTransitionActivation`。
- 不增加统一 V3 transition facade；Engine composition root 可以知道 port 与具体 adapter，现有构造不属于调用方复杂度泄漏。
- 不修改 Core `AdmissionSpaceTransitionV2` 的旧 variant、编码格式或解码能力；格式退役需要独立的跨版本兼容决策。
- 不修改 Application 的四个 transition port、admission checkpoint 状态机、Space rebuild 编排或错误枚举；因此本计划不与规格 031 的 Application 依赖表面工作交叉。
- 不把旧 checkpoint 自动转换为 V3 transition，也不恢复旧 payload rewrap。规格 033 已接受“识别后稳定拒绝”；产品级取消/重试提示不在本计划范围。
- 不重新设计 maintenance-only runtime，也不把整个 `SpaceFacade` 从该对象图移除；本次只替换危险的 legacy adapter。
- 不修改 V3 profile data/control schema、密文格式、keyslot、manifest 或升级 journal。
- 不删除一次性升级器内部为读取 V1/V2 source 所需的旧 codec、AAD 和路径规则。

# 4. Current Architecture Context

```text
Component: ProfileStorageUpgrade
Path: crates/uc-infra/src/security/profile_storage_upgrade/
Responsibility: Ready profile 启动前唯一拥有 V1/V2 检测、全量 V3 转换、验证、promotion、重启恢复和旧 source 清理。
Relationship: Engine 只调用 ensure_v3；成功后普通运行期只能消费 V3 layout。
```

```text
Component: RuntimeStorageSelection
Path: crates/uc-engine/src/assembly/runtime_storage.rs
Responsibility: 从已认证 manifest 原子选择 profile database、control database、blob root 和 payload mode。
Relationship: 当前仍含 V2 manifest 分支；该分支在 Ready gate 成功后不可达，却使 legacy runtime 在类型层面继续合法。
```

```text
Component: DurableAdmissionSpaceTransition
Path: crates/uc-infra/src/security/admission_space_transition.rs
Responsibility: 旧 V2 admission、Device Reset、initial activation、membership branch、source snapshot、payload rewrap 与整库/blob 切换。
Relationship: 仅由 Engine maintenance-only 分支构造；Ready profile 已改用 V3 owners。
```

```text
Component: V3AdmissionSpaceTransition
Path: crates/uc-infra/src/security/v3_admission_space_transition.rs
Responsibility: Fresh、SameSpace 和 CrossSpace 的 control-only admission 状态机。
Relationship: 委托 SpaceControlGeneration 准备完整目标，并委托 SpaceTransitionActivation 生效；不依赖 profile payload store。
```

```text
Component: V3DeviceManagementReset / V3MembershipBranchTransition / V3InitialSpaceActivation
Path: crates/uc-infra/src/security/v3_device_management_reset/
Path: crates/uc-infra/src/security/v3_membership_branch_transition/
Path: crates/uc-infra/src/security/v3_initial_space_activation.rs
Responsibility: 分别拥有 Reset、成员分支和首次激活的完整 V3 流程语义。
Relationship: 各模块保留自身状态机和失败恢复，不机械复用 admission 流程。
```

```text
Component: SpaceControlGeneration
Path: crates/uc-infra/src/security/space_control_generation/
Responsibility: 准备、回读并验证一个完整 control generation，包括 security、关系、credential、ledger 与恢复状态。
Relationship: 向各 V3 flow owner 提供按业务意图命名的完整操作，不暴露逐表写入步骤。
```

```text
Component: SpaceTransitionActivation
Path: crates/uc-infra/src/security/space_transition_activation/
Responsibility: 唯一拥有 V3 manifest promotion、活动 control pool/keyslot/session 重绑和 promoted 后前向恢复。
Relationship: 接收已验证 generation proof；不拥有 profile payload，也不决定 Application checkpoint/retry。
```

```text
Component: Application transition ports and checkpoints
Path: crates/uc-application/src/space/admission/space_transition/ports.rs
Path: crates/uc-application/src/space/membership/recover_conflict/ports.rs
Responsibility: 表达 Application 完整流程所需的 Infra 能力，并保存可恢复业务 checkpoint。
Relationship: interface 与 Core codec 继续兼容旧 state；具体运行 adapter 由 Engine 按 profile lifecycle 选择。
```

当前 Ready 启动数据流：

1. Engine 读取 profile lifecycle；Ready profile 调用 `ProfileStorageUpgrade::ensure_v3()`。
2. `Pending` 或 `Busy` 直接阻止普通运行期构造；`Upgraded`、`UpToDate` 或 `FreshReady` 才产生 V3 storage selection。
3. Engine 构造各自的 V3 transition owners，并由 `SpaceTransitionActivation` 集中完成不可逆 promotion 与前向恢复。
4. profile lifecycle 非 Ready 时，Engine 选择 maintenance-only storage，但当前仍错误构造完整 legacy transition。

# 5. Proposed Design

## Components

### MaintenanceOnlySpaceTransitionPorts

- 位置：`crates/uc-engine/src/assembly/` 下的私有 wiring 模块。
- 职责：表示 profile lifecycle 非 Ready 时 Space transition 能力不可用；满足现有 dependency object 的 port 类型要求，但不读取数据库、manifest、密钥或文件。
- 输入：无业务输入之外的构造依赖；只能由 maintenance-only wiring 分支选择。
- 输出：
  - `AdmissionSpaceTransitionPort` 的 preflight、prepare、advance、discard 全部返回 `AdmissionSpaceTransitionError::Locked`；
  - `DeviceManagementResetDataPort` 四个操作全部返回 `AdmissionSpaceTransitionError::Locked`；
  - `InitialSpaceActivationPort` 返回 `CurrentSpaceIdentityError::Unavailable`；
  - `AdvanceMembershipBranchTransitionPort` 返回 `Unavailable { source }`，source 为固定、脱敏的 lifecycle capability error。
- 关系：复用既有 Application interfaces，不新增 port。它是 composition root 对 lifecycle 状态的显式 adapter，不是业务流程 module。

### ProfileStorageUpgrade legacy source layout

- 位置：`crates/uc-infra/src/security/profile_storage_upgrade/` 的私有 persistence/source 实现。
- 职责：仅根据已认证 V2 manifest 定位旧 generation source，供一次性 upgrade 读取和清理。
- 输入：profile root、旧 SpaceId、V2 database generation。
- 输出：受管 source directory/path，或现有 source-preserving upgrade error。
- 关系：不从 `uc_infra::security` 导出；Engine、V3 transitions 和普通 repository 不可调用。

### RuntimeStorageSelection

- 修改：保留 `None` 对应 maintenance-only legacy storage、`V3` 对应正常双库；遇到 `ActiveRuntimeManifest::V2` 返回明确 `UpgradeRequired`/等价内部错误，不再直接打开 `target.sqlite`。
- 原因：V2 manifest 是 upgrade source，不是可选的普通运行模式。类型层面失败关闭可防止未来绕过 startup gate。

### V3 transition owners

- 修改范围：原则上不改实现；只补缺失的 legacy checkpoint 负向 tracer 和必要注释。
- 原因：删除检查显示现有 modules 已隐藏真实复杂度。`V3DeviceManagementReset` 在 stage 阶段直接把 control pool 指向 mutable target，是 Application 随后写入目标 security state 的 Reset 专属协议，不是重复的 active manifest effect point；不得机械移入通用 activation。

### Architecture preflight

- 修改：在 `scripts/architecture/check-engine-repository.mjs` 增加 legacy retirement 检查。
- 拒绝：
  - `crates/uc-infra/src/security/admission_space_transition.rs` 或 `DurableAdmissionSpaceTransition` 重新出现；
  - Engine 普通 runtime 根据 V2 manifest 打开 generation database；
  - V3 transition 引入 `payload_rewrap`、source/target cipher pair、profile database 或 blob store；
  - `space_generation_directory` 再次从 `uc_infra::security` 公开。
- 允许：`profile_storage_upgrade` 私有实现继续包含 V1/V2 source 解析与一次性密文转换。

## Data Model

本计划不新增或修改持久化数据模型。

- `ActiveRuntimeManifestV3`、profile/control generation 路径和 digest 不变。
- `ProfileStorageUpgrade` journal phase、加密格式和恢复语义不变。
- 旧 `AdmissionSpaceTransitionV2::{Fresh, SameSpace, CrossSpace}` codec 暂时保留，只用于识别已有 checkpoint；不会再对应一个可执行 legacy adapter。
- maintenance-only adapter 不写 marker、journal、checkpoint 或 sentinel。

## API / Interface

不新增公共 interface，也不修改现有 port 方法。

删除的公开 Infra 表面：

```text
uc_infra::security::DurableAdmissionSpaceTransition
uc_infra::security::space_generation_directory
```

保留的外部 seam：

```text
ProfileStorageUpgrade::ensure_v3()
AdmissionSpaceTransitionPort
DeviceManagementResetDataPort
AdvanceMembershipBranchTransitionPort
InitialSpaceActivationPort
```

错误处理：

- maintenance-only admission/reset 使用现有 `Locked` 分类，不伪造 storage/recovery 失败。
- membership branch 的 unavailable 结果必须携带具体 source；不得字符串化、吞错或记录 profile/Space/路径。
- V3 runtime 遇到旧 checkpoint 继续返回现有稳定 `Inconsistent`；该结果发生在任何介质写入前。
- V2 runtime selection 的错误只在 Engine 内部转换为固定、脱敏的 startup prerequisite；Ready profile 正常路径不应观察到它。

## Workflow

### Ready profile

1. Engine 调用 `ensure_v3`。
2. 若升级未完成或锁竞争，启动保持不可用，不构造 transition ports。
3. 若返回 V3 ready，`RuntimeStorageSelection` 只打开 V3 profile/control/blob layout。
4. Engine 构造现有 V3 admission/reset/branch/initial owners。
5. 任何 Space 切换只操作 control generation；profile data generation 保持不变。

### Maintenance-only profile

1. Engine 读取到 Factory Reset 未完成的 lifecycle state。
2. 仅为清理/恢复打开 maintenance storage。
3. transition dependency slots 注入 `MaintenanceOnlySpaceTransitionPorts`。
4. 任一 Space transition 调用在读取或写入介质前返回 Locked/Unavailable。
5. Profile Factory Reset use case 继续通过独立 key/state cleaner 恢复，不依赖 Space transition adapter。

### V2 profile upgrade

1. `ProfileStorageUpgrade` 私有实现解析 V2 manifest 和 legacy generation path。
2. 完成既有快照、V3 转换、验证、promotion 与清理。
3. Engine 只在 reload 到 V3 manifest 后选择普通 runtime storage。
4. V2 path helper 不离开 upgrade module。

### 旧 transition checkpoint

1. Application/Core 仍能解码旧 state variant。
2. V3 adapter 在 variant 分派处稳定返回 `Inconsistent`。
3. 不创建 source backup、target payload generation 或 cipher pair，不修改 active manifest、profile data、blob 或 control store。
4. 后续产品级取消/重新发起由单独需求定义，本计划不恢复旧执行器。

# 6. Implementation Plan

每个切片独立提交；禁止在第一切片复制或移动 legacy 实现。

## Slice 1：断开 legacy production reachability

File: `crates/uc-engine/src/assembly/` 下新增私有 maintenance transition adapter
Change: 先通过现有四个 port 写 fail-closed contract tests，再实现 `MaintenanceOnlySpaceTransitionPorts`；所有方法在无 I/O 前返回规定分类，branch error 保留 source。

File: `crates/uc-engine/src/assembly/wire/mod.rs`
Change: maintenance-only 分支不再构造 `DurableAdmissionSpaceTransition`，改为注入私有 adapter；Ready V3 构造保持原样。

File: `crates/uc-infra/src/security/v3_admission_space_transition/tests.rs`
Change: 增加旧 `Fresh`/`SameSpace`/`CrossSpace` checkpoint 中至少一条代表性负向 tracer，证明稳定拒绝且全部介质摘要不变、无 legacy 路径。

Risk: maintenance-only runtime 仍构造完整 facade，可能有未预期调用。contract 必须逐个覆盖四种 port，而不是只证明 Engine 分支类型可编译。

Core impact: 无。第一切片不得修改 `uc-core` 或 Application port。

## Slice 2：删除 legacy executor 并私有化 V2 layout

File: `crates/uc-infra/src/security/admission_space_transition.rs`
Change: 整体删除，包括 V2 snapshot/rewrap/store/helper 与自有测试；不创建目录模块替代品。

File: `crates/uc-infra/src/security/mod.rs`
Change: 删除 legacy module 声明与 `DurableAdmissionSpaceTransition`、`space_generation_directory` re-export。

File: `crates/uc-infra/src/security/profile_storage_upgrade/`
Change: 在 upgrade 私有实现中拥有 V2 source generation 路径派生；保持现有路径字节和清理行为。外部 integration fixture 可以在测试内显式构造旧格式路径，不新增 production public helper。

File: `crates/uc-engine/src/assembly/runtime_storage.rs`
Change: V2 manifest 不再产生普通 runtime selection；返回明确内部 upgrade-required error。`None` maintenance 与 V3 normal 两种合法选择保持。

File: `crates/uc-infra/tests/profile_storage_upgrade.rs`
Change: 保留真实 V2 upgrade、崩溃恢复、密文探针和清理测试；fixture 不依赖已删除的公开 transition helper。

Risk: V2 source 路径字节若漂移会导致老 profile 无法升级。必须先用既有 V2 fixture 固定路径，再移动知识并运行完整 upgrade 集成测试。

## Slice 3：删除证明与文档收口

File: `scripts/architecture/check-engine-repository.mjs`
Change: 增加 legacy module/type/export、Ready-V2 runtime selection 和 V3 payload rewrap 的正向扫描与可执行负向 fixture。

File: `docs/architecture/architecture-bible.md`
Change: 记录 legacy executor 已删除、V2 layout 只属于 upgrade、maintenance-only transition 失败关闭；不复制 033 的 V3 密文契约。

File: `docs/exec-plans/active/032-admission-space-transition-internal-refactor.md`
Change: 验收完成后记录实际结果并移入 `completed/`；同步 active/completed index 和 tech-debt tracker。

Risk: 只删代码不加检查会让后续 Agent 为“兼容”重新引入旧 rewrap。架构 fixture 必须证明违规示例会失败，而不是只依赖字符串注释。

# 7. Edge Cases

```text
Scenario: Ready profile 的 V2 升级尚未完成或另一进程持有升级锁。
Expected behavior: Engine 返回现有 StorageUpgradePending，不构造 maintenance 或 V3 transition runtime。
Implementation: 保持 ensure_profile_storage_v3 对 Pending/Busy 的现有 gate。
```

```text
Scenario: lifecycle 处于 FactoryReset::Started 或 KeysWiped。
Expected behavior: Profile Factory Reset 可继续；所有 Space transition port 在 I/O 前返回 Locked/Unavailable。
Implementation: maintenance-only adapter 无 repository、manifest、session 或文件依赖。
```

```text
Scenario: 已有 V2 manifest 指向缺失或损坏的 source generation。
Expected behavior: ProfileStorageUpgrade 失败关闭并保留 source chain；RuntimeStorageSelection 不绕过升级直接打开它。
Implementation: V2 path 只由 upgrade source validation 消费。
```

```text
Scenario: 升级后数据库仍保存旧 admission transition checkpoint。
Expected behavior: 能解码但不能执行；返回稳定 Inconsistent，所有 V3 generation 和 payload 字节不变。
Implementation: 保留 Core codec，删除 concrete legacy executor，并补 V3 负向 tracer。
```

```text
Scenario: 旧 binary 打开 V3 profile。
Expected behavior: 仍在首次写入前返回 UnsupportedVersion，磁盘不变。
Implementation: 保留 033 的 manifest version gate 测试。
```

```text
Scenario: Device Reset 在 mutable target staging 后崩溃。
Expected behavior: 重启按 manifest 打开 source，journal 恢复后再次把 control pool 指向 target；不把 staging rebind 误认为 promotion。
Implementation: 不移动 V3DeviceManagementReset 的 stage ownership，保留现有 tracer。
```

```text
Scenario: legacy 文件删除后测试仍需要构造历史 V2 generation。
Expected behavior: compatibility fixture 显式生成旧路径/manifest；production 不重新公开 helper。
Implementation: 测试 fixture 与 upgrade-private implementation分别表达“历史输入”和“当前 reader”，避免测试通过公共运行接口制造旧状态。
```

```text
Scenario: 极端或恶意旧 checkpoint 包含损坏长度/版本。
Expected behavior: Core decode 或 V3 validate 稳定拒绝，不分配无界数据、不记录原始字节。
Implementation: 保留现有有界 codec/validate 测试；本计划不增加 fallback reader。
```

# 8. Testing Strategy

## Unit Test

- Maintenance admission：输入任意 preparation/transition；调用 preflight、prepare、advance、discard；预期全部为 `Locked` 且没有依赖调用。
- Maintenance Reset：对同一 target Space 调用四个方法；预期全部为 `Locked`。
- Maintenance initial activation：输入任意 Space；预期 `CurrentSpaceIdentityError::Unavailable`。
- Maintenance branch：输入合法 transition；预期 `Unavailable` 且 `source()` 非空、Debug 不泄露输入。
- Runtime storage V2：输入合法 V2 manifest 与存在的旧数据库；预期 `UpgradeRequired`，不返回 database/blob 路径。
- V3 old-checkpoint rejection：输入一个可解码旧 transition variant；预期 `Inconsistent`，manifest 和所有介质摘要不变。

## Integration Test

- V2 profile startup upgrade：准备真实 V2 manifest、SQLite、blob、keyslot 和密文；运行 `ensure_v3`；预期最终 V3 双库可读、旧 source 只在 promotion 后清理。
- Upgrade crash matrix：在既有每个 journal phase 重建 owner；预期单调恢复，不依赖已删除 transition module。
- Maintenance Factory Reset recovery：准备 Started/KeysWiped lifecycle；构造 Engine 并继续 reset；预期清理完成，期间没有 Space transition artifact。
- V3 A→B→A：复用 033 tracer；预期 profile data generation、SQLite 和 blob 字节不变，只替换 control generation。
- V3 Reset/branch/Fresh/SameSpace：复用现有真实 repository tracer；预期行为和错误分类不变。

## Regression Test

- `rg` 证明仓库不存在 `DurableAdmissionSpaceTransition`、`rewrap_finalized_source`、`source-backup-v1` 和 legacy module 文件。
- 旧 binary version gate 继续拒绝 V3 profile。
- `ProfileStorageUpgrade` 仍是唯一包含旧 payload reader/转换的运行模块。
- architecture negative fixtures 对 legacy type、Engine V2 runtime 和 V3 rewrap 依赖逐项失败。
- workspace all-target check 保证三个移动平台绑定不依赖任何被删 Infra export。

# 9. Acceptance Criteria

* [ ] maintenance-only wiring 不再构造 `DurableAdmissionSpaceTransition`，四类 transition port 在 I/O 前稳定失败关闭。
* [ ] maintenance branch unavailable error 保留非空 source chain；错误和日志不包含 Space、checkpoint、文件名或路径。
* [ ] `crates/uc-infra/src/security/admission_space_transition.rs`、legacy concrete type及其全部 rewrap/snapshot 测试已删除，无改名副本。
* [ ] `space_generation_directory` 不再从 `uc_infra::security` 导出；V2 source path 只存在于 `profile_storage_upgrade` 私有实现和兼容 fixture。
* [ ] `RuntimeStorageSelection` 不能把 V2 manifest 作为普通 runtime 打开；Ready profile 仍只能在 V3 upgrade 成功后启动。
* [ ] 真实 V2 profile 升级、崩溃恢复、promotion、清理和明文探针测试全部通过。
* [ ] 旧 admission checkpoint 可识别但稳定拒绝，拒绝前后 active manifest、profile/control SQLite 和 blob 摘要完全一致。
* [ ] A→B→A、Device Reset、membership branch、Fresh、SameSpace V3 tracer 全部保持通过，profile payload 不发生 rewrap。
* [ ] Core 和 Application port/状态机没有改动；032 与 031 可独立实施。
* [ ] 架构预检能拒绝 legacy transition、Engine Ready-V2 runtime 和 V3 payload rewrap 三类负向 fixture。
* [ ] architecture bible、计划索引和技术债状态与代码一致；032 完成后移入 completed。
* [ ] `cargo metadata --locked --format-version 1`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、Engine architecture preflight 和 `git diff --check` 全部通过。
* [ ] 未执行的实体设备矩阵明确记录为“跳过”，不记为通过。

# 10. Risks and Trade-offs

- **旧 checkpoint 无法自动继续**：删除 executor 后，旧 transition 只能稳定拒绝。替代方案是把旧 checkpoint 转换成 V3，但旧状态包含 payload snapshot/rewrap phase，无法安全映射到 control-only proof；本计划选择拒绝并保留 codec，不伪造恢复。
- **maintenance-only adapter 是浅实现**：它几乎没有 implementation depth，但没有引入新 interface；它是现有 seam 的生命周期 adapter，用于阻止危险能力，不是假抽象。更彻底的方案是 maintenance runtime 不构造 SpaceFacade，影响面远大于本计划。
- **V2 path 私有化增加 fixture 重复**：兼容测试需要本地构造历史路径。该重复只代表外部历史输入，不成为 production 第二事实来源；相比公开 legacy helper，能阻止普通运行期继续依赖 V2 layout。
- **删除大量测试会降低表面覆盖数**：旧模块测试验证的是已退休行为，保留只会阻止 clean cutover。替代证据来自 V2 upgrade integration、V3 flow tracers、old-checkpoint rejection 和架构负向检查。
- **不统一 V3 owners**：会保留多个 concrete types 和 Engine 构造代码，但每种流程有不同状态与恢复语义。统一 facade 只能隐藏构造，不会减少调用方动作，且容易重新形成依赖袋，因此拒绝。
- **不移动 Reset staging rebind**：它看起来与 activation 的 runtime rebind 相似，但发生在线性化点前并服务于 Application 中间写入。合并会让通用 activation 知道 Reset phase，破坏 locality；本计划接受命名相近但语义不同的两种操作。

# 11. Open Questions

- 旧 Core transition variant 在多少个发布周期后可以删除，需要产品版本兼容窗口或真实存量证据；这不阻塞本计划，因为 codec 很小且不授予执行能力。
- 是否需要为旧 checkpoint 增加用户可见的“取消并重新加入”结果，属于产品恢复语义，需另立 Product Spec；032 只保证稳定、无副作用地拒绝。
- maintenance-only runtime 是否最终应缩减为只构造 Profile Factory Reset facade，需要单独审计 Engine host 生命周期；本计划不以此为删除 legacy executor 的前置条件。
