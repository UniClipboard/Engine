# 计划 042：升级 iroh 1.3 并由产品持有内置 relay 列表

## 状态与执行约定

- **状态**：实施中。Engine 端已完成并在隔离环境验证；Desktop 与 Mobile 展示由协调者单独派发，尚未开始，因此本计划不能标为完成。
- **日期**：2026-10-01。基线 Engine `4ece7ac4`（iroh `1.0.0-rc.1`）。
- **完整负责人**：Core 持有内置 relay 列表与路由决策；Application 的 Settings 流程负责概览与“已保存/已生效”记录；Infra 只接收已决议的列表并绑定；Engine 只装配并公开 `QueryRelayOverview`。
- **调用方唯一动作**：宿主调用一次 `QueryRelayOverview` 取得路由方式与逐条记录；修改自定义 relay 仍走既有操作。
- **成功结果**：宿主可展示内置与自定义 relay 的区域标识、地址、来源与是否被运行中节点使用；该结果不声明已连通。
- **失败结果**：relay 凭据存储不可用等返回既有设置错误码；启用 relay 却没有任何 relay 地址是配置错误，节点拒绝启动，不静默退回上游默认。
- **重试与重启责任**：保存自定义 relay 后需重新构建网络节点才生效；Engine 在 `change_pending` 中如实报告，重建责任沿用既有的 endpoint 重建路径，不由本计划新增。
- **相关文档**：[ADR-031](../../design-docs/decisions/031-product-owned-builtin-relay-list.md)、[安全架构](../../SECURITY.md)。
- **用户范围调整**：仅局域网模式正在独立重构，**按用户要求排除本次验收**；实现上不改变也不绕过现有仅局域网策略。该项不是通过，也不是本次交付阻塞。

## 范围

1. 依赖栈一次性升级：iroh/iroh-base/iroh-relay/iroh-dns 1.3.0、iroh-tickets 1.0.0、noq 系 1.3.0、iroh-blobs 0.103.0（保留 UniClipboard fork 的三项补丁，rebase 到上游 `v0.103.0`，无冲突；fork 分支 `uniclipboard/0.103.0-patched`，`124780fb69768b8506565b2760c435182f9e2d20`）、iroh-mdns-address-lookup 0.6.0；`swarm-discovery` 本地补丁不变；`tests/hosts/connectivity-relay` 同步。
   依赖来源：iroh-blobs fork 是 `uc-infra-p2p` 的直接 git 依赖；打过补丁的 `swarm-discovery` 与随仓库副本 `iroh-mdns-address-lookup`（仅把 swarm-discovery 依赖指向本地路径）是 `uc-infra-p2p` 的 `third_party` 路径依赖，Engine 不再有 `[patch]`，依赖本仓的下游（Desktop）不需要重复任何补丁。
2. 内置 relay 列表进入 Core，脱离 `RelayMode::Default` 的隐含上游列表；节点总是绑定显式列表。
3. `QueryRelayOverview`（Engine、UniFFI、HarmonyOS napi）；下游契约见 `downstream-contract-handoff.md`。

## 验证（隔离环境，ARM Linux 真机的用户命名空间；无真机移动设备）

| 项目 | 结果 |
| --- | --- |
| 工作区 `cargo check --all-targets --locked`、`uc-infra --features lan-compat`、fmt、架构脚本 | 通过 |
| iOS ×3、Android ×2 的 UniFFI 绑定与 HarmonyOS napi 目标编译检查 | 通过（仅编译；未运行） |
| 新旧互通：rc.1 与 1.3 直连配对、互传、断线与长时离线恢复，两个方向 | 通过 |
| 新旧互通：自定义 relay 下 relay-only，新旧 relay 服务端各自搭配，两个方向 | 通过 |
| 同版本对照：直连恢复 | 通过 |
| relay-only 第二轮及之后 | **两个版本都会间歇变慢，不是本次升级引入**：relay 服务端重启后，节点的 relay 客户端可能仍处于重连退避（指数退避，上限 16 秒，iroh rc.1 与 1.3 代码相同），晚些才重新注册，对端拨号超时重试；场景的 3 秒预算因此失败，但双方最终会收敛。同一脚本各 8 次运行：rc.1 失败 1 次，1.3 失败 1 次，无法区分。实验中观测到恢复约 6 到 23 秒，这只是观测值，不是最坏时延保证。本计划不修复它，单独跟踪：#134 |
| known-peer 恢复 | 1.3 失败 5/5，rc.1 基线失败 3/4；错误一致，属既有问题，不由本次升级引入 |
| 概览契约：真实 Engine 上内置/自定义/已保存未生效 | 通过 |
| 内置列表与 iroh 1.3 上游默认逐项一致（含 QUIC 地址发现配置） | 通过 |
| 仅局域网 | **按用户要求排除本次验收** |
| 真机/模拟器上的 iOS、Android、HarmonyOS、桌面运行；对 n0 公共 relay 的任何流量；旧 canary 主机名的存量对端地址重连 | 未验证 |

原始证据与复跑脚本不入库；复现方法见 #134。

## 待办与阻塞

- Desktop 与 Mobile 展示；Desktop 删除自己的 `iroh-blobs` 与 `swarm-discovery` `[patch]` 并对齐 `p2p-bench` 的 iroh 版本；这些在本 PR 合并后由各自仓库处理。
- relay 重启后的恢复偏慢（iroh 自身退避，观测约 6 到 23 秒，非保证值）：#134 单独跟踪；本计划不承诺 3 秒恢复，也不包含重连实验。
- 公共 relay 是否已停服尚未实证；本计划不把它当作已发生的故障，也不依赖该判断。
