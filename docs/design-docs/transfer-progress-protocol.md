# 反向传输进度的线上合同

当前 `/2` 编解码已实现、尚未发布，生产 reporter／Router 尚未迁移。
业务准入、任务登记顺序、结果采用及恢复由 Application 完整负责人承担；实施与验收见
[第八片规格](../exec-plans/active/2026-10-24-transfer-progress-connectivity-spec.md)。

## 版本及事实来源

- 已发布旧协议为 `uniclipboard/transfer-progress/1`：每条单向流一个 34 字节帧；旧字节和向量保持。
- 新协议为 `uniclipboard/transfer-progress/2`，支持双向登记、进度、精确终态确认及任务核对。
- `/1` 在第八片开工前已存在。首层受管单向流 PR 没有新建 `/1`；后续 stack PR 完善同一未发布 `/2`，不按任务增加 `/3` 等版本。
- [消息模型](../../crates/uc-sync-protocol/src/transfer_progress/v2/model.rs)中的单源数值枚举与
  [固定向量](../../crates/uc-sync-protocol/tests/fixtures/transfer-progress-v2-vectors.txt)约束实际字节；
  已发布格式变更必须按协议兼容规则演进，不能只修改旧向量放行。

本片没有新增持久格式、通知 outbox 或 blob／V3 envelope 版本。

## 帧边界和观测载体

一帧为：

```text
UCP(3 bytes) | version=2(u8) | body_length(u32 BE) | body
body = context_present(u8) | [context_length(u16 BE) | traceparent] | message
```

完整帧最多 64KiB，正文长度必须为 1–65528；读取方先检查 8 字节头部，再分配正文。
`context_present` 只能为 0 或 1，存在时 traceparent 为 1–256 字节 UTF-8，长度核验先于载体分配。
复用现有 `WireTraceContext`；这里只承载字节，Infra 才负责从当前 span 注入以及认证后采用有效上下文，
Application 消息、Core 规则及 Engine facade 不获得观测字段。载体的 W3C 格式与父节点合法性不由此 codec 授权。

`decode_frame` 必须消耗整个输入，拒绝尾随字节；`read_frame` 每次读取一个有界帧，可在同一双向流顺序读取登记握手的多帧。
完整交换负责人限制消息顺序、帧数、最终流结束与截止时间，不能将合法握手多帧误作尾随字节，也不能把任意后续帧当成合法业务消息。
编解码不负责拨号、刷新期限、重试、成员授权、结果采用或自动生成 ACK。

## 身份、游标及结果

所有定宽整数为大端。ID 只在本次协议交换内流转，日志和普通 Debug 不展示其原值。

| 结构 | 线上字段顺序 |
| --- | --- |
| TaskScope | source_entry(16)、receive_attempt(16) |
| ProducerDescriptor | producer(16)、owner_revision(u64)、TaskScope、item_count(u16) |
| LogicalItem | kind(u8)、原始位置 index(u16) |
| ActiveProgress | LogicalItem、round(u32)、sequence(u64)、bytes(u64)、可选 total、phase(u8) |
| TerminalResult | LogicalItem、round(u32)、sequence(u64)、result_revision(u64)、bytes(u64)、可选 total、outcome |
| 可选 total／snapshot_revision | present(u8)，为1时跟 u64；None 与 Some(0) 不混淆 |
| SnapshotPage | snapshot_revision(u64)、offset(u16)、total(u16)、count(u16)、逐项类型及记录 |

消息 tag 和原因码以模型中的声明为唯一数值清单。当前合同包含：

- Register → Challenge → CurrentDescriptor → Bound 所需字段。Bound 回显精确 descriptor 和挑战并提供 binding；codec 不判断挑战是否新鲜、producer 是否已退休或描述是否权威。
- Update 与 Contact 绑定 binding、TaskScope 和登记修订。Contact 有独立新消息序号，不增加字节；实际有效性和期限由 owner 判断。
- Terminal 与 TerminalAck 绑定同一任务并携带完整 TerminalResult，避免只确认条目 ID 或错误结果版本；有 ACK 帧类型不等于结果已经采用。
- Query 携带挑战、源条目、可选预期 attempt、分页 offset 与可选快照修订；QueryResult 区分 Current、Superseded、NotRetained、Unsupported、AccessDenied。
- Rejected 仅含固定原因；实际 IO、存储或解析错误不由 codec 伪造为 NotRetained／Unsupported。

业务接收设备来自认证连接，不信任自报设备名或地址；Space／会话世代绑定在完整负责人上下文内，不由源条目 UUID 授予访问权。
来源设备与实际 blob provider 可以不同，这些线格式字段不将二者合并。

## 分页与拒收边界

一个描述有 1–1024 个逻辑结果项；目录结果按完整单元描述，其 index 必须为0。
一页有 1–64 项，`offset + count <= total <= 1024`，total 与对应 descriptor 的 item_count 一致。
页内不能重复 LogicalItem；item index 必须小于1024。读取方检查 count 后才创建记录集合，不能按未验证数量分配。
后续页请求必须携带快照修订；首批请求可没有修订。

codec 只检查当前帧结构。跨页修订一致、页覆盖完整、原始清单身份匹配、跨页重复、旧尝试和旧轮次隔离，
均由后续完整负责人验证，不能仅凭一页已解码就宣称整批结果已确认。
网络字节计数与真实交付状态的业务一致性也不能交给 codec 猜测。

未知 magic／版本／tag／原因码、截断、尾随字节、非法 option、超限正文、超额页计数、非法目录索引及页内重复均拒收。
IO、UTF-8 和未知数值错误保留具体 source；错误显示及 Debug 采用固定分类，不输出原始帧、身份或载体正文。

## 验证及当前限制

```bash
just cargo test -p uc-sync-protocol --tests --locked
```

当前有36条无观测载体的完整消息／状态／原因固定向量和1条带载体向量；15个新增用例还验证每条向量的全部截断前缀、
额外字节、页计数、精确帧大小边界、最大合法页、未知／零总量、原因映射、变体级固定错误分类及完整来源、载体界限。
旧协议测试和旧 golden 向量继续执行。

这些结果只证明字节合同和有界编解码。真实任务登记、生产终态采用、ACK 时点、补发、成员撤销、Router 迁移、
独立进程网络及平台矩阵继续以第八片执行计划为准。
