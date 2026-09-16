# 设备关系选择修复

## 状态

2026-09-07：修复及随后授权的成员历史职责整理已完成验证。Core 39 项、Application 749 项、Engine 148 项、两条真实多设备流程及全仓交付门禁通过。
前置证据见[诊断记录](2026-09-07-device-group-choice-diagnosis.md)，最终职责见[成员历史职责](../../design-docs/membership-history-ownership.md)。下列结果均在修复和重构后的代码上执行。

## 完整责任与完成标准

- Application 的设备关系查询负责分别计算接受、保留后的成员与同步影响；Engine 只映射，Desktop 只显示。
- Core 保存与证据到达顺序无关的分支规则；Application 冲突接收、拒绝决定与恢复用例负责完整选择的保存、去重和重试。
- 用户仍通过同一个查询与选择动作完成操作，网络不会代替用户选择；已提交恢复继续使用原始签名材料。
- Desktop 的请求完成负责清理忙碌状态，名单刷新独立去重；失败保留提示。
- 完成标准：原复现测试全部转绿；新旧记录、合法新分歧、签名恢复包、真实四实例与重启通过；仓库全部交付检查通过。

## 已写入的实现

1. Application `PendingDeviceTrustChange` 增加只读的两份完整影响，选项成员取自已验证的历史结果，保持其他暂停关系；本机移除时可同步名单为空。
2. Engine 保持公开输出结构，直接映射影响；`device_group_choice` 的成员列表也从同一结果读取，删除重复计算。
3. Desktop 继续与停止名单使用这份影响，不再拿移除目标独立拼接；本机移除的旧测试输入同步为新合同。
4. Core 的新分支标识绑定 lineage、不变准入基线与已应用 head，排除旁支证据、决定和回执的增长。
5. 原快照式标识只用于精确匹配旧持久记录和旧签名恢复包；不会按设备来源猜测旧选择，也不放宽历史签名、成员或过期校验。
6. 新证据能精确匹配旧已完成的本机选择时继承结果；旧记录保留。无法与当前分支对应的未选择旧快照不再提供过期选项，等待新证据。
   旧格式未保存完整分支材料时，无法保证还原任意旧决定；不得声称所有历史重复项都已自动归并。
7. 明确拒绝的同一远端移除不再产生第二次选择，已有未选择的同源事项在拒绝提交时一并完成；真正后继成员操作仍单独识别。
8. 同一已应用分支仅已知证据不同、且活动成员相同时，证据交换确认关系一致，不伪造完整历史 ACK 水位。
   回复内容与摘要来自同一次快照；调用方使用实际关系结果，不把每次证据交换都固定报告为冲突。
9. 接收证据在提交时检查读取 revision，避免把旧证据判断应用到新状态；已保存的相反选择和恢复 intent 不被后台覆盖。
10. Desktop 选择完成复用独立刷新，再无条件结算选择结果，不依赖刷新序号决定是否清理忙碌状态。

## 验证状态

- [x] Desktop Provider 8 项通过，包括原先失败的两种并发返回顺序；不再以环境变量跳过。
- [x] Desktop 设备组件 82 项通过。
- [x] Desktop TypeScript 检查通过。
- [x] Rust 格式化可运行，说明本地源码可读；不等于类型检查或编译通过。
- [x] `cargo metadata --locked --format-version 1 --no-deps` 通过；不是完整依赖验证。
- [x] Core 历史与恢复包测试 39 项，包含拆分前固定的三份字节基线。
- [x] Application 成员测试 96 项，包括旧记录继承和相同已应用分支的额外证据；零失败、零跳过。
- [x] Engine 库测试 148 项；四实例接受、保留、重启与五实例 F2 远端分支恢复串行通过，184.69 秒。
- [x] Core 基线身份、旧签名恢复包和完整恢复流程复核；旧快照必须精确匹配，不能按设备来源继承未知选择。
- [x] 完整 metadata、workspace all-targets check、架构/隐私检查、格式与最终 diff 检查通过，保留已有无关警告。
- 跳过：真实产品页面手动流程、实体设备与发布矩阵；不把本机真实 Engine 场景计为产品实机通过。

随后按用户要求将原 2860 行 Core 文件重组为私有实现：入口 83 行，内部最长文件 430 行。
Application 的证据处理从账本提取为唯一完整 `ReconcileMembershipEvidenceUseCase`，两个调用方共用；Application 全部 749 项通过。
增加三项所有权反例检查，分别拒绝流程回到 Core、证据处理回到账本和公开内部目录，均已实际验证。

## 环境阻塞

共享 `target` 仍指向外置盘原目录，卷存在且可列目录，但读取 `.rustc_info.json` 长时间不返回。
Cargo 的进程采样停在 `open`；独立读取该 1.7 KB 文件也停住。没有通过更换 target 或清空 RUSTC_WRAPPER 绕过问题。
临时关闭 rustc 信息缓存后，Cargo 仍停在共享缓存阶段，没有得到任何编译结果；该变量只用于一次命令，不修改持久配置。

Vitest 默认扫描也停在文件系统目录读取；将扫描限制到源码目录后正常运行，得到上述 90 项通过。
系统界面只读检查不可用，工具返回发送方未认证；未操作系统权限、重置配置或强制卸载外置盘。
用户随后授予权限；复测 Engine、Desktop 两个缓存文件分别在 4 ms、3 ms 内读取完成，使用原 target 和默认共享缓存的 Cargo 已正常编译。
没有重启权限服务、重新挂载或清理硬盘。此变化支持访问授权是本次阻塞原因，不推断硬盘物理健康状况。

验证中修正：四成员测试原观察替身只返回两台设备，改为完整的离线观察；随后测试暴露初始准入基线上的拒绝决定错误读取父事件，
改从同一已验证父快照读取成员摘要，创建和验证签名决定共用正确依据。Application 96 项重新通过。

## 复验命令

Rust 命令从 Engine 根目录串行运行，复用现有 target 和共享编译缓存：

```bash
cargo test -p uc-core --test membership_history_v2 --locked
cargo test -p uc-application --lib space::membership:: --locked -- --test-threads=1
cargo test -p uc-engine --lib --locked
cargo test -p uc-engine --features dev-tools --test space_membership_auto_pairing_e2e handoff_four_device --locked -- --nocapture
cargo metadata --locked --format-version 1
cargo check --workspace --all-targets --locked
cargo fmt --all -- --check
node scripts/architecture/check-engine-repository.mjs
git diff --check
```

Desktop 根目录：

```bash
npx --yes --package=node@24 node node_modules/vitest/vitest.mjs run --dir src/contexts --maxWorkers=1 src/contexts/__tests__/DeviceTrustContext.handoff.test.tsx src/contexts/__tests__/DeviceTrustContext.test.tsx
npx --yes --package=node@24 node node_modules/vitest/vitest.mjs run --dir src/components/device --maxWorkers=1
npx --yes --package=node@24 node node_modules/typescript/bin/tsc --noEmit
```

## 保护边界

用户 a/b/c/d 配置、历史、正在运行的产品进程未修改。Desktop 原有升级适配未撤销。
没有提交、推送、发布或替换正在运行的应用。上述测试和检查通过只覆盖列出的本机范围；缺少充分材料的旧选择不自动推断，原资料保留。
