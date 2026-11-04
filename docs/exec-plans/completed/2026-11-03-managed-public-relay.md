# 匿名受管公共 relay（#371）

状态：Engine 实现、本地验收与真实受管 relay 连接验收完成。真机与发布仍然跳过。

## 2026-11-03 修订：内置 relay 访问分类

以下实施最初用上海受管候选替换了四个公共 n0 内置 relay。最终设计保留四个 n0 relay
作为绑定时安装的直接内置，并在凭据签发后追加受管上海 relay。目录替换只作用于受管候选；
Cloud 故障或凭据失败时，直接内置 relay 仍然可用。

## 所有权与契约

既有网络节点持有一个受管 relay 运行时。宿主仍按原方式启动与停止 Engine。Core 决定
禁用、Custom 或 BuiltIn 路由。Infra 运行时负责 Cloud HTTP、内存目录、凭据、续期与
relay map 协调，它不编排业务工作流。不需要账号、持久化令牌、共享密钥或新的稳定 Engine
API。

禁用与 Custom 路由永不访问 relay 控制面。BuiltIn 路由以上海候选（`cn-shanghai-1`、
`cn-shanghai`、`https://sha1.uc-relay.z2blog.com/`）开始。有效目录替换该候选，包括
空目录。请求失败时保留上一份有效目录。只有凭据响应才授权目标地址。凭据绑定当前
endpoint 与请求的稳定 relay ID。凭据响应地址是权威来源，必须是既不带用户信息也不带
查询的 HTTPS 源。

单个串行 worker 持有缓存，并且最多持有一个选中的受管 relay。目录最多 32 个节点、
64 KiB；凭据最多 16 KiB，其中令牌最多 4 KiB。HTTP 调用超时五秒并拒绝重定向。目录
刷新最多每 30 秒一次。续期在最后两分钟之前开始。失败使用有界指数退避，遵循有界的
Retry-After，且永不阻塞直连。凭据在内存中过期。关闭时在关闭 endpoint 之前取消并 join
worker。重启使用当前身份重新请求凭据，没有持久化恢复状态。已安装的传输配置同时携带
凭据的绝对过期时间，每次重连都核对真实时钟。状态变化会唤醒 worker，因此挂起后的重连
即使未到单调时钟续期期限也会请求续期。被移除的 relay 连接方退出，不会累积。

## 实施前枚举的失败模式

- Cloud 超时、重定向、非 JSON、超大响应体、重复 ID、非法源、非法时间戳、空令牌或过期
  凭据。
- 429 与 Retry-After、403 封禁、503 不可用以及连续中断。
- 全新的空目录、节点被移除、节点地址变化以及凭据响应地址不同。
- 身份替换、凭据续期、续期后重连，以及凭据签发不可用期间过期。
- 通过 Debug、错误、设置、工件或 relay 地址泄露令牌。
- 既有 iroh relay 连接方保留构造时复制的令牌，仅更新 RelayMap 不会更新这些连接方。
  网络栈依赖必须先观察配置变化，该功能才能安全续期。
- HTTP 期间取消、部分构造以及正常节点关闭。
- 选择自定义 relay 或仅局域网时误启动 Cloud 工作。

## 验收

实施前先编写传输与控制面验收测试。先证明在真实带认证 relay 连接上的凭据替换，再通过
真实节点生命周期验证模拟 Cloud HTTP，覆盖目录替换/移除、身份变化、续期、过期、中断与
路由优先级。产出脱敏且可重复的验收工件。通过 `just cargo` 与共享 target 串行执行仓库
交付检查。未实际执行的真机、跨网络路由与发布保持跳过。真实 Cloud 检查永不打印或
持久化凭据。

## 已执行验收

- 受管控制面与节点验收：10 项通过；真实场景是单独的显式选择项。
- 真实带认证 relay：凭据替换、过期门禁与移除通过。既有静态令牌传输验收同样通过。
- 节点回归：41 项通过，一项既有忽略场景；该回归命令不包含真实场景。
- 完整 Engine 设置验收：四项通过，包含在真实 SQLite 与宿主安全存储适配器上经
  BuiltIn、Custom 与禁用路由的实际重启。
- 锁定元数据、workspace/全部目标检查、格式、Rust 规范、仓库架构与空白检查通过。
- 受管 worker 的观测验收：目录请求失败时，失败告警确实进入当前进程的记录员，而不是在
  任务启动时被静默隔离。
- 真机、跨网络设备矩阵、发布与资源基准跳过。容量边界属于设计边界，不是已测量的内存
  或 CPU 改进。

iroh 修复固定在
[`65f265d1b64114f500be1eeca90763de535be212`](https://github.com/UniClipboard/iroh/commit/65f265d1b64114f500be1eeca90763de535be212)，
发布在 `UniClipboard/iroh` 的 `fix/relay-credential-refresh` 分支。它不新增生产日志
调用，并在 Debug 中屏蔽 relay 令牌。其配置通知只保留一个变更信号，不保留历史凭据。

## 真实验收

2026-11-03 首轮运行安装了匿名凭据，但未在 45 秒内选中 home relay：公共网关对 `/ping`
返回 404。网关部署补上 `/ping` 后，同一 Engine 提交
`de9edfac71dfabe2a3aa5fc417ad16e73333d1cd` 在未改动 Engine 代码的情况下以 5.59 秒
通过真实验收，公共 `/ping` 返回 200。

真实节点安装了匿名凭据并建立 relay 连接。其状态只包含一个 relay，没有认证拒绝，也没有
relay 错误；关闭完成并 join 了受管 worker。这证明匿名签发与 relay 连通性，不证明剪贴板
负载传输、面向真实服务的凭据续期或真机行为。Engine 侧另有真实断言：凭据签发后概览把受管
relay 标成已生效，四个直接内置 relay 保持生效，生效地址互不重复，且没有待重启变更。

从仓库根目录复现这项聚焦检查：

```bash
UC_MANAGED_RELAY_ARTIFACT_DIR="$PWD/target/managed-relay-live-retry" \
  just cargo test -p uc-infra-p2p \
  live_managed_relay_accepts_current_endpoint_credential --lib --locked -- --ignored
```

重试在 `target/managed-relay-live-retry/` 下产出脱敏的 `live.json`、`live.log` 与绑定
源码的清单。更早的失败运行单独保留。网关路由阻塞已解决。真机、跨网络设备矩阵、发布与
资源测量仍然跳过。

## 复现与工件

本地验收运行 `bash scripts/testing/check-managed-relay.sh`。设置
`UC_MANAGED_RELAY_LIVE=1` 可包含真实 Cloud 与 Relay 场景，脚本会同时运行 infra 侧与
Engine 侧两条真实断言。脚本在 `target/managed-relay-acceptance/` 下写出脱敏 JSON、日志、
源码哈希与清单。真实场景失败时保留证据并返回失败退出码。

运行脚本串行复用仓库 target。在两个 Cargo workspace 之间切换时，需要定向重建 iroh-relay
产物：其 cdylib 与 rlib 使用无哈希文件名，否则一种功能集可能覆盖另一种功能集的产物别名。
运行脚本保留 MBX 动作缓存，只在每个 workspace 边界重建该包。

一次最终重复的 workspace 检查在该编译器进程运行前遇到瞬时的编译器 shim 启动失败
（`ENOENT`）；复查时同一 shim 路径存在，重复未改动的检查在 13.44 秒通过。中断与成功的
日志都已保留；这不是源码编译失败。共享 MBX wrapper 始终启用。
