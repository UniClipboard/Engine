# 单设备修改加密口令

## 目标

为已解锁且设备列表只显示有效本机的 Space 提供一次完整的用户自定义口令替换动作，不要求处于升级重配对状态；多设备明确拒绝，历史内容不重加密。

## 完整负责人

- Application 的单一用例负责资格检查、邀请处理、口令替换和最终结果。
- 调用方收集并确认用户自定义的新口令，再通过一个动作提交修改。
- Infra 隐藏现有资料钥匙重新保护、本机安全存储、配对口令记录替换与失败恢复。

## 成功与失败

- 成功：新口令可在重启后解锁并用于新邀请；旧口令失效；历史保持可读；旧邀请失效。
- 稳定失败：未解锁、不是单设备、成员状态不可确认、存在不可安全取消的配对流程、安全存储不可用或需要恢复。
- 失败或中断：不得留下本机解锁口令与配对口令不一致；下次调用由同一负责人完成或恢复同一替换。

## 阶段

- [completed] 删除系统生成与两步确认流程，改为一个用户自定义口令修改动作。
- [completed] 更新 Engine 与各平台绑定，确保输入仍保持脱敏。
- [completed] 更新测试、实施规格和架构文档。
- [completed] 执行定向测试与全量交付检查。

## 验证

- 定向 Application/Infra/Engine/绑定测试。
- 真实 keyslot、加密配对记录、重启解锁与旧口令拒绝测试。
- `cargo metadata --locked --format-version 1`
- `cargo check --workspace --all-targets --locked`
- `cargo fmt --all -- --check`
- `node scripts/architecture/check-rust-style.mjs`
- `node scripts/architecture/check-engine-repository.mjs`
- `git diff --check`
- 实体设备矩阵若当前无法执行，明确记为跳过。

## 错误记录

| 错误 | 次数 | 处理 |
| --- | --- | --- |
