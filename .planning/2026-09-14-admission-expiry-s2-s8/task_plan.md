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
| S3 | complete | this commit |
| S4 | pending | pending |
| S5 | pending | pending |
| S6 | pending | pending |
| S7 | pending | pending |
| S8 | pending | pending |

## Current Slice: S3

### Owner and boundary
- 现有成员移除用例继续完整负责签名、成员历史提交、安全效果和受限投递责任。
- 公开按设备移除先固定当前精确成员；冲突重试不得重新解析为后来同设备的新成员。
- 准入撤销只提交已验证的 admission、space、成员实例和原 Add 绑定。
- Core 只提供按精确成员查找既有 Remove 的规则，不在准入流程复制移除逻辑。
- 本片只提供可复用的精确撤销能力；到期和取消接线属于 S4。

### S3 Phases
- [x] 调查现有成员移除、签名、账本和安全效果路径
- [x] 先写旧撤销重放不能移除同设备新实例的失败测试
- [x] 固定精确目标并让 CAS 重试保持原实例
- [x] 增加 admission 专用目标、稳定去重和结果分类
- [x] 验证错误空间、错误 Add、无权签名和本机效果待完成
- [x] 更新执行计划和架构圣经
- [x] 严格审查、完整门禁
- [x] 创建 S3 本地提交

## Errors Encountered

| Error | Resolution |
| --- | --- |
| 当前受限环境不能写仓库外的共享构建锁，Cargo 命令在编译前停止 | 权限恢复后继续使用仓库共享构建目录，定向测试和全仓检查均已通过 |
| 设备确认查询测试使用了没有原始 Add 记录的基线成员，无法形成精确绑定 | 改为构造一条真实 Add 及激活记录，再验证确认状态只关联该成员实例 |
