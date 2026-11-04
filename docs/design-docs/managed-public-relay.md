# 匿名受管公共 relay

默认 P2P relay 由两类内置 relay 组合而成。公共 n0 relay（`na-east`、`na-west`、
`eu`、`asia-pacific`）属于直接内置：Core 在绑定时安装，不需要凭据。上海 relay
（`cn-shanghai-1`，区域 `cn-shanghai`，`https://sha1.uc-relay.z2blog.com/`）属于
产品自建受管 relay：它必须先向 Cloud 控制面取得短期匿名凭据，且不需要 Cloud 账号。
两类内置 relay 都由 Core 持有，路由优先级仍为
禁用 > 自定义 > 内置。受管候选既不是访问凭据，也不是永久目录条目。

## 所有权与生命周期

Infra 网络节点持有一个串行的受管 relay worker。Engine 装配层只在 BuiltIn 路由下启用它。
自定义 relay 继续使用既有的安全存储凭据。仅局域网与 Custom 路由永不请求受管 relay
目录或匿名凭据。

worker 最多每 30 秒读取一次
`https://cloud.uniclipboard.app/api/relay/v1/nodes`。有效响应整体替换内存中的受管候选
目录；直接内置 relay 不属于该目录。空响应会移除受管 relay 并保留直接内置 relay。
请求失败时保留上一份有效目录，包括此前为空的目录。重启以随包发布的受管候选开始并重新
请求凭据；既没有磁盘目录缓存，也没有持久化凭据。

只要上海仍在有效目录中，worker 就优先选择它；否则选择目录中的首个候选。它使用选中的
稳定 relay ID 和当前 endpoint 的小写 64 位十六进制 ID 向 `/credentials` 请求凭据。
即使凭据响应中的地址与目录地址不同，响应地址也是权威来源。只接受不带用户信息、路径、
查询或片段的 HTTPS 源。HTTP 重定向被禁用。

在十分钟凭据有效期的最后两分钟之前，worker 会请求续期。续期失败时提前撤下已安装的
凭据。它从不把缓存地址当作授权。目录选择或凭据目标地址变化时移除旧的 relay 配置。
凭据不会被发送给另一个 relay。网络栈的 RelayMap 变化会唤醒活动连接并替换其客户端。
每次重连都读取最新配置；已被移除的凭据无法被该连接重用。
已安装的传输配置同时带有绝对过期时间，每次重连拨号前都会核对真实时钟。relay 状态变化
会唤醒 worker，使其在核对单调时钟续期期限之外，同时核对真实时钟续期。因此挂起不会把
过期凭据变成可重连权限。被移除的连接方退出；等待续期的连接方只属于当前已安装的 relay。

单个 worker 为目录与凭据请求分别维护有界退避：五秒起翻倍，上限五分钟；403 等待五分钟。
429 与其他失败遵循有界的 Retry-After（秒或 HTTP 日期），上限五分钟。目录重试同时遵守
30 秒最小间隔。失败只影响受管 relay：直接内置 relay、直连 P2P 与本地发现仍然可用。
它不会启用 LAN 兼容线，也不会静默改用用户配置的 relay。

节点关闭时先中止并 join worker，再关闭 endpoint。丢弃未完成构造的节点不会启动 worker。
丢弃已运行的节点同样通过其持有的任务句柄中止 worker。

既有 relay 概览接收全部实际安装的地址：绑定时为直接内置 relay，受管 relay 只在凭据
签发后加入。受管候选在凭据签发前不会被标成已生效。目录导致的地址变化不代表存在待重启
的设置变更。受管令牌永远不会变成用户配置的凭据。生效集合会去重，并在每次上报时从直接
内置 relay 重建，因此撤下凭据不会留下陈旧条目。续期成功但目标地址不变时，不重写生效
集合，也不重复记录一次变化。

## 资源与安全边界

每个节点只有一个 HTTP 客户端、一个 worker 和一个选中的受管 relay。没有请求队列，也不
并发调用 Cloud。每个 HTTP 请求超时五秒。目录最多 32 个节点、64 KiB JSON。凭据响应
最多 16 KiB，令牌最多 4 KiB。地址、relay ID、区域与目录版本长度均有上限。

凭据只留在内存中，原生 iroh 仅在 Authorization 头中发送它们。它们不是账号访问令牌。
控制器、RelayConfig 与 relay 客户端构造器的 Debug 输出都会屏蔽令牌。诊断只使用固定
失败分类，永不包含响应正文、令牌、endpoint ID 或目标地址。Engine 中不编译任何共享
relay 密钥。网关侧的签名、身份、有效期与策略校验仍然必需。

## 验收

传输验收使用真实带认证的 relay 服务，验证凭据替换与移除在真实连接上生效。控制面验收
使用真实 HTTP 与真实 endpoint，覆盖目录替换/移除、权威凭据地址、续期、endpoint 身份
变化、退避与非法输入。节点生命周期验收检查路由隔离与 worker 结算。Engine 侧另有一条
可选的真实 Cloud 断言：凭据签发后，relay 概览把受管 relay 标成已生效，四个直接内置
relay 保持生效，生效地址互不重复且没有待重启变更。运行
`bash scripts/testing/check-managed-relay.sh` 执行本地验收。其中的可选真实场景要求已
部署的网关转发 iroh 的 `/ping` 探测路由。仅 Cloud 目录健康不足以证明原生 endpoint 能
选中 relay。

外部契约维护在
[Cloud 的 relay 控制面](https://github.com/mkdir700/uc-cloud/blob/main/docs/RELAY_CONTROL_PLANE.md)。
实施状态属于[执行计划](../exec-plans/completed/2026-11-03-managed-public-relay.md)。
