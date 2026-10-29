# Space 加入受管通信执行计划

状态：实现与本地验收完成；历史发布二进制、Relay 与真机矩阵跳过。

## 目标与负责人

`SpaceAdmissionProtocol` 继续拥有完整加入、认证消息处理、本机取消及持久恢复；宿主只调用现有完整动作。`ConnectivityManager` 接管准入网络连接、流租约、额度、取消和停止监督。非成员端点使用独立类型的资源键，不经成员目录反向授予资格。

成功仍返回原认证交换结果；失败保留固定分类及来源。网络取消不回滚已提交事实。业务重试、续接及重启仍由 Admission Recovery 持有；Engine 只组装与关闭，不编排认证步骤。

## 实施

- [x] 统一连接管理器支持成员与非成员端点的类型隔离，保留成员消费者行为。
- [x] 迁移初次加入及续接出站连接和流；保留 ALPN、线上帧、OPAQUE、MAC 和 peer binding。
- [x] 迁移服务端接流，提供全局和单端点有界额度；仅允许准入协议，业务认证通过后才交给 Application。
- [x] 验证取消、超时、会话切换、单端点占位隔离和关闭资源结算。
- [x] 验证同线上协议双方向互通、初次与续接，以及历史不兼容布局的升级拒绝。
- [x] 完成定向回归和仓库交付检查，同步长期设计。

## 最小完整切片与约束

先证明一对无成员记录的真实 Iroh 端点可以经统一租约完成初次和续接交换；邀请错误及绑定错误继续拒绝。不新增持久格式，不引入协议多流复用，不把认证通道授权交给普通成员通信。

每次交换从入站或出站入口使用同一绝对截止时间；帧限制仍保留。待认证额度和每端额度都由统一管理器拥有，取消的工作在本地结算完成前保留其资源责任。新会话不能接管旧交换。

## 验收

真实 Iroh 定向测试、Application 统一通信回归、Engine 准入与会话测试；运行根目录 metadata、workspace all-targets check、fmt、Rust 风格、仓库架构及 diff 检查。设备和跨网络验证未执行时标为跳过。

## 实际验收结果

- `just cargo metadata --locked --format-version 1`：通过。
- `just cargo check --workspace --all-targets --locked`：通过。
- `just cargo test -p uc-application --lib connectivity --locked`：134/134 通过，覆盖成员请求围栏、取消、关闭、迟到拨号和会话恢复。
- `just cargo test -p uc-infra-p2p --lib space_admission --locked`：35/35 通过，覆盖初次／续接、原生协议兼容、认证拒绝、取消释放、单端点额度与总截止分类。
- 真实交换矩阵独立复测：通过；一秒总预算只推进两秒，三十秒帧预算尚未到期，证明总截止记录为超时而非取消。
- Engine `space_membership_auto_pairing_e2e` 的 `admission::`：9/9 通过，包含回包丢失恢复、重启后传输、移除再加入及最终确认提交边界。
- Engine `space_switch::existing_device_switches_space_through_stable_operations`：1/1 通过。
- fmt、Rust 风格、Engine repository preflight 及 diff 检查通过。

未执行：历史发布二进制、Relay／跨网络及物理设备矩阵，均记为跳过。不宣称已发布或完成设备验收。

长期契约见 [Space 加入通信](../../design-docs/space-admission-connectivity.md)。

## PR 复核校正

认证期可因对端断线结束并返回保留来源的传输错误；认证完成后的业务处理、回复、确认及结算不再被连接关闭整体取消，沿用原绝对截止时间。真实 Iroh 新回归验证断线后仍可完成并保存业务状态，以及未放行的业务仍在原总截止退出；准入测试 36/36 通过。
