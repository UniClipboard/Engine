# 启动期 KEK 单次读取

状态：实现与单次读取验证完成；密码输入次数的产品验收未通过。后续来源：UniClipboard/UniClipboard#2160。

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

首轮自动验证使用内存宿主安全存储。后续真实 macOS 验证见下节；发布签名和其他设备验收仍跳过。


## 真实 macOS 验证

环境：macOS 27.0（26A5416b），Apple Silicon。Desktop 源码为
`897c9d5e5a18ca741c086b2ccc4da9b3ab6abf74`。分别构建引用上述基线和候选
`aa2eae6103e5d48e47340338829a23f2008fa607` 的独立 `uniclipd` 调试测试版本。
此次验证运行真实 daemon 和登录钥匙串，没有运行打包后的 GUI。

使用唯一 `gui-go-kek-native-*` 资料，设置 `UC_DEV_USE_PLATFORM_STORAGE=1` 并禁用系统剪贴板。
测试资料的文件密钥库条目数为零，资料 vault 使用原有密文格式。
只观察该测试服务的 `SecKeychainFindGenericPassword` 调用，保持原参数和返回值，记录次数与
`OSStatus`，不记录账户名、密码或密钥字节。测试口令只在初始化进程内生成和持有。

结果：

- 基线打开已有测试资料：2 次原生 KEK 读取，两次 `OSStatus=0`；会话与设备身份保持正常。
- 候选打开自己创建的测试资料：1 次原生 KEK 读取，`OSStatus=0`；会话与设备身份保持正常。
- 候选打开不同测试签名创建的同一资料：1 次原生 KEK 读取，`OSStatus=0`；会话与设备身份保持正常。
- 用户报告：前一阶段输入过 4 次密码；候选跨签名阶段仍输入了 2 次密码。
  前一阶段包含基线与另一份资料的初始化，不能把 4 次输入逐一归因于基线读取。
- 候选跨签名日志只有一次成功的 KEK 读取。其他同测试服务的查询均返回条目不存在。
  读取次数不能作为系统授权窗口数量的证明；两次输入的具体窗口内容仍待确认。
- 只读检查测试条目的访问控制：存在应用信任的解密 ACL（1 个受信任应用）和分区 ACL。
  此事实不足以证明两次输入分别由哪一个 ACL 触发；未修改 ACL、分区权限或用户钥匙串设置。
- 全部测试 daemon 均已退出；Desktop 源码与 Engine 依赖文件已恢复，未修改既有 UniClipboard 资料。

限制：测试二进制采用独立的临时 ad-hoc 签名，不代表 Developer ID 发布签名或 App/CLI 共享信任验收。
当前只证明真实启动 KEK 读取从两次降为一次。不得将本 PR 描述为已消除重复密码输入或钥匙串弹窗。
