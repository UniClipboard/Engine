常规 CI action 仍以 sccache + R2，专门 MBX workflow 已支持原生 R2。只固定 Linux x86_64/arm64 与 macOS arm64 MBX 包；仓库现有 Rust CI/移动 release 宿主均为这些平台。raw Cargo 的本机全局环境不得修改。native MBX 只从 AWS env 读静态 S3 凭据，必须由任务入口在调用时加载受限凭据文件，不能假设 AWS shared profile 被支持。

## CI 双入口失败与修正契约
602e7f70 CI 同时含仓库 bin 与 retained snapshot bin；脚本 prepend 仓库入口后只移除自己，native MBX 再找到另一入口，递归耗尽进程。metadata 查询不足以覆盖编译/nextest 子调用。修正必须一次移除自身与 CI 公开的保留入口，保留真正 Cargo 与 native shim；以真实最小编译/测试在两入口 PATH 下验收，再复跑远端。
