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

## PR 审查修订

完整材料生成仍由现有 Joiner/Sponsor 准入 adapter 负责，调用方仍只调用原完整动作。
Iroh adapter 实现动作专属的材料 port：Joiner 一次快照同时生成指纹、公钥和地址，Sponsor 每次生成当前继续路由。
失败模型：启动快照陈旧、身份与地址采样不一致、编码错误丢失 source、具体 Endpoint 越界、禁用 Relay 对照退化、签名材料重放被改写。
错误沿原动作错误类型返回；失败由原准入流程负责重试，已经签名或持久化的材料不重写。
先迁移已有 fixture，再修改生产代码；不新增单元测试。重新运行 E15 与 LAN 对照并保留工件。
CI 安全扫描另发现 `libcrux-kem 0.0.9` 的 RUSTSEC-2026-0330 / 0331。直接升级被 HPKE 的精确依赖约束拒绝，故采用官方上游修订，不忽略公告。

### 安全扫描依赖修订

生产、Profile 与独立验证的五个既有 OpenMLS 依赖共用官方固定提交
`e0b21a70a55a9014b8de2772b36bf3d638adad51`，该来源将 HPKE 升至 0.8。
来源写入实际 workspace 依赖，确保下游 Cargo 消费者同样使用修订，不依赖根工作区 patch 或产品仓补丁。
保留无关依赖的已锁定版本；只有新密码栈所需的依赖发生演进。

修改前失败模型：共享 traits 来源不一致、旧安全状态不可读取、签名或 HPKE 编码互不兼容、不同 provider 导致入组或重连失败、锁文件保留易受攻击版本、下游忽略根 patch 导致构建来源漂移、工具链或绑定不兼容。
既有验证 fixture 先适配上游带擦除语义的导出密钥结果，再修改生产 adapter 的两个密钥提取点；标签、长度和业务格式沿用原值，不新增单元测试。

验收覆盖完整 OpenMLS 可执行场景、全工作区编译、安全扫描、隔离网络双向文本/文件、正常 E15，
以及旧 Sponsor / 新 Joiner 配对后用新宿主读取旧 Sponsor 加密 profile，并执行反方向混合版本 E15。
`--restart-host` 仅用于 E15 的独立进程升级验收，不改变产品配置或准入流程。
源码、上游来源、各版二进制 SHA256、网络隔离与清理结果保留于本线程验收工件。
