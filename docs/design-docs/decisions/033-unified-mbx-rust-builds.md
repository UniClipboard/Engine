# ADR-033：Rust 构建与测试统一使用 MBX

- **状态**：已采纳，平台验收按实际执行记录
- **日期**：2026-10-07
- **取代**：[ADR-028](028-optional-mbx-build-cache.md) 的可选入口与 [ADR-029](029-ci-r2-compile-cache.md) 的活动 sccache 装配

## 决定

仓库使用固定 MBX 1.18.0 作为唯一编译动作缓存。`scripts/build-cache/mbx.sh` 负责下载完整性、编译配置与原生命令；`scripts/build-cache/bin/cargo` 保留 Cargo 命令语义，移除自己的 PATH 条目后进入原生 Cargo shim，避免递归。`just cargo` 是本地直接入口；测试、平台绑定、验收宿主、升级锚点、架构检查与性能脚本自动进入同一入口。当前 shell 需要直接执行 Cargo 时，先 source `scripts/build-cache/env.sh`，不运行全局 setup，不修改其他仓库。

常规 CI 通过 `rust-ci-setup` 将该入口加入 job PATH，涵盖 nextest、clippy、llvm-cov、cargo-ndk 及脚本内嵌套 Cargo。只缓存下载和无 R2 时 ref 隔离的本地 CAS，不恢复 target；不叠用 sccache。主线、PR、fork、手工运行全部使用 MBX，但写入权限仍由原生受保护分支 push 策略与桶级令牌共同约束。main 未保护时真实 R2 不会发布，不能伪造 CI 身份。

R2 使用现有私有桶的 `engine/mbx/v1/`，本地/PR 只读。CI 三项凭据在装配步骤写入 runner 临时目录的 0600 文件，后续只有仓库 Cargo 入口在调用时加载；GITHUB_ENV 只保存路径及非敏感配置，原始值不进入日志或第三方 action 环境。该文件和原生子进程不是同用户可信代码之间的隔离；build script 能接触本次凭据，因此 writer 环境只允许可信 main。无凭据使用本地 MBX，部分凭据或非 HTTPS 端点拒绝装配。缓存远端故障由原生 MBX 处理，实际错误统计保留。

正式 release/tag 同样通过 MBX 编译，但只复用本次 job 内的可信本地 CAS，不获取 R2 凭据，也不从 Actions 档案恢复编译产物。release 请求 `write-only`，原生 tag/手工策略关闭远端读写；job 先保留 workflow 当前版本的三个入口文件，再 checkout 不可变发布源码并记录其实际 HEAD，历史版本重验也不能退回旧 sccache 装配。目标、profile、路径重映射、签名、不可变源码和包校验保持原契约。不能把生产发布信任换成远端缓存命中承诺。

## 命中和验收语义

统一走缓存不等于每个动作都命中。首次输入必须编译；metadata、fmt、审计、工具版本查询与测试执行没有编译缓存结果。工具无法安全建模的编译仍按原生 bypass 分类，不伪造命中，不关闭必要 features 或验证。测试程序的编译产物可恢复，但测试每次实际执行，失败照常阻断。

独立空 target 的第二次构建必须以原生 `hits`、`restored_output_files` 与实际 E2E 结果证明复用；真实 R2 还须有 `downloaded_bytes`。本地 CAS 命中与 S3 回环实验不能代替真实 R2 发布/共享命中。`measure-ci-mbx.mjs` 保存每轮原生统计、命令与耗时，不清理现有 target、不缓存测试结果。

固定平台是 Linux x86_64/arm64 和 macOS arm64；其他宿主没有固定包时明确失败。移动交叉目标可经这些宿主编译，但只有实际执行过的目标才能记为通过。全部工具行为与 raw Cargo 绝对路径调用无法由仓库改变全局环境；唯一受支持调用路径见[构建指南](../local-builds.md)。

## 被放弃的方案

保留 MBX/sccache 双路线会使测试/E2E 与开发构建采用不同编译键，因此移除活动 sccache 实现及其专属静态镜像测试。R2 的权限与真实恢复以原生策略和端到端工件验收。全局 setup 会修改用户其他仓库，不采用。强制未命中时报错会使首次源码永远无法构建，不采用。
