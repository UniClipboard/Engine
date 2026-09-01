# Space Application 维护地图

完整入口、事实所有权、Case 手册、Ledger 与测试地图见
[`docs/design-docs/space-application.md`](../../../../docs/design-docs/space-application.md)。

## 首要入口

1. `facade/facade.rs`：唯一公开 `SpaceFacade` 实现。
2. `application.rs`：Space cases、endpoint、ledger 与 runtime 的唯一组装点。
3. 本次业务的 `*/use_case.rs`：完整流程负责人。
4. `membership/ledger/`：成员事实、CAS、scope 与恢复阶段。

## 硬约束

- `space/mod.rs` 是唯一模块出口，外部不得穿透子目录。
- 一个业务动作只由一个完整 case 负责，Runtime 不掌握内部步骤。
- V2 签名成员历史是成员资格的唯一正向事实；普通消费者只读统一 current scope。
- 成员 ledger 的历史、关系、分页、效果和 revision 只做条件原子提交，并使用 MasterKey AEAD。
- 准入状态使用独立加密仓库，不回流 membership ledger。
- 正式 Add/Remove/Decision 提交后不回滚；后续效果由持久阶段恢复。
- 日志不得包含身份、邀请、地址、签名、密钥、文件名、路径或内容。

修改后执行 Space 定向测试、Application check、fmt、架构检查与 diff check，并同步架构圣经。
