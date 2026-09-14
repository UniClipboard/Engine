# Task Plan: Admission Expiry S2-S8

## Goal
按既有规格依次完成 S2 至 S8；每片独立形成可验证的完整结果、同步文档并创建一个本地提交，不推送。

## Completion Standard
- S2：双端共用原始五分钟期限；邀请方能持久查询等待确认、未确认、已确认，迟到合法确认可补齐。
- S3：撤销精确绑定原 Add 实例；旧撤销不伤同设备的新成员实例。
- S4：已决定或远端决定未知时，本机仍能终止并持久保留清理责任。
- S5：放弃事实可靠投递；重复、乱序和迟到消息不能复活旧成员。
- S6：跨空间激活与取消共用最终串行边界，旧任务不能覆盖新意图。
- S7：Joiner、Sponsor、Helper 的到期统一调度、恢复、回收和迁移完成。
- S8：Engine、绑定、双端测试及文档完整收口。
- 每片相关定向测试、全 workspace 检查、格式和架构门禁通过后单独提交。
- 未执行的真实网络、产品端或实体设备项目明确记为跳过。

## Slice Status

| Slice | Status | Commit |
| --- | --- | --- |
| S1 | complete | 91287ca1 |
| S2 | complete | 38470b48 |
| S3 | complete | 0a139a8c |
| S4 | complete | this commit |
| S5 | pending | pending |
| S6 | pending | pending |
| S7 | pending | pending |
| S8 | pending | pending |

## Current Slice: S4

### Owner and boundary
- 准入流程继续完整负责取消、新意图替换和到期恢复；调用方不判断内部阶段。
- Core 决定本机能否结束，并把远端是否已经提交分为已知和未知两种清理责任。
- 准入仓储在结束记录中密文保存清理责任，并在同一提交中释放当前加入槽位。
- 本片不发送放弃事实，不执行跨空间退出；网络投递属于 S5，最终本机隔离属于 S6。

### S4 Phases
- [x] 正式决定前继续允许直接取消和替换
- [x] Prepared 保存远端决定未知的清理责任并立即结束本机
- [x] Committed、Applied、Activating 保存精确成员清理目标并立即结束本机
- [x] 用户取消、到期和新意图替换共用同一条本机终止规则
- [x] 验证重启编码、并发晚到 Commit、旧记录兼容和真实加密仓储
- [x] 更新执行计划和架构圣经
- [x] 严格审查、完整门禁
- [x] 创建 S4 本地提交

## Errors Encountered

| Error | Resolution |
| --- | --- |
| 当前受限环境不能写仓库外的共享构建锁，Cargo 命令在编译前停止 | 权限恢复后继续使用仓库共享构建目录，定向测试和全仓检查均已通过 |
| 设备确认查询测试使用了没有原始 Add 记录的基线成员，无法形成精确绑定 | 改为构造一条真实 Add 及激活记录，再验证确认状态只关联该成员实例 |
