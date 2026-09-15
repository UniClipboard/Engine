# 配对内部处理与排队诊断

状态：Engine 实现与本地验收完成；真实双端验收待执行。用户明确限定只补日志，保持配对、维护调度、重试、超时和保存行为。

## 责任与完成标准

- Application 仍拥有完整配对及维护流程，Infra 拥有真实存储和材料准备，现有生命周期负责人拥有会话切换。
- 调用方只调用现有完整能力；不增加 facade、port、业务结果或 Core 观测字段。
- 内部记录是本地诊断，不创建业务节点。复用现有在线关联，固定步骤和结果，不输出身份、邀请、错误正文或载荷。
- 开始与实际执行对应，完成与返回对应，取消记中断；诊断不改变原始错误、提交和恢复责任。
- 普通采集保留关键内部处理与排队证据。先证明 Sponsor 状态读取的实际文件输出，再扩展到两端材料、保存、维护和会话。
- 排队记录区分唤醒、去重、实际开始与未执行；不能把合并触发误称为执行成功。

## 验收清单

- [x] 最小真实存储切片与普通模式文件输出。
- [x] 两端内部处理、协议锁等待、回复后处理关联；完整本机双端配对及实际文件关联通过。
- [x] 维护触发、排队、步骤结果、更新数量；人为阻塞更新与重复唤醒的确定性验收通过，原调度顺序不变。
- [x] 会话切换内部等待与结果；完整配对文件关联及 7 项生命周期回归通过。
- [x] 延迟、失败、中断、关联和隐私测试，以及根目录交付检查。
- [ ] 新版 Mac 与 iPhone 完整双端时间窗。尚未执行；现有手机包已截断，不能充当全量证据。

原始 121 秒事件只提供定位线索。补记录不代表已确认存储、密码处理或锁竞争的具体根因。
产品导出截断由产品导出负责人处理，本次不修改其他仓库、不修改设备状态、不发布。

## 验证中的修正

最小真实存储文件测试发现：内部被过滤的 tracing 作用域不能单独保证延续父关联。
完整负责人显式延续已有不透明上下文后，通过真实状态读取以及两端完整配对文件验收。

完整 Engine 配对测试首次发现新增异步观测包裹放大大型业务 future 的内联状态，造成栈溢出。
按已有会话观测模式，在进入观测 future 之前固定存放原业务 future，未调整线程栈大小或业务流程；
同一完整配对命令随后通过。

## 本地验收证据

- `cargo test -p uc-infra --locked --test space_admission_state`：25 项通过，覆盖实际加密存储和普通模式文件关联。
- `cargo test -p uc-application --locked --lib space -- --test-threads=1`：230 项通过；独占日志捕获测试另行精确运行。
- `cargo test -p uc-application --locked --lib space::membership::maintenance::diagnostic_tests::blocked_updates_explain_queue_wait_and_coalesced_wakes_without_changing_order -- --ignored --exact`：通过；阻塞更新期间的等待、合并唤醒和原执行顺序均有断言。
- `cargo test -p uc-observability-contract -p uc-observability-runtime --locked`：通过；真实普通模式文件覆盖耗时、失败、中断、关联与隐私。
- `cargo test -p uc-engine --locked --features dev-tools --test space_membership_auto_pairing_e2e uninterrupted_admission_uses_one_trace -- --ignored --exact`：通过；四轮两端内部处理和会话切换实际文件关联完整。
- `cargo test -p uc-engine --locked --features dev-tools --test space_membership_auto_pairing_e2e in_flight_admission_restart_uses_new_traces_and_one_flow -- --ignored --exact`：通过；重启后恢复和原关联规则保持。
- `cargo test -p uc-engine --locked --features dev-tools --lib runtime::session_supervisor::tests`：7 项通过。
- `cargo test -p uc-infra --locked --lib space::admission` 与 `cargo test -p uc-infra --locked --lib space::adapters::re_pairing_state`：通过。
- `cargo metadata --locked --format-version 1`、`cargo check --workspace --all-targets --locked`、`cargo fmt --all -- --check`：通过；未修改的 HarmonyOS 合同测试存在未使用导入警告。
- `node scripts/architecture/check-rust-style.mjs`、`node scripts/architecture/check-engine-repository.mjs`、`git diff --check`：通过，包含隐私检查和架构负例验证。

以上为本机 Engine 验收，不代表 Mac 产品与 iPhone 实机复现，也未修复产品导出截断。
