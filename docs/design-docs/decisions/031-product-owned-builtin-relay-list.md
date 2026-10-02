# ADR-031：内置 relay 列表由产品持有

- **状态**：已采纳（连通性与下游展示的验收范围见执行计划 042）
- **日期**：2026-10-01
- **范围**：Engine 的 relay 路由决策、`QueryRelayOverview` 查询、iroh 依赖栈升级到 1.3
- **相关文件**：[`relay_routing.rs`](../../../crates/uc-core/src/settings/relay_routing.rs)、
  [`node.rs`](../../../crates/uc-infra/src/network/iroh/node.rs)、
  [`network.rs`](../../../crates/uc-engine/src/assembly/network.rs)、
  [执行计划 042](../../exec-plans/active/042-iroh-1-3-upgrade-and-builtin-relays.md)

## 背景

此前未配置自定义 relay 时，infra 使用 `RelayMode::Default`，实际 relay 列表由所用 iroh 版本隐含决定：
产品既不能读取它，也无法在升级时审阅它的变化，宿主界面看不到“当前用了哪些 relay”。

## 决定

1. **Core 持有唯一的内置列表**（`BUILTIN_RELAYS`：稳定区域标识加规范化 URL）。列表是随版本发布的产品默认值，
   **不写入用户设置、不持久化**，因此没有新持久字段，也不需要迁移；升级 Engine 即更新内置列表，用户的自定义列表保持原样。
2. **路由优先级不变**：`allow_relay_fallback = false`（仅局域网）> 非空自定义列表（整体替换内置列表，不合并）> 内置列表。
   `allow_relay_fallback` 到 `disable_relays` 的取反仍只在 `assembly/network.rs` 一处。本决定不改变也不绕过仅局域网策略。
3. **Infra 不再隐含默认列表**：节点只接收一个已决议的 `relay_urls`，总是使用 `RelayMode::Custom`（或 `Disabled`）；
   启用 relay 却传入空列表是配置错误并拒绝启动，不静默退回上游默认。内置条目与自定义条目走同一构造路径
   （`RelayConfig::from(url)`，含 QUIC 地址发现配置），与 iroh 1.3 的 `default_relay_map()` 一致，已按源码核对：iroh 1.3 的 `RelayMode::Default` 与 `Custom(default_relay_map())` 生成相同的传输配置（`is_user_defined` 同为真），唯一差别是环境变量 `IROH_FORCE_STAGING_RELAYS` 只影响 iroh 自己的默认模式，显式列表之后该变量不再改变产品行为。
4. **`QueryRelayOverview`** 返回已保存的路由方式、运行中节点绑定时采用的方式、`change_pending` 与逐条记录
   （来源、区域标识、URL、是否配置凭据、`in_effect`）。`in_effect` 只表示运行中的节点按此地址配置，**不表示已连通**；
   连通状态由既有网络状态给出。节点每次构建时由网络装配记录实际采用的路由，因此“已保存”和“已生效”可区分。
5. 不新增编辑、开关或测速内置条目的能力；展示名称由宿主按区域标识本地化。
6. 已存储的对端 relay 地址不做批量改写：它们由既有的已观察地址持久化在首次成功联系后更新；混合版本行为由端到端矩阵验证。

## 后果

- 升级 iroh 时，上游默认列表的变化不会自动进入产品行为；是否同步内置列表是一次显式、可审阅的修改。
- 内置主机名是第三方运营的服务；其可用性、停服时间与限流不由本仓控制。是否自建 relay 是独立的产品决定。
- 旧客户端缓存里的 n0 默认 URL 若被当作用户自定义 relay 导入，会让用户固定在旧主机名上；下游导入必须按交接文档处理。

## 补充：依赖补丁不依赖下游 `[patch]`

`[patch]` 只读取构建根清单，不会传给依赖 Engine 的仓库。因此 Engine 不再使用 `[patch]`：iroh-blobs fork 是 `uc-infra` 的直接 git 依赖（固定 rev），
打过补丁的 `swarm-discovery` 与随仓库副本 `iroh-mdns-address-lookup`（仅把其 swarm-discovery 依赖指向本地路径）是 `uc-infra` 的 `third_party` 路径依赖，
并在根清单 `exclude`。依赖防火墙脚本只放行这两个路径依赖。下游仓库不需要任何补丁；上游接受修复后删除对应副本。
