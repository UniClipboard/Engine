# ADR-029：CI 编译缓存使用 Cloudflare R2

- **状态**：已被 [ADR-033](033-unified-mbx-rust-builds.md) 取代；下文保留历史决定（R2 凭据配置与实测见“未验证项”）
- **日期**：2026-09-28
- **范围**：经 `.github/actions/rust-ci-setup` 的 CI Rust job（`pr-check.yml`、`engine-real-environment.yml`）
  与手工基准 `compile-cache-benchmark.yml`；不改变发布工作流、本地构建与 [ADR-028](028-optional-mbx-build-cache.md) 的 mbx 入口
- **相关文件**：[`rust-ci-setup`](../../../.github/actions/rust-ci-setup/action.yml)、
  `scripts/build-cache/start-ci-sccache.sh`（历史实现，已移除）、
  `scripts/build-cache/measure-ci-sccache.mjs`（历史实现，已移除）、
  `scripts/build-cache/ci-compile-cache.test.mjs`（历史实现，已移除）

## 背景

CI 原先用 sccache 的 GitHub Actions 缓存后端（`SCCACHE_GHA_ENABLED=true`）。该后端与 rust-cache 共用每仓库
10 GiB 的 Actions 缓存配额，条目按 ref 隔离且会被频繁逐出；sccache-action 未指定版本时安装最新 sccache。
另外 `CARGO_BUILD_JOBS=default` 虽被 Cargo 接受，但 `cargo llvm-cov` 自行解析该变量，覆盖率 job 在构建前即失败。

## 决定

1. **后端。** 可信运行与只读运行使用私有 R2 桶 `uniclipboard-build-cache`（HTTPS 账户端点、`SCCACHE_REGION=auto`）。
   一个 job 只启用一个后端：R2 → GitHub Actions 缓存 → runner 本地磁盘，按顺序取第一个能启动的。
   sccache 在同时配置 S3 与 GHA 时静默只用 S3，因此由启动脚本逐个尝试，不叠加。
2. **信任边界由 R2 令牌权限与 GitHub 环境共同保证，不靠工作流条件。**
   - 写入令牌（Object Read & Write，仅作用于该桶）只存放在 GitHub 环境 `engine-build-cache-writer`，
     该环境的部署分支规则只允许 `main`。GitHub 按运行的 `GITHUB_REF` 匹配规则：PR（`refs/pull/*/merge`）、
     合并队列（`gh-readonly-queue/*`）与其他分支即使改写工作流去引用写入环境，也会被环境规则拒绝。
   - 只读令牌（Object Read only，仅作用于该桶）存放在无分支限制的 `engine-build-cache-reader`。同仓库 PR 可用它
     读取主线写入的缓存，但无法写入或覆盖条目。
   - fork PR 与 Dependabot 触发的运行拿不到任何 secret（含环境 secret），回退为 GitHub Actions 缓存；
     这些条目只对该 PR 的 ref 可见，不会被主线读取，因此不构成对可信缓存的污染。保留该后备是为了 fork PR
     重复推送时仍能复用自身产物。
   - 工作流表达式 `github.ref == 'refs/heads/main'` 只负责选择环境与读写模式；即使被改写也拿不到写入令牌。
   - 不使用 `pull_request_target`；发布工作流不接入 R2，保持“标签构建不读写跨运行 Rust 编译缓存”的发布规则。
3. **凭据只交给 sccache 服务进程。** 启动脚本以干净环境、前台模式（`SCCACHE_NO_DAEMON=1`）启动常驻服务
   （`SCCACHE_IDLE_TIMEOUT=0`）并记录其 PID，凭据不写入
   `GITHUB_ENV`，后续步骤、测试与第三方 Action 的环境中都没有凭据。这减少意外泄露面，但不是对同一 job 内
   可信代码的隔离：同一用户的进程仍可读取服务进程环境，因此写入权限的边界仍是“只有 `main` 的代码能拿到写入令牌”。
   服务的存储错误写入 runner 临时目录，只供脚本匹配、不回显；错误中可能含端点，但不含密钥。
4. **固定版本。** sccache-action 固定到提交，并通过 `version` 固定 sccache v0.18.0；启动脚本再核对各 runner
   平台二进制的 sha256（Linux x86_64、macOS arm64），不一致即失败。
5. **对象键。** 前缀为 `engine/sccache/v1/<os>-<arch>/`。sccache 自身的键已包含 rustc 版本、目标三元组、
   编译参数（含覆盖率插桩与 profile 覆盖）、源码与依赖内容、`CARGO_*` 环境与工作目录，因此不再按 job、
   toolchain 或 target 另切前缀，避免同一产物被重复存储。前缀只为按仓库与平台做容量统计和整体回收，
   不是权限隔离：桶级令牌可访问整个桶。
   `v1` 是整体失效的代际号：怀疑条目损坏或键模型变化时改为 `v2`，旧前缀由生命周期规则回收。
6. **依赖缓存。** rust-cache 继续恢复 Cargo 下载内容与第三方依赖产物（未命中时省去下载与大量依赖编译），
   工作区 crate 由 sccache 按内容缓存。基准测试关闭 rust-cache，以得到空 target 的对照。
7. **并行度。** `CARGO_BUILD_JOBS` 写为 runner 的逻辑核数（数字），同时满足 Cargo 与 `cargo llvm-cov`。
   发布工作流不使用 `cargo llvm-cov`，保留 Cargo 可接受的 `default`。

## 多仓库共用桶

`uniclipboard-build-cache` 供多个仓库共用，Engine 是第一个接入者。约定：

- **前缀按仓库划分。** 每个仓库只使用自己的顶层前缀（Engine 为 `engine/`），在其下自行组织工具与代际，
  例如 `<repo>/sccache/v1/<os>-<arch>/`；不得读写其他仓库的前缀。Engine 的启动脚本拒绝 `engine/` 以外的前缀。
- **令牌按仓库发放。** 每个仓库各有一对令牌，命名为 `uniclipboard-build-cache-<repo>-write` 与
  `uniclipboard-build-cache-<repo>-read`（Engine 为 `uniclipboard-build-cache-engine-write` / `-read`）。
  一个仓库的令牌泄露或轮换时，只需吊销该仓库那一对。
- **共用桶即共用写入信任。** R2 令牌最细只能限定到桶，不能限定到前缀，所以任一仓库的写入令牌都能改写
  所有仓库的条目。接入的每个仓库都必须满足与 Engine 相同的写入边界：写入令牌只放在部署分支限定为主线、
  禁止管理员绕过的 GitHub 环境中；PR、fork 与 Dependabot 最多拿到只读令牌。做不到这一点，或信任级别
  明显不同（例如外部贡献者多、主线无保护且写权限范围大）的仓库，应使用独立的桶，不接入本桶。
- **生命周期与容量按前缀管理。** 各仓库为自己的前缀配置过期规则，仪表盘按桶统计的用量需结合前缀判断归属。

## 失败模式与回退

| 情形 | 行为 |
|---|---|
| 无 secret（fork、Dependabot、secret 未配置） | 使用 GitHub Actions 缓存；无 Actions 缓存服务时用本地磁盘 |
| 端点格式错误 | 警告（不回显端点），使用后备缓存 |
| 端点不可达、DNS 或 TLS 失败、凭据无效 | sccache 启动读探测失败、进程退出 → 警告并使用后备缓存 |
| 启动超过 30 秒仍未就绪 | 脚本按 PID 结束自己启动的服务并确认其退出，再尝试后备；不会留下迟到完成启动、争用端口的服务 |
| 服务端口已被其他 sccache 服务占用 | 不复用配置未知的服务，setup 失败并报错，避免误报后端 |
| 写入令牌缺少写权限 | sccache 自动降为只读；脚本发出警告并标记 `r2-read` |
| 构建中 R2 读写超时或出错 | 计入 sccache 统计中的缓存错误，按未命中直接编译；写入失败只丢失该条目 |
| sccache 服务中途退出 | `SCCACHE_IGNORE_SERVER_IO_ERROR=1` 让本次编译直接调用 rustc；之后由客户端按默认配置以本地磁盘重启服务 |
| 条目损坏 | sccache 读不出的条目按未命中重编；系统性问题通过代际号整体失效 |
| 多个 job 并发写同一键 | 同一键内容由同一输入决定，后写覆盖先写，结果等价 |
| 跨平台 | 键含编译器与目标三元组，且前缀按 os-arch 分开，不会命中其他平台的产物 |
| 覆盖率插桩 | `-C instrument-coverage` 与 `uc-infra-*` profile 覆盖都进入参数哈希，与普通构建分开存储 |

sccache 只在存储读探测通过后才监听端口，脚本据此判断就绪，并再以 `--show-stats` 的缓存位置核对后端类型；
报告的后端（`UC_COMPILE_CACHE_BACKEND` 与 step summary）总是正在监听的那个服务。

## 成本与容量

R2 不收取出站流量费用，费用来自存储量与 A/B 类操作次数（以 Cloudflare 当期价格为准）。以下为按 sccache
实现推断、尚未实测的操作模式：每次缓存写入与每次查询（无论命中与否）各产生对象操作，服务启动时读、写一次
探测对象；实际操作次数与存储量以仪表盘观测为准。建议：

- 为 `engine/sccache/` 设置对象过期（例如 30 天）的生命周期规则；sccache 读取不会刷新对象时间，过期后
  下一次未命中会重新写入。
- 为 `engine/sccache-bench/` 设置 1 天过期，回收基准测试的一次性前缀。
- 在 Cloudflare 仪表盘观察桶的对象数、存储量与操作次数，必要时降低过期天数或提升代际号。

## 备选方案

- **继续只用 GitHub Actions 缓存。** 与 rust-cache 争用 10 GiB 配额，条目频繁逐出。
- **用工作流条件或 PR 标签区分写入权限。** 可被 PR 改写，不能作为安全边界。
- **所有运行共用写入令牌。** 同仓库 PR 的代码即可写入主线读取的条目。
- **把 R2 与 GitHub Actions 缓存组成多级缓存。** 增加配置与额度消耗，收益未测，暂不采用。
- **上传既有 target 目录作为缓存种子。** 来源与完整性无法证明，不作为可信缓存。
- **CI 改用 mbx。** 仍需远端缓存方案且未实测；mbx 继续只作为本地可选入口，两者不叠用。

## 配置

GitHub 仓库 `UniClipboard/Engine` → Settings → Environments：

| 环境 | 部署分支 | secret |
|---|---|---|
| `engine-build-cache-writer` | Selected branches：`main`；不允许管理员绕过 | `BUILD_CACHE_R2_ENDPOINT`、`BUILD_CACHE_R2_ACCESS_KEY_ID`、`BUILD_CACHE_R2_SECRET_ACCESS_KEY`（写入令牌） |
| `engine-build-cache-reader` | 无限制 | 同名三项（只读令牌） |

两个环境都必须在任何工作流引用它们之前创建：GitHub 会自动创建被引用但不存在的环境，且不带分支限制。
`main` 当前没有分支保护，因此写入环境不能选择“Protected branches only”（没有受保护分支时等于不限制）。
两个环境已于 2026-09-28 创建并读回核实（写入环境只允许 `main`、禁止管理员绕过），端点 secret 已写入；
访问密钥 ID 与访问密钥仍待配置，未配置期间 CI 使用 GitHub Actions 缓存后备。
R2 令牌在 Cloudflare 仪表盘 → R2 → Manage API tokens 以 Account API token 创建，名称为
`uniclipboard-build-cache-engine-write` 与 `uniclipboard-build-cache-engine-read`，权限分别为 Object Read & Write
与 Object Read only，均只作用于 `uniclipboard-build-cache`，并设置到期时间。访问密钥 ID 与访问密钥由令牌页面给出，端点为
`https://<ACCOUNT_ID>.r2.cloudflarestorage.com`。secret 只通过 GitHub 界面或 `gh secret set --env <环境>` 的交互输入写入。

## 后果与未验证项

- 回滚：把 `rust-ci-setup` 的启动步骤改回 `SCCACHE_GHA_ENABLED=true`，删除各 job 的 `environment` 与凭据输入；
  也可只删除环境中的 secret，CI 自动回退为 GitHub Actions 缓存。
- 升级 sccache 时同时更新 action 的 `version` 与脚本中的版本号和二进制 sha256，并重跑基准。
- 收益必须以 `compile-cache-benchmark.yml` 在同一提交、同一 runner 类别上的冷/暖对照为准；缓存只减少可缓存的
  rustc 编译，链接、build script、proc-macro 与测试执行不受影响，不承诺固定加速。

## MBX 共用桶补充（2026-10-07）

现有 CI sccache 路线保持本决定的格式、命名空间和服务凭据装配。新增的可选 MBX R2 流程使用
`engine/mbx/v1/`，只共用桶与 GitHub 读写环境，不共用 sccache 条目。主线 MBX 发布还要求真实受保护
分支 push；本机与 PR 为只读。完整接入与验收规则只在[本地构建指南](../local-builds.md#r2-分布式动作缓存)维护。
为 `engine/mbx/` 配置独立对象过期规则并确认容量；不得覆盖现有 sccache 生命周期规则。
