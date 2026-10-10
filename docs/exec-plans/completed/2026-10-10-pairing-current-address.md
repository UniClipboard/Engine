# 配对材料使用当前传输地址

状态：已完成。问题来源：Engine #166；基线为 `0e25f4189301efd68c21c8ffdd51a2f9fbfd4204`。

## 责任、结果与恢复

Application 的 `SpaceAdmissionProtocol` 继续独占配对流程；调用方只执行现有 JoinSpace/邀请操作。
Infra 的 Joiner 初始材料与 Sponsor 候选准备能力在生成相应协议材料时读取同一存活 Iroh endpoint 的当前地址。
Engine 仅装配所需 endpoint 依赖，不读取或编排材料阶段。已有稳定 facade、port 与协议格式不变。

成功：实际配对前已就绪的 Relay 进入 Joiner 签名身份事实和 Sponsor continuation route。
编码失败：由相应材料 port 的 Unavailable 保留完整 source chain 向 Application 返回；不回落旧地址。
Relay 尚未就绪：使用此刻已有的地址（支持显式 LAN-only）；不增加无界等待或隐式模式切换。
材料一经签名/持久化，重放与重启恢复沿用既有材料，不在签名后改写载荷。后续稳定地址更新仍由已确认成员历史的既有路径负责。

## 最小切片与验收

1. 在现有独立进程、Linux namespace、本地真实 Relay 的连接恢复 E2E 中新增当前地址场景，先证明旧实现失败。
2. 删除 Engine 组装中的地址字节与 continuation route 快照；材料 adapter 持有 endpoint，并在生成材料时读取地址。
3. 验证首次对端 stored 地址、所有准入路由均含就绪 Relay，禁止 UDP/发现后仍可完成配对及双向接收。
4. 完整销毁并重建接收方进程，保持身份及密文资料，再验证双向接收；生成脱敏 JSON 工件与二进制来源。
5. 检查现有 LAN-only 独立进程与相关材料/协议验证，再运行根交付门禁。

失败模式：Relay 未就绪、编码失败、旧 endpoint/身份不匹配、缓存/反向连接掩盖、后台地址补齐掩盖、continuation route 仍过期、LAN-only 被意外等待或拒绝、签名后载荷被改写。
本次不新增单元测试；已有构造调用方按必需依赖迁移。Windows/iOS 物理设备与真实公网/NAT 未执行时标为跳过。

## 验收结果

- 同一 Linux E15 脚本先跑基线旧二进制：首存 Joiner relay=0，四次准入路由中三次 relay=0，地址断言按预期失败；正常同步和进程重启同步均成功。
- 修复产物：首存 Joiner relay=1，四次准入路由全部 relay=1；14 次 Relay 连接、零直连；双方文本逐字读回及 Sponsor 完整进程重建后双向读回成功，endpoint 身份保持。
- 显式 Relay disabled 的三独立宿主对照：四方向文本、两方向文件字节核对成功；完整配对为必需前置步骤。
- locked metadata、最终 workspace all-targets check、格式、Rust 风格、Engine 仓库门禁、Node 语法及 diff 检查通过。
- Linux 为 Docker Desktop VM 中的隔离 namespace；真实公网/NAT、物理 Windows/iOS、现场 suspend/resume 和历史两个故障 SHA 构建跳过。
- MBX 在最终 Linux 构建中报告一次缓存还原权限错误并触发较多重编译；最终构建成功，未绕过缓存。自有容器与空挂载点已删除，共享 target 与宿主缓存保留。
