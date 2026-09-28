# 进度

- 2026-09-28：基线 ad40c041（= origin/main）。完成代码路径分析与修复设计。
- 08:42Z 资源检查：`win` 在线；`win-wsl`、`omarchy` 不可用；windows-host 租约由 t-0073 持有。
  协调者核实 `win` 正在运行 t-0089 的原生构建；构建排在 t-0089 之后，由协调者安排，本任务不启动构建。
- 用户同意把 0.19.4 作为静态快照锚点纳入矩阵，同意导出合成 profile 的钥匙串条目，快照在 `win` 上生成。
- 快照生成：在 `win` 的 `D:\uc-legacy-fixture` 下进行；官方 CLI zip 的 SHA-256 与发布一致。
  - 第 1、2 次失败：profile 推导出的端口被占用，或落在 Hyper-V 保留段（错误 10048/10013）。
  - 第 3 次：`send` 在没有对端时不写历史。
  - 第 4 次：daemon 随 SSH 会话被强制结束，资料保留在 `attempt4-data`。
  - 最终采用 oneshot + `watch` 租约，daemon 正常关闭；使用默认 profile 与测试宿主口令。
  - 入库版本由正式脚本 `scripts/testing/create-legacy-desktop-profile.ps1` 在 `final\` 目录生成：
    tar 的 SHA-256 为 `27084ef1…c53b`；integrity ok，3 条条目，库中没有明文内容。
- 子代理核实了 rebuild 期间的写入去向：`install_current_material` 会写 profile vault（只追加）。
  据此完成“Staged 目标可安全重建”的论证与崩溃边界表。
- 已编写（未编译）：Infra 测试 6 个、Application 测试 2 个、矩阵代码、宿主改动、脚本与文档。
  `cargo fmt` 与 `check-rust-style.mjs` 通过。
- 构建主机：windows-host 租约由协调者分配给 t-0090（run=native-engine-recovery），
  工作目录为 `D:\uni-workers\engine-recovery`；`CARGO_HOME` 与 target 都在该目录下，只对本进程生效。
- 红：
  - red-1：Application 1 个失败；Infra 5 个失败（错误链 `inconsistent ← database is locked`），保护性测试通过。
  - red-2-matrix：中断单元首次启动报 1103，retryable=false，升级已完成；之后仍然失败。
- interface：重建流程未持有互斥时，3 个重建测试在行为上失败。
- green-1：目标测试中，同一进程内重试失败（会话仍指向目标），已修正；矩阵两个单元通过。
- green-2（8a2816dd）：目标测试与矩阵单元全部通过。
  - 广泛检查经用户批准提速：编译并发 8、测试线程 4（原限定为并发 2），改为作业 green-2-fast 运行。
- green-2-fast：workspace check 通过。三个完整 lib 测试在 Windows 上共 25 个失败
  （Application 19、Engine 3、Infra 3）。在基线 `red` 上单线程重跑，失败集合完全相同，属于 Windows 上已存在的问题，
  不是本次回归。其中包括 `reset_space_rebuilds_…`（1312）与两个 1373。
- 虚拟机（Parallels Windows 11，x64 模拟，0.19.4 快照，不注入）：修复前宿主 10 轮失败 7 轮（1103/failed）；
  修复后宿主 10 轮全部通过。两组使用同一个测试程序。
- 最终本地检查：metadata、fmt、风格、仓库检查、diff check 均通过；本地代码 diff 与 green-2 逐字节一致。
