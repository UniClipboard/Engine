# 发现

- 出站 `ObservedClipboardDispatch` 与 Infra 接收端已有在线父子关系。
- `ClipboardReceiverPort::subscribe` 传输 `InboundClipboard`，只有 peer/header/ciphertext/transport/receipt，不包含调用上下文；广播交接丢失当前 span。
- Application InboundProcessor 在独立任务中消费，负责策略、解密、apply 与 receipt。
- apply 保存后安排后台系统写入，当前 `.in_current_span()` 无法恢复此前广播边界丢失的关系；不能为了观测把写入改成阻塞网络回执。
- 用户确认较小方案：保留广播，在 Application 接收合同上使用不透明任务信封。不使用内容散列、内存地址或全局 registry 拼接链路。
- 第一轮实现后测试发现发送端 JoinSet 也是独立任务边界，已使用相同不透明上下文延续。上下文只持有不可记录的父身份，不持有原 span，因此异步尾部不拉长原操作。
