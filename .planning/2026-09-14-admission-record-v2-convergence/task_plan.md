# Task Plan: Admission Record V2 Convergence

## Goal
把本分支未发布的 Space admission 持久记录收敛为 V1、V2 两代：保留主线 V1 读取兼容，最终新增状态全部由 V2 保存，不保留 V3 及以上过渡格式。

## Completion Standard
- 当前写入只生成 V2，且 V2 能往返保存本分支全部 admission 状态与清理责任。
- 主线 V1 原始字节仍可读取，并按原有语义恢复。
- admission 记录的 V3 及以上常量、结构、编解码分支和过渡兼容测试全部删除。
- 不改变仓储加密边界、领域状态转换和公开行为。
- 定向测试、workspace check、格式、架构规则与 diff 检查通过。
- 同步架构圣经和实施记录，创建一个独立本地提交，不推送。

## Ownership
- Core 的 admission aggregate 继续拥有领域状态和版本化记录映射。
- Infra 仓储继续只负责加密保存 Core 产生的字节，不解释 aggregate 内部版本。
- 调用方仍只执行既有完整 admission 操作；成功、失败、取消和恢复结果不变。
- 重启恢复仍由既有 recovery owner 扫描 V1/V2 状态，本切片不新增恢复负责人。

## Phases
- [complete] 对照主线 V1 与当前 V2-V6，列出最终 V2 必须覆盖的状态
- [complete] 用单一 V2 替换过渡格式，删除 V3-V6 读写路径
- [complete] 更新 V1 兼容与完整 V2 往返测试
- [complete] 同步文档与架构圣经
- [complete] 运行定向测试和全仓门禁
- [complete] 审查差异并创建本地提交

## Errors Encountered

| Error | Resolution |
| --- | --- |
| 旧 V2 兼容测试仍断言正式决定后不能终止 | 删除未发布 V2 过渡兼容目标，改为验证最终 V2 版本和完整往返；新的正式决定后终止规则由现有测试继续覆盖 |
