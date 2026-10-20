# 设备可达性连接迁移

## Goal
按已确认规格与执行计划交付 Issue #34 第六切片，保留文件／blob／进度等剩余范围。

## 完整责任与验收
Application peer_connections/transport 拥有准入、唯一观察、长期保留与存活；coordinator 拥有调度重试；Infra 提供网络与身份能力；Engine 装配。调用方沿用现有完整动作。成功仅由当前有效资格和完整验证产生；本机读取故障保留最新有效显示与 source，取消不发布迟到结果。无新增持久格式。

## Phases
- [x] 阶段 0：冻结版本、真实旧旧资料与单端升级前置、生产修改前 red。
- [x] 阶段 1–4：唯一 codec、Uni 生命周期、Application transport、生产接线及旧路径删除。
- [x] 阶段 5：全部回归、默认 CI 和平台实际检查。
- [x] 阶段 6：最终审查、提交、证据清理与归档。

## Next Step
实现、本地与九项必需 CI、临时资源清理和规格／计划归档完成。复核 rc.23 归档文档的根门禁与程序输入树等价性，提交／推送归档，并核验 PR #223 最新 head 全部必需检查后交付；最后一轮检查的唯一动态来源是 PR checks，实际结果另存忽略的证据目录与 PR 描述。物理设备和额外历史矩阵为跳过，不关闭整个 #34。

## Current Phase
阶段 6 已完成归档。业务迁移已完成；最终网络 11 格与 workspace 4303 项通过。成员首次两轮失败及全部对照保留；沿用既有固定重启地址 fixture 后完整 56 项通过，不将 fixture 收敛描述为生产根因修复。所有本地回归与根门禁、九项必需平台 CI 均通过；合入 rc.23 主线后的完整提交树与已通过 CI 的合成树相同。

## 已取得的证据
- 两个旧端冻结为 b6fa0ce6b2efd1c1764e76c0a76537bdeead4e1b 与 a07 f6f305d9689e4e79e7ab6d0e4921061f9416e4a6；旧库源码不打补丁。最新整合 main 为 f0627a8b0bcc274707035a86ab2df47154d1e536。
- oct20-final-related.log：109 项相关回归通过，含完整 owner 第二 backend、7 项原生 presence、真实 SQLite／source／隐私及 5 项真实 clipboard trace。
- oct20-required-final-incoming-control：11 个必需网络格全通过；新新 Relay 20 轮、冻结 main 双角色各 3 轮、rc.15 双角色各 1 轮，清理与明文扫描均通过。
- oct20-final-workspace-with-incoming.log：4303 passed、27 skipped（执行 633.977 秒），包含新增原生入站故障控制及原两项本地生命周期失败回归。
- Jaeger／Collector 官方同版本校验后真实导出与浏览器列表／详情通过；临时服务、浏览器和下载已清理，仅保留成员隔离 runner。
- 两次最终成员全集各 55 passed、1 failed、14 skipped；同一缺前置历史恢复用例在 B 已追上 epoch 5 后，C→B 未确认。单项隔离／宿主／隔离三次、并发 g4 两项均通过；原二进制保留。
- 固定端口目标单项通过（36.447 秒）；完整组尚未结算。生产 3 秒／20 秒／70 秒门限及成员 120 秒等待不变。

## 必须完成的剩余工作
- [x] 固定端口对照完整成员全集通过；保留原失败与实际不确定性。
- [x] 升级 smoke、testkit、普通剪贴板／组更新／成员历史／当前状态与 pull、原网络全场景通过。
- [x] 最终根 metadata、all-targets check、fmt、风格、架构、脚本测试、audit、diff check。
- [x] 当前分支提交／新 PR；最终 SHA 的 Linux、macOS、Windows x64／arm64、iOS、Android、HarmonyOS SDK CI。
- [x] 检查进程并回收本次旧源码 src、临时基线快照与 runner；保留脱敏证据与共享编译缓存。
- [x] 规格与计划一起移至 completed 并更新索引；真机和额外历史矩阵明确跳过，不关闭整个 #34。

## Decisions
- 保留用户已有 planning 目录，特别是新增 blob 访谈目录；提交只选择本任务文件。
- 沿用当前分支提交与新 PR，PR #181 已合并；不重写用户历史。
- code-review 技能不可用，按完整调用链、公开边界、source、容量／期限／取消、真实矩阵手工审查。
- Cargo 验证只由本会话串行使用仓库 target 与 MBX；rootless namespace 只执行预建二进制。
- 显式准确来源可避免本机观测 build script 跟踪整个 crates 树的重建；不清空缓存或绕过 wrapper。

## Errors
完整错误与尝试见 progress.md；首次失败工件均单独保留。当前需要解决的是成员全集验收，不是环境权限阻塞；无免密 sudo 已由真实 rootless namespace 执行解决。
