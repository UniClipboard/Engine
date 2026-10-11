# Issue #34：发现模块第一个切片

状态：完成（仅 Issue #34 第一个发现切片）。

## 已确认范围与所有权

- Application 的 discovery 模块统一拥有来源观察、目录合并、候选失效和按需查询；Infra 提供 mDNS/云端 provider 与 iroh 地址查找桥；设备级网络节点拥有运行与关闭。
- 调用方只向既有连接能力提出连接意图。原有 iroh 路径选择、密码身份验证、成员授权、ALPN 和业务重试负责人保持各自职责。
- 允许发现陌生设备；目录以 Uni 自有不透明传输身份索引，不伪造逻辑 DeviceId。产品附近设备列表与陌生设备自动拨号不在本切片。
- mDNS 持续收集，DNS/PKARR 云端地址解析按需执行；邀请码 rendezvous 不属于设备地址发现。
- 成功返回当前有效候选；一种来源失效或查询失败不移除其他来源。查询错误保留来源。取消查询停止该查询，节点关闭停止并等待发现任务。
- 目录仅在内存中，不新增持久化格式。重启重新发现；Space 切换保留网络候选，每次身份映射与授权仍使用当前会话的既有负责人。

## 最小切片及失败矩阵

先固定目录/调度契约测试与现有真实拨号基线，再接入生产 lookup：

1. 同一传输身份的 mDNS/Cloud 共存，单来源更新替换自己的地址；丢失、TTL、重复丢失不影响另一来源。
2. 云端失败/慢查询不阻塞本地候选；取消没有后台查询遗留；provider 结束只移除本来源。
3. 陌生身份可发现；目录容量和 locator 大小有界，不输出地址、公钥或逻辑身份。
4. mDNS 已显式 resolve 时仍发布观察，地址变化覆盖旧缓存，订阅获得当前快照；掉包后候选不能永不失效。
5. 生产节点通过目录 lookup 建立实际双向连接；LAN-only 不启动云端 provider。
6. 沿用 connectivity host/network namespace/relay harness 验证真实同步，不以合成候选测试冒充真实 LAN/Relay 验收。

## 验证与环境

- 定向 Application/Infra 测试与现有真实拨号回归。
- 根 AGENTS.md 要求的 metadata、workspace all-targets check、fmt、Rust 风格、仓库架构与 diff 检查。
- 独立进程 network harness 需要 Linux namespace 权限；`sudo -n true` 要求密码，但 `unshare -Urnm` 可用；通过私有 mount namespace 中的 `/run` 执行现有 harness，不修改宿主网络。
- 真机、Mobile/Desktop 实际宿主、新旧发布互通与发布产物验收均需单独记录，不能以本机测试替代。

## 当前记录

- 基线源码：`007a5a20`。现有 mDNS 会忽略 Expired 的连接提示；目录尚不存在。
- 既有 mDNS adapter 对已主动 resolve 的端点抑制被动发现，且 occupied entry 使用 `or_insert` 不替换已变更地址；复用库时须补齐此行为。
- 用户提供的维护规则要求检查 architecture-bible；当前仓库该文件已不存在，按要求恢复为指向唯一设计正文的维护记录入口，不复制架构事实。

## 已执行的回归

- 生产接入前，以 `007a5a20` 的宿主执行真实三节点配对与 E02 双向文本/文件传输基线；宿主副本与 SHA 保存在 `target/discovery-evidence/baseline/`。
- 真实 mDNS 新增回归先复现两个失败：显式 resolve 抑制订阅、地址更新仍返回旧缓存；修复后订阅、更新/快照、所有克隆共同关闭三个契约通过。
- 74 项定向 nextest 通过：目录、生产 lookup、既有可达性/授权/撤销、关闭与诊断枚举；其他测试因筛选未执行。
- 最后复核新增旧观察不能重新获得完整寿命的回归，先失败后修复；16 项定向复验通过，包含全部 mDNS 契约和切 Space 后复用 endpoint。
- 根目录 metadata、workspace all-targets check、fmt、Rust 风格、仓库架构与 diff 检查全部通过。全工作区检查保留了原有 HarmonyOS public_contract 与 sync-protocol golden_vectors 的未使用导入警告；未改动范围外代码。

## 真实网络验收

使用现有 `scripts/testing/connection-recovery-network.mjs`，独立 OS 进程与 Linux namespace，所有场景 `--repeat 1`；宿主从共享 `target` 构建。隔离网络使用本地 Relay，不把公网 DNS/PKARR 成功解析记为通过。

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| 修改前 E02 文本/文件 | 通过，221 / 288 ms | `target/discovery-evidence/baseline/direct.json` |
| 最终 E02 文本/文件 | 通过，183 / 278 ms | `target/discovery-evidence/final/direct/direct.json` |
| 最终 E04 网络恢复提示 | 通过，8820 ms（含故障等待） | `target/discovery-evidence/final/recovery/direct.json` |
| 最终与 `007a5a20` 混合 E02 文本/文件 | 通过，226 / 279 ms | `target/discovery-evidence/final/mixed/direct-mixed-1.json` |
| E10 直连期间 Relay 中断 | 通过，23133 ms（含持续观察） | `target/discovery-evidence/final/relay/relay.json` |
| E10 屏蔽直连后 Relay 中断/恢复 | 通过，36580 ms（含故障等待） | 同上；恢复预算仍由既有 harness 断言；Relay 就绪后 348 ms 在线、482 ms 完成双向传输 |

四份最终网络证据均 `failed: false`、`cleaned: true`、`plaintext_clean: true`，保留构建 SHA、重放命令、故障证明与内容读回结果。最终构建与源码 SHA 清单位于 `target/discovery-evidence/final-build-manifest.json`。完整重放脚本与门禁输出位于 `target/discovery-evidence/verify-final.sh` 和 `target/discovery-evidence/final-verification.log`，这些属于本地可再生产物，不提交仓库。

## 跳过与后续边界

- 跳过：公网 Cloud 服务成功解析、物理 Desktop/Mobile 设备矩阵、已发布旧版本互通、GitHub Release 产物验收。本切片没有发布操作。
- 混合版本仅指本次修改前后的真实宿主；不代表已发布版本或协议迁移验收。
- 未实现：Connection/ConnectionPool 抽象、发现结果产品列表、邀请码发现、协议迁移及整个 Issue #34 的其余切片。
