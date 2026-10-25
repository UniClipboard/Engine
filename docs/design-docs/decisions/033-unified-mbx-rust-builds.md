# ADR-033：Rust 构建与测试统一使用 MBX

- **状态**：已采纳，平台验收按实际执行记录
- **日期**：2026-10-07
- **取代**：[ADR-028](028-optional-mbx-build-cache.md) 的可选入口与 [ADR-029](029-ci-r2-compile-cache.md) 的活动 sccache 装配

## 决定

仓库使用固定 MBX 1.24.0 作为唯一编译动作缓存。`scripts/build-cache/mbx.sh` 负责下载完整性、编译配置与原生命令；`scripts/build-cache/bin/cargo` 保留 Cargo 命令语义，移除自己的 PATH 条目后进入原生 Cargo shim，避免递归。`just cargo` 是本地直接入口；测试、平台绑定、验收宿主、升级锚点、架构检查与性能脚本自动进入同一入口。当前 shell 需要直接执行 Cargo 时，先 source `scripts/build-cache/env.sh`，不运行全局 setup，不修改其他仓库。

### 2026-10-24 修订：CI 仅使用外层压缩 CAS 快照

常规 CI 通过 `rust-ci-setup` 将入口加入 job PATH，涵盖 nextest、clippy、llvm-cov、cargo-ndk
及脚本内嵌套 Cargo。只缓存 Cargo 下载；不恢复 target，也不保存任何 live MBX 缓存目录。
CI 编译入口清除继承的 R2/AWS 身份与原生远端设置，并显式用空 `MBX_REMOTE_URL` 覆盖机器的
全局配置；显式 `--r2` 在 CI 也只执行本地编译。无凭据、索引未命中、归档损坏或导入失败均
只允许本地冷编译，永远不回退到原生 S3 逐对象缓存。开发机的显式原生只读入口不受此次迁移影响。

完整负责人是 `scripts/build-cache/mbx-snapshot.py`：调用方指定兼容场景并请求 restore/save，
它负责传输、完整性校验、原生导入和脱敏结果。恢复失败保留状态并继续本地编译；发布失败使
发布步骤失败，下一次可信生产者重试。原生 MBX 仍负责动作输入有效性及 CAS 闭包验证。

实现复用 Desktop（`UniClipboard/UniClipboard`）提交
`742bcfdc38a270c113e5aafaf4d8ffdf8fc468b9` 的 `scripts/ci/mbx-snapshot.py`：
按 `MBX_CACHE_EXPORT_GROUP` 导出，删除 Cargo workspace attachments/objects，经隔离的原生
import 验证闭包后，只归档 export manifest、`cas` 与 `action-results`。不上传 target、workspace
附件、凭据、数据库、租约或运行状态。外层格式沿用 version 1 索引（sha256、size、key、source_sha）
和 `<sha256>.tar.gz`；先上传完整不可变归档，再更新 `latest.json`。SDK 可能对大归档使用
分段上传或重试，逻辑归档数不等于实际 HTTP 请求数。读取索引和一个归档后验证
大小、SHA-256、安全路径、常规文件类型、成员与展开大小上限，再由原生 import 校验闭包。
Desktop 未在本次修改；其现有 fallback 策略不被 Engine 继承。

Engine 使用既有私有桶下 `engine/mbx-snapshots/v1/<scene>/<compatibility-hash>/`，不读取
历史 `engine/mbx/v1/` 对象，不修改桶或凭据。键包含宿主 OS/架构、runner 镜像、真实 rustc、
Cargo 输入、固定 MBX 入口、工作流、SDK 选择及场景；场景区分目标、profile、覆盖率和测试链路。
固定 Mobile 产品还包含产品 pin。不同仓库共享格式，不共享场景命名空间或未经证明的动作兼容性。
原生 MBX 对每次编译继续验证完整输入，快照键不是编译正确性的替代物。

每个场景只由相应可信 main push/nightly job 在编译及测试成功后发布一次，不由每个消费者
post-save。nightly 升级矩阵由 `upgrade-anchors` 唯一发布：先构建全部 anchor，再编译当前
`uc-connectivity-host` 和全部 `uc-upgrade-matrix` 测试二进制；d1–d4 只读同一快照并继续实际
运行各维度测试。PR Check 的四种 connection-recovery 模式由 `direct` 唯一发布，它额外编译
relay 宿主组合以覆盖其他模式的编译图；其余模式不重复打包。nightly 网络四模式本来就
编译同一 host/relay 组合，现在共享 `engine-real-environment-network` 场景，只由 direct
发布；手动网络验收只读同一场景，不再保留永远不会执行的发布步骤。共享缓存场景不共享
证据名称：nightly artifact 保留 matrix.mode，手动证据保留 inputs.mode；矩阵 artifact
唯一性由独立于 writer 的展开检查验证，禁止多个执行实例覆盖同一不可变工件。已有 anchor-host 缓存命中时
不强制重编旧宿主，快照仍是可失效的动作种子，不承诺每个动作命中。
`check-snapshot-routing.py` 展开矩阵并按 OS/架构/场景保守检查唯一 writer，拒绝本次五个
升级 writer 及四个网络 writer 的回归；新矩阵语法必须扩展校验，不能静默跳过。
发布脚本同时检查真实 main、protected 标志、允许的事件、指定 writer 及当前源码 SHA；
workflow 继续使用限定 main 的 `engine-build-cache-writer`。PR/合并队列/其他分支只用 reader，
fork/Dependabot 无凭据时只在本地运行；HarmonyOS 的 fork 禁止在 self-hosted 执行的规则不变。
R2 桶级令牌仍是实际权限边界，不能把前缀当作独立授权。三项凭据仅注入快照传输步骤，不写入
临时配置或 `GITHUB_ENV`，也不传给编译子进程；同一个 runner 用户仍不构成敌对代码隔离。
本次不自动清理旧对象或调整保留期，存储治理需另行授权。

正式 release/tag 只复用本次 job 的可信本地 CAS，不获取 R2 凭据，也不恢复共享编译归档。
保留 workflow 当前版本的入口后再 checkout 不可变发布源码，历史重验仍维持本地编译。
目标、profile、路径重映射、签名、来源与包校验保持原契约。

### 覆盖率构建的执行口径

覆盖率作业通过[共享 CI 装配](../../../.github/actions/rust-ci-setup/action.yml)的允许名单减少编译优化开销。
`uc-infra-storage`继续采用[项目定义](../../../Cargo.toml)的开发优化口径：未优化插桩会放大完整安全材料编码、
加密索引维护及并发读取的 CPU 成本，使既有并发回归在资源竞争下超出外层期限。
验证继续执行原累积写入与读取负载、真实交错和原时序门限；构建加速选项须保持这类回归的运行口径。

## 命中和验收语义

统一走缓存不等于每个动作都命中。首次输入必须编译；metadata、fmt、审计、工具版本查询与测试执行没有编译缓存结果。工具无法安全建模的编译仍按原生 bypass 分类，不伪造命中，不关闭必要 features 或验证。测试程序的编译产物可恢复，但测试每次实际执行，失败照常阻断。

独立空 target 的第二次构建必须以原生 `hits`、`restored_output_files` 与实际 E2E 结果证明复用；真实快照恢复还须有独立传输报告的 `status=hit` 与 `downloaded_bytes>0`；
编译原生统计的上传、下载及远端错误必须全部为零。本地 CAS 命中与 S3 回环实验不能代替真实 R2 发布/共享命中。`measure-ci-mbx.mjs` 保存每轮原生统计、命令与耗时，不清理现有 target、不缓存测试结果。

固定包覆盖 Linux x86_64/arm64、macOS arm64 与 Windows x64/arm64；其他宿主没有固定包时明确失败。移动交叉目标可经这些宿主编译，但只有实际执行过的目标才能记为通过。Windows 使用固定 MBX 1.24.0 ZIP 与 Git Bash 原生入口。独立手动 `.github/workflows/windows-cache-validation.yml` 在临时目录用原生 Python → cargo.exe → rustc → exe 验证冷/热与失效，保存统计和源码身份；它不接收 R2 凭据。原生双架构验收 [run 38535044908](https://github.com/UniClipboard/Engine/actions/runs/38535044908) 已通过，但 fixture 不代替产品调用链验收。

正式 Windows 路径 `.github/workflows/windows-build.yml` 已通过共享 `rust-ci-setup` 和 MBX 执行全工作区 `cargo check` 与 Windows 专属存储测试；构建结果与运行结果分步记录。[run 38535418627](https://github.com/UniClipboard/Engine/actions/runs/38535418627) 双架构通过，没有远端下载证据，不能宣称 Windows R2 加速已验收。全部工具行为与 raw Cargo 绝对路径调用无法由仓库改变全局环境；唯一受支持调用路径见[构建指南](../local-builds.md)。

## 被放弃的方案

保留 MBX/sccache 双路线会使测试/E2E 与开发构建采用不同编译键，因此移除活动 sccache 实现及其专属静态镜像测试。快照权限以 writer 环境、发布检查与桶级令牌约束，真实恢复以端到端工件验收。全局 setup 会修改用户其他仓库，不采用。强制未命中时报错会使首次源码永远无法构建，不采用。

## Windows 验收入口的资源边界

`mbx.sh` 用 PowerShell 查询系统架构、解压校验过的官方 ZIP，在私有工具目录复制 `cargo.exe`；不运行全局 setup，不要求 symlink 权限。PATH 同时前置固定 MBX 目录，防止原生 shim 转发到用户其他版本。Unix 的 tar 安装路径保持原实现。

在已有且获准使用的原生 Windows 非 GUI 环境，运行 `python scripts/build-cache/verify-windows-cache.py --out <证据目录>`，并提供任务独占的 `RUNNER_TEMP`。验收限制两个编译任务、1 GiB CAS，每轮使用空 target；三份 fixture 目录代表独立消费者，不代表 Desktop/Mobile 产品构建。它继承不存在的 wrapper 来确认清除冲突，并实际运行每轮 exe；feature/source/profile/RUSTFLAGS 变化必须产生正确行为。无远端验收要求上传/下载/错误均为零。

成功或失败均保留 fixture、锁文件、日志、原生统计和 scratch 所有权；失败/超时不杀共享进程、不清理目录。托管 runner 由平台销毁，其他环境须在检查任务进程结束后由所有者清理。正式 Windows workflow、下游固定 Engine 脚本版本与 R2 路径只能在原生验收后迁移；本地 ZIP/语法检查不是迁移门槛的替代品。

## 显式 linker 的版本更新

MBX 1.18 对显式 linker 下的 rlib/check 绕过，cargo-ndk 会触发；1.24 已包含上游 #694 的支持。最小真实 Android 诊断 run38556938965 在不改 release 参数的情况下验证：1.18 无宏依赖热构建0hits，1.24独立空target恢复1hit/3files。packed debug 的宿主宏链接两版仍绕过，不能删除正式debug参数换取命中。该最小结果不证明完整Android/R2或Windows1.24验收完成；上文既有Windows运行是1.18证据，新版本平台验收须独立记录。

Cargo调用参数与完整bypass理由随每次原生统计保留，宿主生成工具的命中不能充当移动目标命中。下游产品源码pin与缓存自动化pin独立；临时树覆盖缓存工具时必须记录真实dirty文件、各脚本哈希和两份来源，不将它标成完全clean源码构建。

自动统计session保持`stats.json`、`cargo-args.txt`、`bypass.txt`。调用方显式指定`MBX_STATS_REPORT=producer.json`时，同目录生成`producer.cargo-args.txt`及默认`producer.bypass.txt`，避免多个阶段覆盖证据；显式`MBX_BYPASS_LOG`保持调用方路径。参数按调用前原值逐行记录，不记录环境或凭据。

Android目标的首次可缓存动作需要生产者填充，宿主命中不能代替。`mobile-target-cache.yml`只在真实protected-main push使用既有writer环境，先在本地编译固定Mobile产品pin的Android/iOS release库，再发布CAS归档；macOS与Ubuntu分别覆盖实际消费者宿主，不签名或发布产品。与消费者同样记录临时工具覆盖及真实脚本哈希；Mobile产品pin改变时同步预热pin，不允许手动任意源码进入writer。

1.24候选macOS R2检查出现已验证元数据路径读取ENOENT，随后报incomplete_prefetch；不能将它降为普通miss。8GiB空闲水位的分支诊断仅用于检验GC/临时文件竞争假设，保留20GiB动作预算和严格门禁，不据此单独声称根因修复。

## 快照验证边界

`mbx-snapshot.test.py` 覆盖冷路径、命中、损坏、附件与路径拒绝、尺寸限制、写入顺序及身份边界；
`mbx-snapshot.integration.py` 用固定 MBX 真实编译 fixture，导出/压缩/独立导入后在空 target
恢复并执行程序，再修改源码验证失效。回环端点与继承配置投毒用来确认编译不会发起对象请求。
`verify-r2-build.py` 保留旧预取错误漏计检测，并拒绝任何原生上传/下载；快照命中时额外要求本地
hits 与恢复文件。专用 R2 workflow 保留迁移 E2E 和完整 workspace check；main 发布后强制
独立消费者命中，PR 新 key 尚无归档时明确报告 miss，不冒充共享命中。存储诊断仍要求真实归档命中。
Mobile 验证继续走原产品脚本；只在临时驱动/worktree 覆盖本次缓存入口并记录源码与脚本哈希，
不修改产品 pin，不声称是完全 clean 产品树。平台 CI 和托管热缓存收益以实际运行证据为准；
本地 fixture 不能证明 macOS、Windows、移动 SDK 或完整产品构建的加速。
