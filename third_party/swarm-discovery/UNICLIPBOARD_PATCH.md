# swarm-discovery 的 UniClipboard 补丁

本目录来源于 crates.io 的 `swarm-discovery` 0.6.3（上游提交见 `.cargo_vcs_info.json`），许可证为 `Apache-2.0`。

## 本仓变更

- `Cargo.toml` 声明独立 `[workspace]`，与 Engine workspace 分离；Engine 只经 `uc-infra-p2p` 与 `iroh-mdns-address-lookup` 的路径依赖使用本副本。
- updater 的邮箱关闭后，延迟 GC 任务不再重复投递（`src/updater.rs`）。
- 接收者在所有者释放其最后一个 `ActoRef` 时退出（`src/receiver.rs`）。上游接收者从不轮询自己的 `ActoCell`，移除接口只丢弃引用，接收任务连同它持有的接口套接字会一直存活，接口反复变化时套接字持续累积。
- 接口接收者不再交给 acto 监督，由 guardian 自己持有引用与任务句柄（`src/guardian.rs`）。移除接口时先停止并等待接收者退出，再从接口表释放套接字并退出该接口的组播组；正常移除不再被当作 actor 结束而停止整个发现服务。接口接收者意外结束时仍与上游受监督接收者一致，停止整个发现服务。
- 新增 `DropGuard::set_interfaces_v4`：调用方声明期望的完整 IPv4 接口集合，guardian 是期望集合与已安装接口的唯一负责人。只有接口套接字和接收者都建立后才算已安装；仍有期望接口未安装时按 1 秒起、每次翻倍、最长 60 秒的退避重试，全部安装后重置退避。重复提交相同集合会立即再试一次。首次调用后，`add_interface_v4`/`remove_interface_v4` 也修改该集合。从未调用此接口的使用方（例如配对发现）保持上游的单次尝试行为。
- 接口套接字创建经过 `socket::InterfaceProbe`。生产构建中它是零大小类型，不改变行为；单元测试用它注入创建失败，并以弱引用确认套接字已被全部持有者释放。

这些行为由本 crate 的单元测试验证：`removed_interface_releases_receiver_and_socket_while_retained_interface_discovers`（仅 Linux，使用回环地址 127.0.0.2）与 `failed_interface_installation_is_retried_without_a_new_interface_set`。本 crate 不属于 Engine workspace，需用 `--manifest-path third_party/swarm-discovery/Cargo.toml` 单独运行。

## 来源与删除条件

只有上游提供同等的接收者生命周期、期望接口集合与安装重试，且 `iroh-mdns-address-lookup` 的补丁也可移除后，才能删除本副本；替换前重跑上述单元测试以及 Engine 的 `mdns_discovery_contract`。
