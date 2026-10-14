# ADR-033：Rust 构建与测试统一使用 MBX

- **状态**：已采纳，平台验收按实际执行记录
- **日期**：2026-10-07
- **取代**：[ADR-028](028-optional-mbx-build-cache.md) 的可选入口与 [ADR-029](029-ci-r2-compile-cache.md) 的活动 sccache 装配

## 决定

仓库使用固定 MBX 1.24.0 作为唯一编译动作缓存。`scripts/build-cache/mbx.sh` 负责下载完整性、编译配置与原生命令；`scripts/build-cache/bin/cargo` 保留 Cargo 命令语义，移除自己的 PATH 条目后进入原生 Cargo shim，避免递归。`just cargo` 是本地直接入口；测试、平台绑定、验收宿主、升级锚点、架构检查与性能脚本自动进入同一入口。当前 shell 需要直接执行 Cargo 时，先 source `scripts/build-cache/env.sh`，不运行全局 setup，不修改其他仓库。

常规 CI 通过 `rust-ci-setup` 将该入口加入 job PATH，涵盖 nextest、clippy、llvm-cov、cargo-ndk 及脚本内嵌套 Cargo。只缓存下载和无 R2 时 ref 隔离的本地 CAS，不恢复 target；不叠用 sccache。主线、PR、fork、手工运行全部使用 MBX，但写入权限仍由原生受保护分支 push 策略与桶级令牌共同约束。main 未保护时真实 R2 不会发布，不能伪造 CI 身份。

R2 使用现有私有桶的 `engine/mbx/v1/`，本地/PR 只读。CI 三项凭据在装配步骤写入 runner 临时目录的 0600 文件，后续只有仓库 Cargo 入口在调用时加载；GITHUB_ENV 只保存路径及非敏感配置，原始值不进入日志或第三方 action 环境。该文件和原生子进程不是同用户可信代码之间的隔离；build script 能接触本次凭据，因此 writer 环境只允许可信 main。无凭据使用本地 MBX，部分凭据或非 HTTPS 端点拒绝装配。缓存远端故障由原生 MBX 处理，实际错误统计保留。

正式 release/tag 同样通过 MBX 编译，但只复用本次 job 内的可信本地 CAS，不获取 R2 凭据，也不从 Actions 档案恢复编译产物。release 请求 `write-only`，原生 tag/手工策略关闭远端读写；job 先保留 workflow 当前版本的三个入口文件，再 checkout 不可变发布源码并记录其实际 HEAD，历史版本重验也不能退回旧 sccache 装配。目标、profile、路径重映射、签名、不可变源码和包校验保持原契约。不能把生产发布信任换成远端缓存命中承诺。

## 命中和验收语义

统一走缓存不等于每个动作都命中。首次输入必须编译；metadata、fmt、审计、工具版本查询与测试执行没有编译缓存结果。工具无法安全建模的编译仍按原生 bypass 分类，不伪造命中，不关闭必要 features 或验证。测试程序的编译产物可恢复，但测试每次实际执行，失败照常阻断。

独立空 target 的第二次构建必须以原生 `hits`、`restored_output_files` 与实际 E2E 结果证明复用；真实 R2 还须有 `downloaded_bytes`。本地 CAS 命中与 S3 回环实验不能代替真实 R2 发布/共享命中。`measure-ci-mbx.mjs` 保存每轮原生统计、命令与耗时，不清理现有 target、不缓存测试结果。

固定包覆盖 Linux x86_64/arm64、macOS arm64 与 Windows x64/arm64；其他宿主没有固定包时明确失败。移动交叉目标可经这些宿主编译，但只有实际执行过的目标才能记为通过。Windows 使用固定 MBX 1.24.0 ZIP 与 Git Bash 原生入口。独立手动 `.github/workflows/windows-cache-validation.yml` 在临时目录用原生 Python → cargo.exe → rustc → exe 验证冷/热与失效，保存统计和源码身份；它不接收 R2 凭据。原生双架构验收 [run 38535044908](https://github.com/UniClipboard/Engine/actions/runs/38535044908) 已通过，但 fixture 不代替产品调用链验收。

正式 Windows 路径 `.github/workflows/windows-build.yml` 已通过共享 `rust-ci-setup` 和 MBX 执行全工作区 `cargo check` 与 Windows 专属存储测试；构建结果与运行结果分步记录。[run 38535418627](https://github.com/UniClipboard/Engine/actions/runs/38535418627) 双架构通过，没有远端下载证据，不能宣称 Windows R2 加速已验收。全部工具行为与 raw Cargo 绝对路径调用无法由仓库改变全局环境；唯一受支持调用路径见[构建指南](../local-builds.md)。

## 被放弃的方案

保留 MBX/sccache 双路线会使测试/E2E 与开发构建采用不同编译键，因此移除活动 sccache 实现及其专属静态镜像测试。R2 的权限与真实恢复以原生策略和端到端工件验收。全局 setup 会修改用户其他仓库，不采用。强制未命中时报错会使首次源码永远无法构建，不采用。

## Windows 验收入口的资源边界

`mbx.sh` 用 PowerShell 查询系统架构、解压校验过的官方 ZIP，在私有工具目录复制 `cargo.exe`；不运行全局 setup，不要求 symlink 权限。PATH 同时前置固定 MBX 目录，防止原生 shim 转发到用户其他版本。Unix 的 tar 安装路径保持原实现。

在已有且获准使用的原生 Windows 非 GUI 环境，运行 `python scripts/build-cache/verify-windows-cache.py --out <证据目录>`，并提供任务独占的 `RUNNER_TEMP`。验收限制两个编译任务、1 GiB CAS，每轮使用空 target；三份 fixture 目录代表独立消费者，不代表 Desktop/Mobile 产品构建。它继承不存在的 wrapper 来确认清除冲突，并实际运行每轮 exe；feature/source/profile/RUSTFLAGS 变化必须产生正确行为。无远端验收要求上传/下载/错误均为零。

成功或失败均保留 fixture、锁文件、日志、原生统计和 scratch 所有权；失败/超时不杀共享进程、不清理目录。托管 runner 由平台销毁，其他环境须在检查任务进程结束后由所有者清理。正式 Windows workflow、下游固定 Engine 脚本版本与 R2 路径只能在原生验收后迁移；本地 ZIP/语法检查不是迁移门槛的替代品。

## 显式 linker 的版本更新

MBX 1.18 对显式 linker 下的 rlib/check 绕过，cargo-ndk 会触发；1.24 已包含上游 #694 的支持。最小真实 Android 诊断 run38556938965 在不改 release 参数的情况下验证：1.18 无宏依赖热构建0hits，1.24独立空target恢复1hit/3files。packed debug 的宿主宏链接两版仍绕过，不能删除正式debug参数换取命中。该最小结果不证明完整Android/R2或Windows1.24验收完成；上文既有Windows运行是1.18证据，新版本平台验收须独立记录。

Cargo调用参数与完整bypass理由随每次原生统计保留，宿主生成工具的命中不能充当移动目标命中。下游产品源码pin与缓存自动化pin独立；临时树覆盖缓存工具时必须记录真实dirty文件、各脚本哈希和两份来源，不将它标成完全clean源码构建。

Android目标的首次可缓存动作需要生产者填充，宿主命中不能代替。`mobile-target-cache.yml`只在真实protected-main push使用既有writer环境，编译固定Mobile产品pin的Android release库；macOS与Ubuntu分别覆盖实际消费者宿主，不打包、签名或发布。与消费者同样记录临时工具覆盖及真实脚本哈希；Mobile产品pin改变时同步预热pin，不允许手动任意源码进入writer。

1.24候选macOS R2检查出现已验证元数据路径读取ENOENT，随后报incomplete_prefetch；不能将它降为普通miss。8GiB空闲水位的分支诊断仅用于检验GC/临时文件竞争假设，保留20GiB动作预算和严格门禁，不据此单独声称根因修复。
