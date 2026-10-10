# 1. Overview

状态：实施中。Step 1、Step 3 的生产改动已完成，本机端到端测试已补，Step 2 与 Step 4 的环境证据待执行。基线：`193e9d27f0be4157a28d9bc065507e753d8188aa`；依赖 iroh 1.3.0。首次自行新增的 Infra 测试已撤回，其失败不能当作已复现产品缺陷的证据。

本规格承接连接实现调研，按顺序处理地址发布、网络恢复验收、路径观测和拨号成本。当前 `AddrFilter` 收到的是本机待发布地址，却用“对端私网地址与本机同子网”判断删掉公网候选；本机私网地址自然符合条件，导致跨网设备缺少可用候选。过滤器还捕获启动时子网，使切网被错误描述为必须重启。

另一处问题是同步完成后使用 `remote_info` 的 Active 地址推断本次发送路径。Active 表示已打开路径，不能证明这个内容连接选中了该路径。已有连接观察器使用 `Connection::paths().is_selected()`，应复用该语义。

# 2. Goals

- 公网 IPv4、IPv6、私网地址和 relay 同时存在时，发布与 ticket 都仅应用既有虚拟网卡策略。
- 同一 endpoint 更新地址后继续发布新候选，不依赖启动子网快照。
- 恢复验收检查真实双向内容和接收确认，不能只检查在线状态。
- 同步投递的 transport 来自该次 Connection 的选中路径；混合或缺失快照报告 Unknown。
- 使用已有故障宿主对比拨号策略；没有网络故障实测依据前保留现有预算和重试策略。

# 3. Non-Goals

- 不自行实现 ICE、打洞或新增 watchdog，不修改 LAN 兼容线与用户 relay 选择。
- 不修改绑定或稳定 Engine API，不为观测公开业务内部步骤。
- 不重复实现已合并的配对当前地址更新，也不接管 mDNS 多网卡工作的在途修改。
- 不把本机、容器、namespace 验收描述为手机或实体设备睡眠唤醒验收。

# 4. Current Architecture Context

Component: 地址过滤
Path: `crates/uc-infra-p2p/src/network/iroh/addr_filter.rs`、`node.rs`
Responsibility: Infra 决定本机候选发布与 ticket 中的虚拟网卡排除规则。
Relationship: iroh 的地址查找服务在 publish 时调用过滤器；不是对端拨号候选判定器。

Component: 生命周期与成员连接恢复
Path: `crates/uc-application/src/connectivity/`、`crates/uc-infra-p2p/src/network/iroh/net_recovery.rs`
Responsibility: Application 统一承担恢复机会、成功失败、暂停取消与重试；Infra 提供网络刷新能力。
Relationship: Engine 组装既有能力；详见[统一生命周期计划](2026-09-12-unified-runtime-lifecycle.md)。

Component: 内容投递与路径观察
Path: `crates/uc-infra-p2p/src/network/iroh/clipboard_dispatch_adapter.rs`、`clipboard_receiver_adapter.rs`、`conn_path.rs`、`observed_connections.rs`
Responsibility: Infra 用真实连接执行帧写入与回执读取，并观察连接路径。
Relationship: Application 的完整同步动作消费既有 DispatchReport；设备列表查询是 peer 汇总，不能替代单次连接证据。

Component: 故障验收宿主
Path: `scripts/testing/run-connection-recovery-e2e.sh`、`tests/hosts/connectivity/`
Responsibility: 驱动两个真实 Engine、故障注入和内容确认，产出源码身份与验收工件。
Relationship: Linux namespace 网络故障与本机回归分别验收，不能互相替代。

# 5. Proposed Design

## Components

地址过滤删掉子网枚举和 hairpin 推断，只保留 `is_virtual_nic_ip`；原因是地址重叠不能证明可达，发布阶段也没有对端上下文。继续由 iroh 选择路径，relay 保留。`if-addrs` 仍用于配对 mDNS，不删除依赖。

`conn_path` 增加私有的连接快照分类入口，使用该 Connection 的 selected 路径。设备列表保留 peer 汇总入口；同时发现 Direct 与 Relay 时返回 Unknown，不能声明 Direct 优先就是实际发送路径。

恢复继续由现有完整负责人执行。本项首先执行现有真实 Engine 回归；发现缺陷后才在所属负责人修复，不增加另一套恢复控制器。

## Data Model

不新增持久字段。地址集合与路径快照仅在运行期存活；证据只包含固定案例类别、耗时、确认结果和源码身份，不写用户地址或内容。公网候选测试使用固定合成地址，真实收发使用本机 endpoint。

## API / Interface

- `apply_addr_filter(addrs, allow_overlay) -> Cow<Vec<TransportAddr>>`：移除子网参数，无过滤返回借用。
- 私有连接路径查询接收 `&Connection`，返回既有 `ConnectionPath`；无 selected 或连接已关闭返回 Unknown。
- 保持 DispatchReport、连接查询 port 与 Engine facade 的现有形状。
- 下层错误继续保留 source chain，不新增字符串化错误。恢复失败由既有 owner 重试，地址过滤没有重试职责。

## Workflow

1. endpoint 产生本机候选，过滤虚拟网卡候选，地址查找服务发布完整剩余集合。
2. 系统网络变化由现有 owner 触发刷新与恢复；无需重新创建过滤器获取子网。
3. 完整同步动作调用一次 dispatch，Infra 拨号、写帧、读取回执。
4. 在连接仍存活时读取 selected 路径，结算既有结果；快照只表示采样时刻，不证明全程没有迁移。
5. 测试使用内容确认验证恢复，产出结果与复现命令；延迟、失败和取消分别记录。

# 6. Implementation Plan

Step 1:
File: 地址过滤及 node 的端到端测试。
Change: 先编写真实 endpoint 发布、动态更新与回执案例，再删除错误 heuristic；保留并适配既有虚拟网卡测试，删除验证旧错误契约的测试。
Risk: 公网路径重新参与选择，实际 RTT 变化需后续网络实测。

Step 2:
File: 现有 Engine 成员连接 E2E、连接恢复脚本。
Change: 运行当前恢复、失败后连接保留与真实内容场景；依据工件决定是否需要改动 owner。
Risk: Linux 故障注入与实体唤醒需要相应环境，未执行项保留待验收。

Step 3:
File: conn_path、clipboard_dispatch_adapter、clipboard_receiver_adapter。
Change: 先写连接选择及发送确认 E2E，再让发送与接收的内容报告使用本次连接，修正 peer 汇总的混合路径误判。
Risk: 路径可能在帧发送期间迁移，结果仅声明结算时快照。

Step 4:
File: 连接恢复验收证据及本计划。
Change: 对比现有三次错峰拨号和单次拨号在 direct、relay、高延迟、丢包、取消中的成功率、确认耗时和尝试数。只在完整网络矩阵支持时改变生产策略；缺环境时明确保留待验收，不用本机短 RTT 推导网络预算。
Risk: 缩短预算可能降低跨网成功率；多次拨号可能增加连接成本。

# 7. Edge Cases

Scenario: 空地址、纯 relay 或仅 IPv6。
Expected behavior: 空集合不制造地址；relay 与有效 IPv6 保留。
Implementation: 只对 IP 执行既有虚拟网卡谓词。

Scenario: 切网后旧地址与新地址同时出现。
Expected behavior: 不因旧子网匹配删公网候选；刷新由 iroh 与现有 owner 执行。
Implementation: 删除过滤器中的子网状态。

Scenario: overlay 关闭或开启，fake-IP 同时存在。
Expected behavior: 开关只影响 overlay；fake-IP 和 IPv4 link-local 始终排除。
Implementation: 共用既有谓词，ticket 与发布保持一致。

Scenario: 多连接、Direct 与 Relay 同时打开或没有 selected 路径。
Expected behavior: 单次投递看自己的 Connection；peer 混合汇总与缺失选择返回 Unknown。
Implementation: 不以打开路径替代选中路径。

Scenario: 并发恢复、暂停、取消、断网或回执丢失。
Expected behavior: 现有 owner 串行化机会并停止撤销后的工作；无回执不能算内容到达。
Implementation: 复用生命周期与回执验收，不增加旁路。

Scenario: 地址持久数据损坏、旧版设备。
Expected behavior: 继续使用既有错误和兼容拒绝；本规格不修改 wire 或存储格式。
Implementation: 保留现有 resolver 与协议边界。

# 8. Testing Strategy

隔离网络能力前先列出失败方式：发布误删公网、虚拟网卡泄漏、动态更新未发布、ticket 不一致、连接未建立、内容或回执错误、路径分类误报、超时未退出、资源未回收。以这些断言驱动测试，先写再实现。

## Unit Test

遵守仓库要求，不在编写代码后新增单元测试；仅适配既有测试的私有签名和删除被废止契约的测试。

## Integration Test

主要采用端到端：真实 iroh endpoint 经生产过滤器发布固定公网、私网、overlay 候选，更新地址，再由另一个 endpoint 建立连接、传输固定测试帧并收到确认。工件记录发布断言、内容确认、selected 分类、耗时与复现命令。公网地址发布通过不表示公网可达。

## Regression Test

执行现有真实内容投递与成员恢复 E2E。Linux 网络套件覆盖 direct 故障、relay、恢复机会与旧协议；macOS 本机结果不算该矩阵通过。最后执行根 AGENTS 的格式、编译与架构门禁。

# 9. Acceptance Criteria

* [x] 发布与 ticket 保留公网 IPv4、有效 IPv6、私网与 relay（经生产过滤器与 iroh `FilteredAddressLookup` 发布路径的固定合成地址测试）。
* [x] 虚拟网卡开关行为不变，启动子网枚举已移除（既有虚拟网卡测试通过；`uc-infra-p2p` 全量 267 项通过）。
* [ ] 同一 endpoint 动态地址更新通过发布断言与真实连接回执（发布断言已覆盖同一过滤器的前后两组地址；iroh 不允许外部注入地址，真实 endpoint 切网后的回执待网络矩阵）。
* [x] 成员恢复本机 E2E 通过且有可复现工件（`bash scripts/testing/run-connection-recovery-e2e.sh --suite local`，macOS 本机 96 项通过；该套件无网络故障注入，不替代 Linux 矩阵）。
* [x] 单次投递与接收使用本次连接 selected 路径；peer 混合快照返回 Unknown（实现完成，真实双 endpoint 投递测试覆盖两端 Direct；peer 汇总分类改为作用于私有的 `KnownAddr` 最小视图，Direct+Relay 混合返回 Unknown 已由合成数据测试覆盖）。
* [x] 相关回归与仓库交付门禁通过（workspace check、fmt、`check-rust-style`、`check-engine-repository`、`git diff --check`；未运行 Linux 网络矩阵）。
* [ ] Linux 网络矩阵与拨号策略对比有证据，或明确保留未执行状态。
* [ ] 实体设备切网、睡眠唤醒有双向内容证据，或明确保留未执行状态。

# 10. Risks and Trade-offs

删除 heuristic 可能恢复原来被抑制的 hairpin 路径，但发布本机公网地址是跨网连接的必要条件，不能用缺乏对端上下文的猜测代替。路径选择交给成熟的 iroh；若真实测量仍显示路径振荡，再用选中路径事件与 RTT 证据定位。

Unknown 比未经证实的 Direct 更保守，可能改变设备列表显示；保持稳定枚举，无接口迁移成本。端到端运行成本高于纯函数测试，但能验证依赖语义、发布与真实回执。

# 11. Open Questions

- 实体设备切网、睡眠唤醒后的恢复时间与路径迁移是否满足实际产品要求？本机不能回答。
- 三次完整拨号相对于 iroh 自身路径竞争的收益是否抵得过额外成本？必须有 relay 高延迟与丢包对照数据。
- 生产出现的 hairpin 振荡是否仍存在于当前 iroh 1.3.0？旧日志不能替代当前版本测量。
