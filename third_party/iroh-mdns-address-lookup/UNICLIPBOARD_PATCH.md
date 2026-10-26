# iroh-mdns-address-lookup 的 UniClipboard 补丁

本目录来源于 crates.io 的 `iroh-mdns-address-lookup` 0.6.0，许可证为 `MIT OR Apache-2.0`。

## 本仓变更

- `Cargo.toml` 的 `swarm-discovery` 指向 `../swarm-discovery`；移除 example/dev-dependencies 并与 Engine workspace 分离，避免下游重复 `[patch]`。
- 地址变更替换 occupied 缓存；显式 resolve 不抑制发现订阅，迟订阅重放当前快照。线格式保持兼容。
- `Item.last_updated` 反映原观察时间，目录读取缓存不重新续期；时间无法确定的 mDNS 观察不进入目录。
- 增加共用 actor 的显式 shutdown/abort：正常关闭等待任务析构并保留 JoinError，克隆不再使关闭后的服务继续广播。
- 队列满的固定诊断不输出候选身份或地址。
- 构建器增加 `multicast_interfaces_v4(MulticastInterfacesV4)`：启动时读取一次接口来源并交给 swarm-discovery 的多接口 IPv4 模式；启动后以及每次本机发布地址变化时，把最新快照作为期望集合交给 swarm-discovery 的 `set_interfaces_v4`。本包装层不记录已安装接口；接口安装、失败重试以及移除时停止接收者都由 swarm-discovery 负责（见其 `UNICLIPBOARD_PATCH.md`）。未设置时保持上游的默认路由接口行为。
- 长期actor的span显式无parent；启动acto发现服务时不捕获短期调用span，防止延长启动计时或在跨subscriber的取消析构中释放错误parent。实际发现轮询和网络诊断仍正常运行。

这些行为由 Engine 的 `mdns_discovery_contract` 验证；目录自身用有限 TTL 处理丢失的订阅事件。

## 来源与删除条件

`[patch]` 仅从构建根 manifest 读取，因此 Engine 以路径依赖持有两个随仓库源码副本，Desktop 等下游无需重复补丁。只有 swarm-discovery 的已有修复以及本文件列出的发现/关闭契约均获得上游支持后，才可移除副本；替换前重跑 Engine 的 adapter 与真实网络验证。
