# Progress Log

## Session: 2026-09-08

### Current Status
- **Phase:** 1 - Requirements & Discovery
- **Started:** 2026-09-08

### Actions Taken
- Reproduced the desktop daemon failure and narrowed it to legacy UCBL conversion.
- Added temporary diagnostics in Cargo checkout, counted unreadable rows without logging IDs or paths, then removed all diagnostics and rebuilt the unmodified dependency.
- Read Engine root and `uc-infra` rules plus security, error, observability, and module-design guidance.
- Confirmed the existing availability model and Space-transition precedent support preserving ciphertext while marking only affected payloads `Lost`.

### Test Results
| Test | Expected | Actual | Status |
|------|----------|--------|--------|
| Desktop SQLite `quick_check` / `integrity_check` | `ok` | `ok` | pass |
| Legacy UCBL read preflight | Identify unreadable count | 11 unreadable of 13 | reproduced |
| Desktop frontend build | No stale React Router entry | passed after Vite cache rebuild | pass |
| Focused `PrimaryPayloadConverter` regression | Existing implementation rejects unreadable blob | failed at `open legacy UCBL payload` as expected | red |

### Errors
| Error | Resolution |
|-------|------------|

## 继续修复：2026-09-08

- 用户已授权继续实现。使用 diagnosing-bugs 和 planning-with-files，保留其他审计计划及活动指针。
- 本轮复现命令：`cargo test -p uc-infra --lib primary_output_is_atomic_preserves_unreadable_blobs_and_is_digest_bound --locked -- --nocapture`。1 项测试在 `open legacy UCBL payload` / AEAD 解密失败处变红。
- 首次误用 `--exact` 配合短名称，运行了 0 项测试；已改为库测试名称筛选并确认实际执行。
- 完整负责人仍为 ProfileStorageUpgrade；调用方唯一动作 ensure_v3，保留密文并标记 Lost 后正常完成升级，介质/目标完整性错误仍中止，由既有 journal 恢复。
- 发现后续 primary 校验强制所有 blob 都为 V3，必须同步验证保留原字节与 Lost 引用；derived 阶段复制整个 blob tree，最终摘要继续覆盖保留字节。
- 架构圣经原规则要求历史解密失败整体中止，将同步收窄为控制面、未知格式和介质错误失败关闭；明确旧 blob 的不可用保留边界。
- 导航中发现 crates/AGENTS.md、secondary_payloads.rs、filesystem_blob_store.rs 不存在，已使用最近 Infra 规则与实际 derived_payloads.rs、filesystem_store.rs。

- 第一轮核心复现测试已转绿（1 项）；扩充 V1、缺文件、格式与篡改场景后，profile_storage_upgrade 库测试 12 项全部通过。
- 已同步架构圣经稳定规则与维护记录；未增加公开接口、持久字段或正常 V3 兼容 reader。
- 导航命令一次被 zsh 未匹配通配符拒绝；改为明确已存在的完成规格路径读取。

- 完整升级测试首次编译发现 blob.encryption_algo 为 nullable；已修正测试和新增读取映射为 Option，并覆盖算法列为空的旧孤立 blob，避免合法旧行被新增读取拒绝。

- 完整升级首次运行 14/15 通过，新 fixture 的重复 content_hash 违反真实唯一约束；已改为每个测试 blob 独立 hash，保持数据库约束不变。
- 审查补充 primary blob 目录 fsync，确保保留副本的目录项在发布前落稳。架构门禁与 diff check 首轮通过。

- 完整测试添加真实 blob 后发现原空 V2 fixture 只 initialize 密钥，没有群组 catalog，V3 写入正确拒绝未初始化保护上下文；补用正式 GroupBootstrapPort 持久化群组材料后继续验证，不修改生产缺钥规则。

- 完整 profile_storage_upgrade 集成测试 15/15 通过（2.53 秒），包含生产 for_runtime 升级、正常 blob 可读、两条引用同时 Lost、孤立/nullable 算法保留、source 清理、重建 owner 重复启动与明文探针。
- 本轮原始桌面 profile 未复跑：当前会话未提供该资料的位置与密钥环境，既有统计来自前序记录；本轮只使用临时合成真实存储 fixture，不访问或修改用户历史数据。

- 安全相关库回归 `cargo test -p uc-infra --lib security:: --locked`：148/148 通过（包含升级、V1/V2 reader、缺钥拒绝、V3 密文与控制面恢复），运行 3.23 秒。
- `cargo metadata --locked --format-version 1` 已通过；全仓 all-targets check 正在执行，使用仓库 target 与原有缓存。
- 最终审查确认：所有新增生产 unwrap/expect/打印均为零；固定原因不保存原错误文本；原子候选与目录摘要继续由现有 owner 管理。

## 最终验证

| 检查 | 结果 |
| --- | --- |
| security:: 库测试 | 148 通过 |
| profile_storage_upgrade 集成测试 | 15 通过 |
| cargo metadata --locked --format-version 1 | 通过 |
| cargo check --workspace --all-targets --locked | 通过，34.28 秒 |
| cargo fmt --all -- --check | 通过 |
| node scripts/architecture/check-engine-repository.mjs | 通过，包括隐私及所有权负向 fixture |
| git diff --check | 通过 |
| 原始桌面 profile | 跳过：缺少明确资料位置与密钥环境，未修改用户数据 |
| 实体设备、产品构建与发布 | 跳过 |

代码修复和本轮自动验证已完成，未提交或发布；其他依赖审计计划与活动指针保留。

## 手动测试继续：2026-09-08

- 用户报告界面持续 loading；dev daemon 退出，GUI 等待连接 45 秒超时。
- 当前 daemon 已重建，升级的固定错误分类变为 security；真实 dev 的 primary 输出已存在，包含 13 个 blob。
- 使用调试器定位新的失败；不把原错误正文写入新增业务日志。
- 误以为 uniclipd 支持 --help，实际启动了默认 profile；发现后已立即发送 TERM 停止，并向用户说明。后续执行明确指定 UC_PROFILE=dev，输出仅保留固定诊断。
- Engine 的依赖清理由另一会话进行中，保留其 Cargo 与架构记录修改；Cargo 编译等待共享构建任务结束。

- GDB 早期断点未覆盖泛型实例，且首次手动启动缺少 UNICLIPBOARD_ENV=development 导致读了不同 keyring namespace；已与产品启动环境对齐，排除该诊断干扰。最终真实断点锁定 UCFS 解密失败。
- 只读密码校验：当前 keyslot 认证通过，UCFS 0/3、搜索 render 5/55、active register 1/1；仅输出计数。

- 新回归 `unreadable_derived_caches_are_preserved_before_rebuild` 在旧转换器上变红：Security / file-set path AEAD verification failed，与真实 GDB 失败一致。依赖审计修改后首次测试重建约 1 分 32 秒，复用仓库 target，未绕过缓存。
- 实现文件清单和搜索的具体 AEAD 失败分类、完整输入数据库的 V3 加密恢复快照、候选事务内投影失效、快照来源比对/认证以及缺失快照恢复拒绝。

- 新修复验证：安全库测试 149/149；完整升级集成测试 15/15；metadata、workspace all-targets check（48.07 秒）、fmt、架构门禁与 diff check 全部通过。
- desktop 已使用本地 Engine 开始重建并暂存调试 daemon；不改产品业务代码。

## 真实资料验证完成

- desktop 调试 daemon 已重建并暂存 sidecar（24.40 秒），四个 Engine 分层包均解析到本地仓库。构建后完整 metadata 需要重新解析锁文件；运行 offline metadata 更新后，offline locked metadata 与 diff check 均通过。
- 使用 UC_PROFILE=dev、UNICLIPBOARD_ENV=development、RUST_LOG=off 启动本会话拥有的 daemon：健康接口 HTTP 200 / ok，无 degraded，保持运行。
- 正常停止后重新启动：再次 HTTP 200 / ok。旧 uniclipboard.db 已由正式升级流程清理，活动资料内保留 1 份加密恢复快照。两次诊断进程均已停止，不留下后台测试 daemon。
- 本次明确验证了 dev 资料升级和重启；界面交互、其他 profile、实体设备矩阵仍跳过，留待用户手动验收。此前 Phase 4/5 的真实资料跳过记录只反映当时状态。
- 保留 desktop 临时本地 patch；未提交或发布；未修改其他会话的依赖审计、活动指针或 quick-panel 研究。
