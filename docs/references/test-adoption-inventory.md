# Engine 测试采用清单

更新时间：2026-09-22。本清单用于安排渐进采用，不是批量迁移授权。旧测试和 `cargo test` 入口继续保持权威。

## 已采用

| 覆盖 | 路径 | 分组 | 当前证据 |
| --- | --- | --- | --- |
| testkit 自身、成功/受控失败、并行工件、进程超时 | `tests/uc-testkit/` | fast | JSON、摘要、JUnit |
| 五个确定性成员恢复场景 | `crates/uc-application/src/space/` 对应领域测试 | fast/evidence | 固定 seed、公开终态、阶段与复现命令 |
| 加入方完整配对作者入口 | `space/admission/protocol/tests/` | evidence | 专用 fixture 位于 `tests/support/`；一次真实负责人调用返回 Active + final confirmation，不冒充双方 Engine 链路 |
| 两节点成员历史分区与恢复 | `crates/uc-application/src/space/membership/testing/virtual_membership_network.rs` | fast/evidence | 真实 Application endpoint、frame 预算、脱敏 trace；仅为配对后收敛基础 |
| 文件传输完成生命周期 | `crates/uc-application/tests/file_transfer.rs` | fast/evidence | 公开 facade 从登记、进度到唯一 Completed；真实 bytes 仍由 E02 证明 |
| 真实 Engine 完整配对、文字与文件传输 | `scripts/testing/connection-recovery-network.mjs` 的 `E01`/`E02` | real-network/nightly | 独立进程、profile、身份、端口与 namespace；公开 setup/eligibility/peer/history/ReadEntryFile 终态、exact bytes 和 cleanup 证据 |
| rendezvous provider 正常/无效/暂时失败 | `crates/uc-infra/src/rendezvous/invitation_adapter/tests/provider_dependency_evidence.rs` | persistence-provider/evidence | 私有 adapter 场景与业务实现分目录；产品、环境、清理分类 |
| profile storage upgrade 与崩溃恢复 | `crates/uc-infra/tests/profile_storage_upgrade.rs`、`profile_storage_upgrade_crash.rs` | process/evidence/nightly | synthetic migration、子进程退出、持久恢复、资源回收；alpha.5 外部 fixture 单列未验证 |

## 首批五类双线状态

| 类别 | 快速确定性线 | 真实环境线 |
| --- | --- | --- |
| 配对 | 部分：加入方完整 fixture + 成员恢复五场景 + 配对后成员历史网络；无双方完整快速 topology | E01 当前提交 direct/relay 已验证 |
| 文字/文件 | 部分：文件接收完成生命周期已进入 fast；无快速网络或 exact bytes | E02 text/file 当前提交 direct/relay 已验证 exact value/bytes |
| 重连 | 部分：成员消息 partition/heal 后恢复；不等于 Engine transport 重连 | E03/E04/E06/E10/E13 当前 runner 回归通过 |
| 重启 | Application 持久准入重建已覆盖 | E11/E12 当前 runner 回归通过 |
| 旧资料升级 | focused migration/process 18 项本地 5.546 秒 | workflow 已接入但默认分支未生效；alpha.5 fixture 未验证 |

速度实测：快速成员五场景一次 `0.180s`，100 轮 `58.98s`；新 joiner pairing fixture 单次 `0.040s`，与旧 settled
测试双轨 20 轮在 11 秒内全部通过；两节点成员历史场景单次 `0.053s`。文件传输完成场景单次 `0.033s`，与旧
幂等完成测试双轨 20 轮在 11 秒内全部通过。当前 fast 15/15、`0.472s`；evidence 17/17、`2.019s`。当前真实工件的 scenario records 累计为 direct `672.000s`、relay `168.591s`、known-peer
`1.775s`、legacy `0.122s`，相关 network job 墙钟约 66 分 54 秒。E01/E02 单项均低于 60 秒；新的分 mode
准备、场景和清理 30 分钟目标仍无独立 nightly 样本，不登记达标。

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

034 的快速切片已覆盖配对完成后的成员历史分区与恢复；真实环境线复用既有 runner，将完整邀请/准入、双向 exact
text 和真实文件 bytes 登记为 E01/E02。重连复用 E03/E04/E06/E10/E13，重启复用 E11/E12；旧资料升级 nightly
复用现有 synthetic migration 与 crash recovery tests。alpha.5 完整外部 fixture、真实外网和设备继续单列未验证。
后续只补真实缺口，不因路线图存在而批量重写简单测试。t-0010 等活跃修复中的测试在其工作结束前不进入迁移清单。

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
