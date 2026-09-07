# 已确认事实

- 原文件 2860 行，主分支原有 2838 行；本次选择修复净增 22 行。
- 内容包含凭据、事件和准入证明、页模型、历史聚合、签名验证、导入导出、历史与分支计算、持久编码和分包。
- 没有实际网络或数据库 IO；签名规范和历史字节布局被 Core 恢复包、Application 账本和 Infra 准入同时使用，需要唯一且字节稳定的实现。
- Core 内规则可以按记录、历史、校验、决定、交换与存档分成私有实现，保持 membership 根公开出口不变。
- MembershipLedger::exchange_conflict_evidence 承担了完整证据处理流程，包含读取、旧选择衔接、授权恢复识别、关系变化与回复；适合独立 Application case，而不是仓储内业务流程。
- 两个真实调用方为 handle_history_message 与 synchronize_history；仓储保留验证加载与条件原子提交，不决定用户行为。
- 分页导入导出包含发送者授权与完整性校验，属于纯规则转换；保留在 Core 内独立私有 exchange 模块。Application 的持久提交、重试、应答流程不混入这些规则。
- 即将固定存档、完整证据页、后缀页的确定性字节校验值，防止结构搬迁改变已有格式。

## 当前修复的验证证据

- Core membership_history_v2 首轮 38 项通过；之后修复了拒绝初始准入基线时读取父事件的缺陷，需要最终复跑。
- Application membership 96 项通过，包含新增复现和旧记录衔接。
- 真实四实例接受、保留与重启通过：62.13 秒。
- 五实例相反移除后的远端分支恢复 F2 通过：125.69 秒。故障注入期间日志有预计的断连与恢复告警，不能据此判失败。
- Desktop Provider 8 项、设备组件 82 项、TypeScript 与格式/静态检查已通过。
- Engine 全部库测试、最终 workspace check/metadata/架构门禁尚未完成。
