# 设备可达性连接迁移

## Goal
按已确认规格与执行计划落实第六切片；必须先证明真实 rc.15 配对资料单端升级。

## 完整责任与验收
Application peer_connections/transport 拥有准入、唯一观察、长期保留与存活；coordinator 拥有调度重试；Infra 提供网络与身份能力；Engine 装配。调用方沿用现有完整动作。成功仅由当前有效资格和完整验证产生；本机读取故障保留显示与 source，取消不发布迟到结果。无新增持久格式。

## Phases
- [ ] 阶段 0：冻结版本、旧旧配对与单端升级基线、生产修改前置。
- [x] 阶段 1–4：codec、Uni 生命周期、Application transport 与生产接线。
- [ ] 阶段 5：真实网络矩阵、默认 CI、回归。
- [ ] 阶段 6：稳定文档、审查与提交。

## Next Step
native、workspace 和根门禁已通过；最终默认网络矩阵与成员全集运行中。完成升级／前五片／既有网络门禁，核验平台 CI，审查并提交当前分支。所有未执行项仍不勾选，不归档。

## Decisions
- 保留用户已有文档和 planning；当前分支按 implement 技能提交，合入最新 main 作为冻结实现基线。
- 已确认测试边界来自 spec §8：codec、第二 backend、完整 owner 和真实 Engine。
- code-review 技能未在可用目录找到；完成后执行独立逐项 diff 审查。

## Errors
- sudo -n true：当前会话没有免密 sudo；暂不能执行需要 root 的 netns，继续执行普通用户真实旧资料前置。

## Current Phase
阶段 1–4 实现及垂直回归通过；阶段 5 最终网络与成员／升级回归进行中。阶段 0 已取得 rc.15 原资料前置、冻结 main 独立构建与慢读取 red；最终 CI 证据待补齐。

## 近期验证
完整 owner 六项第二 backend 测试通过；codec、Uni 生命周期/额度测试通过；workspace 全目标 check 与 Rust style 曾通过，后续改动仍需最终重跑。

## 必须完成的剩余工作
- 旧生产路径删除及外部 tests 迁移已完成；继续核验仍有价值的并发与时序回归。
- 真实 SQLite 与身份故障、deadline、移除重加/代次/共享 waiter、第三条入站替换及任务 join 故障验收。
- Engine 完整既有能力 decorator 和不透明关联；真实观测结果/结束/隐私回归。
- 新新、冻结 main/rc.15 双角色 × Direct/Relay，v2 三秒恢复 20/3 轮、v1 关闭 70 秒、默认必需 CI。
- 根门禁、完整 suite、相关设备编译、审查、当前分支提交、归档/索引、线上最终 SHA 检查。
- 最终检查并回收任务创建的旧源码 src、隔离 fixture/进程；保留脱敏证据，恢复当前宿主。
