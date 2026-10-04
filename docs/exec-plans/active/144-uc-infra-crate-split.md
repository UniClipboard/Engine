# 144 `uc-infra` 拆成 7 个 crate

## 状态与完整责任

- **状态**：实施中。S0（完整基线实验）按用户指示跳过，直接进入 S1；S1 已完成并合并。S2–S6 未开始。
- **日期**：2026-10-03。
- **跟踪**：[Issue #144](https://github.com/UniClipboard/Engine/issues/144)（设计全文、七个 crate 的职责/允许依赖表、
  六类依赖切断方案、Edge Cases、测试策略、构建性能实验方法、验收标准均在 issue 正文，本文件不复制，只跟踪切片状态）。
- **依据**：issue #144 第 5/6 节的 crate 职责表与 S0–S6 步骤表。
- **完整负责人**：拆分顺序、每个切片的范围与验收由本计划维护；每个切片落地后的架构事实（依赖方向、包清单）
  由 `scripts/architecture/check-engine-repository.mjs` 校验，不在本文件重复记录检查脚本本身的规则。
- **调用方唯一动作**：无新增调用方接口——这是内部实现重组，`uc-engine` 仍是唯一稳定入口；下游产品仓不感知。
- **成功结果**：七个目标 crate 全部落地，原 `uc-infra` 删除，`uc-engine` 之外无人直接依赖七个新 crate；
  issue 第 9 节 Acceptance Criteria 全部勾选，包括真实构建性能 A/B 实验的结论。
- **失败结果**：任何切片发现 issue 设计在实际代码里行不通，必须在本文件和对应 PR 里写明原因，不得为了硬套
  设计破坏正确性（例如本次已确认 `content_key_catalog.rs` 和 `mls_group.rs` 不能跟 S1 一起走，见下）。
- **重启与重试责任**：每个切片（S1–S5）是一套独立可工作的迁移，不留旧路径 re-export；下一个切片可以在任意
  会话重新开始，只需先确认当前 `main` 上七个 crate 的实际边界（见下表状态列），不能假设 issue 原文的耦合点
  分析仍然完全对应当前代码。

## 切片状态

| 步骤 | 范围（issue 原文） | 状态 | 结果/证据 |
| --- | --- | --- | --- |
| S0 | 精确逐文件迁移清单 + 真实 timings/RSS 基线 | **跳过**（用户 2026-10-03 明确指示直接动代码） | 无基线数据；后续构建性能验收缺这一环，需要在 S4/S5 完成后补做公平 A/B（见"遗留风险"） |
| S1 | 提取 `uc-infra-local`、`uc-infra-crypto` | **完成** | [PR #145](https://github.com/UniClipboard/Engine/pull/145)，已 squash merge 到 `main`（`f0f0b5fb`，2026-10-04） |
| S2 | 原子提取 `uc-infra-security`（session/vault/事务代次一起搬，DB 耦合的生命周期部分留给 profile） | 未开始 | — |
| S3 | 提取 `uc-infra-storage`、`uc-infra-content` | 未开始 | — |
| S4 | 完成邀请 codec、错误分类、身份槽位切断，再提取整个 `uc-infra-p2p` | 未开始 | — |
| S5 | 剩余升级/激活能力迁 `uc-infra-profile`，LAN 移 `uc-mobile-lan`，删除 `uc-infra` | 未开始 | — |
| S6 | 更新架构门禁、CI、构建缓存、发布脚本、文档；完成公平性能对照 | 未开始（S1 已顺带同步了 `check-engine-repository.mjs` 的包清单，但完整 S6 清单未逐项核对） | — |

## S1 范围与已知偏离（2026-10-03，PR #145）

- `uc-infra-local`：`fs/`（不含 `key_slot_store.rs`）、`blob/`、`time/`、`device/`、`settings/`，以及四个扁平
  文件状态仓储（`app_version_state`、`engine_version_state`、`first_sync_state`、`migration_state`）和
  `file_secure_storage`。
- `uc-infra-crypto`：`crypto_model`、`secrets`（`MasterKey`/`Kek`）、`v1_aead`、`hashing`（`Blake3Hasher`）、
  `identity_fingerprint`。
- **`fs::key_slot_store.rs` 没有跟着走**：issue 原文把它归为 `uc-infra-local` 的例外（"不含 `fs/key_slot_store.rs`"），
  实际上它依赖 `crate::security::crypto_model::KeySlotFile`——现在是 `uc_infra_crypto::crypto_model`，但它更深层
  还是 `space/security` 的消费对象，留在 `uc-infra` 原地，等 S2 一起走。
- **`content_key_catalog.rs`**（issue 列为 crypto 的"纯 codec"目标）没有跟着 S1 走：它只在 `space/security` 内部
  使用，是 S2 那组"session/vault/事务代次"耦合的边缘部分，提前挪动的风险大于收益。
- **`mls_group.rs`**（OpenMLS 引擎，issue 列为 crypto 候选）依赖 `crate::security::MasterKey` 和更深的会话状态，
  是 issue 自己点名"不能按文件夹切开"的部分，S1 没有动它，留给 S2。
- 大量 `pub(crate)` 的算法/格式类型（`crypto_model` 的 `MAX_KDF_*`/`validate_kdf`、`v1_aead` 全部函数和错误
  类型、`secrets::MasterKey::into_bytes`、`fs::durability`/`work_directory` 的函数）在拆出去之前要先放宽成
  `pub`，因为同 crate 内其他目录（`space/security/`、`config_migration/`）跨目录调用它们——这类可见性收紧的
  真实使用面，比模块目录结构更能决定"这组代码能不能干净独立成 crate"，下一个切片开工前应该先用这个方法核实
  issue 原文列的允许依赖表是否仍然成立。

## 遗留风险 / 下一步必须处理的事项

1. **构建性能 A/B 实验（issue §8/§9）完全没有做**：S1 只分出两个零依赖的叶子 crate，`uc-infra` 仍是编译耗时的
   主体，现在测 A/B 没有意义。必须等 S3（storage/content）或更晚再测，否则"提速 20%"之类的验收标准无法验证。
2. **S2 是风险最高的一步**：issue 原文要求"先列失败矩阵再改动接口"，本计划尚未产出这份失败矩阵。
3. **S2 开工前的分支状态**：PR #145 用 squash merge 合入，本任务原分支
   `hp/uni/t-0161-uc-infra-7-crate-issue-144` 的提交历史和新 `main`（`f0f0b5fb`）不是祖先关系；继续 S2 必须从
   新 `main` 切干净分支，不能在旧分支上继续堆提交。
