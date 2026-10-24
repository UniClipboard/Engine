# 第八片阶段 0：受管单向流接缝证明

状态：**单向流最小接缝已证明，阶段 0 整体尚未完成**（2026-10-24）。
需求见[实现规格](2026-10-24-transfer-progress-connectivity-spec.md)，推进见[执行计划](2026-10-24-transfer-progress-connectivity-plan.md)。
开工基线为 `5cbca8fc24a278fa7b01ae611bf4b925914bef13`；本记录描述该基线上的候选变更，
不将主机探针当作完整生产迁移、独立进程网络矩阵或真机验收。

## 能力缺口及最小改动

原 `ConnectionLease` 只有双向开流／接流。新增探针在原接口上得到 9 个缺少单向方法的编译错误；
这是新增能力缺失证据，不是已经运行的单向功能发生行为回归。

当前候选新增可选 `PeerUnidirectionalConnection` 能力；Iroh bridge 提供原生单向半流的内部包装，
双向 backend 可以明确不支持。`ConnectionLease` 共用准入和出站额度获取，单向租约也持有同一工作使用权。
旧 `/1` ALPN 常量由协议 crate 统一维护，旧帧编码及 golden 向量未变。
新协议类型在不相关组更新处理器中明确拒绝，没有用通配分支放行。

旧进度 reporter、其生产连接池、Engine 发送侧投影和 Router 尚未迁移；新增能力不代表这些任务已经完成。

## 真实主机探针及结果

探针位于 `crates/uc-infra-p2p/src/network/iroh/connectivity/stream_progress_probe.rs`。
使用同进程两个真实 Iroh endpoint、旧进度 ALPN、已建立连接和实际 Uni manager／成员身份准入；
不经过生产 Router，也不证明发现、Relay、独立进程或完整接收业务。

| 验证 | 已观察结果 |
| --- | --- |
| 同连接两条单向流，停止第一条 | 第二条旧进度帧精确读回，两端连接仍打开 |
| 停止网络但保留本地工作使用权 | 第三次开流在额度期限内失败；释放实际工作使用权后可再次开流 |
| 已有连接上的成员资格撤销 | 开流前重新读取权威并拒绝，不因连接已认证而继续发送 |
| 已取消的等待者 | 返回停止，接收端未收到新流 |
| 准入读取被受控暂停后取消 | 调用仍等待实际读取，额度不提前释放；读取结束后返回停止且未开流 |
| 入站额度满时接到另一流 | 仅停止超额流，原流可继续写入并完整读回 |

五个真实探针通过；其中首个同时验证两流隔离及实际工作额度，因此表中共有六个行为断言。
Application 另验证不支持单向流时有来源分类且不消耗单次双向交换资格。
原两个真实 SDK／Uni blob 流测试继续通过，覆盖同内容等待者及普通文件的取消隔离。

## 验证入口

从仓库根目录串行执行并复用既有 target／缓存：

```bash
just cargo test -p uc-application --lib 'connectivity::' --locked
just cargo test -p uc-infra-p2p --lib 'network::iroh::connectivity::stream::' --locked
just cargo test -p uc-sync-protocol --tests --locked
```

本轮结果：Application 连接测试 113／113；真实流测试 7／7（5 个新探针、2 个原 SDK 流回归）；
协议测试 60／60，含原 `/1` golden 向量。原始逐轮日志、源码／锁文件指纹与消费点清单保存在本地工件，
不把第一次编译缺口和后续成功结果混为同一运行。
本轮 metadata、workspace 全目标 check、fmt、Rust style、repository 和 diff 根门禁均通过；
workspace 检查有既有未使用导入／测试字段告警，没有新增告警作为通过条件的豁免。

## 尚未证明的范围

- 新版任务登记、producer 切换及旧消息隔离，尚未完成协议或真实双端证明。
- 完整接收输出后的 Application 终态采用与新 ACK、有限补发和状态核对尚未接线。
- 最大负载、分页、终态淘汰、公平调度及新旧双角色 Direct／Relay 尚未执行。
- Linux 当前头文件合同及 BOOTTIME syscall 已核对；真实系统挂起和其他平台尚未验证，不记跨平台经过时间证明通过。
- 完整 workspace 行为回归、平台编译与设备矩阵仍按正式计划执行；本片尚未整体完成。
