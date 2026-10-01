# 升级备份安全记录生命周期

## 状态与完整责任

- **状态**：已实施，待合并；合并并被下游以不可变版本验证后移入 `completed/`。
- **日期**：2026-10-01。基线：Engine main `e99f7082fd2b3a66c60dac5f104b42f96742a340`（PR #132）。
- **依据**：Windows 现场在出厂重置后首次版本提升启动失败，Engine 启动结果为 `PROFILE_UPGRADE_BACKUP_KEY_MISSING_CODE`（1224，不可重试），每次启动都复现。
- **不变量**：一个密钥和它保护的密文共用一个生命周期；出厂重置负责作废派生密文；文件副本不是派生密文。
- **完整负责人**：
  - 出厂重置的流程顺序与结果由 Application `ProfileFactoryResetFacade` 负责，作废动作是 `KeysWiped` 阶段内、状态清除之前的一步，随该阶段重放，不新增持久阶段，不改变生命周期记录格式。
  - 安全记录文件与记录密钥的退役能力由 Infra `ProfileUpgradeBackupStore` 提供，只有一个实现，出厂重置与升级准备共用它。
  - Engine 只装配 port，不编排步骤。
- **调用方唯一动作**：宿主调用既有出厂重置与既有启动；无新增公开接口、错误码或字段。
- **成功结果**：重置后备份目录中没有 `security-current`、`*.record` 与记录密钥，文件副本可验证、可恢复；重置后的版本提升启动成功；历史上已卡死的形态（旧安全记录仍在、密钥已消失）无需手工操作即可启动。
- **失败结果**：作废失败映射为既有 `FACTORY_RESET_KEY_MATERIAL_FAILED_CODE`，阶段停在 `KeysWiped`，重启后继续；升级准备中的非“密钥缺失”失败（存储读取、记录损坏、解密）保持失败，记录与密钥不被改动。
- **重启与重试责任**：作废动作幂等，目录或记录不存在即完成；先删指针 `security-current` 再删各代记录，中断不留悬空指针；升级准备在持备份租约时作废并重建，中断后下一次启动重做。

## 代码事实（与交接文档的差异）

- `publish_record` 在 `security-current` 已存在且缺钥时自己返回缺钥错误，只放宽读取探测会把失败后移一步。
- `delete_locked`（删除备份与保留清理）会解密目录里每个 `*.record`；换新密钥而不清理旧记录，会让备份超过 5 份时的保留清理失败。因此“缺钥”分支必须连同旧记录一起作废。
- 出厂重置清除密钥时不显式删除记录密钥，它随 `vault/profile-secrets-v1` 在状态清除时消失；作废步骤显式删除，也覆盖不使用该文件的平台。
- 当前没有生产代码读取 `SecurityBackupRecord.secrets` 来恢复；真正需要密钥的严格路径只有 `read_record` 与 `delete_backup`。
- “缺钥”只在密钥存储返回确定不存在时成立：密钥恢复流程（受限恢复、口令解锁）先于装配与备份准备完成，受限状态下不会进入备份准备。

## 失败分类与可观测性

- 安全记录缺钥是 `error_kind=record_key_missing`，`retryable=false`，与 Engine 启动结果一致，先于 IO 与存储兜底分类，不再落入 `backup_internal`。其他分类仍为可重试。
- 升级准备中已处理的缺钥分支不是失败，用 `uc_warn!(cause = "record_key_missing", ..)` 单独记录；日志不含路径、密钥或记录内容。
- 事件 `profile_upgrade.backup.failed` 的 `retryable` 由分类决定（此前写死为 `true`），字段名与低基数约束不变。
- 启动结构化状态（`StartupSnapshot.failure`）原来对任何备份失败都写 `reason=BackupFailed, retryable=true`，与 1224 的不可重试结果矛盾；现在备份包装在错误链含记录密钥缺失时写 `retryable=false`，`allowed_actions.retry=false`、`export_diagnostics` 保持 `true`。字段与取值集合不变，不新增公开接口。

## 验收与证据

失败场景清单、红绿日志、远端运行记录与退出码见线程报告；已有测试覆盖：

- 备份存储：重置作废后文件副本仍可验证和恢复且重复调用无副作用；重置后版本提升；旧记录加密钥丢失后跨越保留上限继续升级并可删除备份；严格读取仍返回缺钥；记录损坏但密钥存在时不被当作缺失。
- Application：作废步骤顺序、各阶段重放、作废失败不推进阶段并保留 source chain。
- Engine 端到端（持久宿主）：重置后无安全记录而文件副本保留；重置后的版本提升启动成功；还原历史卡死形态后启动成功。

## 未覆盖与后续

- 真实 Windows 现场未操作。已卡死 profile 的只读副本（用户授权，仅拷贝，不在原机安装、启动或改动）在本机隔离验证：
  `crates/uc-infra/tests/profile_upgrade_backup_stuck_copy.rs`（设置 `UC_STUCK_PROFILE_COPY` 才运行，否则跳过）。基线提交上同一副本复现
  “记录密钥缺失”启动失败，修复后首次启动、同目标再次启动、更高版本启动均成功，资料文件与原有文件副本字节不变。
  局限：副本没有 Windows 凭据存储中的密钥，生命周期标记用内存存储合成，口令解锁与完整 Engine 启动未在副本上验证。
- `delete_backup` 在缺钥时会先删除 `current` 与 `security-current` 再因读取旧记录失败，属既有的非原子删除，本计划不改变。
- 1224 与邀请域 `INVITATION_FAILED_CODE` 同号（1221–1223、1231 同样重号），本计划不重新编号；宿主按操作上下文匹配。
