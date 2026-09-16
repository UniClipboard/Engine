# 启动资料升级进度交付记录

日期：2026-09-10。状态：本批只读启动进度已完成并验证。

## 范围与责任

稳定契约及接入示例见[启动资料升级进度](../../design-docs/startup-upgrade-progress.md)。
Engine 负责一次完整启动结果；`ProfileStorageUpgrade` 负责转换、真实计量、警告、互斥、校验与持久恢复。
宿主只提交一次启动，观察者不控制步骤。启动成功返回完整 Engine，失败后仍能读取最后快照；新重试使用新输入。
没有修改 Desktop、移动绑定入口、CI 或发布版本，也没有新增启动前密码输入。

## 验证结果

本机 macOS 与 Windows 原生验证的 `crates` Git tree 均为 `6ba8ac8f96a5eca4bdbe012df90549051257538a`。
全局 Cargo 配置及 metadata 已核对，Engine/Infra/Application 均来自本仓。
本机复用既有外置 target 和 sccache；Windows 复用既有验证 checkout 的 target，无额外完整构建目录。

| 检查 | macOS | Windows |
| --- | --- | --- |
| 升级内部行为、兼容尾部、错误分类 | 21 项通过 | 21 项通过 |
| 真实升级集成、租约、每个耐久阶段重建 | 17 项通过 | 17 项通过 |
| 独立进程退出后的恢复 | 1 项通过，覆盖 5 个退出位置 | 同左 |
| 启动通道、真实启动、失败、普通重启 | 9 项通过 | 9 项通过 |
| alpha.5 实际生成资料：启动升级、历史读取、关闭重启 | 1 项通过 | 1 项通过 |
| 大批量内容、计数、恢复、篡改拒绝 | 4098 个内容表示与 2 个 blob，通过 | 同左，通过 |
| 稳定接口与依赖边界 | 45 + 34 项通过 | 未单独执行；启动验收使用同一入口 |
| workspace/all-targets、metadata、fmt、架构/隐私与 diff 检查 | 通过 | 本轮执行原生行为测试，未重复全 workspace 门禁 |

大批量输入为同一历史事件的多个内容表示，并包含一个不可读 blob，检查其原密文保留和警告不重复。
该组合测试耗时 macOS 34.28 秒、Windows 85.05 秒；包含输入生成、多次转换/校验与恢复，不是单次升级耗时。
未知总量的等待另外以模拟时钟跨过 13、46、147 秒验证，不能把它称为原用户 147 秒场景的原机复现。

关键问题先以测试复现后修复：缺少启动通道、非法警告数量未被拒绝、未完成首次初始化的普通重启误报升级。
快照保持有界，10000 次合并后终态可读；断开、丢弃输入、独立重试及资料完成后服务失败均有覆盖。
旧 journal 字节前缀保持兼容，新警告尾部经同一加密认证；旧记录的未知警告不会被补成零。

## 复跑入口

在仓库根目录运行：

```bash
cargo test -p uc-infra --lib security::profile_storage_upgrade --locked
cargo test -p uc-infra --test profile_storage_upgrade --test profile_storage_upgrade_crash --locked
cargo test -p uc-engine --lib startup --locked
cargo test -p uc-engine --test public_contract --test dependency_firewall --locked
```

附加大批量检查设置 `UC_UPGRADE_STRESS_ROWS=4096` 后运行
`security::profile_storage_upgrade::primary_payloads::tests::primary_output_is_atomic_preserves_unreadable_blobs_and_is_digest_bound`。
完整旧版样本通过 `UC_ALPHA5_FIXTURE_DATA` 指定独立合成 profile，再运行忽略入口
`startup_progress_upgrades_alpha5_profile_and_reopens_history -- --ignored`。测试只在临时副本升级，不修改样本源。

## 验证边界和后续责任

- 本次 alpha.5 实际生成的完整 profile 样本仅含文本；blob、文件清单和派生资料通过独立的真实存储测试覆盖。
  包含实际文本、图片、附件的完整 alpha.5 产品场景仍待 Desktop 接入时整体验收。
- 正确/错误密码输入、等待解锁和移动绑定进度入口尚未实现，不能展示相应操作。
- 保护材料不可用与密码错误不混用；底层已丢失细分原因的情况仍给保守分类，不解析英文错误正文。
- 磁盘满和拒绝访问的进度分类使用类型化故障输入验证；物理磁盘填满、断电未执行，记为跳过。
- GUI 关闭重开、认证状态服务、12/45 秒前端等待及自动重连由 Desktop 继续完成，本仓没有修改桌面代码。
- 原用户完整资料与原设备系统密钥未参与本次测试；原机升级复验跳过。Windows 三处既有未使用变量警告保留。
- 没有发布或上传产物，不能将本次核心验证称为完整产品已上线。
