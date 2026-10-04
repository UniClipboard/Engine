# 144 `uc-infra` 拆分：失败矩阵（事后对照）

**性质**：本文是 2026-10-04 收尾验收时**事后补写**的对照表，不是 issue §7 要求的"迁移前先列出"的失败矩阵，也不能代替 S0 基线。
S0 按用户指示跳过，S2–S5 期间也没有产出事前矩阵（见 [实施记录](144-uc-infra-crate-split-record.md)）。下表只把 issue §7 的
九类失败情形，对应到合并后 main `1dd4cbad` 上已有的测试和 CI 证据，并写明缺口。

**证据来源**：
- main run 37206189173（`1dd4cbad`，2026-10-04）8 个 job 全部 success：Engine tests 跑 `run-test-group.sh workspace`，加上 evidence、membership smoke、隔离网络配对冒烟；另有 Rust coverage、Engine repository checks、Upgrade compatibility smoke，以及 4 个 Connection recovery job（direct、relay、known-peer、legacy）。
- 下表列出的测试文件都包含在这次 CI 的 workspace 测试里。
- 单次 CI 通过只说明该次运行没出现失败，不代表偶发问题已经消失。

**状态**："有覆盖"表示有现成测试覆盖这类情形，并且在上述 CI 中通过；"部分覆盖"写明缺口；"未覆盖/跳过"不记为通过。

| # | 失败情形（issue §7） | 必须保持的结果 | 现有证据（测试文件） | 状态与缺口 |
| --- | --- | --- | --- | --- |
| 1 | 空 Profile、尚未解锁 | 保持既有初始化结果；不能生成明文目录或伪造 ready | `crates/uc-engine/tests/host_contract/startup.rs`、`startup/{control,failure,targets}.rs`；`crates/uc-engine/tests/plaintext_probe_script.rs` | 有覆盖。没有逐条核对"无明文目录"的断言是否覆盖了所有新 crate 的写入点 |
| 2 | 激活/读取/锁定并发 | 同一租约与代次；失败恢复快照；已撤销视图不能被重装 | `crates/uc-engine/tests/host_contract/lease.rs`、`key_loss.rs`（含 `repeated_recovery_requests_are_serialized`）；`crates/uc-infra-profile/tests/space_access_adapter_restart.rs` | 部分覆盖。"已撤销视图不能被重装"没有找到专门的跨 crate 测试；本机 `host_contract space_leave::repeated_leaves…` 稳定超时（在干净 main 上也超时，CI 通过） |
| 3 | 迁移中断、进程重启 | 旧 journal/manifest 字节可读；原负责人接续，不删除旧资料 | `crates/uc-infra-profile/tests/profile_storage_upgrade.rs`、`profile_storage_upgrade_crash.rs`；`crates/uc-engine/tests/host_contract/startup/crash.rs`；`crates/uc-engine/tests/config_migration_round_trip_e2e.rs` | 有覆盖。本机 crash E2E `interrupted_file_transfer_recovers_after_receiver_process_restart` 偶发超时，拆分前同样出现，根因在 Iroh 出站握手，未确定 |
| 4 | 密文/目录损坏、口令错误、KDF 超限 | 保持原分类和 source chain；不静默跳过或重建为新的空资料 | `crates/uc-engine/tests/host_contract/key_loss.rs`；`crates/uc-infra-profile/tests/admission_diagnostic_file.rs`、`peer_address_read_diagnostics.rs`、`space_admission_state.rs` | 有覆盖。S4 曾丢失凭据分类，S1 起日志路由失效，都已在 PR #152 修复；Linux 栈帧截断在 PR #152 修复。"KDF 超限"没有找到专门测试 |
| 5 | 网络中断/重连/身份冲突 | 成员与路由身份校验位置不变；不自动切换 LAN | `crates/uc-infra-p2p/tests/{peer_admission_identity_resolution,inbound_peer_rejection_diagnostics,lan_only_relay_mode}.rs`；`crates/uc-infra-profile/tests/inbound_peer_single_owner.rs`；`crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`；CI 的 4 个 Connection recovery job | 有覆盖。CI 的隔离网络只在 Linux 上运行；真实设备与公网环境：**跳过** |
| 6 | 大图、超大解压输出、目录文件 | 原上限、流式/压缩行为与原子发布不变 | `crates/uc-infra-content/src/{clipboard/chunked_transfer.rs,encrypted_blob_store.rs,content_protection/blob_store.rs}` 的单元测试；`crates/uc-infra-profile/tests/{directory_receive_commit_contract,directory_publish_log_contract,inbound_receive_commit_contract}.rs` | 部分覆盖。没有逐条核对"超大解压输出上限"的断言；大图实测耗时没有在拆分前后对照 |
| 7 | V1/V2/V3 资料与旧协议 | 原兼容路径仍可执行；不因 crate 改名新增格式版本 | CI 的 Upgrade compatibility smoke（`tests/upgrade-matrix`）；`crates/uc-sync-protocol/tests/golden_vectors.rs`；拆分前后 migrations（61 个）和格式/版本常量集合逐一相同 | 有覆盖。升级矩阵只跑 smoke 子集，完整矩阵：**未在本次验收中运行** |
| 8 | 多节点/重复启动/关闭 | 节点 lease、任务终止与共享安全会话归属不变 | `crates/uc-infra-p2p/tests/node_lifecycle.rs`；`crates/uc-engine/tests/host_contract/{lease,stale_callback}.rs`；uniffi `public_contract` 的 `lifecycle_targets` | 部分覆盖。本机 `lifecycle_targets.rs:73` 偶发暂停超时，根因**未找到**；第 93 行的阻塞问题已由 PR #153 修复（修复前 3/8 失败，修复后 0/32） |
| 9 | cfg/platform/test-util | 默认、lan-compat、各宿主和测试 feature 分开核验，不靠 workspace feature union 掩盖遗漏 | 默认与 `-p uc-engine --features lan-compat,dev-tools` 的 check；`crates/uc-engine/tests/dependency_firewall.rs`；uniffi/ohos `workspace_contract`；`check-engine-repository.mjs` | 部分覆盖。iOS、Android、HarmonyOS 的实机和模拟器构建与运行：**跳过** |

**构建性能**：不属于 issue §7 的失败情形，结果和预算范围见 [实施记录](144-uc-infra-crate-split-record.md)。只在 Apple M4、`jobs = 2`、dev profile、预热 sccache 的增量构建条件下成立；冷构建、完整 workspace、release 和其他宿主未测。

**缺失的工件**：
- 没有按 issue §8 的格式（场景、revision/dirty、工具链、命令、退出码、双方终态、重启结果、脱敏日志摘要）为上表 E2E 产出独立工件。
- 已有的本地工件只覆盖 crash E2E 对照、uniffi 生命周期对照、Linux 栈帧复现和构建性能实验，保存在执行机的 `<local-artifact-dir>`，不随仓库提交。
