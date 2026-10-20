# 原生测试迁移地图

旧 native adapter 的业务实现和内部状态表断言已删除。验收从完整 owner、Uni、真实网络与既有 coordinator 进入，不为保留旧测试暴露内部接口。

| 行为 | 当前证据入口 |
| --- | --- |
| 多成员长期在线、v1 首轮保留、未知入站限额 | transport/tests.rs：six_members、v1_first_round、seventeen_unknown_connections |
| 本机读取故障的显示、来源和隐私 | transport/tests.rs：local_authority_read_failure、opaque_observation_feedback；原生 tests.rs：local_authority_failure；真实 Profile membership_record/tests.rs |
| 撤销、移除重加、旧入站迟到结果 | transport/tests.rs：forget_and_readd、forgotten_inbound；外部 peer_admission_identity_resolution 和 inbound_peer_rejection_diagnostics |
| 取消后重用健康连接与真实资源结算 | transport/tests.rs：cancelled_network_verification、caller_deadline；Uni connectivity/tests.rs；coordinator 四个慢 scope 读取回归 |
| 第三条已准入入站按成功顺序替换 | transport/tests.rs：third_inbound；原生 tests.rs：third_admitted_inbound |
| 双向 v2、真实拒绝优先、每连接两条入站活性流 | 原生 tests.rs：both_directions、genuine_rejection、genuine_remote_rejection、third_liveness_stream |
| 暂停、关闭、反复 activate、typed join 故障 | transport/tests.rs：repeated_activate、worker_join_failure；既有 coordinator 生命周期和刷新等待者回归 |
| 最新成功、旧失败、关闭及共享刷新交错 | peer_connections/tests.rs：fresh_online、incoming_success、manual_refresh、removed_target、relay_recovery 系列 |
| 实际 ALPN、路径、传输关闭、恢复与双向内容 | 默认 run-peer-reachability-connectivity.sh；原 run-connection-recovery-e2e.sh 全场景 |

整合最新 main 后：oct20-final-related.log 的 109 项相关回归（含原生 7 项与真实观测 5 项）全部通过；oct20-final-workspace-with-incoming.log 的完整工作区 4303 passed、27 skipped，包含新增原生入站故障控制。默认网络矩阵 11 格全部通过；其余回归和平台状态以执行计划最终实际证据为准。
