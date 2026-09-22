# Engine 测试采用清单

更新时间：2026-09-22。本清单用于安排渐进采用，不是批量迁移授权。旧测试和 `cargo test` 入口继续保持权威。

## 已采用

| 覆盖 | 路径 | 分组 | 当前证据 |
| --- | --- | --- | --- |
| testkit 自身、成功/受控失败、并行工件、进程超时 | `tests/uc-testkit/` | fast | JSON、摘要、JUnit |
| 五个确定性成员恢复场景 | `crates/uc-application/src/space/` 对应领域测试 | fast/evidence | 固定 seed、公开终态、阶段与复现命令 |
| 两节点成员历史分区与恢复 | `crates/uc-application/src/space/membership/testing/virtual_membership_network.rs` | fast/evidence | 真实 Application endpoint、frame 预算、脱敏 trace；仅为配对后收敛基础 |
| rendezvous provider 正常/无效/暂时失败 | `crates/uc-infra/src/rendezvous/invitation_adapter.rs` | persistence-provider/evidence | 产品、环境、清理分类 |
| profile storage upgrade 崩溃恢复 | `crates/uc-infra/tests/profile_storage_upgrade_crash.rs` | process/evidence | 子进程退出、持久恢复、资源回收 |

## 保留原样

以下测试通常短小、确定、没有跨阶段资源生命周期，套 testkit 只会增加噪音：

- `crates/uc-core/src/**/tests/` 与 `crates/uc-core/tests/` 的纯规则、值对象、序列化和状态转换。
- `crates/uc-application/src/**/tests.rs` 中只使用内存 port、直接调用单一负责人并同步断言结果的测试。
- `crates/uc-infra/src/security/**/tests.rs` 中单一 codec、密码边界和确定性持久映射测试。
- `crates/uc-engine/tests/public_contract.rs`、`dependency_firewall.rs` 等稳定公开合同检查。
- `crates/uc-observability-contract/tests/` 的纯 schema/分类合同。

## 优先评估采用

长期首批业务覆盖固定为配对、文字和文件传输、断线重连、重启恢复、旧资料升级。按“失败诊断收益 / 迁移成本”
排序，每次只迁移一个代表场景并与旧入口双轨：

1. `crates/uc-observability-runtime/tests/collector_slow.rs`、`collector_unavailable.rs`、`collector_tls_failure.rs`：已有真实 loopback/provider 失败，适合统一预算、端口和环境失败证据。
2. `crates/uc-engine/tests/host_contract/startup/crash.rs` 与 `failure.rs`：包含进程/启动失败边界，适合复用有界进程与 cleanup 报告；不得改变 host contract 断言。
3. `crates/uc-infra/tests/node_lifecycle.rs`：真实 runtime 生命周期和临时资源较多，适合先选一个关闭/超时 case，不迁移整文件。
4. `crates/uc-application/tests/file_transfer/shutdown.rs`：有异步关闭与资源等待，适合事件驱动等待和阶段证据；业务流程继续由 Application 负责人拥有。
5. 其他包含独立进程、多个临时目录、端口或重复手写等待的 integration test：先用实际失败或慢测证据证明收益后再进入清单。

034 的首个多节点基础切片已覆盖配对完成后的成员历史分区与恢复，但尚未覆盖完整邀请和准入。后续仍应从上述五类
中选择已有慢测或不稳定证据最充分的一项；不因路线图存在而批量重写简单测试。t-0010 等活跃修复中的测试在其
工作结束前不进入迁移清单。

## 保留真实网络与设备入口

以下覆盖不能被内存模拟或 testkit 成功替代：

- `scripts/testing/run-connection-recovery-e2e.sh` 与 `tests/hosts/connectivity*`：Linux network namespace、真实断线和恢复。
- `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`：真实 Engine/Iroh 多节点链路；后续只下沉可确定性证明的业务规则，保留最小真实链路矩阵。
- `crates/uc-infra/tests/iroh_*_probe.rs` 与真实 Iroh node/provider 探针：保留实际 transport 合同。
- `tests/hosts/android/`、`tests/hosts/ios/`、`tests/hosts/ohos/`：绑定、安装、启动和设备行为；必须按平台分别报告。

真实环境线后续以 nightly 和 `workflow_dispatch` 单场景运行；先证明本机真实 Engine 多进程和独立资料，再进入
隔离网络。mock/provider 错误不能登记为断网通过。发布前应读取近期 nightly，并补跑受变更影响的关键场景。

## 迁移准入

只有同时满足以下条件才改造现有测试：

1. 已记录当前失败定位、并行冲突、资源泄漏或耗时问题，而不是为了统一样式。
2. 场景仍调用原完整负责人，不在 testkit 建第二套业务状态机。
3. 新旧入口双轨，最终业务断言一致。
4. 工件能新增至少一种实际需要的诊断：阶段、最后事件、资源清理、失败分类或复现命令。
5. 真实网络/设备覆盖若被缩减，必须另有真实矩阵证据和明确批准。
