# ADR-034：iroh 固定到包含路径队列修复的上游提交

- **状态**：已采纳；2026-10-24 DNS 来源补充取代下文初始“不 fork”决定；退出条件见下
- **日期**：2026-10-15
- **范围**：根 `Cargo.toml` 的 `[patch.crates-io]`、`uc-observability-runtime` 网络库日志的路径匹配；不改变任何业务接口
- **部分取代**：[ADR-031](031-product-owned-builtin-relay-list.md) 的补充“依赖补丁不依赖下游 `[patch]`”，仅对 iroh 例外；iroh-blobs 与 swarm-discovery 的处理方式不变
- **来源**：[Engine #214](https://github.com/UniClipboard/Engine/issues/214) 的诊断

## 背景

iroh 1.3.0（当前最新发布版）的 `RemoteStateActor` 在连接停留于 `RemoteCidsExhausted` / `MaxPathIdReached` 时，
每 333 ms 重试并无去重地向 `pending_open_paths` 追加同一批候选地址（`iroh/src/socket/remote_map/remote_state.rs`），
同一对端有多条连接时还会成倍放大。上游已在 `43e232fbcbf11aa9cf03a78ac83f52296f01d6b3`（PR #4551）用包含检查给队列设上限，
但该提交晚于 1.3.0，也没有新的发布版。#214 中守护进程匿名堆在 20–24 小时内增长到 17 GiB；这个队列是依赖内部的无界结构，
是目前证据最强的候选，但**没有在本机复现，也没有在受影响机器上证实**。

## 决定

1. 在根 `Cargo.toml` 用 `[patch.crates-io]` 把 `iroh`、`iroh-base`、`iroh-dns`、`iroh-relay` 一起指向上游仓库的该提交。
   四个包同时覆盖，是因为 `iroh-tickets`、`iroh-util` 是 registry crate，只覆盖其中一部分会产生第二份 `iroh-base`。
   不 fork：固定的是上游提交本身，不引入需要维护的分支。
2. 该提交同时带入此前的 8 个提交，其中两项行为变更：IPv6 字面量 relay URL 的 QAD 探测，以及新增发送
   `X-Iroh-Endpoint-Id` relay 头（同时弃用 `X-Iroh-NodeId`）。其余为 CI、文档与 `iroh-dns-server` 的 clap 更新，不在 Engine 依赖内。
3. 网络库日志按“注册表目录名（`iroh-1.3.0`）”匹配规则；固定后调用点位于 cargo 的 git 签出目录，没有这样的目录名，
   所有 iroh 规则会静默失效。`callsite_path` 因此额外接受 `…/iroh-<16 位十六进制>/<提交前 7 位>/iroh/src/…`，
   且提交必须在 `PINNED_IROH_REVS` 允许列表内，仍按逻辑版本 `iroh-1.3.0` 匹配。测试要求允许列表包含根清单固定的提交；
   换提交必须同时评审规则。

## 后果

- `[patch]` 只读取构建根清单。依赖 Engine 的仓库（Desktop）必须在自己的根清单重复同一段，否则它们仍链接 iroh 1.3.0 注册表版本，
  得不到修复，网络库日志规则也按原样匹配。这与 ADR-031 补充的原则相反，只对 iroh 例外，需要在交接文档中通知下游。
- 构建依赖 GitHub 上游仓库的可用性与提交不被改写；与既有的 iroh-blobs git 依赖同类风险。
- 固定后能否消除 #214 的增长**尚未验证**，不能据此关闭 #214。
- 队列有上限后，卡在路径上限的连接仍会每 333 ms 重试一次；CPU 抖动不被此提交消除。
- 不改变 noq-proto 1.3.0 的流重组与 CID 退役队列（上游仍无修复）、noq#750、iroh#4599。

## 退出条件

iroh 发布的完整依赖链同时包含 `43e232fb` 路径队列修复与下文 EDNS 修复后，删除根清单的 `[patch]` 与
`PINNED_IROH_REVS`，规则改回注册表目录匹配，并通知下游删除重复段。仅发布其中一个修复不足以退出。


## 2026-10-24：EDNS 解析修复来源补充

### 问题与责任

Windows 诊断捕获的公开中继 A 响应在 DNS header 中为成功，OPT TTL 为 `0x00000001`，
但 `simple-dns 0.12.0` 把低位保留标志解释为扩展 RCODE，返回 BADVERS。相同报文清除该位后成功。
[RFC 6891 §6.1.3–6.1.4](https://www.rfc-editor.org/rfc/rfc6891.html#section-6.1.3) 规定扩展 RCODE 位于 TTL 最高 8 位，
版本位于接下来 8 位，低 16 位为标志；保留标志接收时必须忽略。原库版本字段及写入也使用了错误布局。

DNS 库负责完整报文读写；Engine 保持既有解析调用、错误分类及网络恢复责任，不新增忽略 DNS 错误的业务分支，
不修改 TUN、overlay、P2P 或 LAN 策略。调用方仍执行既有网络动作；正常响应返回解析结果，真实 DNS 失败继续向上传递。

### 决定与来源

用户授权创建 UniClipboard fork。保留本 ADR 原有的四包根补丁例外，改为以下不可变来源；
DNS 修复沿实际依赖传递，不要求宿主另建 simple-dns 或 n0-dns-resolver 补丁。

| 包 | 固定来源 | 相对基线的改动 |
| --- | --- | --- |
| iroh、iroh-base、iroh-dns、iroh-relay | [UniClipboard/iroh c8efae06](https://github.com/UniClipboard/iroh/commit/c8efae061e3daa0afc6aa6db856cfcf8576da97b) | 基于原 `43e232fb`，只修改 DNS 依赖清单与来源说明，Rust 源码不变 |
| n0-dns-resolver 0.1.0 | [UniClipboard/n0-dns-resolver b15fdb6a](https://github.com/UniClipboard/n0-dns-resolver/commit/b15fdb6abd50714fd551b4fd93a9b5728457442d) | 基于发布来源 `16a791ce`，只固定 simple-dns 来源 |
| simple-dns 0.12.0 | [UniClipboard/simple-dns efdb13b9](https://github.com/UniClipboard/simple-dns/commit/efdb13b9473131f7172b790b4e8497825aff2890) | 基于上游 `58e2ed41`，修正 OPT 掩码、读写位移和文档报文，新增 wire 回归；移除可选 bind9 子模块以免消费者构建下载它 |

上游 HEAD 的 OPT 实现与 0.12.0 一致。未扩展该库把未知 RCODE 折叠为 `Reserved` 的既有 API。
维护来源增至三个 fork，是确保根 iroh 补丁能沿真实依赖带入修复的代价；不能用只更新锁文件代替这些声明。

iroh Rust 源码与调用点行号相对 `43e232fb` 完全不变；网络日志允许列表及测试改为新提交前缀 `c8efae0`。
规则字段、采样、事件和关联方式不变。宿主必须同步根清单现有四个 iroh 补丁的 URL 和完整 rev；
只更新 Engine Git 引用仍可能链接旧 DNS。核验应在最终构建工作区运行 `cargo tree --locked -i simple-dns`，
确认查询路径经过上述修复来源，并检查没有第二份旧来源。Engine 的锁文件不能证明宿主锁文件正确。

### 验证与边界

simple-dns 的 `tests/edns_wire.rs` 覆盖原始捕获响应、保留标志、DNSSEC DO、真实 BADVERS、版本字节、
扩展 RCODE 固定 wire 编码及无 OPT 报文；同时运行该库既有单元、集成、文档测试及 alloc 编译。
Engine 的回归入口为 `just cargo test -p uc-infra-p2p --test dns_edns_response --locked`，通过本机 UDP
服务重放响应，驱动 Engine 实际使用的 iroh → resolver → parser，避免只测试孤立库而遗漏依赖接入。

本地 Linux aarch64 上，库测试 176 项、Engine UDP 回归、网络日志来源 4 项、完整 workspace/all-targets 编译及根交付门禁全部通过。
Windows 中继恢复、连接数大于零、重新生成邀请和 Windows → Linux 配对尚未实机验证；全部标为跳过。
本次解析缺陷已确认，配对恢复仍需两端使用最终修复来源重新构建后验收，不能由离线报文成功推断。

## 2026-10-24 补充：单一网络栈 submodule（取代根补丁例外）

实际 desktop 只更新 Engine 后仍使用 registry DNS 链，证明要求宿主重复根补丁会泄露实现知识。
当前决策取代本 ADR 之前的根补丁交付方式，并恢复 ADR-031 的“补丁不依赖下游”原则。

Engine 以 `third_party/iroh` 单一 Git submodule 固定 UniClipboard/iroh 网络栈 fork；
iroh-blobs、iroh-tickets、iroh-util 的源码和许可证集中在该 fork，来源记录见其
`UNICLIPBOARD_PATCH.md`。全部内部 iroh 依赖使用路径声明；Engine 自身及现有 mDNS vendor
同样指向这些路径。DNS resolver/parser 保持已修复的不可变 Git 依赖。

完整负责人是 Engine 构建集成，调用方唯一动作是依赖 `uc-engine`。成功构建包含唯一网络栈来源；
缺少 submodule 或来源不符时构建门禁失败，维护者更新 checkout 后重试。运行期 API、失败与恢复责任不变。

CI checkout 递归初始化 submodule。更新网络栈必须审查调用点并同步来源门禁；本次 iroh Rust 源码
相对 EDNS fork 未变化，仅调整包布局与依赖。网络日志识别固定 submodule 路径，来源门禁核对
已审查提交、gitlink、实际 checkout 与干净状态。

验收必须包含无 `[patch]`、仅依赖 `uc-engine` 的独立 Git 消费者；仅检查 Engine 根构建不够。
宿主迁移到包含本决策的 Engine 提交时应删除历史四个 iroh 根补丁。上游完整修复可用后整体评估退出，
不能单独恢复某一个包的 registry 来源而造成类型分裂。
