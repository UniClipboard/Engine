# Findings

## Requirements

- 故意制造会话交接失败。
- 失败后旧 Space 权限不可重新开放。
- 同一身份不能同时出现两个网络入口。
- 故障解除后当前进程自动恢复，不依赖应用重启。
- 保持三秒配对性能方案和现有公开接口。

## Research Findings

- 规格 043 已把“提交成功后新会话构造失败”和“恢复重试”列为未完成验收。
- 上一提交已经让切换 watcher 在网络存在但会话为空时尝试恢复，但缺少可重复的真实故障证据。
- 现有 `dev-tools` 已提供公开测试入口和网络 endpoint 查询，可用于证明恢复前后没有重绑；没有现成的会话准备失败开关。
- 最小新增面应放在 Engine 内部会话工厂，而不是污染 Infra 协议、Application 业务或公开稳定操作。
- 当前 `QueryNetworkEndpointId` 虽从网络测试门读取数据，但执行前仍要求活动 facade；故障窗口内应通过另一个不依赖会话的测试观测入口检查网络是否仍在。
- Engine 的 dev 操作本身仍受最外层 Engine 生命周期保护，但不经过普通业务会话操作门；适合承载测试开关和匿名网络计数。
- `execute_dev` 目前无条件先取得 facade，导致本来只读网络测试门的操作也无法在会话空窗执行；应让各操作只取得自己实际需要的依赖。
- dev-tools 合同集中在 `crates/uc-engine/src/dev/mod.rs`，新增一次性故障动作不会进入默认构建或平台稳定接口。
- 首次真实运行已观察到注入失败：普通操作返回不可用，切换 watcher 一秒后重新准备会话并完成目标 Space 加入。
- endpoint identity 由设备密钥决定，即使重绑也可能相同；必须用网络构造次数而不是只比较 identity 证明没有第二次构造。

## Technical Decisions

| Decision | Rationale |
| --- | --- |
| 优先注入会话准备失败 | 它位于持久切换完成和新权限发布之间，是最危险且最能证明恢复责任的边界 |
| 故障开关与网络计数放在 SessionSupervisor 的 dev-only 测试控制器 | 既能覆盖真实工厂路径，也不向 Infra 或稳定接口泄露生命周期步骤 |
| 同时统计注入消费次数和网络构造次数 | 防止故障未触发或身份相同的重绑造成假通过 |

## Issues Encountered

| Issue | Resolution |
| --- | --- |
