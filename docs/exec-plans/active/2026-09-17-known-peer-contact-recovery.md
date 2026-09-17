# 已知设备联系驱动的成员恢复与地址更新

## 状态

- **状态**：实现完成，Windows 实机与真实公网验收待执行
- **日期**：2026-09-17
- **问题来源**：Windows 保存旧直连地址、公开发现没有返回新地址后，已启动的另一端虽然能主动联系 Windows，Windows 仍可能等待成员历史的最长五分钟重试期限
- **完整负责人**：`uc-application` 的成员维护流程
- **调用方唯一动作**：网络实现只报告一次已经通过传输身份验证、并能映射到当前成员的联系；不得授予成员资格、修改在线状态或编排成员恢复步骤
- **成功结果**：成员维护立即对该成员执行一次有界历史确认；确认成功后既有准入与连接流程自然恢复
- **失败结果**：保留原有失败分类和持久重试欠账；联系通知本身不改变授权、在线状态或地址持久化事实
- **重试与重启责任**：成员维护继续拥有合并、串行、暂停、恢复、周期兜底和持久重试；连接负责人继续拥有建连重试；网络 adapter 不建立第二套重试循环

# 1. Overview

当前普通成员只有在 `PeerReachabilityChanged::Online` 后才会触发定向成员维护。但成员历史尚未确认时，
`IrohPeerReachabilityHandler` 会在已经验证远端传输身份、并将其映射到本机成员后，因为
`PeerAdmissionPort` 尚未允许普通通信而拒绝连接。于是出现循环：成员历史需要一次及时同步才能准入，
而定向成员同步又要等到准入成功并发布 Online 才触发。

成员历史的周期恢复有持久退避，最长为五分钟。保存的直连端口过期、公开发现返回空结果时，主动拨号的一端
无法及时触发另一端跳过这个等待。实际日志表现为：网络往返和协议处理都很快，但 Windows 在旧地址与成员确认
重试之间等待很久，直到周期维护再次运行。

本计划把“已知设备正在联系”定义为恢复机会，而不是授权事实。Iroh 入站握手已经用连接公钥证明远端传输身份，
并且该身份能映射到本机当前成员时，Infra 发出一个脱敏、瞬时的 `KnownPeerContact`。Application 成员维护收到后，
只对这个成员立即执行历史确认，绕过其持久退避期限。未知设备不会产生通知；通知不会直接标记 Online、不会直接
授予准入，也不会自行保存地址。

后续切片再把已经验证的稳定 relay 提示写回现有加密地址仓储，并补充真实三设备、换端口、发现为空的独立进程验收。
不把临时直连 IP/端口作为新的长期事实。

# 2. Goals

- 已知设备在普通通信准入前发起经过传输身份验证的联系时，立即触发针对该设备的成员历史确认。
- 定向确认绕过该设备尚未到期的持久退避；无需推进逻辑时间，也无需手动刷新或发送剪贴板内容。
- 同一设备的重复联系由现有成员维护队列合并；不同设备继续串行处理，不新增并行成员写入。
- 未知设备、身份解析失败和已经移除的设备不能触发成员历史发送。
- 联系通知本身不能改变 Online、成员资格、准入结论或持久地址。
- 已验证 relay 地址可以在后续成功业务交换中写回现有加密地址仓储；动态直连地址不作为长期提示保存。
- 自动化验收可确定性制造旧端口、发现为空、成员历史落后和单向主动联系，并在 20 秒预算内判断恢复结果。

# 3. Non-Goals

- 不降低全局成员历史退避上限，也不缩短普通离线设备的周期重试。
- 不把 mDNS、DNS、pkarr 或地址出现本身当作身份认证或成员资格证明。
- 不允许未知设备通过反复连接触发成员维护、地址写入或 Online。
- 不保存动态直连 IP/端口作为新的长期地址策略。
- 不新增产品公开操作，不要求 Desktop、Mobile 或绑定层参与恢复编排。
- 不用手动刷新、正文发送或完整网络重建掩盖恢复缺口。
- 第一实施切片不改变持久化格式，不实现 relay 地址回写，也不声称完成 Windows 实机验收。
- 不把 test-only 内部阶段或成员状态暴露为 Engine 稳定接口。

# 4. Current Architecture Context

```text
Component: 普通成员连接负责人
Path: crates/uc-application/src/space/connectivity/peer_connections/
Responsibility: 启动、目标选择、连接机会合并、失败重试、暂停、恢复与关闭。
Relationship: 地址变化和网络变化只驱动连接尝试；发现事实不授予成员资格。本计划不把成员恢复步骤移入该模块。
```

```text
Component: 成员维护负责人
Path: crates/uc-application/src/space/membership/maintenance/
Responsibility: 串行执行准入恢复、成员效果、冲突、更新投递、历史同步和本地投影；拥有触发合并、暂停、恢复和周期兜底。
Relationship: 当前只把 Online 事件转为 `PeerOnline` 定向维护；本计划增加联系触发，并限制为历史确认与必要投影。
```

```text
Component: 成员历史同步
Path: crates/uc-application/src/space/membership/synchronize_history/
Responsibility: 选择当前成员、交换签名历史、保存逐成员确认水位、失败退避和关系结果。
Relationship: `AuthenticatedPeer` 已能对单一当前成员跳过周期退避；当前缺口是准入前没有可信触发来源。
```

```text
Component: Iroh 普通成员入站处理
Path: crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs
Responsibility: 从连接公钥派生指纹、映射本机成员、检查准入、维护已验证连接和发布 Online/Offline。
Relationship: 它是最早同时拥有传输身份和本机成员映射的边界；适合报告联系事实，但不得决定成员恢复流程。
```

```text
Component: Engine 网络组合根
Path: crates/uc-engine/src/assembly/sync_engine.rs
Responsibility: 构造 Iroh adapter、Application 对象图并连接内部事件通道。
Relationship: 只创建并传递联系事件通道，不读取事件、不选择目标、不编排成员维护。
```

```text
Component: 已配对设备地址仓储
Path: crates/uc-core/src/ports/peer_address.rs, crates/uc-infra/src/db/repositories/peer_address_repo.rs
Responsibility: 以加密关系记录保存 adapter 定义的地址提示。
Relationship: 当前配对和成员材料会保存 `to_persistable_addr` 结果；有 relay 时剥离动态直连地址，无 relay 时仍保留直连地址。后续切片只写回经过身份和成功交换共同验证的稳定 relay。
```

```text
Component: 独立进程连接恢复验收
Path: scripts/testing/connection-recovery-network.mjs, tests/hosts/connectivity/
Responsibility: 用 Linux 网络命名空间、真实进程、分区、重启、本地 relay 和结构化证据验证恢复。
Relationship: 已能保留身份与资料重启、阻断网络和重复运行；需增加固定换端口、发现阻断证明和三设备成员历史落后场景。
```

# 5. Proposed Design

## Components

### `KnownPeerContact`

- **职责**：表达“一条入站 Iroh 连接已经证明远端传输身份，并映射到本机已知成员”。
- **输入**：映射后的 `DeviceId`。
- **输出**：内部广播事件。
- **生命周期**：仅当前进程内存在；不持久化；允许丢失，因为周期维护仍是兜底。
- **安全边界**：它不是 Online、不是准入成功、不是当前成员关系已经一致的证明。

### Iroh 入站联系报告

- **职责**：公钥指纹成功映射到成员、但普通通信准入尚未通过时报告一个联系。
- **输入**：Iroh 已认证连接的远端公钥、本机成员仓储。
- **输出**：`KnownPeerContact` 或无事件。
- **关系**：未知身份、指纹派生失败、成员读取失败均不报告；原连接拒绝和关闭行为保持不变。

### 成员维护联系触发

- **职责**：接收联系事件，按设备去重并排入既有串行维护队列。
- **输入**：`KnownPeerContact`。
- **输出**：`MembershipMaintenanceTrigger::PeerContact(DeviceId)`。
- **关系**：暂停时不执行；恢复后由既有 Resume 全轮维护兜底；通道滞后由周期维护兜底。

### 联系驱动的成员确认

- **职责**：只执行目标成员的历史同步和随后本地投影。
- **输入**：`PeerContact(DeviceId)`。
- **输出**：既有 `MembershipMaintenanceReport`。
- **关系**：不运行普通 Online 后才有意义的成员更新投递，也不把联系变成准入或 Online。

### 稳定地址回写

- **职责**：Application 在当前成员的完整历史同步通过业务校验并提交成功后，调用 Infra 从 Iroh 当前远端信息中提取 NodeId 与 relay，并更新现有地址记录。
- **输入**：已经完成身份、当前成员资格和业务协议验证的对端；当前远端地址快照。
- **输出**：只包含 NodeId 与 relay 的 `PeerAddressRecord`，或“没有稳定提示”。
- **关系**：普通传输成功、无效业务回复和已移除成员的受限投递不触发写入；写入失败不改变已经完成的业务结果；动态直连地址不写回；未知或未准入连接不写回。

## Data Model

```text
KnownPeerContact
- device_id: 已由本机成员仓储映射出的设备标识
```

该事件不增加时间戳、地址、来源字符串或认证材料。队列去重只需要设备标识；时间由现有维护观测记录实际请求、
排队和执行耗时。事件不落盘，因此没有格式升级或旧版本读取责任。

后续稳定地址回写复用 `PeerAddressRecord`，不新增数据库字段。`addr_blob` 的最终内容必须满足：固定 NodeId；存在时
包含已验证 relay；不包含动态直连地址。`observed_at` 使用成功交换发生时的现有时钟能力。

## API / Interface

- Application 新增 crate 内部 `KnownPeerContact` 类型。
- `ApplicationSpaceAdapters` 与 `SpaceFacadeDeps` 增加一个联系事件接收端。
- `IrohPeerReachabilityAdapter` 构造接收联系事件发送端；`IrohSessionBuilder::install_peer_reachability` 原样传入。
- `SpaceMembershipMaintenanceRuntime::prepare` 接收联系事件接收端。
- `MembershipMaintenanceTrigger` 增加 `PeerContact(DeviceId)`。
- 不修改 `uc-engine` 稳定 `Operation`、`OperationResult`、绑定或产品调用方式。
- 内部广播发送失败表示当前没有消费者或消费者滞后，不改变连接结果；周期维护继续兜底。

错误处理沿用现有规则：身份映射和准入检查失败保持当前安全拒绝；成员同步的网络、存储和损坏错误继续由现有
同步负责人分类并保留来源链。联系广播不引入可对外的新错误。

## Workflow

1. 对端向本机普通成员连接入口建立 Iroh 连接。
2. Iroh 验证连接公钥，Infra 使用现有指纹工厂映射本机成员。
3. 映射失败时按未知设备拒绝，不发联系事件。
4. 映射成功后继续执行原准入检查；检查通过时保持原 Online 流程，不发联系事件。
5. 若成员历史尚未确认，报告 `KnownPeerContact`，原连接仍被拒绝，且不得发布 Online。
6. 成员维护收到联系事件并把 `PeerContact(device)` 排入既有串行队列。
7. 成员维护只对该设备执行历史同步；定向同步不受该设备原持久退避期限限制。
8. 同步成功后保存一致关系与确认水位，并执行本地投影。
9. 连接负责人下一次普通重试通过既有准入检查，双方发布 Online。
10. 同步失败时保存既有退避欠账；后续联系、状态变化或周期维护仍可恢复。

# 6. Implementation Plan

## Slice 1：可信联系立即触发定向成员确认

### Step 1

- **File**：`crates/uc-application/src/space/membership/synchronize_history/tests.rs`
- **Change**：先补充回归测试：目标成员的 `next_attempt_at_ms` 在五分钟后时，联系触发使用定向目标并立即完成，不推进时钟。
- **Risk**：若只直接调用已有定向入口，测试不能证明运行期接线；因此它只证明退避规则，后续 runtime 测试证明触发接线。

### Step 2

- **File**：`crates/uc-application/src/space/membership/maintenance/model.rs`、`use_case.rs`、`runtime.rs`、`tests.rs`
- **Change**：增加 `PeerContact` 触发；runtime 消费联系事件并按设备合并；use case 只执行定向同步与投影。先写 runtime 失败测试，再做最小实现。
- **Risk**：误用 `PeerOnline` 会把尚未准入的设备当在线，并执行不合时宜的更新投递；必须保持两个触发语义独立。

### Step 3

- **File**：`crates/uc-application/src/application.rs`、`space/application.rs`、`space/facade/deps.rs`、`space/facade/facade.rs`
- **Change**：把联系事件接收端交给成员维护 runtime；不增加公开 facade 方法。
- **Risk**：对象图参数增加可能遗漏测试构造；编译与全部 Application 测试负责发现遗漏。

### Step 4

- **File**：`crates/uc-infra/src/network/iroh/peer_reachability_adapter.rs`、`node.rs`
- **Change**：已知身份在准入检查前报告联系；增加已知未准入会报告、未知不会报告、报告后仍不 Online 的测试。
- **Risk**：成员仓储可能包含已经不在当前同步范围的旧事实；Application 定向同步必须再次核对当前范围并失败关闭。

### Step 5

- **File**：`crates/uc-engine/src/assembly/sync_engine.rs`
- **Change**：组合根创建内部广播通道，并分别交给 Infra 发送端与 Application 接收端。
- **Risk**：不得让 Engine 读取事件或依据事件调用成员步骤。

### Step 6

- **File**：`crates/uc-observability-contract/src/diagnostics/`、`docs/design-docs/observability.md`
- **Change**：将联系触发与 Online 触发分开记录，保持固定脱敏分类；不记录设备、地址或连接材料。
- **Risk**：若复用 peer_online 名称会造成诊断误导；若新增任意字符串会破坏封闭合同。

## Slice 2：稳定地址提示写回

### Step 7

- **File**：`crates/uc-infra/src/network/iroh/persistable_addr.rs` 及完成身份与成员验证的协议 adapter
- **Change**：定义“可长期保存”的唯一转换：NodeId 加已验证 relay；无 relay 时返回无稳定提示，不用新观察覆盖已有稳定 relay。
- **Risk**：当前 LAN-only 依赖直连地址；该模式不能被 P2P 稳定地址规则误伤，需单独保留用户明确选择的 LAN 语义。

### Step 8

- **File**：现有 `PeerAddressRepositoryPort` 写入调用点和真实加密仓储测试
- **Change**：成功交换后尽力写回稳定提示，失败不反写业务结果；验证密文边界和重启读取。
- **Risk**：不能把写回扩散到每个协议；选择一个已有完整成功边界，并复用共享能力。

## Slice 3：独立进程可重复验收

### Step 9

- **File**：`tests/hosts/connectivity/src/main.rs`、`scripts/testing/connection-recovery-network.mjs`
- **Change**：构造 A、B、C 三设备：先让 A/B 建立关系，再隔离 A/C 并由 B 接纳 C，使 A 已知 C 但尚未完成关系确认；随后停止 C、替换固定端口，阻断 A 的本地发现与主动发起，只允许 C 主动联系 A。
- **Risk**：网络故障必须有独立端口和防火墙计数证明，不能从 Engine 离线状态反推故障已经生效。

### Step 10

- **File**：同上及 `scripts/testing/run-connection-recovery-e2e.sh`
- **Change**：从 C 重启并开始联系计时，20 秒内通过公开查询观察 A/C 双方 Online；此前禁止 refresh、opportunity 和正文发送；上线后双向传输；默认重复三次并保留脱敏时间线。
- **Risk**：测试必须串行使用共享构建目录；Linux 网络命名空间验收不能登记为 Windows 实机通过。

# 7. Edge Cases

```text
Scenario: 未知设备发起连接
Expected behavior: 原样拒绝；不产生联系事件、不运行成员同步、不保存地址、不发布 Online。
Implementation: 只有公钥指纹成功映射到成员后才发送事件；Application 仍重新核对当前同步范围。
```

```text
Scenario: 已知但尚未准入的设备发起连接
Expected behavior: 产生联系事件，原连接仍被拒绝；成员确认成功前保持非 Online。
Implementation: 身份映射成功但准入检查未通过时发送事件；Online 发布仍只在原准入确认成功之后。
```

```text
Scenario: 同一设备短时间重复连接
Expected behavior: 当前执行不并发，排队中的相同触发合并；最多再执行一次定向确认。
Implementation: 复用 maintenance runtime 的活动轮次与相同触发合并规则。
```

```text
Scenario: 多个设备同时联系
Expected behavior: 按既有队列串行处理，每个设备各保留一个触发；不让一个设备覆盖另一个设备。
Implementation: `PeerContact(DeviceId)` 的相等性包含设备标识。
```

```text
Scenario: 成员维护处于暂停或关闭
Expected behavior: 不执行新网络工作；恢复后的全轮维护或下次联系继续恢复。
Implementation: 复用 runtime 的暂停、恢复和关闭分支，不建立旁路任务。
```

```text
Scenario: 联系事件通道滞后或无人订阅
Expected behavior: 当前连接结果不受影响；周期维护保留最终恢复能力。
Implementation: 广播发送为尽力而为，不把发送失败转换为连接错误。
```

```text
Scenario: 联系发生后设备已被移除
Expected behavior: 不向该设备发送成员历史。
Implementation: 定向同步在执行时再次调用当前成员范围检查，失败关闭。
```

```text
Scenario: 定向成员同步失败
Expected behavior: 保留原失败分类与持久退避；不会形成无界立即循环。
Implementation: 一次联系只对应一次队列触发，失败继续由现有同步逻辑保存重试欠账。
```

```text
Scenario: 只有动态直连地址，没有 relay
Expected behavior: P2P 模式不把该记录视为可长期使用的新提示；LAN-only 保留其明确的直连语义。
Implementation: 后续地址切片按运行模式区分，不全局删除 LAN-only 地址。
```

```text
Scenario: 旧版本对端
Expected behavior: 联系事件完全由本机从已有普通连接握手推导；不修改线上协议，旧对端无需升级才能触发。
Implementation: 不增加 frame 字段、ALPN 或版本协商。
```

# 8. Testing Strategy

## Unit Test

1. **五分钟退避被定向联系绕过**
   - 输入：当前成员；确认水位未完成；`next_attempt_at_ms = now + 300_000`；成功 transport。
   - 操作：执行联系对应的定向成员同步，不推进时钟。
   - 预期：立即向该成员交换一次；关系和水位完成；重试计数与下次时间清零。

2. **联系维护只做必要工作**
   - 输入：`PeerContact(device-b)`。
   - 操作：执行成员维护。
   - 预期：只调用定向同步与本地投影；不调用准入恢复、成员效果和普通 Online 投递。

3. **重复联系合并**
   - 输入：第一轮维护阻塞时连续发送两个相同设备联系和一个不同设备联系。
   - 操作：释放第一轮。
   - 预期：正在执行的设备最多保留一轮后续维护，两个相同排队联系合并；不同设备仍得到一轮；全程串行。

## Integration Test

1. **已知未准入设备会报告联系但不 Online**
   - 输入：真实 Iroh 端点；成员仓储可解析远端；准入返回 false。
   - 操作：远端完成普通连接确认请求。
   - 预期：收到一个 `KnownPeerContact`；发送给远端的结果仍为拒绝；本机 reachability 仍为 Unknown；无 Online 事件。

2. **未知设备不会报告联系**
   - 输入：真实 Iroh 端点；成员仓储为空。
   - 操作：远端连接。
   - 预期：连接被拒绝；联系与 Online 两个通道都无事件。

3. **已移除设备在 Application 再检查时失败关闭**
   - 输入：Infra 曾映射的设备，但当前同步范围已经移除。
   - 操作：交付联系触发。
   - 预期：不发送历史；维护结果保持稳定失败或延期分类，不授予权限。

## Regression Test

1. 已准入设备入站仍只发布一次 Online，普通重复入站不重复广播 Online。
2. 现有 PeerOnline 维护仍执行原完整顺序，剪贴板与成员更新投递行为不变。
3. 暂停期间不开始联系驱动的网络工作；恢复后运行原 Resume 全轮。
4. `cargo test -p uc-application --lib --locked space::membership::`。
5. `cargo test -p uc-infra --lib --locked peer_reachability -- --test-threads=1`。
6. 第一切片接线后运行 `cargo check --workspace --all-targets --locked` 与仓库交付门禁。
7. 第三切片在 Linux 串行运行 `bash scripts/testing/run-connection-recovery-e2e.sh --suite network --repeat 3`。

# 9. Acceptance Criteria

* [x] 持久退避尚余五分钟时，已知设备联系无需推进时间即可开始定向成员确认。
* [x] 联系触发只处理目标设备，并由既有成员维护队列串行、合并。
* [x] 已知但未准入的入站连接产生联系事件，但保持拒绝且不发布 Online。
* [x] 未知、无法解析或已经移除的设备不获得成员历史、地址写入、准入或 Online。
* [x] 本次不新增 Engine 稳定公开操作、设备间协议字段或持久化格式。
* [x] 稳定地址切片只保存 NodeId 与已验证 relay，不保存动态直连 IP/端口。
* [x] 三设备验收能够独立证明旧端口失效、发现被阻断、等待方不能主动发起且换址设备主动联系。
* [x] 从换址设备开始联系起 20 秒内双方 Online，且此前没有调用手动刷新、宿主机会通知或正文发送。
* [x] Online 后双向正文传输成功；同一场景连续三次通过。
* [x] 本机自动检查全部通过；未执行的 Windows 实机与真实公网验收明确记录为跳过。

# 10. Risks and Trade-offs

- **额外联系可能增加一次成员交换**：只对当前成员、只在真实入站联系时触发，并由现有队列合并。相比全局降低退避，它不会持续唤醒所有离线设备。
- **成员仓储与当前历史范围短暂不一致**：Infra 只提供候选身份；Application 在发送完整历史前再次核对当前范围，保持事实来源唯一。
- **广播事件允许丢失**：这是刻意选择。它只缩短恢复时间，不承担最终正确性；持久欠账和周期维护仍负责最终恢复。
- **不复用 PeerOnline**：会增加一个内部触发类型和观测分类，但避免把“正在联系”错误表达成“已准入在线”。
- **不直接保存入站源地址**：会推迟地址自愈切片，但避免把 NAT 临时端口或伪造候选写成长期事实。
- **真实三设备测试比单元测试慢**：只在独立进程网络套件串行执行；快速测试负责每次提交及时发现业务规则回归。
- **替代方案：全局缩短五分钟退避**：拒绝。它增加所有离线设备的后台负载，仍不能利用已经出现的可信机会。
- **替代方案：发现新地址直接触发成员同步**：拒绝。地址发现不是身份认证，不能安全地决定完整成员历史的接收者。
- **替代方案：Engine 收到事件后调用恢复步骤**：拒绝。会让组装层掌握业务顺序，破坏成员维护的唯一责任。

# 11. Open Questions

地址切片已经确认 Iroh 的远端信息会区分当前正在使用和未使用的地址。本实现只接收当前正在使用的 relay；若本次连接
没有这种 relay，则不写回，也不从普通路径快照推断长期地址。

Windows 实机验收还需要可控的防火墙或隔离网络环境，以证明公开发现为空而不是偶然未返回；在该环境执行前只能登记为跳过。

## 实施进度

- [x] 2026-09-17：完成日志与代码路径诊断，确认慢点来自旧直连地址、发现为空和成员历史最长退避的组合。
- [x] 2026-09-17：确定三个验收边界：成员确认、入站身份联系、公开 Online 与双向传输。
- [x] Slice 1：可信联系立即触发定向成员确认。
- [x] Slice 2：稳定 relay 地址提示写回。
- [x] Slice 3：三设备独立进程可重复验收。

### 2026-09-17 第一切片验证

- 成员维护与同步：125 项通过，1 项要求独占日志捕获的测试按既有约定跳过。
- 连接处理：38 项通过；覆盖已知未准入、未知、正常已准入和重复连接。
- 观测合同：51 项通过。
- 仓库级编译、格式、Rust 规范、架构边界、隐私合同与差异检查全部通过。
- Windows 实机、真实公网和三设备独立进程场景尚未执行，均登记为跳过而非通过。

### 2026-09-17 第二切片验证

- 只有 Iroh 标记为本次正在使用的 relay 会形成长期提示；动态 IP、端口和未使用 relay 均被排除。
- 当前成员的历史同步通过完整业务校验并提交成功后才尽力写回；普通传输成功、无效业务回复和受限投递均不会写回。
- 新增应用流程验收，直接证明成功同步只刷新一次、已收到但无效的回复一次也不刷新；地址筛选、保存时间和失败降级继续由 Infra 验收覆盖。
- 地址刷新成为成员同步的必需依赖，生产与测试共用唯一完整构造入口；不提供仅测试可见的便捷构造或默认空实现。
- 真实加密地址仓储在重新构造后能读回相同的 NodeId、relay 与观察时间，密文边界沿用既有仓储合同。

### 2026-09-17 第三切片验证

- Linux 独立网络环境连续三次通过；自动恢复分别用时 1463、1420、1436 毫秒，均低于 20 秒上限。
- 每轮都由独立端口检查证明旧端口关闭、新端口监听；防火墙计数证明本地发现已阻断，且等待方不能主动发起。
- 恢复前没有调用手动刷新、宿主机会通知或正文发送；恢复后双向正文传输均成功。
- 证据导出已确认清理完成、未包含测试正文，运行结果无失败。
- Windows 实机与真实公网验收未执行，继续登记为跳过。
