# swarm-discovery 的 UniClipboard 补丁

本目录来源于 crates.io 的 `swarm-discovery` 0.6.3（上游提交见 `.cargo_vcs_info.json`），许可证为 `Apache-2.0`。

## 本仓变更

- `Cargo.toml` 声明独立 `[workspace]`，与 Engine workspace 分离；Engine 只经 `uc-infra-p2p` 与 `iroh-mdns-address-lookup` 的路径依赖使用本副本。
- updater 的邮箱关闭后，延迟 GC 任务不再重复投递（`src/updater.rs`）。
- 接收者在所有者释放其最后一个 `ActoRef` 时退出（`src/receiver.rs`）。上游接收者从不轮询自己的 `ActoCell`，移除接口只丢弃引用，接收任务连同它持有的接口套接字会一直存活，接口反复变化时套接字持续累积。
- 接口接收者不再交给 acto 监督，由 guardian 自己持有引用与任务句柄（`src/guardian.rs`）。移除接口时先停止并等待接收者退出，再从接口表释放套接字并退出该接口的组播组；正常移除不再被当作 actor 结束而停止整个发现服务。接口接收者意外结束时仍与上游受监督接收者一致，停止整个发现服务。
- 新增 `DropGuard::set_interfaces_v4`：调用方声明期望的完整 IPv4 接口集合，guardian 是期望集合与已安装接口的唯一负责人。只有接口套接字、接收者以及主通配套接字上的组播成员关系都建立后才算已安装；仍有期望接口未安装时按 1 秒起、每次翻倍、最长 60 秒的退避重试，全部安装后重置退避。重复提交相同集合会立即再试一次。首次调用后，`add_interface_v4`/`remove_interface_v4` 也修改该集合。从未调用此接口的使用方（例如配对发现）保持上游的单次尝试行为。
- 主通配 IPv4 套接字的组播成员关系由 `Sockets` 按接口表维护（`src/socket.rs`、`src/guardian.rs`）。成员关系属于（套接字、组播组、操作系统接口），不属于单个地址：同一网卡上的两个地址共用一个成员关系，上游移除其中一个地址时无条件退出组播组，会让仍保留的地址所在网卡也离开组播组，而 guardian 因该地址的接收者仍在而不会重新加入（Windows 的 `IP_MREQ` 按接口生效，macOS 预计相同，Linux 的跨套接字组播投递会掩盖该问题）。现在移除地址并退出后，为其余保留地址逐个先退出再加入，使其所在接口重新成为成员，不依赖各平台对重复加入返回的错误码；加入结果不再被忽略，失败的地址记为成员关系未确认。guardian 安装（包括已安装地址）时确认成员关系，未确认则重新加入一次，失败即视为未完成安装，由期望集合的退避重试补齐。未使用期望集合的调用方仍只尝试一次。
- 接口套接字创建与主通配套接字的加入、退出组播组都经过 `socket::InterfaceProbe`。生产构建中它是零大小类型，不改变行为；单元测试用它注入创建或加入失败，以弱引用确认套接字已被全部持有者释放，并记录加入与退出的顺序。

这些行为由本 crate 的单元测试验证：`removed_interface_releases_receiver_and_socket_while_retained_interface_discovers`（仅 Linux，使用回环地址 127.0.0.2）、`failed_interface_installation_is_retried_without_a_new_interface_set`，以及 `removing_an_address_rejoins_the_group_for_an_address_retained_on_the_same_interface` 与 `a_failed_rejoin_for_a_retained_address_is_retried_without_a_new_interface_set`（仅 Linux，127.0.0.1 与 127.0.0.2 同属回环网卡；断言只依赖观察点记录，不依赖跨套接字组播投递）。本 crate 不属于 Engine workspace，需用 `--manifest-path third_party/swarm-discovery/Cargo.toml` 单独运行。

## 来源与删除条件

只有上游提供同等的接收者生命周期、期望接口集合、安装重试与按接口维护的组播成员关系，且 `iroh-mdns-address-lookup` 的补丁也可移除后，才能删除本副本；替换前重跑上述单元测试以及 Engine 的 `mdns_discovery_contract`。
