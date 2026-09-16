# 剪贴板完整链路

## 目标

从宿主复制通知到本机保存、跨设备发送、对端保存和系统剪贴板写入，能在同一个 trace 中查看；保留异步写入、失败和去重语义。

## 当前状态

已完成本轮实现与验收；改动未提交、未推送。

- 配对修复已本地提交 `f0806cca`，未推送，其他工作未混入。
- 已新增真实复制通知的双设备 E2E，并验证目标记录和系统剪贴板均已更新。
- 原先缺少完整记录的 E2E 已转绿；正常、系统写入失败、慢写入三条真实双 Engine 路径均通过。
- 用户已同意实现较小方案：保留广播、回执及串行接收；只将 Application 专用接收订阅合同移回 Application，由不透明任务信封携带在线关联。Core 业务消息不变。

## 改造边界

1. Application 继续唯一拥有策略、解密、保存、回执和异步系统写入。
2. 保留现有广播订阅和完整接收负责人。接收 port 只迁移所有权，方法数不增加；任务信封在认证后捕获关联，Application 消费时恢复。删除 Core 旧 trait，避免双轨。
3. 不把 TraceId、步骤或观测字段加入 Core 业务负载；不把 Application 私有步骤暴露给 Engine。
4. 接收串行约束、容量、拒绝和关闭等待必须保留；回执继续表示保存结果，不能改成等待系统剪贴板写完。
5. 异步系统写入作为已关联的后续节点；失败要可见，不把“保存成功”当作“粘贴板写入成功”。

## 主要文件

- `crates/uc-core/src/ports/clipboard/sync_receiver.rs`：仅保留原业务数据与回执，旧专用订阅 trait 已移回 Application。
- `crates/uc-application/src/clipboard/inbound/runtime.rs`：完整接收及生命周期。
- `crates/uc-application/src/clipboard/assembly.rs`：完整接收装配。
- `crates/uc-infra/src/network/iroh/clipboard_receiver_adapter.rs`：认证后的完整交接。
- `crates/uc-application/src/clipboard/sync/apply_inbound/usecase.rs`：保存后的异步写入关联。
- `crates/uc-engine/src/assembly/observability/clipboard.rs`：既有完整能力的记录包装。
- `crates/uc-engine/tests/space_membership_auto_pairing_e2e.rs`：真实复制、目标保存和系统写入验收。

## 验收

- 正常文本复制：复制通知触发，双方历史记录与目标系统剪贴板匹配，关键节点在同一个 trace。
- 写入失败、重复接收、拒绝、队列关闭、后台写入迟于保存确认，都有独立结果。
- 不泄露内容、设备名、路径、邀请；维护与手动重发不串入无关复制。
- 根据实际触及的图片/文件路径补充相应验收；未执行范围不得宣称支持。

## 未提交工作边界

AGENTS、旧资料升级、构建存储检查和它们的 planning 都属于既有其他工作，禁止回退或混入本任务。

## 交付检查

- [x] 保留广播、回执与完整业务 owner，仅延续任务关联。
- [x] 真实双 Engine 正常、写入失败、慢写入链路验收。
- [x] Application 剪贴板 399 项、Infra 剪贴板 23 项、Engine 观测 2 项回归。
- [x] 观测合同与运行时 50 + 42 项测试，包括远程失败/慢请求不阻塞。
- [x] 本地 Collector 配置校验、8 个节点与 8 条日志、Jaeger 页面实看。
- [x] 架构与隐私检查、格式与差异检查、完整 metadata。
- [x] 最终全目标编译；通过，保留现有非本任务 unused/dead-code 警告。
- [x] 最终已改源复跑双设备正常链路，1 项通过；最终 fmt 与 diff check 通过。

图片/文件在本轮仅跑既有 Application 回归，未执行其专属双设备完整观测验收；实际 macOS/iOS/Android/HarmonyOS 宿主与真实 PostHog 投递均跳过，不将合成宿主双 Engine 测试冒充平台验收。
