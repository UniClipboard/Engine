# 新会话发布前的出站连接被本机拒绝

## 现象

Space 切换或启动时，`ProductionSessionFactory::prepare` 先执行 `ApplicationRuntime::start`，
`install_active_session` 随后才调用 `IrohNode::activate_session` 发布会话代际。`start` 启动的后台工作
（成员维护 `Startup` 一轮中的准入恢复、连接维护等）立即发起受管协议的出站连接，此时注册表没有当前代际，
`SessionProtocolEndpointHooks::before_connect` 返回 `Reject`，连接以 `locally_rejected` 失败。

加入流程中，joiner 的最终确认（CompleteAck）第一次连接因此必然失败，约 1 秒后由准入恢复重试成功。
每次加入都因此多等约 1 秒。

证据（`committed_final_confirmation_replays_success_after_the_first_reply_is_lost` 日志）：

| 时间 | 事件 |
| --- | --- |
| 22.7087 | `session_prepare` 开始 |
| 22.7208 | `session_start`（`ApplicationRuntime::start`）中准入恢复读取 joiner 状态并拨号 |
| 22.7219 | `connection.finished` `locally_rejected` |
| 22.7221 | `session_prepare` 结束，其后才 `activate_session` |
| 23.75 | 重试，CompleteAck 交换成功 |

plan 043 规定先启动 Application、再整体发布代际（启动失败时丢弃未发布代际），因此不能简单调换两步。

## 已验证但未合入的方案

`outbound-wait-for-publication.patch`：注册表记录“正在准备的下一代”
（`SessionProtocolPreparation`，由 `IrohSessionBuilder` / `PreparedIrohSession` 持有）。
准备期间受管协议出站连接等待发布：发布后放行，放弃准备后拒绝，最长等待 10 秒；没有下一代在准备时仍立即拒绝。

- 效果：上述 e2e 由首连失败加 1 秒重试变为首连成功，`locally_rejected` 为 0；`uc-infra network::iroh` 260 项通过。
- 未合入原因：`testing::host_adapter_contract::offline_lifecycle::crash::interrupted_file_transfer_recovers_after_receiver_process_restart`
  失败率由 0/6（另有 5/5 通过）升至 4/6。

## 暴露出的既有问题

基线在带日志运行时也失败过 1 次，失败过程与上面相同，说明以下问题原本存在，方案只是改变时序使其更易出现：

1. 接收方以同一身份、同一端口重启后，发送方对其的新连接在握手阶段连续超时（3 次共约 4.5 秒），
   同时发送方到旧进程的分发流仍未结束（约 65 秒后才以 `stream_failed` 结束）。
2. 发送方的剪贴板投递恢复在对端已在线时只尝试一次，失败记为 `offline` 后，60 秒内不再触发重试。

## 退出条件

- 先查明问题 1、2 并修复，再重新评估发布前出站连接的处理方式。
- `interrupted_file_transfer_recovers_after_receiver_process_restart` 连续运行不少于 10 次全部通过。
- 加入流程的最终确认首连不再出现 `locally_rejected`。
