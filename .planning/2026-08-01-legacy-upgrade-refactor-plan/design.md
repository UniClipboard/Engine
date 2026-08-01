# 自动安全升级重构设计

## 结论

保留当前行为和网络格式，但把实现重构为一个深模块：`AutomaticLegacyUpgrade`。

这个模块独自负责：

- 启动后的发现窗口；
- 设备上线触发；
- 定时重试；
- 多设备选主与冲突收敛；
- 已配对成员校验；
- 发起加入、接纳成员和安装结果；
- 丢失回复后的安全重放；
- 重启后的继续执行；
- 对外返回当前结果。

调用方只负责安装模块并在关闭时停止它。网络接入只把已经解析出的远端身份和消息交给模块，不再参与成员规则或流程判断。

这次重构必须替换旧结构，不能在当前 `LegacyUpgradeCoordinator` 外再包一层后保留所有旧接口。

## 完成标准

1. `uc-engine` 的主组装文件只出现一次“安装自动升级”和一次“关闭自动升级”，不再出现发现窗口、重试周期或 `allow_isolated_bootstrap`。
2. 应用层对组装方只暴露一个 `AutomaticLegacyUpgrade` 接口；该接口只接收运行事件和已认证的远端请求。
3. 核心层只保留保护状态、组标识、收敛规则和流程结果，不计算摘要、证明或网络帧。
4. 删除 `LegacyUpgradeRepositoryPort`；未完成加入的存储接口降为基础设施内部接缝。
5. 删除 `PreparedLegacyUpgradeRequest` 这类把网络请求和私密加入状态绑在一起的核心公开类型，改为模块内部的不透明尝试句柄。
6. `SpaceKeyMaterial` 不再依赖 `LegacyUpgradeRequest` 或 `LegacyUpgradeAdmission`；安全重放使用不透明记录，并继续与群组材料原子保存。
7. `DefaultSpaceAccessAdapter` 不再实现完整自动升级流程；升级实现位于专用目录，并只复用已有的群组加入、建组和材料操作。
8. 旧 `Ready` 数据缺少保护组标识时可以安全回填并继续运行，不被误判为损坏。
9. 当前的双设备、三设备、并发建组、断网、丢失回复、重启、未知成员和密文落盘行为全部保留。
10. 测试从负责模块的接口验证行为，不再为每个场景重复实现七个内部步骤。

## 目标接口

### 外部接口

概念形状如下，名称可在实现时按仓库惯例微调：

```rust
pub struct AutomaticLegacyUpgrade { /* private */ }

impl AutomaticLegacyUpgrade {
    pub fn start(
        self: Arc<Self>,
        presence_events: broadcast::Receiver<MemberPresenceEvent>,
    ) -> AutomaticLegacyUpgradeRuntime;

    pub async fn handle_authenticated_peer(
        &self,
        peer: &DeviceId,
        request: LegacyUpgradeRequest,
    ) -> Result<LegacyUpgradeResponse, LegacyUpgradeError>;
}

pub struct AutomaticLegacyUpgradeRuntime { /* private task */ }

impl AutomaticLegacyUpgradeRuntime {
    pub async fn shutdown(self);
}
```

真正的组装方不直接构造这些细节。`uc-engine/src/assembly/legacy_upgrade.rs` 提供一个安装函数，完成应用模块、网络协议和运行任务的连接，只返回运行句柄。

### 内部安全接缝

用一个 `LegacyProtectionPort` 替换当前的七步安全端口和三步仓储端口。它只表达四个有完整含义的动作：

1. 读取本地保护快照；
2. 开始一次面向指定设备的尝试，并自动复用持久化的未完成状态；
3. 检查一个入站尝试，内部完成证明校验和安全重放查询；
4. 执行应用层已经选定的命令：建组、接纳或加入，并原子保存结果。

核心层的纯规则仍然决定 `等待 / 建组 / 接纳 / 加入 / 无动作 / 拒绝`。基础设施只执行应用层选定的命令，不拥有业务决策。

### 合法保留的远端接缝

以下两个单方法接缝保留：

- 向一个设备发起交换；
- 接收已经认证的设备请求。

它们是真实网络边界，同时有 Iroh 实现和内存测试实现，符合保留条件。网络编码、大小限制、超时和身份指纹解析继续留在 Iroh 适配器内。

## 职责归属

| 位置 | 保留 | 移走或删除 |
|------|------|------------|
| `uc-core` | 保护状态、组标识、收敛规则、命令和结果 | 摘要算法、证明计算、私密加入状态、仓储步骤 |
| `uc-application` | 完整升级流程、成员规则、发现窗口、重试策略、运行状态 | 逐步指挥加密、缓存和数据库操作 |
| `uc-infra/security` | 证明、群组材料、加入、接纳、原子执行 | 选主、重试和成员业务判断 |
| `uc-infra/db` | 加密保存未完成尝试和重放记录 | 面向应用层的公开仓储接口 |
| `uc-infra/network` | 编解码、连接、超时、远端身份解析 | 再次判断成员是否有效 |
| `uc-engine` | 一次安装、持有运行句柄、关闭 | 发现窗口、定时策略和流程分支 |

## 目标文件结构

```text
crates/uc-core/src/membership/
  legacy_protection.rs        # 状态、组标识、纯收敛规则
  legacy_upgrade_contract.rs  # 最小跨层消息、命令、结果和远端端口

crates/uc-application/src/facade/legacy_upgrade/
  mod.rs                      # AutomaticLegacyUpgrade 的小接口
  reconcile.rs                # 单次出站收敛
  inbound.rs                  # 已认证请求处理
  runtime.rs                  # 发现窗口、上线触发、重试和关闭

crates/uc-infra/src/security/legacy_upgrade/
  mod.rs                      # LegacyProtectionPort 实现
  proof.rs                    # 标识、证明和请求摘要
  transition.rs               # 建组、接纳、加入的原子执行

crates/uc-infra/src/db/repositories/space_security_store/
  legacy_upgrade.rs           # 基础设施内部的加密尝试存储

crates/uc-infra/src/network/iroh/
  legacy_upgrade_adapter.rs   # 保留现有 wire v1 与 Iroh 行为

crates/uc-engine/src/assembly/
  legacy_upgrade.rs           # 唯一安装入口
```

这是一张归属图，不要求机械地创建每个文件。若某个文件只有转发作用，应合并回所属模块。

## 数据与兼容设计

### 旧 `Ready` 记录

新增字段使用默认空值只能保证反序列化成功，不能算完成迁移。读取旧 `Ready` 记录时：

1. 识别“群组材料有效但缺少保护组标识”；
2. 生成新的本地保护组标识；
3. 在同一加密材料中持久化；
4. 再向其他成员进行正常收敛；
5. 多台旧设备若分别生成临时标识，沿用现有字典序规则选出唯一结果。

不得把 OpenMLS 当前的 `group_id` 直接当作保护组标识，因为它等于 Space 标识，无法区分并发形成的临时组。

### 加入方未完成状态

保留当前加密表和不可逆设备查找值。仓储接口移入基础设施内部，调用方只看到“不透明尝试句柄”，不能读取私密群组状态。

### 发起方回复重放

保留“接纳结果与新群组材料同一次保存”的原子性，但把记录改成：

- 不透明请求编号；
- 接收设备；
- 既有 `GroupAdmission` 所需的数据。

通用密钥材料不再知道网络请求类型，也不负责计算请求编号。

### 网络兼容

本次重构不修改 ALPN、wire version、字段含义或大小限制。先保持字节兼容，待结构稳定后再单独评估协议版本升级。

## 分阶段实施

### 阶段 0：锁定行为与修复兼容前提

- 增加旧 `Ready` 记录缺少保护组标识的红灯测试。
- 固定当前行为矩阵：单设备、两台旧设备、一新一旧、三设备、并发临时组、离线、回复丢失、重启、未知成员。
- 确认当前 wire v1 的编码样本，防止重构意外改变网络格式。
- 不移动生产代码，先取得可比较基线。

通过条件：兼容缺口有准确红灯，其余现有行为全绿。

### 阶段 1：收紧核心模型

- 把保护状态与收敛规则从协议请求中分离。
- 把证明摘要和请求编号计算移到基础设施。
- 复用 `GroupAdmission`，删除重复的接纳业务模型。
- 统一 `Ready` 转换，保护组标识必须显式传入。
- 把安全重放改为不透明记录，移除撤销模型对升级请求的依赖。

通过条件：核心测试只验证状态和规则；网络和密码学测试不再位于核心包。

### 阶段 2：建立专用安全模块

- 新建 `uc-infra/src/security/legacy_upgrade/`。
- 从通用会话和空间访问适配器迁出本次新增的证明、准备、安装、接纳与合并流程。
- 用一个深的 `LegacyProtectionPort` 替换当前公开的安全端口和仓储端口。
- 仓储接缝保持基础设施私有；真实数据库和内存实现只供该模块使用。
- 完成旧 `Ready` 记录的加密回填。

通过条件：`DefaultSpaceAccessAdapter` 不再直接实现自动升级端口；重启、密文落盘和原子重放测试通过。

### 阶段 3：深化应用模块

- 把传输依赖放入 `AutomaticLegacyUpgrade` 构造依赖，不再每次调用传入。
- 把发现窗口、重试周期和上线触发移入 `runtime.rs`。
- 删除 `allow_isolated_bootstrap` 参数；是否允许独立建组由模块根据自身启动时间判断。
- 入站和出站共用同一份成员校验、状态读取和结果映射。
- 运行结果使用“当前状态 + 本轮是否改变”，避免 `Created`、`Joined` 等过程细节成为外部契约。

通过条件：应用模块的场景测试只依赖一个保护端口和一个远端传输端口，测试正文集中表达设备行为。

### 阶段 4：收拢网络和组装

- 保留 Iroh 编解码与协议安装行为。
- 网络层只解析并认证远端身份；成员是否仍有效只在应用模块判断一次。
- 新建 `assembly/legacy_upgrade.rs`，完成依赖连接和运行任务创建。
- `sync_engine.rs` 只保存运行句柄并在关闭时停止。
- 从通用 `SpaceAccessPorts` 移除可选的升级字段，改为只在功能安装处构造专用依赖。

通过条件：主组装文件没有升级策略常量、循环或流程分支；不需要修改无关功能测试的装配。

### 阶段 5：替换测试并删除旧形状

- 保留少量纯收敛规则测试。
- 用一个场景世界替换应用测试中重复的手写替身。
- 保留真实数据库加密、重启和原子性测试。
- 为网络适配器补齐 wire 样本、未知身份、超时、过大消息和坏版本测试。
- 删除旧安全端口、仓储端口、重复接纳类型、旧协调者入口和对应浅层测试。
- 全仓搜索旧名称，确认没有兼容壳或双实现残留。

通过条件：删除旧模块后复杂度不会散回调用方；现有行为矩阵全部通过。

## 建议提交顺序

1. `test(security): cover legacy ready identity backfill`
2. `refactor(core): isolate legacy protection policy`
3. `refactor(infra): deepen legacy upgrade security adapter`
4. `refactor(application): own automatic upgrade lifecycle`
5. `refactor(engine): centralize legacy upgrade assembly`
6. `test(upgrade): replace step mocks with scenarios`
7. `chore(upgrade): remove obsolete upgrade seams`

每个提交都必须独立编译并保持已有行为；不要先大搬文件再集中修复。

## 验证矩阵

### 每阶段

```bash
cargo test -p uc-core --test legacy_upgrade --locked
cargo test -p uc-application --test legacy_upgrade --locked
cargo test -p uc-infra legacy_upgrade --lib --locked
cargo fmt --all -- --check
git diff --check
```

### 最终

```bash
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo test -p uc-core --locked
cargo test -p uc-application --locked
cargo test -p uc-infra --locked
cargo test -p uc-engine --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

设备验收若未执行，必须记为“跳过”。本次仅重构内部结构，不需要改变产品界面或发布协议。

## 明确禁止

- 不在旧协调者外新增一层包装并保留旧接口。
- 不为了缩短文件而创建只有转发作用的新文件或新端口。
- 不同时保留旧流程和新流程进行长期双跑。
- 不顺手重构群组更新、剪贴板同步或其他后台任务。
- 不修改当前并行存在的剪贴板接收策略变更。
- 不以测试通过为由跳过旧 `Ready` 数据兼容验证。
