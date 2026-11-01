# 启动期 KEK 单次读取

状态：实现与本地验证完成。后续来源：UniClipboard/UniClipboard#2160。

## 范围与责任

`ProfileKeyRecoveryStore` 负责资料恢复与会话恢复之间的一次 KEK 交付。调用方仍只启动 Engine。
启动准备先校验 keyslot 与资料 vault，然后把同一份已验证 KEK 交给首次会话恢复读取。
交付后移除该材料；启动范围结束、取消、挂起、重置或 KEK 写入/删除时作废未交付材料。
不增加持久化文件，不改变密文格式，不添加公开 Engine 契约。
显式口令认证继续复读安全存储并验证写入结果，不使用历史观察替代验证。

## 失败边界

- 缺失、损坏或不匹配的 KEK 继续进入口令恢复。
- 安全存储不可用继续保留错误结果。
- 资料 vault 验证失败不得发布启动材料。
- 挂起后恢复必须重新访问安全存储，不能使用启动材料绕过失败。
- 改口令、重置和 KEK 替换必须作废未交付材料。
- 启动失败或取消必须释放未交付材料。

## 验收

先添加公开 Engine 端到端回归，再修复。使用临时资料、真实加密与 SQLite；仅宿主安全存储使用内存实现。
启动同一份已有资料，验证历史可读与 KEK 读取次数为一。
挂起后注入安全存储故障，验证恢复失败；修复故障后恢复并读取原历史。
运行已有密钥丢失、改口令、恢复出厂与启动取消端到端场景。
执行 workspace check、fmt、架构检查与 diff check。
真实 macOS 钥匙串弹窗与 App/CLI 签名验收不在本次自动验收范围。

## 已核验结果

基线：`d532978d8bc9740a64460ee08b6e97a85fe4b590`。

- 修复前运行新增的宿主契约端到端回归：失败，KEK 实际读取 2 次，预期 1 次。
- 修复后运行同一回归：通过，包含历史读取、挂起后读取故障和恢复后历史读取。
- `just cargo test -p uc-engine --test host_contract --locked`：82 项通过、1 项失败、2 项跳过。
- 失败项为 `space_leave::repeated_leaves_do_not_accumulate_tasks_or_descriptors`，任务样本在 54 与 57 之间波动。
- 将两个生产修改文件恢复为基线后，独立运行同一失败项仍失败，任务样本仍在 54 与 57 之间波动；随后恢复候选源码。本次不修改该范围外问题。
- `just cargo test -p uc-infra-profile profile_key_recovery --locked`：22 项通过。
- `just cargo metadata --locked --format-version 1`：通过。
- `just cargo check --workspace --all-targets --locked`：通过。
- `just cargo fmt --all -- --check`：通过。
- `git diff --check`：通过。
- `node scripts/architecture/check-rust-style.mjs`：通过。
- `node scripts/architecture/check-engine-repository.mjs`：通过。

复跑 KEK 回归：

```bash
just cargo test -p uc-engine --test host_contract startup_reads_kek_once_and_resume_rechecks_secure_storage --locked
```

宿主安全存储为内存实现。真实 macOS 钥匙串弹窗、发布签名和设备验收均跳过。
