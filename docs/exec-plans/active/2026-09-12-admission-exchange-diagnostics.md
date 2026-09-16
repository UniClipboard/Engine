# 配对通信等待诊断

状态：Engine 实现和本地验收完成，真实产品双端验收待执行。目标是定位真实双端配对的等待位置，不实施性能或协议调整。

## 责任与边界

- Infra 配对通信实现拥有一次完整交换及其内部步骤记录；调用方仍只调用既有连接和交换能力。
- Application 保持业务处理、重试及重启恢复责任，Engine 只装饰既有完整能力。
- 成功仍按现有回复、确认与结束规则判定；超时、拒绝和中断不修改原业务返回及关闭规则。
- trace 保留完整交换，步骤写入关联的本地日志；认证前不采信远端关联。
- 固定步骤、结果、原因与网络数值不包含原始身份、消息编号、内容、地址或凭据。

## 实施与验收

- [x] 最小切片：发送请求、接收回复的步骤记录与真实失败分类，普通采集实际文件输出验证通过。
- [x] 扩展回复准备、发送、校验、确认与结束等待；取消及总截止时间保留准确结果。
- [x] 详细模式增加现有网络库实际可提供的连接指标，缺失明确表达，不新增采样任务。
- [x] 正常、迟到或缺失回复、迟到或缺失确认、中断、无效回复测试，检查日志和 trace 一致及隐私。
- [x] 根目录交付检查，设计文档与架构圣经同步。
- [ ] Mac 与 iPhone 新版本真实复现：待设备配合，尚未执行。

记录能力验证与原事件根因确认分别验收。手机导出裁剪问题不通过修改 Engine 的协议或日志配额掩盖。

## 导出边界

已只读核对手机仓库：`src/support/diagnostics/internal/diagnosticPackage.ts` 中每个文件仅导出末尾 512 KiB，
超过时累加 `truncatedFileCount`。这是手机导出流程的独立限制；本次 Engine 改动不修改该仓库。
真实双端验收前必须取得未裁剪的配对时间窗，不能以当前进程的零丢弃计数替代历史完整性。

## 验证中发现的验收竞态

完整配对 trace 验收曾在看到 Active 后立即关闭双方，偶发打断最后的 settle 通信，表现为客户端失败、
服务端未完成及完成日志缺失。验收改为先收齐原有四轮通信和生命周期完成记录，再关闭双方；
原有通过条件不变，失败输出补充具体的节点状态和时间差。修正后正常完整配对连续两次通过。

## 已执行的验证

- `cargo test -p uc-infra --locked --lib network::iroh::space_admission`：17 项通过，包含真实 Iroh 交换、回复迟到/缺失、确认迟到/缺失、断开、异常格式、错误证明、结束等待超时、中断及总截止时间。
- `cargo test -p uc-observability-contract -p uc-observability-runtime --locked`：整套通过；新增的拒绝自由文本检查及实际文件检查在最后修改后分别重跑通过。
- `cargo test -p uc-observability-runtime --locked --test admission_exchange_file`：普通步骤保留、首次失败保留、完成关联、详细模式网络快照和不可用指标均通过。
- `cargo test -p uc-infra --locked --test admission_diagnostic_file`：真实资料读取、真实通信和实际文件诊断通过。
- `cargo test -p uc-engine --locked --features dev-tools --test space_membership_auto_pairing_e2e uninterrupted_admission_uses_one_trace -- --ignored --exact`：修正验收自身的过早关闭后连续两次通过。
- 同一测试目标的 `in_flight_admission_restart_uses_new_traces_and_one_flow -- --ignored --exact`：通过，重启后恢复并建立新的在线关联。
- `cargo metadata --locked --format-version 1`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`、Rust 风格检查、Engine 仓库检查及 `git diff --check`：通过。
- workspace check 保留未改动的 HarmonyOS 测试文件现有未使用导入警告，无编译失败。

## 跳过的验收

- Mac 与 iPhone 产品更新及真实重新配对：未执行，设备状态未修改。
- Windows、Android、HarmonyOS 设备及真实远程可视化：未执行；本机测试不替代这些结论。
- 发布产物与产品导出包完整时间窗：未执行；没有发布或部署，本次 99 秒等待的根因仍未证实。
