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
| S2 | complete | this commit |
| S3 | pending | pending |
| S4 | pending | pending |
| S5 | pending | pending |
| S6 | pending | pending |
| S7 | pending | pending |
| S8 | pending | pending |

## Current Slice: S2

### Owner and boundary
- `SpaceAdmissionProtocol` 继续完整负责准入流程；Sponsor 角色负责正式应用后的确认状态。
- Engine/Facade 只执行既有 Join、消息处理和设备查询动作，不读取协议内部阶段。
- Core 验证统一期限、确认转换、精确绑定和迟到确认白名单。
- Infra 继续使用现有逐记录密文仓储，不把确认状态写入成员账本。
- Recovery 在原截止点把等待确认转为未确认；未确认不撤销成员、不占加入入口。

### S2 Phases
- [x] 调查当前消息、Sponsor/Helper 状态、查询与持久化接线
- [x] 先写双端期限、等待确认、到期未确认、迟到确认的失败测试
- [x] 实现 Core 消息和状态规则
- [x] 接通 Application/Infra 查询、恢复与通知
- [x] 运行双端、重启、错误绑定和权限不扩张验证
- [x] 更新执行计划和架构圣经
- [x] 严格审查、完整门禁
- [x] 创建 S2 本地提交

## Errors Encountered

| Error | Resolution |
| --- | --- |
| 当前受限环境不能写仓库外的共享构建锁，Cargo 命令在编译前停止 | 权限恢复后继续使用仓库共享构建目录，定向测试和全仓检查均已通过 |
| 设备确认查询测试使用了没有原始 Add 记录的基线成员，无法形成精确绑定 | 改为构造一条真实 Add 及激活记录，再验证确认状态只关联该成员实例 |
